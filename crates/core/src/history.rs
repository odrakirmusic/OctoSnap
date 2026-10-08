// SPDX-License-Identifier: GPL-3.0-or-later

//! Capture history: `HIS-01`–`HIS-04`, `spec/07` §4 and `spec/04` §7's lifecycle.
//!
//! Pure: what an entry is, which filter chip it answers to, how long it may stay, and how
//! its age reads under a thumbnail. The files -- the move out of the spool, the thumbnail,
//! the janitor's timer -- are the app's, and the app calls this so that the strip, the
//! janitor and "Restore recently closed" cannot disagree about any of it.
//!
//! **A directory per capture, not a database.** `spec/07` §4.1 proposes `history.db` and
//! tags it [P]. The spool already keeps every capture as a PNG beside a JSON twin, the
//! app already reads both, and a history of a few hundred entries is listed in
//! milliseconds. A database would be a second serialisation of the same struct behind a
//! native dependency, and a file that can disagree with the directory it indexes
//! (`docs/decisions.md` D64). So an entry is `history/<id>/` holding the two spool files
//! as they were, a thumbnail, and [`ENTRY_FILE`] with what this module adds.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::capture::{CaptureMode, CaptureResult, SourceWindow};

/// The record this module adds to a filed capture's directory.
pub const ENTRY_FILE: &str = "entry.json";
/// The thumbnail's name inside the directory; `spec/07` §4.1's `thumb_path`.
pub const THUMB_FILE: &str = "thumb.png";

/// The shape of every tile in the strip, width to height.
///
/// `spec/07` §4.2's reference strip shows every capture as the same 16:10 card with the
/// picture filling it, whatever its own shape. The first strip made each tile as wide as
/// its picture wanted, so a scrolling capture was a sliver beside a banner beside a
/// square, and the row read as clutter. One shape is what makes a row of different things
/// scan as one row (D109).
pub const TILE_ASPECT: (u32, u32) = (16, 10);
/// A tile thumbnail's largest size in pixels: twice the tile's logical 208 x 130, so a
/// 2x screen draws it pixel for pixel. The first thumbnails fitted inside 256 px, which a
/// 2x tile stretched to twice its size.
pub const THUMB_MAX: (u32, u32) = (416, 260);

/// The part of a `width` x `height` picture a tile shows, as `(x, y, width, height)`.
///
/// The whole height of a picture wider than a tile, centred on it; the full width of a
/// picture taller than a tile, from its **top** -- a scrolling capture shows the start of
/// the page, which is what its owner remembers, and not a slice of its middle.
#[must_use]
pub fn tile_crop(width: u32, height: u32) -> (u32, u32, u32, u32) {
    let (aw, ah) = (u64::from(TILE_ASPECT.0), u64::from(TILE_ASPECT.1));
    let (w, h) = (u64::from(width.max(1)), u64::from(height.max(1)));
    if w * ah > h * aw {
        let crop = ((h * aw + ah / 2) / ah).clamp(1, w);
        let crop = u32::try_from(crop).unwrap_or(width);
        ((width - crop) / 2, 0, crop, height.max(1))
    } else {
        let crop = ((w * ah + aw / 2) / aw).clamp(1, h);
        (0, 0, width.max(1), u32::try_from(crop).unwrap_or(height))
    }
}

/// The thumbnail's size for a picture of that size: its [`tile_crop`], brought down to
/// [`THUMB_MAX`] and never up.
#[must_use]
pub fn tile_thumb_size(width: u32, height: u32) -> (u32, u32) {
    let (_, _, w, h) = tile_crop(width, height);
    if w <= THUMB_MAX.0 {
        return (w, h);
    }
    let scaled = (u64::from(h) * u64::from(THUMB_MAX.0) + u64::from(w) / 2) / u64::from(w);
    (THUMB_MAX.0, u32::try_from(scaled).unwrap_or(THUMB_MAX.1).max(1))
}

