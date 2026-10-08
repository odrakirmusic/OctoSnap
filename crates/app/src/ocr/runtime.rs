// SPDX-License-Identifier: GPL-3.0-or-later

//! What a read says when ONNX Runtime cannot be loaded (D172).
//!
//! D97 gave the missing runtime its own sentence and no button, because nothing in Settings
//! installs a system library. The sentence named the library and stopped there, so a native
//! install was told what it lacked and not where to get it, which only the README said. Now
//! it names the package, for a distribution that has one, from `os-release`, and the README's
//! section is a button away for any other.
//!
//! The Flatpak brings its own copy (`build-aux/flatpak/libonnxruntime.json`) and should never
//! get here. If it does, a package would not help it, since the sandbox does not see the
//! system's libraries, so it is told that the copy it came with did not load.

use std::sync::OnceLock;

/// The README's section on it, which a native install's refusal has a button for.
pub const HELP: &str = "https://github.com/odrakirmusic/OctoSnap#onnx-runtime";

/// Where this machine gets ONNX Runtime from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The Flatpak's own copy, in `/app/lib`.
    Bundle,
    /// The distribution's package, by name.
    Package(&'static str),
    /// A distribution with no package this build can name.
    Unknown,
}

/// The distributions whose package is known, by the names `os-release` gives them in `ID`
/// and `ID_LIKE`. The first of a machine's names found here decides, so a name with no
/// package stops the search where a later one would name the wrong one: Rocky Linux is
/// `rhel centos fedora`, and Fedora's package is not in RHEL.
///
/// Debian's runtime package carries its version in its name (`libonnxruntime1.23` in
/// Ubuntu 26.04 and Debian testing, `1.21` in trixie), so a name that is right today is
/// wrong at the next release. `libonnxruntime-dev` is the one name every release shares;
/// it depends on exactly that release's runtime and adds 0.8 MB of headers. Arch's
/// `onnxruntime` is a choice between five builds, of which `onnxruntime-cpu` is the one
/// that needs no GPU stack. Checked 2026-10-08 against Ubuntu 26.04, Debian forky, Fedora
/// 44, Arch and openSUSE Tumbleweed; README's [`HELP`] section has the same table.
const FAMILIES: &[(&str, Source)] = &[
    ("debian", Source::Package("libonnxruntime-dev")),
    ("ubuntu", Source::Package("libonnxruntime-dev")),
    ("fedora", Source::Package("onnxruntime")),
    ("arch", Source::Package("onnxruntime-cpu")),
    ("opensuse-tumbleweed", Source::Package("libonnxruntime1")),
    ("opensuse-slowroll", Source::Package("libonnxruntime1")),
    ("rhel", Source::Unknown),
    ("centos", Source::Unknown),
    ("opensuse-leap", Source::Unknown),
    // MicroOS, Aeon and Kalpa: `opensuse suse`, and no `zypper install` on a read-only root.
    ("opensuse", Source::Unknown),
    ("suse", Source::Unknown),
];

/// Where ONNX Runtime comes from: the Flatpak's own, or the package of the distribution
/// `os_release` describes.
#[must_use]
pub fn source(sandboxed: bool, os_release: &str) -> Source {
    if sandboxed {
        return Source::Bundle;
    }
    let id = field(os_release, "ID").unwrap_or_default();
    let like = field(os_release, "ID_LIKE").unwrap_or_default();
    std::iter::once(id)
        .chain(like.split_whitespace())
        .find_map(|name| {
            FAMILIES.iter().find(|(known, _)| *known == name).map(|(_, source)| *source)
        })
        .unwrap_or(Source::Unknown)
}

/// One `KEY=value` of an `os-release` file, with its quotes taken off.
fn field<'a>(os_release: &'a str, key: &str) -> Option<&'a str> {
    os_release.lines().find_map(|line| {
        let value = line.trim().strip_prefix(key)?.strip_prefix('=')?.trim();
        let unquoted = ['"', '\'']
            .iter()
            .find_map(|quote| value.strip_prefix(*quote)?.strip_suffix(*quote))
            .unwrap_or(value);
        Some(unquoted)
    })
}

