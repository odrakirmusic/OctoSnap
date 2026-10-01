// SPDX-License-Identifier: GPL-3.0-or-later

//! `octosnap://` URLs (`SYS-03`, appendix A.8's command list).
//!
//! The `.desktop` file already claims `x-scheme-handler/octosnap`, so this has to exist:
//! a registered handler that does nothing is worse than no handler, because the link
//! looks like it should work.
//!
//! Two deliberate departures from the reference command list in appendix A.8. Its
//! coordinates have their **origin bottom-left**, which is a macOS convention and would
//! be actively wrong here -- GNOME's stage origin is top-left and every other coordinate
//! in OctoSnap is too (`spec/01` §1), so a URL that meant one thing in the app and
//! another on the wire would be a coordinate bug with a user-facing spelling. And its
//! `display` is a **1-based index**; this takes a connector name, because an index
//! silently renumbers when a monitor is unplugged and `eDP-1` does not.
//!
//! Unknown commands and unknown parameters are reported rather than ignored. This is an
//! automation surface: a typo that silently does nothing costs far more to debug than
//! one that says what it did not understand.

use std::path::PathBuf;

use octosnap_core::{Rect, ScrollDirection};
use octosnap_core::capture::{CaptureMode, RequestedAction};
use octosnap_core::request::{CaptureRequest, parse_rect};

pub const SCHEME: &str = "octosnap://";

/// What a URL asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Capture(CaptureRequest),
    OpenSettings(String),
    CopyLast,
    SaveLast,
    /// `pin?filepath=`: `spec/07` §3.1's "file" source.
    Pin(PathBuf),
    /// `open-annotate?filepath=`: a picture, or a `.octosnap` project, in the editor.
    OpenAnnotate(PathBuf),
    /// `open-from-clipboard`: the clipboard's image in the editor.
    OpenFromClipboard,
    /// `toggle-desktop-icons`, `hide-desktop-icons`, `show-desktop-icons` (`DSK-01`).
    DesktopIcons(DesktopIcons),
    /// `add-quick-access-overlay?filepath=`: a file as a card (`HIS-04`'s external files).
    AddQuickAccessOverlay(PathBuf),
    /// `open-history` (`spec/07` §4.2).
    OpenHistory,
    /// `restore-recently-closed` (`spec/04` §3).
    RestoreRecentlyClosed,
    /// `record-gif[?x&y&width&height]`: start a GIF recording of a rectangle, or of the
    /// whole current monitor when none is given (`spec/06` §3). Stopped by `stop-recording`.
    /// The rectangle is resolved to the current monitor by the caller, not here, because
    /// this module has no bus connection.
    RecordGif(Option<Rect>),
    /// `stop-recording`: stop the recording in progress (`spec/06` §3).
    StopRecording,
    /// `capture-text?filepath=` (`spec/07` §2.1's "also works on a file").
    ///
    /// Its own command rather than a `Capture` with a path, because it is not a capture:
    /// nothing is selected, the extension is never called, and the file the user names is
    /// read where it lies. `linebreaks` is §2.1's second shortcut as a parameter.
    CaptureText { file: PathBuf, linebreaks: Option<bool> },
    /// `scrolling-capture[?x&y&width&height&direction&start]` (`spec/07` §1.1, `spec/03`
    /// §4's URL param `start`). Its other one, `autoscroll`, went with auto-scroll (D153),
    /// and is reported as any unknown parameter is: a URL that asks for the page to be
    /// scrolled for it should hear that it will not be.
    ///
    /// Without a rect the app cannot choose one -- it is the extension's overlay that asks
    /// the user -- so a bare URL opens the selection rather than guessing a rectangle. That
    /// is the difference from `record-gif`, whose bare form means "the whole monitor": a
    /// scrolling capture of a whole monitor is a screenshot.
    ScrollingCapture(ScrollRequest),
}

/// What a `scrolling-capture` URL asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollRequest {
    pub rect: Option<Rect>,
    pub direction: Option<ScrollDirection>,
    pub start: bool,
}

/// What to do with the desktop icons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopIcons {
    Toggle,
    Hide,
    Show,
}

