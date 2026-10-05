// SPDX-License-Identifier: GPL-3.0-or-later

//! D161: the end of the session, from the session manager, for an update of the extension
//! that the copy GNOME Shell is running must not see.
//!
//! GNOME Shell turns the extension off at every lock and on again at the unlock, with the
//! code it imported at login and the stylesheet and schema it reads from the folder then.
//! An update that changes those waits for the session to end. The app registers with
//! gnome-session as a client, which `--talk-name=org.gnome.SessionManager` allows, and makes
//! the copy when the session manager says the session is ending: past the point where a
//! logout can still be cancelled, and before GNOME Shell ends, since the session manager
//! waits for the app's answer. The next login loads the new copy.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use tracing::{info, warn};

const NAME: &str = "org.gnome.SessionManager";
const PATH: &str = "/org/gnome/SessionManager";
const MANAGER: &str = "org.gnome.SessionManager";
const CLIENT: &str = "org.gnome.SessionManager.ClientPrivate";

/// What the session's end does: the copy, run to its end before the session manager is
/// answered.
pub type Write = Box<dyn Fn() -> Pin<Box<dyn Future<Output = ()>>>>;

/// The update the session's end writes, and its version.
type Update = (String, Rc<Write>);

/// This app's place in the session.
enum Place {
    /// Asked for, and not answered yet. A second update while the question is out waits
    /// here, and not in a second registration, which would leave the session manager a
    /// client that never answers.
    Asking(Option<Update>),
    Client {
        /// Taken when it is written.
        update: Option<Update>,
        /// Dropped, it unsubscribes.
        _signals: gio::SignalSubscription,
    },
}

thread_local! {
    static PLACE: RefCell<Option<Place>> = const { RefCell::new(None) };
}

/// The version an update waiting for the session's end will write.
#[must_use]
pub fn waiting() -> Option<String> {
    PLACE.with(|place| match place.borrow().as_ref() {
        Some(Place::Asking(update) | Place::Client { update, .. }) => update.as_ref().map(|(v, _)| v.clone()),
        None => None,
    })
}

/// Has `write` run when the session ends, for an update to `version`. Registers with the
/// session manager the first time; a later call puts its update in the place of the one
/// that was waiting.
///
/// # Errors
/// No session manager took the registration: a nested shell has none, and a sandbox can
/// have been refused the name. The session's end would then pass unseen.
pub async fn write_at_end(connection: &gio::DBusConnection, version: &str, write: Write) -> Result<(), String> {
    let update = (version.to_owned(), Rc::new(write));
    let ask = PLACE.with(|place| {
        let mut place = place.borrow_mut();
        match place.as_mut() {
            Some(Place::Asking(waiting) | Place::Client { update: waiting, .. }) => {
                *waiting = Some(update);
                false
            }
            None => {
                *place = Some(Place::Asking(Some(update)));
                true
            }
        }
    });
    if !ask {
        info!(version, "the update that waits for the session's end is now this one");
        return Ok(());
    }
    match register(connection).await {
        Ok((path, signals)) => {
            PLACE.with(|place| {
                let mut place = place.borrow_mut();
                let update = match place.take() {
                    Some(Place::Asking(update)) => update,
                    _ => None,
                };
                *place = Some(Place::Client { update, _signals: signals });
            });
            info!(version, client = path, "registered with the session manager: the update is written when the session ends");
            Ok(())
        }
        Err(why) => {
            PLACE.with(|place| *place.borrow_mut() = None);
            Err(why)
        }
    }
}

/// `RegisterClient`, and the subscription to what the session manager then says to the
/// client it made.
async fn register(connection: &gio::DBusConnection) -> Result<(String, gio::SignalSubscription), String> {
    let reply = connection
        .call_future(
            Some(NAME),
            PATH,
            MANAGER,
            "RegisterClient",
            // The app's desktop id, and no startup id: the session manager makes one.
            Some(&(octosnap_core::protocol::APP_BUS_NAME, "").to_variant()),
            Some(glib::VariantTy::new("(o)").map_err(|e| e.to_string())?),
            gio::DBusCallFlags::NONE,
            5000,
        )
        .await
        .map_err(|e| e.message().to_owned())?;
    let path = reply.child_value(0).str().unwrap_or_default().to_owned();
    if path.is_empty() {
        return Err("the session manager gave no client".to_owned());
    }
    let signals = {
        let client = path.clone();
        connection.subscribe_to_signal(
            Some(NAME),
            Some(CLIENT),
            None,
            Some(&path),
            None,
            gio::DBusSignalFlags::NONE,
            move |signal| heard(signal.connection, &client, signal.signal_name),
        )
    };
    Ok((path, signals))
}

/// The session manager's word to this client.
fn heard(connection: &gio::DBusConnection, path: &str, signal: &str) {
    let (connection, path) = (connection.clone(), path.to_owned());
    match signal {
        // A logout asked for, which can still be cancelled: nothing is written yet, and the
        // app does not hold it up.
        "QueryEndSession" => {
            info!("the session may end");
            glib::spawn_future_local(async move { respond(&connection, &path).await });
        }
        // Past the point where it can be cancelled: the copy, then the answer, which the
        // session manager waits for before it goes on.
        "EndSession" => {
            let update = PLACE.with(|place| match place.borrow_mut().as_mut() {
                Some(Place::Client { update, .. }) => update.take(),
                _ => None,
            });
            glib::spawn_future_local(async move {
                if let Some((version, write)) = update {
                    info!(version, "the session is ending: writing the update that waited for it");
                    write().await;
                }
                respond(&connection, &path).await;
            });
        }
        "CancelEndSession" => info!("the session goes on, and the update waits for its end"),
        // The last word: the session manager asks its clients to go.
        "Stop" => {
            info!("the session manager asks the app to quit");
            if let Some(app) = gio::Application::default() {
                app.quit();
            }
        }
        other => warn!(signal = other, "a word from the session manager this app does not know"),
    }
}

async fn respond(connection: &gio::DBusConnection, path: &str) {
    let answer = connection
        .call_future(
            Some(NAME),
            path,
            CLIENT,
            "EndSessionResponse",
            Some(&(true, "").to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            5000,
        )
        .await;
    match answer {
        Ok(_) => info!("answered the session manager"),
        Err(e) => warn!("could not answer the session manager: {e}"),
    }
}
