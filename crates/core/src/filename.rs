// SPDX-License-Identifier: GPL-3.0-or-later

//! File-name templates: `ACT-07`, tokens specified in `spec/08` § "File-name template
//! tokens".
//!
//! Pure, so the live preview `spec/08` asks for in the settings editor and the actual
//! naming on save can never disagree -- they call this.

use chrono::{DateTime, Datelike, Local, Timelike, Utc};
use serde::{Deserialize, Serialize};

/// `spec/08`: the default observed on the reference machine, with its first word the
/// `{type}` token (D133). A still renders exactly as observed; a recording is named
/// "Recording …" rather than "Screenshot ….gif".
pub const DEFAULT_TEMPLATE: &str = "{type} {yyyy}-{MM}-{dd} at {HH}.{mm}.{ss}";

/// `{type}` (`spec/08`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureType {
    Screenshot,
    Recording,
    Scrolling,
}

impl CaptureType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Screenshot => "Screenshot",
            Self::Recording => "Recording",
            Self::Scrolling => "Scrolling",
        }
    }
}

/// Everything a template can refer to.
#[derive(Debug, Clone)]
pub struct Context {
    /// When the capture happened, always held in UTC; `use_utc` decides how it renders.
    pub timestamp: DateTime<Utc>,
    /// `spec/08`: "Use UTC in names", `filename-utc`.
    pub use_utc: bool,
    /// `{n}`, already resolved from `filename-counter-start` plus how many exist.
    pub counter: u32,
    /// Zero-padding width for `{n}`, 1-4 per `spec/08`.
    pub counter_width: u8,
    /// `{app}` -- the source window's application name.
    pub app: String,
    /// `{window}` -- the source window's title.
    pub window: String,
    pub capture_type: CaptureType,
    /// `{w}` and `{h}`, in physical pixels, matching what the file contains.
    pub width: u32,
    pub height: u32,
    /// `spec/08`/`CAP-16`: append `@2x` when the capture is at a higher density and the
    /// setting is on.
    pub retina_suffix: bool,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            timestamp: Utc::now(),
            use_utc: false,
            counter: 1,
            counter_width: 1,
            app: String::new(),
            window: String::new(),
            capture_type: CaptureType::Screenshot,
            width: 0,
            height: 0,
            retina_suffix: false,
        }
    }
}

/// Characters that cannot appear in a file name.
///
/// `spec/08` says only "illegal characters removed", which on macOS means `/` and `:`.
/// The wider set is stripped here deliberately: capture files get shared, dropped into
/// archives and synced to other systems, and a window title containing `?` or `"` is
/// entirely ordinary. Control characters go too, since a newline in a file name is
/// technically legal on Linux and unhelpful everywhere.
fn is_illegal(c: char) -> bool {
    matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
}

/// Strips illegal characters and collapses the whitespace that leaves behind.
///
/// Also trims leading dots, so a title beginning with one cannot produce a hidden file.
#[must_use]
pub fn sanitize(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last_was_space = false;

    for c in value.chars() {
        // A newline or a tab is whitespace, and the sensible normalisation of whitespace
        // is a space, not deletion -- `spec/08` says illegal characters are "removed",
        // but removing a newline turns "line\none" into "lineone". Other control
        // characters carry no such meaning and simply go.
        let c = if c.is_control() && c.is_whitespace() { ' ' } else { c };
        if is_illegal(c) {
            continue;
        }

        // Removing a character often leaves a double space: "Foo : Bar" -> "Foo Bar".
        let is_space = c == ' ';
        if is_space && last_was_space {
            continue;
        }
        last_was_space = is_space;
        out.push(c);
    }

    out.trim().trim_start_matches('.').trim().to_owned()
}

/// Renders a template.
///
/// An unrecognised token is left in place rather than dropped, so a typo is visible in
/// the settings preview instead of silently producing a shorter name.
#[must_use]
pub fn render(template: &str, context: &Context) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;

    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];

        match after.find('}') {
            Some(end) => {
                let token = &after[1..end];
                match substitute(token, context) {
                    Some(value) => out.push_str(&value),
                    // Unknown token: keep it verbatim, braces included.
                    None => out.push_str(&after[..=end]),
                }
                rest = &after[end + 1..];
            }
            // An unclosed brace is literal text.
            None => {
                out.push_str(after);
                rest = "";
            }
        }
    }
    out.push_str(rest);

    if context.retina_suffix {
        out.push_str("@2x");
    }

    let cleaned = sanitize(&out);
    if cleaned.is_empty() {
        // A template that renders to nothing would produce a file called ".png".
        context.capture_type.as_str().to_owned()
    } else {
        cleaned
    }
}

