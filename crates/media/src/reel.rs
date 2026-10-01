// SPDX-License-Identifier: GPL-3.0-or-later
//! A recording's frames, kept losslessly on disk until somebody asks for a GIF (D113).
//!
//! gifski writes a full-size frame five to ten times a second on the target machine, and a
//! recording at a 120 Hz screen's rate takes forty. Encoded while it was taken, a
//! ten-second recording filled the queue in front of gifski three and a half seconds in,
//! the sink dropped half its frames from there on, and Stop waited twenty seconds for
//! gifski to finish the rest (2026-09-24). So a recording is no longer encoded while it is
//! taken. Its frames are kept here, at the rate and the size they were taken at, and a GIF
//! is written from them when somebody asks for one -- by then with the trim, the frame
//! rate, the size and the quality they chose.
//!
//! **One PNG per frame that differs from the one before**, named by its place and its
//! time: `000017-00001250.png` is the eighteenth frame kept, and it shows from 1.250 s. A
//! frame identical to the one before is not written. The one before lasts longer instead,
//! as a GIF's own delay would say, so a still screen costs nothing. `recording.json` says
//! how the frames were taken and, once the recording has stopped, when it ended.
//!
//! PNG because it is lossless, and because the `png` crate's fast deflate keeps a full-size
//! screen frame in a few milliseconds at a tenth of its size and reads it back in about as
//! long. [P] on the target machine, a 1775 x 1025 frame of a web page is written in 5 ms as
//! 660 KB and read in 6 ms. Three workers write them, so a frame arriving every 25 ms is
//! never waited for.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::thread::JoinHandle;
use std::time::Duration;

use imgref::ImgVec;
use rgb::{ComponentBytes, RGBA8};
use serde::{Deserialize, Serialize};

use crate::gif_edit::{FrameTime, Timeline, TrimOptions};
use crate::pipeline::Frame;

/// What a reel's directory ends in: `<id>.reel`, beside the captures in the spool.
pub const EXTENSION: &str = "reel";
/// The file that says how the frames were taken.
const RECORDED: &str = "recording.json";
/// How many threads write frames.
///
/// [P] One keeps up with forty full-size frames a second on the target machine. Three
/// leave room for a page that compresses badly -- dithered or photographic content takes
/// three times as long -- and for a disk that stalls for a moment.
const WORKERS: usize = 3;
/// How many frames may wait for each worker. The graph's own channel is the budget that
/// matters; this only keeps a worker from waiting on the collector.
const QUEUED: usize = 2;

#[derive(Debug, thiserror::Error)]
pub enum ReelError {
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not write the frame {path}: {source}")]
    Encode {
        path: PathBuf,
        #[source]
        source: png::EncodingError,
    },
    #[error("could not read the frame {path}: {source}")]
    Decode {
        path: PathBuf,
        #[source]
        source: png::DecodingError,
    },
    #[error("{path} does not say how its frames were taken: {source}")]
    Recorded {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("the frame {path} is {found} and the recording is {expected}")]
    Size { path: PathBuf, found: String, expected: String },
    #[error("the recording has no frames")]
    Empty,
    #[error("frame {index} is not one of the {frames} the recording has")]
    Range { index: usize, frames: usize },
    #[error("a frame-writing thread panicked")]
    Panicked,
}

/// How a recording's frames were taken, and how its GIF is to be written:
/// `recording.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recorded {
    /// Every frame's size, in pixels.
    pub width: u32,
    pub height: u32,
    /// The rate they were taken at.
    pub fps: u32,
    /// What its GIF is written at unless somebody says otherwise: the quality the
    /// recording asked for, and for a reel the editor cut from another, the frame rate
    /// and the size the editor was set to.
    pub gif: TrimOptions,
    /// When the recording ended, in milliseconds from its first frame. Absent until it has
    /// stopped; a reel without it ends one frame after its last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<u64>,
}

impl Recorded {
    /// One frame at [`Self::fps`], in milliseconds, never zero.
    #[must_use]
    pub fn frame_ms(&self) -> u64 {
        (1000 / u64::from(self.fps.max(1))).max(1)
    }
}

