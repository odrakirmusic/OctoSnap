// SPDX-License-Identifier: GPL-3.0-or-later
//! Reading a GIF back, and writing a shorter one: the pure half of the GIF editor
//! (`spec/04` §1's Trim on a recording card, M5).
//!
//! Two readers, on purpose. [`scan`] walks the file for its *timing* only -- when each
//! frame shows and for how long -- which is what a trim UI needs to draw a playhead and
//! two marks, and is cheap enough to do on open. [`Frames`] composes the frames the way
//! a viewer does, one canvas at a time, streaming: a GIF written by gifski is mostly
//! frame *differences* over the last canvas, so a frame on its own is not a picture,
//! and holding every composed frame of a 800-pixel, 15 fps recording would cost
//! ~1.4 MB a frame. The preview steps this reader; [`trim`] streams it into the same
//! gifski encoder the recorder uses.
//!
//! The `gif` crate rather than gdk-pixbuf's animation iterator, because the iterator
//! loops forever without saying where the sequence ends, and an editor has to know how
//! many frames there are.

use std::fs::File;
use std::io::BufReader;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use imgref::ImgVec;
use rgb::{ComponentBytes, RGBA8};

use crate::encoder::{EncodeError, Encoder, Progress, Written};
use crate::pipeline::Frame;
use crate::reel::{self, Reel, ReelError};

#[derive(Debug, thiserror::Error)]
pub enum GifEditError {
    #[error("could not open {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not a GIF this build can read: {source}")]
    Decode {
        path: PathBuf,
        #[source]
        source: gif::DecodingError,
    },
    #[error("frames {start}..{end} are not inside the {frames} the GIF has")]
    Range { start: usize, end: usize, frames: usize },
    #[error("the range holds no frames")]
    Empty,
    #[error(transparent)]
    Encode(#[from] EncodeError),
    #[error(transparent)]
    Reel(#[from] ReelError),
}

/// When one frame shows and for how long, in milliseconds from the first frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameTime {
    pub start_ms: u64,
    pub delay_ms: u64,
}

/// The GIF's frames in time, without their pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    /// The canvas, in pixels.
    pub width: u32,
    pub height: u32,
    pub frames: Vec<FrameTime>,
    /// The rate the frames were taken at, when the source says: a reel does (D113), a GIF
    /// does not, and [`Self::rate`] reads one off its delays.
    pub fps: Option<u32>,
}

impl Timeline {
    /// The rate the frames play at: the rate they were taken at, or for a GIF the rate its
    /// delays keep while something moves.
    ///
    /// Not the commonest delay. A GIF says hundredths of a second, so forty frames a second
    /// is delays of two and three hundredths in turn, and whichever of the two was commoner
    /// by one frame named the rate: a 40 fps recording's editor said "33 fps" (2026-09-24).
    /// Not the mean of every delay either, because a stretch where nothing moved is one
    /// long frame. So the mean of the delays no longer than half as long again as the
    /// shortest: the frames of the stretches where something moved.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    pub fn rate(&self) -> u32 {
        if let Some(fps) = self.fps {
            return fps.max(1);
        }
        let Some(shortest) = self.frames.iter().map(|f| f.delay_ms.max(1)).min() else {
            return 15;
        };
        let moving: Vec<u64> = self
            .frames
            .iter()
            .map(|f| f.delay_ms.max(1))
            .filter(|&delay| delay * 2 <= shortest * 3)
            .collect();
        let mean = moving.iter().sum::<u64>() as f64 / moving.len().max(1) as f64;
        ((1000.0 / mean).round() as u32).clamp(1, 100)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// The whole GIF's length: the last frame's start plus its delay.
    #[must_use]
    pub fn duration_ms(&self) -> u64 {
        self.frames.last().map_or(0, |f| f.start_ms + f.delay_ms)
    }

    /// How long `range` plays for. Out-of-range indices are ignored rather than counted.
    #[must_use]
    pub fn duration_of(&self, range: Range<usize>) -> u64 {
        self.frames.get(range).map_or(0, |frames| frames.iter().map(|f| f.delay_ms).sum())
    }

    /// The frame showing at `ms` from the start; the last frame past the end.
    #[must_use]
    pub fn frame_at_ms(&self, ms: u64) -> usize {
        match self.frames.iter().position(|f| ms < f.start_ms + f.delay_ms) {
            Some(index) => index,
            None => self.frames.len().saturating_sub(1),
        }
    }
}

/// GIF delays are centiseconds. A zero means "as fast as you can", which every browser
/// shows as 100 ms, so the timeline says what a viewer will actually do.
fn delay_ms(centiseconds: u16) -> u64 {
    if centiseconds == 0 { 100 } else { u64::from(centiseconds) * 10 }
}

