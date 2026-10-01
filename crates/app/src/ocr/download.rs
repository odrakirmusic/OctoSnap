// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/07` §2.1's "downloadable language packs (with size shown)".
//!
//! The **only** thing in OctoSnap that reaches the network, and `spec/10` §10 names it:
//! "the only network calls are user-configured providers and OCR pack downloads". So it
//! is deliberately narrow -- it fetches two files from one URL a commit reviewed, checks
//! each against a checksum that was in that same commit, and has no other reachable
//! destination. `crates/ocr`'s `packs` module holds the manifest and says why it is a
//! manifest rather than something fetched.
//!
//! Blocking, and run on `gio::spawn_blocking` by the caller. The alternative --
//! `gio::File::for_uri` over an `https://` URL -- would put the feature behind gvfs's
//! http backend and libsoup being present in whatever sandbox the app is packaged into,
//! which is not something to discover on a user's machine.
//!
//! Every byte goes through [`glib::Checksum`] on its way to disk, so nothing is read
//! twice and nothing has to be held in memory: a CJK pack is 16 MB.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use octosnap_ocr::Script;
use octosnap_ocr::packs::{self, Source};
use tracing::{info, warn};

/// How much is read at a time. Big enough that the checksum and the write are not called
/// once a packet, small enough that Cancel stops within a frame or two of the press.
const CHUNK: usize = 64 * 1024;

/// How long to wait for the first byte, and for each one after it.
const CONNECT: Duration = Duration::from_secs(20);
const READ: Duration = Duration::from_secs(60);

/// What a half-finished file is called. Renamed into place only once its checksum
/// matches, so an interrupted download can never leave a model the engine would load.
const PARTIAL: &str = "part";

#[derive(Debug, thiserror::Error)]
pub enum Failure {
    /// The user pressed Cancel. Not an error to report, only a reason to stop.
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Why(String),
}

impl Failure {
    fn why(what: impl std::fmt::Display) -> Self {
        Self::Why(what.to_string())
    }
}

/// How far a download has got, readable from the thread that is watching it.
///
/// The `crates/media` encoder's [`Progress`](octosnap_media::encoder::Progress) pattern
/// and for the same reason: shared counters rather than a channel, because the main loop
/// is going to look at them on a ticker anyway and a channel would only add a queue
/// between two numbers and a progress bar.
#[derive(Debug, Clone, Default)]
pub struct Progress(Arc<Counters>);

#[derive(Debug, Default)]
struct Counters {
    done: AtomicU64,
    total: AtomicU64,
    cancelled: AtomicBool,
}

impl Progress {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes written and bytes expected. A total of zero means "not yet known".
    #[must_use]
    pub fn bytes(&self) -> (u64, u64) {
        (self.0.done.load(Ordering::Relaxed), self.0.total.load(Ordering::Relaxed))
    }

    /// 0.0 to 1.0, or `None` while the total is unknown.
    #[must_use]
    pub fn fraction(&self) -> Option<f64> {
        let (done, total) = self.bytes();
        #[allow(clippy::cast_precision_loss)]
        (total > 0).then(|| (done as f64 / total as f64).clamp(0.0, 1.0))
    }

    /// Asks the worker to stop at its next chunk.
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Relaxed)
    }

    fn expect(&self, bytes: u64) {
        self.0.total.store(bytes, Ordering::Relaxed);
    }

    fn advance(&self, bytes: u64) {
        self.0.done.fetch_add(bytes, Ordering::Relaxed);
    }
}