/// What a finished recording amounts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    pub dir: PathBuf,
    /// Frames the graph handed over.
    pub taken: u64,
    /// Frames written: the ones that differed from the frame before.
    pub kept: u64,
    /// The recording's length: its last frame's time plus one frame.
    pub duration: Duration,
    /// What the frames take on disk.
    pub bytes: u64,
}

/// A frame for a worker to write.
struct Job {
    path: PathBuf,
    image: Arc<ImgVec<RGBA8>>,
}

/// A worker's queue, and the thread that says how many bytes it wrote.
type Worker = (SyncSender<Job>, JoinHandle<Result<u64, ReelError>>);

/// A recording's frames being written as they come, on threads of their own.
#[derive(Debug)]
pub struct Writer {
    dir: PathBuf,
    /// Set by [`Self::abort`]: the collector stops at its next frame.
    cancelled: Arc<AtomicBool>,
    collector: Option<JoinHandle<Result<Kept, ReelError>>>,
}

impl Writer {
    /// Starts keeping `frames` in `dir`, which is made, with `recorded` said in it first
    /// -- so the frames of a recording the app never stopped can still be read.
    pub fn start(
        dir: &Path,
        recorded: Recorded,
        frames: Receiver<Frame>,
    ) -> Result<Self, ReelError> {
        std::fs::create_dir_all(dir)
            .map_err(|source| ReelError::Write { path: dir.to_path_buf(), source })?;
        write_recorded(dir, &recorded)?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancelled);
        let at = dir.to_path_buf();
        let collector = std::thread::Builder::new()
            .name("reel-collect".into())
            .spawn(move || collect(&at, recorded, &frames, &stop))
            .map_err(|source| ReelError::Write { path: dir.to_path_buf(), source })?;
        Ok(Self { dir: dir.to_path_buf(), cancelled, collector: Some(collector) })
    }

    /// Waits for every frame to be written and says when the recording ended. The frame
    /// channel must be closed first (the graph does that on shutdown), or this never
    /// returns.
    pub fn finish(mut self) -> Result<Kept, ReelError> {
        self.collector.take().ok_or(ReelError::Panicked)?.join().map_err(|_| ReelError::Panicked)?
    }

    /// Stops keeping frames and removes the directory (`spec/06` §3 Trash). The channel
    /// must be closed first, as for [`Self::finish`].
    pub fn abort(mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Some(collector) = self.collector.take() {
            let _ = collector.join();
        }
        remove(&self.dir);
    }

    /// The directory being written.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Writer {
    /// A writer dropped without `finish` or `abort` -- the recorder failed between the two
    /// -- must not leave threads behind; the channel is closed by then, so the join returns.
    fn drop(&mut self) {
        if let Some(collector) = self.collector.take() {
            let _ = collector.join();
        }
    }
}

/// The collector: takes frames off the graph's channel, leaves out the ones that repeat
/// the frame before, and hands the rest to the workers in turn.
fn collect(
    dir: &Path,
    mut recorded: Recorded,
    frames: &Receiver<Frame>,
    cancelled: &AtomicBool,
) -> Result<Kept, ReelError> {
    let mut workers: Vec<Worker> = Vec::with_capacity(WORKERS);
    for n in 0..WORKERS {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Job>(QUEUED);
        let worker = std::thread::Builder::new()
            .name(format!("reel-write-{n}"))
            .spawn(move || write_frames(&rx))
            .map_err(|source| ReelError::Write { path: dir.to_path_buf(), source })?;
        workers.push((tx, worker));
    }
    let (mut taken, mut kept, mut last_pts) = (0u64, 0u64, 0.0f64);
    let mut previous: Option<Arc<ImgVec<RGBA8>>> = None;
    for frame in frames {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        taken += 1;
        last_pts = frame.pts;
        // A still screen is most of a recording: `videorate` repeats the last frame to
        // keep its rate, and the compositor sends one every keepalive besides. Comparing
        // costs a fraction of a millisecond; writing it again would cost a PNG.
        if previous.as_ref().is_some_and(|before| before.buf() == frame.image.buf()) {
            continue;
        }
        let image = Arc::new(frame.image);
        let job = Job { path: dir.join(name(kept, ms(frame.pts))), image: Arc::clone(&image) };
        // A worker whose queue is full is waited for, here on the collector's own thread:
        // the graph's channel in front of it is the budget, and the sink never waits. A
        // worker that has gone failed, and says why when it is joined below.
        let worker = usize::try_from(kept).unwrap_or(0) % WORKERS;
        if workers[worker].0.send(job).is_err() {
            break;
        }
        previous = Some(image);
        kept += 1;
    }
    let (senders, handles): (Vec<_>, Vec<_>) = workers.into_iter().unzip();
    // Closing their queues is what tells the workers the recording is over.
    drop(senders);
    let mut bytes = 0;
    let mut failed = None;
    for handle in handles {
        match handle.join() {
            Ok(Ok(written)) => bytes += written,
            Ok(Err(e)) => {
                failed.get_or_insert(e);
            }
            Err(_) => {
                failed.get_or_insert(ReelError::Panicked);
            }
        }
    }
    if let Some(e) = failed {
        return Err(e);
    }
    if taken == 0 || cancelled.load(Ordering::Relaxed) {
        remove(dir);
        return Err(ReelError::Empty);
    }
    let end_ms = ms(last_pts) + recorded.frame_ms();
    recorded.end_ms = Some(end_ms);
    write_recorded(dir, &recorded)?;
    Ok(Kept { dir: dir.to_path_buf(), taken, kept, duration: Duration::from_millis(end_ms), bytes })
}

