// SPDX-License-Identifier: GPL-3.0-or-later

//! `ACT-05`: a notification for the actions the user cannot see happen.
//!
//! Deliberately app-side through `GApplication::send_notification` rather than the
//! extension's `Notify` (`spec/10` §3.1). Three reasons, in order of weight: it works in
//! degraded mode with no extension at all, which is where a silent failure is most
//! likely and least explicable; GApplication routes notification buttons back to its own
//! actions with no extra plumbing, which is exactly what `spec/10` §3.1's "actions are
//! GApplication action names" describes; and the notification identity comes from the
//! `.desktop` file, so it shows OctoSnap's name and icon without either half asserting
//! them. The extension's `Notify` stays specified for things only the shell can say --
//! the recording indicator's state, for one.
//!
//! `spec/09` §1's "invoke, act, vanish" is the rule for *when*: notify only when the
//! result is otherwise invisible. A capture that put a card on screen has already told
//! the user everything, and a second notification for it is noise.

use adw::prelude::*;
use gtk::gio;
use tracing::info;

use crate::flow::{Outcome, Recognised};

/// One id for every capture notification, so a burst of captures replaces rather than
/// stacks. `spec/04` is where several captures at once get a visible representation; the
/// notification tray is not it.
const CAPTURE_ID: &str = "capture";

/// Tells the user what happened to a capture, when nothing on screen already has.
///
/// Returns whether anything was sent, which is what the tests assert on: the interesting
/// behaviour here is the *silence*, not the text.
///
/// `enabled` is the Notifications switch, and it does not silence a text capture. That
/// notification is the capture's result, not a notice about one: `spec/07` §2.1 puts the
/// read and its **Show** there and nowhere else, so with the switch off a text capture
/// had no visible end at all. The switch is for captures that worked and would otherwise
/// be silent.
pub fn capture_outcome(
    app: &adw::Application,
    outcome: &Outcome,
    enabled: bool,
) -> bool {
    // D171: a read that found no pack opens the packs instead. A notification was all a
    // text capture had to say it, and one that went by unread left a capture that looked
    // like a screenshot and an empty clipboard. Every read ends here -- a text capture, a
    // file, a card's or a pin's Copy Text, an editor closed while it read -- so they all
    // land on the same Install button, with the read kept until it is pressed.
    if outcome.waits_for_pack() {
        info!("a read waits for a language pack; Settings shows the packs");
        crate::prefs::present(crate::prefs::PACKS);
        return true;
    }
    let Some((title, body)) = describe(outcome) else {
        return false;
    };
    let text = outcome.recognised.is_some();
    if !enabled && !text {
        info!(title, "suppressed a notification; notifications are off");
        return false;
    }

    let notification = gio::Notification::new(&title);
    if let Some(body) = &body {
        notification.set_body(Some(body));
    }
    // A read's buttons are the way to it, and a low-priority notification can go to the
    // tray without ever showing them (`trashed`, below).
    notification.set_priority(if text {
        gio::NotificationPriority::Normal
    } else {
        gio::NotificationPriority::Low
    });

    // A saved file is worth a way to reach it. Copy alone is not: the clipboard has no
    // location to open, and a button that does nothing useful is worse than no button.
    if outcome.saved_to.is_some() {
        notification.add_button("Show in Files", "app.reveal-last");
    }

    // `spec/07` §2.1's **Show**, which is the only way to the result window: a text
    // capture puts nothing on screen, so without this the read is on the clipboard and
    // nowhere the user can look at it.
    if matches!(outcome.recognised, Some(Recognised::Copied(_))) {
        notification.add_button("Show", "app.show-text");
    }

    // A native install with no ONNX Runtime: the body names the package, and the README's
    // section has the rest (D172).
    if outcome.lacks_runtime() && crate::ocr::runtime::refusal().help {
        notification.add_button(HOW_TO_INSTALL, "app.runtime-help");
    }

    app.send_notification(Some(CAPTURE_ID), &notification);
    info!(title, "notified");
    true
}

