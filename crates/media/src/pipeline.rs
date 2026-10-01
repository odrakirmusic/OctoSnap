// SPDX-License-Identifier: GPL-3.0-or-later
//! The GStreamer graph a GIF is taken through, in system memory at the GIF's own frame
//! rate and size (`spec/06` §4.1, D68):
//!
//! ```text
//! pipewiresrc keepalive-time=K ! video/x-raw ! videorate ! video/x-raw,framerate=F/1
//!   ! videocrop ! videoscale method=lanczos ! video/x-raw,width=W,height=H ! videoconvert
//!   ! video/x-raw,format=RGBA ! appsink
//! ```
//!
//! `keepalive-time` first: the stream is damage-driven and a still screen otherwise
//! records nothing (`docs/spikes/11`). `videorate` next, so a frame the GIF will not keep
//! is dropped before it is cropped, scaled or converted. The crop is the selection in the
//! monitor's physical pixels, planned in [`crate::gif`]; the scale is `gif-max-width`, done
//! with Lanczos taps rather than videoscale's default two, so small text survives it.
//! Every frame the sink gets is copied once into an RGBA image and offered to the encoder's
//! thread through a bounded channel -- **offered**, never waited on: a frame the encoder has
//! no room for is dropped at the sink, so the stream stays live whatever the encoder is
//! doing (D103).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gst::prelude::*;
use gst_video::prelude::*;
use imgref::ImgVec;
use rgb::RGBA8;

use crate::gif::GifPlan;
use crate::screencast::Target;

/// One frame for the encoder: its index from zero, when it shows, its pixels.
#[derive(Debug)]
pub struct Frame {
    pub index: usize,
    /// Seconds since the first frame.
    pub pts: f64,
    pub image: ImgVec<RGBA8>,
}