/// What the user is told, in the sizes the places it is told in need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The notification's title.
    pub title: &'static str,
    /// The notification's body. It is also the read's `Recognised::Failed` message, which
    /// is how the notification and the toasts know a read ended here.
    pub body: String,
    /// A toast's one line: the editor's Copy Text, and Settings when the read waited for a
    /// pack (D171).
    pub toast: String,
    /// Whether the README's section has the way forward, which is the button's to open.
    /// Not for the Flatpak, whose remedy is not a package.
    pub help: bool,
}

impl Refusal {
    #[must_use]
    pub fn new(source: Source) -> Self {
        // The sentence it ends on is the alternative that always works.
        const FLATPAK: &str = "or use the OctoSnap Flatpak, which brings its own.";
        match source {
            Source::Package(package) => Self {
                title: "Text recognition needs ONNX Runtime",
                body: format!("Install the {package} package and try again, {FLATPAK}"),
                toast: format!("To read text, install {package}"),
                help: true,
            },
            Source::Unknown => Self {
                title: "Text recognition needs ONNX Runtime",
                body: format!("Install ONNX Runtime 1.17 or newer and try again, {FLATPAK}"),
                toast: "To read text, install ONNX Runtime".to_owned(),
                help: true,
            },
            // A reinstall is a new deployment, and the running app keeps the old one, so it
            // counts from the next login.
            Source::Bundle => Self {
                title: "Could not read the text",
                body: "The ONNX Runtime this Flatpak comes with did not load. Reinstall \
                       OctoSnap, then log out and back in."
                    .to_owned(),
                toast: "ONNX Runtime did not load: reinstall OctoSnap".to_owned(),
                help: false,
            },
        }
    }
}

/// This machine's refusal, worked out once.
#[must_use]
pub fn refusal() -> &'static Refusal {
    static REFUSAL: OnceLock<Refusal> = OnceLock::new();
    REFUSAL.get_or_init(|| {
        let sandboxed = crate::settings::sandboxed();
        // `os-release(5)`: the first, then the second.
        let os_release = if sandboxed {
            String::new()
        } else {
            std::fs::read_to_string("/etc/os-release")
                .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
                .unwrap_or_default()
        };
        let source = source(sandboxed, &os_release);
        tracing::info!(?source, "where ONNX Runtime would come from");
        Refusal::new(source)
    })
}

/// What a read says when it found an ONNX Runtime and the library would not load: one
/// older than 1.17, or not a library at all. `detail` is the file and why.
///
/// Not [`refusal`]'s advice, which could be wrong twice here. A package installed beside
/// the copy that was found may not be the one found next, so the copy is named. And the
/// next read will not try again, because `ort` cannot (`crates/ocr`'s `rapid::runtime`), so
/// the app has to start again, which is the next login. The Flatpak's copy is the only one
/// it could have found, and its refusal already says to reinstall and log in again.
#[must_use]
pub fn would_not_load(detail: &str) -> String {
    would_not_load_in(refusal(), detail)
}

fn would_not_load_in(refusal: &Refusal, detail: &str) -> String {
    if !refusal.help {
        return refusal.body.clone();
    }
    format!(
        "ONNX Runtime would not load ({detail}). Text recognition needs 1.17 or newer: replace \
         that copy, then log out and back in."
    )
}

