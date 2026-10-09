// SPDX-License-Identifier: GPL-3.0-or-later

//! The Preferences dialog: `spec/11` M1's "minimal Preferences (General, Screenshots,
//! Shortcuts)", against `spec/08`'s tables.
//!
//! Two schemas, one window. `spec/08` §11 splits the keys between the app and the
//! extension, and the split is real -- the extension reads its keys on the compositor's
//! main loop at the instant of a grab, the app reads its own after the capture has
//! arrived -- but the user should never have to know that. So this dialog writes both and
//! `docs/decisions.md` D11 records which key lives where and why.
//!
//! Four things from `spec/08` §0 that are deliberate and easy to lose in a refactor:
//! dependent rows stay **visible but insensitive** rather than disappearing, so the user
//! can see what enabling the parent will give them; every modifier trick is written on
//! the row it affects rather than in documentation; there is a **single** export
//! location feeding every save path; and After Capture is a **matrix** of actions rather
//! than a list of unrelated switches.

use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use octosnap_core::actions::AfterAction;
use octosnap_core::qao::{self, AutoClose, Edge};
use octosnap_core::savepath::ImageFormat;
use tracing::{info, warn};

use crate::settings;

/// `spec/10` §3.2's `open-settings(s tab)` names its tabs after CleanShot's; these are
/// the ones this build has. An unknown or empty name lands on the first page.
const PAGE_GENERAL: &str = "general";
const PAGE_SCREENSHOTS: &str = "screenshots";
const PAGE_RECORDING: &str = "recording";
const PAGE_SHORTCUTS: &str = "shortcuts";
const PAGE_QUICK_ACCESS: &str = "quick-access";
const PAGE_ANNOTATE: &str = "annotate";
const PAGE_PETS: &str = "pets";
const PAGE_ADVANCED: &str = "advanced";
const PAGE_ABOUT: &str = "about";
/// Not a page: the Advanced page at its language packs, with the pack to install pointed
/// at (D171). Where a read that found no pack sends the user, and the editor's toast.
pub const PACKS: &str = "language-packs";

/// Where the project lives, for the About page's links.
const REPOSITORY: &str = "https://github.com/odrakirmusic/OctoSnap";

thread_local! {
    /// The dialog is a singleton: `open-settings` arriving twice must raise the window
    /// the user already has, not stack a second copy of it behind the first.
    static OPEN: std::cell::RefCell<Option<adw::PreferencesDialog>> =
        const { std::cell::RefCell::new(None) };
    /// The toast saying a read waits for a pack, while it is up (D171).
    static WAITING: std::cell::RefCell<Option<adw::Toast>> =
        const { std::cell::RefCell::new(None) };
}

/// Makes the open dialog's window a given size, so a long page fits in one dump.
///
/// A debug helper for `dump-settings` and nothing else: `spec/08`'s pages are taller than
/// any sensible dialog, and a screenshot of the top third does not answer whether the
/// bottom third laid out. It does not put the size back, because the window it is
/// resizing belongs to a harness rather than to anyone looking at it.
pub fn resize_dialog(width: i32, height: i32) {
    let Some(dialog) = OPEN.with(|open| open.borrow().clone()) else { return };
    let Some(root) = dialog.root().and_downcast::<gtk::Window>() else { return };
    root.set_default_size(width, height);
    root.present();
}

/// Scrolls the visible page, so the rows below the fold can be dumped too.
///
/// A page taller than the screen cannot be made to fit by resizing -- the window stops at
/// the monitor -- and the viewport allocates its child the viewport's own height, so the
/// part scrolled away is not in the node tree either. Moving the adjustment is what is
/// left. `fraction` is 0 for the top and 1 for the bottom.
pub fn scroll_dialog(fraction: f64) {
    let Some(dialog) = OPEN.with(|open| open.borrow().clone()) else { return };
    let Some(page) = dialog.visible_page() else { return };
    let Some(scroller) = scrolled_window(page.upcast_ref::<gtk::Widget>()) else { return };
    let adjustment = scroller.vadjustment();
    let room = adjustment.upper() - adjustment.page_size();
    adjustment.set_value(adjustment.lower() + room * fraction.clamp(0.0, 1.0));
}

/// The first `GtkScrolledWindow` inside `widget`, which is the one `AdwPreferencesPage`
/// builds for itself. Depth-first, because the page has exactly one and it is shallow.
pub(crate) fn scrolled_window(widget: &gtk::Widget) -> Option<gtk::ScrolledWindow> {
    let mut child = widget.first_child();
    while let Some(candidate) = child {
        if let Ok(scroller) = candidate.clone().downcast::<gtk::ScrolledWindow>() {
            return Some(scroller);
        }
        if let Some(found) = scrolled_window(&candidate) {
            return Some(found);
        }
        child = candidate.next_sibling();
    }
    None
}

/// The dialog as it is drawn, to a PNG: `dump-settings`'s half in this module.
///
/// `None` when there is no dialog open, or when the compositor has not drawn it -- an
/// occluded window has no node tree to hand back, which is the same trap `dump-window`
/// documents.
#[must_use]
pub fn dump(path: &std::path::Path) -> Option<()> {
    let dialog = OPEN.with(|open| open.borrow().clone())?;
    let root = dialog.root()?;
    let renderer = root.native().and_then(|native| native.renderer())?;
    // The *page*, not the window, when there is one: a `GtkViewport` allocates its child
    // the child's natural height, so a page taller than any screen is drawn in full even
    // though only a screen's worth of it is visible. Rendering the window instead would
    // answer "does the top of this page lay out", which is not the question.
    let subject = dialog
        .visible_page()
        .map(|page| page.upcast::<gtk::Widget>())
        .filter(|page| page.height() > root.height())
        .unwrap_or_else(|| root.clone().upcast());
    render_to_png(&subject, &renderer, path)
}

/// `subject` as it was last drawn, to a PNG, through `renderer`. The half of
/// [`dump`] that the welcome window's `dump-welcome` shares.
pub(crate) fn render_to_png(
    subject: &gtk::Widget,
    renderer: &gtk::gsk::Renderer,
    path: &std::path::Path,
) -> Option<()> {
    let (width, height) = (subject.width(), subject.height());
    if width <= 0 || height <= 0 {
        return None;
    }
    let paintable = gtk::WidgetPaintable::new(Some(subject));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
    let node = snapshot.to_node()?;
    #[allow(clippy::cast_precision_loss)]
    let bounds = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    let texture = renderer.render_texture(&node, Some(&bounds));
    if let Err(why) = texture.save_to_png(path) {
        warn!(path = %path.display(), "dump: {why}");
        return None;
    }
    Some(())
}

/// Shows the dialog, on `tab` if it names one.
///
/// The application is not needed: `main.rs` holds the service open for its whole
/// lifetime (`spec/10` §2), so the dialog has nothing to keep alive.
pub fn present(tab: &str) {
    let existing = OPEN.with(|open| open.borrow().clone());
    if let Some(dialog) = existing {
        select_page(&dialog, tab);
        // No parent: `AdwDialog` presented against nothing becomes its own window, which
        // is what a background service with no other windows needs.
        dialog.present(None::<&gtk::Widget>);
        return;
    }

    let dialog = build();
    select_page(&dialog, tab);
    dialog.connect_closed(|_| {
        OPEN.with(|open| *open.borrow_mut() = None);
        WAITING.with(|waiting| *waiting.borrow_mut() = None);
        info!("settings closed");
    });
    OPEN.with(|open| *open.borrow_mut() = Some(dialog.clone()));
    dialog.present(None::<&gtk::Widget>);
}

fn select_page(dialog: &adw::PreferencesDialog, tab: &str) {
    // Appendix A.8 spells the Quick Access tab `quickaccess`; the page is named the way
    // the tab reads. Both land on the same page.
    let tab = if tab == "quickaccess" { PAGE_QUICK_ACCESS } else { tab };
    match tab {
        PAGE_GENERAL | PAGE_SCREENSHOTS | PAGE_RECORDING | PAGE_SHORTCUTS | PAGE_QUICK_ACCESS
        | PAGE_ANNOTATE | PAGE_PETS | PAGE_ADVANCED | PAGE_ABOUT => {
            dialog.set_visible_page_name(tab);
            info!(page = tab, "settings page shown");
        }
        PACKS => show_packs(dialog),
        "" => {}
        // `spec/08` §0's Wallpaper, Screen Recording and Cloud tabs wait for the features
        // that give their rows meaning (M6, M5, M8). The first page, and a line.
        other => info!(tab = other, "no such settings page in this build"),
    }
}

/// The Advanced page at its language packs, the suggested one ready to install (D171).
///
/// What a read that found no pack opens, instead of a notification about it. A text
/// capture puts nothing on screen, so a notification was all it had, and one that went by
/// unread left a shutter, a flight to the corner and an empty clipboard: a screenshot, to
/// anyone who heard and saw it. The read is kept, and installing a pack finishes it
/// ([`read_finished`]); the toast says so while it waits.
fn show_packs(dialog: &adw::PreferencesDialog) {
    dialog.set_visible_page_name(PAGE_ADVANCED);
    info!(page = PAGE_ADVANCED, "settings page shown at the language packs");
    let configured = crate::settings::ocr_config().script;
    let script = crate::ocr::suggested(configured, &glib::language_names());
    crate::ocr::prefs::point_at(script);

    if !crate::capture_flow().is_some_and(|flow| flow.is_waiting()) {
        return;
    }
    let toast = adw::Toast::new("Install a language pack to read the text you captured");
    // Until the pack is in, or the user closes it: it says why this window opened, and
    // a few seconds would leave a download of a minute or more with no reason beside it.
    toast.set_timeout(0);
    WAITING.with(|waiting| {
        if let Some(old) = waiting.borrow_mut().replace(toast.clone()) {
            old.dismiss();
        }
    });
    dialog.add_toast(toast);
}

/// Says how the read that waited for a pack ended, in Settings while it is open (D171).
///
/// The user pressed Install there and is looking there (`spec/13` #6), so the toast that
/// said the read was waiting gives way to the read's result, with the notification's own
/// words and buttons. `false` when Settings has been closed, and the caller notifies.
pub fn read_finished(outcome: &crate::flow::Outcome) -> bool {
    let Some(dialog) = OPEN.with(|open| open.borrow().clone()) else { return false };
    if let Some(waiting) = WAITING.with(|waiting| waiting.borrow_mut().take()) {
        waiting.dismiss();
    }
    let Some(toast) = crate::notify::read_toast(outcome) else { return false };
    let title = toast.title().map(String::from).unwrap_or_default();
    info!(title, "settings told how the waiting read ended");
    dialog.add_toast(toast);
    true
}

fn build() -> adw::PreferencesDialog {
    let dialog = adw::PreferencesDialog::builder().title("OctoSnap Settings").build();

    let app_settings = settings::open_schema(settings::SCHEMA_ID);
    let ext_settings = settings::extension_settings();

    // A dialog whose switches silently do nothing is worse than one that says why. This
    // is the single most likely development-build state, and it used to present as
    // "settings do not stick".
    if app_settings.is_none() || ext_settings.is_none() {
        // In a Flatpak both schemas ship with the app, so a missing one is a broken
        // installation or an extension that is not answering (D123), never a dev build.
        dialog.add_toast(adw::Toast::new(match (&app_settings, &ext_settings, settings::sandboxed()) {
            (None, _, true) => "This installation of OctoSnap is missing its settings; reinstall it",
            (Some(_), _, true) => "The OctoSnap extension is not answering, so its settings are unavailable",
            (None, None, false) => "Neither schema is installed; run scripts/install-dev.sh",
            (None, _, false) => "The app's schema is not installed; run scripts/install-dev.sh",
            (Some(_), _, false) => "The extension's schema is not installed; its settings are unavailable",
        }));
    }

    // `spec/08` §0's order, with About last, minus the three tabs whose features are
    // later milestones. Search is libadwaita's own, over every row title.
    dialog.set_search_enabled(true);
    dialog.add(&general_page(app_settings.as_ref(), ext_settings.as_ref()));
    dialog.add(&shortcuts_page(ext_settings.as_ref()));
    dialog.add(&quick_access_page(app_settings.as_ref()));
    dialog.add(&screenshots_page(app_settings.as_ref(), ext_settings.as_ref()));
    dialog.add(&recording_page(app_settings.as_ref(), ext_settings.as_ref()));
    dialog.add(&annotate_page(app_settings.as_ref()));
    dialog.add(&pets_page(ext_settings.as_ref()));
    dialog.add(&advanced_page(app_settings.as_ref(), ext_settings.as_ref()));
    dialog.add(&about_page());
    dialog
}

// --- Quick Access ---------------------------------------------------------------------