/// A worker: writes each frame it is handed as a PNG, and says how many bytes that took.
fn write_frames(jobs: &Receiver<Job>) -> Result<u64, ReelError> {
    let mut bytes = 0;
    for job in jobs {
        bytes += write_png(&job.path, &job.image)?;
    }
    Ok(bytes)
}

/// One frame as a PNG: lossless, the fast deflate, and the Up filter, which reads back the
/// fastest of the five and within a tenth of the best on screen content ([P] 2026-09-24).
fn write_png(path: &Path, image: &ImgVec<RGBA8>) -> Result<u64, ReelError> {
    let encode = |source| ReelError::Encode { path: path.to_path_buf(), source };
    let (width, height) = (u32::try_from(image.width()), u32::try_from(image.height()));
    let (Ok(width), Ok(height)) = (width, height) else {
        return Err(ReelError::Size {
            path: path.to_path_buf(),
            found: format!("{} x {}", image.width(), image.height()),
            expected: "at most 4294967295 wide".into(),
        });
    };
    let mut bytes = Vec::with_capacity(image.buf().len());
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        encoder.set_filter(png::Filter::Up);
        let mut writer = encoder.write_header().map_err(encode)?;
        // The graph's frames have no padding between rows: its copy out of the buffer
        // already dropped the stride.
        writer.write_image_data(image.buf().as_bytes()).map_err(encode)?;
        writer.finish().map_err(encode)?;
    }
    std::fs::write(path, &bytes)
        .map_err(|source| ReelError::Write { path: path.to_path_buf(), source })?;
    Ok(bytes.len() as u64)
}

fn write_recorded(dir: &Path, recorded: &Recorded) -> Result<(), ReelError> {
    let path = dir.join(RECORDED);
    let text = serde_json::to_vec_pretty(recorded)
        .map_err(|source| ReelError::Recorded { path: path.clone(), source })?;
    std::fs::write(&path, text).map_err(|source| ReelError::Write { path, source })
}

/// A frame's file: its place among the frames kept, and when it shows.
fn name(index: u64, ms: u64) -> String {
    format!("{index:06}-{ms:08}.png")
}

/// The place and the time a frame's file says, or `None` for anything else in the folder.
fn parse(name: &str) -> Option<(u64, u64)> {
    let stem = name.strip_suffix(".png")?;
    let (index, ms) = stem.split_once('-')?;
    Some((index.parse().ok()?, ms.parse().ok()?))
}

/// Seconds as whole milliseconds.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn ms(seconds: f64) -> u64 {
    (seconds.max(0.0) * 1000.0).round() as u64
}

/// Removes a reel's directory, and says so in the log if it could not.
pub fn remove(dir: &Path) {
    if let Err(e) = std::fs::remove_dir_all(dir)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(dir = %dir.display(), "could not remove a recording's frames: {e}");
    }
}

/// Whether `path` is a reel: a directory that says how its frames were taken.
#[must_use]
pub fn is_reel(path: &Path) -> bool {
    path.join(RECORDED).is_file()
}

