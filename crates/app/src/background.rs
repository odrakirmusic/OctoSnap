// SPDX-License-Identifier: GPL-3.0-or-later

//! "Launch at login" from a sandbox, through the Background portal (D126).
//!
//! Natively the switch is an autostart entry the app writes itself
//! (`octosnap_core::autostart`). A Flatpak's `$XDG_CONFIG_HOME` is its own
//! (`~/.var/app/<id>/config`), so an entry written there sits where no session looks, and
//! the host's autostart folder is out of reach. xdg-desktop-portal writes the host's entry
//! instead, when asked with `org.freedesktop.portal.Background.RequestBackground`, and
//! removes it when asked again with `autostart` false.
//!
//! The portal cannot be asked what it last wrote, so its answer is kept in the app's
//! `launch-at-login` key, and that is what the switch shows. The request also asks leave
//! to run in the background at all -- which a service without windows does all day -- and
//! a user who refused it once is not asked again: the answer comes back false at once,
//! and only Settings → Apps can change it.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use tracing::{debug, info, warn};

const PORTAL_NAME: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const BACKGROUND: &str = "org.freedesktop.portal.Background";
const REQUEST: &str = "org.freedesktop.portal.Request";

/// What the dialog says the request is for, if the portal shows one.
const REASON: &str = "OctoSnap starts with your session, so that the first capture is not a cold start";

/// The portal's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Answer {
    /// Allowed to run in the background.
    pub background: bool,
    /// The portal wrote (or kept) the autostart entry.
    pub autostart: bool,
}

type Done = Box<dyn FnOnce(Result<Answer, String>)>;

/// Asks the portal to turn launching at login on or off, and calls `done` with its answer.
/// `parent` is the requesting window's exported handle, or empty.
pub fn request(parent: &str, autostart: bool, done: impl FnOnce(Result<Answer, String>) + 'static) {
    let Some(connection) = gio::Application::default().and_then(|app| app.dbus_connection()) else {
        done(Err("no session bus connection".into()));
        return;
    };
    let Some(sender) = connection.unique_name() else {
        done(Err("the session bus connection has no name".into()));
        return;
    };
    // The portal puts the request at a path made of the sender and this token, so the
    // answer can be listened for before the call is made, and none is missed.
    let token = format!("octosnap{}", glib::random_int());
    let expected = request_path(&sender, &token);
    let done: Rc<RefCell<Option<Done>>> = Rc::new(RefCell::new(Some(Box::new(done))));
    let listening = listen(&connection, &expected, &done);

    let params = request_params(parent, &token, autostart);

    let connection2 = connection.clone();
    connection.call(
        Some(PORTAL_NAME),
        PORTAL_PATH,
        BACKGROUND,
        "RequestBackground",
        Some(&params),
        glib::VariantTy::new("(o)").ok(),
        gio::DBusCallFlags::NONE,
        // The dialog waits for the user; the answer comes on the request, not here.
        -1,
        gio::Cancellable::NONE,
        move |result| match result {
            Err(e) => {
                drop(listening.borrow_mut().take());
                if let Some(done) = done.borrow_mut().take() {
                    done(Err(e.message().to_owned()));
                }
            }
            Ok(reply) => {
                let Some(path) = reply.child_value(0).str().map(str::to_owned) else { return };
                if path != expected {
                    // A portal older than `handle_token`: listen where it says instead. An
                    // answer that came in between is lost, which only a portal from 2018
                    // could make happen.
                    debug!(%path, "the portal put the request somewhere else");
                    drop(listening.borrow_mut().take());
                    listen(&connection2, &path, &done);
                }
            }
        },
    );
}

/// `RequestBackground`'s `(sa{sv})`.
fn request_params(parent: &str, token: &str, autostart: bool) -> glib::Variant {
    let options = glib::VariantDict::new(None);
    options.insert("handle_token", token);
    options.insert("reason", REASON);
    options.insert("autostart", autostart);
    // The service binary, not the launcher: `octosnap` with no arguments opens a window.
    options.insert("commandline", vec!["octosnap-app"]);
    options.insert("dbus-activatable", false);
    // Not `(parent, dict).to_variant()`, which boxes the dictionary into a `v`.
    glib::Variant::tuple_from_iter([parent.to_variant(), options.end()])
}

/// Subscribes to the `Response` at `path`. The subscription keeps itself -- its handler
/// holds the slot it sits in -- until the answer comes and it is taken out, or a caller
/// takes it out first; the returned slot is for that.
fn listen(
    connection: &gio::DBusConnection,
    path: &str,
    done: &Rc<RefCell<Option<Done>>>,
) -> Rc<RefCell<Option<gio::SignalSubscription>>> {
    let slot: Rc<RefCell<Option<gio::SignalSubscription>>> = Rc::new(RefCell::new(None));
    let subscription = {
        let (slot, done) = (Rc::clone(&slot), Rc::clone(done));
        connection.subscribe_to_signal(
            Some(PORTAL_NAME),
            Some(REQUEST),
            Some("Response"),
            Some(path),
            None,
            gio::DBusSignalFlags::NONE,
            move |signal| {
                let answer = parse(signal.parameters);
                // Taken out before `done` runs, so a request made from `done` cannot find
                // this one still listening.
                let subscription = slot.borrow_mut().take();
                if let Some(done) = done.borrow_mut().take() {
                    done(answer);
                }
                drop(subscription);
            },
        )
    };
    *slot.borrow_mut() = Some(subscription);
    slot
}

