// SPDX-License-Identifier: GPL-3.0-or-later
//! A recording's GIF, written from its reel the first time something needs the file (D113).
//!
//! A recording's capture is `<id>.gif`, as it always was, and everything that decides what
//! a capture is still reads that name. What is new is that the file is not there when the
//! recording stops: its frames are, losslessly, in `<id>.reel` beside it, and the GIF is
//! written from them when something needs the file -- a Save, a Copy, the editor handing
//! it back unchanged -- at the quality, frame rate and size the reel says. Until then the
//! card and the editor work from the frames, and nothing has been encoded.
//!
//! One render per file, shared: a Copy pressed while a Save is writing the same GIF waits
//! for that render rather than starting a second gifski over the same frames. The GIF is
//! written under a `.part` name and renamed when it is whole, so a file at `<id>.gif` is
//! always a finished one.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::{gio, glib};
use octosnap_media::encoder::Progress;
use octosnap_media::gif_edit;
use octosnap_media::reel::{self, Reel};
use tracing::{info, warn};

/// How often a caller waiting for a render looks at it.
const POLL: Duration = Duration::from_millis(50);

/// A render under way.
struct Render {
    progress: Progress,
    /// `Some` once it is over.
    done: RefCell<Option<Result<(), String>>>,
}

thread_local! {
    static RENDERS: RefCell<HashMap<PathBuf, Rc<Render>>> = RefCell::default();
}

/// Whether the GIF at `path` has still to be written: no file there, and a reel beside it.
#[must_use]
pub fn pending(path: &Path) -> bool {
    !path.exists() && reel::is_reel(&reel::beside(path))
}

/// Whether the capture at `path` is there to show: its file, or the frames a recording's
/// GIF is still to be written from.
#[must_use]
pub fn present(path: &Path) -> bool {
    path.is_file() || pending(path)
}

/// How far the render writing `path` has got, while one is under way: what a card shows.
#[must_use]
pub fn progress_of(path: &Path) -> Option<Progress> {
    RENDERS.with(|renders| renders.borrow().get(path).map(|render| render.progress.clone()))
}

/// Every GIF being written now. The spool janitor leaves their reels alone.
#[must_use]
pub fn rendering() -> Vec<PathBuf> {
    RENDERS.with(|renders| renders.borrow().keys().cloned().collect())
}

/// Starts writing the GIF at `path` from its reel, or joins the render already doing so,
/// and answers its progress. `None` when there is nothing to write: the file is there, or
/// there is no reel to write it from.
pub fn start(path: &Path) -> Option<Progress> {
    if let Some(progress) = progress_of(path) {
        return Some(progress);
    }
    if !pending(path) {
        return None;
    }
    let render = Rc::new(Render { progress: Progress::new(), done: RefCell::new(None) });
    RENDERS.with(|renders| renders.borrow_mut().insert(path.to_path_buf(), Rc::clone(&render)));
    let (out, dir) = (path.to_path_buf(), reel::beside(path));
    let progress = render.progress.clone();
    glib::spawn_future_local(async move {
        let started = std::time::Instant::now();
        let part = out.with_extension("gif.part");
        let (to, watched) = (part.clone(), render.progress.clone());
        let result = gio::spawn_blocking(move || write(&dir, &to, &watched))
            .await
            .unwrap_or_else(|_| Err("the encoder thread panicked".to_owned()))
            .and_then(|()| {
                std::fs::rename(&part, &out)
                    .map_err(|e| format!("could not put the GIF in place: {e}"))
            });
        match &result {
            Ok(()) => info!(
                path = %out.display(),
                ms = started.elapsed().as_millis(),
                "a recording's GIF was written from its frames"
            ),
            Err(e) => {
                warn!(path = %out.display(), "a recording's GIF could not be written: {e}");
                let _ = std::fs::remove_file(&part);
            }
        }
        *render.done.borrow_mut() = Some(result);
        RENDERS.with(|renders| renders.borrow_mut().remove(&out));
    });
    Some(progress)
}

