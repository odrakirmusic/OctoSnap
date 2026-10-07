// SPDX-License-Identifier: GPL-3.0-or-later

//! Where a capture is written: `ACT-06`.
//!
//! `spec/08`: `screenshot-folder` defaults to empty, meaning XDG Pictures/Screenshots;
//! `recording-folder` to Videos/Screencasts. `spec/08` note 3 is emphatic that there is
//! **one** export location feeding the overlay, the after-capture actions and every Save,
//! rather than one per surface -- so this module resolves a single directory and nothing
//! else is allowed its own.
//!
//! Collision handling takes an `exists` predicate rather than touching the filesystem,
//! which is what makes it testable. It is inherently racy against another process, so the
//! caller still opens with `create_new` and retries; this only picks a sensible candidate.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `spec/08` `shot-format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    #[default]
    Png,
    Jpg,
    Webp,
}

/// `spec/08` `shot-jpg-quality`: the JPEG encoder's 1 to 100 (D164).
pub const DEFAULT_JPEG_QUALITY: u8 = 90;

/// `spec/08`: the subdirectory under XDG Pictures when `screenshot-folder` is unset.
pub const SCREENSHOT_SUBDIR: &str = "Screenshots";
/// `spec/08`: the subdirectory under XDG Videos when `recording-folder` is unset.
pub const RECORDING_SUBDIR: &str = "Screencasts";

/// How many `name (n)` candidates to try before giving up on a readable name.
const MAX_COLLISION_ATTEMPTS: u32 = 1000;

impl ImageFormat {
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "png" => Self::Png,
            // "jpeg" is accepted because it is what half the world writes, even though
            // spec/08 spells the setting "jpg".
            "jpg" | "jpeg" => Self::Jpg,
            "webp" => Self::Webp,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpg => "jpg",
            Self::Webp => "webp",
        }
    }

    #[must_use]
    pub const fn extension(self) -> &'static str {
        self.as_wire()
    }

    /// True when the format cannot carry an alpha channel, so a window capture with a
    /// transparent surround (`CAP-03`) would lose it.
    #[must_use]
    pub const fn is_opaque(self) -> bool {
        matches!(self, Self::Jpg)
    }

    /// The format a file name asks for, by its extension and in any case: `shot.JPEG` is
    /// a JPEG. `None` for a name with no extension or one that is none of the three.
    #[must_use]
    pub fn of_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        Self::from_wire(&extension)
    }

    /// The format a file's first bytes say it is: the signature each format opens with.
    /// Twelve bytes are enough for all three; fewer can still be a PNG's or a JPEG's.
    #[must_use]
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            Some(Self::Jpg)
        } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && bytes[8..12] == *b"WEBP" {
            Some(Self::Webp)
        } else {
            None
        }
    }

    /// The longest side, in pixels, a file of this format can have. JPEG counts in 16 bits
    /// and lossless WebP in 14 (plus one), so a long scrolling capture fits neither; PNG's
    /// limit is 2^31 - 1, which no screen comes near.
    #[must_use]
    pub const fn max_side(self) -> u32 {
        match self {
            Self::Png => i32::MAX.unsigned_abs(),
            Self::Jpg => 65_535,
            Self::Webp => 16_384,
        }
    }

    /// This format if it can hold a `width` x `height` picture, and PNG if it cannot
    /// (D164): a capture is never lost to its format, and a file named for one format is
    /// never another.
    #[must_use]
    pub const fn for_size(self, width: u32, height: u32) -> Self {
        let max = self.max_side();
        if width <= max && height <= max { self } else { Self::Png }
    }
}

/// Where a Save As writes a screenshot, and as what (D164).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveAsTarget {
    pub path: PathBuf,
    pub format: ImageFormat,
    /// True when `path` is the name the user chose, which may be written over: the
    /// chooser has already asked about that. False when it is one made from it, which is
    /// a free name rather than someone else's file.
    pub chosen: bool,
}

