// SPDX-License-Identifier: GPL-3.0-or-later

//! GApplication actions (`spec/10` §3.2).
//!
//! These exist for a reason M0 discovered the hard way, written up in
//! `docs/spikes/13-cold-activation-race.md`: a call to the app's own
//! `io.github.odrakirmusic.OctoSnap.App1` interface is **lost** when it is the call that
//! D-Bus-activates the app. GDBus answers a method call for an unregistered object path
//! from its worker thread, immediately, before the main loop has run `startup` -- and
//! `startup` is where a custom interface can first be exported, because GApplication has
//! already taken the bus name by then.
//!
//! `org.freedesktop.Application` and `org.gtk.Actions` are registered by GApplication
//! *during* registration, before the name is acquired, so a cold call to them is queued
//! and delivered normally. Actions are therefore the only race-free way in.
//!
//! That matters because `spec/10` §2 says the extension "D-Bus-activates the app by
//! calling `HandleCapture`" -- exactly the losing path. Anything that must survive a cold
//! start belongs here; `App1` is for calls made while the app is known to be running.

use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use octosnap_core::ScrollDirection;
use octosnap_shell::{Cue, ShellBridge, capture};
use tracing::{error, info, warn};

use crate::{capture_flow, capture_window, notify, prefs, settings};

/// Every action this build registers, in the order `spec/10` §3.2 lists them. Kept as a
/// constant so the log line and the `About` pane cannot drift from what is actually
/// there, and so a missing registration shows up as a compile error rather than as a
/// menu item that does nothing.
const REGISTERED: &[&str] = &[
    "handle-capture",
    "capture",
    // `spec/06` (M5): the extension delivers a record request; the panel item, the
    // shortcut and the pill stop it.
    "record",
    // The countdown's half of the same request: set the recording up while the numbers
    // count, then begin it or throw it away (D70).
    "arm-record",
    "cancel-record",
    "stop-recording",
    // `spec/07` §1 (M6): the extension delivers a scrolling-capture selection; the pill's
    // arrows change what the next Start will do.
    "scroll-capture",
    "scroll-direction",
    // The pill's button from the keyboard, which the pill never takes (D137).
    "scroll-advance",
    "open-settings",
    "dump-settings",
    "open-project",
    "copy-last",
    "save-last",
    "reveal-last",
    // The same for a file that is not a capture: the diagnostics report's notification.
    "reveal-file",
    // `spec/04` §5's overlay-wide commands, which `spec/08` §2 also binds to global
    // shortcuts. Actions rather than methods on the overlay because the panel menu, the
    // shortcuts and the CLI all need the same entry point, and because a shortcut has to
    // work when the app is not the focused application -- which is every time, since the
    // overlay never takes focus.
    "close-all-overlays",
    "save-all-overlays",
    "toggle-overlays",
    "restore-recent",
    // `spec/07` §4.2's strip, toggled: the panel menu, a shortcut and the CLI all open it.
    "open-history",
    // `HIS-04`, `SYS-03`: files the user opens with the app, and the clipboard's image.
    "pin-file",
    "annotate-file",
    "add-overlay",
    "open-from-clipboard",
    // `spec/08` §3's "annotate-last-capture".
    "annotate-last",
    // `DSK-01`, relayed to the extension, whose windows the icons are.
    "desktop-icons",
    // `spec/04` §3's undo toast. Not a shortcut: it is the target of a button on a
    // notification, which is why it takes the pending deletion's token -- a stale toast
    // must not be able to bring back a capture that has already gone, and a fresh one
    // must not bring back the wrong capture when two are waiting.
    "undo-trash",
    // `spec/07` §3.1's global shortcuts over pinned screenshots.
    "close-all-pins",
    "toggle-pins",
    // `spec/07` §2.1's **Show**, the button on the "Text copied" notification, and its
    // "also works on a file" -- `capture-text?filepath=` and `octosnap text FILE`.
    "show-text",
    "read-text",
    "quit",
];

/// What one of `spec/04` §5's overlay-wide commands does.
type OverlayCommand = fn(&crate::SharedQao);