impl DesktopIcons {
    /// The app action's parameter.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Toggle => "toggle",
            Self::Hide => "hide",
            Self::Show => "show",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UrlError {
    #[error("not an octosnap:// URL")]
    WrongScheme,
    #[error("no command: expected something like octosnap://capture-area")]
    NoCommand,
    #[error("unknown command '{0}'")]
    UnknownCommand(String),
    #[error("'{key}': {detail}")]
    BadParameter { key: String, detail: String },
    #[error("'{0}' is not a parameter this command takes")]
    UnknownParameter(String),
}

/// Parses a whole URL.
pub fn parse(url: &str) -> Result<Command, UrlError> {
    let rest = url.strip_prefix(SCHEME).ok_or(UrlError::WrongScheme)?;
    let (command, query) = match rest.split_once('?') {
        Some((command, query)) => (command, query),
        None => (rest, ""),
    };
    // A trailing slash is what a browser adds to an authority-only URL, so
    // `octosnap://capture-area/` has to mean the same as without it.
    let command = command.trim_end_matches('/');
    if command.is_empty() {
        return Err(UrlError::NoCommand);
    }

    let params = Params::parse(query);
    match command {
        "all-in-one" => capture(CaptureMode::AllInOne, &params),
        "capture-area" => capture(CaptureMode::Area, &params),
        "capture-window" => capture(CaptureMode::Window, &params),
        "capture-fullscreen" => capture(CaptureMode::Fullscreen, &params),
        "capture-previous-area" => capture(CaptureMode::PreviousArea, &params),
        "self-timer" => capture(CaptureMode::SelfTimer, &params),
        "copy-last" => params.done().map(|()| Command::CopyLast),
        "save-last" => params.done().map(|()| Command::SaveLast),
        "open-settings" => {
            let tab = params.take("tab")?.unwrap_or_default();
            params.done()?;
            Ok(Command::OpenSettings(tab))
        }
        "pin" => file(&params).map(Command::Pin),
        "open-annotate" => file(&params).map(Command::OpenAnnotate),
        "add-quick-access-overlay" => file(&params).map(Command::AddQuickAccessOverlay),
        "open-from-clipboard" => params.done().map(|()| Command::OpenFromClipboard),
        "toggle-desktop-icons" => params.done().map(|()| Command::DesktopIcons(DesktopIcons::Toggle)),
        "hide-desktop-icons" => params.done().map(|()| Command::DesktopIcons(DesktopIcons::Hide)),
        "show-desktop-icons" => params.done().map(|()| Command::DesktopIcons(DesktopIcons::Show)),
        "open-history" => params.done().map(|()| Command::OpenHistory),
        "restore-recently-closed" => params.done().map(|()| Command::RestoreRecentlyClosed),
        "record-gif" => {
            let rect = optional_rect(&params)?;
            params.done()?;
            Ok(Command::RecordGif(rect))
        }
        "stop-recording" => params.done().map(|()| Command::StopRecording),
        // With a file it reads that file; without one it is an ordinary area capture in
        // `spec/07` §2.1's text mode, which is the shortcut's path and goes through the
        // overlay like every other selection.
        "capture-text" => {
            let file = params.take("filepath")?;
            let linebreaks = params.take_bool("linebreaks")?;
            match file {
                Some(path) if path.trim().is_empty() => Err(UrlError::BadParameter {
                    key: "filepath".to_owned(),
                    detail: "is empty".to_owned(),
                }),
                Some(path) => {
                    params.done()?;
                    Ok(Command::CaptureText { file: PathBuf::from(path), linebreaks })
                }
                None => capture(CaptureMode::Ocr, &params).map(|command| match command {
                    Command::Capture(request) => Command::Capture(CaptureRequest {
                        linebreaks,
                        ..request
                    }),
                    other => other,
                }),
            }
        }
        "scrolling-capture" => {
            let rect = optional_rect(&params)?;
            let direction = match params.take("direction")? {
                Some(value) => Some(ScrollDirection::from_wire(&value).ok_or_else(|| {
                    UrlError::BadParameter {
                        key: "direction".to_owned(),
                        detail: format!("expected down, up, right or left; got '{value}'"),
                    }
                })?),
                None => None,
            };
            let start = params.take_bool("start")?.unwrap_or(false);
            params.done()?;
            Ok(Command::ScrollingCapture(ScrollRequest { rect, direction, start }))
        }
        other => Err(UrlError::UnknownCommand(other.to_owned())),
    }
}

/// Appendix A.8's `filepath`, which the three file commands require. Relative paths are
/// kept as given: the process that opened the link is the one whose directory they mean,
/// and it makes them absolute before the app -- a different process -- sees them.
fn file(params: &Params) -> Result<PathBuf, UrlError> {
    let path = params.take("filepath")?.ok_or_else(|| UrlError::BadParameter {
        key: "filepath".to_owned(),
        detail: "required: the file to open".to_owned(),
    })?;
    if path.trim().is_empty() {
        return Err(UrlError::BadParameter {
            key: "filepath".to_owned(),
            detail: "is empty".to_owned(),
        });
    }
    params.done()?;
    Ok(PathBuf::from(path))
}

/// Appendix A.8 spells the rect as four separate parameters, and so does this, because
/// that is what a URL wants. `--rect x,y,w,h` on the command line is the same value. Shared
/// by `capture` and `record-gif`: both take the same four, all or none.
fn optional_rect(params: &Params) -> Result<Option<Rect>, UrlError> {
    let x = params.take_int("x")?;
    let y = params.take_int("y")?;
    let width = params.take_int("width")?;
    let height = params.take_int("height")?;
    match (x, y, width, height) {
        (Some(x), Some(y), Some(width), Some(height)) => {
            let text = format!("{x},{y},{width},{height}");
            let rect = parse_rect(&text).map_err(|e| UrlError::BadParameter {
                key: "x,y,width,height".to_owned(),
                detail: e.to_string(),
            })?;
            Ok(Some(rect))
        }
        (None, None, None, None) => Ok(None),
        // Half a rect is a mistake, not a default. Guessing at the missing half would
        // capture the wrong region and look like OctoSnap's fault.
        _ => Err(UrlError::BadParameter {
            key: "x,y,width,height".to_owned(),
            detail: "give all four or none".to_owned(),
        }),
    }
}

fn capture(mode: CaptureMode, params: &Params) -> Result<Command, UrlError> {
    let mut request = CaptureRequest::new(mode);

    if let Some(rect) = optional_rect(params)? {
        request = request.with_rect(Some(rect));
    }

    if let Some(display) = params.take("display")? {
        request = request.with_display(display);
    }
    if let Some(action) = params.take("action")? {
        let parsed = RequestedAction::from_wire(&action).ok_or(UrlError::BadParameter {
            key: "action".to_owned(),
            detail: "expected copy, save, annotate, upload or pin".to_owned(),
        })?;
        request = request.with_action(Some(parsed));
    }
    request = request
        .with_freeze(params.take_bool("freeze")?)
        .with_cursor(params.take_bool("cursor")?)
        .with_timer(params.take_int("timer")?);

    params.done()?;
    Ok(Command::Capture(request))
}

/// A parsed query string that remembers which keys have been read, so an unrecognised
/// one can be reported at the end rather than silently dropped.
#[derive(Debug)]
struct Params {
    pairs: Vec<(String, String)>,
    seen: std::cell::RefCell<Vec<String>>,
}

impl Params {
    fn parse(query: &str) -> Self {
        let pairs = query
            .split('&')
            .filter(|part| !part.is_empty())
            .map(|part| match part.split_once('=') {
                Some((key, value)) => (decode(key), decode(value)),
                // A bare flag: `?freeze` means `?freeze=true`, which is what a person
                // types and what every other CLI-adjacent surface accepts.
                None => (decode(part), "true".to_owned()),
            })
            .collect();
        Self { pairs, seen: std::cell::RefCell::new(Vec::new()) }
    }