fn open_decoder(path: &Path) -> Result<gif::Decoder<BufReader<File>>, GifEditError> {
    let file = File::open(path).map_err(|source| GifEditError::Open { path: path.to_path_buf(), source })?;
    let mut options = gif::DecodeOptions::new();
    // RGBA out, so a transparent index is an alpha of zero and the compositor never has
    // to look a palette up.
    options.set_color_output(gif::ColorOutput::RGBA);
    options
        .read_info(BufReader::new(file))
        .map_err(|source| GifEditError::Decode { path: path.to_path_buf(), source })
}

/// The timing of every frame, read without composing any of them: a GIF's delays, or the
/// times a reel's frames were taken at.
pub fn scan(path: &Path) -> Result<Timeline, GifEditError> {
    if reel::is_reel(path) {
        return Ok(Reel::open(path)?.timeline().clone());
    }
    let mut decoder = open_decoder(path)?;
    let width = u32::from(decoder.width());
    let height = u32::from(decoder.height());
    let mut frames = Vec::new();
    let mut start_ms = 0;
    loop {
        let frame = match decoder.read_next_frame() {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(source) => return Err(GifEditError::Decode { path: path.to_path_buf(), source }),
        };
        let delay = delay_ms(frame.delay);
        frames.push(FrameTime { start_ms, delay_ms: delay });
        start_ms += delay;
    }
    Ok(Timeline { width, height, frames, fps: None })
}

/// Where a frame's pixels land on the canvas, clipped to it.
#[derive(Debug, Clone, Copy)]
struct Area {
    left: usize,
    top: usize,
    width: usize,
    height: usize,
}

/// One composed frame: its place in the sequence. The pixels are [`Frames::pixels`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Composed {
    pub index: usize,
    pub start_ms: u64,
    pub delay_ms: u64,
}

/// A GIF's frames as a viewer composes them, one canvas at a time.
///
/// Each frame is drawn over the canvas the previous one left, and the previous frame's
/// **disposal** is applied first: nothing, the frame's area cleared, or the canvas put
/// back to what it was before that frame. That is the GIF89a rule every viewer follows
/// and the one a frame-difference GIF depends on.
struct GifFrames {
    path: PathBuf,
    decoder: gif::Decoder<BufReader<File>>,
    width: usize,
    height: usize,
    canvas: Vec<RGBA8>,
    /// The canvas before the current frame drew, for `DisposalMethod::Previous`.
    previous: Option<Vec<RGBA8>>,
    /// The current frame's disposal, applied before the next frame draws.
    pending: Option<(gif::DisposalMethod, Area)>,
    /// How many frames have been composed; also the index of the next one.
    produced: usize,
    next_start_ms: u64,
}

impl std::fmt::Debug for GifFrames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GifFrames")
            .field("path", &self.path)
            .field("size", &(self.width, self.height))
            .field("produced", &self.produced)
            .finish_non_exhaustive()
    }
}

impl GifFrames {
    fn open(path: &Path) -> Result<Self, GifEditError> {
        let decoder = open_decoder(path)?;
        let width = usize::from(decoder.width());
        let height = usize::from(decoder.height());
        Ok(Self {
            path: path.to_path_buf(),
            decoder,
            width,
            height,
            canvas: vec![RGBA8::new(0, 0, 0, 0); width * height],
            previous: None,
            pending: None,
            produced: 0,
            next_start_ms: 0,
        })
    }

    #[must_use]
    fn width(&self) -> u32 {
        self.width as u32
    }

    #[must_use]
    fn height(&self) -> u32 {
        self.height as u32
    }

    /// Frames composed so far, which is the index the next one will have.
    #[must_use]
    fn produced(&self) -> usize {
        self.produced
    }

    /// The canvas as the last composed frame left it.
    #[must_use]
    fn pixels(&self) -> &[RGBA8] {
        &self.canvas
    }

    /// Composes the next frame. `None` once the sequence is over; the canvas then still
    /// shows the last frame.
    fn next_frame(&mut self) -> Result<Option<Composed>, GifEditError> {
        // The previous frame's disposal, now that it has been seen.
        if let Some((dispose, area)) = self.pending.take() {
            match dispose {
                gif::DisposalMethod::Background => self.clear(area),
                gif::DisposalMethod::Previous => {
                    if let Some(previous) = self.previous.take() {
                        self.canvas = previous;
                    }
                }
                gif::DisposalMethod::Any | gif::DisposalMethod::Keep => {}
            }
        }
        self.previous = None;

        let frame = match self.decoder.read_next_frame() {
            Ok(Some(frame)) => frame,
            Ok(None) => return Ok(None),
            Err(source) => return Err(GifEditError::Decode { path: self.path.clone(), source }),
        };
        let area = Area {
            left: usize::from(frame.left).min(self.width),
            top: usize::from(frame.top).min(self.height),
            width: usize::from(frame.width).min(self.width.saturating_sub(usize::from(frame.left))),
            height: usize::from(frame.height).min(self.height.saturating_sub(usize::from(frame.top))),
        };
        let dispose = frame.dispose;
        if dispose == gif::DisposalMethod::Previous {
            self.previous = Some(self.canvas.clone());
        }

        // Draw: four bytes a pixel, the frame's own width as its stride; a transparent
        // pixel leaves the canvas alone, which is how a difference frame works.
        let stride = usize::from(frame.width) * 4;
        for y in 0..area.height {
            let row = &frame.buffer[y * stride..y * stride + area.width * 4];
            let dst = (area.top + y) * self.width + area.left;
            for (x, px) in row.as_chunks::<4>().0.iter().enumerate() {
                if px[3] != 0 {
                    self.canvas[dst + x] = RGBA8::new(px[0], px[1], px[2], px[3]);
                }
            }
        }

        let composed = Composed {
            index: self.produced,
            start_ms: self.next_start_ms,
            delay_ms: delay_ms(frame.delay),
        };
        self.pending = Some((dispose, area));
        self.produced += 1;
        self.next_start_ms += composed.delay_ms;
        Ok(Some(composed))
    }