/// The reel a recording's GIF is written from: `<id>.reel` beside `<id>.gif`, whether or
/// not either exists yet.
#[must_use]
pub fn beside(gif: &Path) -> PathBuf {
    gif.with_extension(EXTENSION)
}

/// Where a recording's frames are read from: its reel when it has one -- lossless, and at
/// the rate it was taken -- and its GIF when it does not.
#[must_use]
pub fn source_of(gif: &Path) -> PathBuf {
    let reel = beside(gif);
    if is_reel(&reel) { reel } else { gif.to_path_buf() }
}

/// A reel of frames `range` of `from`, at `to`, whose GIF is written with `gif`: what the
/// GIF editor leaves on a card when it closes on a trim (D108), without writing the GIF.
///
/// Each frame is a hard link to the one it was cut from, so a cut costs a directory and a
/// name per frame, however long the recording: a close that waited for gifski would wait a
/// minute on a full-size one. The times start again from nought.
pub fn cut(
    from: &Reel,
    range: Range<usize>,
    gif: TrimOptions,
    to: &Path,
) -> Result<Reel, ReelError> {
    let frames = from.timeline.frames.get(range.clone()).filter(|f| !f.is_empty());
    let Some(frames) = frames else {
        return Err(ReelError::Range { index: range.end, frames: from.len() });
    };
    let first = frames[0].start_ms;
    let end = frames.last().map_or(first, |f| f.start_ms + f.delay_ms) - first;
    std::fs::create_dir_all(to)
        .map_err(|source| ReelError::Write { path: to.to_path_buf(), source })?;
    for (index, (time, file)) in frames.iter().zip(&from.files[range]).enumerate() {
        let link = to.join(name(index as u64, time.start_ms - first));
        if let Err(e) = std::fs::hard_link(file, &link) {
            // Another file system, or one without hard links: the frame is copied instead.
            tracing::debug!(from = %file.display(), "a cut frame is copied, not linked: {e}");
            std::fs::copy(file, &link)
                .map_err(|source| ReelError::Write { path: link.clone(), source })?;
        }
    }
    let recorded = Recorded { gif, end_ms: Some(end), ..from.recorded };
    write_recorded(to, &recorded)?;
    Reel::open(to)
}

/// A reel, read back: how its frames were taken, when each shows, and each one's pixels.
#[derive(Debug, Clone)]
pub struct Reel {
    dir: PathBuf,
    recorded: Recorded,
    files: Vec<PathBuf>,
    timeline: Timeline,
}