/// What a Save As to `chosen` writes, given the configured format and whether a format
/// can hold the picture (`ImageFormat::for_size`).
///
/// The **name** decides the format, as it does for the editor's project (D164): a user
/// who types `shot.webp` under a chooser pre-filled with `shot.png` has said what they
/// want. A name that names no format -- none at all, or `v1.2 notes`, whose "extension" is
/// `2 notes` -- keeps every character and gains the configured format's, rather than
/// holding bytes its name does not describe. A format too small for the picture gives way
/// to PNG under the same stem.
#[must_use]
pub fn save_as_target(
    chosen: &Path,
    configured: ImageFormat,
    fits: impl Fn(ImageFormat) -> bool,
    exists: impl Fn(&Path) -> bool,
) -> SaveAsTarget {
    let dir = chosen.parent().unwrap_or_else(|| Path::new(""));
    let name = chosen.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match ImageFormat::of_path(chosen) {
        Some(format) if fits(format) => {
            SaveAsTarget { path: chosen.to_path_buf(), format, chosen: true }
        }
        Some(_) => {
            let stem = chosen.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let format = ImageFormat::Png;
            let path = unique_path(dir, &stem, format.extension(), exists);
            SaveAsTarget { path, format, chosen: false }
        }
        None => {
            let format = if fits(configured) { configured } else { ImageFormat::Png };
            let path = unique_path(dir, &name, format.extension(), exists);
            SaveAsTarget { path, format, chosen: false }
        }
    }
}

/// Resolves the screenshot directory.
///
/// `configured` is `screenshot-folder`; an empty or absent value means the XDG default,
/// which is how `spec/08`'s `<default>''</default>` is meant to be read.
#[must_use]
pub fn screenshot_dir(configured: Option<&Path>, xdg_pictures: &Path) -> PathBuf {
    resolve_dir(configured, xdg_pictures, SCREENSHOT_SUBDIR)
}

/// Resolves the recording directory (`recording-folder`).
#[must_use]
pub fn recording_dir(configured: Option<&Path>, xdg_videos: &Path) -> PathBuf {
    resolve_dir(configured, xdg_videos, RECORDING_SUBDIR)
}

fn resolve_dir(configured: Option<&Path>, xdg_base: &Path, subdir: &str) -> PathBuf {
    match configured {
        Some(path) if !path.as_os_str().is_empty() => path.to_path_buf(),
        _ => xdg_base.join(subdir),
    }
}