    /// Composes frames from the start up to and including `index`, so the canvas shows
    /// that frame. `None` when the GIF has fewer frames. Every seek re-reads from the
    /// first frame -- there is no other way to compose frame `n` of a difference GIF --
    /// which is cheap for the small differences a screen recording is made of.
    fn seek(&mut self, index: usize) -> Result<Option<Composed>, GifEditError> {
        if index < self.produced {
            self.rewind()?;
        }
        while self.produced < index {
            if self.next_frame()?.is_none() {
                return Ok(None);
            }
        }
        self.next_frame()
    }

    fn rewind(&mut self) -> Result<(), GifEditError> {
        self.decoder = open_decoder(&self.path)?;
        self.canvas.fill(RGBA8::new(0, 0, 0, 0));
        self.previous = None;
        self.pending = None;
        self.produced = 0;
        self.next_start_ms = 0;
        Ok(())
    }

    fn clear(&mut self, area: Area) {
        for y in 0..area.height {
            let start = (area.top + y) * self.width + area.left;
            self.canvas[start..start + area.width].fill(RGBA8::new(0, 0, 0, 0));
        }
    }
}

/// A recording's frames, one at a time, from either kind of source: a GIF, composed the
/// way a viewer does, or a reel of frames kept losslessly (D113), read one PNG at a time.
///
/// One type for both because everything that shows or writes frames -- the editor's
/// preview and filmstrip, [`trim`], the history's thumbnail -- asks the same of either:
/// the size, the frame at an index or the next one, and its pixels.
pub struct Frames {
    source: Source,
}

enum Source {
    /// Boxed: a GIF decoder is most of a kilobyte, and a reel is a list of names.
    Gif(Box<GifFrames>),
    Reel(ReelFrames),
}

/// A reel's frames. Each is a PNG of its own, so any of them is read without the ones
/// before it: a step back costs what a step forward does.
struct ReelFrames {
    reel: Reel,
    canvas: Vec<RGBA8>,
    produced: usize,
}

impl ReelFrames {
    fn read(&mut self, index: usize) -> Result<Option<Composed>, GifEditError> {
        let Some(time) = self.reel.timeline().frames.get(index).copied() else {
            return Ok(None);
        };
        self.reel.read(index, &mut self.canvas)?;
        self.produced = index + 1;
        Ok(Some(Composed { index, start_ms: time.start_ms, delay_ms: time.delay_ms }))
    }
}

impl std::fmt::Debug for Frames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.source {
            Source::Gif(frames) => frames.fmt(f),
            Source::Reel(frames) => f
                .debug_struct("ReelFrames")
                .field("dir", &frames.reel.dir())
                .field("produced", &frames.produced)
                .finish_non_exhaustive(),
        }
    }
}

impl Frames {
    /// Opens `path`: a reel's directory ([`reel::is_reel`]), or a GIF.
    pub fn open(path: &Path) -> Result<Self, GifEditError> {
        let source = if reel::is_reel(path) {
            let reel = Reel::open(path)?;
            let recorded = reel.recorded();
            let canvas =
                vec![RGBA8::new(0, 0, 0, 0); recorded.width as usize * recorded.height as usize];
            Source::Reel(ReelFrames { reel, canvas, produced: 0 })
        } else {
            Source::Gif(Box::new(GifFrames::open(path)?))
        };
        Ok(Self { source })
    }

    #[must_use]
    pub fn width(&self) -> u32 {
        match &self.source {
            Source::Gif(frames) => frames.width(),
            Source::Reel(frames) => frames.reel.recorded().width,
        }
    }

    #[must_use]
    pub fn height(&self) -> u32 {
        match &self.source {
            Source::Gif(frames) => frames.height(),
            Source::Reel(frames) => frames.reel.recorded().height,
        }
    }

