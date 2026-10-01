// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/11` M7's first-run flow, and the one place that knows what state the extension
//! is in.
//!
//! The app is nothing without its extension, and a GNOME Shell extension can be absent in
//! more ways than one: never installed, installed after the shell started (GNOME Shell 50
//! does not load an extension it did not find at login, `spec/10` §10), turned off, broken,
//! made for another shell version, or loaded from older code than is now on disk because
//! the shell caches modules until a logout. Each has a different remedy and only some have
//! a button. [`status`] tells them apart; the welcome window, the Settings banner, the
//! About page and the report all read it, so they cannot disagree about what is wrong.
//!
//! The bare launch -- the app grid, or `octosnap` with no arguments -- is where this
//! shows: the welcome window until it has been seen through and whenever the extension is
//! not ready, and Settings otherwise.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use octosnap_core::protocol::PROTOCOL_VERSION;
use octosnap_shell::ShellBridge;
use tracing::{info, warn};

use crate::settings;

const EXTENSIONS_NAME: &str = "org.gnome.Shell.Extensions";
const EXTENSIONS_PATH: &str = "/org/gnome/Shell/Extensions";
const EXTENSIONS_INTERFACE: &str = "org.gnome.Shell.Extensions";
/// Where the extension's own install instructions are, for a shell that cannot install
/// it from extensions.gnome.org.
pub const INSTALL_HELP: &str = "https://github.com/odrakirmusic/OctoSnap#installing";

/// GNOME Shell's `ExtensionState`, as `GetExtensionInfo` reports it (a double on the
/// wire). Only the values [`classify`] distinguishes.
mod state {
    pub const ACTIVE: f64 = 1.0;
    pub const ERROR: f64 = 3.0;
    pub const OUT_OF_DATE: f64 = 4.0;
    pub const INITIALIZED: f64 = 6.0;
    pub const ACTIVATING: f64 = 8.0;
}

/// What state the extension is in, from the user's side of it.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// Answering on the bus, speaking this build's protocol.
    Ready { version: String },
    /// Answering, but not with the code that is installed: an older protocol, or an older
    /// version than the one on disk. GNOME Shell loads extension code once per login.
    Stale { running: String, installed: Option<String>, protocol: u32 },
    /// Answering in another protocol, and a logout would load the same code again: the
    /// extension is newer than the app, or older and still so after a logout (D133). The
    /// two come from different releases, and `protocol` says which half is behind.
    Mismatched { running: String, protocol: u32 },
    /// Installed and turned on, but GNOME Shell has not loaded it, because it was installed
    /// after the shell started.
    NeedsRestart,
    /// Installed and turned off.
    Disabled,
    /// Every user extension is off: the main switch in GNOME's Extensions app.
    ExtensionsOff,
    /// GNOME Shell tried to start it and could not.
    Failed(String),
    /// Not made for this version of GNOME Shell.
    OutOfDate,
    /// Not installed, as far as GNOME Shell and this app can tell.
    Missing,
    /// The question could not be asked.
    Unknown(String),
}

impl Status {
    /// Whether everything that needs the extension will work.
    #[must_use]
    pub fn ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    /// One line for a person, in the words the welcome window and Settings use.
    #[must_use]
    pub fn headline(&self) -> String {
        match self {
            Self::Ready { version } => format!("On, version {version}"),
            Self::Stale { .. } => "Updated: log out and back in to finish".to_owned(),
            Self::Mismatched { protocol, .. } if *protocol > PROTOCOL_VERSION => {
                "Newer than this app: update OctoSnap".to_owned()
            }
            Self::Mismatched { .. } => "Older than this app: update the extension".to_owned(),
            Self::NeedsRestart => "Installed: log out and back in to start it".to_owned(),
            Self::Disabled => "Installed, but turned off".to_owned(),
            Self::ExtensionsOff => "Extensions are turned off in GNOME".to_owned(),
            Self::Failed(_) => "GNOME Shell could not start it".to_owned(),
            Self::OutOfDate => "Not made for this version of GNOME Shell".to_owned(),
            Self::Missing => "Not installed".to_owned(),
            Self::Unknown(_) => "Could not ask GNOME Shell".to_owned(),
        }
    }

    /// A sentence for Settings' banner, which has no row title beside it to lean on.
    #[must_use]
    pub fn banner(&self) -> String {
        match self {
            Self::Ready { .. } => "The OctoSnap extension is on".to_owned(),
            Self::Stale { .. } | Self::NeedsRestart => {
                "Log out and back in to finish setting up OctoSnap".to_owned()
            }
            Self::Mismatched { protocol, .. } if *protocol > PROTOCOL_VERSION => {
                "OctoSnap is older than its extension: update OctoSnap".to_owned()
            }
            Self::Mismatched { .. } => "The OctoSnap extension is older than the app".to_owned(),
            Self::Disabled => "The OctoSnap extension is turned off".to_owned(),
            Self::ExtensionsOff => "Extensions are turned off in GNOME".to_owned(),
            Self::Failed(_) => "The OctoSnap extension could not start".to_owned(),
            Self::OutOfDate => "The OctoSnap extension does not support this GNOME version".to_owned(),
            Self::Missing => "The OctoSnap extension is not installed".to_owned(),
            Self::Unknown(_) => "Could not check the OctoSnap extension".to_owned(),
        }
    }

