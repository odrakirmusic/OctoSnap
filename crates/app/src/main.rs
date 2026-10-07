// SPDX-License-Identifier: GPL-3.0-or-later

//! `octosnap-app`: the GTK4/libadwaita half of OctoSnap, running as a background
//! service (`spec/10` §1).
//!
//! M0 scope (`spec/11` M0 task 3): a `gtk::Application` service that owns
//! `io.github.odrakirmusic.OctoSnap`, answers `Ping`, and shows the PNG it is handed by
//! `HandleCapture`. No overlay, no editor, no recorder -- those are the later milestones.

mod editor;
mod actions;
mod background;
mod bundled;
mod capture_window;
mod diagnostics;
mod encode;
mod flow;
mod gif_editor;
mod handshake;
mod history;
mod import;
mod notify;
mod ocr;
mod pets;
mod pin;
mod prefs;
mod qao;
mod recording;
mod remote_settings;
mod scrolling;
mod service;
mod session;
mod settings;
mod setup;

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use octosnap_core::Rect;
use octosnap_core::protocol::APP_BUS_NAME;
use octosnap_shell::{Placement, ShellBridge};
use tracing::{debug, error, info, warn};

thread_local! {
    /// `spec/10` §2: the service "keeps running with no windows (`hold()`), exits only on
    /// Quit". In gio-rs `hold()` returns a guard that releases the hold when dropped, so
    /// it has to be kept alive for the process's lifetime rather than discarded.
    static HOLD: RefCell<Option<gio::ApplicationHoldGuard>> = const { RefCell::new(None) };
}

/// The after-capture flow, built once the app has a bus connection.
///
/// Shared rather than rebuilt per capture because it carries `ACT-07`'s `{n}` counter,
/// which has to advance across captures.
pub type SharedFlow = Rc<flow::CaptureFlow<octosnap_shell::GnomeExtensionBridge>>;

thread_local! {
    static FLOW: RefCell<Option<SharedFlow>> = const { RefCell::new(None) };
}

/// The Quick Access Overlay, shared the same way and for the same reason as the flow:
/// the panel menu and the global shortcuts act on the stack that already exists.
pub type SharedQao = Rc<qao::Qao<octosnap_shell::GnomeExtensionBridge>>;

thread_local! {
    static QAO: RefCell<Option<SharedQao>> = const { RefCell::new(None) };
}

/// The flow, if the app got far enough to build one.
pub fn capture_flow() -> Option<SharedFlow> {
    FLOW.with(|f| f.borrow().clone())
}

/// The overlay, if the app got far enough to build one.
pub fn overlay() -> Option<SharedQao> {
    QAO.with(|q| q.borrow().clone())
}

/// Pinned screenshots (`spec/07` §3), shared for the same reason as the overlay.
pub type SharedPins = Rc<pin::Pins<octosnap_shell::GnomeExtensionBridge>>;

thread_local! {
    static PINS: RefCell<Option<SharedPins>> = const { RefCell::new(None) };
}

/// The pin manager, if the app got far enough to build one.
pub fn pins() -> Option<SharedPins> {
    PINS.with(|p| p.borrow().clone())
}

/// `spec/07` §4's history store, shared for the strip, the shortcuts and the CLI.
pub type SharedHistory = Rc<history::History>;

thread_local! {
    static HISTORY: RefCell<Option<SharedHistory>> = const { RefCell::new(None) };
}

/// The history store, if the app got far enough to build one.
pub fn history() -> Option<SharedHistory> {
    HISTORY.with(|h| h.borrow().clone())
}

/// `spec/06`'s recorder, shared so the panel item, the shortcut and the pill reach the
/// one recording that is running.
pub type SharedRecorder = Rc<recording::Recorder>;
pub type SharedScroller = Rc<scrolling::Scroller>;

thread_local! {
    static RECORDER: RefCell<Option<SharedRecorder>> = const { RefCell::new(None) };
    static SCROLLER: RefCell<Option<SharedScroller>> = const { RefCell::new(None) };
}

/// The recorder, if the app got far enough to build one.
pub fn recorder() -> Option<SharedRecorder> {
    RECORDER.with(|r| r.borrow().clone())
}

/// `spec/07` §1's scrolling capture, if the app got far enough to build it.
pub fn scroller() -> Option<SharedScroller> {
    SCROLLER.with(|s| s.borrow().clone())
}