/// `spec/08` §1's Quick Access tab, and `spec/08` §4's keys.
///
/// Two of that section's rows describe a shape the screenshots contradict, and the
/// screenshots are the later evidence. "Position on screen" is an **edge**, not a corner
/// (`spec/04` §2 [P->V]), and the size is a **five-step slider**, not three named sizes
/// (`spec/04` §1 [V]). The gschema carries the same note.
fn quick_access_page(app_settings: Option<&gio::Settings>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Quick Access")
        .name(PAGE_QUICK_ACCESS)
        .icon_name("view-grid-symbolic")
        .build();

    let appearance = adw::PreferencesGroup::builder().title("Appearance").build();

    let edge = adw::ComboRow::builder()
        .title("Position on screen")
        .model(&gtk::StringList::new(&["Left", "Right"]))
        .build();
    const EDGE_ORDER: [Edge; 2] = [Edge::Left, Edge::Right];
    match with_key(app_settings, "qao-edge") {
        Some(s) => bind_enum_row(s, "qao-edge", &edge, &EDGE_ORDER, |e| e.as_wire()),
        None => edge.set_sensitive(false),
    }
    appearance.add(&edge);

    let follow = adw::SwitchRow::builder()
        .title("Move to active screen")
        .subtitle("Show cards on the screen the pointer is on")
        .build();
    bind_boolean(app_settings, "qao-follow-pointer", &follow);
    appearance.add(&follow);

    let size = adw::SpinRow::builder()
        .title("Overlay size")
        .subtitle(format!(
            "1 is {} pt across, 5 is {} pt",
            qao::SIZE_TABLE[0],
            qao::SIZE_TABLE[qao::SIZE_STEPS as usize - 1]
        ))
        .adjustment(&gtk::Adjustment::new(
            f64::from(qao::DEFAULT_SIZE_STEP),
            1.0,
            f64::from(qao::SIZE_STEPS),
            1.0,
            1.0,
            0.0,
        ))
        .build();
    bind_int(app_settings, "qao-size", &size);
    appearance.add(&size);
    page.add(&appearance);

    let behavior = adw::PreferencesGroup::builder().title("Behavior").build();

    // A combo over `spec/04` §6's menu rather than a free number, because the interval is
    // a choice from a list -- and "Never" is the one value that is qualitatively different
    // from the rest, so it belongs at the top rather than hidden as a zero in a spinner.
    let labels: Vec<String> = AutoClose::CHOICES
        .iter()
        .map(|s| match *s {
            0 => "Never".to_owned(),
            n if n < 60 => format!("{n} seconds"),
            60 => "1 minute".to_owned(),
            n => format!("{} minutes", n / 60),
        })
        .collect();
    let auto_close = adw::ComboRow::builder()
        .title("Close cards automatically")
        .model(&gtk::StringList::new(
            &labels.iter().map(String::as_str).collect::<Vec<_>>(),
        ))
        .build();
    match with_key(app_settings, "qao-auto-close-seconds") {
        Some(s) => bind_auto_close(s, &auto_close),
        None => auto_close.set_sensitive(false),
    }
    behavior.add(&auto_close);

    let after_drag = adw::SwitchRow::builder()
        .title("Close after dragging out")
        .subtitle("Hold Alt while dropping to keep the card")
        .build();
    bind_boolean(app_settings, "qao-close-after-drag", &after_drag);
    behavior.add(&after_drag);

    let ask = adw::SwitchRow::builder()
        .title("Ask where to save")
        .subtitle("Otherwise Save writes straight to the export location")
        .build();
    bind_boolean(app_settings, "qao-ask-destination", &ask);
    behavior.add(&ask);

    // Every key the card binds (`qao/card.rs`), not three of them (D136).
    let shortcuts = adw::SwitchRow::builder()
        .title("Keyboard shortcuts on cards")
        .subtitle(
            "On the card under the pointer: Ctrl+C copies, Ctrl+S saves, Ctrl+E opens the \
             editor, Delete moves it to the Trash, Esc or Ctrl+W closes it, and Space \
             previews it. Hold Alt as you copy to keep the card",
        )
        .build();
    bind_boolean(app_settings, "qao-shortcuts", &shortcuts);
    behavior.add(&shortcuts);
    page.add(&behavior);

    page
}

/// The auto-close combo, which stores seconds rather than an index.
///
/// A value that is not on the menu -- someone's `gsettings set ... 45` -- selects the
/// nearest entry rather than resetting to the default, so opening Preferences does not
/// silently discard a preference it merely cannot display exactly.
fn bind_auto_close(settings: &gio::Settings, row: &adw::ComboRow) {
    let current = u32::try_from(settings.int("qao-auto-close-seconds")).unwrap_or(0);
    let snapped = AutoClose::snapped(current).seconds();
    let index = AutoClose::CHOICES.iter().position(|c| *c == snapped).unwrap_or(0);
    row.set_selected(u32::try_from(index).unwrap_or(0));

    let settings = settings.clone();
    row.connect_selected_notify(move |row| {
        let Some(seconds) = AutoClose::CHOICES.get(row.selected() as usize) else { return };
        if let Err(e) = settings.set_int("qao-auto-close-seconds", i32::try_from(*seconds).unwrap_or(0)) {
            warn!("could not store the auto-close interval: {e}");
        }
    });
}

// --- General -----------------------------------------------------------------------

/// `capture-sound`'s values and what Settings calls them, in the order the extension's
/// schema lists them (D134).
const SHUTTER_SOUNDS: [(&str, &str); 7] = [
    ("classic", "Classic"),
    ("pop", "Pop"),
    ("subtle", "Subtle"),
    ("8bit", "8-bit"),
    ("soft", "Soft"),
    ("system", "System sound"),
    ("none", "Silent"),
];
const SHUTTER_SOUNDS_WIRE: [&str; 7] = {
    let mut wire = [""; 7];
    let mut i = 0;
    while i < 7 {
        wire[i] = SHUTTER_SOUNDS[i].0;
        i += 1;
    }
    wire
};

/// The `'static` spelling of a `capture-sound` value that makes a sound, for
/// [`octosnap_shell::Cue::Shutter`]; `None` for silence or a value this build does not know.
pub(crate) fn shutter_kind(value: &str) -> Option<&'static str> {
    SHUTTER_SOUNDS_WIRE.iter().copied().find(|kind| *kind == value && *kind != "none")
}

fn general_page(
    app_settings: Option<&gio::Settings>,
    ext_settings: Option<&gio::Settings>,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("General")
        .name(PAGE_GENERAL)
        .icon_name("preferences-system-symbolic")
        .build();
    page.set_banner(Some(&extension_banner()));

    // --- The app itself: `SYS-02`.
    let app_group = adw::PreferencesGroup::builder().title("App").build();
    app_group.add(&launch_at_login_row());
    if let Some(row) = updates_row() {
        app_group.add(&row);
    }
    page.add(&app_group);

    // --- After Capture. spec/08 §0's matrix, with the one column this build has.
    let after = adw::PreferencesGroup::builder()
        .title("After Capture")
        .description(
            "What happens to a screenshot once it is taken. \
             Hold Ctrl while confirming a selection to copy as well.",
        )
        .build();
    for (action, title, subtitle) in [
        (
            AfterAction::ShowOverlay,
            "Show the overlay card",
            "A card in the corner with Copy, Save, Annotate and drag-out",
        ),
        (AfterAction::Copy, "Copy to clipboard", ""),
        (AfterAction::Save, "Save to the export location", ""),
        (AfterAction::Annotate, "Open in the editor", ""),
        (AfterAction::Pin, "Pin to the screen", "Where it was taken; the card comes back when the pin closes"),
    ] {
        let row = adw::SwitchRow::builder().title(title).build();
        if !subtitle.is_empty() {
            row.set_subtitle(subtitle);
        }
        // Every one of these the flow performs. Upload is not listed until M8 has
        // somewhere to upload to: a switch that can only be dimmed is what `spec/13` #19
        // rules out, as the card's Upload corner was (D133). Annotate and Pin were dimmed
        // here long after the flow had learned both.
        row.set_sensitive(app_settings.is_some());
        if let Some(s) = app_settings {
            bind_string_list_member(s, "after-screenshot", action.as_wire(), &row);
        }
        after.add(&row);
    }
    page.add(&after);

    // `spec/08` §1's "When Copy and Upload are both on" row is not here: it applies only
    // with Upload on, and Upload is not offered until M8 (D133). Its key,
    // `copy-upload-behavior`, and ACT-01's rule that reads it stay, so M8 adds a row and
    // nothing else.

    // --- Sounds and notifications.
    let feedback = adw::PreferencesGroup::builder().title("Feedback").build();

    let shutter = adw::ComboRow::builder()
        .title("Shutter sound")
        .subtitle("Plays the moment the pixels are read. The countdown ticks unless it is silent")
        .model(&gtk::StringList::new(&SHUTTER_SOUNDS.map(|(_, label)| label)))
        .build();
    const SHUTTER_ORDER: [&str; 7] = SHUTTER_SOUNDS_WIRE;
    match with_key(ext_settings, "capture-sound") {
        Some(s) => {
            bind_enum_row(s, "capture-sound", &shutter, &SHUTTER_ORDER, |v| v);
            // Each choice plays as it is chosen, since a list of names is no way to pick a
            // sound. Connected after the binding has set the row, so opening Settings is
            // quiet.
            shutter.connect_selected_notify(|row| {
                if let Some(kind) = SHUTTER_ORDER.get(row.selected() as usize).filter(|kind| **kind != "none") {
                    activate_app_action_with("preview-shutter", Some(&kind.to_variant()));
                }
            });
        }
        None => {
            shutter.set_sensitive(false);
            shutter.set_subtitle("Unavailable in the installed extension");
        }
    }
    feedback.add(&shutter);

    let notifications = adw::SwitchRow::builder()
        .title("Notifications")
        .subtitle(
            "For captures with no card, which would otherwise be silent. Text recognition \
             and failures always say what happened",
        )
        .build();
    bind_boolean(app_settings, "notifications", &notifications);
    feedback.add(&notifications);

    // `spec/08` §1's "Play sounds for recording/OCR/upload". Upload is left out of the
    // words until M8 has somewhere to upload to (`spec/13` #19), and copying and pinning
    // are in them, since they have the "tink" `spec/09` §4 gives them (D134).
    let ui_sounds = adw::SwitchRow::builder()
        .title("Action sounds")
        .subtitle("When a recording starts and stops, text is recognised, and a copy or a pin is made")
        .build();
    bind_boolean(app_settings, "ui-sounds", &ui_sounds);
    feedback.add(&ui_sounds);
    page.add(&feedback);

    // --- The shell half.
    let shell = adw::PreferencesGroup::builder()
        .title("Shell")
        .description("Provided by the OctoSnap GNOME Shell extension")
        .build();

    let indicator = adw::SwitchRow::builder()
        .title("Show the indicator in the top bar")
        .subtitle("Its menu has every capture mode")
        .build();
    bind_boolean(ext_settings, "show-indicator", &indicator);
    shell.add(&indicator);
    page.add(&shell);

    page
}

/// `spec/08` §1's "Launch at login" (`SYS-02`), which is an autostart entry rather than
/// a key: the file in `~/.config/autostart` is what the desktop reads and what GNOME
/// Settings' Startup Applications lists, so a key beside it would be a second truth that
/// can disagree with the first. `core::autostart` says what the file holds.
/// `spec/13` #15 and `spec/09` §4b's "the settings pane before the extension is enabled":
/// a banner over the first page whenever the extension is not ready, saying what is wrong
/// in the welcome window's words. Its button is the remedy itself -- Log Out…, Turn On,
/// Install… -- rather than the way to a window that has it, and that window, with the
/// whole story, only where there is no remedy to press or the remedy failed.
fn extension_banner() -> adw::Banner {
    use crate::setup::{Remedy, Status};
    fn show(banner: &adw::Banner, found: &Status) {
        let button = found.remedy().map_or("Details\u{2026}", Remedy::label);
        banner.set_title(&found.banner());
        banner.set_button_label(Some(button));
        banner.set_revealed(!found.ready());
        info!(revealed = !found.ready(), button, "settings banner shown");
    }

    let banner = adw::Banner::builder().revealed(false).build();
    let Some(connection) = gio::Application::default().and_then(|app| app.dbus_connection()) else {
        return banner;
    };
    let current: std::rc::Rc<std::cell::RefCell<Option<Status>>> = std::rc::Rc::default();
    {
        let (connection, current) = (connection.clone(), std::rc::Rc::clone(&current));
        banner.connect_button_clicked(move |banner| {
            let Some(remedy) = current.borrow().as_ref().and_then(Status::remedy) else {
                activate_app_action("welcome");
                return;
            };
            let (connection, current, banner) =
                (connection.clone(), std::rc::Rc::clone(&current), banner.clone());
            glib::spawn_future_local(async move {
                info!(?remedy, "settings banner remedy");
                if let Err(why) = crate::setup::apply(&connection, remedy).await {
                    // The welcome window has the manual route, and asks again itself.
                    warn!(?remedy, "the banner's remedy failed: {why}");
                    activate_app_action("welcome");
                    return;
                }
                if remedy == Remedy::LogOut {
                    return;
                }
                // GNOME Shell's answer is not the new state; the welcome window's reason.
                glib::timeout_future(std::time::Duration::from_millis(600)).await;
                let found = crate::setup::status(&connection).await;
                info!(?found, "settings banner after its remedy");
                show(&banner, &found);
                *current.borrow_mut() = Some(found);
            });
        });
    }
    let shown = banner.clone();
    glib::spawn_future_local(async move {
        let found = crate::setup::status(&connection).await;
        info!(?found, "settings banner");
        show(&shown, &found);
        *current.borrow_mut() = Some(found);
    });
    banner
}

/// Runs one of the application's own actions from a button in this dialog.
///
/// Not `action-name`: the dialog is presented with no parent, so its window belongs to no
/// `GtkApplication`, an `app.` name resolves to nothing, and GTK greys the button out
/// (the banner's Set Up did exactly that).
fn activate_app_action(name: &str) {
    activate_app_action_with(name, None);
}

/// [`activate_app_action`] with a parameter. On the application, not through the widget:
/// the dialog is presented with no window of the application's above it, so an `app.`
/// action name on a widget in it finds no `app` group.
fn activate_app_action_with(name: &str, parameter: Option<&glib::Variant>) {
    match gio::Application::default() {
        Some(app) => app.activate_action(name, parameter),
        None => warn!(name, "no application to run the action on"),
    }
}

/// A button that runs `action` on the application; see [`activate_app_action`].
fn app_action_button(label: &str, action: &'static str) -> gtk::Button {
    let button = gtk::Button::builder().label(label).valign(gtk::Align::Center).build();
    button.connect_clicked(move |_| activate_app_action(action));
    button
}