    /// The headline and whatever detail is known, for the report and the About page.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Stale { running, installed, protocol } => format!(
                "{}. Running {running} (protocol {protocol}; this build speaks \
                 {PROTOCOL_VERSION}), installed {}.",
                self.headline(),
                installed.as_deref().unwrap_or("unknown")
            ),
            Self::Mismatched { running, protocol } => format!(
                "{}. Running {running} (protocol {protocol}; this build speaks {PROTOCOL_VERSION}).",
                self.headline()
            ),
            Self::Failed(why) | Self::Unknown(why) => format!("{}: {why}", self.headline()),
            other => format!("{}.", other.headline()),
        }
    }

    /// What the one button beside the status does, if there is one.
    #[must_use]
    pub fn remedy(&self) -> Option<Remedy> {
        match self {
            Self::Ready { .. } | Self::Unknown(_) => None,
            Self::Stale { .. } | Self::NeedsRestart | Self::Failed(_) => Some(Remedy::LogOut),
            // An app older than its extension is updated where it was installed from, which
            // is not something it can do to itself.
            Self::Mismatched { protocol, .. } if *protocol > PROTOCOL_VERSION => None,
            Self::Mismatched { .. } => Some(Remedy::Update),
            Self::Disabled => Some(Remedy::TurnOn),
            Self::ExtensionsOff => Some(Remedy::TurnOnExtensions),
            Self::OutOfDate | Self::Missing => Some(Remedy::Install),
        }
    }
}

/// The action a [`Status`] offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remedy {
    /// `InstallRemoteExtension`: GNOME Shell asks, downloads and starts it.
    Install,
    /// The same call over an extension that is already there, which fetches the newest
    /// one extensions.gnome.org has for this shell. A logout then loads it.
    Update,
    /// `EnableExtension`.
    TurnOn,
    /// GNOME's own main switch for extensions, and then this one.
    TurnOnExtensions,
    /// The session manager's logout, which asks first.
    LogOut,
}

impl Remedy {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Install => "Install\u{2026}",
            Self::Update => "Update\u{2026}",
            Self::TurnOn | Self::TurnOnExtensions => "Turn On",
            Self::LogOut => "Log Out\u{2026}",
        }
    }
}

/// What the extension said about itself, when it answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub version: String,
    pub protocol: u32,
    /// Whether this same version and protocol were already out of date in an earlier
    /// login: a logout has come and gone without loading anything newer (D133).
    pub survived_a_logout: bool,
}

/// What GNOME Shell said about the extension. `None` fields were absent from its reply.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Info {
    /// Whether the shell knows the extension at all (`GetExtensionInfo` answers `{}`
    /// for one it does not).
    pub known: bool,
    pub state: Option<f64>,
    pub enabled: Option<bool>,
    pub error: Option<String>,
    pub version_name: Option<String>,
}

/// The classification, apart from the bus, so every branch is a unit test.
///
/// `own` is the extension's answer to `Version`, `info` GNOME Shell's to
/// `GetExtensionInfo`, `extensions_on` its `UserExtensionsEnabled`, and `on_disk`
/// whether the extension's directory can be seen from here -- which a sandbox cannot,
/// so it is `false` there and a not-yet-loaded install reads as missing, and its
/// Install button hands over to GNOME Shell, which knows better.
#[must_use]
pub fn classify(own: Option<&Answer>, info: &Info, extensions_on: bool, on_disk: bool) -> Status {
    if let Some(answer) = own {
        let newer_on_disk = info
            .version_name
            .as_deref()
            .is_some_and(|installed| !installed.is_empty() && installed != answer.version);
        // Newer than the app, or older and still so after a logout: another logout would
        // load the same code again, so it is not the remedy.
        let logout_will_not_help = !newer_on_disk
            && (answer.protocol > PROTOCOL_VERSION
                || (answer.protocol < PROTOCOL_VERSION && answer.survived_a_logout));
        if logout_will_not_help {
            return Status::Mismatched { running: answer.version.clone(), protocol: answer.protocol };
        }
        if answer.protocol != PROTOCOL_VERSION || newer_on_disk {
            return Status::Stale {
                running: answer.version.clone(),
                installed: info.version_name.clone(),
                protocol: answer.protocol,
            };
        }
        return Status::Ready { version: answer.version.clone() };
    }
    if !info.known {
        return if on_disk { Status::NeedsRestart } else { Status::Missing };
    }
    match info.state {
        Some(s) if s == state::ERROR => {
            return Status::Failed(info.error.clone().unwrap_or_else(|| "no reason given".to_owned()));
        }
        Some(s) if s == state::OUT_OF_DATE => return Status::OutOfDate,
        _ => {}
    }
    if !extensions_on {
        return Status::ExtensionsOff;
    }
    match info.state {
        // Loaded and running, yet not answering: its enable() failed part way.
        Some(s) if s == state::ACTIVE => Status::Failed(
            "GNOME Shell reports it running, but it does not answer".to_owned(),
        ),
        Some(s) if s == state::INITIALIZED || s == state::ACTIVATING => Status::NeedsRestart,
        _ if info.enabled == Some(true) => Status::NeedsRestart,
        _ => Status::Disabled,
    }
}

