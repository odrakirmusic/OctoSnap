// SPDX-License-Identifier: GPL-3.0-or-later

//! The downloadable model packs, with the size `spec/07` §2.1 wants shown next to each.
//!
//! > Language: auto-detect by default (script detection + model confidence), manual
//! > override list, downloadable language packs [D 4.8].
//!
//! The manifest is here rather than fetched, so that a first run with no network shows the
//! same list as one with, and so that every file that lands on disk was named and
//! checksummed in a commit somebody reviewed. The application does the fetching -- it is
//! the half that has a main loop, a progress bar and a place to put a cancel button.

use std::path::Path;

use crate::engine::Script;

/// The file names PaddleX exports a model under. Every pack is these two files, which is
/// what makes the manifest below two numbers and two checksums apiece.
pub const MODEL: &str = "inference.onnx";
pub const METADATA: &str = "inference.yml";

/// The detection model, which every pack needs and only one of which is ever downloaded.
pub const DETECT: Source = Source {
    tag: "detect",
    url: "https://huggingface.co/PaddlePaddle/PP-OCRv5_mobile_det_onnx/resolve/main/",
    model: 4_826_518,
    model_sha256: "a431985659dc921974177a95adcfbb90fd9e51989a5e04d70d0b75f597b6e61d",
    metadata: 903,
    metadata_sha256: "98069072e1b6b37d727fd9d9f11725faa46d6ea0de012f2ed26caea011c37699",
};

/// One downloadable thing: a PaddleX export, which is always a model and its metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Source {
    /// The directory it installs into, under `rapid::HOME`.
    pub tag: &'static str,
    /// What to prefix `inference.onnx` and `inference.yml` with.
    pub url: &'static str,
    pub model: u64,
    pub model_sha256: &'static str,
    pub metadata: u64,
    pub metadata_sha256: &'static str,
}

impl Source {
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.model + self.metadata
    }

    #[must_use]
    pub fn model_url(&self) -> String {
        format!("{}inference.onnx", self.url)
    }

    #[must_use]
    pub fn metadata_url(&self) -> String {
        format!("{}inference.yml", self.url)
    }
}

/// Whether a script's pack is on disk.
///
/// Asked of the directory rather than of an engine, and that is the point: two `stat`s,
/// no ONNX Runtime and no 4.8 MB model loaded. The settings page and the capture path both
/// need this answer *before* they promise the user anything, and neither of them can spend
/// a tenth of a second of the main loop on it (`docs/decisions.md` D97).
#[must_use]
pub fn installed(home: &Path, script: Script) -> bool {
    let directory = home.join(script.tag());
    directory.join(MODEL).is_file() && directory.join(METADATA).is_file()
}

/// Whether the detection model is on disk.
///
/// Every pack needs it and only one is ever downloaded, so it is not one of the scripts
/// and cannot be found by looking through them. A home with a script pack and no `detect/`
/// is a half-finished download rather than a machine with no packs, and the two want
/// different words.
#[must_use]
pub fn detector(home: &Path) -> bool {
    let directory = home.join(DETECT.tag);
    directory.join(MODEL).is_file() && directory.join(METADATA).is_file()
}

/// A script's pack: the recognition model for it, and what to call it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Available {
    pub script: Script,
    pub source: Source,
    /// The pack's own size *plus* the detection model, because that is what the first
    /// download costs and a size shown next to a button has to be the one it will take.
    pub bytes: u64,
}

/// Every pack there is.
///
/// The recognition models are PaddlePaddle's own ONNX exports of PP-OCRv5 mobile, which is
/// the newest line they publish in ONNX and the only source under an obviously compatible
/// licence (Apache-2.0) that is not somebody's re-upload. `spec/07` §2.1's "30+ languages"
/// is what these five scripts between them read: the Latin pack covers the Western
/// European languages, Cyrillic the Slavic ones, and so on.
#[must_use]
pub fn all() -> Vec<Available> {
    [
        (
            Script::Latin,
            "https://huggingface.co/PaddlePaddle/latin_PP-OCRv5_mobile_rec_onnx/resolve/main/",
            8_042_023,
            "7888113072263cb471b93f66dd5e2ad70548dc526fa1ace760d0d973dd121498",
            6_817,
            "0bbe984570f597af3638e50bdf2e8276f3ab26a61966096538b3b0d1849f5c84",
        ),
        (
            Script::Cjk,
            "https://huggingface.co/PaddlePaddle/PP-OCRv5_mobile_rec_onnx/resolve/main/",
            16_534_782,
            "da72dc72ca4dc220df0dfde68c1dedc31c58d3e76a25871122e5056227d50092",
            148_345,
            "5dfeb2777f6d0db8177d8128a8acfcf6e6276dc4ac73ea3bf0dc06d6a5e85d8e",
        ),
        (
            Script::Cyrillic,
            "https://huggingface.co/PaddlePaddle/cyrillic_PP-OCRv5_mobile_rec_onnx/resolve/main/",
            8_048_799,
            "5371ee1ddaa7983cc62d0818d99e982b6804638c85e4f960d59a574094e172e5",
            6_991,
            "5c76cc91fa98410178a09f498db10050d0ec1634a660053d3005ab7be581f501",
        ),
        (
            Script::Arabic,
            "https://huggingface.co/PaddlePaddle/arabic_PP-OCRv5_mobile_rec_onnx/resolve/main/",
            7_998_947,
            "799113ebf267fbe742deb99eb36e8d42c9ddc5291ceacf92add41b4d52a59110",
            6_165,
            "21368419e6c016c31db55d316d59e11c128e1913e6e6fe10287084710043d3a6",
        ),
        (
            Script::Devanagari,
            "https://huggingface.co/PaddlePaddle/devanagari_PP-OCRv5_mobile_rec_onnx/resolve/main/",
            7_912_311,
            "cb789212ce96c69d3e74728ae4309d179281d68cb3945d0616b67cafab41c986",
            5_027,
            "9bd172dd26440c8ce94d1cde5d5baea6aefdc7cf3c5c8492e0beedef656d4e54",
        ),
    ]
    .into_iter()
    .map(|(script, url, model, model_sha256, metadata, metadata_sha256)| Available {
        script,
        source: Source { tag: script.tag(), url, model, model_sha256, metadata, metadata_sha256 },
        bytes: model + metadata + DETECT.bytes(),
    })
    .collect()
}