/// D170: what the portal found, and the button that installs it. Only where the app can
/// update itself, a Flatpak; a native build's updates are its package manager's, and the
/// row is not there.
fn updates_row() -> Option<adw::ActionRow> {
    use crate::update::State;
    fn show(row: &adw::ActionRow, button: &gtk::Button, spinner: &adw::Spinner, state: &State) {
        let line = match state {
            State::Current => format!("OctoSnap {}. {}", env!("CARGO_PKG_VERSION"), state.subtitle()),
            _ => state.subtitle(),
        };
        row.set_subtitle(&line);
        row.set_visible(*state != State::Unavailable);
        button.set_visible(state.installable());
        spinner.set_visible(matches!(state, State::Installing { .. }));
    }

    let state = crate::update::state();
    if state == State::Unavailable {
        return None;
    }
    let row = adw::ActionRow::builder().title("Updates").build();
    let spinner = adw::Spinner::new();
    let button = gtk::Button::builder()
        .label("Update")
        .valign(gtk::Align::Center)
        .css_classes(["suggested-action"])
        .build();
    button.connect_clicked(|_| crate::update::install());
    row.add_suffix(&spinner);
    row.add_suffix(&button);
    show(&row, &button, &spinner, &state);
    info!(?state, "settings' updates row");
    let weak = (row.downgrade(), button.downgrade(), spinner.downgrade());
    crate::update::watch(move |state| {
        let (Some(row), Some(button), Some(spinner)) = (weak.0.upgrade(), weak.1.upgrade(), weak.2.upgrade()) else {
            return false;
        };
        show(&row, &button, &spinner, state);
        info!(?state, "settings' updates row");
        true
    });
    Some(row)
}

pub(crate) fn launch_at_login_row() -> adw::SwitchRow {
    const SUBTITLE: &str = "Starts the service with the session, so the first capture is not a cold start";
    let row = adw::SwitchRow::builder().title("Launch at login").subtitle(SUBTITLE).build();
    if !settings::sandboxed() {
        row.set_active(autostart_enabled());
        row.connect_active_notify(|row| set_autostart(row.is_active()));
        return row;
    }
    // D126: from a sandbox the portal writes the entry, and says afterwards whether it did.
    row.set_active(settings::Settings::load().launch_at_login());
    let answering = std::rc::Rc::new(std::cell::Cell::new(false));
    row.connect_active_notify(move |row| {
        if answering.get() {
            return;
        }
        let wanted = row.is_active();
        row.set_sensitive(false);
        let (row, answering) = (row.clone(), std::rc::Rc::clone(&answering));
        crate::background::set("", wanted, move |answer| {
            row.set_sensitive(true);
            let now = match answer {
                Ok(answer) => {
                    if answer.background {
                        row.set_subtitle(SUBTITLE);
                    } else {
                        row.set_subtitle("Not allowed to run in the background. Settings → Apps → OctoSnap can allow it");
                    }
                    answer.autostart
                }
                Err(_) => settings::Settings::load().launch_at_login(),
            };
            if row.is_active() != now {
                answering.set(true);
                row.set_active(now);
                answering.set(false);
            }
        });
    });
    row
}

fn autostart_path() -> std::path::PathBuf {
    octosnap_core::autostart::path(&glib::user_config_dir())
}

fn autostart_enabled() -> bool {
    std::fs::read_to_string(autostart_path())
        .map(|contents| octosnap_core::autostart::is_enabled(&contents))
        .unwrap_or(false)
}

/// D128: an entry an earlier build wrote starts the service without the malloc settings,
/// and the one start that matters for it is the login. Brought up to date where it is,
/// naming the same binary. A Flatpak's entry is the Background portal's (D126), and the
/// manifest's `--env` covers it.
pub(crate) fn refresh_autostart() {
    if settings::sandboxed() {
        return;
    }
    let path = autostart_path();
    let Some(updated) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|contents| octosnap_core::autostart::refresh(&contents))
    else {
        return;
    };
    match std::fs::write(&path, updated) {
        Ok(()) => info!(path = %path.display(), "launch at login now starts with the malloc settings"),
        Err(e) => warn!(path = %path.display(), "could not update launch at login: {e}"),
    }
}

fn set_autostart(on: bool) {
    let path = autostart_path();
    if on == autostart_enabled() {
        return;
    }
    let result = if on {
        std::env::current_exe()
            .and_then(|exe| {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::write(&path, octosnap_core::autostart::entry(&exe))
            })
    } else {
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    };
    match result {
        Ok(()) => info!(on, path = %path.display(), "launch at login"),
        Err(e) => warn!(on, path = %path.display(), "could not change launch at login: {e}"),
    }
}

// --- Screenshots -------------------------------------------------------------------

fn screenshots_page(
    app_settings: Option<&gio::Settings>,
    ext_settings: Option<&gio::Settings>,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Screenshots")
        .name(PAGE_SCREENSHOTS)
        .icon_name("camera-photo-symbolic")
        .build();

    // --- Output. spec/08 §6.
    let output = adw::PreferencesGroup::builder().title("Output").build();

    let format = adw::ComboRow::builder()
        .title("File format")
        .model(&gtk::StringList::new(&["PNG", "JPEG", "WebP"]))
        .build();
    const FORMAT_ORDER: [ImageFormat; 3] = [ImageFormat::Png, ImageFormat::Jpg, ImageFormat::Webp];
    match with_key(app_settings, "shot-format") {
        Some(s) => bind_enum_row(s, "shot-format", &format, &FORMAT_ORDER, |f| f.as_wire()),
        None => format.set_sensitive(false),
    }
    output.add(&format);

    let quality = adw::SpinRow::builder()
        .title("JPEG quality")
        .adjustment(&gtk::Adjustment::new(90.0, 1.0, 100.0, 1.0, 10.0, 0.0))
        .build();
    bind_int(app_settings, "shot-jpg-quality", &quality);
    output.add(&quality);
    // spec/08 §0 point 1: a dependent row stays on screen and goes insensitive, so the
    // user learns that choosing JPEG will give them a quality control.
    // The format from the signal: a copy of it held in its own handler kept it (D139).
    // The subtitle says what the choice costs (D164): JPEG has nowhere to keep a window's
    // see-through shadow and corners, and WebP is lossless here, not a smaller JPEG.
    let follow_format = {
        let quality = quality.clone();
        let available = app_settings.is_some();
        move |format: &adw::ComboRow| {
            let chosen = FORMAT_ORDER.get(format.selected() as usize).copied().unwrap_or_default();
            quality.set_sensitive(chosen == ImageFormat::Jpg && available);
            format.set_subtitle(match chosen {
                ImageFormat::Png => "Lossless, and keeps transparency",
                ImageFormat::Jpg => "Smallest files. Transparent parts become white",
                ImageFormat::Webp => "Lossless and keeps transparency, smaller than PNG",
            });
        }
    };
    follow_format(&format);
    format.connect_selected_notify(follow_format);

    let retina = adw::SwitchRow::builder()
        .title("Add \u{201c}@2x\u{201d} to names of HiDPI captures")
        .build();
    bind_boolean(app_settings, "retina-suffix", &retina);
    output.add(&retina);
    page.add(&output);

    // --- Export location. spec/08 §0 point 3: one setting, not one per surface.
    let location = adw::PreferencesGroup::builder()
        .title("Export location")
        .description("Used by Save, the overlay card and every after-capture action")
        .build();
    location.add(&folder_row(app_settings, "screenshot-folder", || {
        octosnap_core::savepath::screenshot_dir(None, &pictures_dir())
    }));
    page.add(&location);

    // --- File names. ACT-07.
    let names = adw::PreferencesGroup::builder()
        .title("File names")
        .description(
            "Tokens: {yyyy} {yy} {MM} {MMM} {MMMM} {dd} {ddd} {HH} {hh} {mm} {ss} {a} \
             {n} {app} {window} {type} {w} {h}",
        )
        .build();

    let template = adw::EntryRow::builder().title("Name template").build();
    bind_text(app_settings, "filename-template", &template);
    names.add(&template);

    let counter_start = adw::SpinRow::builder()
        .title("Counter starts at")
        .subtitle("The value {n} takes on the next capture")
        .adjustment(&gtk::Adjustment::new(1.0, 0.0, 1_000_000.0, 1.0, 100.0, 0.0))
        .build();
    bind_int(app_settings, "filename-counter-start", &counter_start);
    names.add(&counter_start);

    let counter_width = adw::SpinRow::builder()
        .title("Counter digits")
        .subtitle("2 renders {n} as 01, 02, 03")
        .adjustment(&gtk::Adjustment::new(1.0, 1.0, 4.0, 1.0, 1.0, 0.0))
        .build();
    bind_int(app_settings, "filename-counter-width", &counter_width);
    names.add(&counter_width);

    let utc = adw::SwitchRow::builder()
        .title("Use UTC in file names")
        .subtitle("Off means local time")
        .build();
    bind_boolean(app_settings, "filename-utc", &utc);
    names.add(&utc);
    page.add(&names);

    // --- Capture behaviour: the extension's half, since it acts at grab time.
    let capture = adw::PreferencesGroup::builder()
        .title("Capture")
        .description("Provided by the OctoSnap GNOME Shell extension")
        .build();

    // `spec/08` §1's "Self-Timer interval [5 Seconds]", first as the spec lists it
    // (D136).
    let timer = adw::SpinRow::builder()
        .title("Self-timer")
        .subtitle("Seconds it counts down before the capture")
        .adjustment(&gtk::Adjustment::new(5.0, 1.0, 10.0, 1.0, 1.0, 0.0))
        .build();
    match with_key(ext_settings, "shot-timer") {
        Some(s) => s.bind("shot-timer", &timer, "value").build(),
        None => {
            timer.set_sensitive(false);
            timer.set_subtitle("Unavailable in the installed extension");
        }
    }
    capture.add(&timer);

    let cursor = adw::SwitchRow::builder().title("Include the pointer").build();
    bind_boolean(ext_settings, "shot-cursor", &cursor);
    capture.add(&cursor);

    let freeze = adw::SwitchRow::builder()
        .title("Freeze the screen while selecting")
        .subtitle("Stops animations and video moving under the selection")
        .build();
    bind_boolean(ext_settings, "shot-freeze", &freeze);
    capture.add(&freeze);

    // `CAP-12`. The extension's, like every setting read at grab time (D11), and the
    // extension is what puts the icons back whatever happens to the app (`spec/07` §7
    // item 7).
    let hide_desktop = adw::SwitchRow::builder()
        .title("Hide desktop icons while capturing")
        .subtitle("They come back the moment the capture is done")
        .build();
    bind_boolean(ext_settings, "shot-hide-desktop", &hide_desktop);
    capture.add(&hide_desktop);

    let crosshair = adw::ComboRow::builder()
        .title("Crosshair and magnifier")
        .model(&gtk::StringList::new(&[
            // The modifier is `CROSSHAIR_MODIFIER` in `extension/src/overlay/engine.ts`,
            // and a row that did not name it left the reader to find out which (D136).
            "Only while holding Ctrl",
            "Always",
            "Never",
        ]))
        .build();
    const CROSSHAIR_ORDER: [&str; 3] = ["modifier", "always", "off"];
    match with_key(ext_settings, "shot-crosshair") {
        Some(s) => bind_enum_row(s, "shot-crosshair", &crosshair, &CROSSHAIR_ORDER, |v| v),
        None => {
            crosshair.set_sensitive(false);
            crosshair.set_subtitle("Unavailable in the installed extension");
        }
    }
    capture.add(&crosshair);

    let magnifier = adw::SwitchRow::builder()
        .title("Magnify pixels under the crosshair")
        .build();
    bind_boolean(ext_settings, "shot-magnifier", &magnifier);
    capture.add(&magnifier);
    // Another dependent row: no crosshair means no magnifier to show.
    let follow_crosshair = {
        let magnifier = magnifier.clone();
        let available = ext_settings.is_some();
        move |crosshair: &adw::ComboRow| {
            let off = CROSSHAIR_ORDER.get(crosshair.selected() as usize) == Some(&"off");
            magnifier.set_sensitive(available && !off);
        }
    };
    follow_crosshair(&crosshair);
    crosshair.connect_selected_notify(follow_crosshair);

    // After Capture is on the General page, not above this row, where the title used to
    // point (D136).
    let shortcuts_respect = adw::SwitchRow::builder()
        .title("\u{201c}Capture Area and \u{2026}\u{201d} shortcuts add to the After Capture actions")
        .subtitle("Those are on the General page. Off means such a shortcut does only what its name says")
        .build();
    bind_boolean(app_settings, "area-shortcuts-respect-actions", &shortcuts_respect);
    capture.add(&shortcuts_respect);
    page.add(&capture);

    page.add(&window_captures_group(app_settings));

    // `spec/07` §3.1's look for a pinned screenshot, "both switchable": the corners and
    // the shadow, and the border beside them. The shadow is the compositor's (D132).
    let pins = adw::PreferencesGroup::builder()
        .title("Pinned screenshots")
        .description("How a capture pinned to the screen is framed; applies to new pins")
        .build();
    let rounded = adw::SwitchRow::builder().title("Rounded corners").build();
    bind_boolean(app_settings, "pin-rounded-corners", &rounded);
    pins.add(&rounded);
    let shadow = adw::SwitchRow::builder().title("Shadow").build();
    bind_boolean(app_settings, "pin-shadow", &shadow);
    pins.add(&shadow);
    let border = adw::SwitchRow::builder()
        .title("Thin border")
        .subtitle("Keeps a pale capture apart from a pale desktop")
        .build();
    bind_boolean(app_settings, "pin-border", &border);
    pins.add(&border);
    page.add(&pins);

    page
}

