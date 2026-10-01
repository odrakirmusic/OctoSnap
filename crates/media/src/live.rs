// SPDX-License-Identifier: GPL-3.0-or-later
//! A live view of one rectangle of one monitor, for `spec/07` §1's scrolling capture (D106).
//!
//! The extension's `GrabFrame` answers with a PNG of the selection, and on the target
//! machine that costs a third of a second at 1.6 Mpx: the shell encodes it, the app decodes
//! it, and a settled frame is two of them. So a page scrolled by hand was looked at about
//! once a second -- and a person scrolling reads further than a screen in a second. The
//! first frame missed, and because a frame nobody could place leaves the anchor where it
//! was, every frame after it was measured against the same stale picture and missed too
//! (2026-09-23). This is the recorder's stream pointed at the selection instead: frames
//! arrive as the compositor paints them, in memory, and the loop takes the newest whenever
//! it is ready for one.
//!
//! ```text
//! pipewiresrc ! video/x-raw ! videorate max-rate=R ! videocrop ! videoconvert
//!   ! video/x-raw,format=RGBA ! appsink max-buffers=1 drop=true
//! ```
//!
//! `max-rate` drops and never duplicates, so a page standing still sends nothing -- which
//! is the most useful thing it could say. The sink keeps one sample and replaces it, and
//! nothing is copied until the loop asks for a frame: a frame it never asks for costs the
//! compositor's copy and the crop, and nothing else.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use gst::prelude::*;
use gst_video::prelude::*;
use octosnap_core::geometry::Rect;

use crate::gif::{GifPlan, GifSettings, PlanError, Size, StreamSource};
use crate::pipeline::{GraphError, factory, int};
use crate::screencast::{ScreenCastError, Session};

