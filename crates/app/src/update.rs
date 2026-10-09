// SPDX-License-Identifier: GPL-3.0-or-later

//! Updates from inside the app (D170).
//!
//! A Flatpak installed from OctoSnap's own repository updates from it like any other, in
//! Software or with `flatpak update`. Ubuntu's App Center shows no Flatpaks, though, so the
//! app offers the update itself, through what a sandbox may ask of Flatpak's portal:
//!
//! - its update monitor says when the repository holds a newer release than the one
//!   installed, and when a newer one was installed while this one ran;
//! - `Update` installs it, once the user has said, the first time, that the app may update
//!   itself; it refuses a release that asks for more permissions than this one has;
//! - `Spawn` with the latest-version flag starts the new release, which takes the app's bus
//!   name from this one (`--gapplication-replace`), and this one quits.
//!
//! The restart waits until no window of the app is open and nothing is being recorded,
//! scrolled or written, so an update never takes a capture or an editor with it. The new
//! release then finds the extension it carries newer than the one GNOME Shell runs, and
//! writes it at the end of the session, as D161 does for any update.
//!
//! The app makes no network request for any of it: the portal looks, twice an hour. A
//! native build, and a Flatpak installed from a file that names no repository, have no
//! update to offer, and Settings shows none.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use octosnap_shell::ShellBridge as _;
use tracing::{debug, info, warn};

const PORTAL: &str = "org.freedesktop.portal.Flatpak";
const PORTAL_PATH: &str = "/org/freedesktop/portal/Flatpak";
const MONITOR: &str = "org.freedesktop.portal.Flatpak.UpdateMonitor";
/// `FLATPAK_SPAWN_FLAGS_LATEST_VERSION`: the newest installed release, not the caller's.
const SPAWN_LATEST_VERSION: u32 = 2;
/// The notification's id, so a later state replaces or withdraws it.
const NOTIFICATION: &str = "update";
/// How often an installed update looks for a moment with nothing open to restart in.
const IDLE_POLL: Duration = Duration::from_secs(5);

/// Where the app stands with updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Not a Flatpak, or no portal: nothing to offer.
    Unavailable,
    /// The portal watches, and has found nothing newer.
    Current,
    /// The repository has a newer release, the commit named.
    Available { commit: String },
    /// Being installed, with the portal's percentage.
    Installing { percent: u32 },
    /// A newer release is installed and this process is the older one.
    Installed,
    /// A newer release is installed, but ended before it took this one's place: this one
    /// runs on, and the next login starts the new release.
    Deferred,
    /// The last install did not happen.
    Failed(Failure),
}

/// Why an install did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The user said no to the app updating itself, and the portal remembers.
    NotAllowed,
    /// The release asks for permissions this one has not got, which only Software and
    /// `flatpak update` show and grant.
    NeedsPermissions,
    /// Anything else, in the portal's words.
    Other(String),
}

impl State {
    /// The Settings row's line.
    #[must_use]
    pub fn subtitle(&self) -> String {
        match self {
            Self::Unavailable => String::new(),
            Self::Current => "Flatpak looks for a newer release twice an hour".to_owned(),
            Self::Available { .. } => "A new release is ready to install".to_owned(),
            Self::Installing { percent } => format!("Installing\u{2026} {percent} %"),
            Self::Installed => "Installed. OctoSnap starts the new release once its windows are closed".to_owned(),
            Self::Deferred => "Installed. The new release starts at the next login".to_owned(),
            Self::Failed(Failure::NotAllowed) => "OctoSnap was not allowed to update itself. Software or \u{201c}flatpak update\u{201d} can, \
                 and \u{201c}flatpak permission-reset io.github.odrakirmusic.OctoSnap\u{201d} lets it ask again"
                .to_owned(),
            Self::Failed(Failure::NeedsPermissions) => "This release asks for new permissions, so it is installed from Software or with \
                 \u{201c}flatpak update\u{201d}, which show them first"
                .to_owned(),
            Self::Failed(Failure::Other(why)) => {
                format!("The update did not install: {why}. Software or \u{201c}flatpak update\u{201d} can install it too")
            }
        }
    }

