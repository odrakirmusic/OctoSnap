// SPDX-License-Identifier: GPL-3.0-or-later
//! One GIF recording from start to reel: the ScreenCast session, the graph, the reel its
//! frames are kept in, and the two ways it ends. Stop is an EOS the recorder waits for, so
//! the last frame is in the reel before anyone reads it; Trash is a teardown that deletes.
//! Every failure along the way leaves a stopped session and no half-open stream, because a
//! stream the compositor thinks is still being recorded keeps its indicator on and its
//! buffers allocated (`spec/06` §4.4).
//!
//! No GIF is written here. A recording's frames are kept losslessly, and the GIF is
//! written from them when somebody asks for one ([`crate::reel`], D113).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures_channel::oneshot;
use futures_util::future::{Either, select};
use gst::prelude::*;

use crate::gif::{GifPlan, Size};
use crate::gif_edit::TrimOptions;
use crate::pipeline::{Graph, GraphError};
use crate::reel::{Kept, Recorded, ReelError, Writer};
use crate::screencast::{ScreenCastError, Session, Target, resolve_serial};

/// How long Stop waits for the graph to drain to EOS before giving up on the stream.
///
/// The sink never waits for the reel (D103), so nothing downstream of the source can
/// hold an EOS back and the drain is a few milliseconds. This is the ceiling for a stream
/// that has genuinely wedged -- and even then the frames already taken are kept, so the
/// wait buys nothing past a few seconds. It was twenty, which is how long the user of
/// 2026-09-23 watched "Processing" before being told the recording had failed.
const EOS_TIMEOUT: Duration = Duration::from_secs(5);
/// How many bytes of frames may wait between the graph and the reel: room for the disk to
/// stall for a few seconds without a frame being dropped.
///
/// [P]. It was four frames (D103), then half a gigabyte in front of gifski (D111), which
/// writes a full-size frame five to ten times a second on the target machine: a 1920 x 784
/// recording at forty filled it in three and a half seconds and dropped half its frames
/// from there on (2026-09-24). The reel writes such a frame in a few milliseconds on each
/// of three threads (D113), so the queue is empty but for a moment, and the budget is only
/// ever spent on a disk that stops answering. The sink still never waits (D103); past
/// this, it drops.
const CHANNEL_BYTES: usize = 512 * 1024 * 1024;
/// The fewest frames the channel holds, however large they are.
const CHANNEL_FRAMES: usize = 4;
/// The most, however small: a channel allocates every slot it has when it is made.
const CHANNEL_MOST: usize = 1024;

/// How many frames of `output` fit in [`CHANNEL_BYTES`], between [`CHANNEL_FRAMES`] and
/// [`CHANNEL_MOST`]. The graph's frames are RGBA, four bytes a pixel.
fn channel_frames(output: Size) -> usize {
    let each = (output.width as usize * output.height as usize * 4).max(1);
    (CHANNEL_BYTES / each).clamp(CHANNEL_FRAMES, CHANNEL_MOST)
}

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error(transparent)]
    ScreenCast(#[from] ScreenCastError),
    #[error(transparent)]
    Graph(#[from] GraphError),
    #[error(transparent)]
    Reel(#[from] ReelError),
    #[error("the stream failed: {0}")]
    Stream(String),
    #[error("the stream did not finish within {0:?}")]
    NoEos(Duration),
}

/// How `pipewiresrc` finds the node (`docs/spikes/11`; settled by `octosnap record`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Targeting {
    /// `path=<node id>`, straight from `PipeWireStreamAdded`.
    #[default]
    NodeId,
    /// `target-object=<object.serial>`, resolved through `pw-cli`.
    Serial,
}

/// What a recording is told beyond its plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    pub targeting: Targeting,
    /// Mutter's `is-recording`: the compositor shows its own indicator.
    pub mark_recording: bool,
}

/// How the stream ended, as the bus reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Eos,
    Error(String),
}

/// The shared end-of-stream slot: the bus watch fills it, whoever is waiting is woken.
#[derive(Debug, Default)]
struct Ending {
    outcome: RefCell<Option<Outcome>>,
    waiter: RefCell<Option<oneshot::Sender<()>>>,
}

impl Ending {
    fn set(&self, outcome: Outcome) {
        if self.outcome.borrow().is_some() {
            return;
        }
        *self.outcome.borrow_mut() = Some(outcome);
        if let Some(waiter) = self.waiter.borrow_mut().take() {
            let _ = waiter.send(());
        }
    }
}