/// `spec/04` §5's commands over the whole stack.
fn overlay_actions() -> [(&'static str, OverlayCommand); 4] {
    [
        ("close-all-overlays", |o| o.close_all()),
        ("save-all-overlays", |o| o.save_all()),
        ("toggle-overlays", |o| o.toggle_hidden()),
        ("restore-recent", |o| {
            if !o.restore_recent() {
                info!("nothing recently closed");
            }
        }),
    ]
}

/// What one of `spec/07` §3.1's pinned-screenshot commands does.
type PinCommand = fn(&crate::SharedPins);

fn pin_actions() -> [(&'static str, PinCommand); 2] {
    [
        ("close-all-pins", |p| p.close_all()),
        ("toggle-pins", |p| p.toggle_hidden()),
    ]
}

pub fn register(app: &adw::Application) {
    let handle_capture =
        gio::SimpleAction::new("handle-capture", Some(glib::VariantTy::VARDICT));
    {
        let app = app.clone();
        handle_capture.connect_activate(move |_, parameter| {
            let Some(payload) = parameter else {
                warn!("handle-capture invoked without a payload");
                return;
            };
            match capture::from_variant(payload) {
                Ok(result) => {
                    info!(capture = %capture_window::describe(&result), "handle-capture");

                    // Run the ACT-01 plan, then show the capture. The plan is awaited
                    // because copy-to-clipboard is a D-Bus round trip; showing the window
                    // afterwards means the user never sees a card for a capture whose
                    // copy silently failed.
                    let app = app.clone();
                    glib::spawn_future_local(async move {
                        match capture_flow() {
                            Some(flow) => {
                                // The flow shows the card itself, because whether one
                                // appears is part of the outcome rather than something
                                // decided afterwards (`docs/decisions.md` D27).
                                let outcome = flow.handle(&result).await;
                                // `ACT-05`: with no card, nothing on screen has told the
                                // user the capture happened. `notify` decides whether
                                // there is anything worth saying.
                                notify::capture_outcome(
                                    &app,
                                    &outcome,
                                    settings::Settings::load().notifications_enabled(),
                                );
                            }
                            None => {
                                // No flow means no settings and no bridge, so there is
                                // nothing to run and no card to show. The capture is on
                                // disk with its twin; a plain window is the only way to
                                // put it in front of the user.
                                warn!("no capture flow available; showing the capture only");
                                capture_window::show(&app, &result);
                            }
                        }
                    });
                }
                // An action activation has no reply channel, so the error can only be
                // logged. The capture is not lost: `spec/10` §8 requires the PNG and its
                // JSON twin to be on disk before the app is ever notified.
                Err(e) => warn!("handle-capture received a malformed payload: {e}"),
            }
        });
    }
    app.add_action(&handle_capture);

    // `spec/10` §3.2's `capture(s mode, a{sv})`. This is the CLI's and the URL scheme's
    // way in, and it goes through the app rather than straight to the extension on
    // purpose: the app is the only half that knows whether the extension is there at all
    // (`spec/00` §4.3's degraded mode), and it owns `api-enabled`. Routing here also
    // starts the app *before* the capture rather than during it, which takes the
    // cold-activation race off the capture path entirely.
    let request_type = glib::VariantTy::new("(sa{sv})").ok();
    let capture_action = gio::SimpleAction::new("capture", request_type);
    {
        let app = app.clone();
        capture_action.connect_activate(move |_, parameter| {
            let Some(payload) = parameter else {
                warn!("capture invoked without a mode");
                return;
            };
            let request = match capture::request_from_variant(payload) {
                Ok(request) => request,
                Err(e) => {
                    warn!("capture received a malformed request: {e}");
                    return;
                }
            };

            // `spec/08` §9. Read live rather than cached, so turning the API off takes
            // effect on the next invocation and not on the next login.
            if !settings::Settings::load().api_enabled() {
                warn!(
                    mode = request.mode.as_wire(),
                    "refused a capture: the CLI and URL scheme are disabled (api-enabled)"
                );
                return;
            }

            let app = app.clone();
            glib::spawn_future_local(async move {
                let Some(flow) = capture_flow() else {
                    error!("no capture flow available; cannot start a capture");
                    return;
                };
                match flow.begin(&request).await {
                    Ok(_handle) => {}
                    Err(e) => {
                        // The user asked for a capture and got none, and there is no
                        // reply channel on an action activation, so this is the one place
                        // that has to speak up.
                        error!("could not start a capture: {e}");
                        let title = if e.is_extension_missing() {
                            "OctoSnap needs its GNOME Shell extension"
                        } else {
                            "Could not start the capture"
                        };
                        let notification = gio::Notification::new(title);
                        notification.set_body(Some(&e.to_string()));
                        app.send_notification(Some("capture-failed"), &notification);
                    }
                }
            });
        });
    }
    app.add_action(&capture_action);

    // `spec/06` (M5): the extension delivers a record request -- a rectangle on a monitor,
    // not pixels -- and the app owns the recording from there (`crate::recording`). Like
    // `handle-capture`, this is the result of a user action in the overlay, so it is not
    // gated by `api-enabled`; the CLI's and URL's way in is the `capture` action with the
    // record mode, which is (Step 6).
    let record = gio::SimpleAction::new("record", Some(glib::VariantTy::VARDICT));
    record.connect_activate(|_, parameter| {
        let Some(payload) = parameter else {
            warn!("record invoked without a request");
            return;
        };
        let Some(request) = crate::recording::RecordRequest::from_variant(payload) else {
            warn!("record received a malformed request");
            return;
        };
        info!(?request, "record");
        let Some(recorder) = crate::recorder() else {
            error!("no recorder available; cannot start recording");
            return;
        };
        glib::spawn_future_local(async move { recorder.start(request).await });
    });
    app.add_action(&record);

    // The same request, delivered while the countdown runs. Everything that makes a
    // recording slow to start -- the session, the node, the graph, the encoder, the
    // stream's negotiation -- happens here, and `record` then only stops the frames being
    // thrown away (D70). Ignorable: an extension that never sends it, or an app that
    // cannot arm, still records when `record` arrives, just as slowly as before.
    let arm_record = gio::SimpleAction::new("arm-record", Some(glib::VariantTy::VARDICT));
    arm_record.connect_activate(|_, parameter| {
        let Some(request) =
            parameter.and_then(crate::recording::RecordRequest::from_variant)
        else {
            warn!("arm-record received a malformed request");
            return;
        };
        info!(?request, "arm-record");
        let Some(recorder) = crate::recorder() else {
            return;
        };
        glib::spawn_future_local(async move { recorder.arm(request).await });
    });
    app.add_action(&arm_record);

    // The countdown was cancelled, or the overlay closed: give the session back rather
    // than leaving Mutter's recording indicator lit over a recording that will not happen.
    let cancel_record = gio::SimpleAction::new("cancel-record", None);
    cancel_record.connect_activate(|_, _| {
        info!("cancel-record");
        if let Some(recorder) = crate::recorder() {
            recorder.disarm();
        }
    });
    app.add_action(&cancel_record);

    // `spec/07` §1.1: the selection has been made, so put its controls up. Not gated by
    // `api-enabled` for the same reason `record` is not -- the usual sender is the user's
    // own drag in the overlay -- and the CLI and URL reach it through `capture` with the
    // scrolling mode, which is gated where every other API entry point is.
    let scroll_capture = gio::SimpleAction::new("scroll-capture", Some(glib::VariantTy::VARDICT));
    scroll_capture.connect_activate(|_, parameter| {
        let Some(request) = parameter.and_then(crate::scrolling::Request::from_variant) else {
            warn!("scroll-capture received a malformed request");
            return;
        };
        info!(?request, "scroll-capture");
        let Some(scroller) = crate::scroller() else {
            error!("no scroller available; cannot start a scrolling capture");
            return;
        };
        glib::spawn_future_local(async move { scroller.begin(request).await });
    });
    app.add_action(&scroll_capture);

    // The pill's arrows. A stateful action so whatever shows the direction can read which
    // one is set, and one action rather than four so the arrows are a loop over
    // `ScrollDirection::ALL`. Each pill sets the state to its own request's (D152).
    let scroll_direction = gio::SimpleAction::new_stateful(
        "scroll-direction",
        Some(glib::VariantTy::STRING),
        &ScrollDirection::default().as_wire().to_variant(),
    );
    scroll_direction.connect_activate(|action, parameter| {
        let Some(direction) =
            parameter.and_then(glib::Variant::str).and_then(ScrollDirection::from_wire)
        else {
            warn!("scroll-direction invoked without a direction");
            return;
        };
        action.set_state(&direction.as_wire().to_variant());
        if let Some(scroller) = crate::scroller() {
            scroller.set_direction(direction);
        }
    });
    app.add_action(&scroll_direction);

    // The pill's button without the pointer: the Scrolling Capture key pressed again, and
    // the panel menu's item while a pill is up. The pill never takes the keyboard -- the
    // page it captures needs it -- so its button was the pointer's alone (`spec/00` §9,
    // D137). Start, and once the capture runs, Done.
    let scroll_advance = gio::SimpleAction::new("scroll-advance", None);
    scroll_advance.connect_activate(|_, _| {
        info!("scroll-advance");
        if let Some(scroller) = crate::scroller() {
            scroller.advance();
        }
    });
    app.add_action(&scroll_advance);

    // `spec/06` §3 Stop, for the panel item, the `recording-stop` shortcut and the CLI.
    // A no-op when nothing is recording, so every source can fire it without checking.
    let stop_recording = gio::SimpleAction::new("stop-recording", None);
    stop_recording.connect_activate(|_, _| {
        info!("stop-recording");
        match crate::recorder() {
            Some(recorder) if recorder.is_recording() => recorder.request_stop(),
            Some(_) => info!("stop-recording: nothing is recording"),
            None => warn!("no recorder available"),
        }
    });
    app.add_action(&stop_recording);

    // `spec/07` §4.2: the history strip, shown or hidden.
    let open_history = gio::SimpleAction::new("open-history", None);
    {
        let app = app.clone();
        open_history.connect_activate(move |_, _| {
            info!("open-history");
            crate::history::strip::toggle(&app);
        });
    }
    app.add_action(&open_history);

    // `HIS-04`, `SYS-03`: a file becomes a capture in the spool and then a pin, an editor
    // or a card; the clipboard's image becomes one and opens in the editor.
    {
        let pin_file = gio::SimpleAction::new("pin-file", Some(glib::VariantTy::STRING));
        pin_file.connect_activate(|_, parameter| {
            let Some(path) = parameter.and_then(glib::Variant::str).map(std::path::PathBuf::from) else { return };
            info!(path = %path.display(), "pin-file");
            let (Some(pins), Some(capture)) = (crate::pins(), crate::import::import_file(&path)) else { return };
            pins.pin(&capture, None);
        });
        app.add_action(&pin_file);

        let annotate_file = gio::SimpleAction::new("annotate-file", Some(glib::VariantTy::STRING));
        annotate_file.connect_activate(|_, parameter| {
            let Some(path) = parameter.and_then(glib::Variant::str).map(std::path::PathBuf::from) else { return };
            info!(path = %path.display(), "annotate-file");
            let Some(overlay) = crate::overlay() else { return };
            // A project opens as a project (`spec/05` §8); anything else is a picture.
            if octosnap_core::project::is_project(&path) {
                overlay.open_project(&path);
                return;
            }
            if let Some(capture) = crate::import::import_file(&path) {
                overlay.open_editor(&capture);
            }
        });
        app.add_action(&annotate_file);

        let add_overlay = gio::SimpleAction::new("add-overlay", Some(glib::VariantTy::STRING));
        add_overlay.connect_activate(|_, parameter| {
            let Some(path) = parameter.and_then(glib::Variant::str).map(std::path::PathBuf::from) else { return };
            info!(path = %path.display(), "add-overlay");
            let (Some(overlay), Some(capture)) = (crate::overlay(), crate::import::import_file(&path)) else { return };
            overlay.show(&capture, None);
        });
        app.add_action(&add_overlay);

        let from_clipboard = gio::SimpleAction::new("open-from-clipboard", None);
        {
            let app = app.clone();
            from_clipboard.connect_activate(move |_, _| {
                info!("open-from-clipboard");
                let app = app.clone();
                glib::spawn_future_local(async move {
                    match crate::import::import_clipboard().await {
                        Some(capture) => {
                            if let Some(overlay) = crate::overlay() {
                                overlay.open_editor(&capture);
                            }
                        }
                        None => notify::action_failed(&app, "Open from clipboard", "The clipboard holds no image"),
                    }
                });
            });
        }
        app.add_action(&from_clipboard);

        // `spec/08` §3's `annotate-last-capture`: the editor on whatever the flow saw last.
        let annotate_last = gio::SimpleAction::new("annotate-last", None);
        {
            let app = app.clone();
            annotate_last.connect_activate(move |_, _| {
                info!("annotate-last");
                let (Some(flow), Some(overlay)) = (capture_flow(), crate::overlay()) else { return };
                match flow.last_capture() {
                    Some(capture) => overlay.open_editor(&capture),
                    None => notify::action_failed(&app, "Annotate the last capture", "No capture yet"),
                }
            });
        }
        app.add_action(&annotate_last);

        // `DSK-01`: the icons are the extension's windows to hide, so this only relays and
        // reports. An extension without the method is an older one: the answer names the
        // logout, which is the only thing that loads a newer one (`docs/spikes/16`).
        let desktop = gio::SimpleAction::new("desktop-icons", Some(glib::VariantTy::STRING));
        {
            let app = app.clone();
            desktop.connect_activate(move |_, parameter| {
                let what = parameter.and_then(glib::Variant::str).unwrap_or("toggle").to_owned();
                let app = app.clone();
                glib::spawn_future_local(async move {
                    use octosnap_shell::ShellBridge;
                    let Some(flow) = capture_flow() else { return };
                    match flow.bridge().desktop_icons(&what).await {
                        Ok((hidden, available)) => {
                            info!(what, hidden, available, "desktop icons");
                            if !available {
                                notify::action_failed(&app, "Desktop icons", "This session has no desktop icons to hide");
                            }
                        }
                        Err(e) if e.is_version_mismatch() => {
                            warn!("desktop icons: the extension does not know the method: {e}");
                            notify::action_failed(
                                &app,
                                "Desktop icons",
                                "OctoSnap's GNOME Shell extension needs its update: log out and back in once",
                            );
                        }
                        Err(e) => {
                            warn!("desktop icons failed: {e}");
                            notify::action_failed(&app, "Desktop icons", &e.to_string());
                        }
                    }
                });
            });
        }
        app.add_action(&desktop);
    }

    // `spec/10` §3.2's `open-settings(s tab)`. An empty tab name means "wherever it was",
    // which is what the panel menu's plain Settings item sends.
    let open_settings = gio::SimpleAction::new("open-settings", Some(glib::VariantTy::STRING));
    open_settings.connect_activate(move |_, parameter| {
        let tab = parameter.and_then(glib::Variant::str).unwrap_or_default();
        info!(tab, "open-settings");
        prefs::present(tab);
    });
    app.add_action(&open_settings);

    // Settings' Shutter sound row plays each shutter as it is chosen (D134), whatever
    // `ui-sounds` says: the user is choosing a sound, and a preview that stayed silent
    // would choose for them. The parameter is a `capture-sound` value.
    let preview_shutter = gio::SimpleAction::new("preview-shutter", Some(glib::VariantTy::STRING));
    preview_shutter.connect_activate(move |_, parameter| {
        let Some(kind) = parameter.and_then(glib::Variant::str).and_then(prefs::shutter_kind) else {
            return;
        };
        glib::spawn_future_local(async move {
            let Some(flow) = capture_flow() else { return };
            match flow.bridge().play_sound(Cue::Shutter(kind)).await {
                Ok(()) => info!(kind, "previewed the shutter"),
                Err(e) => warn!(kind, "could not preview the shutter: {e}"),
            }
        });
    });
    app.add_action(&preview_shutter);

    // `dump-settings (path)`: the settings dialog as it is drawn, to a PNG.
    //
    // The editor's `dump-window` for the other window, and for the same reason: on a real
    // GNOME session `org.gnome.Shell.Screenshot` answers "not allowed", so without this
    // the only way to look at `spec/08`'s pages is to be sitting in front of them. The
    // dialog is its own toplevel rather than an `ApplicationWindow` (`prefs::present`
    // takes no parent), so it is not on the bus and the editor's action cannot see it --
    // hence one here, on the application.
    let dump_settings = gio::SimpleAction::new("dump-settings", Some(glib::VariantTy::STRING));
    dump_settings.connect_activate(move |_, parameter| {
        let Some(argument) = parameter.and_then(glib::Variant::str) else {
            warn!("dump-settings needs a path");
            return;
        };
        // An optional `|WIDTHxHEIGHT` and `|FRACTION` after the path, the way `add-image`
        // takes its zone: `spec/08`'s pages are taller than any window the monitor will
        // hold, and a dump of the top third does not answer whether the bottom third
        // laid out. `…|860x1000|1` is the foot of the page.
        let mut parts = argument.split('|');
        let path = parts.next().unwrap_or(argument).to_owned();
        if let Some((w, h)) = parts.next().and_then(|size| size.split_once('x'))
            && let (Ok(w), Ok(h)) = (w.trim().parse(), h.trim().parse())
        {
            prefs::resize_dialog(w, h);
        }
        if let Some(Ok(fraction)) = parts.next().map(str::parse) {
            prefs::scroll_dialog(fraction);
        }
        // A frame first: a `GtkWidgetPaintable` hands back the widget's *last drawn* node
        // tree, and a dialog that has only just been presented has none.
        glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
            match prefs::dump(std::path::Path::new(&path)) {
                Some(()) => info!(path, "settings dumped"),
                None => warn!(path, "the settings dialog could not be rendered"),
            }
        });
    });
    app.add_action(&dump_settings);

    // `spec/05` §8: "opening a project from a card or history restores full editability".
    // An action rather than a command-line flag because that is how every other way into
    // this app works -- the CLI, the URL scheme and the extension all land on an action --
    // and because `spec/05` §11 item 8's "reopen" has to be reachable from outside the
    // process to be checked at all.
    let open_project = gio::SimpleAction::new("open-project", Some(glib::VariantTy::STRING));
    {
        let app = app.clone();
        open_project.connect_activate(move |_, parameter| {
            let Some(path) = parameter.and_then(|p| p.get::<String>()) else {
                warn!("open-project needs a path");
                return;
            };
            let path = std::path::PathBuf::from(path);
            let Some(overlay) = crate::overlay() else {
                warn!("no overlay; a project needs the editor actions it builds");
                notify::unavailable(&app, "Open project", "the capture overlay");
                return;
            };
            overlay.open_project(&path);
        });
    }
    app.add_action(&open_project);

    for (name, run) in overlay_actions() {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate(move |_, _| match crate::overlay() {
            Some(overlay) => run(&overlay),
            // Before the first capture there is no overlay to command. Not a warning:
            // pressing "close all overlays" when none are open is a reasonable thing to
            // do and nothing is wrong.
            None => info!(name, "no overlay yet"),
        });
        app.add_action(&action);
    }

    for (name, run) in pin_actions() {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate(move |_, _| match crate::pins() {
            Some(pins) => run(&pins),
            None => info!(name, "no pins yet"),
        });
        app.add_action(&action);
    }

    // `spec/04` §3's Undo, as the notification's button target. `t` -- the token, not
    // the card id: the card is gone by the time the toast is up.
    let undo_trash = gio::SimpleAction::new("undo-trash", Some(glib::VariantTy::UINT64));
    undo_trash.connect_activate(move |_, parameter| {
        let Some(overlay) = crate::overlay() else {
            info!("undo-trash with no overlay");
            return;
        };
        // A toast with no token is one this build did not send, so the newest pending
        // deletion is the only sensible reading -- and it is also what a keyboard-driven
        // undo would want if one is ever bound.
        match parameter.and_then(|p| p.get::<u64>()) {
            Some(token) => {
                overlay.undo_trash(token);
            }
            None => {
                overlay.undo_trash_newest();
            }
        }
    });
    app.add_action(&undo_trash);

    let copy_last = gio::SimpleAction::new("copy-last", None);
    copy_last.connect_activate(move |_, _| {
        glib::spawn_future_local(async move {
            let Some(flow) = capture_flow() else { return };
            match flow.copy_last().await {
                Ok(path) => info!(path = %path.display(), "copy-last"),
                Err(e) => warn!("copy-last failed: {e}"),
            }
        });
    });
    app.add_action(&copy_last);

    let save_last = gio::SimpleAction::new("save-last", None);
    {
        let app = app.clone();
        save_last.connect_activate(move |_, _| {
            let app = app.clone();
            glib::spawn_future_local(async move {
                let Some(flow) = capture_flow() else { return };
                match flow.save_last().await {
                    Ok(path) => {
                        info!(path = %path.display(), "save-last");
                        // A save with no card and no overlay is exactly ACT-05's
                        // "silent action": without this the shortcut looks like a no-op.
                        let outcome = crate::flow::Outcome {
                            saved_to: Some(path),
                            recording: flow.last_is_recording(),
                            ..crate::flow::Outcome::default()
                        };
                        notify::capture_outcome(
                            &app,
                            &outcome,
                            settings::Settings::load().notifications_enabled(),
                        );
                    }
                    Err(e) => warn!("save-last failed: {e}"),
                }
            });
        });
    }
    app.add_action(&save_last);

    // The button on `ACT-05`'s save notification. `GtkFileLauncher` is what asks the
    // file manager to open the folder *with the file selected*, going through the
    // FileManager1 interface where it exists and falling back to opening the folder
    // where it does not. Doing that by hand would mean reimplementing both halves.
    let reveal_last = gio::SimpleAction::new("reveal-last", None);
    reveal_last.connect_activate(move |_, _| {
        let Some(flow) = capture_flow() else { return };
        let Some(path) = flow.last_saved() else {
            info!("reveal-last with nothing saved yet");
            return;
        };
        reveal(&path);
    });
    app.add_action(&reveal_last);

    // The path comes back from the notification server as the button's target, which is
    // all a button can carry.
    let reveal_file = gio::SimpleAction::new("reveal-file", Some(glib::VariantTy::STRING));
    reveal_file.connect_activate(|_, target| {
        match target.and_then(|v| v.str().map(std::path::PathBuf::from)) {
            Some(path) => reveal(&path),
            None => warn!("reveal-file without a path"),
        }
    });
    app.add_action(&reveal_file);

    // `spec/07` §2.1's **Show**. The read is held in `crate::ocr` rather than passed as
    // the action's parameter: a notification's target value goes out over D-Bus and comes
    // back, and putting a page of recognised text through that would mean the clipboard's
    // copy and the window's copy could differ by whatever the wire did to it.
    let show_text = gio::SimpleAction::new("show-text", None);
    {
        let app = app.clone();
        show_text.connect_activate(move |_, _| {
            let Some((read, config)) = crate::ocr::last() else {
                // A notification in the tray outlives the app that sent it, and its Show
                // then starts one that has read nothing.
                info!("show-text with nothing read yet");
                notify::tell(
                    &app,
                    "show-text",
                    "No text to show",
                    "Nothing has been read since OctoSnap started.",
                    false,
                );
                return;
            };
            crate::ocr::window::show(&app, &read, config.breaks, config.links);
        });
    }
    app.add_action(&show_text);

    // `spec/07` §2.1's file source. A vardict rather than a plain path because the two
    // shortcuts differ by one boolean and `octosnap text --linebreaks FILE` has to be
    // able to say so; `a{sv}` is what every other request in `spec/10` §3.2 uses.
    let read_text = gio::SimpleAction::new("read-text", Some(glib::VariantTy::VARDICT));
    {
        let app = app.clone();
        read_text.connect_activate(move |_, parameter| {
            let dict = glib::VariantDict::new(parameter);
            let Some(path) = dict.lookup::<String>("path").ok().flatten().filter(|p| !p.is_empty())
            else {
                warn!("read-text invoked without a path");
                return;
            };
            let linebreaks = dict.lookup::<bool>("linebreaks").ok().flatten();
            let app = app.clone();
            glib::spawn_future_local(async move {
                let Some(flow) = capture_flow() else {
                    error!("no capture flow available; cannot read a file");
                    return;
                };
                info!(path, "read-text");
                let outcome = flow.read_text(std::path::Path::new(&path), None, linebreaks).await;
                notify::capture_outcome(
                    &app,
                    &outcome,
                    settings::Settings::load().notifications_enabled(),
                );
            });
        });
    }
    app.add_action(&read_text);

    let quit = gio::SimpleAction::new("quit", None);
    {
        let app = app.clone();
        quit.connect_activate(move |_, _| {
            info!("quit requested over D-Bus");
            app.quit();
        });
    }
    app.add_action(&quit);

    info!(actions = ?REGISTERED, "registered GApplication actions");
}

/// Opens the folder holding `path` in the file manager, with the file selected.
/// `GtkFileLauncher` is what asks for the selection, going through the FileManager1
/// interface where it exists and falling back to opening the folder where it does not.
/// Doing that by hand would mean reimplementing both halves.
fn reveal(path: &std::path::Path) {
    let folder = path.parent().unwrap_or(path).to_path_buf();
    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
    launcher.open_containing_folder(None::<&gtk::Window>, gio::Cancellable::NONE, move |result| {
        match result {
            Ok(()) => info!(folder = %folder.display(), "revealed"),
            Err(e) => warn!("could not open {}: {e}", folder.display()),
        }
    });
}