    /// Whether the Update button and the top-bar item are there: something to install, or
    /// an install to try again whose refusal was not the user's or the release's.
    #[must_use]
    pub fn installable(&self) -> bool {
        matches!(self, Self::Available { .. } | Self::Failed(Failure::Other(_)))
    }
}

/// What the monitor's `UpdateAvailable` says: the commit this process runs, the newest one
/// installed, and the newest one in the repository.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Found {
    pub running: String,
    pub local: String,
    pub remote: String,
}

impl Found {
    /// From the signal's `a{sv}`.
    #[must_use]
    pub fn from_variant(info: &glib::Variant) -> Self {
        let dict = glib::VariantDict::new(Some(info));
        let read = |key: &str| dict.lookup::<String>(key).ok().flatten().unwrap_or_default();
        Self { running: read("running-commit"), local: read("local-commit"), remote: read("remote-commit") }
    }

    /// What it means. An install that already happened, by Software or `flatpak update`,
    /// comes first: the repository may be newer still, and the new release offers that.
    #[must_use]
    pub fn state(&self) -> State {
        if !self.local.is_empty() && self.local != self.running {
            State::Installed
        } else if !self.remote.is_empty() && self.remote != self.local {
            State::Available { commit: self.remote.clone() }
        } else {
            State::Current
        }
    }
}

/// What a `Progress` says: `status` 0 running, 1 nothing to do, 2 done, 3 failed.
#[must_use]
pub fn progressed(status: u32, percent: u32, error: &str, message: &str) -> State {
    match status {
        0 => State::Installing { percent: percent.min(100) },
        1 => State::Current,
        2 => State::Installed,
        _ => State::Failed(match error {
            // The portal's `request_update_permissions_sync`: the user's no, remembered.
            "org.freedesktop.DBus.Error.AccessDenied" => Failure::NotAllowed,
            // Its `transaction_ready`: "new version requires new permissions".
            "org.freedesktop.DBus.Error.NotSupported" => Failure::NeedsPermissions,
            _ if !message.is_empty() => Failure::Other(message.trim_end_matches('.').to_owned()),
            _ if !error.is_empty() => Failure::Other(error.to_owned()),
            _ => Failure::Other("the portal gave no reason".to_owned()),
        }),
    }
}

/// Whether a newly found release is one the user has not been told about.
#[must_use]
pub fn tell(found: &State, told: &str) -> bool {
    matches!(found, State::Available { commit } if commit != told)
}

type Watcher = Box<dyn Fn(&State) -> bool>;

struct Updates {
    app: adw::Application,
    state: RefCell<State>,
    monitor: RefCell<Option<String>>,
    signals: RefCell<Option<gio::SignalSubscription>>,
    watchers: RefCell<Vec<Watcher>>,
    /// The commit the last install was of, so that a refusal of it is not offered again
    /// at the next poll.
    attempted: RefCell<Option<String>>,
    restarting: Cell<bool>,
    spawn: RefCell<Spawned>,
    /// This run's marker, once it is handed to the new release (`diagnostics::hand_over`).
    handed_over: RefCell<Option<String>>,
}

/// The new release's `flatpak run`, as `Spawn` and `SpawnExited` say. Either may come
/// first: the reply's future runs after the signal's callback can.
#[derive(Default)]
struct Spawned {
    pid: Option<u32>,
    exited: Vec<(u32, u32)>,
    subscription: Option<gio::SignalSubscription>,
}

thread_local! {
    static UPDATES: RefCell<Option<Rc<Updates>>> = const { RefCell::new(None) };
}

fn updates() -> Option<Rc<Updates>> {
    UPDATES.with(|u| u.borrow().clone())
}

/// The state now, for the Settings row as it is built.
#[must_use]
pub fn state() -> State {
    updates().map_or(State::Unavailable, |u| u.state.borrow().clone())
}

