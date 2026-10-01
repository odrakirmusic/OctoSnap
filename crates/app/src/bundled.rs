// SPDX-License-Identifier: GPL-3.0-or-later

//! The extension this app carries, and its copy where GNOME Shell finds it (D160).
//!
//! The Flatpak is built with the extension inside it (`-Dbundle-extension=true`): the
//! release zip's files, from the same sources, with the schema compiled beside them. The
//! welcome window's Install copies them into the user's own extensions folder, where
//! `gnome-extensions install` puts an extension too, and which is all of it the sandbox is
//! given. GNOME Shell finds an extension at login only, so a logout follows, and the first
//! start after it turns the extension on (`setup::finish_install`).

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use gtk::glib;

use crate::settings;

/// Where the carried copy is under a data directory: beside the app's other data. Under
/// `gnome-shell/extensions` it would read as installed, since the sandbox's own data
/// directories are searched for an installed extension too.
const CARRIED: &str = "octosnap/extension";

/// Where GNOME Shell looks for a user's extensions, under their data directory.
const EXTENSIONS: &str = "gnome-shell/extensions";

/// The extension the app carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carried {
    pub dir: PathBuf,
    pub version: String,
    /// `metadata.json`'s `shell-version`.
    pub shells: Vec<String>,
}

/// The carried extension, when this build has one, as the Flatpak does.
#[must_use]
pub fn carried() -> Option<&'static Carried> {
    static FOUND: OnceLock<Option<Carried>> = OnceLock::new();
    FOUND
        .get_or_init(|| glib::system_data_dirs().into_iter().find_map(|root| read(&root.join(CARRIED))))
        .as_ref()
}

fn read(dir: &Path) -> Option<Carried> {
    let metadata: serde_json::Value = serde_json::from_str(&fs::read_to_string(dir.join("metadata.json")).ok()?).ok()?;
    if metadata.get("uuid")?.as_str()? != settings::EXTENSION_UUID {
        return None;
    }
    Some(Carried {
        dir: dir.to_owned(),
        version: metadata.get("version-name")?.as_str()?.to_owned(),
        shells: metadata
            .get("shell-version")?
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
    })
}

/// The user's data directory as GNOME Shell sees it. A sandbox's `XDG_DATA_HOME` is its
/// own, so there it is the host's, which Flatpak passes on as `HOST_XDG_DATA_HOME`.
#[must_use]
pub fn data_home() -> PathBuf {
    if settings::sandboxed() {
        if let Some(dir) = std::env::var_os("HOST_XDG_DATA_HOME").filter(|dir| !dir.is_empty()) {
            return dir.into();
        }
        return glib::home_dir().join(".local/share");
    }
    glib::user_data_dir()
}

/// The extension's own folder under `data_home`.
#[must_use]
pub fn folder(data_home: &Path) -> PathBuf {
    data_home.join(EXTENSIONS).join(settings::EXTENSION_UUID)
}

/// The `version-name` of the extension in `dir`: `Some("")` for one whose metadata names
/// none, and `None` where there is no extension at all.
#[must_use]
pub fn version_at(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join("metadata.json")).ok()?;
    let metadata: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    Some(metadata.get("version-name").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned())
}

/// Whether `carried` is a later release than `installed`, both `version-name`s ("0.1.2").
/// Only numbers are compared: a name that is not numbers and dots is no release this can
/// place, and an extension that has one is left as it is.
#[must_use]
pub fn later(carried: &str, installed: &str) -> bool {
    let parse = |name: &str| name.split('.').map(str::parse::<u64>).collect::<Result<Vec<_>, _>>().ok();
    matches!((parse(carried), parse(installed)), (Some(carried), Some(installed)) if carried > installed)
}

/// Whether GNOME Shell `running`, its `ShellVersion` ("50.1"), loads an extension made for
/// `shells`, by the shell's own rule (`versionCheck`, in `extensionUtils.js`): an entry
/// that is a major version takes that major's numbered releases, and any other entry
/// must match the major and the minor.
#[must_use]
pub fn supports(shells: &[String], running: &str) -> bool {
    let mut parts = running.split('.');
    let (major, minor) = (parts.next(), parts.next());
    shells.iter().any(|entry| {
        let mut wanted = entry.split('.');
        wanted.next() == major
            && match wanted.next() {
                None => minor.is_some_and(|m| m.parse::<f64>().is_ok_and(f64::is_finite)),
                wanted_minor => wanted_minor == minor,
            }
    })
}