    /// Frames read so far, which is the index the next one will have.
    #[must_use]
    pub fn produced(&self) -> usize {
        match &self.source {
            Source::Gif(frames) => frames.produced(),
            Source::Reel(frames) => frames.produced,
        }
    }

    /// The last frame read.
    #[must_use]
    pub fn pixels(&self) -> &[RGBA8] {
        match &self.source {
            Source::Gif(frames) => frames.pixels(),
            Source::Reel(frames) => &frames.canvas,
        }
    }

    /// [`Self::pixels`] as bytes, `R G B A` per pixel, row after row with no padding --
    /// the layout a `GdkMemoryTexture` takes, so the app needs no pixel types of its own.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.pixels().as_bytes()
    }

    /// The next frame. `None` once the sequence is over; [`Self::pixels`] then still shows
    /// the last frame.
    pub fn next_frame(&mut self) -> Result<Option<Composed>, GifEditError> {
        match &mut self.source {
            Source::Gif(frames) => frames.next_frame(),
            Source::Reel(frames) => frames.read(frames.produced),
        }
    }

    /// Frame `index`, so [`Self::pixels`] shows it. `None` when there are fewer frames. A
    /// GIF re-composes from its first frame to go back, a reel reads the one frame.
    pub fn seek(&mut self, index: usize) -> Result<Option<Composed>, GifEditError> {
        match &mut self.source {
            Source::Gif(frames) => frames.seek(index),
            Source::Reel(frames) => frames.read(index),
        }
    }
}

/// How a range is written out: gifski's quality, and the two things that can still be
/// changed after the fact.
///
/// A recording's *pixels* are fixed the moment it is taken -- a GIF cannot be sharpened
/// later -- but its **frame rate** and its **size** can both come down, and those are the
/// two knobs that decide whether a GIF is 12 MB or 2 MB. `None` for either keeps what the
/// source has, which costs nothing and loses nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TrimOptions {
    /// gifski's 1-100.
    pub quality: u8,
    /// Resample to this many frames a second. Only ever drops frames: a rate above the
    /// source's own changes nothing, because there is nothing between two frames to show.
    pub fps: Option<u32>,
    /// Scale to this percentage of the source's size, 10-100.
    pub scale_percent: Option<u8>,
}

impl Default for TrimOptions {
    fn default() -> Self {
        Self { quality: 80, fps: None, scale_percent: None }
    }
}

/// One frame of the plan: which source frame to show, and when it starts in the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Planned {
    source: usize,
    start_ms: u64,
}

/// Which frames the output has and when each starts, from the range and the frame rate.
///
/// Without a rate this is the range itself, each frame keeping its own delay. With one,
/// the range is *sampled* on the rate's grid and a tick showing the same source frame as
/// the tick before is dropped -- so a 10 fps pass over a 15 fps recording drops every
/// third frame rather than duplicating anything, and a still stretch stays one long frame
/// however fast the grid ticks.
fn plan(timeline: &Timeline, range: Range<usize>, fps: Option<u32>) -> Vec<Planned> {
    let origin_ms = timeline.frames[range.start].start_ms;
    let Some(fps) = fps.filter(|&f| f > 0) else {
        return range
            .filter_map(|index| {
                timeline
                    .frames
                    .get(index)
                    .map(|frame| Planned { source: index, start_ms: frame.start_ms - origin_ms })
            })
            .collect();
    };
    let length = timeline.duration_of(range.clone()).max(1);
    let step = (1000.0 / f64::from(fps)).max(1.0);
    let ticks = ((length as f64 / step).ceil() as u64).max(1);
    let mut planned: Vec<Planned> = Vec::new();
    for tick in 0..ticks {
        let at = (tick as f64 * step).round() as u64;
        // The source frame showing at this instant, clamped into the range: a tick past
        // the last frame's start still shows the last frame.
        let source = timeline.frame_at_ms(origin_ms + at).clamp(range.start, range.end - 1);
        if planned.last().is_none_or(|last| last.source != source) {
            planned.push(Planned { source, start_ms: at });
        }
    }
    planned
}

/// Writes frames `range` of the GIF at `path` to `out`, re-encoded with gifski.
///
/// Streaming: the reader composes one frame at a time and the encoder's threads take it
/// as it comes, so a long GIF costs no more memory than a short one (the same reason the
/// recorder streams).
pub fn trim(
    path: &Path,
    range: Range<usize>,
    options: TrimOptions,
    out: &Path,
) -> Result<Written, GifEditError> {
    trim_reporting(path, range, options, out, &Progress::new())
}