/// Calls `watcher` at every change until it returns `false`, as a row that is gone does.
pub fn watch(watcher: impl Fn(&State) -> bool + 'static) {
    if let Some(u) = updates() {
        u.watchers.borrow_mut().push(Box::new(watcher));
    }
}

/// Starts watching, in a Flatpak. Called once the flow, and so the bridge, exists.
pub fn start(app: &adw::Application) {
    if !crate::settings::sandboxed() {
        debug!("not a Flatpak: updates are the package manager's");
        return;
    }
    let Some(connection) = app.dbus_connection() else { return };
    let updates = Rc::new(Updates {
        app: app.clone(),
        state: RefCell::new(State::Current),
        monitor: RefCell::default(),
        signals: RefCell::default(),
        watchers: RefCell::default(),
        attempted: RefCell::default(),
        restarting: Cell::new(false),
        spawn: RefCell::default(),
        handed_over: RefCell::default(),
    });
    UPDATES.with(|u| *u.borrow_mut() = Some(Rc::clone(&updates)));
    // An item an earlier process left in the top bar, which crashed before it said
    // otherwise, goes until there is something to offer again.
    tell_extension(false);
    glib::spawn_future_local(async move {
        if let Err(why) = ensure_monitor(&updates, &connection).await {
            warn!("no update monitor: {why}");
            updates.set(State::Unavailable);
        }
    });
}

/// Whether this run was handed to a new release, which then owns the run marker: the
/// shutdown leaves it be.
#[must_use]
pub fn handed_over() -> bool {
    updates().is_some_and(|u| u.handed_over.borrow().is_some())
}

/// `app.update`: Settings' button, the notification's and the top bar's.
pub fn register(app: &adw::Application) {
    let action = gio::SimpleAction::new("update", None);
    action.connect_activate(|_, _| install());
    app.add_action(&action);
}

/// Asks the portal to install the newest release.
pub fn install() {
    let Some(updates) = updates() else {
        info!("asked to update, in a build that cannot");
        return;
    };
    if matches!(*updates.state.borrow(), State::Installing { .. } | State::Installed) {
        return;
    }
    let Some(connection) = updates.app.dbus_connection() else { return };
    glib::spawn_future_local(async move {
        let monitor = match ensure_monitor(&updates, &connection).await {
            Ok(monitor) => monitor,
            Err(why) => return updates.set(State::Failed(Failure::Other(why))),
        };
        if let State::Available { commit } = &*updates.state.borrow() {
            *updates.attempted.borrow_mut() = Some(commit.clone());
        }
        updates.set(State::Installing { percent: 0 });
        info!("asking the portal to install the update");
        // No parent window: the dialog the portal shows the first time is its own.
        let asked = connection
            .call_future(
                Some(PORTAL),
                &monitor,
                MONITOR,
                "Update",
                Some(&update_parameters()),
                None,
                gio::DBusCallFlags::NONE,
                -1,
            )
            .await;
        if let Err(e) = asked {
            updates.set(State::Failed(Failure::Other(e.message().to_owned())));
        }
    });
}

/// The monitor's object path, made the first time it is needed. The portal keeps it for as
/// long as this connection lasts.
async fn ensure_monitor(updates: &Rc<Updates>, connection: &gio::DBusConnection) -> Result<String, String> {
    if let Some(path) = updates.monitor.borrow().clone() {
        return Ok(path);
    }
    let reply = connection
        .call_future(
            Some(PORTAL),
            PORTAL_PATH,
            PORTAL,
            "CreateUpdateMonitor",
            Some(&monitor_parameters()),
            Some(glib::VariantTy::new("(o)").map_err(|e| e.to_string())?),
            gio::DBusCallFlags::NONE,
            5000,
        )
        .await
        .map_err(|e| e.message().to_owned())?;
    let path = reply.child_value(0).str().unwrap_or_default().to_owned();
    let weak = Rc::downgrade(updates);
    let subscription = connection.subscribe_to_signal(
        Some(PORTAL),
        Some(MONITOR),
        None,
        Some(&path),
        None,
        gio::DBusSignalFlags::NONE,
        move |signal| {
            if let Some(updates) = weak.upgrade() {
                updates.heard(signal.signal_name, signal.parameters);
            }
        },
    );
    info!(monitor = %path, "watching for updates");
    *updates.signals.borrow_mut() = Some(subscription);
    *updates.monitor.borrow_mut() = Some(path.clone());
    Ok(path)
}