/// `spec/08` §2's Wallpaper table, which is about window captures and lives here.
///
/// Four of the six rows are a choice about what is drawn *behind* the window, and they
/// end up as `spec/05` §4.13's parameters on an ordinary background object (D90) -- which
/// is why the editor's panel can change any of them afterwards and this page is only the
/// default. The other two are about what the compositor already put in the file, and they
/// are the reason this group carries prose: `window-shadow` can subtract the shadow it
/// finds (D93) and `window-rounded-corners` cannot put back pixels that were never taken.
fn window_captures_group(app_settings: Option<&gio::Settings>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Window captures")
        .description(
            "A window is captured with its own transparent edges. This is what goes \
             behind it, and the annotation editor can change it afterwards.",
        )
        .build();

    let background = adw::ComboRow::builder()
        .title("Background")
        .model(&gtk::StringList::new(&[
            "Transparent",
            "Desktop wallpaper",
            "An image",
            "A colour",
        ]))
        .build();
    const BACKGROUND_ORDER: [&str; 4] = ["transparent", "wallpaper", "image", "color"];
    match with_key(app_settings, "window-background") {
        Some(s) => bind_enum_row(s, "window-background", &background, &BACKGROUND_ORDER, |v| v),
        None => background.set_sensitive(false),
    }
    group.add(&background);

    let image = image_row(app_settings, "window-background-image");
    group.add(&image);
    let colour = colour_row(app_settings, "window-background-color");
    group.add(&colour);

    // The two rows only one choice apiece uses, dimmed rather than hidden: a row that
    // appears and disappears as a menu changes is harder to find a second time than one
    // that is always in the same place saying why it is unavailable.
    // Weakly: the settings hold this, and both rows' own handlers hold the settings (D139).
    let follow = {
        let (image, colour) = (image.downgrade(), colour.downgrade());
        move |chosen: &str| {
            if let Some(image) = image.upgrade() {
                image.set_sensitive(chosen == "image");
            }
            if let Some(colour) = colour.upgrade() {
                colour.set_sensitive(chosen == "color");
            }
        }
    };
    match with_key(app_settings, "window-background") {
        Some(settings) => {
            follow(settings.string("window-background").as_str());
            watch(settings, "window-background", &group, move |_, settings| {
                follow(settings.string("window-background").as_str());
            });
        }
        None => follow(""),
    }

    let padding = adw::SpinRow::builder()
        .title("Padding")
        .subtitle("Room around the window, in points")
        .adjustment(&gtk::Adjustment::new(40.0, 0.0, 400.0, 4.0, 20.0, 0.0))
        .build();
    bind_int(app_settings, "window-padding", &padding);
    group.add(&padding);

    let shadow = adw::SwitchRow::builder()
        .title("Keep the window's shadow")
        .subtitle("Off crops the capture to the window itself")
        .build();
    bind_boolean(app_settings, "window-shadow", &shadow);
    group.add(&shadow);

    // Honest, and dimmed for the reason in D93: the corners are cut out of the alpha
    // before the file exists, and what was behind them was never captured.
    let corners = adw::SwitchRow::builder()
        .title("Keep the window's rounded corners")
        .subtitle(
            "Always on: GNOME cuts the corners before the screenshot is written, so what \
             was behind them is not in the file",
        )
        .active(true)
        .sensitive(false)
        .build();
    group.add(&corners);

    group
}

/// An image-file row: the chosen file's name, and a chooser that filters to images.
fn image_row(app_settings: Option<&gio::Settings>, key: &'static str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title("Image").build();
    let choose =
        gtk::Button::builder().label("Choose\u{2026}").valign(gtk::Align::Center).build();
    row.add_suffix(&choose);
    row.set_activatable_widget(Some(&choose));

    let Some(settings) = with_key(app_settings, key).cloned() else {
        row.set_subtitle("Unavailable: the schema is not installed");
        row.set_sensitive(false);
        return row;
    };

    let refresh = move |row: &adw::ActionRow, settings: &gio::Settings| {
        let chosen = settings.string(key);
        row.set_subtitle(if chosen.is_empty() { "None chosen" } else { chosen.as_str() });
    };
    refresh(&row, &settings);
    watch(&settings, key, &row, refresh);

    // The window from the button itself: the row held here, whose suffix the button is,
    // was a ring that kept the row (D139).
    choose.connect_clicked(move |choose| {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Images"));
        for mime in ["image/png", "image/jpeg", "image/webp", "image/avif"] {
            filter.add_mime_type(mime);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog =
            gtk::FileDialog::builder().title("Background image").filters(&filters).build();
        let settings = settings.clone();
        dialog.open(
            choose.root().and_downcast_ref::<gtk::Window>(),
            gio::Cancellable::NONE,
            move |result| match result {
                Ok(file) => {
                    if let Some(path) = file.path() {
                        wrote(key, settings.set_string(key, &path.to_string_lossy()));
                    }
                }
                // Dismissing a file chooser is the normal case, not a failure.
                Err(e) => info!("no image chosen: {e}"),
            },
        );
    });
    row
}

/// A colour row, stored as the `#rrggbb` string `spec/08` §2's key holds.
fn colour_row(app_settings: Option<&gio::Settings>, key: &'static str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title("Colour").build();
    let button = gtk::ColorDialogButton::builder()
        .dialog(&gtk::ColorDialog::new())
        .valign(gtk::Align::Center)
        .build();
    row.add_suffix(&button);
    row.set_activatable_widget(Some(&button));

    let Some(settings) = with_key(app_settings, key).cloned() else {
        row.set_subtitle("Unavailable: the schema is not installed");
        row.set_sensitive(false);
        return row;
    };

    // Guards the button against its own signal, the same way the tool strip does: setting
    // the colour to what the setting says emits `rgba` again.
    let syncing = std::rc::Rc::new(std::cell::Cell::new(false));
    let refresh = {
        let syncing = syncing.clone();
        move |button: &gtk::ColorDialogButton, settings: &gio::Settings| {
            let text = settings.string(key);
            let Ok(rgba) = text.parse::<gtk::gdk::RGBA>() else { return };
            syncing.set(true);
            button.set_rgba(&rgba);
            syncing.set(false);
        }
    };
    refresh(&button, &settings);
    watch(&settings, key, &button, refresh);

    button.connect_rgba_notify(move |button| {
        if syncing.get() {
            return;
        }
        let rgba = button.rgba();
        let hex = format!(
            "#{:02x}{:02x}{:02x}",
            (rgba.red() * 255.0).round() as u8,
            (rgba.green() * 255.0).round() as u8,
            (rgba.blue() * 255.0).round() as u8,
        );
        wrote(key, settings.set_string(key, &hex));
    });
    row
}

/// The export-location row: a folder chooser whose subtitle shows where captures
/// actually go, including the XDG default when the setting is empty.
/// `spec/08` §5's Recording page, at what M5 actually does: GIF capture (D68). No audio,
/// no camera, no video codec, no trim -- those are M9. `gif-optimize` is absent because
/// gifsicle is not installed (D68); gifski's own quantisation is the only pass there is.
fn recording_page(
    app_settings: Option<&gio::Settings>,
    ext_settings: Option<&gio::Settings>,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Recording")
        .name(PAGE_RECORDING)
        .icon_name("camera-video-symbolic")
        .build();

    // --- GIF. The app owns these: it owns the stream and the gifski encode.
    let gif = adw::PreferencesGroup::builder()
        .title("GIF")
        .description("OctoSnap records GIFs; video recording is coming in a later release")
        .build();

    let fps = adw::ComboRow::builder()
        .title("Frame rate")
        .subtitle("More frames a second is smoother and larger; a GIF plays at most 50")
        .model(&gtk::StringList::new(&[
            "10 fps",
            "15 fps",
            "24 fps",
            "30 fps",
            "50 fps",
            "Match the screen",
        ]))
        .build();
    match with_key(app_settings, "gif-fps") {
        Some(s) => bind_int_choices(s, "gif-fps", &fps, &GIF_FPS_VALUES, 1),
        None => fps.set_sensitive(false),
    }
    gif.add(&fps);

    let max_width = adw::SpinRow::builder()
        .title("Maximum width")
        .subtitle("Pixels; 0 records at the selection\u{2019}s own width")
        .adjustment(&gtk::Adjustment::new(1280.0, 0.0, 7680.0, 10.0, 100.0, 0.0))
        .build();
    bind_int(app_settings, "gif-max-width", &max_width);
    gif.add(&max_width);

    let quality = adw::SpinRow::builder()
        .title("Quality")
        .subtitle("How much effort gifski spends on the palette, 1 to 100")
        .adjustment(&gtk::Adjustment::new(80.0, 1.0, 100.0, 1.0, 10.0, 0.0))
        .build();
    bind_int(app_settings, "gif-quality", &quality);
    gif.add(&quality);
    page.add(&gif);

    // --- Recording behaviour. The cursor is the app's (it sets Mutter's cursor-mode on
    // the stream); the countdown and the desktop-icon toggle are the extension's, run on
    // the compositor at record time (D11).
    let behaviour = adw::PreferencesGroup::builder().title("Recording").build();

    let cursor = adw::SwitchRow::builder().title("Include the pointer").build();
    bind_boolean(app_settings, "rec-cursor", &cursor);
    behaviour.add(&cursor);

    let countdown = adw::SpinRow::builder()
        .title("Countdown")
        .subtitle("Seconds before recording starts; 0 for none")
        .adjustment(&gtk::Adjustment::new(3.0, 0.0, 10.0, 1.0, 1.0, 0.0))
        .build();
    bind_int(ext_settings, "rec-countdown", &countdown);
    behaviour.add(&countdown);

    let hide_desktop = adw::SwitchRow::builder()
        .title("Hide desktop icons while recording")
        .subtitle("They come back when the recording ends, whatever happens to the app")
        .build();
    bind_boolean(ext_settings, "rec-hide-desktop", &hide_desktop);
    behaviour.add(&hide_desktop);
    page.add(&behaviour);

    // --- Export location.
    let location = adw::PreferencesGroup::builder()
        .title("Export location")
        .description("Where a recording is saved, and where Save on its card writes")
        .build();
    location.add(&folder_row(app_settings, "recording-folder", || {
        octosnap_core::savepath::recording_dir(None, &videos_dir())
    }));
    page.add(&location);

    // --- After Recording. A GIF has no editor (M9's video trim), so Annotate is not here.
    let after = adw::PreferencesGroup::builder()
        .title("After Recording")
        .description("What happens to a GIF once the recording stops")
        .build();
    for (action, title, subtitle) in [
        (AfterAction::ShowOverlay, "Show the overlay card", "A card in the corner with Copy, Save and drag-out"),
        (AfterAction::Copy, "Copy to clipboard", ""),
        (AfterAction::Save, "Save to the export location", ""),
    ] {
        let row = adw::SwitchRow::builder().title(title).build();
        if !subtitle.is_empty() {
            row.set_subtitle(subtitle);
        }
        // Upload comes back with M8, as on the screenshot side.
        row.set_sensitive(app_settings.is_some());
        if let Some(s) = app_settings {
            bind_string_list_member(s, "after-recording", action.as_wire(), &row);
        }
        after.add(&row);
    }
    page.add(&after);

    page
}

fn folder_row(
    app_settings: Option<&gio::Settings>,
    key: &'static str,
    default_dir: fn() -> std::path::PathBuf,
) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title("Folder").build();
    let choose = gtk::Button::builder()
        .label("Choose\u{2026}")
        .valign(gtk::Align::Center)
        .build();
    row.add_suffix(&choose);
    row.set_activatable_widget(Some(&choose));

    let Some(settings) = with_key(app_settings, key).cloned() else {
        row.set_subtitle("Unavailable: the schema is not installed");
        row.set_sensitive(false);
        return row;
    };

    let refresh = move |row: &adw::ActionRow, settings: &gio::Settings| {
        let configured = settings.string(key);
        row.set_subtitle(&if configured.is_empty() {
            let default = default_dir();
            format!("{} (default)", default.display())
        } else {
            configured.to_string()
        });
    };
    refresh(&row, &settings);
    watch(&settings, key, &row, refresh);

    {
        let settings = settings.clone();
        // The window from the button itself, as `image_row` takes it (D139).
        choose.connect_clicked(move |choose| {
            let dialog = gtk::FileDialog::builder().title("Export location").build();
            let start = settings.string(key);
            if !start.is_empty() {
                dialog.set_initial_folder(Some(&gio::File::for_path(start.as_str())));
            }
            let settings = settings.clone();
            dialog.select_folder(
                choose.root().and_downcast_ref::<gtk::Window>(),
                gio::Cancellable::NONE,
                move |result| match result {
                    Ok(folder) => {
                        if let Some(path) = folder.path() {
                            wrote(key, settings.set_string(key, &path.to_string_lossy()));
                        }
                    }
                    // Dismissing a file chooser is the normal case, not a failure.
                    Err(e) => info!("no folder chosen: {e}"),
                },
            );
        });
    }

    row
}

fn pictures_dir() -> std::path::PathBuf {
    glib::user_special_dir(glib::UserDirectory::Pictures)
        .unwrap_or_else(|| glib::home_dir().join("Pictures"))
}

fn videos_dir() -> std::path::PathBuf {
    glib::user_special_dir(glib::UserDirectory::Videos)
        .unwrap_or_else(|| glib::home_dir().join("Videos"))
}

// --- Shortcuts ---------------------------------------------------------------------