/// Whether a read's failure is this one.
#[must_use]
pub fn is_refusal(why: &str) -> bool {
    why == refusal().body
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This machine's, as shipped.
    const UBUNTU: &str = "PRETTY_NAME=\"Ubuntu 26.04.1 LTS\"\nNAME=\"Ubuntu\"\n\
        VERSION_ID=\"26.04\"\nVERSION_CODENAME=resolute\nID=ubuntu\nID_LIKE=debian\n\
        UBUNTU_CODENAME=resolute\n";

    #[test]
    fn a_distribution_is_known_by_its_id_then_what_it_is_like() {
        assert_eq!(source(false, UBUNTU), Source::Package("libonnxruntime-dev"));
        assert_eq!(source(false, "ID=debian\n"), Source::Package("libonnxruntime-dev"));
        assert_eq!(
            source(false, "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n"),
            Source::Package("libonnxruntime-dev"),
            "Mint is known by what it is like"
        );
        assert_eq!(source(false, "ID=fedora\nVERSION_ID=44\n"), Source::Package("onnxruntime"));
        assert_eq!(source(false, "ID=arch\n"), Source::Package("onnxruntime-cpu"));
        assert_eq!(source(false, "ID=manjaro\nID_LIKE=arch\n"), Source::Package("onnxruntime-cpu"));
        assert_eq!(
            source(false, "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n"),
            Source::Package("libonnxruntime1")
        );
    }

    /// A name with no package stops the search before a later one names the wrong package.
    #[test]
    fn a_distribution_without_the_package_is_not_sent_to_another_ones() {
        let rocky = "ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n";
        assert_eq!(source(false, rocky), Source::Unknown);
        let leap = "ID=\"opensuse-leap\"\nID_LIKE=\"suse opensuse\"\n";
        assert_eq!(source(false, leap), Source::Unknown);
        assert_eq!(source(false, "ID=nixos\n"), Source::Unknown);
        assert_eq!(source(false, ""), Source::Unknown, "no os-release at all");
        assert_eq!(source(false, "ID_LIKE=debian\n"), Source::Package("libonnxruntime-dev"));
    }

    /// `ID` is not `ID_LIKE`, and single quotes are quotes too.
    #[test]
    fn the_fields_are_read_whole() {
        assert_eq!(field("ID_LIKE=debian\nID=ubuntu\n", "ID"), Some("ubuntu"));
        assert_eq!(field("ID='fedora'\n", "ID"), Some("fedora"));
        assert_eq!(field("  ID=\"arch\"  \n", "ID"), Some("arch"));
        assert_eq!(field("NAME=x\n", "ID"), None);
    }

    /// The Flatpak's answer does not depend on the host's distribution, which it cannot use.
    #[test]
    fn the_flatpak_is_its_own_source() {
        assert_eq!(source(true, UBUNTU), Source::Bundle);
    }

    /// D172's point: every native refusal says how to get the library.
    #[test]
    fn a_native_refusal_says_how_to_get_it() {
        let ubuntu = Refusal::new(source(false, UBUNTU));
        assert_eq!(
            ubuntu.body,
            "Install the libonnxruntime-dev package and try again, or use the OctoSnap \
             Flatpak, which brings its own."
        );
        assert_eq!(ubuntu.toast, "To read text, install libonnxruntime-dev");
        assert!(ubuntu.help, "the README's section is a button away");

        let unknown = Refusal::new(Source::Unknown);
        assert!(unknown.body.contains("ONNX Runtime 1.17 or newer"), "{}", unknown.body);
        assert!(unknown.body.contains("Flatpak"), "{}", unknown.body);
        assert!(unknown.help);
    }

    /// The Flatpak is not sent to a package it could not see, nor to the README's section
    /// about one.
    #[test]
    fn the_flatpak_is_told_its_own_copy_did_not_load() {
        let flatpak = Refusal::new(Source::Bundle);
        assert!(!flatpak.body.contains("package"), "{}", flatpak.body);
        assert!(flatpak.body.contains("Reinstall OctoSnap"), "{}", flatpak.body);
        assert!(!flatpak.help);
    }

    /// A copy that was found and would not load is named, and the way past it is a new
    /// login, since the next read will not try again. In the Flatpak it is the bundle's.
    #[test]
    fn a_copy_that_would_not_load_is_named() {
        let detail = "/usr/local/lib/libonnxruntime.so.1.16.3: expected version >= '1.17.x'";
        let native = would_not_load_in(&Refusal::new(Source::Package("onnxruntime")), detail);
        assert!(native.contains(detail), "{native}");
        assert!(native.contains("log out and back in"), "{native}");
        assert!(!native.contains("Install the"), "a package beside it may not be the one found");

        let flatpak = Refusal::new(Source::Bundle);
        assert_eq!(would_not_load_in(&flatpak, detail), flatpak.body);
    }

    /// What the app names and what the README's section names are the same packages, and
    /// the section is where [`HELP`] points.
    #[test]
    fn the_readme_names_every_package_the_app_does() {
        let readme = include_str!("../../../../README.md");
        let anchor = HELP.rsplit_once('#').map(|(_, anchor)| anchor).unwrap_or_default();
        assert_eq!(anchor, "onnx-runtime");
        assert!(readme.contains("\n#### ONNX Runtime\n"), "README has no ONNX Runtime section");
        for (_, source) in FAMILIES {
            if let Source::Package(package) = source {
                assert!(readme.contains(&format!("`{package}`")), "README does not name {package}");
            }
        }
    }
}