/// The pack for a script.
#[must_use]
pub fn of(script: Script) -> Option<Available> {
    all().into_iter().find(|pack| pack.script == script)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn every_script_has_a_pack_and_every_pack_a_script() {
        let packs = all();
        assert_eq!(packs.len(), Script::ALL.len());
        for script in Script::ALL {
            let pack = of(script).expect("a pack");
            assert_eq!(pack.source.tag, script.tag());
            assert!(pack.source.url.starts_with("https://huggingface.co/PaddlePaddle/"));
            assert!(pack.source.url.ends_with('/'));
        }
    }

    #[test]
    fn the_size_shown_is_the_size_the_first_download_costs() {
        // A pack is useless without the detection model, so the number next to the button
        // has to include it -- and the second pack a person installs does not pay it twice.
        let latin = of(Script::Latin).expect("a pack");
        assert_eq!(latin.bytes, latin.source.bytes() + DETECT.bytes());
        assert!(latin.bytes > 12_000_000 && latin.bytes < 14_000_000, "{}", latin.bytes);
    }

    /// Every byte that lands on disk was named in a commit somebody reviewed. Both files,
    /// not just the model: the `inference.yml` carries the alphabet the decoder indexes
    /// into, so a wrong one is not a broken read but a *plausible* wrong one.
    #[test]
    fn every_downloaded_file_is_pinned_to_a_checksum() {
        assert_eq!(DETECT.model_sha256.len(), 64);
        assert_eq!(DETECT.metadata_sha256.len(), 64);
        for pack in all() {
            assert_eq!(pack.source.model_sha256.len(), 64, "{:?}", pack.script);
            assert_eq!(pack.source.metadata_sha256.len(), 64, "{:?}", pack.script);
        }
    }

    #[test]
    fn the_urls_name_the_two_files_paddlex_exports() {
        assert!(DETECT.model_url().ends_with("/inference.onnx"));
        assert!(DETECT.metadata_url().ends_with("/inference.yml"));
    }

    /// Writes both of a pack's files into `home/<tag>/`, the way a finished download does.
    fn install(home: &Path, tag: &str) {
        let directory = home.join(tag);
        std::fs::create_dir_all(&directory).expect("a pack directory");
        std::fs::write(directory.join(MODEL), b"weights").expect("a model");
        std::fs::write(directory.join(METADATA), b"alphabet").expect("metadata");
    }

    #[test]
    fn a_pack_is_installed_when_both_of_its_files_are_there() {
        let home = tempfile::tempdir().expect("tempdir");
        assert!(!installed(home.path(), Script::Latin));
        install(home.path(), Script::Latin.tag());
        assert!(installed(home.path(), Script::Latin));
        assert!(!installed(home.path(), Script::Cyrillic), "one pack is not all of them");
    }

    /// The alphabet is half the pack: a model with no `inference.yml` decodes into
    /// nothing, so a download interrupted between the two files is not an install.
    #[test]
    fn a_pack_missing_its_alphabet_is_not_installed() {
        let home = tempfile::tempdir().expect("tempdir");
        let directory = home.path().join(Script::Latin.tag());
        std::fs::create_dir_all(&directory).expect("a pack directory");
        std::fs::write(directory.join(MODEL), b"weights").expect("a model");
        assert!(!installed(home.path(), Script::Latin));
    }

    /// The detector is not one of the scripts, so looking through them cannot find it --
    /// which is the whole reason it has its own question.
    #[test]
    fn the_detector_is_found_on_its_own_and_not_among_the_scripts() {
        let home = tempfile::tempdir().expect("tempdir");
        install(home.path(), Script::Latin.tag());
        assert!(!detector(home.path()), "a script pack is not a detection model");
        install(home.path(), DETECT.tag);
        assert!(detector(home.path()));
        assert!(!all().iter().any(|pack| pack.script.tag() == DETECT.tag));
    }
}