/// `spec/08` §3's action list, in its groups, limited to what this build can do.
///
/// A row for an action the extension does not implement would record a binding that
/// never fires, which is the same dishonesty as a switch with nothing behind it. A row
/// for an action the *app* serves but the installed extension's schema does not yet
/// carry renders insensitive (`with_key`), which is the honest state between the two
/// halves' releases. Text capture is the one still missing, and arrives with it.
const SHORTCUT_GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "Screenshots",
        &[
            ("all-in-one", "All-In-One"),
            ("capture-area", "Capture Area"),
            ("capture-window", "Capture Window"),
            ("capture-fullscreen", "Capture Fullscreen"),
            ("capture-previous-area", "Capture Previous Area"),
            ("self-timer", "Self-Timer"),
        ],
    ),
    (
        "Capture Area and \u{2026}",
        &[
            ("capture-area-copy", "\u{2026} Copy to Clipboard"),
            ("capture-area-save", "\u{2026} Save"),
            ("capture-area-annotate", "\u{2026} Annotate"),
            // "… Upload" comes back with M8's providers (D133); until then the key it
            // would bind could only capture and say that it cannot upload.
            ("capture-area-pin", "\u{2026} Pin to the Screen"),
        ],
    ),
    (
        "Recording and Scrolling",
        &[
            ("record-gif", "Record a GIF"),
            ("recording-stop", "Stop the Recording"),
            ("scrolling-capture", "Scrolling Capture"),
        ],
    ),
    // `spec/07` §2.1's two, which differ by one boolean that rides on the capture rather
    // than by a setting either of them writes -- so they are two shortcuts and not one
    // shortcut and a switch. `spec/08` §3 lists both under Screenshots; they get a group
    // of their own because the pair only makes sense read together.
    (
        "Text Recognition",
        &[
            ("capture-text", "Capture Text"),
            ("capture-text-single-line", "Capture Text as One Line"),
        ],
    ),
    (
        "Overlays and Pins",
        &[
            ("overlays-toggle-visibility", "Hide or Show the Cards"),
            ("overlays-save-all", "Save All Cards"),
            ("overlays-close-all", "Close All Cards"),
            ("restore-recently-closed", "Restore Recently Closed"),
            ("pins-toggle-visibility", "Hide or Show Pinned Screenshots"),
            ("pins-close-all", "Close All Pinned Screenshots"),
        ],
    ),
    (
        "The Last Capture",
        &[
            ("copy-last-capture", "Copy the Last Capture"),
            ("save-last-capture", "Save the Last Capture"),
            ("annotate-last-capture", "Annotate the Last Capture"),
            ("open-from-clipboard", "Annotate the Clipboard's Image"),
        ],
    ),
    (
        "Other",
        &[
            ("open-history", "Open the Capture History"),
            ("toggle-desktop-icons", "Hide or Show Desktop Icons"),
            ("toggle-pets", "Hide or Show the Desktop Pets"),
            ("open-settings", "Open OctoSnap Settings"),
        ],
    ),
];

/// A page's whole content while the extension its keys belong to is not there.
///
/// `spec/09` §4b's "the settings pane before the extension is enabled": what is missing
/// and the way to it, rather than a group titled Unavailable. Setup is the way, since it
/// knows which of a missing, off or silent extension this is. `whose` says what the page
/// has to do with the extension, and the rest of the sentence says what is wrong with it.
fn needs_extension(title: &str, whose: &str) -> adw::PreferencesGroup {
    let status = adw::StatusPage::builder()
        .icon_name("io.github.odrakirmusic.OctoSnap-symbolic")
        .title(title)
        .description(if settings::sandboxed() {
            format!("{whose}, and the extension is not answering.")
        } else {
            format!("{whose}, and the extension is not installed.")
        })
        .build();
    let set_up = app_action_button("Set Up\u{2026}", "welcome");
    set_up.set_halign(gtk::Align::Center);
    set_up.add_css_class("pill");
    set_up.add_css_class("suggested-action");
    status.set_child(Some(&set_up));
    let group = adw::PreferencesGroup::new();
    group.add(&status);
    group
}

fn shortcuts_page(ext_settings: Option<&gio::Settings>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Shortcuts")
        .name(PAGE_SHORTCUTS)
        .icon_name("preferences-desktop-keyboard-symbolic")
        .build();

    let Some(settings) = ext_settings else {
        page.add(&needs_extension(
            "Shortcuts come with the extension",
            "OctoSnap's keys are its GNOME Shell extension's",
        ));
        return page;
    };

    page.add(&print_keys_group(settings, true));

    for (title, rows) in SHORTCUT_GROUPS {
        let group = adw::PreferencesGroup::builder().title(*title).build();
        if *title == "Capture Area and \u{2026}" {
            group.set_description(Some(
                "Unbound by default. These replace the after-capture actions unless \
                 the Screenshots page says otherwise.",
            ));
        }
        for (key, label) in *rows {
            match with_key(Some(settings), key) {
                Some(settings) => group.add(&shortcut_row(settings, key, label)),
                None => {
                    let row = adw::ActionRow::builder()
                        .title(*label)
                        .subtitle("Unavailable in the installed extension")
                        .sensitive(false)
                        .build();
                    group.add(&row);
                }
            }
        }
        page.add(&group);
    }

    let note = adw::PreferencesGroup::builder()
        .description(
            "Changing a shortcut takes effect immediately. A key GNOME Shell, the window \
             manager or the media keys already use is refused and the row says which.",
        )
        .build();
    page.add(&note);
    page
}

/// `spec/08` §3's onboarding step, as a row that works in both directions: "Use
/// OctoSnap for the system screenshot keys" rebinds Print, Shift+Print and Alt+Print
/// to All-In-One, Capture Area and Capture Window and clears GNOME's three entries;
/// the same row gives them back. `core::shortcuts` says exactly what is written, and
/// whether the take-over is in force is read from both schemas rather than remembered.
/// The Recording page's frame rates, in the order its row lists them.
///
/// Fifty and not sixty, because a GIF stores its delays in hundredths of a second and
/// gifski writes nothing shorter than two: sixty does not drop frames, it stretches time
/// (`GIF_FPS_MAX`). "Match the screen" is stored as zero and resolved against the recorded
/// monitor's refresh rate, divided until it is under the same ceiling so that every frame
/// is the same number of refreshes after the last (D101, D111). The GIF editor offers the
/// same rates below a GIF's own (D133).
pub(crate) const GIF_FPS_VALUES: [i32; 6] = [10, 15, 24, 30, 50, 0];

/// `accent`: whether "Use Them for OctoSnap" is the suggested action. In Settings it is
/// the page's one; in the welcome window the setup's remedy and Done are.
pub(crate) fn print_keys_group(ext: &gio::Settings, accent: bool) -> adw::PreferencesGroup {
    use octosnap_core::shortcuts::{
        GNOME_SHELL_KEYBINDINGS, PRINT_KEYS, Write, give_back_print_keys, print_keys_taken,
        take_over_print_keys,
    };
    let group = adw::PreferencesGroup::builder()
        .title("System screenshot keys")
        .description(
            "GNOME binds Print, Shift+Print and Alt+Print to its own screenshot tool. \
             OctoSnap can take them for All-In-One, Capture Area and Capture Window, and \
             give them back.",
        )
        .build();
    let row = adw::ActionRow::builder().build();
    let button = gtk::Button::builder().valign(gtk::Align::Center).build();
    row.add_suffix(&button);
    row.set_activatable_widget(Some(&button));
    group.add(&row);

    let Some(gnome) = settings::shell_keybindings() else {
        row.set_title("GNOME's keybinding schema is not available");
        row.set_sensitive(false);
        return group;
    };
    let usable = PRINT_KEYS.iter().all(|(gnome_key, our_key, _)| {
        gnome.settings_schema().is_some_and(|s| s.has_key(gnome_key))
            && ext.settings_schema().is_some_and(|s| s.has_key(our_key))
    });
    if !usable {
        row.set_title("Unavailable in the installed extension");
        row.set_sensitive(false);
        return group;
    }

    // Owned from here on: the handlers below outlive this call.
    let ext = ext.clone();
    let strv = |settings: &gio::Settings| {
        let settings = settings.clone();
        move |key: &str| -> Vec<String> { settings.strv(key).iter().map(ToString::to_string).collect() }
    };
    // Weakly, all four: both settings objects hold this, and so does the button's own
    // handler, which holds the settings (D139).
    let refresh = {
        let (row, button, ext, gnome) = (row.downgrade(), button.downgrade(), ext.downgrade(), gnome.downgrade());
        move || {
            let (Some(row), Some(button), Some(ext), Some(gnome)) =
                (row.upgrade(), button.upgrade(), ext.upgrade(), gnome.upgrade())
            else {
                return false;
            };
            let taken = print_keys_taken(strv(&ext), strv(&gnome));
            // For the harness, which cannot read a row's title but can read a line.
            info!(taken, "print keys row");
            if taken {
                row.set_title("OctoSnap has the Print keys");
                row.set_subtitle("Print, Shift+Print and Alt+Print open OctoSnap");
                button.set_label("Give Them Back to GNOME");
                button.remove_css_class("suggested-action");
            } else {
                row.set_title("GNOME has the Print keys");
                row.set_subtitle("OctoSnap's own shortcuts are the ones above");
                button.set_label("Use Them for OctoSnap");
                if accent {
                    button.add_css_class("suggested-action");
                }
            }
            taken
        }
    };
    refresh();
    for (gnome_key, our_key, _) in PRINT_KEYS {
        let on_gnome = refresh.clone();
        watch(&gnome, gnome_key, &row, move |_, _| {
            on_gnome();
        });
        let on_ours = refresh.clone();
        watch(&ext, our_key, &row, move |_, _| {
            on_ours();
        });
    }
    button.connect_clicked(move |_| {
        let taken = print_keys_taken(strv(&ext), strv(&gnome));
        let writes = if taken {
            give_back_print_keys(settings::EXTENSION_SCHEMA_ID)
        } else {
            take_over_print_keys(settings::EXTENSION_SCHEMA_ID)
        };
        for write in &writes {
            match write {
                Write::Set { schema, key, accelerators } => {
                    let target = if *schema == GNOME_SHELL_KEYBINDINGS { &gnome } else { &ext };
                    wrote(key, target.set_strv(key, accelerators.clone()));
                }
                Write::Reset { schema, key } => {
                    let target = if *schema == GNOME_SHELL_KEYBINDINGS { &gnome } else { &ext };
                    target.reset(key);
                }
            }
        }
        info!(taken_over = !taken, writes = writes.len(), "print keys");
        refresh();
    });
    group
}

/// Every binding on this desktop that a new shortcut could collide with: GNOME Shell's,
/// the window manager's, the media keys' (fixed and custom), and OctoSnap's own.
fn known_bindings(ext: &gio::Settings) -> Vec<octosnap_core::shortcuts::Binding> {
    use octosnap_core::shortcuts::{Binding, GNOME_SHELL_KEYBINDINGS, MEDIA_KEYS, WM_KEYBINDINGS};
    let mut known = Vec::new();
    let mut collect = |schema_id: &str, settings: &gio::Settings| {
        let Some(schema) = settings.settings_schema() else { return };
        for key in schema.list_keys() {
            if schema.key(&key).value_type().as_str() != "as" {
                continue;
            }
            known.push(Binding {
                schema: schema_id.to_owned(),
                key: key.to_string(),
                accelerators: settings.strv(&key).iter().map(ToString::to_string).collect(),
            });
        }
    };
    collect(settings::EXTENSION_SCHEMA_ID, ext);
    // In a sandbox GNOME's schemas are the runtime's defaults rather than the user's
    // bindings, or not there at all: the extension reads the real ones (D123).
    if settings::sandboxed() {
        known.extend(crate::remote_settings::keybindings().unwrap_or_default());
        return known;
    }
    for schema_id in [GNOME_SHELL_KEYBINDINGS, WM_KEYBINDINGS, MEDIA_KEYS] {
        if let Some(settings) = settings::open_schema(schema_id) {
            collect(schema_id, &settings);
        }
    }

    // The media keys' custom shortcuts live in a relocatable schema, one path each.
    if let (Some(media), Some(source)) =
        (settings::open_schema(MEDIA_KEYS), gio::SettingsSchemaSource::default())
    {
        let custom_id = format!("{MEDIA_KEYS}.custom-keybinding");
        if let Some(schema) = source.lookup(&custom_id, true)
            && media.settings_schema().is_some_and(|s| s.has_key("custom-keybindings"))
        {
            for path in media.strv("custom-keybindings") {
                let custom =
                    gio::Settings::new_full(&schema, None::<&gio::SettingsBackend>, Some(path.as_str()));
                known.push(Binding {
                    schema: custom_id.clone(),
                    key: custom.string("name").to_string(),
                    accelerators: vec![custom.string("binding").to_string()],
                });
            }
        }
    }
    known
}