/// How long each phase of the start took, for the CLI's `started:` line and the app's
/// log. A slow start is otherwise one number with five suspects behind it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StartTimings {
    /// `RecordMonitor`: the compositor making the stream.
    pub stream: Duration,
    /// `Start` and the `PipeWireStreamAdded` that answers it.
    pub node: Duration,
    /// Building our own graph, `gst::init` included.
    pub graph: Duration,
    /// Starting the threads that keep the frames.
    pub reel: Duration,
}

impl StartTimings {
    /// Everything before the graph played.
    #[must_use]
    pub fn total(&self) -> Duration {
        self.stream + self.node + self.graph + self.reel
    }
}

/// Everything `start_stream` built, before the session joins it.
struct Started {
    graph: Graph,
    writer: Writer,
    ending: Rc<Ending>,
    watch: gst::bus::BusWatchGuard,
    node: u32,
    target: Target,
    at: Instant,
    timings: StartTimings,
}

/// A recording in progress. Ends through [`Recording::stop`] or [`Recording::trash`].
#[derive(Debug)]
pub struct Recording {
    session: Session,
    graph: Graph,
    writer: Option<Writer>,
    /// The reel's directory.
    path: PathBuf,
    /// When frames started being kept. Set at [`Recording::begin`], which for an armed
    /// recording is some seconds after the graph began playing.
    started: Cell<Instant>,
    ending: Rc<Ending>,
    _watch: gst::bus::BusWatchGuard,
    /// When the graph began playing, armed or not: `begin` reports the gap.
    armed: Instant,
    /// The node the stream came up on, for the log and the harness.
    pub node: u32,
    pub target: Target,
    timings: StartTimings,
}

impl Recording {
    /// Opens the session, starts the stream, builds and plays the graph, starts keeping
    /// frames in the reel at `path`, and keeps them from the first one on.
    ///
    /// On any failure the session is stopped before the error is returned.
    pub async fn start(
        connection: &gio::DBusConnection,
        plan: &GifPlan,
        path: &Path,
        options: Options,
    ) -> Result<Self, RecordError> {
        Self::open(connection, plan, path, options, true).await
    }

    /// Everything [`Recording::start`] does, except keep the frames.
    ///
    /// **This is the whole of the recorder's start latency, moved off the critical path**
    /// (D70). Opening the session, asking Mutter to record the monitor, waiting for the
    /// PipeWire node, building the graph, starting the reel and waiting for the stream
    /// to negotiate its first buffer measured 266 ms warm and 504 ms cold on the target
    /// machine -- all of it in front of the user, after a countdown they had already
    /// watched. Armed, it runs *during* that countdown instead: the graph plays, the sink
    /// throws every frame away, and [`Recording::begin`] is one atomic store.
    ///
    /// An armed recording holds a ScreenCast session open, so Mutter shows its recording
    /// indicator from here: the caller is expected to begin or drop it within seconds.
    pub async fn arm(
        connection: &gio::DBusConnection,
        plan: &GifPlan,
        path: &Path,
        options: Options,
    ) -> Result<Self, RecordError> {
        Self::open(connection, plan, path, options, false).await
    }

    /// Starts keeping frames. The next one to arrive is frame zero at pts zero.
    ///
    /// A no-op on a recording that was never armed, and cheap enough to call from a
    /// button handler: nothing is built, nothing is awaited.
    pub fn begin(&self) {
        self.graph.set_capturing(true);
        self.started.set(Instant::now());
        tracing::info!(
            armed_ms = ms(self.armed.elapsed()),
            first_frame_ms = self.graph.first_frame_ms(),
            "recording begins"
        );
    }

    async fn open(
        connection: &gio::DBusConnection,
        plan: &GifPlan,
        path: &Path,
        options: Options,
        capturing: bool,
    ) -> Result<Self, RecordError> {
        let mut session = Session::create(connection).await?;
        match Self::start_stream(&mut session, plan, path, options, capturing).await {
            Ok(started) => Ok(Self {
                session,
                graph: started.graph,
                writer: Some(started.writer),
                path: path.to_path_buf(),
                started: Cell::new(started.at),
                armed: started.at,
                ending: started.ending,
                _watch: started.watch,
                node: started.node,
                target: started.target,
                timings: started.timings,
            }),
            Err(e) => {
                if let Err(stop) = session.stop().await {
                    tracing::warn!("could not stop the session after a failed start: {stop}");
                }
                Err(e)
            }
        }
    }