/// Installs a script's pack under `home`, blocking until it is on disk.
///
/// The detection model comes with the first pack and is shared by every one after it,
/// which is why the size next to the button includes it and the second install is
/// smaller than the first.
///
/// # Errors
/// [`Failure::Cancelled`] when the user asked to stop, and [`Failure::Why`] with a
/// message already phrased for them otherwise.
pub fn install(home: &Path, script: Script, progress: &Progress) -> Result<(), Failure> {
    let pack = packs::of(script).ok_or_else(|| Failure::Why("no such pack".to_owned()))?;

    // The detection model only when it is not already there: two packs share it, and a
    // second download of five megabytes to overwrite an identical file is pure waste.
    let mut wanted: Vec<(&str, Source)> = Vec::with_capacity(2);
    if !complete(&home.join(packs::DETECT.tag), &packs::DETECT) {
        wanted.push((packs::DETECT.tag, packs::DETECT));
    }
    wanted.push((pack.source.tag, pack.source));

    progress.expect(wanted.iter().map(|(_, source)| source.bytes()).sum());
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT))
        .timeout_per_call(Some(READ))
        // Nothing here follows a redirect off HuggingFace's own CDN by accident: the
        // checksum is the real guard, and a body that does not match it is thrown away.
        .build()
        .new_agent();

    for (tag, source) in wanted {
        let directory = home.join(tag);
        std::fs::create_dir_all(&directory).map_err(Failure::why)?;
        fetch(
            &agent,
            &source.model_url(),
            &directory.join(packs::MODEL),
            source.model_sha256,
            progress,
        )?;
        fetch(
            &agent,
            &source.metadata_url(),
            &directory.join(packs::METADATA),
            source.metadata_sha256,
            progress,
        )?;
        info!(tag, directory = %directory.display(), "installed an OCR model");
    }
    Ok(())
}

/// Deletes a script's pack. The detection model stays: it belongs to every pack, and the
/// user removing one of five has not said they are finished with the feature.
///
/// # Errors
/// The message to show, if the directory is there and will not go.
pub fn remove(home: &Path, script: Script) -> Result<(), String> {
    let directory = home.join(script.tag());
    if !directory.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&directory).map_err(|why| why.to_string())?;
    info!(directory = %directory.display(), "removed an OCR model");
    Ok(())
}

/// Whether a directory already holds both of a source's files at the right size.
///
/// The size and not the checksum: this runs on the main thread to decide whether to
/// charge the user five megabytes, and hashing 16 MB to answer it would freeze the
/// dialog. The checksum is still what decides whether a *downloaded* file is kept.
fn complete(directory: &Path, source: &Source) -> bool {
    let right = |name: &str, bytes: u64| {
        std::fs::metadata(directory.join(name)).is_ok_and(|meta| meta.len() == bytes)
    };
    right(packs::MODEL, source.model) && right(packs::METADATA, source.metadata)
}

/// Fetches one file, hashing as it goes, and puts it in place only if the hash matches.
fn fetch(
    agent: &ureq::Agent,
    url: &str,
    destination: &Path,
    sha256: &str,
    progress: &Progress,
) -> Result<(), Failure> {
    let partial = destination.with_extension(PARTIAL);
    let outcome = stream(agent, url, &partial, sha256, progress);
    match outcome {
        Ok(()) => std::fs::rename(&partial, destination).map_err(Failure::why),
        Err(e) => {
            // A half-file is worse than no file: it is the right size on a second glance
            // and the wrong bytes on every read after that.
            if let Err(why) = std::fs::remove_file(&partial)
                && why.kind() != std::io::ErrorKind::NotFound
            {
                warn!(path = %partial.display(), "could not remove a partial download: {why}");
            }
            Err(e)
        }
    }
}

fn stream(
    agent: &ureq::Agent,
    url: &str,
    partial: &Path,
    sha256: &str,
    progress: &Progress,
) -> Result<(), Failure> {
    let response = agent.get(url).call().map_err(|e| Failure::Why(reason(url, &e)))?;
    let status = response.status();
    if !status.is_success() {
        return Err(Failure::Why(format!("{url} answered {status}")));
    }
    let mut body = response.into_body().into_reader();

    let mut file = std::fs::File::create(partial).map_err(Failure::why)?;
    let mut digest =
        glib::Checksum::new(glib::ChecksumType::Sha256).ok_or_else(|| {
            Failure::Why("this build of GLib has no SHA-256".to_owned())
        })?;
    let mut buffer = vec![0_u8; CHUNK];
    loop {
        if progress.cancelled() {
            return Err(Failure::Cancelled);
        }
        let read = body.read(&mut buffer).map_err(Failure::why)?;
        if read == 0 {
            break;
        }
        let chunk = buffer.get(..read).unwrap_or(&[]);
        digest.update(chunk);
        file.write_all(chunk).map_err(Failure::why)?;
        progress.advance(read as u64);
    }
    file.flush().map_err(Failure::why)?;

    let got = digest.string().unwrap_or_default();
    if got != sha256 {
        return Err(Failure::Why(format!(
            "{} is not the file it should be: expected {}, got {got}",
            name(url),
            sha256
        )));
    }
    Ok(())
}