/// One rebindable action.
///
/// A real recorder rather than a text field: an accelerator has to round-trip through
/// `accelerator_parse`/`accelerator_name` to be valid GSettings, and asking a user to
/// type `<Control><Alt>a` is asking them to learn a serialisation format. The recorder
/// state lives on the button rather than in a shared cell so two rows cannot record at
/// once.
fn shortcut_row(settings: &gio::Settings, key: &'static str, label: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(label).build();

    let button = gtk::Button::builder()
        .valign(gtk::Align::Center)
        .width_request(180)
        .build();
    button.add_css_class("flat");

    let clear = gtk::Button::builder()
        .icon_name("edit-clear-symbolic")
        .valign(gtk::Align::Center)
        .tooltip_text("Remove this shortcut")
        .build();
    clear.add_css_class("flat");

    // Every widget here weakly, or from its own signal, and the settings from theirs: each
    // handler below is owned by the settings or by one of these widgets, and a strong copy
    // in any of them was a ring that kept the row (D139).
    let refresh = {
        let (button, clear) = (button.downgrade(), clear.downgrade());
        move |settings: &gio::Settings| {
            let (Some(button), Some(clear)) = (button.upgrade(), clear.upgrade()) else { return };
            let bindings = settings.strv(key);
            match bindings.first().map(|b| b.as_str()) {
                Some(accel) if !accel.is_empty() => {
                    button.set_label(&pretty_accelerator(accel));
                    button.remove_css_class("dim-label");
                    clear.set_visible(true);
                }
                _ => {
                    button.set_label("Disabled");
                    button.add_css_class("dim-label");
                    clear.set_visible(false);
                }
            }
        }
    };
    refresh(settings);
    {
        let refresh = refresh.clone();
        watch(settings, key, &row, move |_, settings| refresh(settings));
    }

    {
        let settings = settings.clone();
        let row = row.downgrade();
        clear.connect_clicked(move |_| {
            wrote(key, settings.set_strv(key, Vec::<&str>::new()));
            if let Some(row) = row.upgrade() {
                row.set_subtitle("");
            }
            info!(key, "shortcut cleared");
        });
    }

    {
        let settings = settings.clone();
        let refresh = refresh.clone();
        let row = row.downgrade();
        button.connect_clicked(move |button| {
            button.set_label("Press a shortcut\u{2026}");
            // Not flat while it listens: a flat suggested-action button keeps the white
            // label and loses the fill, and "Press a shortcut…" was white on white (D136).
            button.remove_css_class("flat");
            button.add_css_class("suggested-action");

            let controller = gtk::EventControllerKey::new();
            // CAPTURE, so the keystroke is seen before any widget or mnemonic consumes
            // it. Without this, Escape closes the dialog instead of cancelling.
            controller.set_propagation_phase(gtk::PropagationPhase::Capture);

            let (settings, refresh, row) = (settings.clone(), refresh.clone(), row.clone());
            controller.connect_key_pressed(move |controller, keyval, _keycode, state| {
                let Some(button) = controller.widget() else { return glib::Propagation::Proceed };
                let row_inner = row.upgrade();
                let say = |text: &str| {
                    if let Some(row) = &row_inner {
                        row.set_subtitle(text);
                    }
                };
                let finish = || {
                    button.remove_css_class("suggested-action");
                    button.add_css_class("flat");
                    button.remove_controller(controller);
                    refresh(&settings);
                };

                // A bare modifier is the user still assembling the chord, not a binding.
                if is_modifier(keyval) {
                    return glib::Propagation::Stop;
                }

                match keyval {
                    gtk::gdk::Key::Escape => {
                        say("");
                        finish();
                        return glib::Propagation::Stop;
                    }
                    gtk::gdk::Key::BackSpace | gtk::gdk::Key::Delete => {
                        wrote(key, settings.set_strv(key, Vec::<&str>::new()));
                        finish();
                        return glib::Propagation::Stop;
                    }
                    _ => {}
                }

                // Only the modifiers GNOME will actually route on. Lock and the group
                // bits arrive with Caps Lock or a non-default keyboard layout and would
                // otherwise be baked into the stored accelerator.
                let mods = state & gtk::gdk::ModifierType::from_bits_truncate(
                    gtk::gdk::ModifierType::CONTROL_MASK.bits()
                        | gtk::gdk::ModifierType::SHIFT_MASK.bits()
                        | gtk::gdk::ModifierType::ALT_MASK.bits()
                        | gtk::gdk::ModifierType::SUPER_MASK.bits(),
                );

                if !gtk::accelerator_valid(keyval, mods) {
                    info!(?keyval, "refused an unusable accelerator");
                    return glib::Propagation::Stop;
                }
                let accel = gtk::accelerator_name(keyval, mods);

                // A bare letter is a valid accelerator to GTK but a terrible global
                // shortcut: GNOME would take it from every application for as long as the
                // binding stood. This comment used to say it was refused, above, where
                // only what GTK calls invalid is (D136). The recorder keeps listening, so
                // the next press can be the chord.
                if !usable_as_global(keyval, mods) {
                    say(&format!(
                        "{} alone would stop working in every other app. Hold Ctrl, Alt or Super with it",
                        octosnap_core::shortcuts::Accelerator::parse(&accel)
                            .map_or_else(|| accel.to_string(), |a| a.label())
                    ));
                    info!(accel = accel.as_str(), "refused a key that needs Ctrl, Alt or Super");
                    return glib::Propagation::Stop;
                }

                // `spec/08` §3's conflict detection. A binding GNOME already owns would be
                // refused by the shell silently -- `Main.wm.addKeybinding` just answers
                // NONE -- so the row says so here, where the user is looking, and keeps
                // the shortcut it had.
                let known = known_bindings(&settings);
                let taken = octosnap_core::shortcuts::conflicts(
                    &accel,
                    settings::EXTENSION_SCHEMA_ID,
                    key,
                    &known,
                );
                if let Some(other) = taken.first() {
                    say(&format!(
                        "{} is already used by {}: {}",
                        octosnap_core::shortcuts::Accelerator::parse(&accel)
                            .map_or_else(|| accel.to_string(), |a| a.label()),
                        other.owner(),
                        other.label()
                    ));
                    info!(
                        key,
                        accel = accel.as_str(),
                        by = other.key.as_str(),
                        schema = other.schema.as_str(),
                        "refused a shortcut already in use"
                    );
                    finish();
                    return glib::Propagation::Stop;
                }
                say("");
                wrote(key, settings.set_strv(key, vec![accel.as_str()]));
                info!(key, accel = accel.as_str(), "shortcut bound");
                finish();
                glib::Propagation::Stop
            });
            button.add_controller(controller);
        });
    }

    row.add_suffix(&button);
    row.add_suffix(&clear);
    row.set_activatable_widget(Some(&button));
    row
}

/// Whether a chord can be a global shortcut, by GNOME Settings' own rule (its
/// `is_valid_binding`). With Ctrl, Alt or Super, anything can. Without them, a key that
/// types something, Shift+letter included, or that moves a caret or focus, cannot: the
/// shell would take it from every application. The keys that do neither on their own,
/// such as the function keys, Print and the media keys, can.
fn usable_as_global(keyval: gtk::gdk::Key, mods: gtk::gdk::ModifierType) -> bool {
    use gtk::gdk::{Key, ModifierType};
    if mods.intersects(ModifierType::CONTROL_MASK | ModifierType::ALT_MASK | ModifierType::SUPER_MASK) {
        return true;
    }
    let types = keyval.to_unicode().is_some();
    let moves = matches!(
        keyval,
        Key::Home
            | Key::End
            | Key::Left
            | Key::Right
            | Key::Up
            | Key::Down
            | Key::Page_Up
            | Key::Page_Down
            | Key::KP_Home
            | Key::KP_End
            | Key::KP_Left
            | Key::KP_Right
            | Key::KP_Up
            | Key::KP_Down
            | Key::KP_Page_Up
            | Key::KP_Page_Down
            | Key::Tab
            | Key::ISO_Left_Tab
            | Key::Return
            | Key::KP_Enter
            | Key::Menu
    );
    !types && !moves
}

/// True for keys that only ever appear as part of a chord.
fn is_modifier(keyval: gtk::gdk::Key) -> bool {
    use gtk::gdk::Key;
    matches!(
        keyval,
        Key::Shift_L
            | Key::Shift_R
            | Key::Control_L
            | Key::Control_R
            | Key::Alt_L
            | Key::Alt_R
            | Key::Super_L
            | Key::Super_R
            | Key::Meta_L
            | Key::Meta_R
            | Key::Hyper_L
            | Key::Hyper_R
            | Key::Caps_Lock
            | Key::Shift_Lock
            | Key::ISO_Level3_Shift
    )
}

/// Turns `<Control><Alt>a` into something a person reads.
fn pretty_accelerator(accel: &str) -> String {
    let Some((keyval, mods)) = gtk::accelerator_parse(accel) else {
        // A hand-edited dconf value that GTK cannot parse. Showing it verbatim is more
        // use than showing nothing, because it is what the user has to go and fix.
        warn!(accel, "could not parse a stored accelerator");
        return accel.to_owned();
    };
    gtk::accelerator_get_label(keyval, mods).to_string()
}

// --- Binding helpers ---------------------------------------------------------------

/// The settings object, but only when its schema actually has this key.
///
/// Every read of an *extension* key goes through this. The two halves ship as separate
/// artifacts -- a `cargo build` does not reinstall the extension, and the user enables it
/// independently -- so an app that is newer than the installed extension is routine, not
/// exotic. And `gio::Settings::boolean` on an absent key calls `g_error`, which
/// **aborts the process**: the very first version of this dialog killed the service
/// outright against a schema one commit old. A row that is visibly insensitive is the
/// right answer; a dead service is not.
fn with_key<'a>(settings: Option<&'a gio::Settings>, key: &str) -> Option<&'a gio::Settings> {
    let settings = settings?;
    let schema = settings.settings_schema()?;
    if schema.has_key(key) {
        return Some(settings);
    }
    warn!(
        key,
        schema = schema.id().as_str(),
        "the installed schema has no such key; the setting is unavailable"
    );
    None
}

// --- Annotate ------------------------------------------------------------------------

/// `spec/08` §7, limited to the rows the editor reads: the crop's snapping (a verified
/// preference, `snapInAnnotateCrop`) and the export scale. Shadows, arrow direction and
/// the default tool are the editor's own per-tool memory, which §7 also lists and which
/// the options row already keeps.
fn annotate_page(app_settings: Option<&gio::Settings>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Annotate")
        .name(PAGE_ANNOTATE)
        .icon_name("document-edit-symbolic")
        .build();

    let crop = adw::PreferencesGroup::builder().title("Crop").build();
    let snap = adw::SwitchRow::builder()
        .title("Snap to the image's edges and to other objects")
        .subtitle("The crop bar's Snap button is this setting")
        .build();
    bind_boolean(app_settings, "annotate-crop-snap", &snap);
    crop.add(&snap);
    page.add(&crop);

    let export = adw::PreferencesGroup::builder()
        .title("Export")
        .description("What Copy, Save, Pin and Drag me produce")
        .build();
    let scale = adw::ComboRow::builder()
        .title("Export scale")
        .subtitle("A HiDPI capture is exported at its own pixel density, or scaled to 1\u{d7}")
        .model(&gtk::StringList::new(&["The capture's own", "1\u{d7}"]))
        .build();
    const EXPORT_ORDER: [&str; 2] = ["document", "1x"];
    match with_key(app_settings, "ann-export-scale") {
        Some(s) => bind_enum_row(s, "ann-export-scale", &scale, &EXPORT_ORDER, |v| v),
        None => scale.set_sensitive(false),
    }
    export.add(&scale);
    page.add(&export);

    // `spec/08` §1's Annotate row. It is about the editor rather than the document, so it
    // sits apart from Crop and Export: a project carries its own background, and this
    // decides only whether the panel that edits one is showing when the window opens.
    let tools = adw::PreferencesGroup::builder().title("Tools").build();
    let remember = adw::SwitchRow::builder()
        .title("Remember if the background panel was open")
        .subtitle("A new editor opens the panel if the last one had it open")
        .build();
    bind_boolean(app_settings, "ann-background-remember", &remember);
    tools.add(&remember);

    // `spec/05` §4.13's "automatically apply preset to all screenshots". The key was in
    // the schema from M6 with nothing that could set it, so the editor's default preset
    // never reached a screenshot (D167). The preset itself is chosen in the editor, where
    // the presets are, and the subtitle names it so the switch says what it adds.
    let auto = adw::SwitchRow::builder().title("Add the default background").build();
    bind_boolean(app_settings, "ann-background-auto", &auto);
    if let Some(settings) = with_key(app_settings, "ann-background-default") {
        let name = |row: &adw::SwitchRow, settings: &gio::Settings| {
            row.set_subtitle(&default_background_subtitle(settings));
        };
        name(&auto, settings);
        watch(settings, "ann-background-default", &auto, name);
        watch(settings, "ann-background-presets", &auto, name);
    }
    tools.add(&auto);
    page.add(&tools);
    page
}

/// The automatic background row's subtitle: which preset it adds, or where to choose one.
fn default_background_subtitle(settings: &gio::Settings) -> String {
    let id = settings.string("ann-background-default");
    let preset = (!id.is_empty())
        .then(|| {
            settings
                .strv("ann-background-presets")
                .iter()
                .filter_map(|text| octosnap_scene::Preset::parse(text))
                .find(|preset| preset.id == id)
        })
        .flatten();
    match preset {
        // What the switch adds, which is true whether it is on or off.
        Some(preset) => format!(
            "Adds \u{201c}{}\u{201d}, the editor\u{2019}s default preset. \
             Hold Shift as you capture to leave it off",
            preset.name
        ),
        None => "Choose a default in the editor\u{2019}s Background panel, under Presets".into(),
    }
}

// --- Pets ----------------------------------------------------------------------------

/// `pet-size`'s choices, in the Size row's order: smallest first. Tiny came first on
/// 2026-09-28 (D155), and Smol between it and Small on 2026-10-07 (D166).
const PET_SIZES_WIRE: [&str; 6] = ["tiny", "smol", "small", "medium", "large", "huge"];

/// `pet-activity`'s choices, in the Liveliness row's order: quietest first. Silent and Zen
/// came before Calm on 2026-10-07 (D166).
const PET_ACTIVITY_WIRE: [&str; 5] = ["silent", "zen", "calm", "normal", "lively"];