    async fn start_stream(
        session: &mut Session,
        plan: &GifPlan,
        path: &Path,
        options: Options,
        capturing: bool,
    ) -> Result<Started, RecordError> {
        let began = Instant::now();
        let stream =
            session.record_monitor(&plan.connector, plan.cursor_mode, options.mark_recording).await?;
        let streamed = began.elapsed();
        let node = session.start(&stream).await?;
        let announced = began.elapsed();
        let mut timings = StartTimings { stream: streamed, node: announced - streamed, ..StartTimings::default() };
        let target = match options.targeting {
            Targeting::NodeId => Target::NodeId(node),
            Targeting::Serial => Target::Serial(resolve_serial(node)?),
        };

        let (tx, rx) = std::sync::mpsc::sync_channel(channel_frames(plan.output));
        let graph = Graph::build(plan, target, tx, capturing)?;
        let built = began.elapsed();
        timings.graph = built - announced;
        let recorded = Recorded {
            width: plan.output.width,
            height: plan.output.height,
            fps: plan.fps,
            gif: TrimOptions { quality: plan.quality, fps: None, scale_percent: None },
            end_ms: None,
        };
        let writer = Writer::start(path, recorded, rx)?;
        timings.reel = began.elapsed() - built;

        let ending = Rc::new(Ending::default());
        let watch = {
            let ending = Rc::clone(&ending);
            graph
                .bus()?
                .add_watch_local(move |_, message| {
                    match message.view() {
                        gst::MessageView::Eos(_) => {
                            tracing::debug!("the graph reached EOS");
                            ending.set(Outcome::Eos);
                        }
                        gst::MessageView::Error(e) => {
                            let detail = e.debug().map(|d| format!(" ({d})")).unwrap_or_default();
                            let text = format!("{}{detail}", e.error());
                            tracing::warn!(source = ?e.src().map(|s| s.name()), "graph error: {text}");
                            ending.set(Outcome::Error(text));
                        }
                        gst::MessageView::Warning(w) => {
                            tracing::debug!(source = ?w.src().map(|s| s.name()), "graph warning: {}", w.error());
                        }
                        _ => {}
                    }
                    glib::ControlFlow::Continue
                })
                .map_err(GraphError::Link)?
        };

        // The compositor closing the session under the graph ends the recording with the
        // frames already taken. The handler must NOT touch the pipeline: `Closed` fires
        // from inside `pipewiresrc`'s own teardown (it is what `set_state(Null)` on Stop
        // provokes), and pushing an event back into a `pipewiresrc` that is disconnecting
        // faults inside libgstpipewire -- the segfault that cost this spike a day. So it
        // only records the end and wakes whoever is waiting; `stop`/`drain` then skip the
        // EOS they would otherwise send into a source that is already gone, set the graph
        // to Null and keep what arrived.
        {
            let ending = Rc::clone(&ending);
            session.connect_closed(move || ending.set(Outcome::Eos));
        }

        graph.play()?;
        let at = Instant::now();
        // Every phase, because "the recorder starts slowly" is otherwise one number with
        // five suspects behind it: the compositor's two calls, our own graph, the reel.
        tracing::info!(
            stream_ms = ms(timings.stream),
            node_ms = ms(timings.node),
            graph_ms = ms(timings.graph),
            reel_ms = ms(timings.reel),
            total_ms = ms(timings.total()),
            armed = !capturing,
            "recording start phases"
        );
        tracing::info!(
            node,
            ?target,
            connector = plan.connector,
            fps = plan.fps,
            width = plan.output.width,
            height = plan.output.height,
            path = %path.display(),
            "recording"
        );
        Ok(Started { graph, writer, ending, watch, node, target, at, timings })
    }