/// `spec/07` §4.1's `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Screenshot,
    Video,
    Gif,
    Scrolling,
    Ocr,
    /// A file opened with the app rather than captured by it (`HIS-04`).
    External,
    /// A `.octosnap` project: §4.2's "Video Projects" chip, generalised to the one
    /// project type this app has. A picture is one when it is the render of a project
    /// file that holds exactly what it shows (D167); nothing filed as one before that.
    Project,
}

impl Kind {
    /// What a capture files as: a file the user opened is a file whatever mode it
    /// claims (`HIS-04`); everything else is its mode.
    #[must_use]
    pub fn of(capture: &CaptureResult) -> Self {
        if capture.external {
            return Self::External;
        }
        match capture.mode {
            // A recording is a GIF or a video by the file it left, because the mode is
            // the same `record` for both (`spec/03` §1) and the format is chosen on the
            // recorder bar (`spec/06` §2), after the mode.
            CaptureMode::Record if is_gif(&capture.path) => Self::Gif,
            CaptureMode::Record => Self::Video,
            CaptureMode::Scrolling => Self::Scrolling,
            CaptureMode::Ocr => Self::Ocr,
            CaptureMode::AllInOne
            | CaptureMode::Area
            | CaptureMode::Window
            | CaptureMode::Fullscreen
            | CaptureMode::PreviousArea
            | CaptureMode::SelfTimer => Self::Screenshot,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Screenshot => "Screenshot",
            Self::Video => "Video",
            Self::Gif => "GIF",
            Self::Scrolling => "Scrolling capture",
            Self::Ocr => "Text capture",
            Self::External => "File",
            Self::Project => "Project",
        }
    }

    /// The symbolic icon the strip marks the kind with (D109): the capture toolbar's own
    /// icon for the modes it has one for -- Scrolling's arrow, Text's magnifier -- so a
    /// kind looks the same where it is taken as where it is found again.
    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Screenshot => "camera-photo-symbolic",
            Self::Video => "camera-video-symbolic",
            // A loop: what a GIF does that a video does not.
            Self::Gif => "media-playlist-repeat-symbolic",
            Self::Scrolling => "go-bottom-symbolic",
            Self::Ocr => "edit-find-symbolic",
            Self::External => "document-open-symbolic",
            Self::Project => "document-edit-symbolic",
        }
    }

    /// Every kind, for the checks that have to cover them all.
    pub const ALL: [Self; 7] = [
        Self::Screenshot,
        Self::Video,
        Self::Gif,
        Self::Scrolling,
        Self::Ocr,
        Self::External,
        Self::Project,
    ];
}

/// `.gif` by extension, case-insensitively; what the recorder writes and what a user
/// opens with the app.
pub fn is_gif(path: &std::path::Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("gif"))
}

/// `spec/07` §4.2's filter chips: "All, Screenshots, Videos, GIFs, Video Projects".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Filter {
    All,
    Screenshots,
    Videos,
    Gifs,
    Projects,
}

impl Filter {
    /// In the order the chips sit, `All` first and active.
    pub const ALL: [Self; 5] = [Self::All, Self::Screenshots, Self::Videos, Self::Gifs, Self::Projects];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Screenshots => "Screenshots",
            Self::Videos => "Videos",
            Self::Gifs => "GIFs",
            Self::Projects => "Projects",
        }
    }

    /// The chip's icon: its kind's, so a chip and the tiles it shows carry the same mark.
    /// `All` has none.
    #[must_use]
    pub const fn icon(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            Self::Screenshots => Some(Kind::Screenshot.icon()),
            Self::Videos => Some(Kind::Video.icon()),
            Self::Gifs => Some(Kind::Gif.icon()),
            Self::Projects => Some(Kind::Project.icon()),
        }
    }

    /// Whether an entry of this kind shows under the chip. A scrolling capture and a
    /// text capture are pictures, so they are screenshots here; a file opened with the
    /// app is whatever picture it was.
    #[must_use]
    pub const fn matches(self, kind: Kind) -> bool {
        match self {
            Self::All => true,
            Self::Screenshots => matches!(
                kind,
                Kind::Screenshot | Kind::Scrolling | Kind::Ocr | Kind::External
            ),
            Self::Videos => matches!(kind, Kind::Video),
            Self::Gifs => matches!(kind, Kind::Gif),
            Self::Projects => matches!(kind, Kind::Project),
        }
    }
}