    fn take(&self, key: &str) -> Result<Option<String>, UrlError> {
        self.seen.borrow_mut().push(key.to_owned());
        Ok(self.pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()))
    }

    fn take_int(&self, key: &str) -> Result<Option<i32>, UrlError> {
        match self.take(key)? {
            Some(value) => value.trim().parse::<i32>().map(Some).map_err(|_| {
                UrlError::BadParameter {
                    key: key.to_owned(),
                    detail: format!("'{value}' is not a whole number"),
                }
            }),
            None => Ok(None),
        }
    }

    fn take_bool(&self, key: &str) -> Result<Option<bool>, UrlError> {
        match self.take(key)?.as_deref() {
            Some("true" | "1" | "yes" | "on") => Ok(Some(true)),
            Some("false" | "0" | "no" | "off") => Ok(Some(false)),
            None => Ok(None),
            Some(other) => Err(UrlError::BadParameter {
                key: key.to_owned(),
                detail: format!("'{other}' is not true or false"),
            }),
        }
    }

    /// Reports the first parameter the command never asked about.
    fn done(&self) -> Result<(), UrlError> {
        let seen = self.seen.borrow();
        match self.pairs.iter().find(|(key, _)| !seen.contains(key)) {
            Some((key, _)) => Err(UrlError::UnknownParameter(key.clone())),
            None => Ok(()),
        }
    }
}

