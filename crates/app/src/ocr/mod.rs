// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/07` §2's text recognition, in the application.
//!
//! > Select an area -> sound -> recognized text is copied -> notification "Text copied"
//! > with the first line as preview and a **Show** action opening a small window with the
//! > text (editable, Copy, Search).
//!
//! `crates/ocr` decides what a capture *says*; this decides what happens next, and does
//! it. The engine is held for the process rather than per read, because opening the
//! detection model costs a fifth of a second and a person who reads one region reads
//! another.

pub mod download;
pub mod pill;
pub mod prefs;
pub mod reading;
pub mod window;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use octosnap_ocr::{Breaks, Engine, Gray, Read, Reader, Script, packs, rapid::Rapid};
use octosnap_stitch::Frame;
use tracing::{info, warn};

/// The one engine, or nothing yet.
fn slot() -> &'static Mutex<Option<Arc<dyn Engine>>> {
    static ENGINE: OnceLock<Mutex<Option<Arc<dyn Engine>>>> = OnceLock::new();
    ENGINE.get_or_init(|| Mutex::new(None))
}

/// What to tell the user when the models are there and the runtime that runs them is not.
///
/// `spec/07` §2.1's read has two ways to be impossible and they want different words. D85
/// gave the missing pack an **Open Settings** button because there is something to press;
/// this one has nothing in the app to press, so it names the library and stops (D97).
pub const NO_ENGINE: &str =
    "Text recognition needs the ONNX Runtime library, which is not installed on this machine.";

/// The engine, opened on first use and kept for the process.
///
/// **Only ever called from a worker thread.** Opening it is loading ONNX Runtime and a
/// 4.8 MB detection model, measured at 105–220 ms, and that is a tenth to a fifth of a
/// second of frozen shell wherever it happens on the main loop (D97). [`ready`] and
/// [`packs`] used to reach it and no longer do; [`read`] is the one caller left, from
/// inside its `spawn_blocking`.
///
/// Behind a lock rather than a `OnceLock`, and that is [`reopen`]'s doing. The first pack
/// a user installs brings the first detection model on the machine with it, so a process
/// that had already tried to open an engine would otherwise go on believing there was
/// none until it was restarted.
///
/// A failure is **not** kept. The two reasons to fail -- no ONNX Runtime, no detection
/// model -- are both things a user fixes from outside this process, and a cached refusal
/// would outlive the fix. Retrying costs a failed `dlopen`, which is a few microseconds.
///
/// # Errors
/// The message to show the user, already phrased for them.
fn engine() -> Result<Arc<dyn Engine>, String> {
    let mut held = slot().lock().map_err(|_| {
        // Poisoned: something panicked while the engine was being swapped.
        warn!("the text-recognition engine is unavailable after a panic");
        NO_ENGINE.to_owned()
    })?;
    if let Some(engine) = held.as_ref() {
        return Ok(Arc::clone(engine));
    }
    let home = home();
    let at = std::time::Instant::now();
    let rapid = Rapid::open(&home).map_err(|why| {
        warn!(home = %home.display(), "no text-recognition engine: {why}");
        NO_ENGINE.to_owned()
    })?;
    let ms = at.elapsed().as_millis();
    info!(home = %home.display(), ms, "opened the text-recognition engine");
    let engine: Arc<dyn Engine> = Arc::new(rapid);
    Ok(Arc::clone(held.insert(engine)))
}

/// Throws the engine away, so the next read opens a fresh one.
///
/// Called after a pack is installed or removed. Cheap, and not on the main loop: opening
/// the detection session costs about a fifth of a second and happens on the next read,
/// which is already on a worker thread.
pub fn reopen() {
    if let Ok(mut held) = slot().lock() {
        *held = None;
        info!("the text-recognition engine will be opened again on the next read");
    }
}

/// Where the model packs live. `spec/10` §4 names it: `~/.local/share/octosnap/ocr/`.
#[must_use]
pub fn home() -> PathBuf {
    glib::user_data_dir().join("octosnap").join("ocr")
}

/// Whether anything can be read at all, which is what the settings page and the
/// post-capture path both need to know before they promise the user anything.
///
/// Asked of the disk, not of the engine (D97). It used to open ONNX Runtime and the
/// detection model to find out, which is a tenth of a second of frozen main loop to answer
/// a question about files -- and it answered it wrong on a machine that had the packs and
/// not the runtime, sending the user to Settings to download what was already there.
///
/// The detection model counts, because every pack needs it and only one is ever
/// downloaded: a home with `latin/` and no `detect/` is an interrupted download, and D85's
/// "no language pack is installed" is the right thing to say about it.
#[must_use]
pub fn ready() -> bool {
    ready_in(&home())
}

/// [`ready`] against a named directory, so the rule can be tested without a home.
fn ready_in(home: &Path) -> bool {
    packs::detector(home) && packs::all().iter().any(|pack| packs::installed(home, pack.script))
}

