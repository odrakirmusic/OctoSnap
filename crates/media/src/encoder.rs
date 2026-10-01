// SPDX-License-Identifier: GPL-3.0-or-later
//! The GIF encoder: `gifski` (D68), streaming, on two threads of its own. The collector
//! takes frames off the graph's channel and hands them to gifski as they come; the writer
//! quantises, dithers and writes the file as frames are ready. Neither holds more than a
//! few frames, so a long recording costs no more memory than a short one.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::gif::GifPlan;
use crate::pipeline::Frame;

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("gifski refused the settings: {0}")]
    Settings(gifski::Error),
    #[error("the collector thread could not start: {0}")]
    Thread(#[from] std::io::Error),
    #[error("the encoder took no frame: {0}")]
    NoFrames(gifski::Error),
    #[error("the encoder failed: {0}")]
    Encode(gifski::Error),
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("an encoder thread panicked")]
    Panicked,
}

/// How far an encode has got, readable from the thread that is waiting for it.
///
/// A trim of a ten-second recording takes seconds -- gifski quantises every frame to its
/// own 256-colour palette and diffs it against the last -- and a window with three grey
/// buttons and a toast that has already faded is indistinguishable from a window that has
/// hung. This is what a progress bar reads.
///
/// Counted in **frames written**, from gifski's own reporter, rather than in frames fed
/// in: the feeding is the fast half, and a bar that filled while the reader ran and then
/// sat at 100 % through the encode would be a worse lie than no bar at all. `total` is set
/// by whoever knows it -- the caller planning the frames -- because gifski does not.
///
/// Clone is cheap and shares the counters: one half goes to the writer thread, the other
/// stays with the caller.
#[derive(Debug, Clone, Default)]
pub struct Progress(Arc<Counters>);

#[derive(Debug, Default)]
struct Counters {
    written: AtomicU32,
    total: AtomicU32,
    /// Set by [`Encoder::abort`]: gifski stops at its next frame instead of quantising
    /// the rest of a recording nobody is going to keep.
    cancelled: AtomicBool,
}

impl Progress {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many frames the encode will write, once the caller knows.
    pub fn expect(&self, frames: usize) {
        self.0.total.store(u32::try_from(frames).unwrap_or(u32::MAX), Ordering::Relaxed);
    }

    /// Frames written, and frames expected. A total of zero means "not yet known", which
    /// is the caller's cue to pulse rather than to draw a fraction.
    #[must_use]
    pub fn frames(&self) -> (u32, u32) {
        (self.0.written.load(Ordering::Relaxed), self.0.total.load(Ordering::Relaxed))
    }

    /// 0.0 to 1.0, or `None` while the total is unknown.
    #[must_use]
    pub fn fraction(&self) -> Option<f64> {
        let (written, total) = self.frames();
        (total > 0).then(|| (f64::from(written) / f64::from(total)).clamp(0.0, 1.0))
    }

    /// Asks the encode to stop at its next frame: the capture it was writing went in the
    /// bin. The encode then ends with an error, and writes nothing more.
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Relaxed);
    }
}

/// The half of [`Progress`] gifski writes through.
struct Reporter(Progress);

impl gifski::progress::ProgressReporter for Reporter {
    fn increase(&mut self) -> bool {
        self.0.0.written.fetch_add(1, Ordering::Relaxed);
        // `false` asks gifski to stop, which only a trashed recording wants: its frames
        // are going in the bin, and joining an encoder that is still quantising them
        // would hold whoever is waiting for as long as that takes.
        !self.0.0.cancelled.load(Ordering::Relaxed)
    }

    /// Nothing. gifski 1.34's library never calls this -- only its own CLI does -- so
    /// "finished" is the encoder thread returning, which the caller is already waiting on.
    fn done(&mut self, _message: &str) {}
}

/// What a finished recording amounts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub path: PathBuf,
    pub frames: u64,
    /// The GIF's own length: the last frame's time plus one frame.
    pub duration: Duration,
    pub bytes: u64,
}

#[derive(Debug)]
struct Collected {
    frames: u64,
    last_pts: f64,
}

/// A running encoder: two threads and the file they are writing.
#[derive(Debug)]
pub struct Encoder {
    path: PathBuf,
    fps: u32,
    /// The writer's own counters, kept so [`Self::abort`] can ask it to stop.
    progress: Progress,
    collector: Option<JoinHandle<Result<Collected, gifski::Error>>>,
    writer: Option<JoinHandle<Result<(), EncodeError>>>,
}