/// Asks the extension, then GNOME Shell, and classifies.
pub async fn status(connection: &gio::DBusConnection) -> Status {
    let bridge = octosnap_shell::GnomeExtensionBridge::from_connection(connection.clone());
    let mut own = match bridge.version().await {
        Ok(v) => Some(Answer { version: v.version, protocol: v.protocol, survived_a_logout: false }),
        Err(e) if e.is_extension_missing() => None,
        Err(e) => {
            warn!("could not ask the extension its version: {e}");
            return Status::Unknown(e.to_string());
        }
    };
    let info = match extension_info(connection).await {
        Ok(info) => info,
        // The extension answered, so its own word is enough to go on.
        Err(_) if own.is_some() => Info::default(),
        Err(e) => {
            warn!("could not ask GNOME Shell about the extension: {e}");
            return Status::Unknown(e);
        }
    };
    let extensions_on = user_extensions_enabled(connection).await.unwrap_or(true);
    if let Some(answer) = own.as_mut()
        && answer.protocol < PROTOCOL_VERSION
    {
        answer.survived_a_logout = survived_a_logout(connection, answer).await;
    }
    let found = classify(own.as_ref(), &info, extensions_on, installed_on_disk());
    if found.ready() {
        settings::Settings::load().set_stale_extension("");
    }
    found
}

/// Which login this is, as far as the session bus can tell: the bus's own id and GNOME
/// Shell's name on it. A logout ends GNOME Shell, whose name a bus never gives out twice,
/// and usually ends the bus as well. `None` when the bus will not say, which a sandbox's
/// proxy may do.
async fn login(connection: &gio::DBusConnection) -> Option<String> {
    let ask = |method: &'static str, args: Option<glib::Variant>| {
        let connection = connection.clone();
        async move {
            let reply = connection
                .call_future(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus",
                    method,
                    args.as_ref(),
                    Some(glib::VariantTy::new("(s)").ok()?),
                    gio::DBusCallFlags::NONE,
                    2000,
                )
                .await
                .ok()?;
            reply.child_value(0).str().map(str::to_owned)
        }
    };
    let bus = ask("GetId", None).await?;
    let shell = ask("GetNameOwner", Some(("org.gnome.Shell",).to_variant())).await?;
    Some(format!("{bus} {shell}"))
}

/// Whether a logout has already been tried on this out-of-date extension (D133).
///
/// The first time a version and protocol are found behind the app, the login is written
/// down and the answer is no: the code on disk may well be newer, and a logout loads it.
/// Found again in another login, the code on disk was the same, and saying "log out" a
/// second time would send the user round the same loop. A bus that will not say which
/// login this is keeps the first answer.
async fn survived_a_logout(connection: &gio::DBusConnection, answer: &Answer) -> bool {
    let Some(login) = login(connection).await else { return false };
    let settings = settings::Settings::load();
    let what = format!("{}|{}|", answer.version, answer.protocol);
    if let Some(then) = settings.stale_extension().strip_prefix(&what) {
        return then != login;
    }
    settings.set_stale_extension(&format!("{what}{login}"));
    false
}

async fn extension_info(connection: &gio::DBusConnection) -> Result<Info, String> {
    let reply = connection
        .call_future(
            Some(EXTENSIONS_NAME),
            EXTENSIONS_PATH,
            EXTENSIONS_INTERFACE,
            "GetExtensionInfo",
            Some(&(settings::EXTENSION_UUID,).to_variant()),
            Some(glib::VariantTy::new("(a{sv})").map_err(|e| e.to_string())?),
            gio::DBusCallFlags::NONE,
            2000,
        )
        .await
        .map_err(|e| e.message().to_owned())?;
    let dict = glib::VariantDict::new(Some(&reply.child_value(0)));
    let known = reply.child_value(0).n_children() > 0;
    Ok(Info {
        known,
        state: dict.lookup::<f64>("state").ok().flatten(),
        enabled: dict.lookup::<bool>("enabled").ok().flatten(),
        error: dict.lookup::<String>("error").ok().flatten().filter(|e| !e.is_empty()),
        version_name: dict.lookup::<String>("version-name").ok().flatten(),
    })
}