fn substitute(token: &str, context: &Context) -> Option<String> {
    // Rendering in local time is the default; `spec/08`'s UTC setting opts out.
    let local: DateTime<Local> = context.timestamp.into();

    macro_rules! date {
        ($f:expr) => {
            Some(if context.use_utc {
                context.timestamp.format($f).to_string()
            } else {
                local.format($f).to_string()
            })
        };
    }

    let (year, month, day) = if context.use_utc {
        (context.timestamp.year(), context.timestamp.month(), context.timestamp.day())
    } else {
        (local.year(), local.month(), local.day())
    };
    let (hour24, minute, second) = if context.use_utc {
        (context.timestamp.hour(), context.timestamp.minute(), context.timestamp.second())
    } else {
        (local.hour(), local.minute(), local.second())
    };

    match token {
        "yyyy" => Some(format!("{year:04}")),
        "yy" => Some(format!("{:02}", year.rem_euclid(100))),
        "MM" => Some(format!("{month:02}")),
        "MMM" => date!("%b"),
        "MMMM" => date!("%B"),
        "dd" => Some(format!("{day:02}")),
        "ddd" => date!("%a"),
        "HH" => Some(format!("{hour24:02}")),
        "hh" => {
            let h = match hour24 % 12 {
                0 => 12,
                h => h,
            };
            Some(format!("{h:02}"))
        }
        "mm" => Some(format!("{minute:02}")),
        "ss" => Some(format!("{second:02}")),
        "a" => Some(if hour24 < 12 { "AM".to_owned() } else { "PM".to_owned() }),
        "n" => {
            let width = context.counter_width.clamp(1, 4) as usize;
            Some(format!("{:0width$}", context.counter, width = width))
        }
        "app" => Some(sanitize(&context.app)),
        "window" => Some(sanitize(&context.window)),
        "type" => Some(context.capture_type.as_str().to_owned()),
        "w" => Some(context.width.to_string()),
        "h" => Some(context.height.to_string()),
        _ => None,
    }
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// 2026-09-07 13:04:05 UTC, a Monday.
    fn ctx() -> Context {
        Context {
            timestamp: Utc.with_ymd_and_hms(2026, 9, 7, 13, 4, 5).unwrap(),
            use_utc: true,
            counter: 7,
            counter_width: 1,
            app: "Firefox".to_owned(),
            window: "OctoSnap — Mozilla Firefox".to_owned(),
            capture_type: CaptureType::Screenshot,
            width: 1920,
            height: 1200,
            retina_suffix: false,
        }
    }

    #[test]
    fn the_default_template_renders_the_observed_shape() {
        assert_eq!(render(DEFAULT_TEMPLATE, &ctx()), "Screenshot 2026-09-07 at 13.04.05");
    }

    /// D133: the same default, for a GIF, says what the file is.
    #[test]
    fn the_default_template_names_a_recording_as_one() {
        let recording = Context { capture_type: CaptureType::Recording, ..ctx() };
        assert_eq!(render(DEFAULT_TEMPLATE, &recording), "Recording 2026-09-07 at 13.04.05");
    }

    #[test]
    fn every_date_and_time_token_resolves() {
        let c = ctx();
        assert_eq!(render("{yyyy}", &c), "2026");
        assert_eq!(render("{yy}", &c), "26");
        assert_eq!(render("{MM}", &c), "09");
        assert_eq!(render("{MMM}", &c), "Sep");
        assert_eq!(render("{MMMM}", &c), "September");
        assert_eq!(render("{dd}", &c), "07");
        assert_eq!(render("{ddd}", &c), "Mon");
        assert_eq!(render("{HH}", &c), "13");
        assert_eq!(render("{mm}", &c), "04");
        assert_eq!(render("{ss}", &c), "05");
    }

    #[test]
    fn twelve_hour_and_meridiem_agree() {
        let mut c = ctx();
        assert_eq!(render("{hh}{a}", &c), "01PM");

        c.timestamp = Utc.with_ymd_and_hms(2026, 9, 7, 0, 30, 0).unwrap();
        assert_eq!(render("{hh}{a}", &c), "12AM");

        c.timestamp = Utc.with_ymd_and_hms(2026, 9, 7, 12, 30, 0).unwrap();
        assert_eq!(render("{hh}{a}", &c), "12PM");

        c.timestamp = Utc.with_ymd_and_hms(2026, 9, 7, 9, 0, 0).unwrap();
        assert_eq!(render("{hh}{a}", &c), "09AM");
    }

    #[test]
    fn the_counter_pads_to_its_width() {
        let mut c = ctx();
        assert_eq!(render("{n}", &c), "7");
        c.counter_width = 3;
        assert_eq!(render("{n}", &c), "007");
        c.counter_width = 4;
        c.counter = 1234;
        assert_eq!(render("{n}", &c), "1234");
    }

    /// `spec/08` bounds the width at 1-4; anything else must not produce a wild name.
    #[test]
    fn an_out_of_range_counter_width_is_clamped() {
        let mut c = ctx();
        c.counter_width = 40;
        assert_eq!(render("{n}", &c).len(), 4);
        c.counter_width = 0;
        assert_eq!(render("{n}", &c), "7");
    }

    #[test]
    fn a_counter_wider_than_its_padding_is_not_truncated() {
        let mut c = ctx();
        c.counter = 99_999;
        c.counter_width = 2;
        assert_eq!(render("{n}", &c), "99999");
    }

    #[test]
    fn metadata_tokens_resolve() {
        let c = ctx();
        assert_eq!(render("{app}", &c), "Firefox");
        assert_eq!(render("{type}", &c), "Screenshot");
        assert_eq!(render("{w}x{h}", &c), "1920x1200");
    }

    // --- illegal characters -------------------------------------------------

    /// A window title with a path or a colon in it is entirely ordinary, and would
    /// otherwise produce an unopenable name or a stray directory.
    #[test]
    fn illegal_characters_are_stripped_from_substituted_values() {
        let mut c = ctx();
        c.window = "src/main.rs: line 42 <modified>".to_owned();
        let out = render("{window}", &c);
        assert!(!out.contains('/'), "{out}");
        assert!(!out.contains(':'), "{out}");
        assert!(!out.contains('<'), "{out}");
        assert_eq!(out, "srcmain.rs line 42 modified");
    }

    #[test]
    fn removing_a_character_does_not_leave_a_double_space() {
        assert_eq!(sanitize("Foo : Bar"), "Foo Bar");
        assert_eq!(sanitize("a  b"), "a b");
    }

    #[test]
    fn control_characters_and_newlines_go() {
        assert_eq!(sanitize("line\none\ttwo\u{0}"), "line one two");
    }

    /// A leading dot would make the capture a hidden file.
    #[test]
    fn a_leading_dot_is_trimmed() {
        assert_eq!(sanitize(".hidden"), "hidden");
        assert_eq!(sanitize("  ..nested"), "nested");
    }

    #[test]
    fn illegal_characters_in_the_template_itself_are_stripped_too() {
        let c = ctx();
        assert_eq!(render("shots/{yyyy}", &c), "shots2026");
    }

    // --- robustness ---------------------------------------------------------

    /// A typo should be visible in the settings preview, not silently swallowed.
    #[test]
    fn an_unknown_token_is_left_verbatim() {
        let c = ctx();
        assert_eq!(render("{yyyy}-{nope}", &c), "2026-{nope}");
    }

    #[test]
    fn an_unclosed_brace_is_literal_text() {
        let c = ctx();
        assert_eq!(render("{yyyy}-{oops", &c), "2026-{oops");
    }

    #[test]
    fn a_template_that_renders_to_nothing_still_yields_a_name() {
        let c = ctx();
        assert_eq!(render("", &c), "Screenshot");
        assert_eq!(render("///", &c), "Screenshot");
    }

    #[test]
    fn an_empty_metadata_value_is_simply_empty() {
        let mut c = ctx();
        c.app = String::new();
        assert_eq!(render("shot-{app}-{n}", &c), "shot--7");
    }

    #[test]
    fn the_retina_suffix_lands_at_the_end() {
        let mut c = ctx();
        c.retina_suffix = true;
        assert_eq!(render("{type} {n}", &c), "Screenshot 7@2x");
    }

    #[test]
    fn text_outside_tokens_is_preserved() {
        let c = ctx();
        assert_eq!(render("pre {yyyy} mid {MM} post", &c), "pre 2026 mid 09 post");
    }

    /// The whole point of the UTC setting: the same instant renders differently.
    /// Asserted as a relationship rather than a fixed string, because the test machine's
    /// zone is not fixed.
    #[test]
    fn the_utc_setting_changes_the_rendering() {
        let utc = Context { use_utc: true, ..ctx() };
        let local = Context { use_utc: false, ..ctx() };

        let rendered_utc = render("{yyyy}{MM}{dd}{HH}{mm}", &utc);
        let rendered_local = render("{yyyy}{MM}{dd}{HH}{mm}", &local);

        assert_eq!(rendered_utc, "202609071304");
        let offset = Local
            .from_utc_datetime(&utc.timestamp.naive_utc())
            .offset()
            .local_minus_utc();
        if offset == 0 {
            assert_eq!(rendered_local, rendered_utc);
        } else {
            assert_ne!(rendered_local, rendered_utc);
        }
    }
}