/// `(u response, a{sv} results)`: 0 is an answer, 1 the dialog cancelled, 2 anything else.
fn parse(params: &glib::Variant) -> Result<Answer, String> {
    // Checked first: `child_value` and `VariantDict::new` panic on anything else.
    if params.type_().as_str() != "(ua{sv})" {
        return Err(format!("an answer of type {}", params.type_()));
    }
    let response = params.child_value(0).get::<u32>().ok_or("a response of the wrong type")?;
    if response != 0 {
        return Err(if response == 1 { "cancelled".into() } else { format!("the portal failed ({response})") });
    }
    let results = glib::VariantDict::new(Some(&params.child_value(1)));
    let flag = |key: &str| results.lookup::<bool>(key).ok().flatten().unwrap_or(false);
    Ok(Answer { background: flag("background"), autostart: flag("autostart") })
}

/// `/org/freedesktop/portal/desktop/request/<sender>/<token>`, the sender's unique name
/// without its colon and with its dots as underscores.
fn request_path(sender: &str, token: &str) -> String {
    format!("{PORTAL_PATH}/request/{}/{token}", sender.trim_start_matches(':').replace('.', "_"))
}

/// Asks, and records the answer where the switch reads it; `done` gets what is now true
/// (the last answer again, when there is no new one).
pub fn set(parent: &str, on: bool, done: impl FnOnce(Result<Answer, String>) + 'static) {
    request(parent, on, move |answer| {
        match &answer {
            Ok(answer) => remember(answer),
            Err(e) => warn!(on, "the Background portal did not answer: {e}"),
        }
        done(answer);
    });
}

/// `set-launch-at-login (b)`, for the harness (`docs/spikes/tools/flatpak-test.sh`): what
/// the switch does, without the switch.
pub fn register(app: &adw::Application) {
    let action = gio::SimpleAction::new("set-launch-at-login", Some(glib::VariantTy::BOOLEAN));
    action.connect_activate(|_, parameter| {
        let Some(on) = parameter.and_then(glib::Variant::get::<bool>) else { return };
        set("", on, |_| {});
    });
    app.add_action(&action);
}

/// Records the portal's answer where the switch reads it.
fn remember(answer: &Answer) {
    let config = crate::settings::Settings::load();
    config.set_launch_at_login(answer.autostart);
    info!(?answer, "the Background portal answered");
    if !answer.background {
        warn!("not allowed to run in the background; Settings → Apps can change that");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `Response` signal's `(ua{sv})`.
    fn response(code: u32, results: glib::Variant) -> glib::Variant {
        glib::Variant::tuple_from_iter([code.to_variant(), results])
    }

    #[test]
    fn a_request_is_a_parent_and_a_dictionary() {
        let params = request_params("", "octosnap7", true);
        assert_eq!(params.type_().as_str(), "(sa{sv})");
        let options = glib::VariantDict::new(Some(&params.child_value(1)));
        assert_eq!(options.lookup::<String>("handle_token").ok().flatten().as_deref(), Some("octosnap7"));
        assert_eq!(options.lookup::<bool>("autostart").ok().flatten(), Some(true));
        assert_eq!(options.lookup::<Vec<String>>("commandline").ok().flatten(), Some(vec!["octosnap-app".to_owned()]));
    }

    #[test]
    fn a_request_path_is_the_sender_and_the_token() {
        assert_eq!(
            request_path(":1.42", "octosnap7"),
            "/org/freedesktop/portal/desktop/request/1_42/octosnap7"
        );
    }

    #[test]
    fn an_answer_reads_both_flags() {
        let results = glib::VariantDict::new(None);
        results.insert("background", true);
        results.insert("autostart", false);
        let params = response(0, results.end());
        assert_eq!(parse(&params), Ok(Answer { background: true, autostart: false }));
    }

    #[test]
    fn a_missing_flag_is_no() {
        let params = response(0, glib::VariantDict::new(None).end());
        assert_eq!(parse(&params), Ok(Answer { background: false, autostart: false }));
    }

    #[test]
    fn a_cancelled_dialog_is_not_an_answer() {
        let params = response(1, glib::VariantDict::new(None).end());
        assert_eq!(parse(&params), Err("cancelled".to_owned()));
        let params = response(2, glib::VariantDict::new(None).end());
        assert!(parse(&params).is_err());
    }

    #[test]
    fn an_answer_of_another_shape_is_refused_not_a_panic() {
        assert!(parse(&(0u32, true).to_variant()).is_err());
        assert!(parse(&0u32.to_variant()).is_err());
    }
}