/// [`trim`], filling `progress` in as gifski writes, for a caller with a progress bar.
///
/// Separate rather than a fifth argument on `trim` because most callers have nothing to
/// show it on, and a `Progress::new()` at every call site would be noise around the one
/// place it matters.
pub fn trim_reporting(
    path: &Path,
    range: Range<usize>,
    options: TrimOptions,
    out: &Path,
    progress: &Progress,
) -> Result<Written, GifEditError> {
    let timeline = scan(path)?;
    if range.start >= range.end {
        return Err(GifEditError::Empty);
    }
    if range.end > timeline.len() {
        return Err(GifEditError::Range { start: range.start, end: range.end, frames: timeline.len() });
    }
    let length = timeline.duration_of(range.clone());
    let planned = plan(&timeline, range.clone(), options.fps);
    let Some(last) = planned.last().copied() else { return Err(GifEditError::Empty) };
    // gifski gives the last frame the duration of the interval before it -- right for a
    // steady stream, wrong for a trim that ends on a hold (a screen recording is full of
    // them: identical frames merged into one long delay). Its own rule (`make_diffs`): a
    // first frame with a non-zero pts shifts every pts back by that offset, and the offset
    // *is* the last frame's duration. So the range is fed starting at the duration the
    // last frame is owed, and every frame lasts exactly what the plan says, that one
    // included. Twenty milliseconds is the smallest delay gifski writes and the smallest
    // offset the rule accepts.
    let shift = length.saturating_sub(last.start_ms).max(20) as f64 / 1000.0;
    let nominal = nominal_fps(&timeline, range, options.fps);

    let mut frames = Frames::open(path)?;
    let (tx, rx) = mpsc::sync_channel::<Frame>(4);
    // The bar can draw a fraction from here: the plan is what gifski will be asked for.
    progress.expect(planned.len());
    let encoder = Encoder::start_with(options.quality, nominal, out, rx, progress.clone())?;
    let (width, height) = (frames.width() as usize, frames.height() as usize);
    let size = scaled_size(width, height, options.scale_percent);
    for (index, step) in planned.iter().enumerate() {
        if frames.seek(step.source)?.is_none() {
            break;
        }
        let pixels = if size == (width, height) {
            frames.pixels().to_vec()
        } else {
            downscale(frames.pixels(), width, height, size.0, size.1)
        };
        let frame = Frame {
            index,
            pts: shift + step.start_ms as f64 / 1000.0,
            image: ImgVec::new(pixels, size.0, size.1),
        };
        // The encoder gone means it failed; `finish` below reports why.
        if tx.send(frame).is_err() {
            break;
        }
    }
    drop(tx);
    let mut written = encoder.finish()?;
    // The encoder guesses the last frame from `fps`; the plan knows it exactly.
    written.duration = Duration::from_millis(length);
    Ok(written)
}

/// The output's size for a scale percentage, never below one pixel and never above the
/// source (a GIF has no pixels beyond the ones it was recorded with).
fn scaled_size(width: usize, height: usize, percent: Option<u8>) -> (usize, usize) {
    let Some(percent) = percent.filter(|&p| p < 100) else { return (width, height) };
    let factor = f64::from(percent.max(10)) / 100.0;
    (
        ((width as f64 * factor).round() as usize).max(1),
        ((height as f64 * factor).round() as usize).max(1),
    )
}

/// A box downscale: every output pixel is the average of the source box it covers.
///
/// A box filter over exact source boxes, the same choice `history::thumbnail` makes and
/// for the same reason -- no ringing on the hard edges screen content is made of, and no
/// seam. Only ever called with a target no larger than the source.
fn downscale(
    pixels: &[RGBA8],
    width: usize,
    height: usize,
    to_w: usize,
    to_h: usize,
) -> Vec<RGBA8> {
    let mut out = Vec::with_capacity(to_w * to_h);
    for y in 0..to_h {
        let y0 = y * height / to_h;
        let y1 = (((y + 1) * height).div_ceil(to_h)).max(y0 + 1).min(height);
        for x in 0..to_w {
            let x0 = x * width / to_w;
            let x1 = (((x + 1) * width).div_ceil(to_w)).max(x0 + 1).min(width);
            let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let Some(px) = pixels.get(sy * width + sx) else { continue };
                    r += u32::from(px.r);
                    g += u32::from(px.g);
                    b += u32::from(px.b);
                    a += u32::from(px.a);
                    n += 1;
                }
            }
            let n = n.max(1);
            out.push(RGBA8::new((r / n) as u8, (g / n) as u8, (b / n) as u8, (a / n) as u8));
        }
    }
    out
}