impl Updates {
    fn heard(self: &Rc<Self>, signal: &str, parameters: &glib::Variant) {
        let info = parameters.child_value(0);
        match signal {
            "UpdateAvailable" => {
                let found = Found::from_variant(&info);
                debug!(?found, "the portal found an update");
                let state = found.state();
                let current = self.state.borrow().clone();
                match (&state, &current) {
                    // Mid-install the next poll says nothing new; the install says how it ends.
                    // A restart that did not happen is not tried again at every poll.
                    (_, State::Installing { .. } | State::Installed | State::Deferred) => {}
                    // The release the user, or the release itself, already refused.
                    (State::Available { commit }, State::Failed(Failure::NotAllowed | Failure::NeedsPermissions))
                        if self.attempted.borrow().as_deref() == Some(commit.as_str()) => {}
                    _ => self.set(state),
                }
            }
            "Progress" => {
                let dict = glib::VariantDict::new(Some(&info));
                let number = |key: &str| dict.lookup::<u32>(key).ok().flatten().unwrap_or_default();
                let text = |key: &str| dict.lookup::<String>(key).ok().flatten().unwrap_or_default();
                let state = progressed(number("status"), number("progress"), &text("error"), &text("error_message"));
                if state == State::Current {
                    info!("the portal had nothing to install");
                }
                self.set(state);
            }
            other => debug!(signal = other, "the update monitor said something else"),
        }
    }

    fn set(self: &Rc<Self>, state: State) {
        let was = self.state.replace(state.clone());
        if was == state {
            return;
        }
        info!(?state, "update state");
        self.watchers.borrow_mut().retain(|watcher| watcher(&state));
        if was.installable() != state.installable() {
            tell_extension(state.installable());
        }
        match &state {
            State::Available { commit } => self.offer(commit),
            State::Installing { .. } => {}
            _ => self.app.withdraw_notification(NOTIFICATION),
        }
        if state == State::Installed {
            self.restart_when_idle();
        }
    }

    /// One notification a release, however many logins it stays uninstalled through.
    fn offer(&self, commit: &str) {
        let settings = crate::settings::Settings::load();
        if !tell(&State::Available { commit: commit.to_owned() }, &settings.update_offered()) {
            return;
        }
        settings.set_update_offered(commit);
        let notification = gio::Notification::new("An update to OctoSnap is ready");
        notification.set_body(Some("Install it now, or later from Settings."));
        notification.add_button("Update", "app.update");
        self.app.send_notification(Some(NOTIFICATION), &notification);
        info!("told the user about the update");
    }

    fn restart_when_idle(self: &Rc<Self>) {
        if self.restarting.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(IDLE_POLL, move || {
            let Some(updates) = weak.upgrade() else { return glib::ControlFlow::Break };
            if !idle() {
                return glib::ControlFlow::Continue;
            }
            glib::spawn_future_local(async move { updates.restart().await });
            glib::ControlFlow::Break
        });
    }