/// The pack Settings points at when a read found none (D171).
///
/// The script `ocr-language` names, when it names one: the user has said what they read.
/// Otherwise the one the user's own language is written in, from the first of `languages`
/// (`g_get_language_names`, most preferred first) that names a language, and Latin when
/// none does, because Latin is the script of the most languages and of every URL.
#[must_use]
pub fn suggested<S: AsRef<str>>(configured: Option<Script>, languages: &[S]) -> Script {
    if let Some(script) = configured {
        return script;
    }
    let language = languages
        .iter()
        .map(|name| {
            // `de_CH.UTF-8@euro` -> `de`.
            let name = name.as_ref();
            name.split(['_', '.', '@']).next().unwrap_or(name)
        })
        .find(|code| !code.is_empty() && *code != "C" && *code != "POSIX");
    match language {
        Some("zh" | "ja" | "ko") => Script::Cjk,
        Some("ru" | "uk" | "be" | "bg" | "sr" | "mk" | "kk" | "ky" | "mn" | "tg" | "tt") => {
            Script::Cyrillic
        }
        Some("ar" | "fa" | "ur" | "ps" | "ug" | "ckb") => Script::Arabic,
        Some("hi" | "mr" | "ne" | "sa" | "mai" | "bho") => Script::Devanagari,
        _ => Script::Latin,
    }
}

/// A pack has just been installed: the read that waited for one is read now (D171).
///
/// Its result goes where the user is looking. Settings, where they pressed Install, says
/// it in a toast while it is open; when it has been closed during the download, the
/// notification a text capture always ends with says it instead.
pub fn pack_installed() {
    use gtk::prelude::*;
    let Some(flow) = crate::capture_flow() else { return };
    glib::spawn_future_local(async move {
        let Some(outcome) = flow.read_waiting().await else { return };
        if crate::prefs::read_finished(&outcome) {
            return;
        }
        let Some(app) = gio::Application::default().and_downcast::<adw::Application>() else {
            warn!("no application to tell how the waiting read ended");
            return;
        };
        crate::notify::capture_outcome(
            &app,
            &outcome,
            crate::settings::Settings::load().notifications_enabled(),
        );
    });
}

/// Every pack, installed or not, with the size `spec/07` §2.1 wants shown.
///
/// One branch where there were two: the engine's own list and a hand-built copy of it for
/// when there was no engine. Both were the manifest with a `stat` against each row, and
/// now it is only ever that.
#[must_use]
pub fn packs() -> Vec<octosnap_ocr::Pack> {
    let home = home();
    packs::all()
        .into_iter()
        .map(|pack| octosnap_ocr::Pack {
            script: pack.script,
            bytes: pack.bytes,
            installed: packs::installed(&home, pack.script),
        })
        .collect()
}

/// Reads a capture, on a worker thread.
///
/// **Opening the engine, the decode and both models** run off the main loop: `spec/10` §7
/// allows a second and a half for a region, and a second and a half of a frozen shell is
/// not a trade anyone would make. The engine open was the exception until D97 -- a tenth
/// of a second of it, on the first read of every process, in the wrong place.
///
/// # Errors
/// The message to show the user, already phrased for them.
pub async fn read(path: &Path, script: Option<Script>) -> Result<Read, String> {
    let path = path.to_path_buf();
    let joined = gio::spawn_blocking(move || {
        // The file before the engine: a capture that cannot be read fails at once, not
        // after the fifth of a second the engine takes to open on a first read.
        let frame = Frame::read(&path).map_err(|why| format!("{why}"))?;
        let engine = engine()?;
        let image = Gray::from_rgba(frame.width(), frame.height(), frame.pixels())
            .map_err(|why| format!("{why}"))?;
        Reader::new(engine).scan(&image, script).map_err(|why| format!("{why}"))
    })
    .await;
    match joined {
        Ok(read) => read,
        Err(_) => Err("the reader stopped unexpectedly".to_owned()),
    }
}

/// What `spec/08` §1's "Keep line breaks" switch means here.
#[must_use]
pub const fn breaks(keep: bool) -> Breaks {
    if keep { Breaks::Keep } else { Breaks::Join }
}

/// What `spec/08` §1's "Language [Automatically Detect Language]" row offers.
///
/// A script or the absence of one, rather than `Option<Script>` with a name hung off it,
/// because the settings row is a list of six things the user picks from and the sixth is
/// as real a choice as the other five.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Auto,
    Only(Script),
}

impl Language {
    /// Auto first, then the scripts in `Script::ALL`'s order.
    pub const ALL: [Self; 6] = [
        Self::Auto,
        Self::Only(Script::Latin),
        Self::Only(Script::Cyrillic),
        Self::Only(Script::Cjk),
        Self::Only(Script::Arabic),
        Self::Only(Script::Devanagari),
    ];