/// One capture that has left the screen: `spec/07` §4.1's row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// The capture's ULID -- the spool file's stem, and the directory's name.
    pub id: String,
    pub kind: Kind,
    /// When the capture was taken, microseconds since the epoch
    /// (`CaptureResult::timestamp`). What the strip sorts and labels by.
    pub created_at: u64,
    /// When it was filed here. What retention counts from: a card left on screen for two
    /// days has not been *in history* for two days.
    pub filed_at: u64,
    /// The image, inside the entry's directory.
    pub path: PathBuf,
    /// The capture's JSON twin, moved with it, so that a restore is a capture again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta_path: Option<PathBuf>,
    /// The copy the user saved, if any -- `spec/04` §7's `saved_path`. The card that
    /// comes back offers Trash rather than a second Save when this is set (D47).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_path: Option<PathBuf>,
    /// The `.octosnap` project this capture was saved as, if any (`HIS-03`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb_path: Option<PathBuf>,
    /// Physical pixels, as the file has them.
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    /// `HIS-03`'s "source app + window title", the badge on the thumbnail.
    #[serde(flatten)]
    pub source: SourceWindow,
    /// The monitor's connector, kept so a restore can go back to that screen.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display: String,
    /// A recording's length, for the strip's badge (D109). `None` for a still, and for an
    /// entry filed before the field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl Entry {
    /// The entry for a capture being filed into `dir`, with its files named as they were
    /// in the spool -- so that restoring is moving them back, nothing more.
    ///
    /// `saved_path` and `project_path` travel with the capture (`spec/04` §7), and the
    /// physical size is the rect at its scale, which is what the PNG contains. The
    /// project is the capture's own when the caller names none, and a capture with one
    /// is a [`Kind::Project`] (D167).
    #[must_use]
    pub fn filed(
        capture: &CaptureResult,
        dir: &Path,
        filed_at: u64,
        saved_path: Option<&Path>,
        project_path: Option<&Path>,
    ) -> Self {
        let inside = |file: &Path| {
            file.file_name().map_or_else(|| dir.join("capture.png"), |name| dir.join(name))
        };
        let project_path = project_path.or(capture.project.as_deref());
        Self {
            id: id_of(capture).unwrap_or_else(|| "capture".to_owned()),
            kind: if project_path.is_some() { Kind::Project } else { Kind::of(capture) },
            created_at: capture.timestamp,
            filed_at,
            path: inside(&capture.path),
            meta_path: Some(inside(&capture.meta_path)),
            saved_path: saved_path.map(Path::to_path_buf),
            project_path: project_path.map(Path::to_path_buf),
            thumb_path: Some(dir.join(THUMB_FILE)),
            width: physical(capture.rect.width, capture.scale),
            height: physical(capture.rect.height, capture.scale),
            scale: capture.scale,
            source: capture.source_window.clone(),
            display: capture.display.clone(),
            duration_ms: capture.duration_ms,
        }
    }

    /// The directory the entry's files live in.
    #[must_use]
    pub fn dir(&self) -> Option<&Path> {
        self.path.parent()
    }

    /// `spec/07` §4.2's "3 minutes ago".
    #[must_use]
    pub fn age_label(&self, now_us: u64) -> String {
        relative_time(self.created_at, now_us)
    }

    /// Whether the janitor removes this entry now.
    #[must_use]
    pub fn expired(&self, now_us: u64, retention: Retention) -> bool {
        now_us.saturating_sub(self.filed_at) >= retention.micros()
    }

    /// The badge's text: the app's name when the capture came from a window.
    #[must_use]
    pub fn badge(&self) -> Option<&str> {
        let name = self.source.app_name.trim();
        (!name.is_empty()).then_some(name)
    }
}

