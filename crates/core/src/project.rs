// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §8's project file, as far as anything outside the editor needs to know it.
//!
//! The *format* lives in `octosnap_scene::project`, which owns the zip, the manifest and
//! the objects. These three constants are here because they are shared vocabulary rather
//! than format: the CLI has to recognise a `.octosnap` argument handed over by a file
//! manager, the `.desktop` file claims the MIME type, and `shared-mime-info` declares it.
//! Putting them in the scene crate would mean the CLI depended on a zip encoder to know
//! what a file is called.

/// `spec/05` §8: "extension `.octosnap`". Without the dot.
pub const EXTENSION: &str = "octosnap";

/// `spec/05` §8: "MIME `application/x-octosnap-project`".
pub const MIME_TYPE: &str = "application/x-octosnap-project";

/// `spec/05` §8: "thumbnail.png 256 px preview for file managers".
pub const THUMBNAIL_SIZE: u32 = 256;

/// The extension projects were saved with before the rename (D141). The format is the
/// same, so such a file still opens as a project; nothing writes this extension any more.
pub const LEGACY_EXTENSION: &str = "pengushot";

/// Whether a path names a project, by its extension: [`EXTENSION`] or, for a file saved
/// before the rename, [`LEGACY_EXTENSION`]. Either case, the way a file manager matches.
#[must_use]
pub fn is_project(path: &std::path::Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION) || e.eq_ignore_ascii_case(LEGACY_EXTENSION))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_project_is_known_by_either_extension() {
        assert!(is_project(Path::new("/h/notes.octosnap")));
        assert!(is_project(Path::new("/h/NOTES.OCTOSNAP")));
        assert!(is_project(Path::new("/h/saved-before-the-rename.pengushot"))); // legacy
        assert!(!is_project(Path::new("/h/notes.png")));
        assert!(!is_project(Path::new("/h/octosnap")));
    }
}