#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("GStreamer could not be initialised: {0}")]
    Init(#[from] glib::Error),
    #[error("GStreamer has no `{0}` element; is the plugin installed?")]
    MissingElement(&'static str),
    #[error("the graph could not be linked: {0}")]
    Link(#[from] glib::BoolError),
    #[error("a size of {0} does not fit the graph's properties")]
    Range(u32),
    #[error("the graph would not start playing")]
    Play,
}

/// The built graph. Frames flow to the channel it was built with until EOS or
/// [`Graph::shutdown`].
#[derive(Debug)]
pub struct Graph {
    pipeline: gst::Pipeline,
    frames: Arc<Mutex<Option<SyncSender<Frame>>>>,
    taken: Arc<AtomicU64>,
    /// Frames the sink dropped because the encoder had no room for them.
    dropped: Arc<AtomicU64>,
    /// When `play` was called, so the sink can say how long the first frame took.
    played: Arc<Mutex<Option<Instant>>>,
    /// How long the first frame took after `play`, in milliseconds; zero until it comes.
    first_frame: Arc<AtomicU64>,
    /// False while the graph is *armed*: running, negotiated, and throwing its frames
    /// away. See [`Graph::set_capturing`].
    capturing: Arc<AtomicBool>,
}

impl Graph {
    /// Builds the graph for `plan` against `target`, delivering into `frames`.
    ///
    /// `capturing` false builds it **armed**: it plays and negotiates like any other, and
    /// drops every frame at the sink until [`Graph::set_capturing`] turns it on. That is
    /// what lets the whole start happen during the countdown (D70).
    pub fn build(
        plan: &GifPlan,
        target: Target,
        frames: SyncSender<Frame>,
        capturing: bool,
    ) -> Result<Self, GraphError> {
        gst::init()?;

        let pipewiresrc = factory("pipewiresrc")?;
        let src = pipewiresrc
            .create()
            .property("keepalive-time", int(plan.keepalive_ms)?)
            .property("do-timestamp", true);
        let src = match target {
            Target::NodeId(id) => src.property("path", id.to_string()),
            Target::Serial(serial) => src.property("target-object", serial.to_string()),
        }
        .build()?;

        // System memory, which Mutter fills by painting the stage afresh. The extension
        // keeps the red frame out of what that paint puts in this crop (D115); a stream of
        // another kind is not known to put its pixels in the same places.
        let system_memory = factory("capsfilter")?.create()
            .property("caps", gst::Caps::builder("video/x-raw").build())
            .build()?;
        let rate = factory("videorate")?.create().build()?;
        let at_rate = factory("capsfilter")?.create()
            .property(
                "caps",
                gst::Caps::builder("video/x-raw")
                    .field("framerate", gst::Fraction::new(int(plan.fps)?, 1))
                    .build(),
            )
            .build()?;
        let crop = factory("videocrop")?.create()
            .property("left", int(plan.crop.left)?)
            .property("top", int(plan.crop.top)?)
            .property("right", int(plan.crop.right)?)
            .property("bottom", int(plan.crop.bottom)?)
            .build()?;
        // Lanczos, not the default bilinear. GStreamer's bilinear is a **2-tap** filter:
        // scaling 1920 px down to `gif-max-width` it reads two source pixels per output
        // pixel and throws the rest away, which on screen content -- one-pixel strokes,
        // hairlines, hard edges -- aliases the small text into mush and then hands the
        // mush to a 256-colour palette. Lanczos sizes its taps from the scale factor, so
        // every source pixel that contributes is read; text downscaled with it stays
        // legible. It costs nothing at 1:1, where videoscale passes the buffer through,
        // and at GIF sizes the cost is well under a frame interval.
        let scale = factory("videoscale")?.create()
            .property_from_str("method", "lanczos")
            .build()?;
        let at_size = factory("capsfilter")?.create()
            .property(
                "caps",
                gst::Caps::builder("video/x-raw")
                    .field("width", int(plan.output.width)?)
                    .field("height", int(plan.output.height)?)
                    .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
                    .build(),
            )
            .build()?;
        let convert = factory("videoconvert")?.create().build()?;

        let frames = Arc::new(Mutex::new(Some(frames)));
        let taken = Arc::new(AtomicU64::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let played: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let first_frame = Arc::new(AtomicU64::new(0));
        let capturing = Arc::new(AtomicBool::new(capturing));
        let sink = gst_app::AppSink::builder()
            .caps(&gst::Caps::builder("video/x-raw").field("format", "RGBA").build())
            .sync(false)
            // Shallow, and never full: the callback below takes every sample the moment it
            // arrives and never waits, so this only absorbs a scheduling hiccup.
            .max_buffers(3)
            .callbacks(
                gst_app::AppSinkCallbacks::builder()
                    .new_sample(new_sample(
                        Arc::clone(&frames),
                        Arc::clone(&taken),
                        Arc::clone(&dropped),
                        Arc::clone(&played),
                        Arc::clone(&first_frame),
                        Arc::clone(&capturing),
                        plan.fps,
                    ))
                    .build(),
            )
            .build();

        let pipeline = gst::Pipeline::builder().name("octosnap-gif").build();
        let elements = [
            &src,
            &system_memory,
            &rate,
            &at_rate,
            &crop,
            &scale,
            &at_size,
            &convert,
            sink.upcast_ref::<gst::Element>(),
        ];
        pipeline.add_many(elements)?;
        gst::Element::link_many(elements)?;
        Ok(Self { pipeline, frames, taken, dropped, played, first_frame, capturing })
    }

    /// Starts the graph. The first frame follows when the stream negotiates.
    pub fn play(&self) -> Result<(), GraphError> {
        if let Ok(mut slot) = self.played.lock() {
            *slot = Some(Instant::now());
        }
        self.pipeline.set_state(gst::State::Playing).map_err(|_| GraphError::Play)?;
        Ok(())
    }

    /// Turns the sink's frames on or off without touching the pipeline's state.
    ///
    /// An armed graph is already playing: the ScreenCast session is open, PipeWire has
    /// negotiated, `videorate` has its cadence and the sink is being handed frames --
    /// which it drops before it copies them. Flipping this is therefore the whole of
    /// "start recording", and costs one atomic store instead of the second and a half
    /// that building all of that takes (D70). The frame that arrives after it becomes
    /// frame zero at pts zero, so a discarded countdown leaves no trace in the GIF.
    pub fn set_capturing(&self, on: bool) {
        self.capturing.store(on, Ordering::Relaxed);
    }

    /// The graph's bus, for EOS and errors.
    pub fn bus(&self) -> Result<gst::Bus, GraphError> {
        self.pipeline.bus().ok_or(GraphError::Play)
    }

    /// Asks the graph to finish: every frame in flight reaches the sink, then EOS is posted
    /// on the bus. Without it the encoder never learns the recording is over.
    pub fn send_eos(&self) -> bool {
        self.pipeline.send_event(gst::event::Eos::new())
    }

    /// Frames the sink has handed on so far.
    #[must_use]
    pub fn frames_taken(&self) -> u64 {
        self.taken.load(Ordering::Relaxed)
    }

    /// Frames the sink dropped because the encoder was still busy with earlier ones --
    /// the difference between the rate asked for and the rate the encoder can keep.
    #[must_use]
    pub fn frames_dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// How long after `play` the first frame reached the sink, in milliseconds. Zero
    /// until one has: the compositor's negotiation, which is most of a slow start.
    #[must_use]
    pub fn first_frame_ms(&self) -> u64 {
        self.first_frame.load(Ordering::Relaxed)
    }

    /// Tears the graph down and closes the frame channel, which is what ends the encoder's
    /// collector thread. Idempotent.
    pub fn shutdown(&self) {
        if let Err(e) = self.pipeline.set_state(gst::State::Null) {
            tracing::warn!("the graph would not stop: {e}");
        }
        if let Ok(mut slot) = self.frames.lock() {
            slot.take();
        }
    }
}

impl Drop for Graph {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The factory for `name`, or the message that says which plugin is missing.
pub(crate) fn factory(name: &'static str) -> Result<gst::ElementFactory, GraphError> {
    gst::ElementFactory::find(name).ok_or(GraphError::MissingElement(name))
}

pub(crate) fn int(value: u32) -> Result<i32, GraphError> {
    i32::try_from(value).map_err(|_| GraphError::Range(value))
}

/// The sink's callback, on the streaming thread: one copy per frame into an RGBA image,
/// then the channel -- or, when the encoder has no room, nowhere.
fn new_sample(
    frames: Arc<Mutex<Option<SyncSender<Frame>>>>,
    taken: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    played: Arc<Mutex<Option<Instant>>>,
    first_frame: Arc<AtomicU64>,
    capturing: Arc<AtomicBool>,
    fps: u32,
) -> impl FnMut(&gst_app::AppSink) -> Result<gst::FlowSuccess, gst::FlowError> + Send + 'static {
    let mut first_pts: Option<u64> = None;
    let mut index: usize = 0;
    let mut arrived = false;
    move |sink| {
        let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
        if !arrived {
            arrived = true;
            // The number that says whether a slow start is ours or the compositor's:
            // everything before this is setup, this is PipeWire negotiating and the first
            // buffer coming out of it. Stamped on the first frame to *arrive*, kept or
            // dropped, because that is when the stream is ready -- which for an armed
            // graph is during the countdown, where it costs nothing.
            let waited = played
                .lock()
                .ok()
                .and_then(|slot| *slot)
                .map_or(0, |at| u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX));
            first_frame.store(waited, Ordering::Relaxed);
            tracing::info!(first_frame_ms = waited, "the stream produced its first frame");
        }
        if !capturing.load(Ordering::Relaxed) {
            // Armed, not recording: this frame is the countdown's. Dropped before it is
            // copied, so the arming costs a negotiated stream and nothing else.
            return Ok(gst::FlowSuccess::Ok);
        }
        let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
        let caps = sample.caps().ok_or(gst::FlowError::NotNegotiated)?;
        let info = gst_video::VideoInfo::from_caps(caps).map_err(|_| gst::FlowError::NotNegotiated)?;
        let frame = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info)
            .map_err(|_| gst::FlowError::Error)?;
        let image = rgba_image(&frame).ok_or(gst::FlowError::Error)?;

        // Seconds since the first frame, from the buffer's own clock; a buffer without one
        // is placed by its index, which `videorate` makes exact anyway.
        let pts = buffer.pts().map(gst::ClockTime::nseconds);
        let seconds = match (pts, first_pts) {
            (Some(now), Some(first)) => now.saturating_sub(first) as f64 / 1e9,
            (Some(now), None) => {
                first_pts = Some(now);
                0.0
            }
            (None, _) => index as f64 / f64::from(fps.max(1)),
        };

        // Offered, never waited on. This runs on the streaming thread, and a sink that
        // waits for the encoder stalls the source: `videorate` then fills each stalled
        // interval with copies of the last frame, which gives the encoder more to do,
        // which stalls the source for longer. On the target machine an unoptimised
        // encoder managed two frames a second against fifty asked for, the stream fell
        // further behind with every frame, Stop's EOS queued behind a burst of copies for
        // twenty seconds, and the recording was thrown away (2026-09-23, D103). A frame
        // the encoder has no room for is dropped here instead: the GIF holds the previous
        // one for longer, which is exactly what a lower frame rate looks like, and the
        // stream -- and Stop -- stay live. `index` counts only frames sent, because
        // gifski waits for every index it has not seen yet.
        let tx = frames.lock().ok().and_then(|slot| slot.as_ref().cloned());
        let Some(tx) = tx else {
            tracing::warn!("the encoder is gone; the graph stops");
            return Err(gst::FlowError::Error);
        };
        match tx.try_send(Frame { index, pts: seconds, image }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                dropped.fetch_add(1, Ordering::Relaxed);
                return Ok(gst::FlowSuccess::Ok);
            }
            Err(TrySendError::Disconnected(_)) => {
                tracing::warn!("the encoder is gone; the graph stops");
                return Err(gst::FlowError::Error);
            }
        }
        if index == 0 {
            tracing::info!(
                width = info.width(),
                height = info.height(),
                "the first frame reached the sink"
            );
        }
        index += 1;
        taken.fetch_add(1, Ordering::Relaxed);
        Ok(gst::FlowSuccess::Ok)
    }
}

/// The frame's RGBA plane, row by row past the stride, as the image the encoder wants.
fn rgba_image(frame: &gst_video::VideoFrameRef<&gst::BufferRef>) -> Option<ImgVec<RGBA8>> {
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    let stride = usize::try_from(*frame.plane_stride().first()?).ok()?;
    let data = frame.plane_data(0).ok()?;
    let row_bytes = width.checked_mul(4)?;
    let mut pixels = Vec::with_capacity(width.checked_mul(height)?);
    for y in 0..height {
        let row = data.get(y * stride..y * stride + row_bytes)?;
        pixels.extend(row.as_chunks::<4>().0.iter().map(|p| RGBA8::new(p[0], p[1], p[2], p[3])));
    }
    Some(ImgVec::new(pixels, width, height))
}