    /// Time since the graph started playing.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.started.get().elapsed()
    }

    /// Frames the graph has handed to the reel so far.
    #[must_use]
    pub fn frames_taken(&self) -> u64 {
        self.graph.frames_taken()
    }

    /// How long each phase of the start took, and how long the first frame then took.
    #[must_use]
    pub fn timings(&self) -> (StartTimings, u64) {
        (self.timings, self.graph.first_frame_ms())
    }

    /// True once the stream has ended on its own -- the compositor closed the session,
    /// or the graph failed -- and Stop will not have to wait.
    #[must_use]
    pub fn has_ended(&self) -> bool {
        self.ending.outcome.borrow().is_some()
    }

    /// The reel's directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Records for `duration`, or until the stream ends on its own, then stops.
    pub async fn record_for(self, duration: Duration) -> Result<Kept, RecordError> {
        let (tx, rx) = oneshot::channel::<()>();
        *self.ending.waiter.borrow_mut() = Some(tx);
        if !self.has_ended() {
            match select(rx, glib::timeout_future(duration)).await {
                Either::Left(_) => tracing::info!("the stream ended before the time was up"),
                Either::Right(_) => {}
            }
        }
        self.stop().await
    }

    /// `spec/06` §3 Stop: EOS through the graph, the reel finished, the session stopped,
    /// and the recording's frames and length reported.
    pub async fn stop(mut self) -> Result<Kept, RecordError> {
        let outcome = self.drain().await;
        self.graph.shutdown();
        let writer = self.writer.take();
        if let Err(e) = self.session.stop().await {
            tracing::warn!("could not stop the session: {e}");
        }
        let dropped = self.graph.frames_dropped();
        // Off the main loop: the workers write the frames still queued on threads of their
        // own, and joining them here would freeze every window the app has while they do.
        let kept = match writer {
            Some(writer) => {
                let finished = gio::spawn_blocking(move || writer.finish()).await;
                finished.unwrap_or(Err(ReelError::Panicked))
            }
            None => Err(ReelError::Panicked),
        };
        match (outcome, kept) {
            (Ok(()), Ok(kept)) => {
                tracing::info!(
                    taken = kept.taken,
                    kept = kept.kept,
                    dropped,
                    duration_ms = kept.duration.as_millis(),
                    bytes = kept.bytes,
                    "recorded"
                );
                Ok(kept)
            }
            // A stream that ended badly still left frames, and they are the recording. An
            // EOS that never came or a compositor that closed the session is news for the
            // log, not a reason to throw away what the user recorded (D103); a reel with
            // no frame at all is an error of its own, so this one has some.
            (Err(e), Ok(kept)) => {
                tracing::warn!(
                    taken = kept.taken,
                    dropped,
                    "the stream ended badly; keeping what it recorded: {e}"
                );
                Ok(kept)
            }
            (_, Err(e)) => Err(e.into()),
        }
    }

    /// `spec/06` §3 Trash: everything torn down, the frames removed.
    pub async fn trash(mut self) {
        self.graph.shutdown();
        if let Some(writer) = self.writer.take() {
            writer.abort();
        }
        if let Err(e) = self.session.stop().await {
            tracing::warn!("could not stop the session: {e}");
        }
        tracing::info!(path = %self.path.display(), "recording trashed");
    }

    /// Sends EOS unless the stream has already ended, and waits for the bus to say so.
    async fn drain(&self) -> Result<(), RecordError> {
        if self.ending.outcome.borrow().is_none() {
            let (tx, rx) = oneshot::channel::<()>();
            *self.ending.waiter.borrow_mut() = Some(tx);
            if !self.graph.send_eos() {
                return Err(RecordError::Stream("the graph refused EOS".into()));
            }
            if self.ending.outcome.borrow().is_none()
                && matches!(select(rx, glib::timeout_future(EOS_TIMEOUT)).await, Either::Right(_))
            {
                return Err(RecordError::NoEos(EOS_TIMEOUT));
            }
        }
        match self.ending.outcome.borrow().clone() {
            Some(Outcome::Eos) => Ok(()),
            Some(Outcome::Error(text)) => Err(RecordError::Stream(text)),
            None => Err(RecordError::NoEos(EOS_TIMEOUT)),
        }
    }
}

impl Drop for Recording {
    /// A recording dropped without Stop or Trash is torn down without a reel being
    /// promised; the session goes with the connection if `stop` never ran.
    fn drop(&mut self) {
        self.graph.shutdown();
        if let Some(writer) = self.writer.take() {
            writer.abort();
        }
    }
}

/// A duration as whole milliseconds, for a log line that is read, not parsed.
fn ms(duration: Duration) -> u128 {
    duration.as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D111: the channel is a budget of bytes, so a full-size recording has room for the
    /// disk to stall -- a small one has more room than it will ever use, one too large for
    /// the budget still has the four it always had, and none is so small that its
    /// channel's slots are a budget of their own.
    #[test]
    fn the_frames_waiting_for_the_encoder_are_a_budget_of_bytes() {
        let frames = |width, height| channel_frames(Size { width, height });
        assert_eq!(frames(1363, 836), 117);
        assert_eq!(frames(640, 400), 524);
        assert_eq!(frames(7680, 4320), CHANNEL_FRAMES);
        assert_eq!(frames(64, 40), CHANNEL_MOST);
        assert_eq!(frames(0, 0), CHANNEL_MOST, "a size of nothing does not divide by zero");
    }
}