/// What went wrong, in words rather than in a transport's vocabulary.
///
/// Every one of these is a thing the user can act on -- turn the network on, wait, try
/// again -- and "http error: connection failed" is not.
fn reason(url: &str, error: &ureq::Error) -> String {
    match error {
        ureq::Error::ConnectionFailed | ureq::Error::Io(_) => {
            format!("could not reach {}", host(url))
        }
        ureq::Error::Timeout(_) => format!("{} did not answer in time", host(url)),
        ureq::Error::StatusCode(code) => format!("{} answered {code}", host(url)),
        other => format!("could not download from {}: {other}", host(url)),
    }
}

/// The host part of a URL, for a message that names somewhere the user recognises.
fn host(url: &str) -> &str {
    url.trim_start_matches("https://").split('/').next().unwrap_or(url)
}

/// The file's own name, for the checksum message.
fn name(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or(url)
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_fraction_needs_a_total() {
        let progress = Progress::new();
        assert_eq!(progress.fraction(), None);
        progress.expect(100);
        progress.advance(25);
        assert_eq!(progress.fraction(), Some(0.25));
        assert_eq!(progress.bytes(), (25, 100));
    }

    /// A server that sends more than it promised must not push the bar past the end.
    #[test]
    fn a_fraction_never_goes_past_one() {
        let progress = Progress::new();
        progress.expect(10);
        progress.advance(40);
        assert_eq!(progress.fraction(), Some(1.0));
    }

    #[test]
    fn cancelling_is_visible_to_the_worker() {
        let progress = Progress::new();
        assert!(!progress.cancelled());
        progress.clone().cancel();
        assert!(progress.cancelled());
    }

    /// The message names somewhere the user has heard of, not a URL path.
    #[test]
    fn a_failure_names_the_host() {
        let url = "https://huggingface.co/PaddlePaddle/x/resolve/main/inference.onnx";
        assert_eq!(host(url), "huggingface.co");
        assert_eq!(name(url), "inference.onnx");
    }

    /// The one test that actually reaches the network, and therefore the only one that
    /// can say the manifest is right: the URLs resolve, the sizes are the sizes and every
    /// checksum is the file's. Ignored by default so `cargo test` stays offline and
    /// silent; run it after touching `packs.rs` with
    /// `cargo test -p octosnap-app --bin octosnap-app -- --ignored --nocapture`.
    #[test]
    #[ignore = "downloads about 13 MB from huggingface.co"]
    fn a_real_pack_downloads_and_verifies() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let progress = Progress::new();
        install(dir.path(), Script::Cyrillic, &progress).expect("the pack installs");

        let detect = packs::DETECT;
        assert!(complete(&dir.path().join(detect.tag), &detect), "the detection model");
        let pack = packs::of(Script::Cyrillic).expect("a pack");
        assert!(complete(&dir.path().join(pack.source.tag), &pack.source), "the recogniser");

        let (done, total) = progress.bytes();
        assert_eq!(total, detect.bytes() + pack.source.bytes());
        assert_eq!(done, total, "every byte the manifest promised");

        // And it goes away again, leaving the shared model for the next pack.
        remove(dir.path(), Script::Cyrillic).expect("removed");
        assert!(!dir.path().join(pack.source.tag).exists());
        assert!(complete(&dir.path().join(detect.tag), &detect), "the detection model stays");
    }

    /// `complete` is what decides whether the detection model is downloaded again, so a
    /// truncated file has to read as missing rather than as there.
    #[test]
    fn a_file_of_the_wrong_size_is_not_complete() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let source = packs::DETECT;
        assert!(!complete(dir.path(), &source));
        std::fs::write(dir.path().join(packs::MODEL), b"not the model").expect("write");
        std::fs::write(dir.path().join(packs::METADATA), b"nor the metadata").expect("write");
        assert!(!complete(dir.path(), &source));
    }
}