async fn user_extensions_enabled(connection: &gio::DBusConnection) -> Option<bool> {
    let reply = connection
        .call_future(
            Some(EXTENSIONS_NAME),
            EXTENSIONS_PATH,
            "org.freedesktop.DBus.Properties",
            "Get",
            Some(&(EXTENSIONS_INTERFACE, "UserExtensionsEnabled").to_variant()),
            Some(glib::VariantTy::new("(v)").ok()?),
            gio::DBusCallFlags::NONE,
            2000,
        )
        .await
        .ok()?;
    reply.child_value(0).as_variant()?.get::<bool>()
}

/// Whether the extension's directory is where GNOME Shell looks. Always `false` in the
/// sandbox, which cannot see those directories.
fn installed_on_disk() -> bool {
    let relative = std::path::Path::new("gnome-shell").join("extensions").join(settings::EXTENSION_UUID);
    std::iter::once(glib::user_data_dir())
        .chain(glib::system_data_dirs())
        .any(|root| root.join(&relative).join("metadata.json").is_file())
}

/// Carries out `remedy`. Resolves when GNOME Shell has answered; the caller asks for the
/// [`status`] again afterwards, since that answer is not the new state.
///
/// # Errors
/// GNOME Shell's refusal, in its words.
pub async fn apply(connection: &gio::DBusConnection, remedy: Remedy) -> Result<(), String> {
    let call = |method: &'static str, params: glib::Variant, reply: &'static str| {
        let connection = connection.clone();
        async move {
            connection
                .call_future(
                    Some(EXTENSIONS_NAME),
                    EXTENSIONS_PATH,
                    EXTENSIONS_INTERFACE,
                    method,
                    Some(&params),
                    Some(glib::VariantTy::new(reply).map_err(|e| e.to_string())?),
                    gio::DBusCallFlags::NONE,
                    // Installing waits for a person to answer GNOME Shell's dialog.
                    if method == "InstallRemoteExtension" { i32::MAX } else { 5000 },
                )
                .await
                .map_err(|e| e.message().to_owned())
        }
    };
    let uuid = settings::EXTENSION_UUID;
    match remedy {
        Remedy::Install | Remedy::Update => {
            let reply = call("InstallRemoteExtension", (uuid,).to_variant(), "(s)").await?;
            let result = reply.child_value(0).str().unwrap_or_default().to_owned();
            info!(result, "install from extensions.gnome.org");
            if result == "cancelled" { Err("Cancelled".to_owned()) } else { Ok(()) }
        }
        Remedy::TurnOn => {
            let reply = call("EnableExtension", (uuid,).to_variant(), "(b)").await?;
            if reply.child_value(0).get::<bool>() == Some(true) {
                Ok(())
            } else {
                Err("GNOME Shell did not turn it on".to_owned())
            }
        }
        Remedy::TurnOnExtensions => {
            connection
                .call_future(
                    Some(EXTENSIONS_NAME),
                    EXTENSIONS_PATH,
                    "org.freedesktop.DBus.Properties",
                    "Set",
                    Some(&(EXTENSIONS_INTERFACE, "UserExtensionsEnabled", true.to_variant()).to_variant()),
                    None,
                    gio::DBusCallFlags::NONE,
                    5000,
                )
                .await
                .map_err(|e| e.message().to_owned())?;
            Box::pin(apply(connection, Remedy::TurnOn)).await
        }
        Remedy::LogOut => {
            // Mode 0 is the ordinary logout, which shows GNOME's own confirmation first.
            connection
                .call_future(
                    Some("org.gnome.SessionManager"),
                    "/org/gnome/SessionManager",
                    "org.gnome.SessionManager",
                    "Logout",
                    Some(&(0u32,).to_variant()),
                    None,
                    gio::DBusCallFlags::NONE,
                    5000,
                )
                .await
                .map_err(|e| e.message().to_owned())?;
            Ok(())
        }
    }
}

// --- The welcome window -------------------------------------------------------------

/// Asks for the status again and redraws the window with it.
type Refresh = Rc<dyn Fn()>;

thread_local! {
    static OPEN: RefCell<Option<(adw::Dialog, Refresh)>> = const { RefCell::new(None) };
}

/// What a bare launch opens: the welcome window until it has been seen through or while
/// the extension is not ready, Settings otherwise.
pub fn launched(app: &adw::Application) {
    let Some(connection) = app.dbus_connection() else {
        crate::prefs::present("");
        return;
    };
    glib::spawn_future_local(async move {
        let found = status(&connection).await;
        let welcomed = settings::Settings::load().welcomed();
        info!(?found, welcomed, "launched from the app grid");
        if welcomed && found.ready() {
            crate::prefs::present("");
        } else {
            present(&connection, found);
        }
    });
}

/// Shows the welcome window, or brings the open one forward with its status asked again.
pub fn present(connection: &gio::DBusConnection, found: Status) {
    if let Some((dialog, refresh)) = OPEN.with(|open| open.borrow().clone()) {
        refresh();
        dialog.present(None::<&gtk::Widget>);
        return;
    }
    let (dialog, refresh) = build(connection, found);
    OPEN.with(|open| *open.borrow_mut() = Some((dialog.clone(), refresh)));
    dialog.present(None::<&gtk::Widget>);
    info!("welcome window shown");
}

