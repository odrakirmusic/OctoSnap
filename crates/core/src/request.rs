// SPDX-License-Identifier: GPL-3.0-or-later

//! What a caller asks for when it starts a capture: `BeginCapture`'s options dictionary
//! (`spec/10` §3.1) and the `capture` GApplication action that fronts it (`spec/10` §3.2).
//!
//! This lives in `core` rather than in `crates/shell` because three separate callers
//! build one: the CLI, the `octosnap://` URL handler, and the panel menu by way of the
//! app. Encoding the dictionary is `crates/shell`'s job; agreeing on what is *in* it is
//! this type's.
//!
//! Every field is `Option`, and `None` means "the extension decides" rather than a value
//! chosen here. That distinction is the point of the type: `freeze` and `cursor` have
//! real defaults that are user settings the extension owns (`spec/08` §5), so a CLI that
//! always sent a boolean would silently override the user's preference on every
//! invocation. Absent is not the same as false.

use crate::capture::{CaptureMode, RequestedAction};
use crate::geometry::Rect;

/// A parsed capture request, ready to be encoded as `BeginCapture`'s arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureRequest {
    pub mode: CaptureMode,
    /// An explicit rect skips the overlay entirely (`CAP-05`'s instant path).
    pub rect: Option<Rect>,
    /// Connector name, e.g. `eDP-1` (`spec/10` §3.1 calls this `display`).
    pub display: Option<String>,
    /// `CAP-09`. Freeze the screen for the duration of the selection.
    pub freeze: Option<bool>,
    /// `CAP-11`. Include the pointer in the capture.
    pub cursor: Option<bool>,
    /// `CAP-06`. Self-timer delay in seconds; `spec/08` §5's default is 5.
    pub timer: Option<i32>,
    /// What to do with the result, overriding the configured after-capture set for this
    /// one capture (`ACT-01`, `spec/10` §3.3's `requested_action`).
    pub action: Option<RequestedAction>,
    /// `spec/03` §4's scrolling URL parameter: begin without waiting for the Start button.
    /// Its twin, `autoscroll`, went with auto-scroll (D153).
    pub start: Option<bool>,
    /// `spec/07` §2.1's two text shortcuts, and appendix A.8's `capture-text?linebreaks=`:
    /// keep the page's line breaks for *this* read rather than joining the paragraphs.
    ///
    /// `None` leaves it to `spec/08` §1's `ocr-line-breaks`, which is the whole reason it
    /// is an `Option`: `capture-text-single-line` is not a setting the user changed, so a
    /// shortcut that wrote one would turn a one-off into a preference.
    pub linebreaks: Option<bool>,
}

impl CaptureRequest {
    #[must_use]
    pub const fn new(mode: CaptureMode) -> Self {
        Self {
            mode,
            rect: None,
            display: None,
            freeze: None,
            cursor: None,
            timer: None,
            action: None,
            start: None,
            linebreaks: None,
        }
    }

    #[must_use]
    pub fn with_rect(mut self, rect: Option<Rect>) -> Self {
        self.rect = rect;
        self
    }

    /// An empty connector is treated as unset, because that is what a caller that did
    /// not pass `--display` looks like once the value has been through D-Bus: `a{sv}`
    /// has no null, so the app sends `''` rather than omitting the key.
    #[must_use]
    pub fn with_display(mut self, display: impl Into<String>) -> Self {
        let display = display.into();
        self.display = if display.is_empty() { None } else { Some(display) };
        self
    }

    #[must_use]
    pub fn with_action(mut self, action: Option<RequestedAction>) -> Self {
        self.action = action;
        self
    }

    #[must_use]
    pub fn with_freeze(mut self, freeze: Option<bool>) -> Self {
        self.freeze = freeze;
        self
    }

    #[must_use]
    pub fn with_cursor(mut self, cursor: Option<bool>) -> Self {
        self.cursor = cursor;
        self
    }

    #[must_use]
    pub fn with_timer(mut self, timer: Option<i32>) -> Self {
        self.timer = timer;
        self
    }
}

/// Parses `x,y,w,h` as the CLI and the URL scheme both spell it.
///
/// Rejects a zero or negative extent rather than normalising it: an empty rect would
/// reach the extension as "capture nothing", and `spec/10` §8 prefers a typed error at
/// the boundary to a capture that silently does the wrong thing.
pub fn parse_rect(text: &str) -> Result<Rect, RectParseError> {
    let mut parts = text.split(',');
    let mut next = |field: &'static str| -> Result<i32, RectParseError> {
        let raw = parts.next().ok_or(RectParseError::Missing { field })?.trim();
        raw.parse::<i32>().map_err(|_| RectParseError::NotANumber { field })
    };

    let x = next("x")?;
    let y = next("y")?;
    let width = next("width")?;
    let height = next("height")?;
    if parts.next().is_some() {
        return Err(RectParseError::TooManyParts);
    }
    if width <= 0 || height <= 0 {
        return Err(RectParseError::EmptyRect { width, height });
    }
    Ok(Rect::new(x, y, width, height))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RectParseError {
    #[error("expected x,y,width,height but '{field}' is missing")]
    Missing { field: &'static str },
    #[error("'{field}' is not a whole number")]
    NotANumber { field: &'static str },
    #[error("expected exactly four comma-separated values")]
    TooManyParts,
    #[error("a {width}x{height} rect has no area")]
    EmptyRect { width: i32, height: i32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plain_rect() {
        assert_eq!(parse_rect("10,20,300,200"), Ok(Rect::new(10, 20, 300, 200)));
    }

    #[test]
    fn tolerates_spaces_between_the_parts() {
        assert_eq!(parse_rect("10, 20, 300, 200"), Ok(Rect::new(10, 20, 300, 200)));
    }

    #[test]
    fn accepts_negative_origins_because_monitors_can_sit_left_of_zero() {
        assert_eq!(parse_rect("-1920,0,100,100"), Ok(Rect::new(-1920, 0, 100, 100)));
    }

    #[test]
    fn rejects_a_short_rect() {
        assert_eq!(parse_rect("10,20,300"), Err(RectParseError::Missing { field: "height" }));
    }

    #[test]
    fn rejects_a_long_rect() {
        assert_eq!(parse_rect("1,2,3,4,5"), Err(RectParseError::TooManyParts));
    }

    #[test]
    fn rejects_a_non_numeric_part() {
        assert_eq!(
            parse_rect("10,20,wide,200"),
            Err(RectParseError::NotANumber { field: "width" })
        );
    }

    #[test]
    fn rejects_an_empty_rect() {
        assert_eq!(parse_rect("0,0,0,50"), Err(RectParseError::EmptyRect { width: 0, height: 50 }));
        assert_eq!(
            parse_rect("0,0,50,-3"),
            Err(RectParseError::EmptyRect { width: 50, height: -3 })
        );
    }

    #[test]
    fn an_empty_display_is_treated_as_unset() {
        let request = CaptureRequest::new(CaptureMode::Fullscreen).with_display("");
        assert_eq!(request.display, None);
    }

    #[test]
    fn a_fresh_request_leaves_every_setting_to_the_extension() {
        let request = CaptureRequest::new(CaptureMode::Area);
        assert_eq!(request.freeze, None);
        assert_eq!(request.cursor, None);
        assert_eq!(request.timer, None);
        assert_eq!(request.action, None);
        assert_eq!(request.rect, None);
        assert_eq!(request.display, None);
    }
}