/// Picks a path that does not already exist.
///
/// `stem` is expected to have come from `filename::render`, so it is already sanitised;
/// an empty one is still guarded against, because a bare `.png` is a hidden file.
///
/// Collisions get a ` (2)`, ` (3)` suffix. That is a `[P]` choice: `spec/08` does not say.
/// A numeric suffix beats Nautilus's "(copy)" here because captures collide by the
/// second, and it beats appending to the extension because the name stays readable.
#[must_use]
pub fn unique_path<F>(dir: &Path, stem: &str, extension: &str, exists: F) -> PathBuf
where
    F: Fn(&Path) -> bool,
{
    let stem = if stem.trim().is_empty() { "Capture" } else { stem };

    let first = dir.join(format!("{stem}.{extension}"));
    if !exists(&first) {
        return first;
    }

    for n in 2..=MAX_COLLISION_ATTEMPTS {
        let candidate = dir.join(format!("{stem} ({n}).{extension}"));
        if !exists(&candidate) {
            return candidate;
        }
    }

    // A thousand collisions means something is wrong with the template, not with this
    // capture. Fall back to something that cannot collide rather than failing the save.
    // ulid 3.0 dropped Ulid::new(); from_datetime is the current constructor.
    let unique = ulid::Ulid::from_datetime(std::time::SystemTime::now()).to_string();
    dir.join(format!("{stem} {unique}.{extension}"))
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn taken(paths: &[&str]) -> impl Fn(&Path) -> bool + use<> {
        let set: HashSet<PathBuf> = paths.iter().map(PathBuf::from).collect();
        move |p: &Path| set.contains(p)
    }

    // --- directories ---------------------------------------------------------

    #[test]
    fn an_unset_folder_falls_back_to_the_xdg_default() {
        let pictures = Path::new("/home/u/Pictures");
        assert_eq!(
            screenshot_dir(None, pictures),
            PathBuf::from("/home/u/Pictures/Screenshots")
        );
        assert_eq!(
            recording_dir(None, Path::new("/home/u/Videos")),
            PathBuf::from("/home/u/Videos/Screencasts")
        );
    }

    /// spec/08 spells the default as `''`, so an empty string must behave as unset
    /// rather than resolving to the current directory.
    #[test]
    fn an_empty_configured_folder_is_treated_as_unset() {
        assert_eq!(
            screenshot_dir(Some(Path::new("")), Path::new("/home/u/Pictures")),
            PathBuf::from("/home/u/Pictures/Screenshots")
        );
    }

    #[test]
    fn a_configured_folder_wins() {
        assert_eq!(
            screenshot_dir(Some(Path::new("/data/shots")), Path::new("/home/u/Pictures")),
            PathBuf::from("/data/shots")
        );
    }

    // --- collisions ----------------------------------------------------------

    #[test]
    fn a_free_name_is_used_as_is() {
        let path = unique_path(Path::new("/shots"), "Screenshot", "png", taken(&[]));
        assert_eq!(path, PathBuf::from("/shots/Screenshot.png"));
    }

    #[test]
    fn a_collision_gets_a_numbered_suffix() {
        let path = unique_path(
            Path::new("/shots"),
            "Screenshot",
            "png",
            taken(&["/shots/Screenshot.png"]),
        );
        assert_eq!(path, PathBuf::from("/shots/Screenshot (2).png"));
    }

    #[test]
    fn the_suffix_counts_up_past_existing_numbered_files() {
        let path = unique_path(
            Path::new("/shots"),
            "Screenshot",
            "png",
            taken(&[
                "/shots/Screenshot.png",
                "/shots/Screenshot (2).png",
                "/shots/Screenshot (3).png",
            ]),
        );
        assert_eq!(path, PathBuf::from("/shots/Screenshot (4).png"));
    }

    /// Captures collide by the second, so the common case is a template without a
    /// seconds token producing many same-named files in a row.
    #[test]
    fn many_collisions_still_resolve() {
        let existing: Vec<String> = std::iter::once("/shots/Shot.png".to_owned())
            .chain((2..=50).map(|n| format!("/shots/Shot ({n}).png")))
            .collect();
        let refs: Vec<&str> = existing.iter().map(String::as_str).collect();
        let path = unique_path(Path::new("/shots"), "Shot", "png", taken(&refs));
        assert_eq!(path, PathBuf::from("/shots/Shot (51).png"));
    }

    /// A bare ".png" would be a hidden file, so an empty stem must not produce one.
    #[test]
    fn an_empty_stem_is_replaced() {
        let path = unique_path(Path::new("/shots"), "   ", "png", taken(&[]));
        assert_eq!(path, PathBuf::from("/shots/Capture.png"));
    }

    /// When every candidate is taken, saving must still succeed with something unique
    /// rather than fail or loop.
    #[test]
    fn exhausting_the_candidates_falls_back_to_a_unique_name() {
        let path = unique_path(Path::new("/shots"), "Shot", "png", |_| true);
        let name = path.file_name().expect("name").to_string_lossy().to_string();
        assert!(name.starts_with("Shot "), "{name}");
        assert!(name.ends_with(".png"), "{name}");
        // A ULID is 26 characters: "Shot " + 26 + ".png".
        assert_eq!(name.len(), "Shot ".len() + 26 + ".png".len());
    }

    // --- formats -------------------------------------------------------------

    #[test]
    fn formats_round_trip_and_jpeg_is_accepted_as_a_spelling() {
        for f in [ImageFormat::Png, ImageFormat::Jpg, ImageFormat::Webp] {
            assert_eq!(ImageFormat::from_wire(f.as_wire()), Some(f));
        }
        assert_eq!(ImageFormat::from_wire("jpeg"), Some(ImageFormat::Jpg));
        assert_eq!(ImageFormat::from_wire("tiff"), None);
    }

    /// CAP-03 stores window captures with real alpha, so the format has to be able to
    /// say when that would be thrown away.
    #[test]
    fn only_jpeg_is_opaque() {
        assert!(ImageFormat::Jpg.is_opaque());
        assert!(!ImageFormat::Png.is_opaque());
        assert!(!ImageFormat::Webp.is_opaque());
    }

    #[test]
    fn the_default_format_is_png() {
        assert_eq!(ImageFormat::default(), ImageFormat::Png);
        assert_eq!(ImageFormat::default().extension(), "png");
    }

    #[test]
    fn a_name_asks_for_a_format_in_any_case() {
        let of = |name: &str| ImageFormat::of_path(Path::new(name));
        assert_eq!(of("/s/Shot.png"), Some(ImageFormat::Png));
        assert_eq!(of("/s/Shot.JPG"), Some(ImageFormat::Jpg));
        assert_eq!(of("/s/Shot.jpeg"), Some(ImageFormat::Jpg));
        assert_eq!(of("/s/Shot.WebP"), Some(ImageFormat::Webp));
        assert_eq!(of("/s/Shot.tiff"), None);
        assert_eq!(of("/s/Shot"), None);
        assert_eq!(of("/s/v1.2 notes"), None);
    }

    #[test]
    fn a_file_is_sniffed_by_its_signature() {
        assert_eq!(ImageFormat::sniff(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), Some(ImageFormat::Png));
        assert_eq!(ImageFormat::sniff(&[0xff, 0xd8, 0xff, 0xe0]), Some(ImageFormat::Jpg));
        assert_eq!(ImageFormat::sniff(b"RIFF\x24\0\0\0WEBPVP8L"), Some(ImageFormat::Webp));
        // A RIFF that is not a WebP -- a WAV -- and a GIF are neither.
        assert_eq!(ImageFormat::sniff(b"RIFF\x24\0\0\0WAVEfmt "), None);
        assert_eq!(ImageFormat::sniff(b"GIF89a"), None);
        assert_eq!(ImageFormat::sniff(b"RIFF"), None);
        assert_eq!(ImageFormat::sniff(b""), None);
    }

    /// A scrolling capture is the one that finds these limits: a long page at 2x is past
    /// WebP's 16384 rows in a few screens.
    #[test]
    fn a_format_too_small_for_the_picture_gives_way_to_png() {
        assert_eq!(ImageFormat::Webp.for_size(1920, 16_384), ImageFormat::Webp);
        assert_eq!(ImageFormat::Webp.for_size(1920, 16_385), ImageFormat::Png);
        assert_eq!(ImageFormat::Jpg.for_size(65_535, 1080), ImageFormat::Jpg);
        assert_eq!(ImageFormat::Jpg.for_size(1920, 65_536), ImageFormat::Png);
        assert_eq!(ImageFormat::Png.for_size(1920, 400_000), ImageFormat::Png);
    }

    // --- save as -------------------------------------------------------------

    fn target(chosen: &str, configured: ImageFormat, fits: &[ImageFormat], exists: &[&str]) -> SaveAsTarget {
        save_as_target(Path::new(chosen), configured, |f| fits.contains(&f), taken(exists))
    }

    const ALL: [ImageFormat; 3] = [ImageFormat::Png, ImageFormat::Jpg, ImageFormat::Webp];

    #[test]
    fn the_chosen_name_decides_the_format() {
        let t = target("/s/Shot.webp", ImageFormat::Jpg, &ALL, &["/s/Shot.webp"]);
        assert_eq!(t, SaveAsTarget { path: "/s/Shot.webp".into(), format: ImageFormat::Webp, chosen: true });
        let t = target("/s/Shot.JPEG", ImageFormat::Png, &ALL, &[]);
        assert_eq!(t, SaveAsTarget { path: "/s/Shot.JPEG".into(), format: ImageFormat::Jpg, chosen: true });
    }

    #[test]
    fn a_name_without_a_format_keeps_every_character_and_gains_one() {
        let t = target("/s/v1.2 notes", ImageFormat::Jpg, &ALL, &[]);
        assert_eq!(t, SaveAsTarget { path: "/s/v1.2 notes.jpg".into(), format: ImageFormat::Jpg, chosen: false });
        // Not the user's name, so not the user's to overwrite.
        let t = target("/s/Shot.tiff", ImageFormat::Png, &ALL, &["/s/Shot.tiff.png"]);
        assert_eq!(t, SaveAsTarget { path: "/s/Shot.tiff (2).png".into(), format: ImageFormat::Png, chosen: false });
    }

    #[test]
    fn a_format_too_small_saves_as_png_beside_the_chosen_name() {
        let png = [ImageFormat::Png];
        let t = target("/s/Long.webp", ImageFormat::Webp, &png, &["/s/Long.png"]);
        assert_eq!(t, SaveAsTarget { path: "/s/Long (2).png".into(), format: ImageFormat::Png, chosen: false });
        let t = target("/s/Long", ImageFormat::Webp, &png, &[]);
        assert_eq!(t, SaveAsTarget { path: "/s/Long.png".into(), format: ImageFormat::Png, chosen: false });
    }
}
