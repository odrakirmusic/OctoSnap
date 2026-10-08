// SPDX-License-Identifier: GPL-3.0-or-later

//! The capture result that crosses from the extension to the app.
//!
//! `spec/10` §3.3 defines this dictionary, and `spec/10` §8 requires it to be written as
//! a JSON twin beside the PNG *before* the app is notified, so a lost D-Bus message
//! cannot lose a capture. That is why this type is `Serialize`/`Deserialize` and not just
//! a decoded-variant struct: the JSON file is the source of truth for recovery.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::geometry::Rect;

/// Capture modes accepted by `BeginCapture` (`spec/10` §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureMode {
    AllInOne,
    Area,
    Window,
    Fullscreen,
    PreviousArea,
    SelfTimer,
    Scrolling,
    Ocr,
    Record,
}

impl CaptureMode {
    /// Parses the wire form. Kept explicit rather than derived from serde so that an
    /// unknown mode from a newer extension is a recoverable `None` rather than an error
    /// that discards an otherwise valid capture.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "all-in-one" => Self::AllInOne,
            "area" => Self::Area,
            "window" => Self::Window,
            "fullscreen" => Self::Fullscreen,
            "previous-area" => Self::PreviousArea,
            "self-timer" => Self::SelfTimer,
            "scrolling" => Self::Scrolling,
            "ocr" => Self::Ocr,
            "record" => Self::Record,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::AllInOne => "all-in-one",
            Self::Area => "area",
            Self::Window => "window",
            Self::Fullscreen => "fullscreen",
            Self::PreviousArea => "previous-area",
            Self::SelfTimer => "self-timer",
            Self::Scrolling => "scrolling",
            Self::Ocr => "ocr",
            Self::Record => "record",
        }
    }
}

/// What the invoker asked to happen to the capture, from the CLI or the URL scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestedAction {
    Copy,
    Save,
    Annotate,
    Upload,
    Pin,
}

impl RequestedAction {
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "copy" => Self::Copy,
            "save" => Self::Save,
            "annotate" => Self::Annotate,
            "upload" => Self::Upload,
            "pin" => Self::Pin,
            // An empty string is the documented "no action requested" value, not an error.
            _ => return None,
        })
    }

    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Save => "save",
            Self::Annotate => "annotate",
            Self::Upload => "upload",
            Self::Pin => "pin",
        }
    }

    /// Every value, for a CLI's `--action` help text and the settings UI.
    pub const ALL: [Self; 5] =
        [Self::Copy, Self::Save, Self::Annotate, Self::Upload, Self::Pin];
}

/// Source window information, present for window captures and for the top window under
/// the centre of an area selection.
///
/// Flattened into `CaptureResult`'s serialised form so the JSON twin has the same shape
/// as the D-Bus dictionary in `spec/10` §3.3. One wire shape, not two: the twin exists to
/// recover a capture whose D-Bus message was lost (`spec/10` §8), and a twin the app
/// cannot parse would defeat the point.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceWindow {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub app_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub app_name: String,
    #[serde(default, rename = "window_title", skip_serializing_if = "String::is_empty")]
    pub title: String,
}

impl SourceWindow {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.app_id.is_empty() && self.app_name.is_empty() && self.title.is_empty()
    }
}

/// A fresh capture id: a ULID, as the extension's spool names its files (`spec/10` §4),
/// for a capture the app itself makes -- a file the user opened, the clipboard's image.
#[must_use]
pub fn fresh_id() -> String {
    // ulid 3.0 dropped `Ulid::new()`; `from_datetime` is the current constructor.
    ulid::Ulid::from_datetime(std::time::SystemTime::now()).to_string()
}