/// How a read ended, as a toast for the window the user asked from: the editor's Copy
/// Text, and Settings when a pack it installed finished a read that waited (D171).
///
/// The notification's words and buttons, so a read says the same thing wherever it ends:
/// **Show** for the text, for a missing pack the button that installs one, and for a
/// missing runtime the README's section on getting one (D172). `None` for an outcome that
/// is not a read.
#[must_use]
pub fn read_toast(outcome: &Outcome) -> Option<adw::Toast> {
    let (title, button) = toast_words(outcome)?;
    let toast = adw::Toast::new(&title);
    toast.set_timeout(crate::editor::actions::toast_seconds(button.is_some()));
    if let Some((label, action, target)) = button {
        toast.set_button_label(Some(label));
        // Activated on the application, not named on the toast. A toast's action name is
        // looked up from the window it is in, and Settings is a window of its own rather
        // than one of the application's, where `app.show-text` named nothing and Show
        // was drawn insensitive (D171).
        toast.connect_button_clicked(move |_| {
            let Some(app) = gio::Application::default() else { return };
            app.activate_action(action, target.map(ToVariant::to_variant).as_ref());
        });
    }
    Some(toast)
}

/// A toast button's label, its application action, and the action's string target if any.
type Button = (&'static str, &'static str, Option<&'static str>);

/// The button that opens the README's section on ONNX Runtime, on the notification and on
/// the toasts.
const HOW_TO_INSTALL: &str = "How to Install";

/// [`read_toast`]'s words, apart from the widget, so they can be tested without a display.
fn toast_words(outcome: &Outcome) -> Option<(String, Option<Button>)> {
    Some(match outcome.recognised.as_ref()? {
        Recognised::Copied(_) => ("Text copied".to_owned(), Some(("Show", "show-text", None))),
        Recognised::Nothing => ("No text found".to_owned(), None),
        Recognised::Failed(why) if why == crate::flow::NO_PACK => (
            "No language pack is installed".to_owned(),
            Some(("Install\u{2026}", "open-settings", Some(crate::prefs::PACKS))),
        ),
        // One line, where the notification has two: a toast is not wide enough for the
        // sentence, and its button has the rest.
        Recognised::Failed(_) if outcome.lacks_runtime() => {
            let refusal = crate::ocr::runtime::refusal();
            let button = refusal.help.then_some((HOW_TO_INSTALL, "runtime-help", None));
            (refusal.toast.clone(), button)
        }
        Recognised::Failed(why) => (format!("Could not read the text: {why}"), None),
    })
}

/// One id for "this build cannot do that yet", so repeated presses replace rather than
/// stack. A user who clicks Annotate three times has learned the same thing three times.
const UNAVAILABLE_ID: &str = "unavailable";

/// The notification id for one pending deletion.
///
/// One per capture rather than one shared id, unlike `CAPTURE_ID` and `UNAVAILABLE_ID`.
/// Those two replace because the message is the *same* message; two trashed captures are
/// two different files with two different undos, and collapsing them would silently take
/// away the way back from the first one while its three seconds were still running.
#[must_use]
pub fn trash_id(token: u64) -> String {
    format!("trash-{token}")
}

/// `spec/04` §3's undo toast, as a notification with a button.
///
/// The wording is the convention every toast follows -- it says the thing is done and
/// offers the way back -- and it is true from the user's side: the card is gone and the
/// file is going. The three seconds are the mechanism, not the message.
///
/// Normal priority, not `Low`. This one is the only notice of something the user cannot
/// otherwise undo, and a low-priority notification can be collapsed into the tray without
/// ever showing its button.
pub fn trashed(app: &adw::Application, token: u64, name: &str) {
    let notification = gio::Notification::new("Moved to Trash");
    notification.set_body(Some(name));
    notification.set_priority(gio::NotificationPriority::Normal);
    notification.add_button_with_target_value("Undo", "app.undo-trash", Some(&token.to_variant()));
    app.send_notification(Some(&trash_id(token)), &notification);
    info!(token, name, "offered an undo for a trashed capture");
}

/// What a flow has to say for itself beyond a capture's outputs: a recording that would
/// not start, a scrolling capture that stopped on its own. Never behind the Notifications
/// switch, which is for the captures that worked and would otherwise be silent; the
/// project's rule is that a failure always speaks (`action_failed`). `urgent` is Normal
/// priority, which shows a banner; the rest go to the tray.
///
/// `id` replaces: one per flow, so a second failure of the same thing is one message.
pub fn tell(app: &adw::Application, id: &str, title: &str, body: &str, urgent: bool) {
    let notification = gio::Notification::new(title);
    notification.set_body(Some(body));
    notification.set_priority(if urgent {
        gio::NotificationPriority::Normal
    } else {
        gio::NotificationPriority::Low
    });
    app.send_notification(Some(id), &notification);
    info!(id, title, body, "told the user");
}

/// One id for "that did not work", so a run of failures replaces rather than stacks.
const FAILED_ID: &str = "action-failed";

/// Says that an action the user asked for did not happen.
///
/// The counterpart to the card's tick: `spec/09` §1's rule is to speak when the result is
/// otherwise invisible, and a *failure* is the case where that matters most -- the card
/// stays up, which is the right thing for the capture and says nothing at all about why.
pub fn action_failed(app: &adw::Application, action: &str, reason: &str) {
    let notification = gio::Notification::new(&format!("{action} did not work"));
    notification.set_body(Some(reason));
    notification.set_priority(gio::NotificationPriority::Normal);
    app.send_notification(Some(FAILED_ID), &notification);
    info!(action, reason, "reported a failed action");
}

/// Says that a control the user just used belongs to a milestone that has not landed.
///
/// The alternative was a log line, which is what this replaced. A button that logs is
/// indistinguishable from a button that is broken: the card sits there, nothing happens,
/// and the only difference between "not built yet" and "just failed" is in a journal the
/// user is not reading. `spec/09` §1's rule -- speak when the result is otherwise
/// invisible -- applies most strongly when the result is *nothing*.
pub fn unavailable(app: &adw::Application, action: &str, milestone: &str) {
    let notification = gio::Notification::new(&format!("{action} is not available yet"));
    notification.set_body(Some(&format!("It arrives with {milestone}.")));
    notification.set_priority(gio::NotificationPriority::Low);
    app.send_notification(Some(UNAVAILABLE_ID), &notification);
    info!(action, milestone, "reported an unavailable action");
}

/// What the outputs a recording's plan held for its card came to, once the card has gone
/// and they have run (D113). Nothing is on screen by then to say where the GIF went.
pub fn recording_saved(app: &adw::Application, outcome: &Outcome, enabled: bool) {
    let (title, body) = match (&outcome.saved_to, outcome.copied) {
        (Some(path), true) => ("GIF saved and copied", Some(display_name(path))),
        (Some(path), false) => ("GIF saved", Some(display_name(path))),
        (None, true) => ("GIF copied", None),
        (None, false) => return,
    };
    info!(title, "a recording's held outputs ran");
    if !enabled {
        return;
    }
    let notification = gio::Notification::new(title);
    if let Some(body) = body {
        notification.set_body(Some(&body));
    }
    notification.set_priority(gio::NotificationPriority::Low);
    app.send_notification(None, &notification);
}

/// The message for an outcome, or `None` when the user can already see what happened.
///
/// Split out from the sending so `spec/09`'s "only when it is otherwise invisible" rule
/// is testable without a session bus.
#[must_use]
pub fn describe(outcome: &Outcome) -> Option<(String, Option<String>)> {
    // A text capture has no card and never had one, so the rule below does not apply to
    // it -- and it is the one capture whose *result* is invisible rather than merely its
    // side effects. `spec/07` §2.1 gives the wording: "Text copied" with the first line.
    if let Some(recognised) = &outcome.recognised {
        return Some(match recognised {
            Recognised::Copied(first) => {
                ("Text copied".to_owned(), Some(preview(first)))
            }
            // Not a failure and not silent: an area was read and a moment went by, so
            // something has to say why nothing arrived on the clipboard.
            Recognised::Nothing => (
                "No text found".to_owned(),
                Some("There was nothing to recognise in that area.".to_owned()),
            ),
            // Its own title, so a native install is told what it needs before why (D172).
            Recognised::Failed(why) if outcome.lacks_runtime() => {
                (crate::ocr::runtime::refusal().title.to_owned(), Some(why.clone()))
            }
            Recognised::Failed(why) => ("Could not read the text".to_owned(), Some(why.clone())),
        });
    }

    // The card is the notification. Anything else would be saying it twice.
    //
    // `card_shown`, not "a card was requested": a build with no card must still tell the
    // user something, or a capture is a shutter sound and nothing else. D27.
    if outcome.card_shown || outcome.pinned {
        return None;
    }

    // A recording whose plan put up no card says what it made, not "Screenshot".
    let noun = if outcome.recording { "GIF" } else { "Screenshot" };
    match (&outcome.saved_to, outcome.copied) {
        (Some(path), true) => Some((format!("{noun} saved and copied"), Some(display_name(path)))),
        (Some(path), false) => Some((format!("{noun} saved"), Some(display_name(path)))),
        (None, true) => Some((format!("{noun} copied"), None)),
        // Nothing happened that the user asked for. The flow has already logged why, and
        // an empty after-capture set is a configuration the user chose.
        (None, false) => None,
    }
}

/// How much of the first line a notification shows.
///
/// A notification body is one or two lines wide and a recognised line can be a whole
/// paragraph joined into one (`Breaks::Join` is the default), so the untrimmed first line
/// would either be clipped by the daemon at whatever width it happens to have or, on the
/// daemons that wrap, push the buttons off the bottom of the popup.
const PREVIEW: usize = 90;

/// The first line, shortened to something a notification can hold.
fn preview(line: &str) -> String {
    let line = line.trim();
    if line.chars().count() <= PREVIEW {
        return line.to_owned();
    }
    let kept: String = line.chars().take(PREVIEW).collect();
    // Cut at the last space rather than mid-word where there is one near the end.
    let cut = kept.rfind(' ').filter(|at| *at > PREVIEW / 2).unwrap_or(kept.len());
    format!("{}\u{2026}", kept[..cut].trim_end())
}

/// The file's own name, not its whole path: the path is long, the folder is a setting the
/// user picked, and the button below opens it anyway.
fn display_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn outcome() -> Outcome {
        Outcome::default()
    }

    /// The rule that matters: a capture that produced a card says nothing else.
    #[test]
    fn a_capture_that_produced_a_card_is_not_notified() {
        let shown = Outcome {
            card_shown: true,
            copied: true,
            saved_to: Some(PathBuf::from("/tmp/Shot.png")),
            ..outcome()
        };
        assert_eq!(describe(&shown), None);
    }

    /// The default `after-screenshot` is `['show-overlay', 'copy']`, and until `M2` there
    /// is no card. Before D27 that combination produced **no card and no notification**:
    /// the shutter sounded, the thumbnail flew to the corner, and nothing said the image
    /// was on the clipboard. A prototype has to be able to tell you it worked.
    #[test]
    fn a_requested_but_unshown_card_still_notifies() {
        let no_card_yet = Outcome { card_shown: false, copied: true, ..outcome() };
        let (title, _) = describe(&no_card_yet).expect("something must tell the user");
        assert_eq!(title, "Screenshot copied");
    }

    #[test]
    fn a_silent_copy_is_notified_without_a_body() {
        let copied = Outcome { copied: true, ..outcome() };
        let (title, body) = describe(&copied).expect("a message");
        assert_eq!(title, "Screenshot copied");
        assert_eq!(body, None);
    }

    #[test]
    fn a_silent_save_names_the_file_not_the_path() {
        let saved = Outcome {
            saved_to: Some(PathBuf::from("/home/u/Pictures/Screenshots/Shot 1.png")),
            ..outcome()
        };
        let (title, body) = describe(&saved).expect("a message");
        assert_eq!(title, "Screenshot saved");
        assert_eq!(body.as_deref(), Some("Shot 1.png"));
    }

    /// A recording's plan can have no card, and its notification is then about a GIF.
    #[test]
    fn a_recording_is_called_a_gif() {
        let gif = Outcome {
            recording: true,
            saved_to: Some(PathBuf::from("/tmp/Recording.gif")),
            ..outcome()
        };
        let (title, body) = describe(&gif).expect("a message");
        assert_eq!(title, "GIF saved");
        assert_eq!(body.as_deref(), Some("Recording.gif"));
    }

    #[test]
    fn a_save_and_copy_says_both() {
        let both = Outcome {
            copied: true,
            saved_to: Some(PathBuf::from("/tmp/Shot.png")),
            ..outcome()
        };
        let (title, _) = describe(&both).expect("a message");
        assert_eq!(title, "Screenshot saved and copied");
    }

    /// An empty after-capture set is a choice, not a failure to report.
    #[test]
    fn an_outcome_with_nothing_in_it_says_nothing() {
        assert_eq!(describe(&outcome()), None);
    }

    /// `spec/07` §2.1's wording, and its preview.
    #[test]
    fn a_text_capture_says_what_it_copied() {
        let read = Outcome {
            copied: true,
            recognised: Some(Recognised::Copied("The quick brown fox".to_owned())),
            ..outcome()
        };
        let (title, body) = describe(&read).expect("a message");
        assert_eq!(title, "Text copied");
        assert_eq!(body.as_deref(), Some("The quick brown fox"));
    }

    /// The rule against saying it twice is about cards, and a text capture has none --
    /// `card_shown` is false for one by construction, but a future path that set it must
    /// not silence the only thing that tells the user the read worked.
    #[test]
    fn a_text_capture_speaks_even_beside_a_card() {
        let read = Outcome {
            card_shown: true,
            recognised: Some(Recognised::Copied("hello".to_owned())),
            ..outcome()
        };
        assert!(describe(&read).is_some());
    }

    #[test]
    fn a_capture_with_no_text_in_it_is_not_reported_as_a_failure() {
        let empty = Outcome { recognised: Some(Recognised::Nothing), ..outcome() };
        let (title, _) = describe(&empty).expect("a message");
        assert_eq!(title, "No text found");
    }

    #[test]
    fn a_read_that_could_not_run_says_why() {
        let broken = Outcome {
            recognised: Some(Recognised::Failed("no Latin pack is installed".to_owned())),
            ..outcome()
        };
        let (title, body) = describe(&broken).expect("a message");
        assert_eq!(title, "Could not read the text");
        assert_eq!(body.as_deref(), Some("no Latin pack is installed"));
    }

    /// D172: a read that could not load ONNX Runtime says what it needs in the title and
    /// where to get it in the body, and its toast is one line with the way to the rest.
    /// Whichever refusal this machine has: a test run in the Flatpak's builder gets the
    /// Flatpak's, and `ocr::runtime`'s own tests read every one.
    #[test]
    fn a_read_with_no_runtime_says_how_to_get_one() {
        let refusal = crate::ocr::runtime::refusal();
        let broken = Outcome {
            recognised: Some(Recognised::Failed(refusal.body.clone())),
            ..outcome()
        };
        assert!(broken.lacks_runtime());
        assert!(!broken.waits_for_pack());
        let (title, body) = describe(&broken).expect("a message");
        assert_eq!(title, refusal.title);
        assert_eq!(body.as_deref(), Some(refusal.body.as_str()));

        let (line, button) = toast_words(&broken).expect("a toast");
        assert_eq!(line, refusal.toast, "the toast's own line, not the sentence after a colon");
        assert_eq!(button.is_some(), refusal.help);
        if let Some((label, action, target)) = button {
            assert_eq!((label, action, target), (HOW_TO_INSTALL, "runtime-help", None));
        }
    }

    /// Any other failure keeps its words, and is not taken for the runtime's.
    #[test]
    fn a_failure_that_is_not_the_runtime_is_not_its_refusal() {
        let broken = Outcome {
            recognised: Some(Recognised::Failed("the capture is not a PNG".to_owned())),
            ..outcome()
        };
        assert!(!broken.lacks_runtime());
        let (line, button) = toast_words(&broken).expect("a toast");
        assert_eq!(line, "Could not read the text: the capture is not a PNG");
        assert_eq!(button, None);
    }

    /// A joined paragraph is one line and can be hundreds of characters; the popup is not.
    #[test]
    fn a_long_first_line_is_cut_at_a_word() {
        let line = "lorem ipsum dolor sit amet ".repeat(8);
        let short = preview(&line);
        assert!(short.chars().count() <= PREVIEW + 1, "{short}");
        assert!(short.ends_with('\u{2026}'));
        assert!(!short.contains("ipsu\u{2026}"), "cut mid-word: {short}");
    }

    #[test]
    fn a_short_first_line_is_left_alone() {
        assert_eq!(preview("  https://example.com/  "), "https://example.com/");
    }
}