fn build(connection: &gio::DBusConnection, found: Status) -> (adw::Dialog, Refresh) {
    let dialog = adw::Dialog::builder()
        .title("Welcome to OctoSnap")
        .content_width(520)
        .content_height(640)
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());

    let page = adw::PreferencesPage::new();
    let hello = adw::PreferencesGroup::new();
    let heading = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_bottom(12)
        .build();
    let icon = gtk::Image::builder()
        .icon_name(octosnap_core::protocol::APP_BUS_NAME)
        .pixel_size(128)
        .build();
    icon.add_css_class("icon-dropshadow");
    heading.append(&icon);
    let title = gtk::Label::new(Some("Welcome to OctoSnap"));
    title.add_css_class("title-1");
    heading.append(&title);
    let blurb = gtk::Label::builder()
        .label("Capture any part of the screen with a shortcut, then copy it, save it, mark it up or pin it where you can see it.")
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();
    blurb.add_css_class("dim-label");
    heading.append(&blurb);
    hello.add(&heading);
    page.add(&hello);

    // The extension, which the rest of the window is worth nothing without.
    let group = adw::PreferencesGroup::builder().title("Setup").build();
    let row = adw::ActionRow::builder().title("GNOME Shell extension").build();
    let tick = gtk::Image::from_icon_name("emblem-ok-symbolic");
    tick.add_css_class("success");
    let button = gtk::Button::builder().valign(gtk::Align::Center).build();
    let spinner = adw::Spinner::builder().visible(false).build();
    row.add_suffix(&spinner);
    row.add_suffix(&tick);
    row.add_suffix(&button);
    group.add(&row);
    let help = adw::ActionRow::builder()
        .title("Installing the extension by hand")
        .subtitle(INSTALL_HELP)
        .visible(false)
        .build();
    let help_link = gtk::LinkButton::builder().uri(INSTALL_HELP).label("Open").valign(gtk::Align::Center).build();
    help.add_suffix(&help_link);
    help.set_activatable_widget(Some(&help_link));
    group.add(&help);
    group.add(&crate::prefs::launch_at_login_row());
    page.add(&group);

    // Made here, before the status is shown, because the status decides its colour.
    let done = gtk::Button::builder()
        .label("Done")
        .halign(gtk::Align::Center)
        .margin_top(12)
        .margin_bottom(24)
        .build();
    done.add_css_class("pill");

    let current = Rc::new(RefCell::new(found));
    let show = {
        // Weakly, because the button's own handler below keeps a copy of this: holding
        // the button strongly here closed a ring through that handler, and GTK disposes
        // nothing a ring still holds, so every closed dialog left these widgets behind.
        let (row, tick, button, spinner, help, done) = (
            row.downgrade(),
            tick.downgrade(),
            button.downgrade(),
            spinner.downgrade(),
            help.downgrade(),
            done.downgrade(),
        );
        move |found: &Status, note: Option<&str>| {
            let (Some(row), Some(tick), Some(button), Some(spinner), Some(help), Some(done)) = (
                row.upgrade(),
                tick.upgrade(),
                button.upgrade(),
                spinner.upgrade(),
                help.upgrade(),
                done.upgrade(),
            ) else {
                return;
            };
            spinner.set_visible(false);
            row.set_subtitle(&match (note, found) {
                (Some(note), _) => note.to_owned(),
                (None, Status::Failed(why)) => format!("{}: {why}", found.headline()),
                (None, _) => found.headline(),
            });
            tick.set_visible(found.ready());
            // One accent at a time: the remedy while there is one to press, Done after.
            // A logout is not accented, since it ends everything else that is open.
            let remedy = found.remedy();
            let accent_remedy = remedy.is_some_and(|r| r != Remedy::LogOut);
            match remedy {
                Some(remedy) => {
                    button.set_label(remedy.label());
                    button.set_visible(true);
                }
                None => button.set_visible(false),
            }
            if accent_remedy {
                button.add_css_class("suggested-action");
                done.remove_css_class("suggested-action");
            } else {
                button.remove_css_class("suggested-action");
                done.add_css_class("suggested-action");
            }
            // Wherever the shell cannot help, the manual route is one click away.
            help.set_visible(
                matches!(found, Status::Missing | Status::OutOfDate | Status::Mismatched { .. })
                    && note.is_some(),
            );
            info!(?found, accent = if accent_remedy { "remedy" } else { "done" }, "welcome window status");
        }
    };
    show(&current.borrow(), None);

    // The window follows the extension while it is open: a person who turns it on in
    // GNOME's Extensions app, or whose install finishes, sees the row change without
    // closing anything. GNOME Shell says when an extension changes state; the
    // extension's own name coming and going is what "answering" means.
    let refresh: Refresh = {
        let (connection, current, show) = (connection.clone(), Rc::clone(&current), show.clone());
        Rc::new(move || {
            let (connection, current, show) = (connection.clone(), Rc::clone(&current), show.clone());
            glib::spawn_future_local(async move {
                let found = status(&connection).await;
                if *current.borrow() != found {
                    show(&found, None);
                    *current.borrow_mut() = found;
                }
            });
        })
    };
    let state_changed = {
        let refresh = Rc::clone(&refresh);
        connection.subscribe_to_signal(
            Some(EXTENSIONS_NAME),
            Some(EXTENSIONS_INTERFACE),
            Some("ExtensionStateChanged"),
            Some(EXTENSIONS_PATH),
            Some(settings::EXTENSION_UUID),
            gio::DBusSignalFlags::NONE,
            move |_| refresh(),
        )
    };
    let watcher = {
        let (appeared, vanished) = (Rc::clone(&refresh), Rc::clone(&refresh));
        gio::bus_watch_name_on_connection(
            connection,
            octosnap_core::protocol::SHELL_BUS_NAME,
            gio::BusNameWatcherFlags::NONE,
            move |_, _, _| appeared(),
            move |_, _| vanished(),
        )
    };
    {
        let state_changed = RefCell::new(Some(state_changed));
        let watcher = std::cell::Cell::new(Some(watcher));
        dialog.connect_closed(move |_| {
            drop(state_changed.borrow_mut().take());
            if let Some(watcher) = watcher.take() {
                gio::bus_unwatch_name(watcher);
            }
            OPEN.with(|open| *open.borrow_mut() = None);
            info!("welcome window closed");
        });
    }
    {
        let (connection, current, show, spinner) =
            (connection.clone(), Rc::clone(&current), show.clone(), spinner.clone());
        button.connect_clicked(move |button| {
            let Some(remedy) = current.borrow().remedy() else { return };
            button.set_visible(false);
            spinner.set_visible(true);
            let (connection, current, show) = (connection.clone(), Rc::clone(&current), show.clone());
            glib::spawn_future_local(async move {
                let outcome = apply(&connection, remedy).await;
                if remedy == Remedy::LogOut {
                    // The session is ending or the person said no; either way the window
                    // has nothing new to say.
                    let found = current.borrow().clone();
                    show(&found, None);
                    return;
                }
                // GNOME Shell's answer is not the new state: an enabled extension GNOME
                // Shell has never loaded stays unloaded until a logout. Ask again.
                glib::timeout_future(std::time::Duration::from_millis(600)).await;
                let found = status(&connection).await;
                let note = match &outcome {
                    Err(why) if matches!(remedy, Remedy::Install | Remedy::Update) => Some(format!(
                        "Could not install from extensions.gnome.org: {why}"
                    )),
                    Err(why) => Some(why.clone()),
                    Ok(()) => None,
                };
                if let Err(why) = &outcome {
                    warn!(?remedy, "the remedy failed: {why}");
                }
                show(&found, note.as_deref());
                *current.borrow_mut() = found;
            });
        });
    }

    // The keys, which is the whole interface once setup is done: OctoSnap's own first,
    // since the Print keys' row calls them "the ones above", and that row without an
    // accent -- taking the keys is a choice, not a step of the setup.
    if let Some(ext) = settings::extension_settings() {
        let keys = adw::PreferencesGroup::builder()
            .title("Shortcuts")
            .description("Change them in Settings, under Shortcuts")
            .build();
        // The labels Settings' Shortcuts page uses, so the two windows name them alike.
        for (key, label) in [
            ("all-in-one", "All-In-One"),
            ("capture-area", "Capture Area"),
            ("capture-text", "Capture Text"),
            ("record-gif", "Record a GIF"),
        ] {
            let Some(schema) = ext.settings_schema() else { break };
            if !schema.has_key(key) {
                continue;
            }
            let accelerators = ext.strv(key);
            let row = adw::ActionRow::builder().title(label).build();
            let keys_label = adw::ShortcutLabel::new(accelerators.first().map_or("", |accel| accel.as_str()));
            // Settings' word for an unbound shortcut, and GNOME Settings' (D136).
            keys_label.set_disabled_text("Disabled");
            keys_label.set_valign(gtk::Align::Center);
            row.add_suffix(&keys_label);
            keys.add(&row);
        }
        page.add(&keys);
        page.add(&crate::prefs::print_keys_group(&ext, false));

        // The pets last, once the setup is done: one of them, and one switch. The others
        // and how they behave are Settings' Pets page.
        if ext.settings_schema().is_some_and(|schema| schema.has_key("pets-enabled")) {
            let pets = adw::PreferencesGroup::builder()
                .title("Desktop pets")
                .description("Four more, and how they behave, are in Settings, under Pets")
                .build();
            let row = adw::SwitchRow::builder()
                .title(format!("Let {} out", crate::pets::name("octopus")))
                .subtitle("OctoSnap\u{2019}s octopus, on your desktop. Drag it about, or right-click it to capture. Captures never show it")
                .build();
            row.add_prefix(&crate::pets::PetPreview::new("octopus", 2));
            ext.bind("pets-enabled", &row, "active").build();
            pets.add(&row);
            page.add(&pets);
        }
    }

    {
        // Weakly, as `show` holds its widgets: Done is inside the dialog, so the dialog
        // held here was a ring that kept the whole window (D139).
        let dialog = dialog.downgrade();
        done.connect_clicked(move |_| {
            settings::Settings::load().set_welcomed();
            info!("welcome window done");
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        });
    }
    toolbar.set_content(Some(&page));
    toolbar.add_bottom_bar(&done);
    dialog.set_child(Some(&toolbar));
    (dialog, refresh)
}