/// What can go wrong between a selection and a live view of it.
#[derive(Debug, thiserror::Error)]
pub enum LiveError {
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error(transparent)]
    ScreenCast(#[from] ScreenCastError),
    #[error(transparent)]
    Graph(#[from] GraphError),
}

/// One frame's pixels: RGBA, rows packed with no padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// The newest sample, and the serial it arrived with -- kept together, so a reader never
/// pairs one frame's pixels with the next frame's number.
type Newest = Arc<Mutex<Option<(u64, gst::Sample)>>>;

/// A live view in progress. Ends through [`Live::stop`]; dropped, it stops its graph, and
/// the session goes with the connection.
#[derive(Debug)]
pub struct Live {
    session: Session,
    pipeline: gst::Pipeline,
    newest: Newest,
    serial: Arc<AtomicU64>,
    ended: Rc<Cell<bool>>,
    _watch: gst::bus::BusWatchGuard,
    /// The selection in the stream's physical pixels: the size every frame will be.
    pub size: Size,
}

impl Live {
    /// Opens a session, records the monitor the selection is on with the pointer hidden,
    /// and plays a graph that keeps the newest frame of the selection.
    ///
    /// `rate` caps how many frames a second are taken. On any failure the session is
    /// stopped before the error is returned.
    pub async fn open(
        connection: &gio::DBusConnection,
        source: &StreamSource,
        selection: Rect,
        rate: u32,
    ) -> Result<Self, LiveError> {
        // The recorder's own plan, for the one thing it and this share: the selection as a
        // crop of the monitor's stream, in physical pixels, rounded the way the compositor
        // rounds (D23). No scaling, no pointer, and a quality nobody reads.
        let settings = GifSettings { fps: rate.max(1), max_width: 0, quality: 100, cursor: false };
        let plan = GifPlan::new(selection, source, settings)?;
        let mut session = Session::create(connection).await?;
        match Self::play(&mut session, &plan).await {
            Ok(played) => {
                let ended = Rc::new(Cell::new(false));
                {
                    let ended = Rc::clone(&ended);
                    session.connect_closed(move || ended.set(true));
                }
                let watch = {
                    let ended = Rc::clone(&ended);
                    played
                        .pipeline
                        .bus()
                        .ok_or(GraphError::Play)?
                        .add_watch_local(move |_, message| {
                            match message.view() {
                                gst::MessageView::Eos(_) => ended.set(true),
                                gst::MessageView::Error(e) => {
                                    tracing::warn!("the live view failed: {}", e.error());
                                    ended.set(true);
                                }
                                _ => {}
                            }
                            glib::ControlFlow::Continue
                        })
                        .map_err(GraphError::Link)?
                };
                tracing::info!(
                    connector = plan.connector,
                    width = plan.region.width,
                    height = plan.region.height,
                    rate = plan.fps,
                    "the live view is playing"
                );
                Ok(Self {
                    session,
                    pipeline: played.pipeline,
                    newest: played.newest,
                    serial: played.serial,
                    ended,
                    _watch: watch,
                    size: plan.region,
                })
            }
            Err(e) => {
                if let Err(stop) = session.stop().await {
                    tracing::warn!("could not stop the session after a failed start: {stop}");
                }
                Err(e)
            }
        }
    }

    async fn play(session: &mut Session, plan: &GifPlan) -> Result<Played, LiveError> {
        let stream = session.record_monitor(&plan.connector, 0, false).await?;
        let node = session.start(&stream).await?;
        let played = graph(plan, node)?;
        played.pipeline.set_state(gst::State::Playing).map_err(|_| GraphError::Play)?;
        Ok(played)
    }

    /// Grows by one with every frame that arrives. Cheap enough to poll.
    #[must_use]
    pub fn serial(&self) -> u64 {
        self.serial.load(Ordering::Acquire)
    }

    /// The newest frame and the serial it arrived with, or `None` before the first.
    ///
    /// A sample, not pixels: it is a reference, and [`Live::pixels`] is the copy -- which a
    /// caller can make on another thread, because a sample is `Send`.
    #[must_use]
    pub fn newest(&self) -> Option<(u64, gst::Sample)> {
        self.newest.lock().ok()?.clone()
    }

    /// True once the stream has stopped for good: the compositor closed the session, or
    /// the graph failed or ran out.
    #[must_use]
    pub fn has_ended(&self) -> bool {
        self.ended.get()
    }

    /// A sample's pixels as packed RGBA rows.
    #[must_use]
    pub fn pixels(sample: &gst::Sample) -> Option<Rgba> {
        let buffer = sample.buffer()?;
        let info = gst_video::VideoInfo::from_caps(sample.caps()?).ok()?;
        let frame = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info).ok()?;
        let (width, height) = (frame.width(), frame.height());
        let stride = usize::try_from(*frame.plane_stride().first()?).ok()?;
        let data = frame.plane_data(0).ok()?;
        let row = (width as usize).checked_mul(4)?;
        let mut pixels = Vec::with_capacity(row.checked_mul(height as usize)?);
        for y in 0..height as usize {
            pixels.extend_from_slice(data.get(y * stride..y * stride + row)?);
        }
        Some(Rgba { width, height, pixels })
    }

    /// Stops the graph and the session.
    pub async fn stop(self) {
        if let Err(e) = self.pipeline.set_state(gst::State::Null) {
            tracing::warn!("the live view would not stop: {e}");
        }
        if let Err(e) = self.session.stop().await {
            tracing::warn!("could not stop the live view's session: {e}");
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

/// A graph that plays, and the slot its sink fills.
struct Played {
    pipeline: gst::Pipeline,
    newest: Newest,
    serial: Arc<AtomicU64>,
}

/// The graph in the module comment, against the stream's node.
fn graph(plan: &GifPlan, node: u32) -> Result<Played, GraphError> {
    gst::init()?;
    let src = factory("pipewiresrc")?.create().property("path", node.to_string()).build()?;
    // System memory, which Mutter fills by painting the stage afresh. The extension keeps
    // the scroll outline out of what that paint puts in this crop (D115); a stream of
    // another kind is not known to put its pixels in the same places.
    let system_memory = factory("capsfilter")?
        .create()
        .property("caps", gst::Caps::builder("video/x-raw").build())
        .build()?;
    let rate = factory("videorate")?.create().property("max-rate", int(plan.fps)?).build()?;
    let crop = factory("videocrop")?
        .create()
        .property("left", int(plan.crop.left)?)
        .property("top", int(plan.crop.top)?)
        .property("right", int(plan.crop.right)?)
        .property("bottom", int(plan.crop.bottom)?)
        .build()?;
    let convert = factory("videoconvert")?.create().build()?;

    let newest: Newest = Arc::new(Mutex::new(None));
    let serial = Arc::new(AtomicU64::new(0));
    let sink = {
        let newest = Arc::clone(&newest);
        let serial = Arc::clone(&serial);
        gst_app::AppSink::builder()
            .caps(&gst::Caps::builder("video/x-raw").field("format", "RGBA").build())
            .sync(false)
            // One sample, replaced by the next: the loop wants the newest frame, never a
            // queue of old ones, and the streaming thread must never wait for it.
            .max_buffers(1)
            .drop(true)
            .callbacks(
                gst_app::AppSinkCallbacks::builder()
                    .new_sample(move |sink| {
                        let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                        let number = serial.load(Ordering::Relaxed) + 1;
                        if let Ok(mut slot) = newest.lock() {
                            *slot = Some((number, sample));
                        }
                        serial.store(number, Ordering::Release);
                        Ok(gst::FlowSuccess::Ok)
                    })
                    .build(),
            )
            .build()
    };

    let pipeline = gst::Pipeline::builder().name("octosnap-live").build();
    let sink_element = sink.upcast_ref::<gst::Element>();
    let elements = [&src, &system_memory, &rate, &crop, &convert, sink_element];
    pipeline.add_many(elements)?;
    gst::Element::link_many(elements)?;
    Ok(Played { pipeline, newest, serial })
}