/// A frame rate for the encoder's last-frame guess: the one asked for, else the first
/// trimmed frame's own delay.
fn nominal_fps(timeline: &Timeline, range: Range<usize>, asked: Option<u32>) -> u32 {
    if let Some(fps) = asked.filter(|&f| f > 0) {
        return fps.clamp(1, 60);
    }
    let delay = timeline.frames.get(range.start).map_or(100, |f| f.delay_ms.max(1));
    ((1000.0 / delay as f64).round() as u32).clamp(1, 60)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    const W: usize = 32;
    const H: usize = 20;

    fn shade(index: usize) -> u8 {
        (index * 40) as u8
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("octosnap-gif-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// `frames` solid frames of distinct colours at `fps`, through the recorder's encoder.
    fn write_gif(path: &Path, frames: usize, fps: u32) {
        let (tx, rx) = mpsc::sync_channel::<Frame>(2);
        let encoder = Encoder::start_with(80, fps, path, rx, Progress::new()).unwrap();
        for index in 0..frames {
            let pixels = vec![RGBA8::new(shade(index), 0, 255 - shade(index), 255); W * H];
            tx.send(Frame { index, pts: index as f64 / f64::from(fps), image: ImgVec::new(pixels, W, H) }).unwrap();
        }
        drop(tx);
        encoder.finish().unwrap();
    }

    /// A GIF written frame by frame with the `gif` crate: exact delays (centiseconds),
    /// and frames that may repeat -- which gifski, the other writer here, would merge on
    /// the way in, and the point of the test that uses this is what `trim` does with them.
    fn write_gif_raw(path: &Path, frames: &[(usize, u16)]) {
        let mut file = std::fs::File::create(path).unwrap();
        let mut encoder = gif::Encoder::new(&mut file, W as u16, H as u16, &[]).unwrap();
        encoder.set_repeat(gif::Repeat::Infinite).unwrap();
        for &(tone, delay) in frames {
            let mut pixels = Vec::with_capacity(W * H * 4);
            for _ in 0..W * H {
                pixels.extend_from_slice(&[shade(tone), 0, 255 - shade(tone), 255]);
            }
            let mut frame = gif::Frame::from_rgba_speed(W as u16, H as u16, &mut pixels, 10);
            frame.delay = delay;
            encoder.write_frame(&frame).unwrap();
        }
    }

    fn close(a: u8, b: u8) -> bool {
        a.abs_diff(b) < 24
    }

    /// The editor's progress bar reads this, so it has to move and it has to arrive.
    #[test]
    fn a_trim_reports_every_frame_it_writes() {
        let dir = std::env::temp_dir().join(format!("octosnap-gif-progress-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("eight.gif");
        write_gif_raw(&source, &(0..8).map(|i| (i, 7)).collect::<Vec<_>>());

        let progress = Progress::new();
        // Nothing to draw before the plan is made: the bar pulses on this.
        assert_eq!(progress.fraction(), None);

        let out = dir.join("trimmed.gif");
        trim_reporting(&source, 0..8, TrimOptions::default(), &out, &progress).unwrap();

        let (written, total) = progress.frames();
        assert_eq!(total, 8, "the plan's frames are what the bar is a fraction of");
        assert!(written > 0, "gifski wrote {written} frames and reported none");
        assert_eq!(progress.fraction(), Some(1.0), "the bar has to arrive, not stop short");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_frame_rate_drops_frames_and_never_duplicates_them() {
        let dir = std::env::temp_dir().join(format!("octosnap-gif-fps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 15 fps of movement, then a hold: what a screen recording looks like.
        let source = dir.join("fifteen.gif");
        let mut frames: Vec<(usize, u16)> = (0..15).map(|i| (i, 7)).collect();
        frames.push((15, 100));
        write_gif_raw(&source, &frames);
        let timeline = scan(&source).unwrap();
        assert_eq!(timeline.len(), 16);
        let whole = 0..timeline.len();

        // 10 fps over a ~14 fps source keeps two frames in three, and the hold stays one
        // frame however slowly it is sampled.
        let out = dir.join("ten.gif");
        let options = TrimOptions { fps: Some(10), ..TrimOptions::default() };
        trim(&source, whole.clone(), options, &out).unwrap();
        let ten = scan(&out).unwrap();
        assert!(ten.len() < timeline.len(), "{} frames is not a drop", ten.len());
        assert_eq!(ten.duration_ms(), timeline.duration_ms(), "the length is untouched");

        // A rate above the source's own has nothing to add: no frame is duplicated.
        let out = dir.join("sixty.gif");
        let options = TrimOptions { fps: Some(60), ..TrimOptions::default() };
        trim(&source, whole.clone(), options, &out).unwrap();
        let sixty = scan(&out).unwrap();
        assert!(sixty.len() <= timeline.len(), "{} frames is more than the source", sixty.len());
        assert_eq!(sixty.duration_ms(), timeline.duration_ms());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_scale_shrinks_the_canvas_and_never_grows_it() {
        let dir = std::env::temp_dir().join(format!("octosnap-gif-scale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("full.gif");
        write_gif_raw(&source, &[(0, 10), (1, 10), (2, 10)]);
        let out = dir.join("half.gif");
        let options = TrimOptions { scale_percent: Some(50), ..TrimOptions::default() };
        trim(&source, 0..3, options, &out).unwrap();
        let half = scan(&out).unwrap();
        assert_eq!((half.width, half.height), ((W / 2) as u32, (H / 2) as u32));
        // 100 and above are the source's own size, not an upscale.
        let out = dir.join("whole.gif");
        let options = TrimOptions { scale_percent: Some(100), ..TrimOptions::default() };
        trim(&source, 0..3, options, &out).unwrap();
        let whole = scan(&out).unwrap();
        assert_eq!((whole.width, whole.height), (W as u32, H as u32));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_trim_keeps_every_delay_the_last_frames_included() {
        let dir = std::env::temp_dir().join(format!("octosnap-gif-trim-delays-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Irregular holds, as a deduplicated screen recording has. Frames 1..3 end on the
        // short hold; the interval before it is the long one, which is what gifski would
        // give the last frame without the offset.
        let source = dir.join("holds.gif");
        write_gif_raw(&source, &[(0, 10), (1, 50), (2, 14), (3, 90)]);
        let timeline = scan(&source).unwrap();
        assert_eq!(timeline.duration_of(1..3), 640);
        let out = dir.join("holds-trim.gif");
        trim(&source, 1..3, TrimOptions::default(), &out).unwrap();
        let trimmed = scan(&out).unwrap();
        assert_eq!(trimmed.duration_ms(), 640, "500 + 140, not 500 + 500");
        assert_eq!(trimmed.len(), 2);

        // The same picture held three times: gifski merges the frames into one, and the
        // one must still last the whole range.
        let still = dir.join("still.gif");
        write_gif_raw(&still, &[(0, 7), (1, 7), (1, 120), (1, 30)]);
        let timeline = scan(&still).unwrap();
        assert_eq!(timeline.len(), 4);
        let out = dir.join("still-trim.gif");
        trim(&still, 1..4, TrimOptions::default(), &out).unwrap();
        let trimmed = scan(&out).unwrap();
        assert_eq!(trimmed.duration_ms(), timeline.duration_of(1..4));
        assert_eq!(trimmed.len(), 1, "merged into one hold");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_zero_delay_reads_as_a_hundred_milliseconds() {
        assert_eq!(delay_ms(0), 100);
        assert_eq!(delay_ms(7), 70);
        assert_eq!(delay_ms(10), 100);
    }

    #[test]
    fn scan_reads_every_frame_and_its_delay() {
        let path = scratch("scan.gif");
        write_gif(&path, 6, 10);
        let timeline = scan(&path).unwrap();
        assert_eq!((timeline.width, timeline.height), (W as u32, H as u32));
        assert_eq!(timeline.len(), 6);
        assert!(timeline.frames.iter().all(|f| f.delay_ms == 100), "{:?}", timeline.frames);
        assert_eq!(timeline.duration_ms(), 600);
        assert_eq!(timeline.duration_of(2..5), 300);
        assert_eq!(timeline.frame_at_ms(0), 0);
        assert_eq!(timeline.frame_at_ms(250), 2);
        assert_eq!(timeline.frame_at_ms(10_000), 5);
    }

    #[test]
    fn frames_compose_each_frame_over_the_last() {
        let path = scratch("frames.gif");
        write_gif(&path, 6, 10);
        let mut frames = Frames::open(&path).unwrap();
        let mut seen = 0;
        while let Some(composed) = frames.next_frame().unwrap() {
            assert_eq!(composed.index, seen);
            assert_eq!(composed.start_ms, seen as u64 * 100);
            let px = frames.pixels()[W * H / 2];
            assert!(close(px.r, shade(seen)), "frame {seen}: {px:?}");
            assert_eq!(px.a, 255);
            seen += 1;
        }
        assert_eq!(seen, 6);
        assert_eq!(frames.bytes().len(), W * H * 4);
    }

    #[test]
    fn seek_lands_on_the_frame_asked_for_in_either_direction() {
        let path = scratch("seek.gif");
        write_gif(&path, 6, 10);
        let mut frames = Frames::open(&path).unwrap();
        let forward = frames.seek(4).unwrap().unwrap();
        assert_eq!(forward.index, 4);
        assert!(close(frames.pixels()[0].r, shade(4)));
        // Backwards means from the start again.
        let back = frames.seek(1).unwrap().unwrap();
        assert_eq!(back.index, 1);
        assert!(close(frames.pixels()[0].r, shade(1)));
        assert!(frames.seek(9).unwrap().is_none(), "past the end is None");
    }

    #[test]
    fn trim_keeps_only_the_range_and_its_length() {
        let path = scratch("trim-in.gif");
        let out = scratch("trim-out.gif");
        write_gif(&path, 6, 10);
        let written = trim(&path, 2..5, TrimOptions::default(), &out).unwrap();
        assert_eq!(written.frames, 3);
        assert_eq!(written.duration, Duration::from_millis(300));
        assert!(written.bytes > 0);

        let timeline = scan(&out).unwrap();
        assert_eq!(timeline.len(), 3);
        assert_eq!(timeline.duration_ms(), 300);
        let mut frames = Frames::open(&out).unwrap();
        frames.next_frame().unwrap().unwrap();
        assert!(close(frames.pixels()[0].r, shade(2)), "the trimmed GIF starts at frame 2");
    }

    #[test]
    fn trim_refuses_an_empty_or_outside_range() {
        let path = scratch("trim-bad.gif");
        let out = scratch("trim-bad-out.gif");
        write_gif(&path, 4, 10);
        assert!(matches!(trim(&path, 3..3, TrimOptions::default(), &out), Err(GifEditError::Empty)));
        assert!(matches!(trim(&path, 2..9, TrimOptions::default(), &out), Err(GifEditError::Range { frames: 4, .. })));
        assert!(!out.exists(), "nothing is written for a refused range");
    }

    /// A timeline of these delays, in milliseconds, with no rate of its own: a GIF's.
    fn timeline_of(delays: &[u64]) -> Timeline {
        let mut start_ms = 0;
        let frames = delays
            .iter()
            .map(|&delay_ms| {
                let frame = FrameTime { start_ms, delay_ms };
                start_ms += delay_ms;
                frame
            })
            .collect();
        Timeline { width: W as u32, height: H as u32, frames, fps: None }
    }

    /// A GIF says hundredths of a second, so forty frames a second is two and three in
    /// turn, and the commonest of the two -- by one frame -- named the rate: "33 fps" for a
    /// recording taken at forty (2026-09-24). A stretch where nothing moved is one long
    /// frame, and says nothing about the rate either.
    #[test]
    fn a_gif_plays_at_the_rate_its_moving_frames_keep() {
        let mut forty = vec![80];
        for _ in 0..72 {
            forty.extend([20, 30]);
        }
        forty.extend([30, 70, 100, 230, 150]);
        assert_eq!(timeline_of(&forty).rate(), 40);
        assert_eq!(timeline_of(&[30, 30, 40, 30, 30, 40, 330]).rate(), 30);
        assert_eq!(timeline_of(&[60, 70, 70, 60, 70, 70, 1000]).rate(), 15);
        assert_eq!(timeline_of(&[40, 40, 40, 50, 40, 40, 40, 50]).rate(), 24);
        assert_eq!(timeline_of(&[20; 10]).rate(), 50);
        assert_eq!(timeline_of(&[]).rate(), 15, "nothing to read a rate off");
        let reel = Timeline { fps: Some(40), ..timeline_of(&[25, 25, 500]) };
        assert_eq!(reel.rate(), 40, "a reel says the rate it was taken at");
    }

    /// A reel is written out the way a GIF is trimmed, through the same reader: the range,
    /// each frame at its own time, and the frame rate asked for.
    #[test]
    fn a_reel_is_written_out_like_a_gif_is_trimmed() {
        let dir = scratch("frames").with_extension(reel::EXTENSION);
        let _ = std::fs::remove_dir_all(&dir);
        let (tx, rx) = mpsc::sync_channel::<Frame>(4);
        let recorded = reel::Recorded {
            width: W as u32,
            height: H as u32,
            fps: 20,
            gif: TrimOptions::default(),
            end_ms: None,
        };
        let writer = reel::Writer::start(&dir, recorded, rx).unwrap();
        for index in 0..10 {
            let pixels = vec![RGBA8::new(shade(index), 0, 255 - shade(index), 255); W * H];
            let image = ImgVec::new(pixels, W, H);
            tx.send(Frame { index, pts: index as f64 / 20.0, image }).unwrap();
        }
        drop(tx);
        writer.finish().unwrap();

        let timeline = scan(&dir).unwrap();
        assert_eq!((timeline.len(), timeline.duration_ms(), timeline.rate()), (10, 500, 20));
        let mut frames = Frames::open(&dir).unwrap();
        frames.seek(7).unwrap().unwrap();
        assert!(close(frames.pixels()[0].r, shade(7)), "a reel seeks straight to the frame");
        frames.seek(2).unwrap().unwrap();
        assert!(close(frames.pixels()[0].r, shade(2)), "and back");

        let out = scratch("from-a-reel.gif");
        let written = trim(&dir, 2..8, TrimOptions::default(), &out).unwrap();
        assert_eq!(written.duration, Duration::from_millis(300));
        let gif = scan(&out).unwrap();
        assert_eq!((gif.len(), gif.duration_ms()), (6, 300));
        let options = TrimOptions { fps: Some(10), ..TrimOptions::default() };
        let halved = trim(&dir, 0..10, options, &out).unwrap();
        assert_eq!((scan(&out).unwrap().len(), halved.duration), (5, Duration::from_millis(500)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_is_not_a_gif_is_an_error_not_a_panic() {
        let path = scratch("not.gif");
        std::fs::write(&path, b"PNG? no.").unwrap();
        assert!(matches!(scan(&path), Err(GifEditError::Decode { .. })));
        assert!(matches!(Frames::open(&path), Err(GifEditError::Decode { .. })));
        assert!(matches!(scan(Path::new("/nonexistent/x.gif")), Err(GifEditError::Open { .. })));
    }
}