/// The welcome window as it is drawn, to a PNG: `dump-welcome`'s half, for the harness
/// (`firstrun-test.sh`), since `org.gnome.Shell.Screenshot` refuses on a real session.
#[must_use]
pub fn dump(path: &std::path::Path) -> Option<()> {
    let (dialog, _) = OPEN.with(|open| open.borrow().clone())?;
    let root = dialog.root()?;
    let renderer = root.native().and_then(|native| native.renderer())?;
    crate::prefs::render_to_png(root.upcast_ref(), &renderer, path)
}

/// Scrolls the open welcome window: 0 for the top and 1 for the foot.
fn scroll(fraction: f64) {
    let Some((dialog, _)) = OPEN.with(|open| open.borrow().clone()) else { return };
    let Some(scroller) = crate::prefs::scrolled_window(dialog.upcast_ref()) else { return };
    let adjustment = scroller.vadjustment();
    let room = adjustment.upper() - adjustment.page_size();
    adjustment.set_value(adjustment.lower() + room * fraction.clamp(0.0, 1.0));
}

/// After the startup handshake: an extension that is not the one this app needs is worth
/// a notification, because nothing on screen would otherwise say so (`spec/13` #15). Each
/// state says what it is and offers its own way out.
pub fn after_handshake(app: &adw::Application, found: &Status) {
    let (title, body, button) = match found {
        Status::Stale { .. } => (
            "Log out to finish updating OctoSnap",
            "GNOME Shell is still running the OctoSnap extension it loaded at login. \
             Captures may not work until you log out and back in.",
            ("Log Out\u{2026}", "app.log-out"),
        ),
        Status::NeedsRestart => (
            "Log out to finish setting up OctoSnap",
            "GNOME Shell starts extensions at login, and OctoSnap's was installed after \
             that. Captures need it, so log out and back in once.",
            ("Log Out\u{2026}", "app.log-out"),
        ),
        Status::Mismatched { protocol, .. } if *protocol > PROTOCOL_VERSION => (
            "OctoSnap needs an update",
            "Its GNOME Shell extension is from a newer release than the app. Captures may \
             not work until OctoSnap is updated too.",
            ("Details\u{2026}", "app.welcome"),
        ),
        Status::Mismatched { .. } => (
            "The OctoSnap extension needs an update",
            "It is from an older release than the app, and logging out did not change \
             that. Captures may not work until it is updated.",
            ("Details\u{2026}", "app.welcome"),
        ),
        _ => return,
    };
    let notification = gio::Notification::new(title);
    notification.set_body(Some(body));
    notification.add_button(button.0, button.1);
    app.send_notification(Some("extension-stale"), &notification);
    info!(?found, title, "told the user about the extension");
}