/// Why the copy was not made.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("{} is a link, and OctoSnap does not write through it", .0.display())]
    Link(PathBuf),
    #[error("could not write {}: {source}", .path.display())]
    Io { path: PathBuf, source: io::Error },
}

fn at(path: &Path) -> impl FnOnce(io::Error) -> InstallError + '_ {
    move |source| InstallError::Io { path: path.to_owned(), source }
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// The files under `dir`, as paths relative to it, without following a link: one counts
/// as a file, to be replaced or taken out, and is never written through.
fn files(dir: &Path) -> io::Result<BTreeSet<PathBuf>> {
    let mut found = BTreeSet::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(under) = pending.pop() {
        for entry in fs::read_dir(dir.join(&under))? {
            let entry = entry?;
            let rel = under.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                pending.push(rel);
            } else {
                found.insert(rel);
            }
        }
    }
    Ok(found)
}

/// Copies the carried extension in `from` into the extension's own folder under
/// `data_home`, and takes out whatever the carried copy does not have, so the folder holds
/// its files and nothing else. Each file is written beside its place and renamed into it,
/// so none is ever half written. Nothing outside that folder is written or taken out.
/// Blocking: run it off the main loop. Returns how many files were copied.
///
/// # Errors
/// A link anywhere from `data_home` down to a file's folder, such as a developer's
/// extension linked into a checkout, or a test's store linked to the real one; or the copy
/// failing.
pub fn install(from: &Path, data_home: &Path) -> Result<usize, InstallError> {
    let to = folder(data_home);
    for dir in [data_home.join("gnome-shell"), data_home.join(EXTENSIONS), to.clone()] {
        if is_link(&dir) {
            return Err(InstallError::Link(dir));
        }
    }
    let wanted = files(from).map_err(at(from))?;
    fs::create_dir_all(&to).map_err(at(&to))?;
    for rel in &wanted {
        let target = to.join(rel);
        let mut dir = to.clone();
        for part in rel.parent().into_iter().flat_map(Path::components) {
            dir.push(part);
            if is_link(&dir) {
                return Err(InstallError::Link(dir));
            }
            if !dir.is_dir() {
                fs::create_dir(&dir).map_err(at(&dir))?;
            }
        }
        let name = rel.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let fresh = target.with_file_name(format!(".{name}.octosnap-new"));
        fs::copy(from.join(rel), &fresh).map_err(at(&fresh))?;
        fs::rename(&fresh, &target).map_err(at(&target))?;
    }
    for rel in files(&to).map_err(at(&to))?.difference(&wanted) {
        let gone = to.join(rel);
        fs::remove_file(&gone).map_err(at(&gone))?;
    }
    // Folders the carried copy no longer has, deepest first; one that still holds
    // something stays, and so does the extension's own.
    let mut dirs = Vec::new();
    let mut pending = vec![to.clone()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).map_err(at(&dir))?.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                pending.push(entry.path());
                dirs.push(entry.path());
            }
        }
    }
    for dir in dirs.iter().rev() {
        let _ = fs::remove_dir(dir);
    }
    Ok(wanted.len())
}

