// SPDX-License-Identifier: GPL-3.0-or-later

//! After-capture actions: `ACT-01`, with the modifier overrides from `CAP-15` and the
//! shortcut-variant override from `CAP-14`.
//!
//! Settings live in `spec/08`: `after-screenshot`, `after-recording`,
//! `copy-upload-behavior`, `area-shortcuts-respect-actions`.
//!
//! Pure, and worth being pure: what happens after a capture is decided by four inputs
//! that interact (the configured set, which hotkey was used, which modifiers were held,
//! and whether the overlay is enabled at all), and getting it wrong means silently
//! losing a user's capture or writing a file they did not ask for.

use serde::{Deserialize, Serialize};

use crate::capture::RequestedAction;

/// One after-capture action. `spec/08`'s `after-screenshot` / `after-recording` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AfterAction {
    /// Show the Quick Access Overlay card (M2).
    ShowOverlay,
    Copy,
    /// Write to the export location. Absent means the capture stays in the spool.
    Save,
    Annotate,
    Upload,
    Pin,
    /// Prompt for a name first (`ACT-03`, M4).
    AskName,
    /// Recordings only: open the trim editor.
    OpenEditor,
}

impl AfterAction {
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "show-overlay" => Self::ShowOverlay,
            "copy" => Self::Copy,
            "save" => Self::Save,
            "annotate" => Self::Annotate,
            "upload" => Self::Upload,
            "pin" => Self::Pin,
            "ask-name" => Self::AskName,
            "open-editor" => Self::OpenEditor,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::ShowOverlay => "show-overlay",
            Self::Copy => "copy",
            Self::Save => "save",
            Self::Annotate => "annotate",
            Self::Upload => "upload",
            Self::Pin => "pin",
            Self::AskName => "ask-name",
            Self::OpenEditor => "open-editor",
        }
    }

    /// Every action, in the order `spec/08` §1 writes them and the flow runs them.
    ///
    /// Order is significant, not cosmetic: `show-overlay` must precede `copy` so the
    /// card is on screen while the clipboard write happens, and a settings UI that
    /// appends a re-enabled action at the end of the list would silently reorder the
    /// plan. Kept here rather than in the UI so both ends agree.
    pub const CANONICAL_ORDER: [Self; 8] = [
        Self::ShowOverlay,
        Self::Copy,
        Self::Save,
        Self::Annotate,
        Self::Upload,
        Self::Pin,
        Self::AskName,
        Self::OpenEditor,
    ];
}

/// `spec/08`: "When Copy and Upload are both on", `copy-upload-behavior`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CopyUploadBehavior {
    /// The default: put the image on the clipboard now, and replace it with the link
    /// once the upload finishes.
    #[default]
    ImageThenLink,
    /// Never put the image on the clipboard; wait for the link.
    LinkOnly,
    /// Ignore the upload as far as the clipboard is concerned.
    ImageOnly,
}

impl CopyUploadBehavior {
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "image-then-link" => Self::ImageThenLink,
            "link-only" => Self::LinkOnly,
            "image-only" => Self::ImageOnly,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::ImageThenLink => "image-then-link",
            Self::LinkOnly => "link-only",
            Self::ImageOnly => "image-only",
        }
    }
}

/// What the clipboard should end up holding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardPlan {
    /// Copy the image (and its file URI — `spec/08` Advanced: "File & Image").
    Image,
    /// Copy the image now, then replace it with the link when the upload completes.
    ImageThenLink,
    /// Copy nothing until the link is available.
    LinkWhenUploaded,
}

/// Modifiers held at the moment the capture was confirmed (`CAP-15`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConfirmModifiers {
    /// Ctrl: copy to clipboard regardless of the configured actions.
    pub ctrl: bool,
    /// Shift: skip the auto-applied background preset.
    pub shift: bool,
}

/// The settings this resolution depends on.
#[derive(Debug, Clone)]
pub struct Policy {
    /// The configured set for this media type, in order.
    pub configured: Vec<AfterAction>,
    /// `spec/08` `area-shortcuts-respect-actions`. Off by default, meaning an
    /// "& Copy"-style shortcut *replaces* the configured set rather than adding to it.
    pub shortcuts_respect_configured: bool,
    pub copy_upload: CopyUploadBehavior,
}