/// `app.welcome` and `app.log-out`.
pub fn register(app: &adw::Application) {
    let welcome = gio::SimpleAction::new("welcome", None);
    {
        let app = app.clone();
        welcome.connect_activate(move |_, _| {
            let Some(connection) = app.dbus_connection() else { return };
            glib::spawn_future_local(async move {
                let found = status(&connection).await;
                present(&connection, found);
            });
        });
    }
    app.add_action(&welcome);

    let log_out = gio::SimpleAction::new("log-out", None);
    {
        let app = app.clone();
        log_out.connect_activate(move |_, _| {
            let Some(connection) = app.dbus_connection() else { return };
            glib::spawn_future_local(async move {
                if let Err(why) = apply(&connection, Remedy::LogOut).await {
                    warn!("could not ask for a logout: {why}");
                }
            });
        });
    }
    app.add_action(&log_out);

    // `dump-welcome (path)`, for the harness; see [`dump`]. An optional `|FRACTION` after
    // the path scrolls the window first, as `dump-settings` takes one: the window is taller
    // than its content area, and the pets are at its foot.
    let dump_welcome = gio::SimpleAction::new("dump-welcome", Some(glib::VariantTy::STRING));
    dump_welcome.connect_activate(|_, parameter| {
        let Some(argument) = parameter.and_then(glib::Variant::str) else {
            warn!("dump-welcome needs a path");
            return;
        };
        let mut parts = argument.split('|');
        let path = parts.next().unwrap_or(argument).to_owned();
        if let Some(Ok(fraction)) = parts.next().map(str::parse::<f64>) {
            scroll(fraction);
        }
        // A frame first, for the reason `dump-settings` gives.
        glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
            match dump(std::path::Path::new(&path)) {
                Some(()) => info!(path, "welcome window dumped"),
                None => warn!(path, "the welcome window could not be rendered"),
            }
        });
    });
    app.add_action(&dump_welcome);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(version: &str, protocol: u32) -> Answer {
        Answer { version: version.to_owned(), protocol, survived_a_logout: false }
    }

    fn known(state: f64) -> Info {
        Info { known: true, state: Some(state), ..Info::default() }
    }

    #[test]
    fn an_answering_extension_of_this_protocol_is_ready() {
        let found = classify(Some(&answer("0.1.0", PROTOCOL_VERSION)), &known(state::ACTIVE), true, true);
        assert_eq!(found, Status::Ready { version: "0.1.0".to_owned() });
        assert_eq!(found.remedy(), None);
    }

    #[test]
    fn an_older_protocol_is_stale_and_wants_a_logout() {
        let found = classify(Some(&answer("0.0.9", PROTOCOL_VERSION - 1)), &Info::default(), true, true);
        assert!(matches!(found, Status::Stale { .. }));
        assert_eq!(found.remedy(), Some(Remedy::LogOut));
    }

    /// D133: the same old extension after a logout was the one on disk all along.
    #[test]
    fn an_older_protocol_after_a_logout_wants_an_update() {
        let older = Answer { survived_a_logout: true, ..answer("0.0.9", PROTOCOL_VERSION - 1) };
        let found = classify(Some(&older), &Info::default(), true, true);
        assert_eq!(found, Status::Mismatched { running: "0.0.9".to_owned(), protocol: PROTOCOL_VERSION - 1 });
        assert_eq!(found.remedy(), Some(Remedy::Update));
        assert!(found.headline().contains("update the extension"), "{}", found.headline());
    }

    /// D133: an extension newer than the app is not fixed by a logout, and not by anything
    /// the app can press either.
    #[test]
    fn a_newer_protocol_wants_the_app_updated() {
        let found = classify(Some(&answer("0.2.0", PROTOCOL_VERSION + 1)), &Info::default(), true, true);
        assert_eq!(found, Status::Mismatched { running: "0.2.0".to_owned(), protocol: PROTOCOL_VERSION + 1 });
        assert_eq!(found.remedy(), None);
        assert!(found.headline().contains("update OctoSnap"), "{}", found.headline());
    }

    /// Newer code on disk is still a logout away, whatever the running one speaks.
    #[test]
    fn newer_code_on_disk_wins_over_a_logout_already_tried() {
        let info = Info { version_name: Some("0.2.0".to_owned()), ..known(state::ACTIVE) };
        let older = Answer { survived_a_logout: true, ..answer("0.1.0", PROTOCOL_VERSION - 1) };
        assert!(matches!(classify(Some(&older), &info, true, true), Status::Stale { .. }));
    }

    #[test]
    fn newer_code_on_disk_than_running_is_stale() {
        let info = Info { version_name: Some("0.2.0".to_owned()), ..known(state::ACTIVE) };
        let found = classify(Some(&answer("0.1.0", PROTOCOL_VERSION)), &info, true, true);
        assert_eq!(
            found,
            Status::Stale { running: "0.1.0".to_owned(), installed: Some("0.2.0".to_owned()), protocol: PROTOCOL_VERSION }
        );
    }

    #[test]
    fn unknown_to_the_shell_is_missing_unless_its_files_are_there() {
        assert_eq!(classify(None, &Info::default(), true, false), Status::Missing);
        assert_eq!(classify(None, &Info::default(), true, true), Status::NeedsRestart);
        assert_eq!(Status::Missing.remedy(), Some(Remedy::Install));
    }

    #[test]
    fn a_broken_extension_says_why() {
        let info = Info { error: Some("SyntaxError".to_owned()), ..known(state::ERROR) };
        assert_eq!(classify(None, &info, true, true), Status::Failed("SyntaxError".to_owned()));
    }

    #[test]
    fn the_shell_version_mismatch_is_out_of_date() {
        assert_eq!(classify(None, &known(state::OUT_OF_DATE), true, true), Status::OutOfDate);
    }

    #[test]
    fn extensions_switched_off_is_its_own_state() {
        let found = classify(None, &known(2.0), false, true);
        assert_eq!(found, Status::ExtensionsOff);
        assert_eq!(found.remedy(), Some(Remedy::TurnOnExtensions));
    }

    #[test]
    fn off_is_disabled_and_on_but_unloaded_needs_a_logout() {
        assert_eq!(classify(None, &known(2.0), true, true), Status::Disabled);
        let on = Info { enabled: Some(true), ..known(2.0) };
        assert_eq!(classify(None, &on, true, true), Status::NeedsRestart);
        assert_eq!(classify(None, &known(state::INITIALIZED), true, true), Status::NeedsRestart);
    }

    #[test]
    fn running_but_silent_is_a_failure() {
        assert!(matches!(classify(None, &known(state::ACTIVE), true, true), Status::Failed(_)));
    }
}