impl Encoder {
    /// Starts encoding `frames` into `path` with `plan`'s quality. The graph has already
    /// scaled every frame to `plan.output`, so gifski is asked for no resize.
    pub fn start(plan: &GifPlan, path: &Path, frames: Receiver<Frame>) -> Result<Self, EncodeError> {
        Self::start_with(plan.quality, plan.fps, path, frames, Progress::new())
    }

    /// [`Self::start`] without a plan: the frames are already the size they will be
    /// written at. This is how the GIF editor re-encodes a trimmed range (`gif_edit`),
    /// where there is no stream to plan a crop of. `fps` only sizes the last frame in
    /// [`Written::duration`].
    ///
    /// `progress` is filled in as gifski writes. A recording's own encode passes one
    /// nobody reads: it runs while the user is doing something else, and the pill has
    /// already told them it is happening.
    pub fn start_with(
        quality: u8,
        fps: u32,
        path: &Path,
        frames: Receiver<Frame>,
        progress: Progress,
    ) -> Result<Self, EncodeError> {
        let settings = gifski::Settings {
            // The frames arrive at the size they are to be written at -- the graph has
            // already scaled them to the width cap -- and gifski must leave them there.
            // `None` is not "as they come": gifski then shrinks any frame over about
            // 0.7 Mpx to fit 800 x 600, so a 1363 x 836 recording came out at 681 x 418
            // whatever width or quality was asked for (2026-09-24). The largest a GIF
            // can be, with no height, keeps every frame at its own size.
            width: Some(u32::from(u16::MAX)),
            height: None,
            quality,
            fast: false,
            repeat: gifski::Repeat::Infinite,
        };
        let (collector, writer) = gifski::new(settings).map_err(EncodeError::Settings)?;

        let file = std::fs::File::create(path)
            .map_err(|source| EncodeError::Write { path: path.to_path_buf(), source })?;
        let out = path.to_path_buf();
        let kept = progress.clone();
        let writer = std::thread::Builder::new().name("gif-write".into()).spawn(move || {
            let mut reporter = Reporter(progress);
            writer
                .write(std::io::BufWriter::new(file), &mut reporter)
                .map_err(|e| match e {
                    gifski::Error::NoFrames => EncodeError::NoFrames(e),
                    gifski::Error::Io(source) => EncodeError::Write { path: out, source },
                    other => EncodeError::Encode(other),
                })
        })?;
        let collector =
            std::thread::Builder::new().name("gif-collect".into()).spawn(move || {
                let mut collected = Collected { frames: 0, last_pts: 0.0 };
                for frame in frames {
                    collector.add_frame_rgba(frame.index, frame.image, frame.pts)?;
                    collected.frames += 1;
                    collected.last_pts = frame.pts;
                }
                // Dropping the collector is what tells the writer the sequence is over.
                drop(collector);
                Ok(collected)
            })?;

        Ok(Self {
            path: path.to_path_buf(),
            fps,
            progress: kept,
            collector: Some(collector),
            writer: Some(writer),
        })
    }

    /// Waits for both threads. The frame channel must be closed first (the graph does
    /// that on shutdown), or this never returns.
    pub fn finish(mut self) -> Result<Written, EncodeError> {
        let collected = self
            .collector
            .take()
            .ok_or(EncodeError::Panicked)?
            .join()
            .map_err(|_| EncodeError::Panicked)?
            .map_err(EncodeError::Encode)?;
        self.writer.take().ok_or(EncodeError::Panicked)?.join().map_err(|_| EncodeError::Panicked)??;
        let bytes = std::fs::metadata(&self.path)
            .map_err(|source| EncodeError::Write { path: self.path.clone(), source })?
            .len();
        let frame = 1.0 / f64::from(self.fps.max(1));
        Ok(Written {
            path: self.path.clone(),
            frames: collected.frames,
            duration: Duration::from_secs_f64(collected.last_pts + frame),
            bytes,
        })
    }

    /// Stops caring about the result and removes the file (`spec/06` §3 Trash). The
    /// channel must be closed first, as for [`Self::finish`]. gifski is told to stop at
    /// its next frame, so the join is as long as one frame takes, not the whole backlog.
    pub fn abort(mut self) {
        self.progress.0.cancelled.store(true, Ordering::Relaxed);
        if let Some(collector) = self.collector.take() {
            let _ = collector.join();
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        if let Err(e) = std::fs::remove_file(&self.path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %self.path.display(), "could not remove the trashed GIF: {e}");
        }
    }
}