impl Default for Policy {
    /// `spec/08`'s defaults for screenshots: show the overlay and copy.
    fn default() -> Self {
        Self {
            configured: vec![AfterAction::ShowOverlay, AfterAction::Copy],
            shortcuts_respect_configured: false,
            copy_upload: CopyUploadBehavior::default(),
        }
    }
}

impl Policy {
    /// `spec/08`'s defaults for recordings: show the overlay and save.
    #[must_use]
    pub fn recording_defaults() -> Self {
        Self {
            configured: vec![AfterAction::ShowOverlay, AfterAction::Save],
            ..Self::default()
        }
    }
}

/// What should actually happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// In the order they should run.
    pub actions: Vec<AfterAction>,
    pub clipboard: Option<ClipboardPlan>,
    /// `CAP-15`: Shift was held, so the auto-applied background preset is skipped.
    pub skip_background_preset: bool,
}

impl Plan {
    #[must_use]
    pub fn has(&self, action: AfterAction) -> bool {
        self.actions.contains(&action)
    }

    /// `ACT-06`: "do not save automatically when only Copy is chosen". Nothing is written
    /// to the export location unless `Save` survived resolution; until then the capture
    /// lives only in the spool.
    #[must_use]
    pub fn writes_to_export_location(&self) -> bool {
        self.has(AfterAction::Save)
    }
}

/// Resolves the configured policy against how this particular capture was invoked.
///
/// `requested` is the action a `Capture Area & …` shortcut, the CLI, or a `octosnap://`
/// URL asked for (`CAP-14`).
#[must_use]
pub fn resolve(
    policy: &Policy,
    requested: Option<RequestedAction>,
    modifiers: ConfirmModifiers,
) -> Plan {
    let mut actions: Vec<AfterAction> = match requested {
        // CAP-14: the "& ..." variants override the configured actions, unless the
        // setting says to respect them, in which case they add to the set.
        Some(request) => {
            let requested_action = from_request(request);
            if policy.shortcuts_respect_configured {
                let mut merged = policy.configured.clone();
                push_unique(&mut merged, requested_action);
                merged
            } else {
                // The overlay is governed by its own setting rather than by the
                // shortcut, so it survives an override.
                let mut replaced = Vec::with_capacity(2);
                if policy.configured.contains(&AfterAction::ShowOverlay) {
                    replaced.push(AfterAction::ShowOverlay);
                }
                push_unique(&mut replaced, requested_action);
                replaced
            }
        }
        None => policy.configured.clone(),
    };

    // CAP-15: Ctrl at confirm copies, whatever the settings say.
    if modifiers.ctrl {
        push_unique(&mut actions, AfterAction::Copy);
    }

    let clipboard = clipboard_plan(&actions, policy.copy_upload);

    Plan {
        actions,
        clipboard,
        skip_background_preset: modifiers.shift,
    }
}

fn from_request(request: RequestedAction) -> AfterAction {
    match request {
        RequestedAction::Copy => AfterAction::Copy,
        RequestedAction::Save => AfterAction::Save,
        RequestedAction::Annotate => AfterAction::Annotate,
        RequestedAction::Upload => AfterAction::Upload,
        RequestedAction::Pin => AfterAction::Pin,
    }
}

fn push_unique(actions: &mut Vec<AfterAction>, action: AfterAction) {
    if !actions.contains(&action) {
        actions.push(action);
    }
}