/// `spec/14` §11: the desktop pets. Every key here is the extension's, since the pets live
/// in the shell (D142).
///
/// While the pets are away, the rows about how they behave go insensitive, as dependent
/// rows do everywhere in this dialog (`spec/08` §0). The cards stay live: choosing who
/// comes out before bringing them out is a fair order to do it in, and the cards are also
/// the page's picture of what the switch does.
fn pets_page(ext_settings: Option<&gio::Settings>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Pets")
        .name(PAGE_PETS)
        .icon_name("pets-symbolic")
        .build();

    let Some(settings) = ext_settings else {
        page.add(&needs_extension(
            "Pets come with the extension",
            "The pets live in OctoSnap's GNOME Shell extension",
        ));
        return page;
    };

    let show = adw::PreferencesGroup::new();
    let enabled = adw::SwitchRow::builder()
        .title("Show desktop pets")
        .subtitle("Pixel-art friends who live on the desktop. Captures and recordings never show them")
        .build();
    bind_boolean(Some(settings), "pets-enabled", &enabled);
    show.add(&enabled);
    page.add(&show);

    crate::pets::install_style();
    let who = adw::PreferencesGroup::builder()
        .title("Who comes out")
        .description("Drag a pet to move it, and right-click it for the capture menu")
        .build();
    let wrap = adw::WrapBox::builder().child_spacing(12).line_spacing(12).align(0.5).build();
    let cards: Vec<(&'static str, gtk::ToggleButton)> =
        crate::pets::KINDS.iter().map(|&kind| (kind, pet_card(kind))).collect();
    for (_, card) in &cards {
        wrap.append(card);
    }
    match with_key(Some(settings), "pets") {
        Some(s) => bind_pet_cards(s, &cards),
        None => wrap.set_sensitive(false),
    }
    who.add(&wrap);
    page.add(&who);

    let behaviour = adw::PreferencesGroup::builder().title("Behaviour").build();
    let size = adw::ComboRow::builder()
        .title("Size")
        .subtitle("Drawn pixel by pixel, so they stay sharp at any scale")
        .model(&gtk::StringList::new(&["Tiny", "Smol", "Small", "Medium", "Large", "Huge"]))
        .build();
    match with_key(Some(settings), "pet-size") {
        Some(s) => bind_enum_row(s, "pet-size", &size, &PET_SIZES_WIRE, |v| v),
        None => size.set_sensitive(false),
    }
    behaviour.add(&size);

    let activity = adw::ComboRow::builder()
        .title("Liveliness")
        .subtitle("How often they get up to something by themselves")
        .model(&gtk::StringList::new(&["Silent", "Zen", "Calm", "Normal", "Lively"]))
        .build();
    match with_key(Some(settings), "pet-activity") {
        Some(s) => bind_enum_row(s, "pet-activity", &activity, &PET_ACTIVITY_WIRE, |v| v),
        None => activity.set_sensitive(false),
    }
    behaviour.add(&activity);

    let wander = adw::SwitchRow::builder()
        .title("Move on their own")
        .subtitle("Walk, hop and climb about by themselves. Off, each stays where it is put, and plays there")
        .build();
    bind_boolean(Some(settings), "pet-wander", &wander);
    behaviour.add(&wander);

    // `spec/14` §4: the owner's idea of 2026-09-27, the screen as a desk seen from above.
    let roam = adw::ComboRow::builder()
        .title("Where they can be")
        .subtitle("Anywhere, a pet put down away from the bottom stays there, seen from above")
        .model(&gtk::StringList::new(&["Along the bottom", "Anywhere on the screen"]))
        .build();
    const ROAM_ORDER: [&str; 2] = ["floor", "anywhere"];
    match with_key(Some(settings), "pet-roam") {
        Some(s) => bind_enum_row(s, "pet-roam", &roam, &ROAM_ORDER, |v| v),
        None => roam.set_sensitive(false),
    }
    behaviour.add(&roam);

    let menu = adw::ComboRow::builder()
        .title("Menu placement")
        .subtitle("Where a right-click on a pet opens its capture menu")
        .model(&gtk::StringList::new(&["Automatic", "Above the pet", "Below the pet", "Around the pet"]))
        .build();
    const MENU_ORDER: [&str; 4] = ["auto", "above", "below", "around"];
    match with_key(Some(settings), "pet-menu") {
        Some(s) => bind_enum_row(s, "pet-menu", &menu, &MENU_ORDER, |v| v),
        None => menu.set_sensitive(false),
    }
    behaviour.add(&menu);
    page.add(&behaviour);

    let sharing = adw::PreferencesGroup::builder().title("Screen sharing").build();
    let hide = adw::SwitchRow::builder()
        .title("Hide while the screen is shared")
        .subtitle("While another app records or shares the screen. OctoSnap\u{2019}s own recordings never show them")
        .build();
    bind_boolean(Some(settings), "pet-hide-sharing", &hide);
    sharing.add(&hide);
    page.add(&sharing);

    if with_key(Some(settings), "pets-enabled").is_some() {
        let follow = |group: &adw::PreferencesGroup, settings: &gio::Settings| {
            group.set_sensitive(settings.boolean("pets-enabled"));
        };
        for group in [&behaviour, &sharing] {
            follow(group, settings);
            watch(settings, "pets-enabled", group, follow);
        }
    }
    page
}

/// A pet's card on the Pets page: the pet playing its loop, its name and what it is. On
/// means it comes out.
fn pet_card(kind: &'static str) -> gtk::ToggleButton {
    let name = crate::pets::name(kind);
    let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).build();
    column.append(&crate::pets::PetPreview::new(kind, 2));
    let title = gtk::Label::new(Some(&name));
    title.add_css_class("heading");
    column.append(&title);
    let what = gtk::Label::new(Some(kind));
    what.add_css_class("caption");
    what.add_css_class("dim-label");
    column.append(&what);

    // A tick as well as the colour, so the choice does not rest on colour alone.
    let tick = gtk::Image::builder()
        .icon_name("object-select-symbolic")
        .halign(gtk::Align::End)
        .valign(gtk::Align::Start)
        .build();
    tick.add_css_class("octosnap-pet-check");
    let overlay = gtk::Overlay::builder().child(&column).build();
    overlay.add_overlay(&tick);

    let card = gtk::ToggleButton::builder().child(&overlay).width_request(96).build();
    card.add_css_class("card");
    card.add_css_class("octosnap-pet-card");
    card.bind_property("active", &tick, "visible").sync_create().build();
    card.update_property(&[gtk::accessible::Property::Label(&format!("{name} the {kind}"))]);
    card
}

/// Binds the `pets` list to the pets' cards: a card that is on is a pet that is out.
///
/// The list keeps the extension's order (`pets::KINDS`) whatever order the pets were chosen
/// in, and keeps any pet this app does not know, after the ones it does, for an extension
/// newer than the app. It is never left empty: with the pets on, an empty list is pets
/// that never come, and the switch above is the way to put them all away. So the last card
/// on stays on, and the dialog says why.
fn bind_pet_cards(settings: &gio::Settings, cards: &[(&'static str, gtk::ToggleButton)]) {
    const KEY: &str = "pets";
    for (kind, card) in cards {
        let kind = *kind;
        let apply = move |card: &gtk::ToggleButton, settings: &gio::Settings| {
            let out = settings.strv(KEY).iter().any(|v| v.as_str() == kind);
            // Guarded, or writing the setting below re-enters this and fights the user.
            if card.is_active() != out {
                card.set_active(out);
            }
        };
        apply(card, settings);
        watch(settings, KEY, card, apply);

        let settings = settings.clone();
        card.connect_toggled(move |card| {
            let current: Vec<String> =
                settings.strv(KEY).iter().map(|v| v.as_str().to_owned()).collect();
            if card.is_active() == current.iter().any(|v| v == kind) {
                return;
            }
            let out = |k: &str| if k == kind { card.is_active() } else { current.iter().any(|v| v == k) };
            let next: Vec<&str> = crate::pets::KINDS
                .iter()
                .copied()
                .filter(|k| out(k))
                .chain(current.iter().map(String::as_str).filter(|v| !crate::pets::KINDS.contains(v)))
                .collect();
            if next.is_empty() {
                card.set_active(true);
                if let Some(dialog) =
                    card.ancestor(adw::PreferencesDialog::static_type()).and_downcast::<adw::PreferencesDialog>()
                {
                    dialog.add_toast(adw::Toast::new("One pet stays out. The switch above puts them all away"));
                }
                return;
            }
            wrote(KEY, settings.set_strv(KEY, next));
        });
    }
}

// --- Advanced ------------------------------------------------------------------------

/// `spec/08` §9: the command line and links, history retention, and debug logging.
fn advanced_page(app_settings: Option<&gio::Settings>, ext_settings: Option<&gio::Settings>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Advanced")
        .name(PAGE_ADVANCED)
        .icon_name("preferences-other-symbolic")
        .build();

    let automation = adw::PreferencesGroup::builder().title("Command line and links").build();
    let api = adw::SwitchRow::builder()
        .title("Allow the command line and octosnap:// links")
        .subtitle("Off makes `octosnap capture` and the links refuse to run")
        .build();
    bind_boolean(app_settings, "api-enabled", &api);
    automation.add(&api);
    page.add(&automation);

    // `spec/07` §4.1's retention and `spec/04` §7's rule about saved captures.
    let history = adw::PreferencesGroup::builder()
        .title("Capture history")
        .description("Closed cards go to the history, where they can be restored")
        .build();
    let retention = adw::ComboRow::builder()
        .title("Keep closed captures for")
        .model(&gtk::StringList::new(
            &octosnap_core::history::Retention::ALL.map(octosnap_core::history::Retention::label),
        ))
        .build();
    match with_key(app_settings, "history-retention-days") {
        Some(s) => bind_retention(s, &retention),
        None => retention.set_sensitive(false),
    }
    history.add(&retention);
    let keep_saved = adw::SwitchRow::builder()
        .title("Keep saved captures in the history")
        .subtitle("Off means a capture already in your pictures folder is not kept a second time")
        .build();
    bind_boolean(app_settings, "history-keep-saved", &keep_saved);
    history.add(&keep_saved);
    let clear = adw::ActionRow::builder()
        .title("Clear the history")
        .subtitle("Removes every entry; saved copies stay where they are")
        .build();
    let clear_button = gtk::Button::builder().label("Clear\u{2026}").valign(gtk::Align::Center).build();
    clear_button.add_css_class("destructive-action");
    // Anchored to the button itself: the row held here, whose suffix the button is, was a
    // ring that kept the row (D139).
    clear_button.connect_clicked(confirm_clear_history);
    clear.add_suffix(&clear_button);
    clear.set_activatable_widget(Some(&clear_button));
    history.add(&clear);
    page.add(&history);

    // `spec/08` §1 puts these three under Advanced > Text Recognition, and `spec/07` §2.1
    // is what they mean. The packs are their own group below, because a list of five
    // downloads with a progress bar apiece does not belong under a description that is
    // about line breaks.
    let text = adw::PreferencesGroup::builder()
        .title("Text recognition")
        .description(
            "Reads the text in a capture and copies it. Everything happens on this \
             machine; the only thing downloaded is the language pack.",
        )
        .build();
    let language = adw::ComboRow::builder()
        .title("Language")
        .subtitle("Automatic tries the packs you have installed and keeps the best read")
        .model(&gtk::StringList::new(
            &crate::ocr::Language::ALL.map(crate::ocr::Language::name),
        ))
        .build();
    match with_key(app_settings, "ocr-language") {
        Some(s) => bind_enum_row(
            s,
            "ocr-language",
            &language,
            &crate::ocr::Language::ALL,
            crate::ocr::Language::tag,
        ),
        None => language.set_sensitive(false),
    }
    text.add(&language);
    let breaks = adw::SwitchRow::builder()
        .title("Keep line breaks")
        .subtitle("Off joins each paragraph into one line, which is what pasting into prose wants")
        .build();
    bind_boolean(app_settings, "ocr-line-breaks", &breaks);
    text.add(&breaks);
    let links = adw::SwitchRow::builder()
        .title("Detect links")
        .subtitle("Makes addresses and e-mail addresses clickable in the result window")
        .build();
    bind_boolean(app_settings, "ocr-detect-links", &links);
    text.add(&links);
    page.add(&text);
    page.add(&crate::ocr::prefs::packs_group());

    let troubleshooting = adw::PreferencesGroup::builder().title("Troubleshooting").build();
    let debug = adw::SwitchRow::builder()
        .title("Debug logging")
        // A sandboxed app's lines reach the journal under Flatpak's name, not this one's,
        // so the log file of its own is the place to point at (D118).
        .subtitle(if settings::sandboxed() {
            "Every placement, gesture and setting in OctoSnap's log, which About → Troubleshooting opens"
        } else {
            "Every placement, gesture and setting in the journal: journalctl --user -t octosnap-app \
             for the app, journalctl --user -g octosnap for the extension"
        })
        .build();
    bind_boolean(app_settings, "debug", &debug);
    // The extension's half of the same switch: its routine lines reach the journal only
    // with its own `debug-log` on (D135), so it follows this row, and one switch turns on
    // the detail of both. An extension from before the key is left alone, quietly: the
    // app's half still works, so the row is not unavailable.
    if let Some(ext) = ext_settings.filter(|s| s.settings_schema().is_some_and(|schema| schema.has_key("debug-log"))) {
        let ext = ext.clone();
        debug.connect_active_notify(move |row| wrote("debug-log", ext.set_boolean("debug-log", row.is_active())));
    }
    if std::env::var_os("RUST_LOG").is_some() {
        debug.set_subtitle("RUST_LOG is set for this process and takes precedence");
    }
    troubleshooting.add(&debug);
    page.add(&troubleshooting);
    page
}

/// `history-retention-days` as a menu of the five choices `spec/08` §9 lists. A value
/// set by hand lands on the choice `Retention::from_days` snaps it to.
fn bind_retention(settings: &gio::Settings, row: &adw::ComboRow) {
    use octosnap_core::history::Retention;
    const KEY: &str = "history-retention-days";
    let apply = |row: &adw::ComboRow, settings: &gio::Settings| {
        let current = Retention::from_days(settings.int(KEY));
        let index = Retention::ALL.iter().position(|r| *r == current).unwrap_or(2);
        if row.selected() as usize != index {
            row.set_selected(index as u32);
        }
    };
    apply(row, settings);
    watch(settings, KEY, row, apply);
    let settings = settings.clone();
    row.connect_selected_notify(move |row| {
        if let Some(choice) = Retention::ALL.get(row.selected() as usize) {
            wrote(KEY, settings.set_int(KEY, i32::try_from(choice.days()).unwrap_or(7)));
        }
    });
}

/// The same question the strip asks before emptying the history.
fn confirm_clear_history(anchor: &impl IsA<gtk::Widget>) {
    let Some(history) = crate::history() else { return };
    let count = history.len();
    let dialog = adw::AlertDialog::builder()
        .heading("Clear the history?")
        .body(if count == 0 {
            "The history is already empty.".to_owned()
        } else {
            format!(
                "{count} capture{} will be removed from the history. Copies you saved to your \
                 pictures folder stay where they are.",
                if count == 1 { "" } else { "s" }
            )
        })
        .build();
    dialog.add_response("cancel", "Cancel");
    if count > 0 {
        dialog.add_response("clear", "Clear");
        dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
    }
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(None, move |_, response| {
        if response == "clear" {
            history.clear();
        }
    });
    dialog.present(Some(anchor));
}

// --- About ---------------------------------------------------------------------------

/// `spec/08` §10: "Version, extension status (installed/enabled/version match),
/// changelog, acknowledgments, link to docs."
fn about_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("About")
        .name(PAGE_ABOUT)
        .icon_name("help-about-symbolic")
        .build();

    let app_group = adw::PreferencesGroup::builder().title("OctoSnap").build();
    let version = adw::ActionRow::builder()
        .title("Version")
        .subtitle(env!("CARGO_PKG_VERSION"))
        .build();
    app_group.add(&version);
    let protocol = adw::ActionRow::builder()
        .title("Protocol")
        .subtitle(format!(
            "This build speaks version {} of the extension protocol",
            octosnap_core::protocol::PROTOCOL_VERSION
        ))
        .build();
    app_group.add(&protocol);
    page.add(&app_group);

    // The extension's state, in `setup.rs`'s words, which are the welcome window's.
    let ext_group = adw::PreferencesGroup::builder()
        .title("GNOME Shell extension")
        .description("The half of OctoSnap that lives in the compositor")
        .build();
    let running = adw::ActionRow::builder().title("State").subtitle("Asking\u{2026}").build();
    let set_up = app_action_button("Set Up\u{2026}", "welcome");
    set_up.set_visible(false);
    running.add_suffix(&set_up);
    ext_group.add(&running);
    if let Some(connection) = gio::Application::default().and_then(|app| app.dbus_connection()) {
        let running = running.clone();
        glib::spawn_future_local(async move {
            let found = crate::setup::status(&connection).await;
            running.set_subtitle(&found.describe());
            set_up.set_visible(!found.ready());
        });
    } else {
        running.set_subtitle("No session bus: the app started without one");
    }
    page.add(&ext_group);

    // `spec/11` M7's crash log. Nothing leaves the machine unless the person sends it.
    let trouble = adw::PreferencesGroup::builder()
        .title("Troubleshooting")
        .description(
            "A report is one text file with the versions, the settings and the recent logs \
             of both halves. Nothing is sent anywhere; attach it to an issue if you want to.",
        )
        .build();
    let report = adw::ActionRow::builder().title("Save a report").build();
    let save = app_action_button("Save\u{2026}", "export-report");
    report.add_suffix(&save);
    report.set_activatable_widget(Some(&save));
    trouble.add(&report);
    let logs = adw::ActionRow::builder()
        .title("Log files")
        .subtitle(crate::diagnostics::directory().display().to_string())
        .build();
    let open_logs = gtk::Button::builder().label("Open").valign(gtk::Align::Center).build();
    open_logs.connect_clicked(|button| {
        let folder = gio::File::for_path(crate::diagnostics::directory());
        let launcher = gtk::FileLauncher::new(Some(&folder));
        let parent = button.root().and_downcast::<gtk::Window>();
        launcher.launch(parent.as_ref(), gio::Cancellable::NONE, |result| {
            if let Err(e) = result {
                warn!("could not open the log folder: {e}");
            }
        });
    });
    logs.add_suffix(&open_logs);
    logs.set_activatable_widget(Some(&open_logs));
    trouble.add(&logs);
    let welcome = adw::ActionRow::builder().title("Welcome window").build();
    let show = app_action_button("Show", "welcome");
    welcome.add_suffix(&show);
    welcome.set_activatable_widget(Some(&show));
    trouble.add(&welcome);
    page.add(&trouble);

    let links = adw::PreferencesGroup::builder().title("Links").build();
    for (title, url) in [
        ("Source code", REPOSITORY.to_owned()),
        ("Report an issue", format!("{REPOSITORY}/issues")),
        // What the public repository has: the development notes are not in it (D156).
        ("User guide", format!("{REPOSITORY}/blob/main/docs/guide.md")),
        ("Releases", format!("{REPOSITORY}/releases")),
    ] {
        let row = adw::ActionRow::builder().title(title).subtitle(&url).build();
        let open = gtk::LinkButton::builder().uri(&url).label("Open").valign(gtk::Align::Center).build();
        row.add_suffix(&open);
        row.set_activatable_widget(Some(&open));
        links.add(&row);
    }
    page.add(&links);

    let thanks = adw::PreferencesGroup::builder().title("Built with").build();
    let label = gtk::Label::new(Some(
        "GTK 4 and libadwaita for the windows, GNOME Shell for the half that needs the \
         compositor, and the Rust crates named in Cargo.toml. Licensed GPL-3.0-or-later; \
         the GIF encoder, gifski, is AGPL-3.0-or-later.",
    ));
    label.set_wrap(true);
    label.set_xalign(0.0);
    label.add_css_class("dim-label");
    thanks.add(&label);
    page.add(&thanks);
    page
}

/// Writes a setting, reporting rather than ignoring a refusal.
///
/// `gio::Settings::set_*` returns `false` when the key is locked by a system
/// administrator or the backend is read-only. Swallowing that produces a control that
/// snaps back with no explanation, which is the worst of the three outcomes.
fn wrote(key: &str, result: Result<(), glib::BoolError>) {
    if let Err(e) = result {
        warn!(key, "the setting could not be written: {e}");
    }
}

/// Runs `apply` on `widget` whenever `key` changes, for as long as the widget is there.
///
/// The settings object owns the handler. A handler that held the widget, or the settings
/// themselves, was a ring GTK never frees, so every Settings dialog ever opened stayed in
/// memory with its rows still answering every change (D139). So the widget is held weakly
/// and the settings come from the signal, and `apply` must hold nothing strongly that
/// holds the settings. The handler is also taken off when the widget goes. The sandbox's
/// settings objects live as long as the process (`remote_settings`), and would otherwise
/// collect the handlers of every dialog ever opened.
fn watch<W: IsA<gtk::Widget>>(
    settings: &gio::Settings,
    key: &str,
    widget: &W,
    apply: impl Fn(&W, &gio::Settings) + 'static,
) {
    let weak = widget.downgrade();
    let handler = settings.connect_changed(Some(key), move |settings, _| {
        if let Some(widget) = weak.upgrade() {
            apply(&widget, settings);
        }
    });
    let (settings, handler) = (settings.downgrade(), std::cell::Cell::new(Some(handler)));
    widget.connect_destroy(move |_| {
        if let (Some(settings), Some(handler)) = (settings.upgrade(), handler.take()) {
            settings.disconnect(handler);
        }
    });
}

fn bind_boolean(settings: Option<&gio::Settings>, key: &str, row: &adw::SwitchRow) {
    match with_key(settings, key) {
        Some(s) => s.bind(key, row, "active").build(),
        None => {
            row.set_sensitive(false);
            row.set_subtitle("Unavailable in the installed version");
        }
    }
}

/// Binds a numeric key to a spin row, or greys the row out when the key is not there.
fn bind_int(settings: Option<&gio::Settings>, key: &str, row: &adw::SpinRow) {
    match with_key(settings, key) {
        Some(s) => s.bind(key, row, "value").build(),
        None => row.set_sensitive(false),
    }
}

/// Binds a text key to an entry row, or greys the row out when the key is not there.
fn bind_text(settings: Option<&gio::Settings>, key: &str, row: &adw::EntryRow) {
    match with_key(settings, key) {
        Some(s) => s.bind(key, row, "text").build(),
        None => row.set_sensitive(false),
    }
}

/// Binds a string-valued key to a combo row, given the wire values in display order.
///
/// `gio::Settings::bind` cannot do this: it maps a `s` key to a `&str` property, and a
/// combo's selection is a `u32` index. Doing it by hand also means an unknown value in
/// dconf lands on index 0 instead of leaving the row blank.
fn bind_enum_row<T: PartialEq + Copy + 'static>(
    settings: &gio::Settings,
    key: &'static str,
    row: &adw::ComboRow,
    order: &'static [T],
    to_wire: fn(T) -> &'static str,
) {
    let apply = move |row: &adw::ComboRow, settings: &gio::Settings| {
        let current = settings.string(key);
        let index = order
            .iter()
            .position(|candidate| to_wire(*candidate) == current.as_str())
            .unwrap_or(0);
        // Guarded, or writing the setting below re-enters this and fights the user.
        if row.selected() as usize != index {
            row.set_selected(index as u32);
        }
    };
    apply(row, settings);
    watch(settings, key, row, apply);

    let settings = settings.clone();
    row.connect_selected_notify(move |row| {
        if let Some(value) = order.get(row.selected() as usize) {
            wrote(key, settings.set_string(key, to_wire(*value)));
        }
    });
}