fn main() -> glib::ExitCode {
    // `spec/10` §8 wants tracing in the journal. A D-Bus-activated service has its
    // stderr captured by systemd, so writing there is enough and avoids a journal
    // dependency this early. Reachable with: journalctl --user -t octosnap
    //
    // `RUST_LOG` is honoured, defaulting to info. Without the filter the `debug!` lines
    // on the card and placement paths were unreachable -- `tracing_subscriber::fmt()`
    // alone ignores the variable entirely, so setting it looked like it worked and
    // changed nothing.
    //
    // Behind a reload layer, so `spec/08` §9's debug switch can raise the level while
    // the service runs; `RUST_LOG`, when set, is the environment asking and wins.
    //
    // And to a file of the app's own (`spec/11` M7's crash log, `diagnostics.rs`), which
    // is what a report reads and what a sandboxed app can reach. The run marker is written
    // here, before anything can go wrong, and a marker the last run left behind is how
    // this one knows that run crashed.
    let log_dir = diagnostics::directory();
    let begun = diagnostics::begin(&log_dir, &diagnostics::stamp(&chrono::Local::now()));
    let log_file = begun.as_ref().ok().and_then(|_| diagnostics::LogFile::open(&log_dir).ok());
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        let from_environment = std::env::var_os("RUST_LOG").is_some();
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        let (filter, handle) = tracing_subscriber::reload::Layer::new(filter);
        tracing_subscriber::registry()
            .with(filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_target(false)
                    // Colour only for a human at a terminal. A D-Bus-activated service's
                    // stderr goes to the journal, and a redirected one goes to a file, and
                    // escape sequences in either are noise that also breaks anything
                    // grepping the output -- which is how the harnesses read what the app
                    // did.
                    .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr())),
            )
            .with(log_file.map(|file| {
                tracing_subscriber::fmt::layer()
                    .with_writer(move || file.clone())
                    .with_target(false)
                    .with_ansi(false)
            }))
            .init();
        settings::install_log_control(handle, from_environment);
    }
    diagnostics::install_panic_hook();
    let crashed = match begun {
        Ok(begun) => begun.crashed,
        Err(e) => {
            warn!(dir = %log_dir.display(), "no log file this run: {e}");
            None
        }
    };
    if let Some(crash) = &crashed {
        warn!(log = %crash.display(), "the last run did not end cleanly; its log is kept");
    }

    let app = adw::Application::builder()
        .application_id(APP_BUS_NAME)
        .flags(gio::ApplicationFlags::IS_SERVICE)
        .build();

    app.connect_startup(move |app| {
        HOLD.with(|hold| *hold.borrow_mut() = Some(app.hold()));
        register_icons();

        match service::register(app) {
            Ok(_id) => {}
            Err(e) => {
                // Without the interface the extension cannot reach us at all, so this is
                // fatal to the app's purpose even though the process could limp on.
                error!("could not export {APP_BUS_NAME}: {e}");
                app.quit();
                return;
            }
        }

        if let Some(connection) = app.dbus_connection() {
            let config = settings::Settings::load();
            if !config.is_installed() {
                warn!("running on built-in defaults; captures will still work");
            }
            if config.debug() {
                settings::apply_debug_logging(true);
            }
            {
                let config = config.clone();
                let was = std::cell::Cell::new(config.debug());
                config.clone().connect_changed(move || {
                    let now = config.debug();
                    if now != was.replace(now) {
                        settings::apply_debug_logging(now);
                    }
                });
            }

            let bridge = octosnap_shell::GnomeExtensionBridge::from_connection(connection);
            let built = Rc::new(flow::CaptureFlow::new(
                bridge,
                config.screenshot_policy(),
                config.save_config(),
            ));
            built.set_recording_policy(config.recording_policy());

            // D125: the All-In-One toolbar shows these four from the extension's copy.
            {
                let config = config.clone();
                config.clone().connect_keys_changed(settings::GIF_DEFAULT_KEYS, move || {
                    settings::sync_gif_defaults(&config);
                });
            }

            // A running service picks up the Preferences dialog without a restart.
            {
                let flow = Rc::clone(&built);
                let config = config.clone();
                config.clone().connect_changed(move || {
                    flow.update(
                        config.screenshot_policy(),
                        config.recording_policy(),
                        config.save_config(),
                    );
                });
            }

            // The Quick Access Overlay (`spec/04`). Built after the flow because it
            // needs Copy and Save, and installed *into* the flow because the flow is what
            // decides whether a capture gets a card at all.
            let pinned = pin::Pins::new(app, Rc::clone(&built));
            PINS.with(|p| *p.borrow_mut() = Some(Rc::clone(&pinned)));

            let overlay = qao::Qao::new(app, Rc::clone(&built), config.qao_config());
            {
                // `ACT-01`'s `pin`: the plan pins where it was taken, with no card behind it.
                let pins = Rc::clone(&pinned);
                built.set_pinner(Box::new(move |capture, saved_to| pins.pin_as_captured(capture, saved_to)));
            }
            overlay.set_pins(pinned);
            {
                let overlay = Rc::clone(&overlay);
                built.set_card_shower(Box::new(move |capture, already_saved| {
                    overlay.show(capture, already_saved)
                }));
            }
            {
                let overlay = Rc::clone(&overlay);
                built.set_editor_opener(Box::new(move |capture| overlay.open_editor(capture)));
            }
            {
                // D96: a read that outstays 600 ms says so, over the area it is reading.
                // A weak handle back to the flow, because unlike the three showers above
                // this closure is kept by a timer and would otherwise close the cycle.
                let app = app.clone();
                let weak = Rc::downgrade(&built);
                built.set_reading_shower(Rc::new(move |rect| {
                    debug!(?rect, "a read is taking long enough to say so");
                    let pill = ocr::pill::Pill::new(&app);
                    if let Some(flow) = weak.upgrade() {
                        place_reading_pill(flow, Rc::clone(&pill), rect);
                    }
                    Some(Box::new(move || {
                        debug!("the read landed; the indicator comes down");
                        pill.close();
                    }))
                }));
            }
            {
                let overlay = Rc::clone(&overlay);
                let config = config.clone();
                config.clone().connect_changed(move || overlay.update_config(config.qao_config()));
            }
            // `spec/07` §4's history: where closed cards go. Built after the overlay,
            // installed into it, and given every window's files so its janitor sweeps
            // the spool around them rather than through them.
            let store = history::History::open(
                history::History::default_root(),
                history::History::default_spool(),
                config.history_retention(),
                config.history_keep_saved(),
            );
            overlay.set_history(Rc::clone(&store));
            {
                let store = Rc::clone(&store);
                let config = config.clone();
                config.clone().connect_changed(move || {
                    store.set_policy(config.history_retention(), config.history_keep_saved());
                });
            }
            // Through the globals rather than captured references: the janitor first
            // runs from the idle loop, by when both are installed, and holding the
            // overlay from a closure the store owns would be a cycle.
            store.start_janitor(|| {
                let mut open: std::collections::HashSet<std::path::PathBuf> =
                    qao::open_editor_paths().into_iter().collect();
                if let Some(overlay) = crate::overlay() {
                    open.extend(overlay.open_paths());
                }
                if let Some(pins) = crate::pins() {
                    open.extend(pins.open_paths());
                }
                // A recording whose GIF is being written from its frames holds them until
                // it is done, whether or not a window still shows it (D113).
                open.extend(crate::recording::render::rendering());
                open
            });
            HISTORY.with(|h| *h.borrow_mut() = Some(store));
            QAO.with(|q| *q.borrow_mut() = Some(overlay));

            // `spec/06`'s recorder. Built last because it finalises a recording through
            // the flow (a GIF gets a card and an after-recording plan like any capture).
            let recorder = recording::Recorder::new(app, Rc::clone(&built));
            RECORDER.with(|r| *r.borrow_mut() = Some(recorder));

            // `spec/07` §1's scrolling capture, for the same reason and in the same place:
            // the stitched result goes through the flow like any other capture.
            let scroller = scrolling::Scroller::new(app, Rc::clone(&built));
            SCROLLER.with(|s| *s.borrow_mut() = Some(scroller));

            FLOW.with(|f| *f.borrow_mut() = Some(built));
        }

        actions::register(app);
        setup::register(app);
        diagnostics::register(app);
        background::register(app);
        prefs::refresh_autostart();
        handshake::run(app);
        if let Some(crash) = &crashed {
            diagnostics::offer_report(app, crash);
        }
        info!("service started, holding with no windows");
    });

    // A logout ends the session bus, and GDBus answers a closed bus by raising SIGTERM;
    // systemd stops the service the same way. Either is a clean end, and without these the
    // marker would outlive it and the next login would report a crash that never happened.
    // SIGINT is a developer's Ctrl+C in a terminal, which is not a crash either.
    for signal in [SIGHUP, SIGINT, SIGTERM] {
        let app = app.clone();
        glib_unix::unix_signal_add_local(signal, move || {
            info!(signal, "asked to stop");
            app.quit();
            glib::ControlFlow::Break
        });
    }
    {
        let log_dir = log_dir.clone();
        app.connect_shutdown(move |_| {
            info!("service stopping");
            diagnostics::end(&log_dir);
        });
    }

    // Captures arrive as actions and never activate the app, so an activation is a person
    // opening it: from the app grid, or `octosnap` with no arguments. `spec/09` §1's
    // "invoke, act, vanish" is about captures; a launch asks for a window, and gets the
    // welcome window or Settings (`setup.rs`).
    app.connect_activate(|app| {
        info!("activated");
        setup::launched(app);
    });

    app.run()
}