impl Drop for Encoder {
    /// An encoder dropped without `finish` or `abort` -- the recorder failed between the
    /// two -- must not leave threads behind; the channel is closed by then, so the joins
    /// return.
    fn drop(&mut self) {
        if let Some(collector) = self.collector.take() {
            let _ = collector.join();
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::gif::{GifSettings, StreamSource};
    use imgref::ImgVec;
    use octosnap_core::geometry::Rect;
    use rgb::RGBA8;

    fn plan() -> GifPlan {
        let source = StreamSource {
            connector: "Meta-0".into(),
            rect: Rect::new(0, 0, 640, 400),
            scale: 1.0,
            refresh: None,
        };
        GifPlan::new(Rect::new(0, 0, 64, 40), &source, GifSettings { fps: 10, ..GifSettings::default() }).unwrap()
    }

    fn frame(index: usize, fps: u32, shade: u8) -> Frame {
        let pixels = vec![RGBA8::new(shade, 0, 255 - shade, 255); 64 * 40];
        Frame { index, pts: index as f64 / f64::from(fps), image: ImgVec::new(pixels, 64, 40) }
    }

    /// Three frames in, a GIF out, with the duration the frames add up to.
    #[test]
    fn frames_become_a_looping_gif() {
        let dir = std::env::temp_dir().join(format!("octosnap-encoder-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("three.gif");
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        let encoder = Encoder::start(&plan(), &path, rx).unwrap();
        for i in 0..3 {
            tx.send(frame(i, 10, (i * 100) as u8)).unwrap();
        }
        drop(tx);
        let written = encoder.finish().unwrap();
        assert_eq!(written.frames, 3);
        assert_eq!(written.duration, Duration::from_millis(300));
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..6], b"GIF89a", "a GIF89a header");
        assert_eq!(written.bytes, bytes.len() as u64);
        // NETSCAPE2.0 is the looping extension gifski writes for Repeat::Infinite.
        assert!(bytes.windows(11).any(|w| w == b"NETSCAPE2.0"), "the GIF loops");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A frame is written at the size it arrives at, however large: gifski's own default
    /// halved anything over about 0.7 Mpx, so a 1363 x 836 recording came out 681 x 418.
    #[test]
    fn a_large_frame_is_written_at_its_own_size() {
        let dir = std::env::temp_dir()
            .join(format!("octosnap-encoder-large-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("large.gif");
        let (tx, rx) = std::sync::mpsc::sync_channel(2);
        let encoder = Encoder::start_with(80, 10, &path, rx, Progress::new()).unwrap();
        let (width, height) = (1363, 836);
        for index in 0..2 {
            let shade = if index == 0 { 40 } else { 200 };
            let pixels = vec![RGBA8::new(shade, 90, 255 - shade, 255); width * height];
            let image = ImgVec::new(pixels, width, height);
            tx.send(Frame { index, pts: index as f64 / 10.0, image }).unwrap();
        }
        drop(tx);
        encoder.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        // The logical screen size, little-endian, right after the six-byte header.
        let size = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        assert_eq!((size(6), size(8)), (1363, 836));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Trash: the threads end and the file is gone.
    /// The bar has three states to tell apart: nothing known yet, running, and done.
    #[test]
    fn progress_pulses_until_it_knows_the_total_and_then_reads_a_fraction() {
        use gifski::progress::ProgressReporter;
        let progress = Progress::new();
        // Before the caller has planned the frames there is no fraction to draw.
        assert_eq!(progress.frames(), (0, 0));
        assert_eq!(progress.fraction(), None);

        progress.expect(4);
        assert_eq!(progress.fraction(), Some(0.0));

        let mut reporter = Reporter(progress.clone());
        reporter.increase();
        reporter.increase();
        assert_eq!(progress.frames(), (2, 4));
        assert_eq!(progress.fraction(), Some(0.5));

        // gifski may write more frames than the caller planned for; the bar must not.
        for _ in 0..6 {
            reporter.increase();
        }
        assert_eq!(progress.fraction(), Some(1.0));
    }

    #[test]
    fn abort_removes_the_file() {
        let dir = std::env::temp_dir().join(format!("octosnap-encoder-abort-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trashed.gif");
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        let encoder = Encoder::start(&plan(), &path, rx).unwrap();
        tx.send(frame(0, 10, 7)).unwrap();
        drop(tx);
        encoder.abort();
        assert!(!path.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// No frame at all is an error the recorder can name, not an empty file.
    #[test]
    fn no_frames_is_an_error() {
        let dir = std::env::temp_dir().join(format!("octosnap-encoder-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("empty.gif");
        let (tx, rx) = std::sync::mpsc::sync_channel::<Frame>(1);
        let encoder = Encoder::start(&plan(), &path, rx).unwrap();
        drop(tx);
        assert!(matches!(encoder.finish(), Err(EncodeError::NoFrames(_))));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