    /// Starts the newest release in this one's place. It takes the bus name, which quits
    /// this process (`connect_name_lost` in `main.rs`). A release that ends before it has
    /// leaves this one running, and is not started again: the next login starts it.
    async fn restart(self: &Rc<Self>) {
        let Some(connection) = self.app.dbus_connection() else { return };
        let exe = std::env::current_exe().map_or_else(|_| "/app/bin/octosnap-app".into(), |p| p.to_string_lossy().into_owned());
        // The new release writes where this one does. The portal also numbers the
        // environment's descriptor after the highest one it was given, which with none is
        // 0, and `flatpak run` refuses that: update-test.sh found it at the first restart.
        let stdio = gio::UnixFDList::new();
        let fds: Vec<(u32, i32)> = [
            (0, stdio.append(std::io::stdin())),
            (1, stdio.append(std::io::stdout())),
            (2, stdio.append(std::io::stderr())),
        ]
        .into_iter()
        .filter_map(|(to, handle)| handle.ok().map(|handle| (to, handle)))
        .collect();
        // The harnesses' log level goes on to the new release, when it safely can.
        let log = std::env::var("RUST_LOG").ok().filter(|_| !fds.is_empty());
        let parameters = spawn_parameters(&exe, log, &fds);
        let weak = Rc::downgrade(self);
        let subscription = connection.subscribe_to_signal(
            Some(PORTAL),
            Some(PORTAL),
            Some("SpawnExited"),
            Some(PORTAL_PATH),
            None,
            gio::DBusSignalFlags::NONE,
            move |signal| {
                let Some(updates) = weak.upgrade() else { return };
                if let Some(exit) = signal.parameters.get::<(u32, u32)>() {
                    updates.spawn.borrow_mut().exited.push(exit);
                    updates.spawn_ended();
                }
            },
        );
        self.spawn.borrow_mut().subscription = Some(subscription);
        // The new release starts while this one still runs, and would keep this run's log
        // as a crash's: update-test.sh found every restart filing one.
        *self.handed_over.borrow_mut() = Some(crate::diagnostics::hand_over(&crate::diagnostics::directory()).unwrap_or_default());
        info!("starting the new release in this one's place");
        match connection
            .call_with_unix_fd_list_future(
                Some(PORTAL),
                PORTAL_PATH,
                PORTAL,
                "Spawn",
                Some(&parameters),
                None,
                gio::DBusCallFlags::NONE,
                5000,
                Some(&stdio),
            )
            .await
        {
            Ok((reply, _)) => {
                let pid = reply.child_value(0).get::<u32>();
                info!(?pid, "the new release is starting, and takes the name from this one");
                self.spawn.borrow_mut().pid = pid;
                self.spawn_ended();
            }
            Err(e) => {
                warn!("could not start the new release: {}", e.message());
                self.take_back();
                self.set(State::Deferred);
            }
        }
    }

    /// This run is in progress again, when no new release took the marker meanwhile.
    fn take_back(&self) {
        let marker = self.handed_over.borrow().clone().unwrap_or_default();
        if crate::diagnostics::take_back(&crate::diagnostics::directory(), &marker) {
            *self.handed_over.borrow_mut() = None;
        }
    }

    /// The new release ended while this one still had the name: it did not take over.
    fn spawn_ended(self: &Rc<Self>) {
        let status = {
            let spawn = self.spawn.borrow();
            spawn.pid.and_then(|pid| spawn.exited.iter().find(|(exited, _)| *exited == pid).map(|(_, status)| *status))
        };
        if let Some(status) = status {
            warn!(status, "the new release ended before it took over; this one runs on, and the next login starts it");
            self.take_back();
            self.set(State::Deferred);
        }
    }
}

/// `CreateUpdateMonitor`'s `(a{sv})`: no options. Built as a tuple of the dictionary
/// itself: a Rust tuple holding a `Variant` sends it boxed, as `(v)`, which the portal
/// refuses.
fn monitor_parameters() -> glib::Variant {
    glib::Variant::tuple_from_iter([glib::VariantDict::new(None).end()])
}

/// `Update`'s `(sa{sv})`: no parent window, and no options.
fn update_parameters() -> glib::Variant {
    glib::Variant::tuple_from_iter(["".to_variant(), glib::VariantDict::new(None).end()])
}