/// The three signals that ask a process to stop. Linux's numbers, which are fixed by the
/// kernel ABI on every architecture this app is built for.
const SIGHUP: i32 = 1;
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;

/// Puts D96's reading indicator where the read is happening.
///
/// Off in its own task because `PlaceWindow` is a D-Bus round trip: an indicator a few
/// milliseconds late is still an indicator, while a read that stalled to position one
/// would be a worse bug than the silence it was added to fix.
fn place_reading_pill(flow: SharedFlow, pill: Rc<ocr::pill::Pill>, rect: Option<Rect>) {
    glib::spawn_future_local(async move {
        // Every failure below leaves the pill where GTK mapped it, which is still on a
        // screen and still says what it has to say. Nothing here is worth losing it over.
        let Some(path) = pill.object_path() else {
            warn!("the reading indicator has no object path; leaving it where it was mapped");
            return;
        };
        let size = pill.size();
        let (x, y) = match rect {
            Some(rect) => ocr::pill::centred(rect, size),
            None => {
                let monitors = flow.bridge().monitors().await.unwrap_or_default();
                let seen = monitors.iter().find(|monitor| monitor.current);
                let Some(monitor) = seen.or_else(|| monitors.first()) else {
                    warn!("no monitors to place the reading indicator on");
                    return;
                };
                ocr::pill::on_a_monitor(monitor.work_area, size)
            }
        };
        match flow.bridge().place_window(&path, "reader", &Placement::At { x, y }, 0).await {
            Ok(landed) => debug!(?landed, "the reading indicator is on screen"),
            Err(e) => warn!("could not place the reading indicator: {e}"),
        }
    });
}