/// Binds an integer key with a small fixed set of values to a combo, index by index.
///
/// `gio::Settings::bind` maps an `i` key to an integer property, but a combo's selection is
/// an *index* into a value list, not the value. This does the two-way mapping by hand, the
/// way [`bind_enum_row`] does for string keys, and falls back to `default_index` for a
/// value in dconf that is not one of the offered ones.
fn bind_int_choices(
    settings: &gio::Settings,
    key: &'static str,
    row: &adw::ComboRow,
    values: &'static [i32],
    default_index: usize,
) {
    let apply = move |row: &adw::ComboRow, settings: &gio::Settings| {
        let current = settings.int(key);
        let index = values.iter().position(|&v| v == current).unwrap_or(default_index);
        // Guarded, or writing the setting below re-enters this and fights the user.
        if row.selected() as usize != index {
            row.set_selected(index as u32);
        }
    };
    apply(row, settings);
    watch(settings, key, row, apply);

    let settings = settings.clone();
    row.connect_selected_notify(move |row| {
        if let Some(&value) = values.get(row.selected() as usize) {
            wrote(key, settings.set_int(key, value));
        }
    });
}

/// Binds one member of an `as` key to a switch: on means "present in the list".
///
/// Order is preserved on insert, because `ACT-01`'s action list is order-significant --
/// `spec/08` §1 lists `show-overlay` before `copy` and the flow runs them in order.
fn bind_string_list_member(
    settings: &gio::Settings,
    key: &'static str,
    member: &'static str,
    row: &adw::SwitchRow,
) {
    let apply = move |row: &adw::SwitchRow, settings: &gio::Settings| {
        let present = settings.strv(key).iter().any(|v| v.as_str() == member);
        if row.is_active() != present {
            row.set_active(present);
        }
    };
    apply(row, settings);
    watch(settings, key, row, apply);

    let settings = settings.clone();
    row.connect_active_notify(move |row| {
        let current: Vec<String> =
            settings.strv(key).iter().map(|v| v.as_str().to_owned()).collect();
        let present = current.iter().any(|v| v == member);
        if row.is_active() == present {
            return;
        }

        let next: Vec<String> = if row.is_active() {
            // Appended in the canonical order rather than at the end, so toggling
            // show-overlay off and on again does not move it behind copy.
            let mut next = current;
            next.push(member.to_owned());
            AfterAction::CANONICAL_ORDER
                .iter()
                .filter(|a| next.iter().any(|v| v == a.as_wire()))
                .map(|a| a.as_wire().to_owned())
                .collect()
        } else {
            current.into_iter().filter(|v| v != member).collect()
        };

        wrote(
            key,
            settings.set_strv(key, next.iter().map(String::as_str).collect::<Vec<_>>()),
        );
    });
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{
        PET_ACTIVITY_WIRE, PET_SIZES_WIRE, SHUTTER_SOUNDS_WIRE, shutter_kind, usable_as_global,
    };

    /// Settings writes the index of the row the user picked, through this list, into a
    /// key whose schema only takes its own choices. A list that drifted from the schema
    /// would write a value GSettings refuses, and the row would snap back.
    #[test]
    fn the_shutter_row_offers_the_schemas_choices_in_its_order() {
        let schema = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../extension/schemas/org.gnome.shell.extensions.octosnap.gschema.xml"
        ));
        let key = &schema[schema.find(r#"<key name="capture-sound""#).expect("the key")..];
        let key = &key[..key.find("</key>").expect("its end")];
        let choices: Vec<&str> = key
            .split("<choice value='")
            .skip(1)
            .map(|rest| &rest[..rest.find('\'').expect("a closing quote")])
            .collect();
        assert_eq!(choices, SHUTTER_SOUNDS_WIRE);
    }

    /// The same for the pets' Size and Liveliness rows, whose schema quotes its choices the
    /// other way.
    #[test]
    fn the_pet_rows_offer_the_schemas_choices_in_their_order() {
        let schema = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../extension/schemas/org.gnome.shell.extensions.octosnap.gschema.xml"
        ));
        let choices = |name: &str| -> Vec<&str> {
            let key = &schema[schema.find(&format!(r#"<key name="{name}""#)).expect("the key")..];
            let key = &key[..key.find("</key>").expect("its end")];
            key.split(r#"<choice value=""#)
                .skip(1)
                .map(|rest| &rest[..rest.find('"').expect("a closing quote")])
                .collect()
        };
        assert_eq!(choices("pet-size"), PET_SIZES_WIRE);
        assert_eq!(choices("pet-activity"), PET_ACTIVITY_WIRE);
    }

    /// D136: the recorder stored a bare letter, which the shell would then have taken
    /// from every application.
    #[test]
    fn a_global_shortcut_needs_ctrl_alt_or_super_unless_the_key_types_nothing() {
        use gtk::gdk::{Key, ModifierType};
        let none = ModifierType::empty();
        for key in [Key::a, Key::_5, Key::space, Key::comma, Key::Left, Key::Tab, Key::Return, Key::KP_1] {
            assert!(!usable_as_global(key, none), "{key:?} alone");
        }
        assert!(!usable_as_global(Key::A, ModifierType::SHIFT_MASK), "Shift+A types a capital");
        for mods in [ModifierType::CONTROL_MASK, ModifierType::ALT_MASK, ModifierType::SUPER_MASK] {
            assert!(usable_as_global(Key::a, mods | ModifierType::SHIFT_MASK));
            assert!(usable_as_global(Key::Left, mods));
        }
        for key in [Key::F5, Key::Print, Key::AudioPlay] {
            assert!(usable_as_global(key, none), "{key:?} alone");
        }
    }

    #[test]
    fn only_a_sound_can_be_previewed() {
        assert_eq!(shutter_kind("pop"), Some("pop"));
        assert_eq!(shutter_kind("system"), Some("system"));
        assert_eq!(shutter_kind("none"), None);
        assert_eq!(shutter_kind("dubstep"), None);
    }
}