/// `Spawn`'s arguments: the working directory and the command line as the portal reads
/// them, nul-terminated byte strings, the descriptors to pass as the handles they are in the
/// call's list, the log level when there is one, and the newest release rather than this one.
fn spawn_parameters(exe: &str, log: Option<String>, fds: &[(u32, i32)]) -> glib::Variant {
    let bytes = |s: &str| {
        let mut b = s.as_bytes().to_vec();
        b.push(0);
        b
    };
    let argv: Vec<Vec<u8>> = vec![bytes(exe), bytes("--gapplication-replace")];
    let env: std::collections::HashMap<String, String> =
        log.map(|level| ("RUST_LOG".to_owned(), level)).into_iter().collect();
    let fds: std::collections::HashMap<u32, glib::variant::Handle> =
        fds.iter().map(|&(to, handle)| (to, glib::variant::Handle(handle))).collect();
    glib::Variant::tuple_from_iter([
        bytes("/").to_variant(),
        argv.to_variant(),
        fds.to_variant(),
        env.to_variant(),
        SPAWN_LATEST_VERSION.to_variant(),
        glib::VariantDict::new(None).end(),
    ])
}

/// Nothing a restart would take with it: no window shown, the Settings dialog among them,
/// which has none of the app's windows for a parent, and no recording, scrolling capture or
/// GIF being written.
fn idle() -> bool {
    // The model's item type is GObject, not GtkWindow: iterating it as windows fails gio's
    // type assertion and aborts, which update-test.sh found at the first restart.
    let shown = gtk::Window::toplevels()
        .iter::<glib::Object>()
        .filter_map(Result::ok)
        .filter_map(|object| object.downcast::<gtk::Window>().ok())
        .any(|window| window.is_visible());
    let recording = crate::recorder().is_some_and(|r| r.is_recording());
    let scrolling = crate::scroller().is_some_and(|s| s.is_scrolling());
    let writing = !crate::recording::render::rendering().is_empty();
    debug!(shown, recording, scrolling, writing, "is the app idle for the restart");
    !(shown || recording || scrolling || writing)
}