/// Percent-decoding, plus `+` for space as query strings have always spelled it.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    // A stray `%` is more likely a literal than a broken escape, and
                    // failing the whole URL over it would be unhelpful.
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use octosnap_core::{Rect, ScrollDirection};

    use super::*;

    fn request(url: &str) -> CaptureRequest {
        match parse(url).expect("a valid URL") {
            Command::Capture(request) => request,
            other => panic!("expected a capture, got {other:?}"),
        }
    }

    #[test]
    fn a_bare_command_is_a_capture_in_that_mode() {
        assert_eq!(request("octosnap://capture-area").mode, CaptureMode::Area);
        assert_eq!(request("octosnap://capture-fullscreen").mode, CaptureMode::Fullscreen);
        assert_eq!(request("octosnap://all-in-one").mode, CaptureMode::AllInOne);
    }

    /// A browser turns `octosnap://capture-area` into an authority with a trailing
    /// slash, so both spellings have to work.
    #[test]
    fn a_trailing_slash_is_ignored() {
        assert_eq!(request("octosnap://capture-area/").mode, CaptureMode::Area);
    }

    #[test]
    fn the_rect_comes_from_four_parameters() {
        let r = request("octosnap://capture-area?x=10&y=20&width=300&height=200");
        assert_eq!(r.rect, Some(Rect::new(10, 20, 300, 200)));
    }

    /// Guessing at a missing half would capture the wrong region.
    #[test]
    fn a_partial_rect_is_an_error() {
        let e = parse("octosnap://capture-area?x=10&y=20&width=300").expect_err("rejected");
        assert!(matches!(e, UrlError::BadParameter { .. }), "got {e:?}");
    }

    #[test]
    fn an_action_and_a_display_are_carried_through() {
        let r = request("octosnap://capture-window?action=upload&display=DP-2");
        assert_eq!(r.action, Some(RequestedAction::Upload));
        assert_eq!(r.display.as_deref(), Some("DP-2"));
    }

    #[test]
    fn booleans_accept_the_usual_spellings() {
        assert_eq!(request("octosnap://capture-area?freeze=true").freeze, Some(true));
        assert_eq!(request("octosnap://capture-area?freeze=1").freeze, Some(true));
        assert_eq!(request("octosnap://capture-area?freeze=off").freeze, Some(false));
        // A bare flag, as a person would type it.
        assert_eq!(request("octosnap://capture-area?freeze").freeze, Some(true));
        // Absent stays absent, so the user's setting wins.
        assert_eq!(request("octosnap://capture-area").freeze, None);
    }

    #[test]
    fn percent_and_plus_are_decoded() {
        let Command::OpenSettings(tab) =
            parse("octosnap://open-settings?tab=screen%73hots").expect("valid")
        else {
            panic!("expected open-settings");
        };
        assert_eq!(tab, "screenshots");
        assert_eq!(decode("a+b%2Fc"), "a b/c");
    }

    /// An automation surface must not silently ignore a typo.
    #[test]
    fn an_unknown_parameter_is_reported() {
        let e = parse("octosnap://capture-area?actoin=copy").expect_err("rejected");
        assert_eq!(e, UrlError::UnknownParameter("actoin".to_owned()));
    }

    #[test]
    fn an_unknown_command_is_reported() {
        let e = parse("octosnap://capture-everything").expect_err("rejected");
        assert_eq!(e, UrlError::UnknownCommand("capture-everything".to_owned()));
    }

    #[test]
    fn a_bad_action_names_the_alternatives() {
        let e = parse("octosnap://capture-area?action=frobnicate").expect_err("rejected");
        assert!(e.to_string().contains("copy, save, annotate, upload or pin"), "{e}");
    }

    #[test]
    fn another_scheme_is_not_ours() {
        assert_eq!(parse("cleanshot://capture-area"), Err(UrlError::WrongScheme));
        assert_eq!(parse("https://example.com"), Err(UrlError::WrongScheme));
    }

    #[test]
    fn an_empty_command_says_what_one_looks_like() {
        assert_eq!(parse("octosnap://"), Err(UrlError::NoCommand));
    }

    /// `spec/07` §2.1's two ways in: a file, and a selection.
    #[test]
    fn capture_text_reads_a_file_or_starts_a_capture() {
        assert_eq!(
            parse("octosnap://capture-text?filepath=/tmp/page.png"),
            Ok(Command::CaptureText {
                file: PathBuf::from("/tmp/page.png"),
                linebreaks: None
            })
        );
        assert_eq!(
            parse("octosnap://capture-text?filepath=/tmp/page.png&linebreaks=false"),
            Ok(Command::CaptureText {
                file: PathBuf::from("/tmp/page.png"),
                linebreaks: Some(false)
            })
        );
        let Ok(Command::Capture(request)) = parse("octosnap://capture-text") else {
            panic!("a bare capture-text is a capture");
        };
        assert_eq!(request.mode, CaptureMode::Ocr);
        assert_eq!(request.linebreaks, None);
    }

    /// The second shortcut is this parameter, and it must not become a setting: absent
    /// means "whatever `ocr-line-breaks` says", which is not the same as `false`.
    #[test]
    fn linebreaks_is_absent_unless_it_was_asked_for() {
        let Ok(Command::Capture(request)) = parse("octosnap://capture-text?linebreaks=true")
        else {
            panic!("a capture");
        };
        assert_eq!(request.linebreaks, Some(true));
    }

    #[test]
    fn an_empty_filepath_is_rejected_for_a_text_read_too() {
        assert!(matches!(
            parse("octosnap://capture-text?filepath="),
            Err(UrlError::BadParameter { key, .. }) if key == "filepath"
        ));
    }

    #[test]
    fn the_file_commands_take_a_filepath_and_nothing_else() {
        assert_eq!(
            parse("octosnap://pin?filepath=/tmp/a%20b.png"),
            Ok(Command::Pin(PathBuf::from("/tmp/a b.png")))
        );
        assert_eq!(
            parse("octosnap://open-annotate?filepath=shot.png"),
            Ok(Command::OpenAnnotate(PathBuf::from("shot.png")))
        );
        assert_eq!(
            parse("octosnap://add-quick-access-overlay?filepath=/tmp/x.png"),
            Ok(Command::AddQuickAccessOverlay(PathBuf::from("/tmp/x.png")))
        );
        assert!(matches!(
            parse("octosnap://pin"),
            Err(UrlError::BadParameter { key, .. }) if key == "filepath"
        ));
        assert!(matches!(
            parse("octosnap://pin?filepath="),
            Err(UrlError::BadParameter { key, .. }) if key == "filepath"
        ));
        assert_eq!(
            parse("octosnap://pin?filepath=/a.png&x=1"),
            Err(UrlError::UnknownParameter("x".to_owned()))
        );
    }

    #[test]
    fn the_desktop_icon_and_history_commands_take_no_parameters() {
        assert_eq!(parse("octosnap://toggle-desktop-icons"), Ok(Command::DesktopIcons(DesktopIcons::Toggle)));
        assert_eq!(parse("octosnap://hide-desktop-icons/"), Ok(Command::DesktopIcons(DesktopIcons::Hide)));
        assert_eq!(parse("octosnap://show-desktop-icons"), Ok(Command::DesktopIcons(DesktopIcons::Show)));
        assert_eq!(DesktopIcons::Hide.as_wire(), "hide");
        assert_eq!(parse("octosnap://open-history"), Ok(Command::OpenHistory));
        assert_eq!(parse("octosnap://restore-recently-closed"), Ok(Command::RestoreRecentlyClosed));
        assert_eq!(parse("octosnap://open-from-clipboard"), Ok(Command::OpenFromClipboard));
        assert_eq!(
            parse("octosnap://open-history?x=1"),
            Err(UrlError::UnknownParameter("x".to_owned()))
        );
    }

    #[test]
    fn the_last_capture_commands_take_no_parameters() {
        assert_eq!(parse("octosnap://copy-last"), Ok(Command::CopyLast));
        assert_eq!(parse("octosnap://save-last"), Ok(Command::SaveLast));
        assert!(parse("octosnap://copy-last?x=1").is_err());
    }

    #[test]
    fn record_gif_takes_an_optional_rect() {
        // Bare: the caller resolves the whole current monitor.
        assert_eq!(parse("octosnap://record-gif"), Ok(Command::RecordGif(None)));
        assert_eq!(parse("octosnap://record-gif/"), Ok(Command::RecordGif(None)));
        // With a rect, the same four parameters `capture` takes.
        assert_eq!(
            parse("octosnap://record-gif?x=10&y=20&width=640&height=480"),
            Ok(Command::RecordGif(Some(Rect::new(10, 20, 640, 480)))),
        );
        // Half a rect is a mistake here too.
        assert!(matches!(
            parse("octosnap://record-gif?x=10&y=20&width=640"),
            Err(UrlError::BadParameter { .. }),
        ));
        // And it does not silently swallow a typo.
        assert_eq!(
            parse("octosnap://record-gif?witdh=640"),
            Err(UrlError::UnknownParameter("witdh".to_owned())),
        );
    }

    fn scrolling(url: &str) -> ScrollRequest {
        match parse(url).expect("a valid URL") {
            Command::ScrollingCapture(request) => request,
            other => panic!("expected a scrolling capture, got {other:?}"),
        }
    }

    /// `spec/07` §1.1: without a rect the app cannot choose one, so the bare URL opens the
    /// selection rather than guessing a rectangle.
    #[test]
    fn a_bare_scrolling_capture_asks_for_a_selection() {
        assert_eq!(scrolling("octosnap://scrolling-capture"), ScrollRequest::default());
        assert_eq!(scrolling("octosnap://scrolling-capture/").rect, None);
    }

    /// `spec/03` §4's URL param `start`, and the direction beside it.
    #[test]
    fn a_scrolling_capture_carries_its_rect_direction_and_start() {
        let r = scrolling("octosnap://scrolling-capture?x=20&y=100&width=600&height=900&direction=right&start");
        assert_eq!(r.rect, Some(Rect::new(20, 100, 600, 900)));
        assert_eq!(r.direction, Some(ScrollDirection::Right));
        assert!(r.start);
        // Absent stays absent: `start` unset is a pill waiting to be pressed.
        let bare = scrolling("octosnap://scrolling-capture?direction=up");
        assert_eq!(bare.direction, Some(ScrollDirection::Up));
        assert!(!bare.start);
    }

    /// Auto-scroll is gone (D153), and a URL still asking for it hears so rather than
    /// getting a capture it has to scroll by hand without being told.
    #[test]
    fn a_scrolling_capture_reports_autoscroll_as_unknown() {
        assert_eq!(
            parse("octosnap://scrolling-capture?autoscroll&start"),
            Err(UrlError::UnknownParameter("autoscroll".to_owned())),
        );
    }

    #[test]
    fn a_scrolling_capture_refuses_a_direction_it_does_not_know() {
        assert!(matches!(
            parse("octosnap://scrolling-capture?direction=sideways"),
            Err(UrlError::BadParameter { .. }),
        ));
        assert_eq!(
            parse("octosnap://scrolling-capture?heigth=900"),
            Err(UrlError::UnknownParameter("heigth".to_owned())),
        );
    }

    #[test]
    fn stop_recording_takes_no_parameters() {
        assert_eq!(parse("octosnap://stop-recording"), Ok(Command::StopRecording));
        assert_eq!(parse("octosnap://stop-recording/"), Ok(Command::StopRecording));
        assert_eq!(
            parse("octosnap://stop-recording?x=1"),
            Err(UrlError::UnknownParameter("x".to_owned())),
        );
    }
}