/// Makes the application's own icons findable by name.
///
/// `spec/05` §2's tool strip needs a line, a rectangle, an ellipse, a crop frame and a
/// highlighter nib, and Adwaita ships none of them: the first version of the strip named
/// plausible-sounding icons and drew six broken-image glyphs. `resources/` holds the gaps
/// and `Tool::icon` names them.
///
/// Registered on startup rather than when the first editor opens, because an icon theme
/// that gains a search path after a widget has already asked for an icon keeps the
/// missing-image it cached.
fn register_icons() {
    gio::resources_register_include!("octosnap.gresource")
        .unwrap_or_else(|e| warn!("could not register icon resources: {e}"));
    let Some(display) = gtk::gdk::Display::default() else {
        // No display: a service started before a session is up. The editor cannot open
        // yet either, so there is nothing to warn about.
        return;
    };
    let theme = gtk::IconTheme::for_display(&display);
    theme.add_resource_path("/io/github/odrakirmusic/OctoSnap/icons");

    // Every tool icon, checked by name at startup.
    //
    // A missing icon is silent: GTK substitutes `image-missing` and draws it, so the
    // editor showed six broken glyphs and said nothing. The names were plausible ones
    // Adwaita does not have. `has_icon` turns that into a line in the log, which is a
    // thing a harness can assert on -- `editor-test.sh` does -- and which no unit test
    // could, because resolving a name needs a display.
    let names: Vec<&str> = octosnap_scene::tool::Tool::ALL
        .iter()
        .map(|tool| tool.icon())
        .chain(octosnap_scene::ArrowStyle::ALL.iter().map(|style| style.icon()))
        .chain(octosnap_scene::RedactStyle::ALL.iter().map(|style| style.icon()))
        .chain(octosnap_core::ScrollDirection::ALL.iter().map(|&way| scrolling::controls::icon_for(way)))
        .collect();
    let missing: Vec<&str> = names.iter().copied().filter(|name| !theme.has_icon(name)).collect();
    if missing.is_empty() {
        info!(count = names.len(), "every tool icon resolves");
    } else {
        warn!(missing = missing.join(", "), "tool icons the theme cannot resolve");
    }
}