/// `ACT-01`'s "special rules when Copy + Upload are both on (copy link vs. image)".
fn clipboard_plan(
    actions: &[AfterAction],
    behavior: CopyUploadBehavior,
) -> Option<ClipboardPlan> {
    let copying = actions.contains(&AfterAction::Copy);
    let uploading = actions.contains(&AfterAction::Upload);

    match (copying, uploading) {
        (true, true) => Some(match behavior {
            CopyUploadBehavior::ImageThenLink => ClipboardPlan::ImageThenLink,
            CopyUploadBehavior::LinkOnly => ClipboardPlan::LinkWhenUploaded,
            CopyUploadBehavior::ImageOnly => ClipboardPlan::Image,
        }),
        (true, false) => Some(ClipboardPlan::Image),
        // Uploading without Copy does not touch the clipboard at all.
        (false, _) => None,
    }
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    const NO_MODS: ConfirmModifiers = ConfirmModifiers { ctrl: false, shift: false };
    const CTRL: ConfirmModifiers = ConfirmModifiers { ctrl: true, shift: false };
    const SHIFT: ConfirmModifiers = ConfirmModifiers { ctrl: false, shift: true };

    #[test]
    fn the_default_policy_shows_the_card_and_copies() {
        let plan = resolve(&Policy::default(), None, NO_MODS);
        assert_eq!(plan.actions, vec![AfterAction::ShowOverlay, AfterAction::Copy]);
        assert_eq!(plan.clipboard, Some(ClipboardPlan::Image));
    }

    /// ACT-06: nothing is written to the export location unless Save is in the plan.
    #[test]
    fn the_default_policy_does_not_write_a_file() {
        assert!(!resolve(&Policy::default(), None, NO_MODS).writes_to_export_location());
    }

    #[test]
    fn recording_defaults_save_instead_of_copying() {
        let plan = resolve(&Policy::recording_defaults(), None, NO_MODS);
        assert_eq!(plan.actions, vec![AfterAction::ShowOverlay, AfterAction::Save]);
        assert!(plan.writes_to_export_location());
        assert_eq!(plan.clipboard, None);
    }

    // --- CAP-15 modifiers ---------------------------------------------------

    #[test]
    fn ctrl_at_confirm_copies_even_when_copy_is_not_configured() {
        let policy = Policy {
            configured: vec![AfterAction::Save],
            ..Policy::default()
        };
        let plan = resolve(&policy, None, CTRL);
        assert!(plan.has(AfterAction::Copy));
        assert!(plan.has(AfterAction::Save));
        assert_eq!(plan.clipboard, Some(ClipboardPlan::Image));
    }

    #[test]
    fn ctrl_does_not_duplicate_an_already_configured_copy() {
        let plan = resolve(&Policy::default(), None, CTRL);
        assert_eq!(
            plan.actions.iter().filter(|a| **a == AfterAction::Copy).count(),
            1
        );
    }

    #[test]
    fn shift_only_skips_the_background_preset() {
        let plan = resolve(&Policy::default(), None, SHIFT);
        assert!(plan.skip_background_preset);
        assert_eq!(plan.actions, vec![AfterAction::ShowOverlay, AfterAction::Copy]);
    }

    // --- CAP-14 shortcut variants -------------------------------------------

    /// By default a "Capture Area & Save" shortcut replaces the configured actions, so
    /// it must not also copy.
    #[test]
    fn a_shortcut_variant_replaces_the_configured_set_by_default() {
        let plan = resolve(&Policy::default(), Some(RequestedAction::Save), NO_MODS);
        assert!(plan.has(AfterAction::Save));
        assert!(!plan.has(AfterAction::Copy));
        // The card is governed by the configured set, not by the shortcut, so it stays.
        assert!(plan.has(AfterAction::ShowOverlay));
    }

    #[test]
    fn the_setting_makes_a_shortcut_variant_additive_instead() {
        let policy = Policy {
            shortcuts_respect_configured: true,
            ..Policy::default()
        };
        let plan = resolve(&policy, Some(RequestedAction::Save), NO_MODS);
        assert!(plan.has(AfterAction::Save));
        assert!(plan.has(AfterAction::Copy));
    }

    #[test]
    fn an_override_still_honours_ctrl() {
        let plan = resolve(&Policy::default(), Some(RequestedAction::Pin), CTRL);
        assert!(plan.has(AfterAction::Pin));
        assert!(plan.has(AfterAction::Copy));
    }

    /// A card that is switched off must not reappear because a shortcut was used.
    ///
    /// There used to be a second setting for this, `qao-enabled`, that could only ever
    /// remove `show-overlay` from the plan. It meant the same thing as leaving
    /// `show-overlay` out of `after-screenshot`, it was not in the Preferences dialog,
    /// and it could therefore make the visible switch lie: on, with no card. One
    /// setting, and this test is what proves the override path never resurrects the card.
    #[test]
    fn a_disabled_overlay_stays_disabled_under_an_override() {
        let policy = Policy {
            configured: vec![AfterAction::Copy],
            ..Policy::default()
        };
        let plan = resolve(&policy, Some(RequestedAction::Save), NO_MODS);
        assert!(!plan.has(AfterAction::ShowOverlay));
        assert_eq!(plan.actions, vec![AfterAction::Save]);
    }

    // --- ACT-01 Copy + Upload ------------------------------------------------

    #[test]
    fn copy_plus_upload_copies_the_image_then_the_link_by_default() {
        let policy = Policy {
            configured: vec![AfterAction::Copy, AfterAction::Upload],
            ..Policy::default()
        };
        assert_eq!(
            resolve(&policy, None, NO_MODS).clipboard,
            Some(ClipboardPlan::ImageThenLink)
        );
    }

    #[test]
    fn link_only_waits_for_the_upload() {
        let policy = Policy {
            configured: vec![AfterAction::Copy, AfterAction::Upload],
            copy_upload: CopyUploadBehavior::LinkOnly,
            ..Policy::default()
        };
        assert_eq!(
            resolve(&policy, None, NO_MODS).clipboard,
            Some(ClipboardPlan::LinkWhenUploaded)
        );
    }

    #[test]
    fn image_only_ignores_the_upload() {
        let policy = Policy {
            configured: vec![AfterAction::Copy, AfterAction::Upload],
            copy_upload: CopyUploadBehavior::ImageOnly,
            ..Policy::default()
        };
        assert_eq!(resolve(&policy, None, NO_MODS).clipboard, Some(ClipboardPlan::Image));
    }

    /// Uploading without Copy must leave the clipboard alone — overwriting it would be
    /// an unrequested side effect on something the user may be mid-way through using.
    #[test]
    fn uploading_alone_never_touches_the_clipboard() {
        let policy = Policy {
            configured: vec![AfterAction::Upload],
            ..Policy::default()
        };
        assert_eq!(resolve(&policy, None, NO_MODS).clipboard, None);
    }

    /// The Copy+Upload rule must also fire when Copy arrived via Ctrl rather than
    /// from the settings.
    #[test]
    fn ctrl_copy_alongside_a_configured_upload_uses_the_upload_rule() {
        let policy = Policy {
            configured: vec![AfterAction::Upload],
            ..Policy::default()
        };
        assert_eq!(
            resolve(&policy, None, CTRL).clipboard,
            Some(ClipboardPlan::ImageThenLink)
        );
    }

    // --- wire round trip ----------------------------------------------------

    #[test]
    fn every_action_round_trips_through_the_wire_form() {
        for action in [
            AfterAction::ShowOverlay,
            AfterAction::Copy,
            AfterAction::Save,
            AfterAction::Annotate,
            AfterAction::Upload,
            AfterAction::Pin,
            AfterAction::AskName,
            AfterAction::OpenEditor,
        ] {
            assert_eq!(AfterAction::from_wire(action.as_wire()), Some(action));
        }
    }

    /// A settings value written by a newer build must not be fatal.
    #[test]
    fn copy_upload_behaviors_round_trip() {
        for b in [
            CopyUploadBehavior::ImageThenLink,
            CopyUploadBehavior::LinkOnly,
            CopyUploadBehavior::ImageOnly,
        ] {
            assert_eq!(CopyUploadBehavior::from_wire(b.as_wire()), Some(b));
        }
        assert_eq!(CopyUploadBehavior::from_wire("telepathy"), None);
        // The GSettings default must be the documented one.
        assert_eq!(
            CopyUploadBehavior::from_wire("image-then-link"),
            Some(CopyUploadBehavior::default())
        );
    }

    #[test]
    fn an_unknown_action_is_none() {
        assert_eq!(AfterAction::from_wire("teleport"), None);
    }

    #[test]
    fn an_empty_configured_set_yields_an_empty_plan() {
        let policy = Policy {
            configured: vec![],
            ..Policy::default()
        };
        let plan = resolve(&policy, None, NO_MODS);
        assert!(plan.actions.is_empty());
        assert_eq!(plan.clipboard, None);
        assert!(!plan.writes_to_export_location());
    }
}