/// The capture's id: its spool file's stem.
#[must_use]
pub fn id_of(capture: &CaptureResult) -> Option<String> {
    capture.path.file_stem().map(|stem| stem.to_string_lossy().into_owned())
}

/// `<root>/<id>`: where an entry's files go.
#[must_use]
pub fn entry_dir(root: &Path, id: &str) -> PathBuf {
    root.join(id)
}

/// A logical length at a scale, rounded to the pixel the file has.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn physical(logical: i32, scale: f64) -> u32 {
    (f64::from(logical) * scale).round().max(0.0) as u32
}

/// `spec/08` §9's `history-retention-days`: "1 day / 3 days / 1 week / 2 weeks / 1 month".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Retention {
    Day,
    ThreeDays,
    Week,
    TwoWeeks,
    Month,
}

impl Retention {
    /// The menu's order.
    pub const ALL: [Self; 5] = [Self::Day, Self::ThreeDays, Self::Week, Self::TwoWeeks, Self::Month];

    #[must_use]
    pub const fn days(self) -> u32 {
        match self {
            Self::Day => 1,
            Self::ThreeDays => 3,
            Self::Week => 7,
            Self::TwoWeeks => 14,
            Self::Month => 30,
        }
    }

    /// The setting as stored, snapped **up** to a choice the menu has: a hand-edited `5`
    /// keeps captures for a week rather than silently for three days, because losing
    /// files earlier than the user asked is the worse of the two mistakes.
    #[must_use]
    pub fn from_days(days: i32) -> Self {
        let days = u32::try_from(days).unwrap_or(0);
        Self::ALL
            .into_iter()
            .find(|choice| choice.days() >= days)
            .unwrap_or(Self::Month)
    }

    #[must_use]
    pub const fn micros(self) -> u64 {
        self.days() as u64 * 86_400 * 1_000_000
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Day => "1 day",
            Self::ThreeDays => "3 days",
            Self::Week => "1 week",
            Self::TwoWeeks => "2 weeks",
            Self::Month => "1 month",
        }
    }
}

/// `spec/04` §7: a closing card's capture goes to history "unless saved and 'keep
/// history' is off" (`spec/08` §9's `history-keep-saved`).
#[must_use]
pub const fn files(saved: bool, keep_saved: bool) -> bool {
    !saved || keep_saved
}

/// The entries the janitor removes now.
#[must_use]
pub fn expired(entries: &[Entry], now_us: u64, retention: Retention) -> Vec<&Entry> {
    entries.iter().filter(|entry| entry.expired(now_us, retention)).collect()
}

/// `spec/07` §4.2: "newest first". Ties -- two captures in the same microsecond -- fall
/// back to the id, which is a ULID and so also time-ordered.
pub fn sort_newest_first(entries: &mut [Entry]) {
    entries.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.id.cmp(&a.id)));
}