#[cfg(test)]
// A failing step should panic: the workspace's ban keeps panics out of D-Bus handlers, not
// out of tests.
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// A carried copy as the Flatpak has one: modules in a subfolder, and the schema.
    fn carried_copy(root: &Path, version: &str) -> PathBuf {
        let dir = root.join("app/share").join(CARRIED);
        write(
            &dir.join("metadata.json"),
            &format!(
                r#"{{"uuid": "{}", "version-name": "{version}", "shell-version": ["48", "49", "50"]}}"#,
                settings::EXTENSION_UUID
            ),
        );
        write(&dir.join("extension.js"), &format!("// {version}\n"));
        write(&dir.join("pets/art/painter.js"), "// paint\n");
        write(&dir.join("schemas/gschemas.compiled"), "GVariant");
        dir
    }

    fn contents(dir: &Path) -> Vec<(PathBuf, String)> {
        files(dir).unwrap().into_iter().map(|rel| (rel.clone(), fs::read_to_string(dir.join(&rel)).unwrap())).collect()
    }

    #[test]
    fn a_first_install_copies_every_file() {
        let root = tempfile::tempdir().unwrap();
        let from = carried_copy(root.path(), "0.1.2");
        let home = root.path().join("home/.local/share");
        assert_eq!(install(&from, &home).unwrap(), 4);
        assert_eq!(contents(&folder(&home)), contents(&from));
        assert_eq!(version_at(&folder(&home)).as_deref(), Some("0.1.2"));
    }

    #[test]
    fn an_update_replaces_and_takes_out_what_the_copy_has_not() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home/.local/share");
        let to = folder(&home);
        write(&to.join("extension.js"), "// 0.1.1\n");
        write(&to.join("old/module.js"), "// gone in 0.1.2\n");
        write(&to.join(".extension.js.octosnap-new"), "// a copy that never finished\n");
        let from = carried_copy(root.path(), "0.1.2");
        install(&from, &home).unwrap();
        assert_eq!(contents(&to), contents(&from));
        assert!(!to.join("old").exists(), "an emptied folder goes too");
    }

    #[test]
    fn nothing_beside_the_extension_is_touched() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home/.local/share");
        let other = home.join(EXTENSIONS).join("dash-to-dock@micxgx.gmail.com/extension.js");
        write(&other, "// someone else's\n");
        install(&carried_copy(root.path(), "0.1.2"), &home).unwrap();
        assert_eq!(fs::read_to_string(&other).unwrap(), "// someone else's\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_folder_is_refused_and_what_it_points_at_is_untouched() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real/extension");
        write(&real.join("extension.js"), "// the developer's own\n");
        let home = root.path().join("nested/data");
        fs::create_dir_all(home.join(EXTENSIONS)).unwrap();
        std::os::unix::fs::symlink(&real, folder(&home)).unwrap();
        let refused = install(&carried_copy(root.path(), "0.1.2"), &home).unwrap_err();
        assert!(matches!(refused, InstallError::Link(ref at) if *at == folder(&home)), "{refused}");
        assert_eq!(contents(&real), vec![(PathBuf::from("extension.js"), "// the developer's own\n".to_owned())]);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_above_the_folder_or_inside_it_is_refused_too() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = root.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        let from = carried_copy(root.path(), "0.1.2");

        let home = root.path().join("a/share");
        fs::create_dir_all(home.join("gnome-shell")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, home.join(EXTENSIONS)).unwrap();
        assert!(matches!(install(&from, &home), Err(InstallError::Link(_))));

        let home = root.path().join("b/share");
        fs::create_dir_all(folder(&home)).unwrap();
        std::os::unix::fs::symlink(&elsewhere, folder(&home).join("pets")).unwrap();
        assert!(matches!(install(&from, &home), Err(InstallError::Link(_))));
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0, "nothing was written through either link");
    }

    #[test]
    fn the_carried_copy_is_read_from_its_metadata() {
        let root = tempfile::tempdir().unwrap();
        let found = read(&carried_copy(root.path(), "0.1.2")).unwrap();
        assert_eq!(found.version, "0.1.2");
        assert_eq!(found.shells, ["48", "49", "50"]);
        let stranger = root.path().join("stranger");
        write(&stranger.join("metadata.json"), r#"{"uuid": "someone@else", "version-name": "1"}"#);
        assert_eq!(read(&stranger), None);
    }

    #[test]
    fn shell_versions_match_the_way_gnome_shell_matches_them() {
        let shells: Vec<String> = ["48", "49", "50"].map(str::to_owned).to_vec();
        assert!(supports(&shells, "50.1"));
        assert!(supports(&shells, "48.0"));
        assert!(!supports(&shells, "51.0"));
        assert!(!supports(&shells, "47.4"));
        // A development release is taken only when it is named.
        assert!(!supports(&shells, "50.alpha"));
        assert!(supports(&["51.alpha".to_owned()], "51.alpha"));
        assert!(!supports(&shells, "50"));
    }

    #[test]
    fn releases_are_placed_by_their_numbers() {
        assert!(later("0.1.10", "0.1.9"));
        assert!(later("0.2.0", "0.1.9"));
        assert!(!later("0.1.2", "0.1.2"));
        assert!(!later("0.1.1", "0.1.2"));
        // A name with anything else in it is not placed, whichever side it is on.
        assert!(!later("0.1.3", ""));
        assert!(!later("0.1.3", "0.1.3-dev"));
        assert!(!later("next", "0.1.2"));
    }
}