impl Reel {
    /// Reads `recording.json` and the names of the frames. No frame is decoded.
    pub fn open(dir: &Path) -> Result<Self, ReelError> {
        let path = dir.join(RECORDED);
        let text = std::fs::read(&path)
            .map_err(|source| ReelError::Read { path: path.clone(), source })?;
        let recorded: Recorded = serde_json::from_slice(&text)
            .map_err(|source| ReelError::Recorded { path: path.clone(), source })?;
        let entries = std::fs::read_dir(dir)
            .map_err(|source| ReelError::Read { path: dir.to_path_buf(), source })?;
        let mut found: Vec<(u64, u64, PathBuf)> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let (index, ms) = parse(entry.file_name().to_str()?)?;
                Some((index, ms, entry.path()))
            })
            .collect();
        found.sort_unstable_by_key(|&(index, _, _)| index);
        if found.is_empty() {
            return Err(ReelError::Empty);
        }
        let end = recorded
            .end_ms
            .unwrap_or_else(|| found.last().map_or(0, |&(_, ms, _)| ms) + recorded.frame_ms());
        let starts: Vec<u64> = found.iter().map(|&(_, ms, _)| ms).collect();
        let frames = starts
            .iter()
            .enumerate()
            .map(|(i, &start_ms)| {
                let next = starts.get(i + 1).copied().unwrap_or(end);
                FrameTime { start_ms, delay_ms: next.saturating_sub(start_ms).max(1) }
            })
            .collect();
        let timeline = Timeline {
            width: recorded.width,
            height: recorded.height,
            frames,
            fps: Some(recorded.fps),
        };
        let files = found.into_iter().map(|(_, _, path)| path).collect();
        Ok(Self { dir: dir.to_path_buf(), recorded, files, timeline })
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    #[must_use]
    pub fn recorded(&self) -> Recorded {
        self.recorded
    }

    #[must_use]
    pub fn timeline(&self) -> &Timeline {
        &self.timeline
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The file frame `index` is kept in: a PNG anything can show, which is what a card's
    /// or the history's thumbnail is made from.
    #[must_use]
    pub fn file(&self, index: usize) -> Option<&Path> {
        self.files.get(index).map(PathBuf::as_path)
    }

    /// Decodes frame `index` into `canvas`, which is resized to fit it.
    pub fn read(&self, index: usize, canvas: &mut Vec<RGBA8>) -> Result<(), ReelError> {
        let path = self
            .files
            .get(index)
            .ok_or(ReelError::Range { index, frames: self.files.len() })?;
        let decode = |source| ReelError::Decode { path: path.clone(), source };
        let file = std::fs::File::open(path)
            .map_err(|source| ReelError::Read { path: path.clone(), source })?;
        let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
        let mut reader = decoder.read_info().map_err(decode)?;
        let (width, height) = (reader.info().width, reader.info().height);
        if (width, height) != (self.recorded.width, self.recorded.height) {
            return Err(ReelError::Size {
                path: path.clone(),
                found: format!("{width} x {height}"),
                expected: format!("{} x {}", self.recorded.width, self.recorded.height),
            });
        }
        canvas.resize(width as usize * height as usize, RGBA8::new(0, 0, 0, 0));
        reader.next_frame(canvas.as_mut_slice().as_bytes_mut()).map_err(decode)?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const W: usize = 40;
    const H: usize = 24;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("octosnap-reel-{}-{name}", std::process::id()))
            .with_extension(EXTENSION);
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn recorded(fps: u32) -> Recorded {
        let gif = TrimOptions { quality: 90, ..TrimOptions::default() };
        Recorded { width: W as u32, height: H as u32, fps, gif, end_ms: None }
    }

    fn frame(index: usize, fps: u32, shade: u8) -> Frame {
        let pixels = (0..W * H)
            .map(|i| RGBA8::new(shade, (i % 256) as u8, 255 - shade, 255))
            .collect();
        Frame { index, pts: index as f64 / f64::from(fps), image: ImgVec::new(pixels, W, H) }
    }

    /// Frames go in, and come back out exactly, each at the time it was taken.
    #[test]
    fn frames_come_back_as_they_went_in() {
        let dir = scratch("roundtrip");
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let writer = Writer::start(&dir, recorded(10), rx).unwrap();
        let shades = [10, 60, 110, 160, 210];
        for (i, &shade) in shades.iter().enumerate() {
            tx.send(frame(i, 10, shade)).unwrap();
        }
        drop(tx);
        let kept = writer.finish().unwrap();
        assert_eq!((kept.taken, kept.kept), (5, 5));
        assert_eq!(kept.duration, Duration::from_millis(500));
        assert!(kept.bytes > 0);

        let reel = Reel::open(&dir).unwrap();
        assert_eq!(reel.len(), 5);
        assert_eq!(reel.recorded().end_ms, Some(500));
        let starts: Vec<u64> = reel.timeline().frames.iter().map(|f| f.start_ms).collect();
        assert_eq!(starts, [0, 100, 200, 300, 400]);
        assert_eq!(reel.timeline().duration_ms(), 500);
        let mut canvas = Vec::new();
        for (i, &shade) in shades.iter().enumerate() {
            reel.read(i, &mut canvas).unwrap();
            assert_eq!(canvas, frame(i, 10, shade).image.into_buf(), "frame {i}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A still screen costs nothing: a frame that repeats the one before is not written,
    /// and the one before lasts until the next that differs.
    #[test]
    fn a_frame_that_repeats_the_one_before_lengthens_it() {
        let dir = scratch("still");
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let writer = Writer::start(&dir, recorded(10), rx).unwrap();
        for (i, shade) in [10, 10, 10, 90, 90, 10].into_iter().enumerate() {
            tx.send(frame(i, 10, shade)).unwrap();
        }
        drop(tx);
        let kept = writer.finish().unwrap();
        assert_eq!((kept.taken, kept.kept), (6, 3));
        let reel = Reel::open(&dir).unwrap();
        let times: Vec<(u64, u64)> =
            reel.timeline().frames.iter().map(|f| (f.start_ms, f.delay_ms)).collect();
        assert_eq!(times, [(0, 300), (300, 200), (500, 100)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The frames of a recording that never stopped can still be read: it ends one frame
    /// after its last.
    #[test]
    fn a_reel_nobody_finished_ends_one_frame_after_its_last() {
        let dir = scratch("unfinished");
        std::fs::create_dir_all(&dir).unwrap();
        write_recorded(&dir, &recorded(25)).unwrap();
        write_png(&dir.join(name(0, 0)), &frame(0, 25, 1).image).unwrap();
        write_png(&dir.join(name(1, 40)), &frame(1, 25, 2).image).unwrap();
        std::fs::write(dir.join("notes.txt"), "not a frame").unwrap();
        let reel = Reel::open(&dir).unwrap();
        let times: Vec<(u64, u64)> =
            reel.timeline().frames.iter().map(|f| (f.start_ms, f.delay_ms)).collect();
        assert_eq!(times, [(0, 40), (40, 40)]);
        assert_eq!(reel.timeline().fps, Some(25));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Trash: the threads end and the frames are gone.
    #[test]
    fn abort_removes_the_frames() {
        let dir = scratch("trashed");
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let writer = Writer::start(&dir, recorded(10), rx).unwrap();
        tx.send(frame(0, 10, 7)).unwrap();
        drop(tx);
        writer.abort();
        assert!(!dir.exists());
    }

    /// No frame at all is an error the recorder can name, not an empty reel left behind.
    #[test]
    fn no_frames_is_an_error() {
        let dir = scratch("empty");
        let (tx, rx) = std::sync::mpsc::sync_channel::<Frame>(1);
        let writer = Writer::start(&dir, recorded(10), rx).unwrap();
        drop(tx);
        assert!(matches!(writer.finish(), Err(ReelError::Empty)));
        assert!(!dir.exists(), "nothing is left of a recording with no frames");
    }

    /// A cut is the range's own frames, linked rather than written again, starting from
    /// nought, and saying how its GIF is to be written.
    #[test]
    fn a_cut_is_the_range_starting_again_from_nought() {
        let dir = scratch("uncut");
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let writer = Writer::start(&dir, recorded(10), rx).unwrap();
        for i in 0..6 {
            tx.send(frame(i, 10, (i * 40) as u8)).unwrap();
        }
        drop(tx);
        writer.finish().unwrap();
        let reel = Reel::open(&dir).unwrap();
        let to = scratch("cut");
        let gif = TrimOptions { quality: 70, fps: Some(5), scale_percent: Some(50) };
        let cut = cut(&reel, 2..5, gif, &to).unwrap();
        let times: Vec<(u64, u64)> =
            cut.timeline().frames.iter().map(|f| (f.start_ms, f.delay_ms)).collect();
        assert_eq!(times, [(0, 100), (100, 100), (200, 100)]);
        assert_eq!(cut.recorded().gif, gif);
        assert_eq!(cut.recorded().end_ms, Some(300));
        let (mut canvas, mut original) = (Vec::new(), Vec::new());
        cut.read(0, &mut canvas).unwrap();
        reel.read(2, &mut original).unwrap();
        assert_eq!(canvas, original, "the cut's first frame is the range's first");
        let empty = super::cut(&reel, 4..4, gif, &scratch("none"));
        assert!(matches!(empty, Err(ReelError::Range { .. })));
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&to).unwrap();
    }

    #[test]
    fn a_recording_is_read_from_its_reel_when_it_has_one() {
        let dir = scratch("beside");
        let gif = dir.with_extension("gif");
        assert_eq!(source_of(&gif), gif, "no reel: the GIF");
        std::fs::create_dir_all(&dir).unwrap();
        write_recorded(&dir, &recorded(10)).unwrap();
        assert_eq!(beside(&gif), dir);
        assert_eq!(source_of(&gif), dir, "a reel: the reel");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_frame_file_names_its_place_and_its_time() {
        assert_eq!(name(17, 1250), "000017-00001250.png");
        assert_eq!(parse("000017-00001250.png"), Some((17, 1250)));
        assert_eq!(parse("recording.json"), None);
        assert_eq!(parse("000017.png"), None);
    }
}