/// `spec/07` §4.2's relative timestamp: "3 minutes ago", "2 hours ago", "18 hours ago",
/// "rather than an absolute date".
///
/// Whole units, rounded down, and a word where a number would be odd: "yesterday" for
/// anything between one and two days, "last week" and "last month" for the first of
/// each. A time in the future -- a clock that moved -- reads as "just now" rather than
/// as a negative number.
#[must_use]
pub fn relative_time(then_us: u64, now_us: u64) -> String {
    let seconds = now_us.saturating_sub(then_us) / 1_000_000;
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;
    const MONTH: u64 = 30 * DAY;
    let plural = |n: u64, unit: &str| {
        if n == 1 { format!("1 {unit} ago") } else { format!("{n} {unit}s ago") }
    };
    match seconds {
        s if s < MINUTE => "just now".to_owned(),
        s if s < HOUR => plural(s / MINUTE, "minute"),
        s if s < DAY => plural(s / HOUR, "hour"),
        s if s < 2 * DAY => "yesterday".to_owned(),
        s if s < WEEK => plural(s / DAY, "day"),
        s if s < 2 * WEEK => "last week".to_owned(),
        s if s < MONTH => plural(s / WEEK, "week"),
        s if s < 2 * MONTH => "last month".to_owned(),
        s => plural(s / MONTH, "month"),
    }
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::Rect;

    const US: u64 = 1_000_000;
    const DAY_US: u64 = 86_400 * US;

    fn capture() -> CaptureResult {
        CaptureResult {
            path: PathBuf::from("/home/u/.cache/octosnap/spool/01JTEST.png"),
            meta_path: PathBuf::from("/home/u/.cache/octosnap/spool/01JTEST.json"),
            mode: CaptureMode::Area,
            rect: Rect::new(10, 20, 700, 450),
            scale: 1.25,
            display: "eDP-1".to_owned(),
            cursor_rect: None,
            source_window: SourceWindow {
                app_id: "org.mozilla.firefox".to_owned(),
                app_name: "Firefox".to_owned(),
                title: "GitHub".to_owned(),
            },
            window_alpha: false,
            timestamp: 1_700_000_000 * US,
            confirmed_at: None,
            animation_ms: 220,
            requested_action: None,
            modifiers: 0,
            external: false,
            linebreaks: None,
            project: None,
            duration_ms: None,
        }
    }

    fn entry(id: &str, created_at: u64, filed_at: u64) -> Entry {
        let mut capture = capture();
        capture.path = PathBuf::from(format!("/spool/{id}.png"));
        capture.timestamp = created_at;
        Entry::filed(&capture, Path::new("/history").join(id).as_path(), filed_at, None, None)
    }

    /// The recorder's two outputs share one mode, so the file decides (M5, D68).
    #[test]
    fn a_recording_is_a_gif_or_a_video_by_its_file() {
        let recorded = |path: &str| CaptureResult {
            mode: CaptureMode::Record,
            path: PathBuf::from(path),
            ..capture()
        };
        assert_eq!(Kind::of(&recorded("/spool/01JREC.gif")), Kind::Gif);
        assert_eq!(Kind::of(&recorded("/spool/01JREC.GIF")), Kind::Gif);
        assert_eq!(Kind::of(&recorded("/spool/01JREC.mp4")), Kind::Video);
        assert_eq!(Kind::of(&recorded("/spool/01JREC")), Kind::Video);
        // A GIF opened with the app is external first, whatever its extension.
        let opened = CaptureResult { external: true, ..recorded("/spool/01JOPEN.gif") };
        assert_eq!(Kind::of(&opened), Kind::External);
    }

    #[test]
    fn every_screenshot_mode_is_a_screenshot_and_the_others_are_themselves() {
        let in_mode = |mode: CaptureMode| CaptureResult { mode, ..capture() };
        for mode in [
            CaptureMode::AllInOne,
            CaptureMode::Area,
            CaptureMode::Window,
            CaptureMode::Fullscreen,
            CaptureMode::PreviousArea,
            CaptureMode::SelfTimer,
        ] {
            assert_eq!(Kind::of(&in_mode(mode)), Kind::Screenshot, "{mode:?}");
        }
        assert_eq!(Kind::of(&in_mode(CaptureMode::Record)), Kind::Video);
        assert_eq!(Kind::of(&in_mode(CaptureMode::Scrolling)), Kind::Scrolling);
        assert_eq!(Kind::of(&in_mode(CaptureMode::Ocr)), Kind::Ocr);
        // A file opened with the app is a file, whatever mode it was given (HIS-04).
        let opened = CaptureResult { external: true, ..capture() };
        assert_eq!(Kind::of(&opened), Kind::External);
        assert!(Filter::Screenshots.matches(Kind::of(&opened)));
    }

    #[test]
    fn the_chips_partition_the_kinds_and_all_takes_everything() {
        let kinds = [
            Kind::Screenshot,
            Kind::Video,
            Kind::Gif,
            Kind::Scrolling,
            Kind::Ocr,
            Kind::External,
            Kind::Project,
        ];
        for kind in kinds {
            assert!(Filter::All.matches(kind));
            let owners = Filter::ALL
                .into_iter()
                .filter(|chip| *chip != Filter::All && chip.matches(kind))
                .count();
            assert_eq!(owners, 1, "{kind:?} should be under exactly one chip");
        }
        assert!(Filter::Screenshots.matches(Kind::External));
        assert!(!Filter::Screenshots.matches(Kind::Video));
        assert!(Filter::Projects.matches(Kind::Project));
    }

    /// D167: nothing was ever filed as a project, so the Projects chip showed nothing. A
    /// render of a project file is one now, whether the capture carries the project or
    /// the caller names it, and only that chip shows it.
    #[test]
    fn a_capture_with_a_project_files_under_projects() {
        let dir = Path::new("/home/u/.local/share/octosnap/history/01JTEST");
        assert_eq!(Entry::filed(&capture(), dir, 1, None, None).kind, Kind::Screenshot);
        let project = Path::new("/home/u/Documents/work.octosnap");
        let render = CaptureResult { project: Some(project.to_path_buf()), ..capture() };
        let entry = Entry::filed(&render, dir, 1, None, None);
        assert_eq!(entry.kind, Kind::Project);
        assert_eq!(entry.project_path.as_deref(), Some(project));
        let chips: Vec<_> =
            Filter::ALL.into_iter().filter(|chip| chip.matches(entry.kind)).collect();
        assert_eq!(chips, [Filter::All, Filter::Projects]);
        let named = Entry::filed(&capture(), dir, 1, None, Some(project));
        assert_eq!(named.kind, Kind::Project);
    }

    #[test]
    fn filing_keeps_the_spool_names_inside_the_new_directory() {
        let dir = Path::new("/home/u/.local/share/octosnap/history/01JTEST");
        let entry = Entry::filed(
            &capture(),
            dir,
            42,
            Some(Path::new("/home/u/Pictures/Screenshots/shot.png")),
            None,
        );
        assert_eq!(entry.id, "01JTEST");
        assert_eq!(entry.kind, Kind::Screenshot);
        assert_eq!(entry.path, dir.join("01JTEST.png"));
        assert_eq!(entry.meta_path.as_deref(), Some(dir.join("01JTEST.json").as_path()));
        assert_eq!(entry.thumb_path.as_deref(), Some(dir.join(THUMB_FILE).as_path()));
        assert_eq!(entry.dir(), Some(dir));
        assert_eq!(entry.filed_at, 42);
        assert_eq!(entry.created_at, 1_700_000_000 * US);
        assert_eq!(
            entry.saved_path.as_deref(),
            Some(Path::new("/home/u/Pictures/Screenshots/shot.png"))
        );
        assert_eq!(entry.project_path, None);
        assert_eq!(entry.display, "eDP-1");
    }

    #[test]
    fn the_size_is_physical_pixels() {
        let entry = Entry::filed(&capture(), Path::new("/h/x"), 0, None, None);
        // 700 x 450 logical at 1.25 is 875 x 562.5, and the file has whole pixels.
        assert_eq!((entry.width, entry.height), (875, 563));
        assert!((entry.scale - 1.25).abs() < f64::EPSILON);
    }

    #[test]
    fn the_badge_is_the_source_app_and_absent_without_one() {
        let entry = Entry::filed(&capture(), Path::new("/h/x"), 0, None, None);
        assert_eq!(entry.badge(), Some("Firefox"));
        let mut anonymous = capture();
        anonymous.source_window = SourceWindow::default();
        let entry = Entry::filed(&anonymous, Path::new("/h/x"), 0, None, None);
        assert_eq!(entry.badge(), None);
    }

    #[test]
    fn an_entry_round_trips_through_json_and_an_old_one_without_the_options_still_reads() {
        let entry = Entry::filed(
            &capture(),
            Path::new("/h/01JTEST"),
            7,
            None,
            Some(Path::new("/h/p.octosnap")),
        );
        let json = serde_json::to_string(&entry).expect("serialises");
        assert!(json.contains("\"app_name\":\"Firefox\""), "{json}");
        assert!(json.contains("\"window_title\":\"GitHub\""), "{json}");
        assert!(!json.contains("saved_path"), "an absent option is not written: {json}");
        let back: Entry = serde_json::from_str(&json).expect("parses");
        assert_eq!(back, entry);

        let minimal = r#"{"id":"01J","kind":"screenshot","created_at":1,"filed_at":2,
            "path":"/h/01J/01J.png","width":10,"height":20,"scale":1.0}"#;
        let old: Entry = serde_json::from_str(minimal).expect("an old entry parses");
        assert_eq!(old.meta_path, None);
        assert!(old.source.is_empty());
        assert_eq!(old.display, "");
    }

    #[test]
    fn retention_snaps_up_to_a_choice_the_menu_has() {
        assert_eq!(Retention::from_days(1), Retention::Day);
        assert_eq!(Retention::from_days(2), Retention::ThreeDays);
        assert_eq!(Retention::from_days(3), Retention::ThreeDays);
        assert_eq!(Retention::from_days(5), Retention::Week);
        assert_eq!(Retention::from_days(7), Retention::Week);
        assert_eq!(Retention::from_days(10), Retention::TwoWeeks);
        assert_eq!(Retention::from_days(30), Retention::Month);
        assert_eq!(Retention::from_days(365), Retention::Month);
        assert_eq!(Retention::from_days(0), Retention::Day);
        assert_eq!(Retention::from_days(-4), Retention::Day);
        assert_eq!(Retention::Week.micros(), 7 * DAY_US);
    }

    #[test]
    fn expiry_counts_from_the_filing_and_is_inclusive_at_the_boundary() {
        let filed = 10 * DAY_US;
        let entry = entry("01J", 1, filed);
        assert!(!entry.expired(filed + 3 * DAY_US - 1, Retention::ThreeDays));
        assert!(entry.expired(filed + 3 * DAY_US, Retention::ThreeDays));
        // Older than the capture's own age would suggest, because the card sat open.
        assert!(!entry.expired(filed + DAY_US, Retention::ThreeDays));
    }

    #[test]
    fn the_janitor_takes_only_what_has_expired() {
        let entries = vec![entry("01A", 1, 0), entry("01B", 2, 5 * DAY_US), entry("01C", 3, 9 * DAY_US)];
        let now = 10 * DAY_US;
        let gone: Vec<&str> = expired(&entries, now, Retention::Week).iter().map(|e| e.id.as_str()).collect();
        assert_eq!(gone, vec!["01A"]);
        let gone: Vec<&str> = expired(&entries, now, Retention::Day).iter().map(|e| e.id.as_str()).collect();
        assert_eq!(gone, vec!["01A", "01B", "01C"]);
        assert!(expired(&entries, now, Retention::Month).is_empty());
    }

    #[test]
    fn saved_captures_are_filed_only_when_the_setting_keeps_them() {
        assert!(files(false, false));
        assert!(files(false, true));
        assert!(!files(true, false));
        assert!(files(true, true));
    }

    #[test]
    fn newest_first_by_creation_then_by_id() {
        let mut entries = vec![entry("01A", 5, 0), entry("01C", 9, 0), entry("01B", 9, 0), entry("01D", 1, 0)];
        sort_newest_first(&mut entries);
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["01C", "01B", "01A", "01D"]);
    }

    #[test]
    fn relative_times_read_like_the_reference_panel() {
        let now = 1_000 * DAY_US;
        let at = |seconds_ago: u64| relative_time(now - seconds_ago * US, now);
        assert_eq!(at(0), "just now");
        assert_eq!(at(59), "just now");
        assert_eq!(at(60), "1 minute ago");
        assert_eq!(at(3 * 60), "3 minutes ago");
        assert_eq!(at(2 * 3600), "2 hours ago");
        assert_eq!(at(18 * 3600 + 59 * 60), "18 hours ago");
        assert_eq!(at(24 * 3600), "yesterday");
        assert_eq!(at(47 * 3600), "yesterday");
        assert_eq!(at(3 * 86_400), "3 days ago");
        assert_eq!(at(7 * 86_400), "last week");
        assert_eq!(at(20 * 86_400), "2 weeks ago");
        assert_eq!(at(31 * 86_400), "last month");
        assert_eq!(at(95 * 86_400), "3 months ago");
        assert_eq!(relative_time(now + US, now), "just now", "a clock that moved back");
        assert_eq!(entry("01A", now - 120 * US, 0).age_label(now), "2 minutes ago");
    }

    /// D109: every tile is one shape, and what it shows of a picture of another shape is
    /// the middle of a wide one and the top of a tall one.
    #[test]
    fn a_tile_shows_the_middle_of_a_wide_picture_and_the_top_of_a_tall_one() {
        assert_eq!(tile_crop(1920, 1200), (0, 0, 1920, 1200), "a 16:10 screen is all shown");
        assert_eq!(tile_crop(1920, 600), (480, 0, 960, 600), "a banner, from its middle");
        assert_eq!(tile_crop(1920, 5253), (0, 0, 1920, 1200), "a scrolled page, from its top");
        assert_eq!(tile_crop(300, 300), (0, 0, 300, 188));
        assert_eq!(tile_crop(1, 5000), (0, 0, 1, 1), "a sliver never collapses to nothing");
        assert_eq!(tile_crop(5000, 1), (2499, 0, 2, 1));
        assert_eq!(tile_crop(0, 0), (0, 0, 1, 1), "nothing is still a pixel");
    }

    #[test]
    fn a_tile_thumbnail_comes_down_to_twice_the_tile_and_never_goes_up() {
        assert_eq!(tile_thumb_size(3840, 2400), THUMB_MAX);
        assert_eq!(tile_thumb_size(1920, 5253), THUMB_MAX);
        assert_eq!(tile_thumb_size(2560, 1080), (416, 260));
        assert_eq!(tile_thumb_size(320, 200), (320, 200), "small pictures stay their size");
        for (w, h) in [(1366, 768), (1710, 1107), (690, 750), (5120, 2880), (833, 97)] {
            let (tw, th) = tile_thumb_size(w, h);
            let ratio = f64::from(tw) / f64::from(th);
            assert!((ratio - 1.6).abs() < 0.03, "{w}x{h} -> {tw}x{th}");
            assert!(tw <= THUMB_MAX.0 && th <= THUMB_MAX.1, "{w}x{h} -> {tw}x{th}");
        }
    }

    #[test]
    fn every_kind_and_every_chip_but_all_has_a_symbolic_icon() {
        for kind in Kind::ALL {
            assert!(kind.icon().ends_with("-symbolic"), "{kind:?}");
        }
        let icons: std::collections::HashSet<_> = Kind::ALL.iter().map(|k| k.icon()).collect();
        assert_eq!(icons.len(), Kind::ALL.len(), "two kinds would look alike");
        assert_eq!(Filter::All.icon(), None);
        for filter in &Filter::ALL[1..] {
            assert!(filter.icon().is_some(), "{filter:?}");
        }
    }
}