    /// The value in `ocr-language`.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Only(script) => script.tag(),
        }
    }

    /// The row's label. `spec/08` §1 gives the first one its wording.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Auto => "Automatically Detect Language",
            Self::Only(script) => script.name(),
        }
    }
}

/// `spec/08` §1's Advanced > Text Recognition, as the capture path uses it.
///
/// Read through [`crate::settings::ocr_config`], which is where every other GSettings
/// read in this app lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// `None` is `spec/07` §2.1's "Automatically Detect Language".
    pub script: Option<Script>,
    pub breaks: Breaks,
    /// Whether the result window makes addresses clickable.
    pub links: bool,
    /// `ui-sounds`, which `spec/08` §1 groups OCR's cue under.
    pub sounds: bool,
}

impl Default for Config {
    /// `spec/08` §1's own defaults: auto, line breaks **off**, links on.
    ///
    /// Line breaks off is `Breaks::Join` and not the library's `Breaks::default()`. The
    /// library defaults to keeping them because that is what the pixels said; the
    /// *setting* defaults to joining them because what people paste text into is prose.
    fn default() -> Self {
        Self { script: None, breaks: Breaks::Join, links: true, sounds: true }
    }
}

thread_local! {
    /// The last read, for the notification's **Show** action.
    ///
    /// The text rather than a path to re-read: `spec/07` §2.1's Show opens the window on
    /// *that* capture's text, and reading the PNG again would take another second and a
    /// half to arrive at the same answer -- or a different one, if the packs changed in
    /// between. One slot, because the notification has one id.
    static LAST: RefCell<Option<(Read, Config)>> = const { RefCell::new(None) };
}

/// Remembers a read so **Show** can open it.
pub fn remember(read: Read, config: Config) {
    LAST.with(|last| *last.borrow_mut() = Some((read, config)));
}

/// The last read, if there has been one this session.
#[must_use]
pub fn last() -> Option<(Read, Config)> {
    LAST.with(|last| last.borrow().clone())
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Writes both of a pack's files into `home/<tag>/`, the way a download does.
    fn install(home: &Path, tag: &str) {
        let directory = home.join(tag);
        std::fs::create_dir_all(&directory).expect("a pack directory");
        std::fs::write(directory.join(octosnap_ocr::packs::MODEL), b"weights").expect("model");
        std::fs::write(directory.join(octosnap_ocr::packs::METADATA), b"alphabet").expect("yml");
    }

    /// D171: the pack Settings points at.
    #[test]
    fn the_suggested_pack_is_the_chosen_script_then_the_users_language() {
        let none: [&str; 0] = [];
        assert_eq!(suggested(None, &none), Script::Latin, "nothing to go on reads Latin");
        assert_eq!(suggested(None, &["C"]), Script::Latin);
        assert_eq!(suggested(None, &["de_CH.UTF-8", "de_CH", "de", "C"]), Script::Latin);
        assert_eq!(suggested(None, &["ja_JP.UTF-8", "ja", "C"]), Script::Cjk);
        assert_eq!(suggested(None, &["uk_UA.UTF-8"]), Script::Cyrillic);
        assert_eq!(suggested(None, &["fa_IR"]), Script::Arabic);
        assert_eq!(suggested(None, &["hi_IN.UTF-8"]), Script::Devanagari);
        assert_eq!(suggested(None, &["C", "ru_RU"]), Script::Cyrillic, "C is no language");
        assert_eq!(
            suggested(Some(Script::Arabic), &["ja_JP.UTF-8"]),
            Script::Arabic,
            "a script chosen in Settings wins over the locale"
        );
    }

    #[test]
    fn a_home_with_nothing_in_it_is_not_ready() {
        let home = tempfile::tempdir().expect("tempdir");
        assert!(!ready_in(home.path()));
    }

    #[test]
    fn a_detection_model_and_one_script_is_ready() {
        let home = tempfile::tempdir().expect("tempdir");
        install(home.path(), octosnap_ocr::packs::DETECT.tag);
        install(home.path(), Script::Latin.tag());
        assert!(ready_in(home.path()));
    }

    /// The case D97 exists to keep right. A pack is downloaded with the detection model,
    /// so `latin/` without `detect/` is an interrupted download -- and D85's "no language
    /// pack is installed", which sends the user to Settings, is what to say about it.
    #[test]
    fn a_script_pack_with_no_detection_model_is_an_unfinished_download() {
        let home = tempfile::tempdir().expect("tempdir");
        install(home.path(), Script::Latin.tag());
        assert!(!ready_in(home.path()), "half a download is not something to read with");
    }

    /// And the other half: a detection model on its own reads nothing, because there is no
    /// alphabet to decode into.
    #[test]
    fn a_detection_model_with_no_script_pack_is_not_ready_either() {
        let home = tempfile::tempdir().expect("tempdir");
        install(home.path(), octosnap_ocr::packs::DETECT.tag);
        assert!(!ready_in(home.path()));
    }
}
