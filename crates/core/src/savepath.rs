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

/// `spec/08` `shot-jpg-quality`.
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
}