/// `spec/10` §3.3 capture result dictionary.
///
/// `background` from the spec is deliberately absent: it only carries meaning once the
/// background tool exists (M6, `spec/07`), and inventing its shape now would fix a
/// contract before the feature that defines it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureResult {
    /// PNG in the spool, in **physical** pixels.
    pub path: PathBuf,
    /// The JSON twin of this struct, written before the app is notified.
    pub meta_path: PathBuf,
    pub mode: CaptureMode,
    /// The captured region in **logical** stage coordinates.
    pub rect: Rect,
    /// Monitor or resource scale the capture was taken at. `path` is `rect` x `scale`.
    pub scale: f64,
    /// Connector of the monitor the capture came from.
    pub display: String,
    /// Cursor position and size, logical, when the cursor was included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_rect: Option<Rect>,
    #[serde(flatten)]
    pub source_window: SourceWindow,
    /// True when `path` carries real alpha, i.e. a window capture with its shadow cut out.
    pub window_alpha: bool,
    /// Microseconds since the Unix epoch.
    pub timestamp: u64,
    /// When the user finished deciding, in microseconds since the Unix epoch.
    ///
    /// The start of `spec/00` §9's 300 ms budget. `Option` and `#[serde(default)]`
    /// because a twin written before this field existed must still open -- the spool
    /// outlives a version of the app, and `spec/04` §7's history is meant to be
    /// restorable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_at: Option<u64>,
    /// How long the shell's fly animation runs; the card fades in when it ends.
    pub animation_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_action: Option<RequestedAction>,
    /// Modifier mask held at the moment of confirmation, for copy-override and
    /// skip-preset behaviour (`spec/03`).
    pub modifiers: u32,
    /// A finished recording's own length, in milliseconds (`spec/04` §1's badge, with
    /// the byte size read from the file). `None` for a screenshot. Persisted in the twin
    /// so a GIF restored from history still shows its badge (`spec/04` §7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// A file the user opened with the app rather than a capture the extension took
    /// (`HIS-04`): `octosnap pin`, `annotate`, `add-overlay`, or "Open with". The file was
    /// copied into the spool so that everything downstream -- the card, the pin, the
    /// editor, history -- treats it as a capture; this is what remembers that it was not.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub external: bool,
    /// `spec/07` §2.1's second text shortcut, carried back from the request that started
    /// the capture: keep the line breaks for this read. `None` is `spec/08` §1's setting.
    ///
    /// On the result and not only on the request because the read happens *after* the
    /// round trip -- the extension takes the pixels and hands them back, and by then the
    /// request is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linebreaks: Option<bool>,
    /// The `.octosnap` project this picture is a render of, when the file holds exactly
    /// the state rendered (D167). An editor's render carries it to its card and, in the
    /// twin, into the history and back out: Annotate opens the project rather than the
    /// flattened picture, and the history's Projects chip finds the entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<PathBuf>,
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn sample() -> CaptureResult {
        CaptureResult {
            path: PathBuf::from("/home/u/.cache/octosnap/spool/01J.png"),
            meta_path: PathBuf::from("/home/u/.cache/octosnap/spool/01J.json"),
            mode: CaptureMode::Area,
            rect: Rect::new(10, 20, 512, 384),
            scale: 1.25,
            display: "eDP-1".to_owned(),
            cursor_rect: None,
            source_window: SourceWindow::default(),
            window_alpha: false,
            timestamp: 1_757_251_200_000_000,
            confirmed_at: None,
            animation_ms: 220,
            requested_action: Some(RequestedAction::Copy),
            modifiers: 0,
            external: false,
            linebreaks: None,
            project: None,
            duration_ms: None,
        }
    }

    /// The JSON twin is a recovery path, so a round trip has to be lossless.
    #[test]
    fn json_twin_round_trips() {
        let original = sample();
        let json = serde_json::to_string(&original).expect("serialize");
        let back: CaptureResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, back);
    }

    /// The twin must be flat, matching `spec/10` §3.3's dictionary, or the recovery
    /// path in `spec/10` §8 cannot read what the extension wrote.
    #[test]
    fn the_json_twin_is_flat_and_omits_empty_fields() {
        let mut c = sample();
        c.source_window = SourceWindow {
            app_id: "org.mozilla.firefox".to_owned(),
            app_name: "Firefox".to_owned(),
            title: "OctoSnap".to_owned(),
        };
        let json = serde_json::to_string(&c).expect("serialize");

        assert!(json.contains(r#""app_id":"org.mozilla.firefox""#), "{json}");
        assert!(json.contains(r#""window_title":"OctoSnap""#), "{json}");
        // Nested under a source_window key would break the extension's writer.
        assert!(!json.contains("source_window"), "{json}");
        // cursor_rect is None here and must not appear as a null.
        assert!(!json.contains("cursor_rect"), "{json}");
        // Nor a project, which only an editor's render has (D167).
        assert!(!json.contains("project"), "{json}");

        let back: CaptureResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, c);
    }

    /// The real thing: a twin captured from `extension/src/spool.ts`'s actual output,
    /// kept as a fixture. This is the cross-language contract -- the extension writes
    /// this file and the app's recovery path (`spec/10` §8) has to read it, and the two
    /// are written in different languages by different code. A shape change on either
    /// side fails here.
    #[test]
    fn the_extensions_real_twin_output_parses() {
        let json = include_str!("../../../tests/fixtures/spool-twin-area.json");
        let c: CaptureResult = serde_json::from_str(json).expect("parse the fixture");

        assert_eq!(c.mode, CaptureMode::Area);
        assert_eq!(c.rect, Rect::new(704, 348, 512, 384));
        assert_eq!(c.scale, 1.0);
        assert_eq!(c.rect.to_physical(c.scale), (512, 384));
        assert!(c.source_window.is_empty());
        assert_eq!(c.requested_action, None);
        assert_eq!(c.cursor_rect, None);
        assert!(c.timestamp > 0);
        // PathBuf::ends_with matches whole path components, not string suffixes, so
        // `ends_with(".png")` would ask for a component literally named ".png".
        assert_eq!(c.path.extension(), Some(std::ffi::OsStr::new("png")));
        assert_eq!(c.meta_path.extension(), Some(std::ffi::OsStr::new("json")));
        // The two must name the same spool entry.
        assert_eq!(c.path.file_stem(), c.meta_path.file_stem());

        // And it must survive a round trip, since the app rewrites twins when a capture
        // is renamed or retained into history.
        let again = serde_json::to_string(&c).expect("serialize");
        assert_eq!(serde_json::from_str::<CaptureResult>(&again).expect("reparse"), c);
    }

    /// A twin written by the extension, by hand, must parse -- this is the exact shape
    /// `extension/src/spool.ts` emits.
    #[test]
    fn a_hand_written_twin_parses() {
        let json = r#"{
            "path": "/spool/01J.png",
            "meta_path": "/spool/01J.json",
            "mode": "area",
            "rect": {"x": 10, "y": 20, "width": 512, "height": 384},
            "scale": 1.25,
            "display": "eDP-1",
            "window_alpha": false,
            "timestamp": 1757251200000000,
            "animation_ms": 220,
            "modifiers": 0
        }"#;
        let c: CaptureResult = serde_json::from_str(json).expect("parse twin");
        assert_eq!(c.mode, CaptureMode::Area);
        assert_eq!(c.rect.to_physical(c.scale), (640, 480));
        assert!(c.source_window.is_empty());
        assert_eq!(c.requested_action, None);
    }

    #[test]
    fn capture_modes_round_trip_through_the_wire_form() {
        for mode in [
            CaptureMode::AllInOne,
            CaptureMode::Area,
            CaptureMode::Window,
            CaptureMode::Fullscreen,
            CaptureMode::PreviousArea,
            CaptureMode::SelfTimer,
            CaptureMode::Scrolling,
            CaptureMode::Ocr,
            CaptureMode::Record,
        ] {
            assert_eq!(CaptureMode::from_wire(mode.as_wire()), Some(mode));
        }
    }

    /// A mode from a newer extension must not be fatal.
    #[test]
    fn unknown_mode_is_none_not_a_panic() {
        assert_eq!(CaptureMode::from_wire("teleport"), None);
        assert_eq!(RequestedAction::from_wire(""), None);
    }

    /// A capture at 1.25 must describe a PNG of exactly the physical size.
    #[test]
    fn rect_and_scale_describe_the_png_size() {
        let c = sample();
        assert_eq!(c.rect.to_physical(c.scale), (640, 480));
    }
}