/// The top bar's Update OctoSnap item. An extension from before D170 has no such method,
/// and keeps no item.
fn tell_extension(offered: bool) {
    let Some(flow) = crate::capture_flow() else { return };
    glib::spawn_future_local(async move {
        match flow.bridge().set_update_offered(offered).await {
            Ok(()) => debug!(offered, "told the extension"),
            Err(e) => debug!("the extension has no update item: {e}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(running: &str, local: &str, remote: &str) -> Found {
        Found { running: running.into(), local: local.into(), remote: remote.into() }
    }

    #[test]
    fn a_newer_commit_in_the_repository_is_available() {
        assert_eq!(found("a", "a", "b").state(), State::Available { commit: "b".into() });
    }

    #[test]
    fn the_same_commit_everywhere_is_current() {
        assert_eq!(found("a", "a", "a").state(), State::Current);
        // A remote the portal could not read says nothing.
        assert_eq!(found("a", "a", "").state(), State::Current);
    }

    #[test]
    fn a_release_installed_while_this_one_ran_comes_first() {
        // Software installed b; the repository already has c. The restart offers c.
        assert_eq!(found("a", "b", "c").state(), State::Installed);
        assert_eq!(found("a", "b", "b").state(), State::Installed);
    }

    #[test]
    fn progress_runs_then_ends() {
        assert_eq!(progressed(0, 42, "", ""), State::Installing { percent: 42 });
        assert_eq!(progressed(0, 180, "", ""), State::Installing { percent: 100 });
        assert_eq!(progressed(1, 0, "", ""), State::Current);
        assert_eq!(progressed(2, 100, "", ""), State::Installed);
    }

    #[test]
    fn the_portals_refusals_are_told_apart() {
        // As flatpak 1.16's portal sends them, seen in the D170 spike.
        assert_eq!(
            progressed(3, 0, "org.freedesktop.DBus.Error.AccessDenied", "Application update not allowed"),
            State::Failed(Failure::NotAllowed)
        );
        assert_eq!(
            progressed(
                3,
                0,
                "org.freedesktop.DBus.Error.NotSupported",
                "Self update not supported, new version requires new permissions"
            ),
            State::Failed(Failure::NeedsPermissions)
        );
        assert_eq!(
            progressed(3, 0, "org.freedesktop.DBus.Error.Failed", "No network."),
            State::Failed(Failure::Other("No network".into()))
        );
        assert_eq!(progressed(3, 0, "", ""), State::Failed(Failure::Other("the portal gave no reason".into())));
    }

    #[test]
    fn only_an_install_worth_trying_offers_the_button() {
        assert!(State::Available { commit: "b".into() }.installable());
        assert!(State::Failed(Failure::Other("x".into())).installable());
        assert!(!State::Failed(Failure::NotAllowed).installable());
        assert!(!State::Failed(Failure::NeedsPermissions).installable());
        assert!(!State::Current.installable());
        assert!(!State::Installing { percent: 3 }.installable());
        assert!(!State::Installed.installable());
        assert!(!State::Deferred.installable());
    }

    #[test]
    fn a_release_is_told_about_once() {
        let available = State::Available { commit: "b".into() };
        assert!(tell(&available, ""));
        assert!(tell(&available, "a"));
        assert!(!tell(&available, "b"));
        assert!(!tell(&State::Current, ""));
    }

    #[test]
    fn the_signal_reads_as_the_portal_sends_it() {
        let dict = glib::VariantDict::new(None);
        dict.insert("running-commit", "a");
        dict.insert("local-commit", "a");
        dict.insert("remote-commit", "b");
        assert_eq!(Found::from_variant(&dict.end()), found("a", "a", "b"));
        assert_eq!(Found::from_variant(&glib::VariantDict::new(None).end()), Found::default());
    }

    #[test]
    fn the_monitor_and_update_are_called_with_the_portals_signatures() {
        // The first build sent `(v)` for both, and the portal refused them: found by
        // update-test.sh, which no unit test had covered.
        assert_eq!(monitor_parameters().type_().as_str(), "(a{sv})");
        assert_eq!(update_parameters().type_().as_str(), "(sa{sv})");
    }

    #[test]
    fn spawn_is_called_with_the_portals_signature() {
        assert_eq!(spawn_parameters("/app/bin/octosnap-app", None, &[]).type_().as_str(), "(ayaaya{uh}a{ss}ua{sv})");
        let with_log = spawn_parameters("/app/bin/octosnap-app", Some("debug".into()), &[(0, 0), (1, 1), (2, 2)]);
        let argv: Vec<Vec<u8>> = with_log.child_value(1).get().unwrap_or_default();
        assert_eq!(argv, vec![b"/app/bin/octosnap-app\0".to_vec(), b"--gapplication-replace\0".to_vec()]);
        assert_eq!(with_log.child_value(4).get::<u32>(), Some(SPAWN_LATEST_VERSION));
        assert_eq!(with_log.child_value(3).n_children(), 1);
    }

    #[test]
    fn spawn_passes_the_standard_descriptors() {
        // With none, the portal hands `flatpak run` its environment as descriptor 0, which
        // it refuses: the first restart update-test.sh ran ended so.
        let parameters = spawn_parameters("/app/bin/octosnap-app", None, &[(0, 0), (1, 1), (2, 2)]);
        let fds: std::collections::HashMap<u32, glib::variant::Handle> = parameters.child_value(2).get().unwrap_or_default();
        let mut passed: Vec<(u32, i32)> = fds.into_iter().map(|(to, handle)| (to, handle.0)).collect();
        passed.sort_unstable();
        assert_eq!(passed, vec![(0, 0), (1, 1), (2, 2)]);
    }

    #[test]
    fn every_state_but_unavailable_has_a_line() {
        for state in [
            State::Current,
            State::Available { commit: "b".into() },
            State::Installing { percent: 1 },
            State::Installed,
            State::Deferred,
            State::Failed(Failure::NotAllowed),
            State::Failed(Failure::NeedsPermissions),
            State::Failed(Failure::Other("x".into())),
        ] {
            assert!(!state.subtitle().is_empty(), "{state:?}");
        }
        assert!(State::Unavailable.subtitle().is_empty());
    }
}