/// Makes sure the GIF at `path` is there: at once when it is, after writing it from its
/// reel when it is not ([`start`]). `Err` is a sentence for the user.
pub async fn ensure(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    let render = RENDERS.with(|renders| renders.borrow().get(path).cloned());
    let render = match render {
        Some(render) => render,
        None => {
            if start(path).is_none() {
                return Err(format!("{} is not there any more", path.display()));
            }
            let Some(render) = RENDERS.with(|renders| renders.borrow().get(path).cloned()) else {
                return Err("the GIF's render ended before it began".to_owned());
            };
            render
        }
    };
    loop {
        if let Some(result) = render.done.borrow().clone() {
            return result;
        }
        glib::timeout_future(POLL).await;
    }
}

/// Stops the render writing `path`, if one is: its capture went in the bin.
pub fn cancel(path: &Path) {
    if let Some(progress) = progress_of(path) {
        progress.cancel();
    }
}

/// The whole reel at `dir`, written to `out` the way its `recording.json` says.
fn write(dir: &Path, out: &Path, progress: &Progress) -> Result<(), String> {
    let reel = Reel::open(dir).map_err(|e| e.to_string())?;
    let options = reel.recorded().gif;
    gif_edit::trim_reporting(dir, 0..reel.len(), options, out, progress)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
pub(crate) mod tests {
    use octosnap_media::gif_edit::TrimOptions;
    use octosnap_media::reel::Recorded;

    use super::*;

    /// A reel of `frames` small frames at `dir`, a tenth of a second each, laid out the way
    /// the recorder lays one out: what a recording leaves behind when it stops (D113).
    pub(crate) fn write_reel(dir: &Path, frames: u64) {
        let (width, height) = (8, 6);
        std::fs::create_dir_all(dir).expect("reel dir");
        for index in 0..frames {
            let name = format!("{index:06}-{:08}.png", index * 100);
            let file = std::fs::File::create(dir.join(name)).expect("frame file");
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let shade = u8::try_from(index * 60 % 256).expect("a shade");
            let pixels: Vec<u8> =
                (0..width * height).flat_map(|_| [shade, 128, 255 - shade, 255]).collect();
            let mut writer = encoder.write_header().expect("header");
            writer.write_image_data(&pixels).expect("pixels");
        }
        let recorded = Recorded {
            width,
            height,
            fps: 10,
            gif: TrimOptions { quality: 90, fps: None, scale_percent: None },
            end_ms: Some(frames * 100),
        };
        let json = serde_json::to_vec(&recorded).expect("json");
        std::fs::write(dir.join("recording.json"), json).expect("recording.json");
    }

    /// A fresh context per test, as `flow`'s tests have: cargo runs these in parallel.
    fn run<F: std::future::Future>(future: F) -> F::Output {
        glib::MainContext::new().block_on(future)
    }

    /// A Copy pressed while a Save is writing the same GIF joins that render: one gifski
    /// over the frames, one file, and no `.part` left behind.
    #[test]
    fn a_pending_gif_is_written_once_from_its_frames_whoever_asks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let gif = dir.path().join("01REC.gif");
        write_reel(&reel::beside(&gif), 3);
        assert!(pending(&gif));
        run(async {
            start(&gif).expect("a render starts");
            assert_eq!(rendering(), std::slice::from_ref(&gif));
            assert!(start(&gif).is_some(), "a second caller joins it");
            assert_eq!(rendering().len(), 1, "and does not start another");
            ensure(&gif).await.expect("the GIF is written");
        });
        assert!(gif.is_file());
        assert!(!pending(&gif), "once written, it is a GIF like any other");
        assert!(rendering().is_empty());
        assert!(!gif.with_extension("gif.part").exists());
        assert_eq!(gif_edit::scan(&gif).expect("a GIF").frames.len(), 3);
        assert!(reel::is_reel(&reel::beside(&gif)), "the frames stay, for the editor");
    }

    /// Nothing to write it from: a sentence for the user, not a wait that never ends.
    #[test]
    fn a_gif_with_no_frames_to_write_it_from_says_so() {
        let dir = tempfile::tempdir().expect("tempdir");
        let gif = dir.path().join("01GONE.gif");
        assert!(!pending(&gif));
        assert!(start(&gif).is_none());
        let said = run(ensure(&gif)).expect_err("nothing to write");
        assert!(said.contains("not there"), "{said}");
    }
}
