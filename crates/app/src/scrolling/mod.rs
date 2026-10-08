// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/07` §1's scrolling capture, as the app runs it.
//!
//! > The app runs the loop so stitching stays in Rust and the shell never blocks.
//!
//! Everything that decides what a frame *means* is in `octosnap-stitch`, and everything
//! that touches the screen is in the extension. What is left -- and what is here -- is the
//! part that is neither: which frames to show the matcher, what to do with one it would
//! not place, and how the result becomes a capture like any other. The user does the
//! scrolling; the loop that scrolled the page for them went with auto-scroll (D153).
//!
//! The one thing worth knowing before reading it: **the extension is told a logical rect
//! and answers with physical pixels**, exactly as an ordinary area capture does. So the
//! step the matcher first expects is in logical pixels and every offset the stitcher
//! reports is in physical rows, and the scale between them is not looked up anywhere --
//! it falls out of the first frame, whose width is the selection's width times it.

pub mod controls;

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use octosnap_core::selection::clamp_to_monitor;
use octosnap_core::{CaptureMode, CaptureResult, Rect, ScrollDirection, SourceWindow};
use octosnap_media::gif::StreamSource;
use octosnap_media::live::Live;
use octosnap_shell::{Placement, ShellBridge};
use octosnap_stitch::{Direction, End, Frame, Limits, Next, Stitcher};
use tracing::{debug, info, warn};

use crate::notify;

/// `spec/07` §1.2: "wait until two consecutive frames are identical (content settled) or
/// 400 ms".
///
/// The first wait is longer than the poll because almost every scroll lands inside it:
/// GTK, GNOME Web and Firefox all animate a wheel scroll over roughly 150 ms, so waiting
/// 120 ms and then confirming with one more frame costs two grabs in the common case
/// instead of five.
const SETTLE_FIRST: Duration = Duration::from_millis(120);
const SETTLE_POLL: Duration = Duration::from_millis(60);
const SETTLE_MAX: Duration = Duration::from_millis(400);

/// `spec/07` §1.1 item 5: the capture "captures after each scroll settles (120 ms idle)".
/// How often it looks, when the page is not moving.
const WATCH_POLL: Duration = Duration::from_millis(120);

/// How often the preview strip is redrawn, at most.
///
/// [P]. Composing it means copying the whole canvas -- 12 ms at 1920x4800 and growing --
/// so redrawing it for every frame would spend more on showing the capture than on making
/// it, and a live view stitches up to [`LIVE_RATE`] frames a second. Four redraws a second
/// reads as continuous. It was "every third accepted frame", which was the same thing at
/// the pace grabs came in.
const PREVIEW_INTERVAL: Duration = Duration::from_millis(250);

/// The most frames a second a live view is asked for (D106).
///
/// [P]. Enough that a page scrolled by hand moves a few dozen rows between two frames --
/// a touchpad flick of 3 000 px a second is a hundred rows at thirty -- and about as many
/// as the loop can stitch: some 25 ms a frame at 1.6 Mpx, optimised.
pub const LIVE_RATE: u32 = 30;

/// How often a live view is looked at for a newer frame. Half a frame interval: looking
/// more often finds nothing new.
const LIVE_POLL: Duration = Duration::from_millis(15);

/// How long a live view has to send its first frame: negotiation, measured at ~150 ms.
const LIVE_FIRST: Duration = Duration::from_secs(2);

/// How often a live view that has sent nothing is given something to paint.
///
/// [P]. Negotiation takes about 150 ms, and a nudge before the stream is flowing is a
/// frame nobody receives; a fifth of a second is one or two tries inside that, and a few
/// more before [`LIVE_FIRST`] gives up.
const LIVE_NUDGE: Duration = Duration::from_millis(200);

/// How often the extension's scroll session is shown a sign of life when nothing else
/// asks it for anything -- a live view, which grabs nothing. Its watchdog ends a session left
/// alone for thirty seconds, puts the pointer back where it was, and takes the outline
/// away mid-capture (`extension/src/scrollAssist.ts`).
const KEEPALIVE: Duration = Duration::from_secs(10);

/// How long frames nobody can place must go on before the pill says the scroll was too
/// big.
///
/// A live view offers a dozen frames in the time one grab took, and one of them caught
/// mid-repaint is not the user's doing; a flick too big is a run of misses that does not
/// end until they scroll back. Said on the first miss, the pill told the user of
/// 2026-09-23 to go back before they had finished their first scroll (D106).
const TROUBLE_AFTER: Duration = Duration::from_millis(600);

/// The preview strip's box, in physical pixels: twice the pill's logical size, so a 2x
/// screen has a sharp one and a 1x screen loses nothing to the scale-down.
const PREVIEW_BOX: (u32, u32) = (264, 336);

/// What a scrolling capture is being asked for.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub direction: ScrollDirection,
    /// How far apart the matcher first expects two frames to be, in logical pixels, before
    /// the user's own scrolls have said. `None` is `spec/07` §1.2's step, 0.6 x the
    /// selection.
    pub step: Option<u32>,
    pub limits: Limits,
}

/// What the capture looks like right now, for `spec/07` §1.1 item 3's preview strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Progress {
    pub width: u32,
    pub height: u32,
    /// How many frames past the first have added to the capture ([`Stitcher::accepted`]).
    pub frames: u32,
    /// `spec/07` §1.1 item 4's "very long" warning.
    pub long: bool,
    /// The last frame could not be placed: a flick too big to match.
    pub trouble: bool,
}

impl Progress {
    #[must_use]
    const fn with(self, trouble: bool) -> Self {
        Self { trouble, ..self }
    }
}

/// What the user can do to a capture in flight.
///
/// `spec/07` §1.1 item 2's **Pause** was here too, for an auto-scroll to hold; it went with
/// auto-scroll (D153). A capture the user scrolls is paused by their not scrolling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Command {
    #[default]
    Run,
    /// **Done**: stop and keep what is stitched.
    Done,
    /// **Cancel**: stop and keep nothing.
    Cancel,
}

/// The handle the controls hold, and the loop reads.
///
/// A cell rather than a channel because there is nothing to queue: the loop only ever
/// wants the *latest* thing the user asked for.
#[derive(Debug, Clone, Default)]
pub struct Control(Rc<Cell<Command>>);

impl Control {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, command: Command) {
        self.0.set(command);
    }

    #[must_use]
    pub fn get(&self) -> Command {
        self.0.get()
    }
}

/// A finished scrolling capture, before it is a file.
#[derive(Debug)]
pub struct Shot {
    pub frame: Frame,
    /// Why it stopped, or `None` when the user pressed Done.
    pub ended: Option<End>,
    /// The physical pixels one logical pixel of the selection turned out to be.
    pub scale: f64,
}

/// Frames as the compositor paints them (D106): what the media crate's live view is to the
/// loop, and what a test can be instead.
pub trait Stream {
    /// Grows by one with every frame that arrives. Cheap enough to poll.
    fn serial(&self) -> u64;
    /// The newest frame and the serial it arrived with, or `None` before the first.
    fn newest(&self) -> impl std::future::Future<Output = Option<(u64, Frame)>>;
    /// True once no more frames will come.
    fn ended(&self) -> bool;
}

/// No stream at all: the type [`run`] names for the capture that grabs its frames instead.
/// It has no values, so it can never be asked for one.
#[derive(Debug)]
pub enum Grabbed {}

impl Stream for Grabbed {
    fn serial(&self) -> u64 {
        match *self {}
    }

    fn newest(&self) -> impl std::future::Future<Output = Option<(u64, Frame)>> {
        std::future::ready(None)
    }

    fn ended(&self) -> bool {
        match *self {}
    }
}

/// Runs one scrolling capture from start to finish, from frames the extension grabs.
///
/// The session is ended on every path out, including the ones that carry an error: the
/// extension is holding a virtual pointer device and has parked the real pointer inside
/// the selection, and leaving either behind is the one way this code can make the desktop
/// worse than it found it.
pub async fn run<B: ShellBridge>(
    bridge: &B,
    rect: Rect,
    options: Options,
    control: &Control,
    progress: impl FnMut(Progress, Option<Frame>),
) -> Result<Shot, String> {
    run_with::<B, Grabbed>(bridge, None, rect, options, control, progress).await
}

/// [`run`], from a live view of the selection instead of the extension's grabs (D106).
///
/// The extension's session is still started and ended: it draws the outline that says
/// which rectangle is being captured, and it parks the pointer. Only the frames come from
/// the stream.
pub async fn run_live<B: ShellBridge, S: Stream>(
    bridge: &B,
    stream: &S,
    rect: Rect,
    options: Options,
    control: &Control,
    progress: impl FnMut(Progress, Option<Frame>),
) -> Result<Shot, String> {
    run_with(bridge, Some(stream), rect, options, control, progress).await
}

async fn run_with<B: ShellBridge, S: Stream>(
    bridge: &B,
    stream: Option<&S>,
    rect: Rect,
    options: Options,
    control: &Control,
    mut progress: impl FnMut(Progress, Option<Frame>),
) -> Result<Shot, String> {
    let step = options.step.unwrap_or_else(|| octosnap_stitch::step_for(along(rect, options.direction)));
    let handle = bridge
        .start_scroll_assist(rect, options.direction)
        .await
        .map_err(|e| format!("the shell would not start a scrolling capture: {e}"))?;
    let (direction, live) = (options.direction.as_wire(), stream.is_some());
    info!(%handle, step, direction, live, "scrolling capture begins");
    let source = match stream {
        Some(stream) => Source::Live {
            stream,
            bridge,
            handle: &handle,
            taken: Cell::new(0),
            touched: Cell::new(Instant::now()),
        },
        None => Source::Grabs { bridge, handle: &handle },
    };
    let outcome = drive(&source, rect, options, step, control, &mut progress).await;
    if let Err(e) = bridge.end_scroll_assist(&handle).await {
        warn!(%handle, "the scroll session would not end cleanly: {e}");
    }
    outcome
}

/// Where the loop's frames come from.
enum Source<'a, B, S> {
    /// The extension's `GrabFrame`: a PNG a frame, a third of a second each at 1.6 Mpx.
    Grabs { bridge: &'a B, handle: &'a str },
    /// A live view of the selection (D106), and the scroll session it keeps alive.
    Live {
        stream: &'a S,
        bridge: &'a B,
        handle: &'a str,
        /// The serial of the last frame taken off the stream.
        taken: Cell<u64>,
        /// When the scroll session was last shown a sign of life.
        touched: Cell<Instant>,
    },
}

impl<B: ShellBridge, S: Stream> Source<'_, B, S> {
    const fn live(&self) -> bool {
        matches!(self, Self::Live { .. })
    }

    /// True once a live view has stopped for good. Grabs never stop by themselves.
    fn ended(&self) -> bool {
        match self {
            Self::Grabs { .. } => false,
            Self::Live { stream, .. } => stream.ended(),
        }
    }

    /// The frame the capture starts from.
    async fn first(&self) -> Result<Frame, String> {
        match self {
            Self::Grabs { bridge, handle } => grab(*bridge, handle).await,
            Self::Live { stream, taken, .. } => {
                let deadline = Instant::now() + LIVE_FIRST;
                loop {
                    if let Some((serial, frame)) = stream.newest().await {
                        taken.set(serial);
                        return Ok(frame);
                    }
                    if stream.ended() || Instant::now() >= deadline {
                        return Err("the live view of the selection sent no frame".to_owned());
                    }
                    glib::timeout_future(LIVE_POLL).await;
                }
            }
        }
    }

    /// The next frame that differs from `last` -- or `None` when there is none yet, so the
    /// caller can look at its controls again.
    ///
    /// The loop watches rather than drives, so an unchanged frame is a user who has not
    /// scrolled yet -- not a page that has ended -- and there is nothing to ask the
    /// matcher about it. Bytes, and only bytes: a frame that differs by a caret or a
    /// highlight goes through to the matcher like any other, because the matcher is the
    /// one thing here that can tell a page that moved from one that did not, and a cheaper
    /// guess in front of it was tried and measured out (D98).
    async fn watch(&self, last: &Frame) -> Result<Option<Frame>, String> {
        match self {
            Self::Grabs { bridge, handle } => {
                let frame = settle(*bridge, handle).await?;
                if frame == *last {
                    glib::timeout_future(WATCH_POLL).await;
                    return Ok(None);
                }
                Ok(Some(frame))
            }
            Self::Live { stream, bridge, handle, taken, touched } => {
                if touched.get().elapsed() >= KEEPALIVE {
                    touched.set(Instant::now());
                    keep_alive(*bridge, handle).await;
                }
                let deadline = Instant::now() + WATCH_POLL;
                while stream.serial() == taken.get() {
                    if stream.ended() || Instant::now() >= deadline {
                        return Ok(None);
                    }
                    glib::timeout_future(LIVE_POLL).await;
                }
                let Some((serial, frame)) = stream.newest().await else {
                    return Ok(None);
                };
                taken.set(serial);
                Ok((frame != *last).then_some(frame))
            }
        }
    }
}

/// A grab thrown away, as the scroll session's sign of life (see [`KEEPALIVE`]).
async fn keep_alive<B: ShellBridge>(bridge: &B, handle: &str) {
    match bridge.grab_frame(handle).await {
        Ok(path) => {
            if let Err(e) = std::fs::remove_file(&path) {
                debug!(path = %path.display(), "a keep-alive frame outlived its grab: {e}");
            }
        }
        Err(e) => debug!("the scroll session's keep-alive was refused: {e}"),
    }
}

async fn drive<B: ShellBridge, S: Stream>(
    source: &Source<'_, B, S>,
    rect: Rect,
    options: Options,
    step: u32,
    control: &Control,
    progress: &mut impl FnMut(Progress, Option<Frame>),
) -> Result<Shot, String> {
    // Somebody scrolling a page by hand goes whichever way it will go, and a conversation
    // standing at its foot only goes up (D112).
    let mut stitcher = Stitcher::new(facing(options.direction), options.limits).both_ways();
    let first = source.first().await?;
    // A live view's frames are never files, so `OCTOSNAP_KEEP_FRAMES` keeps the ones the
    // matcher is shown here instead -- numbered, the first one 0000.
    let kept = keep_frames().filter(|_| source.live());
    keep(kept.as_deref(), 0, &first).await;
    // The scale the extension captured at, which nothing has to be asked for: the frame is
    // the selection in physical pixels, so its size over the selection's *is* the scale.
    let scale = scale_of(&first, rect, options.direction);
    let hint = ((f64::from(step) * scale).round() as u32).max(1);
    debug!(scale, hint, width = first.width(), height = first.height(), "the first frame");
    let mut settled = first.clone();
    let (next_stitcher, _, preview) = push(stitcher, first, hint, true).await?;
    stitcher = next_stitcher;
    progress(report(&stitcher), preview);
    let mut shown = Instant::now();

    let mut trouble = false;
    let mut missing: Option<Instant> = None;
    // Frames the matcher was shown, accepted or not -- the denominator `accepted` needs.
    let mut offered = 0u32;
    let mut ended = None;
    loop {
        match control.get() {
            Command::Run => {}
            Command::Done => break,
            Command::Cancel => return Err("cancelled".to_owned()),
        }
        if source.ended() {
            // The compositor closed the stream under the capture: the monitor went away,
            // or the shell restarted. What is stitched is still a capture.
            warn!("the live view ended; keeping what was stitched");
            break;
        }
        let Some(frame) = source.watch(&settled).await? else { continue };
        settled = frame.clone();
        keep(kept.as_deref(), offered + 1, &frame).await;
        let wanted = shown.elapsed() >= PREVIEW_INTERVAL;
        // Nothing here knows how far the page went, and the step is a poor guess: a user
        // flicking a wheel repeats their own flick, not 0.6 of the selection. So the last
        // accepted distance is the hint once there is one -- which is what
        // `Stitcher::push` means by "manual mode passes `growth`". Before there is one it
        // stays the step, smaller as a live view's first scroll usually is: the hint is
        // also what a sticky run read off the first pair is checked against, and a quarter
        // of the step let an hour grid's false one through and clipped the search below
        // the answer (D94, found again by its test).
        let expected = if stitcher.growth() > 0 { stitcher.growth() } else { hint };
        let (next_stitcher, next, preview) = push(stitcher, frame, expected, wanted).await?;
        stitcher = next_stitcher;
        offered += 1;
        if preview.is_some() {
            shown = Instant::now();
        }
        let grew = matches!(next, Next::Scroll);
        let (wide, tall) = stitcher.size();
        debug!(?next, wide, tall, accepted = stitcher.accepted(), "a frame was offered");
        match next {
            Next::Scroll => {}
            Next::Smaller | Next::Stop(End::Unmatched) => {
                // Nothing here can retry: the user has already scrolled, and the only
                // thing that makes the next frame placeable is them scrolling back into
                // the overlap. So the pill says so -- once the misses have gone on long
                // enough to be a flick too big and not one frame caught mid-repaint -- and
                // the capture keeps waiting: frames too big are not a reason to close a
                // capture somebody is driving.
                let since = *missing.get_or_insert_with(Instant::now);
                trouble = since.elapsed() >= TROUBLE_AFTER;
                if !source.live() {
                    glib::timeout_future(WATCH_POLL).await;
                }
            }
            // "The page did not move" is the ordinary state of a capture that is waiting
            // for the user, and it reaches here whenever a frame differs without the page
            // having scrolled -- a caret blinking, a row lit up under the pointer, the
            // pill's own preview changing inside the selection. Ending on it would close
            // the capture while the user was reading (D76), and *saying* it -- D94's "that
            // looks like the end of the page" -- told a user who had paused to read that
            // the page was over (D98). A stopped page and a reading user are the same
            // picture to the matcher, so this verdict means nothing here and nothing is
            // done with it. The frame count that stops growing is what the pill has to say
            // about it; only the size cap stops a capture the user is driving, and
            // `spec/07` §1.1 item 2's Done does the rest.
            Next::Stop(End::Content) => {
                if !source.live() {
                    glib::timeout_future(WATCH_POLL).await;
                }
            }
            Next::Stop(end) => {
                ended = Some(end);
                break;
            }
        }
        if grew {
            trouble = false;
            missing = None;
        }
        progress(report(&stitcher).with(trouble), preview.filter(|_| grew));
    }

    let (width, height) = stitcher.size();
    let frames = stitcher.accepted();
    info!(width, height, frames, offered, ?ended, "scrolling capture ends");
    // A capture that grabbed frames and stitched none of them is the shape of D94's
    // report, and the numbers that say why are all here rather than in a debug line
    // nobody had turned on: what the mask decided, and what the matcher was told to
    // expect. `OCTOSNAP_KEEP_FRAMES` keeps the pixels themselves.
    if frames == 0 && offered > 0 {
        warn!(
            offered,
            fixed = ?stitcher.fixed(),
            hint,
            "not one frame of this capture could be placed"
        );
    }
    let frame = stitcher.finish().map_err(|e| e.to_string())?;
    Ok(Shot { frame, ended, scale })
}

/// `spec/07` §1.2: "wait until two consecutive frames are identical (content settled) or
/// 400 ms".
async fn settle<B: ShellBridge>(bridge: &B, handle: &str) -> Result<Frame, String> {
    glib::timeout_future(SETTLE_FIRST).await;
    let mut last = grab(bridge, handle).await?;
    let deadline = Instant::now() + SETTLE_MAX;
    while Instant::now() < deadline {
        glib::timeout_future(SETTLE_POLL).await;
        let next = grab(bridge, handle).await?;
        if next == last {
            return Ok(next);
        }
        last = next;
    }
    debug!("the page was still moving after the settle window");
    Ok(last)
}

/// One frame, read and then deleted.
///
/// Deleted here rather than at the end of the session, because the frames land in
/// `$XDG_RUNTIME_DIR` -- which on every systemd machine is a tmpfs, so a hundred frames of
/// a 4K selection is a gigabyte of *memory*. The extension removes the directory when the
/// session ends; this keeps it from filling in the meantime.
async fn grab<B: ShellBridge>(bridge: &B, handle: &str) -> Result<Frame, String> {
    let path = bridge
        .grab_frame(handle)
        .await
        .map_err(|e| format!("the shell would not grab a frame: {e}"))?;
    read(path).await
}

async fn read(path: PathBuf) -> Result<Frame, String> {
    let keep = keep_frames();
    gio::spawn_blocking(move || {
        let frame = Frame::read(&path).map_err(|e| e.to_string());
        if let Some(into) = keep {
            // A matcher that refuses a real page cannot be argued with from a log line:
            // D94 was found by reading the pixels and nothing else would have found it.
            // So there is a way to keep them, off by default because of what the doc
            // comment above says about tmpfs.
            let name = path.file_name().unwrap_or(std::ffi::OsStr::new("frame.png"));
            match std::fs::create_dir_all(&into).and_then(|()| std::fs::copy(&path, into.join(name)))
            {
                Ok(_) => info!(path = %into.join(name).display(), "kept a scroll frame"),
                Err(e) => warn!(into = %into.display(), "could not keep a scroll frame: {e}"),
            }
        }
        if let Err(e) = std::fs::remove_file(&path) {
            debug!(path = %path.display(), "a scroll frame outlived its read: {e}");
        }
        frame
    })
    .await
    .map_err(|_| "the frame reader did not come back".to_owned())?
}

/// Where to keep every grabbed frame, when somebody is debugging the matcher.
///
/// `OCTOSNAP_KEEP_FRAMES=/some/directory`. Not a setting, because it is not a thing a
/// user wants -- it is the one way to answer "why would it not place this page", and the
/// only honest answer to that question is the pixels the matcher saw.
fn keep_frames() -> Option<PathBuf> {
    std::env::var_os("OCTOSNAP_KEEP_FRAMES").map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())
}

/// Writes `frame` into `dir` as the `index`th frame the matcher was shown, when there is a
/// `dir` ([`keep_frames`]); off the main loop, because a PNG of a 4K frame is a quarter of
/// a second.
async fn keep(dir: Option<&Path>, index: u32, frame: &Frame) {
    let Some(dir) = dir else { return };
    let (path, frame) = (dir.join(format!("{index:04}.png")), frame.clone());
    let written = gio::spawn_blocking(move || frame.write(&path).map(|()| path)).await;
    match written {
        Ok(Ok(path)) => debug!(path = %path.display(), "a live frame kept"),
        Ok(Err(e)) => warn!("could not keep a live frame: {e}"),
        Err(_) => warn!("the frame writer did not come back"),
    }
}

/// One stitch, off the main loop.
///
/// The match is ~16 ms at 1920x1200 (`spec/10` §7 budgets 120 ms for the whole iteration),
/// which is a dropped frame of the preview strip every step if it runs here. The stitcher
/// travels to the worker and back rather than living behind a lock, because there is
/// exactly one of it and exactly one thing using it at a time.
async fn push(
    mut stitcher: Stitcher,
    frame: Frame,
    hint: u32,
    preview: bool,
) -> Result<(Stitcher, Next, Option<Frame>), String> {
    let joined = gio::spawn_blocking(move || {
        let next = stitcher.push(frame, hint);
        // Composed here too, on the same trip: the canvas is already warm in this
        // thread's cache and the main loop never sees either copy. The end shown is the
        // end that grew, which a capture driven by hand can turn round (D112).
        let thumb = preview
            .then(|| stitcher.composed())
            .flatten()
            .map(|canvas| canvas.strip(PREVIEW_BOX.0, PREVIEW_BOX.1, stitcher.growing()));
        (stitcher, next, thumb)
    })
    .await
    .map_err(|_| "the stitcher did not come back".to_owned())?;
    let (stitcher, next, thumb) = joined;
    Ok((stitcher, next.map_err(|e| e.to_string())?, thumb))
}

fn report(stitcher: &Stitcher) -> Progress {
    let (width, height) = stitcher.size();
    Progress {
        width,
        height,
        frames: stitcher.accepted(),
        long: stitcher.long(),
        trouble: false,
    }
}

/// The selection's length along the scroll, which is what the step is a fraction of.
const fn along(rect: Rect, direction: ScrollDirection) -> u32 {
    let length = if direction.horizontal() { rect.width } else { rect.height };
    length.unsigned_abs()
}

const fn facing(direction: ScrollDirection) -> Direction {
    match direction {
        ScrollDirection::Down => Direction::Down,
        ScrollDirection::Up => Direction::Up,
        ScrollDirection::Right => Direction::Right,
        ScrollDirection::Left => Direction::Left,
    }
}

/// The physical pixels one logical pixel of the selection turned out to be.
fn scale_of(frame: &Frame, rect: Rect, direction: ScrollDirection) -> f64 {
    let (physical, logical) = if direction.horizontal() {
        (frame.height(), rect.height.unsigned_abs())
    } else {
        (frame.width(), rect.width.unsigned_abs())
    };
    if logical == 0 { 1.0 } else { f64::from(physical) / f64::from(logical) }
}

/// The stitched result as a capture, filed in the spool with its twin.
///
/// The same shape a recording takes (`recording::build_capture`): the file is the app's,
/// the twin describes it, and everything downstream -- the card, the editor, the history,
/// a save -- treats it as it treats a screenshot, which is the point of `spec/07` §1.1's
/// "the stitched image goes through the normal post-capture path".
pub fn file(shot: &Shot, rect: Rect, display: String) -> Result<CaptureResult, String> {
    let spool = crate::history::History::default_spool();
    std::fs::create_dir_all(&spool)
        .map_err(|e| format!("the spool {} could not be created: {e}", spool.display()))?;
    let path = spool.join(format!("{}.png", octosnap_core::capture::fresh_id()));
    shot.frame.write(&path).map_err(|e| format!("the capture could not be written: {e}"))?;
    let now = glib::real_time().unsigned_abs();
    let capture = CaptureResult {
        meta_path: path.with_extension("json"),
        path,
        mode: CaptureMode::Scrolling,
        // The selection's logical origin, with the length the capture actually grew to:
        // a card, a pin and a restore all measure from this, and a scrolling capture is
        // taller than the rect it was taken through.
        rect: grown(rect, shot),
        scale: shot.scale,
        display,
        cursor_rect: None,
        source_window: SourceWindow::default(),
        window_alpha: false,
        timestamp: now,
        confirmed_at: Some(now),
        animation_ms: 0,
        requested_action: None,
        modifiers: 0,
        duration_ms: None,
        external: false,
        linebreaks: None,
        project: None,
    };
    match serde_json::to_vec_pretty(&capture) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(&capture.meta_path, bytes) {
                warn!(path = %capture.meta_path.display(), "could not write the twin: {e}");
            }
        }
        Err(e) => warn!("could not serialise the twin: {e}"),
    }
    Ok(capture)
}

/// The selection, grown to the size the capture reached.
fn grown(rect: Rect, shot: &Shot) -> Rect {
    let logical = |physical: u32| {
        if shot.scale <= 0.0 {
            i32::try_from(physical).unwrap_or(i32::MAX)
        } else {
            (f64::from(physical) / shot.scale).round() as i32
        }
    };
    Rect::new(rect.x, rect.y, logical(shot.frame.width()), logical(shot.frame.height()))
}

/// The `scroll-capture` action's payload: a rectangle, and what to do with it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Request {
    pub rect: Rect,
    pub direction: ScrollDirection,
    /// `spec/07`'s URL parameter `start`: begin without waiting for the button.
    pub start: bool,
}

impl Request {
    /// Parses the `a{sv}` the extension, the CLI and the URL all send.
    ///
    /// An absent key leaves the setting alone, exactly as `BeginCapture`'s options do
    /// (`spec/10` §3.1): a script that says nothing about the direction should get the
    /// user's, not the wire's default. An `autoscroll` or a `scroll-mode` from a caller
    /// older than D153 is not read: the user scrolls.
    #[must_use]
    pub fn from_variant(payload: &glib::Variant) -> Option<Self> {
        use octosnap_shell::variant;
        let dict = glib::VariantDict::new(Some(payload));
        let rect = variant::rect(&dict, "rect")?;
        let direction = variant::string(&dict, "direction")
            .and_then(|value| ScrollDirection::from_wire(&value))
            .unwrap_or_default();
        Some(Self { rect, direction, start: variant::bool_(&dict, "start").unwrap_or(false) })
    }
}

/// One scrolling capture in flight, and the pill that drives it.
#[derive(Debug)]
struct Active {
    pill: Rc<controls::Pill>,
    control: Control,
    rect: Rect,
    display: String,
    direction: Cell<ScrollDirection>,
    /// Whether the pill had to go inside the selection, where it is in every frame.
    in_shot: bool,
    /// Bumped by every start, so a loop that finishes after the user has cancelled and
    /// started another one finds a generation that is not its own and files nothing.
    run: u64,
}

/// The app's half of `spec/07` §1: one capture at a time, its controls, and the loop.
#[derive(Debug)]
pub struct Scroller {
    app: adw::Application,
    flow: crate::SharedFlow,
    active: std::cell::RefCell<Option<Active>>,
    runs: Cell<u64>,
}

/// How far the pill sits from the selection it belongs to.
const PILL_GAP: i32 = 12;

/// How long a card stepped aside is given to leave the screen before the first frame.
///
/// [P]. A window hidden is gone at the compositor's next frame, which is 17 ms at 60 Hz
/// and 33 ms at 30; two of the slower is room for a busy one.
const ASIDE_SETTLE: Duration = Duration::from_millis(70);

impl Scroller {
    #[must_use]
    pub fn new(app: &adw::Application, flow: crate::SharedFlow) -> Rc<Self> {
        Rc::new(Self {
            app: app.clone(),
            flow,
            active: std::cell::RefCell::new(None),
            runs: Cell::new(0),
        })
    }

    /// True while a capture is up, so a second request can be refused rather than queued.
    #[must_use]
    pub fn is_scrolling(&self) -> bool {
        self.active.borrow().is_some()
    }

    /// `spec/07` §1.1 items 1 and 2: the selection is made, so show its controls.
    ///
    /// Nothing scrolls and nothing is grabbed until Start. The one exception is a request
    /// that says `start`, which is the URL API's way of skipping the button.
    pub async fn begin(self: &Rc<Self>, request: Request) {
        if self.is_scrolling() {
            warn!("a scrolling capture is already running");
            return;
        }
        let display = self.display_for(request.rect).await;
        let (pill, in_shot) = self.build_pill(request).await;
        let run = self.runs.get() + 1;
        self.runs.set(run);
        *self.active.borrow_mut() = Some(Active {
            pill,
            control: Control::new(),
            rect: request.rect,
            display,
            direction: Cell::new(request.direction),
            in_shot,
            run,
        });
        info!(rect = ?request.rect, direction = request.direction.as_wire(), "scrolling capture ready");
        if let Some(active) = self.active.borrow().as_ref() {
            active.pill.log_layout("ready");
        }
        if request.start {
            self.ask(controls::Ask::Start);
        }
    }

    /// What the pill's buttons mean.
    pub fn ask(self: &Rc<Self>, ask: controls::Ask) {
        let Some(active) = self.active.borrow().as_ref().map(|a| (a.control.clone(), Rc::clone(&a.pill))) else {
            return;
        };
        let (control, pill) = active;
        match ask {
            controls::Ask::Start => {
                if pill.is_running() {
                    return;
                }
                pill.started();
                self.start();
            }
            controls::Ask::Done if pill.is_running() => control.set(Command::Done),
            // Done is not on the pill until Start has been pressed.
            controls::Ask::Done => {}
            // Asked twice already, if a capture was running: the pill does that half.
            controls::Ask::Cancel => {
                info!(running = pill.is_running(), "the scrolling capture was cancelled");
                control.set(Command::Cancel);
                self.close();
            }
            // Through the action rather than straight to `set_direction`, so its state --
            // which anything else that shows the direction reads -- says the same.
            controls::Ask::Direction(way) => {
                self.app.activate_action("scroll-direction", Some(&way.as_wire().to_variant()));
            }
        }
    }

    /// The pill's button, pressed from the keyboard (D137): Start, and Done once the
    /// capture runs, as the one button the pill shows in each state.
    pub fn advance(self: &Rc<Self>) {
        let running = match self.active.borrow().as_ref() {
            Some(active) => active.pill.is_running(),
            None => {
                debug!("scroll-advance with no scrolling capture up");
                return;
            }
        };
        self.ask(if running { controls::Ask::Done } else { controls::Ask::Start });
    }

    /// Changes the direction before the capture starts.
    pub fn set_direction(&self, direction: ScrollDirection) {
        if let Some(active) = self.active.borrow().as_ref() {
            if active.pill.is_running() {
                return;
            }
            active.direction.set(direction);
            active.pill.show_direction(direction);
        }
    }

    /// Runs the loop, and files what it produces.
    fn start(self: &Rc<Self>) {
        let Some((rect, direction, display, control, pill, generation, in_shot)) =
            self.active.borrow().as_ref().map(|a| {
                let (pill, control) = (Rc::clone(&a.pill), a.control.clone());
                (a.rect, a.direction.get(), a.display.clone(), control, pill, a.run, a.in_shot)
            })
        else {
            return;
        };
        if in_shot {
            info!("the scrolling controls are in the shot; they hold still until it ends");
        }
        let options = Options { direction, ..Options::default() };
        let scroller = Rc::downgrade(self);
        let bridge = self.flow.bridge().clone();
        let app = self.app.clone();
        glib::spawn_future_local(async move {
            let shown = Rc::downgrade(&pill);
            let report = move |progress: Progress, preview: Option<Frame>| {
                // A pill in the shot is in every frame, and one that redraws itself is a
                // patch that changes while the page scrolls under it -- which, over a page
                // with little else in it, can cost a match (D105). Held still, it is a
                // banner like any other, and the matcher leaves it out.
                if in_shot {
                    return;
                }
                let Some(pill) = shown.upgrade() else { return };
                pill.set_trouble(progress.trouble);
                pill.show(
                    preview.as_ref().and_then(texture_of).as_ref(),
                    progress.width,
                    progress.height,
                    progress.long,
                );
            };
            // Our own cards off the page first: they stand where new rows come from.
            let overlay = crate::overlay();
            if overlay.as_ref().is_some_and(|overlay| overlay.step_aside(rect)) {
                glib::timeout_future(ASIDE_SETTLE).await;
            }
            let nudge = {
                let pill = Rc::downgrade(&pill);
                move || {
                    if let Some(pill) = pill.upgrade() {
                        pill.nudge();
                    }
                }
            };
            let live = open_live(&app, &bridge, rect, &nudge).await;
            pill.settle();
            let outcome = match &live {
                Some(stream) => run_live(&bridge, stream, rect, options, &control, report).await,
                None => run(&bridge, rect, options, &control, report).await,
            };
            if let Some(stream) = live {
                stream.0.stop().await;
            }
            if let Some(overlay) = overlay {
                overlay.step_back();
            }
            let Some(scroller) = scroller.upgrade() else { return };
            // A capture the user cancelled, or a second one begun while this ran: the
            // generation says which, and neither should file anything.
            if scroller.active.borrow().as_ref().is_none_or(|a| a.run != generation) {
                return;
            }
            scroller.close();
            match outcome {
                Ok(shot) => scroller.finish(shot, rect, display).await,
                Err(message) if message == "cancelled" => info!("scrolling capture cancelled"),
                Err(message) => {
                    warn!("scrolling capture failed: {message}");
                    notify::tell(&scroller.app, "scrolling", "Scrolling capture failed", &message, true);
                }
            }
        });
    }

    /// The stitched result through `spec/07` §1.1 item 4's "normal post-capture path".
    async fn finish(self: &Rc<Self>, shot: Shot, rect: Rect, display: String) {
        let (width, height) = (shot.frame.width(), shot.frame.height());
        match file(&shot, rect, display) {
            Ok(capture) => {
                info!(
                    path = %capture.path.display(),
                    width, height, ended = ?shot.ended,
                    "scrolling capture filed"
                );
                // `spec/07` §1.1 item 4: "a warning appears when the result is very long".
                // Told after the fact rather than during, because it is not a reason to
                // stop -- a 30 000 px changelog is a real thing to capture -- and the pill
                // has already said so in yellow while it grew.
                // A capture that stopped on its own was cut short, which the user has to
                // hear about; one that is merely long gets the tray.
                if let Some(reason) = ending(shot.ended, width.max(height)) {
                    notify::tell(&self.app, "scrolling", "Scrolling capture", &reason, shot.ended.is_some());
                }
                let outcome = self.flow.handle(&capture).await;
                // `ACT-05`, as for a screenshot: a plan with no card has nothing on screen.
                notify::capture_outcome(
                    &self.app,
                    &outcome,
                    crate::settings::Settings::load().notifications_enabled(),
                );
            }
            Err(message) => {
                warn!("the scrolling capture could not be filed: {message}");
                notify::tell(&self.app, "scrolling", "Scrolling capture failed", &message, true);
            }
        }
    }

    /// Takes the pill away and forgets the capture.
    ///
    /// The capture is taken out first and the pill closed after, with nothing borrowed:
    /// a window going away can tell its handlers the pointer left, and they ask this.
    fn close(&self) {
        let active = self.active.borrow_mut().take();
        if let Some(active) = active {
            active.pill.close();
        }
    }

    async fn build_pill(self: &Rc<Self>, request: Request) -> (Rc<controls::Pill>, bool) {
        let scroller = Rc::downgrade(self);
        let pill = controls::Pill::new(&self.app, request.direction, move |ask| {
            if let Some(scroller) = scroller.upgrade() {
                scroller.ask(ask);
            }
        });
        // The direction action outlives the pill, so it still holds whatever the last
        // capture was set to; this one starts from its own request's.
        if let Some(action) = self.app.lookup_action("scroll-direction").and_downcast::<gio::SimpleAction>() {
            action.set_state(&request.direction.as_wire().to_variant());
        }

        let in_shot = if let Some(path) = pill.object_path() {
            let (in_shot, landed) = self.perch(&path, request.rect, pill.size(), request.direction).await;
            // Help's panel opens off the selection, which needs to know where the pill is.
            if let Some((landed, work_area)) = landed {
                pill.set_room(landed, work_area, request.rect);
            }
            // The selection outlined from now rather than from Start: once the overlay has
            // closed, nothing else says which rectangle the pill is for (D152). The outline
            // goes with the pill's window. An extension from before it has no such call,
            // and the capture is what it was, outlined from Start.
            if let Err(e) = self.flow.bridge().show_scroll_frame(&path, request.rect).await {
                debug!("the selection is not outlined before Start: {e}");
            }
            in_shot
        } else {
            warn!("the scrolling controls have no object path; they cannot be placed");
            false
        };
        (pill, in_shot)
    }

    /// Puts the pill somewhere it will not be in the shot, and wholly on a screen.
    ///
    /// Every placement is computed against the monitors' work areas *before* it is asked
    /// for, and then read back: the app knows what it built but not what Mutter gave it,
    /// and `PlaceWindow` answers with the rect the window **landed** in. Until 2026-09-23
    /// only the second placement was computed; the first was "beside the selection" taken
    /// on trust, and on a laptop panel under two monitors that sent the pill to x = 1778
    /// on a 1920 px panel with nothing to its right. It landed there -- outside the
    /// selection, so nothing checked it further -- with half of it off the edge of the
    /// desktop (D104).
    ///
    /// True when the pill ended up in the shot after all, which only a selection that
    /// leaves no room on any screen does: the loop then holds the pill still (D105). With
    /// it, where the pill landed and the work area that wholly holds it, once it has.
    async fn perch(
        self: &Rc<Self>,
        path: &str,
        rect: Rect,
        pill: (i32, i32),
        direction: ScrollDirection,
    ) -> (bool, Option<(Rect, Rect)>) {
        let bridge = self.flow.bridge();
        let monitors = bridge.monitors().await.unwrap_or_default();
        let screens: Vec<Rect> = monitors.iter().map(|monitor| monitor.work_area).collect();
        let mut size = pill;
        let mut remeasured = false;
        let mut candidates = spots(rect, size, &screens, direction);
        let mut next = 0;
        let mut in_shot = false;
        // Whether every candidate is in the shot, which is [`inside`]'s list: there the
        // first that lands wholly on a screen is the answer, and trying on would only put
        // the pill in the last corner of the list, the worst one (2026-09-23).
        let inside_only = |candidates: &[(i32, i32)], size: (i32, i32)| {
            candidates.iter().all(|&(x, y)| Rect::new(x, y, size.0, size.1).overlap_area(rect) > 0)
        };
        let mut settle_inside = inside_only(&candidates, size);
        while let Some(&(x, y)) = candidates.get(next) {
            next += 1;
            let placement = Placement::At { x, y };
            let landed = match bridge.place_window(path, "scroller", &placement, 0).await {
                Ok(landed) => landed,
                Err(e) => {
                    warn!("could not place the scrolling controls: {e}");
                    return (in_shot, None);
                }
            };
            in_shot = landed.overlap_area(rect) > 0;
            if (!in_shot || settle_inside) && wholly_on_a_screen(landed, &screens) {
                if in_shot {
                    info!(?landed, "scrolling controls placed in the shot; nowhere else fits");
                } else {
                    info!(?landed, "scrolling controls placed");
                }
                let area = i64::from(landed.width) * i64::from(landed.height);
                let work_area = screens.iter().copied().find(|screen| landed.overlap_area(*screen) == area);
                return (in_shot, work_area.map(|work_area| (landed, work_area)));
            }
            // The compositor's size is the one that is drawn. A pill it reports bigger
            // than GTK measured is placed again from that size, once -- measured too big
            // only puts it further out, measured too small puts it back into the shot.
            if !remeasured && (landed.width > size.0 || landed.height > size.1) {
                remeasured = true;
                size = (landed.width.max(size.0), landed.height.max(size.1));
                candidates = spots(rect, size, &screens, direction);
                settle_inside = inside_only(&candidates, size);
                next = 0;
                continue;
            }
            debug!(?landed, "the compositor put the controls somewhere they should not be");
        }
        warn!(?rect, "the selection leaves the scrolling controls nowhere wholly outside it");
        (in_shot, None)
    }

    /// The monitor the selection sits on, for the capture's record.
    async fn display_for(&self, rect: Rect) -> String {
        let Ok(monitors) = self.flow.bridge().monitors().await else { return String::new() };
        monitors
            .iter()
            .find(|m| {
                let (cx, cy) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
                cx >= m.geometry.x
                    && cx < m.geometry.x + m.geometry.width
                    && cy >= m.geometry.y
                    && cy < m.geometry.y + m.geometry.height
            })
            .or_else(|| monitors.iter().find(|m| m.current))
            .or_else(|| monitors.first())
            .map(|m| m.connector.clone())
            .unwrap_or_default()
    }

}

/// Where the pill can sit without being in the shot, best first.
///
/// `spec/07` §1.1 item 3 puts the controls "beside the area", and the reason they have to
/// be strictly *outside* it is harder than taste: every frame of the capture is the
/// selection's pixels, so a window overlapping the selection is captured with it. On
/// 2026-09-16 a 1917 px selection filling the middle monitor of three sent the pill to
/// x = 3851, Mutter clamped it back to 3520 -- inside the selection -- and every frame of
/// the capture had the controls in its corner.
///
/// A list rather than a point, because one point is what Mutter is free to overrule. Each
/// of the four sides -- beside on the right, beside on the left, under, over -- is put
/// inside the work area of **every** monitor it fits on, not only the one it mostly
/// overlaps: the side that wants to be beside a selection filling a laptop panel is, on a
/// desk with a monitor above that panel, best put at the foot of the monitor above (D104).
/// Candidates still in the shot are dropped, and the rest are ranked by how far the clamp
/// had to move them from where their side wanted them -- so a pill that fits where the
/// spec asks is exactly there, and one that does not goes to the nearest place that is
/// wholly on a screen. Only a selection that leaves no such place gets [`inside`]'s
/// answer instead.
fn spots(
    rect: Rect,
    pill: (i32, i32),
    screens: &[Rect],
    direction: ScrollDirection,
) -> Vec<(i32, i32)> {
    if screens.is_empty() {
        // Nothing to measure against: the spec's own answer, and the landed rect is
        // what gets believed.
        return vec![(rect.x + rect.width + PILL_GAP, rect.y)];
    }
    let found = perches(rect, pill, screens);
    if found.is_empty() { inside(rect, pill, screens, direction) } else { found }
}

/// The candidates wholly outside the selection, best first; see [`spots`].
fn perches(rect: Rect, pill: (i32, i32), screens: &[Rect]) -> Vec<(i32, i32)> {
    let (width, height) = pill;
    let sides = [
        (rect.x + rect.width + PILL_GAP, rect.y),
        (rect.x - PILL_GAP - width, rect.y),
        (rect.x, rect.y + rect.height + PILL_GAP),
        (rect.x, rect.y - PILL_GAP - height),
    ];
    let mut found: Vec<(i32, usize, (i32, i32))> = Vec::new();
    for (side, &(x, y)) in sides.iter().enumerate() {
        let want = Rect::new(x, y, width, height);
        for screen in screens {
            // A pill bigger than the screen cannot be wholly on it, and clamping it
            // there would shrink it on paper and nowhere else.
            if width > screen.width || height > screen.height {
                continue;
            }
            let landed = clamp_to_monitor(want, *screen);
            if landed.overlap_area(rect) == 0 {
                let moved = (landed.x - x).abs() + (landed.y - y).abs();
                found.push((moved, side, (landed.x, landed.y)));
            }
        }
    }
    found.sort_by_key(|&(moved, side, _)| (moved, side));
    let mut kept: Vec<(i32, i32)> = Vec::new();
    for (_, _, point) in found {
        if !kept.contains(&point) {
            kept.push(point);
        }
    }
    kept
}

/// Where the pill goes when nothing outside the selection is wholly on a screen: the
/// corner of a work area that covers the least of the selection, and of those, one on the
/// edge the page leaves by.
///
/// A pill in the shot is a flaw in the capture; a pill off the edge of the desktop is a
/// capture nobody can stop. Visible wins. And a pill on the edge the page leaves by is
/// drawn into the capture once, with the first frame: every later frame adds rows at the
/// other edge, so a pill there would be drawn into every one of them (D105).
fn inside(
    rect: Rect,
    pill: (i32, i32),
    screens: &[Rect],
    direction: ScrollDirection,
) -> Vec<(i32, i32)> {
    let (width, height) = pill;
    let mut found: Vec<(i64, bool, (i32, i32))> = Vec::new();
    for screen in screens.iter().filter(|screen| width <= screen.width && height <= screen.height) {
        let (left, top) = (screen.x, screen.y);
        let (right, bottom) = (screen.x + screen.width - width, screen.y + screen.height - height);
        for point in [(right, bottom), (left, bottom), (right, top), (left, top)] {
            let covered = Rect::new(point.0, point.1, width, height).overlap_area(rect);
            let leaving = match direction {
                ScrollDirection::Down => point.1 == top,
                ScrollDirection::Up => point.1 == bottom,
                ScrollDirection::Right => point.0 == left,
                ScrollDirection::Left => point.0 == right,
            };
            found.push((covered, !leaving, point));
        }
    }
    found.sort_by_key(|&(covered, arriving, _)| (covered, arriving));
    let mut kept: Vec<(i32, i32)> = Vec::new();
    for (_, _, point) in found {
        if !kept.contains(&point) {
            kept.push(point);
        }
    }
    kept
}

/// Whether a landed rect is wholly inside one work area -- not cut off by the edge of the
/// desktop, and not straddling two monitors of different sizes. An empty list is a list
/// nobody could measure against, and believes the compositor.
fn wholly_on_a_screen(landed: Rect, screens: &[Rect]) -> bool {
    let area = i64::from(landed.width) * i64::from(landed.height);
    screens.is_empty() || screens.iter().any(|screen| landed.overlap_area(*screen) == area)
}

/// What to tell the user about a capture that stopped by itself, if anything.
///
/// Silence for the ordinary ending -- the page ran out, which is what Done would have done
/// anyway -- and a sentence for the two that mean the capture is not the whole of what the
/// user was after.
fn ending(ended: Option<End>, longest: u32) -> Option<String> {
    let limits = Limits::default();
    match ended {
        Some(End::Full) => Some(format!("Stopped at the size limit, {longest} px")),
        Some(End::Unmatched) => {
            Some("Stopped: the page stopped matching. Try a smaller area.".to_owned())
        }
        _ if longest >= limits.warn_at => Some(format!("That is a very long capture: {longest} px")),
        _ => None,
    }
}

/// The media crate's live view, as the loop's [`Stream`].
#[derive(Debug)]
struct LiveStream(Live);

impl Stream for LiveStream {
    fn serial(&self) -> u64 {
        self.0.serial()
    }

    fn newest(&self) -> impl std::future::Future<Output = Option<(u64, Frame)>> {
        let newest = self.0.newest();
        async move {
            let (serial, sample) = newest?;
            // The copy out of the sample is the one per frame the loop takes, and at
            // 1.6 Mpx it is a few milliseconds nobody should wait for on the main loop.
            let rgba = gio::spawn_blocking(move || Live::pixels(&sample)).await.ok()??;
            let frame = Frame::new(rgba.width, rgba.height, rgba.pixels).ok()?;
            Some((serial, frame))
        }
    }

    fn ended(&self) -> bool {
        self.0.has_ended()
    }
}

/// A live view of `rect`, when the selection is wholly on one monitor and the compositor
/// will stream it and send a first frame (D106).
///
/// `None` sends the capture back to the extension's grabs, which are three times slower
/// than a hand can scroll but ask nothing of PipeWire -- so a selection across two
/// monitors, a session with no ScreenCast, or a stream that never negotiates still
/// captures, the way it did before.
async fn open_live<B: ShellBridge>(
    app: &adw::Application,
    bridge: &B,
    rect: Rect,
    nudge: &dyn Fn(),
) -> Option<LiveStream> {
    let monitors = match bridge.monitors().await {
        Ok(monitors) => monitors,
        Err(e) => {
            warn!("no monitors for a live view; grabbing frames instead: {e}");
            return None;
        }
    };
    let Some(monitor) = monitors.iter().find(|monitor| holds(monitor.geometry, rect)) else {
        info!(?rect, "the selection is not on one monitor; grabbing frames instead");
        return None;
    };
    if monitor.connector.is_empty() {
        warn!("the selection's monitor has no connector; grabbing frames instead");
        return None;
    }
    let source = StreamSource {
        connector: monitor.connector.clone(),
        rect: monitor.geometry,
        scale: monitor.scale,
        refresh: monitor.refresh,
    };
    let connection = app.dbus_connection()?;
    let live = match Live::open(&connection, &source, rect, LIVE_RATE).await {
        Ok(live) => live,
        Err(e) => {
            warn!("no live view of the selection; grabbing frames instead: {e}");
            return None;
        }
    };
    // The compositor sends a frame when it paints, and a screen with nothing moving on it
    // is not painted: the stream opens, and nothing comes until the user scrolls -- by
    // which time the first frame is not the page as it was. That is what happened once
    // the cards stepped aside, whose countdowns had been painting all along (D107). So
    // the pill is drawn again, as it is, until a frame arrives.
    let deadline = Instant::now() + LIVE_FIRST;
    let mut nudged: Option<Instant> = None;
    while live.serial() == 0 && !live.has_ended() && Instant::now() < deadline {
        if nudged.is_none_or(|at| at.elapsed() >= LIVE_NUDGE) {
            nudge();
            nudged = Some(Instant::now());
        }
        glib::timeout_future(LIVE_POLL).await;
    }
    if live.serial() == 0 {
        warn!("the live view sent no first frame; grabbing frames instead");
        live.stop().await;
        return None;
    }
    Some(LiveStream(live))
}

/// Whether `rect` lies wholly inside `monitor`.
const fn holds(monitor: Rect, rect: Rect) -> bool {
    rect.x >= monitor.x
        && rect.y >= monitor.y
        && rect.x + rect.width <= monitor.x + monitor.width
        && rect.y + rect.height <= monitor.y + monitor.height
}

/// The preview as something GTK can draw.
fn texture_of(frame: &Frame) -> Option<gtk::gdk::Texture> {
    let (width, height) = (i32::try_from(frame.width()).ok()?, i32::try_from(frame.height()).ok()?);
    Some(
        gtk::gdk::MemoryTexture::new(
            width,
            height,
            gtk::gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from(frame.pixels()),
            (width as usize) * 4,
        )
        .upcast(),
    )
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use std::path::Path;

    use octosnap_shell::NullBridge;

    use super::*;

    /// A fresh context per test: the default one is a process-wide singleton only one
    /// thread may own, and cargo runs these in parallel (`flow`'s tests say the same).
    fn run_on<F: std::future::Future>(future: F) -> F::Output {
        glib::MainContext::new().block_on(future)
    }

    /// One frame of a day in Google Calendar, which is where D94 came from: a dark ground
    /// ruled every 58 rows, with an appointment in about one hour in three so that the
    /// hours can be told apart at all. `mark` paints a small patch at the top right,
    /// standing in for the controls pill, whose preview strip changed on every accepted
    /// frame and so stopped two identical grabs from ever *being* identical.
    fn ruled(dir: &Path, name: &str, top: u32, mark: u8) -> PathBuf {
        const RULE: u32 = 58;
        let (width, height) = (400u32, 300u32);
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            let row = top + y;
            let (hour, within) = (row / RULE, row % RULE);
            // Aperiodic, so that scrolling by a whole number of hours does not map one
            // appointment onto the next -- which is a page with no information in it at
            // all, and not the page anyone was capturing.
            let busy = hour.wrapping_mul(2_654_435_761) >> 13;
            let event = busy.is_multiple_of(3) && (8..24).contains(&within);
            let tone = if within == 0 { 58u8 } else { 26 };
            for x in 0..width {
                let here = if x >= width - 40 && y < 40 {
                    mark
                } else if event && (width / 5..width * 17 / 20).contains(&x) {
                    110
                } else {
                    tone
                };
                pixels.extend_from_slice(&[here, here, here, 255]);
            }
        }
        let path = dir.join(name);
        Frame::new(width, height, pixels).expect("a frame").write(&path).expect("a png");
        path
    }

    /// The loop asks for a frame, waits, and asks again until two agree; a page that has
    /// settled answers the same thing twice. So each frame is listed twice, which is what
    /// a settled page looks like from `settle`'s side.
    fn settled(frames: &[PathBuf]) -> Vec<PathBuf> {
        let mut out = vec![frames[0].clone()];
        for frame in &frames[1..] {
            out.push(frame.clone());
            out.push(frame.clone());
        }
        out
    }

    /// The three monitors the report of 2026-09-16 came from: two side by side and a
    /// laptop panel under them, with nothing to the right of x = 3840.
    fn desk() -> Vec<Rect> {
        vec![
            Rect::new(0, 0, 1920, 1080),
            Rect::new(1920, 0, 1920, 1080),
            Rect::new(600, 1080, 1440, 900),
        ]
    }

    /// The capture itself: a selection filling the right-hand monitor, which leaves the
    /// place the pill is asked for off the end of the desktop.
    #[test]
    fn a_selection_filling_a_monitor_sends_the_pill_to_the_one_beside_it() {
        let rect = Rect::new(1922, 110, 1917, 969);
        let perches = perches(rect, (330, 420), &desk());
        let &(x, y) = perches.first().expect("two whole monitors are free of this selection");
        assert!(x + 330 <= rect.x, "{x} is not clear of the selection at {}", rect.x);
        assert!(x >= 0, "{x} is off the left of the desktop");
        assert_eq!(y, rect.y, "beside means beside, not above or below");
        for (x, y) in perches {
            let perch = Rect::new(x, y, 330, 420);
            assert_eq!(perch.overlap_area(rect), 0, "({x}, {y}) is in the shot");
        }
    }

    /// The ordinary case, which must not have changed: a window-sized selection with room
    /// to its right keeps the placement `spec/07` §1.1 item 3 asks for.
    #[test]
    fn an_ordinary_selection_keeps_the_pill_beside_it_on_the_right() {
        let rect = Rect::new(300, 200, 900, 700);
        assert_eq!(perches(rect, (330, 420), &desk()).first(), Some(&(300 + 900 + PILL_GAP, 200)));
    }

    /// A selection that covers every monitor has nowhere outside it, and saying so is
    /// better than moving the pill to a second place that is inside it too.
    #[test]
    fn a_selection_over_the_whole_desktop_has_nowhere_to_put_the_pill() {
        assert!(perches(Rect::new(0, 0, 3840, 1980), (330, 420), &desk()).is_empty());
    }

    /// A pill that fits under the selection but not beside it goes under it.
    #[test]
    fn a_wide_selection_puts_the_pill_underneath() {
        let rect = Rect::new(0, 0, 1920, 400);
        let screens = vec![Rect::new(0, 0, 1920, 1080)];
        assert_eq!(perches(rect, (330, 420), &screens).first(), Some(&(0, 400 + PILL_GAP)));
    }

    /// The desk of 2026-09-23: two monitors side by side and a laptop panel under the
    /// left one, with nothing to the right of the panel. A selection filling most of the
    /// panel sent the pill beside it on the right -- x = 1778 on a 1920 px panel -- where
    /// it landed half off the desktop and, being outside the selection, was never
    /// checked again (D104).
    fn laptop_desk() -> Vec<Rect> {
        vec![
            Rect::new(0, 0, 1920, 1080),
            Rect::new(1920, 0, 1920, 1080),
            Rect::new(0, 1112, 1920, 1168),
        ]
    }

    #[test]
    fn a_pill_with_no_room_beside_the_selection_goes_wholly_onto_a_screen() {
        let rect = Rect::new(225, 1124, 1541, 1073);
        let pill = (320, 261);
        let found = spots(rect, pill, &laptop_desk(), ScrollDirection::Down);
        let &(x, y) = found.first().expect("the monitor above has room");
        // At the foot of the monitor above, right over the selection.
        assert_eq!((x, y), (225, 1080 - 261));
        for (x, y) in found {
            let perch = Rect::new(x, y, pill.0, pill.1);
            assert_eq!(perch.overlap_area(rect), 0, "({x}, {y}) is in the shot");
            assert!(wholly_on_a_screen(perch, &laptop_desk()), "({x}, {y}) is cut off");
        }
    }

    /// When every place outside the selection is off the desktop, the pill stays visible
    /// in the corner that covers the least of it, rather than half off the edge.
    #[test]
    fn a_selection_with_nowhere_outside_it_keeps_the_pill_visible() {
        let screens = vec![Rect::new(0, 0, 1920, 1080)];
        let rect = Rect::new(0, 0, 1920, 1080);
        let found = spots(rect, (320, 261), &screens, ScrollDirection::Down);
        let &(x, y) = found.first().expect("somewhere on the one screen");
        assert!(wholly_on_a_screen(Rect::new(x, y, 320, 261), &screens));
        // On the edge the page leaves by, so it is drawn into the capture once.
        assert_eq!(y, 0, "a page scrolling down leaves by the top");
        let up = spots(rect, (320, 261), &screens, ScrollDirection::Up);
        assert_eq!(up.first().map(|&(_, y)| y), Some(1080 - 261), "and up by the bottom");
        let left = spots(rect, (320, 261), &screens, ScrollDirection::Left);
        assert_eq!(left.first().map(|&(x, _)| x), Some(1920 - 320), "and leftwards by the right");
        // A selection leaving a strip too narrow for the pill gets the corner that covers
        // the least of it: ten columns of the shot, not a hundred and thirty.
        let rect = Rect::new(0, 0, 1800, 1080);
        let found = spots(rect, (130, 130), &screens, ScrollDirection::Down);
        assert_eq!(found.first(), Some(&(1790, 0)));
    }
    /// D94 through the app's own loop, which is where the user met it: a page scrolled
    /// twice that then stands at the foot of the day, where every offset a whole number of
    /// hours apart scores a perfect match. The capture is the user's to end (D76), so
    /// reaching the foot of the page must not close it. The frames differ here,
    /// in a corner, as they did on the day: the pill was inside the captured rectangle and
    /// its preview strip changed on every grab, so "this frame is the same as the last
    /// one" was never true again.
    ///
    /// Those frames reach the matcher and it answers "the page did not move" to each,
    /// and under D94 the second such answer earned the pill its "looks like the end of the
    /// page" line -- which was how this test used to know when to press Done. Under D98
    /// the loop does nothing with that answer: a page that stood still and a user who
    /// paused to read are the same picture, and the app has stopped guessing which. So
    /// Done is pressed here once every frame listed has been offered, and what is asserted
    /// is that the two real scrolls landed and not one of the stopped ones did.
    #[test]
    fn a_capture_at_the_foot_of_a_page_waits_for_done() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let at = dir.path();
        let frames = settled(&[
            ruled(at, "m0.png", 0, 26),
            ruled(at, "m1.png", 174, 26),
            // The last scroll, and then stopped but never twice alike: the corner changes
            // under every grab. Not in the same grab as the scroll, which on a page this
            // empty is a change in place big enough to cost the match -- and the thing
            // that used to do that, the pill in the shot, since D105 holds still.
            ruled(at, "m2.png", 348, 26),
            ruled(at, "m3.png", 348, 90),
            ruled(at, "m4.png", 348, 150),
            ruled(at, "m5.png", 348, 180),
            ruled(at, "m6.png", 348, 210),
        ]);
        let listed = frames.len();
        let bridge = NullBridge::default().with_scroll_frames(frames);
        let options = Options::default();
        let control = Control::new();
        let seen = Rc::new(Cell::new(Progress::default()));
        let watching = Rc::clone(&seen);
        let shot = run_on(async {
            // Done is the only thing that ends the capture. The loop reports nothing
            // for a frame it declines, so the bridge's own count is what says every frame
            // above has been offered -- plus two, so the last one has been repeated at it.
            let stop = Control::clone(&control);
            let offered = bridge.clone();
            glib::spawn_future_local(async move {
                while offered.scroll_grabs() < listed + 2 {
                    glib::timeout_future(WATCH_POLL / 4).await;
                }
                stop.set(Command::Done);
            });
            let report = |p, _| watching.set(p);
            run(&bridge, Rect::new(0, 0, 400, 300), options, &control, report).await
        })
        .expect("a capture");
        assert_eq!(shot.ended, None, "Done ends the capture; the page does not");
        assert_eq!(seen.get().frames, 2, "the pill was told of the two scrolls and no more");
        // Two real steps, and then nothing: every frame after the foot of the page is the
        // same rows under a different corner, and none of them reached the canvas.
        assert_eq!(shot.frame.height(), 300 + 2 * 174, "nothing after the foot of the page was new");
        assert!(bridge.scroll_grabs() >= listed, "the stopped frames were never even offered");
        assert_eq!(bridge.scroll_ends().len(), 1, "the session is always ended");
    }

    /// The promise at the top of `run`: the extension is holding a virtual pointer device
    /// and has parked the real one inside the selection, so every path out ends the
    /// session -- including the one the user cancelled.
    /// One frame of a page of type: rows `[top, top + height)`, every line its own glyphs
    /// and nine rows of leading under it.
    fn typed(width: u32, height: u32, top: u32) -> Frame {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            let (line, within) = ((top + y) / 20, (top + y) % 20);
            for x in 0..width {
                let bits = (line.wrapping_mul(2_654_435_761) ^ (x / 3).wrapping_mul(40_503)) >> 7;
                let ink = within < 11 && bits.is_multiple_of(3) && x > 10;
                let tone = if ink { 30u8 } else { 245 };
                pixels.extend_from_slice(&[tone, tone, tone, 255]);
            }
        }
        Frame::new(width, height, pixels).expect("a frame")
    }

    /// A live view handed its frames in advance. Each `newest` hands over the frame it is
    /// showing and moves on to the next, and when they run out the page stands still: the
    /// serial stops growing, which is what nobody scrolling looks like on a real stream.
    /// `ends` makes it stop for good instead, as a session the compositor closed does.
    #[derive(Debug)]
    struct Played {
        frames: Vec<Frame>,
        at: Cell<usize>,
        handed: Cell<usize>,
        ends: bool,
    }

    impl Played {
        fn new(frames: Vec<Frame>, ends: bool) -> Self {
            Self { frames, at: Cell::new(0), handed: Cell::new(0), ends }
        }
    }

    impl Stream for Played {
        fn serial(&self) -> u64 {
            self.at.get() as u64 + 1
        }

        fn newest(&self) -> impl std::future::Future<Output = Option<(u64, Frame)>> {
            let at = self.at.get();
            let shown = self.frames.get(at).cloned().map(|frame| (at as u64 + 1, frame));
            if at + 1 < self.frames.len() {
                self.at.set(at + 1);
            }
            self.handed.set(self.handed.get() + 1);
            std::future::ready(shown)
        }

        fn ended(&self) -> bool {
            self.ends && self.handed.get() >= self.frames.len()
        }
    }

    /// D106 through the loop: a live view of a page scrolled by hand, taken the way the
    /// stream offers it -- small, uneven steps, many a second. Every frame is placed and
    /// the stitch is the page, row for row. Done is pressed once every frame has been
    /// handed over, and a deadline cancels instead of hanging if that never happens.
    #[test]
    fn a_live_view_scrolled_by_hand_is_stitched_row_for_row() {
        let tops = [0u32, 18, 41, 77, 120, 168, 230];
        let count = tops.len();
        let frames = tops.iter().map(|&top| typed(400, 300, top)).collect();
        let stream = Rc::new(Played::new(frames, false));
        let bridge = NullBridge::default();
        let options = Options::default();
        let control = Control::new();
        let seen = Rc::new(Cell::new(Progress::default()));
        let watching = Rc::clone(&seen);
        let shot = run_on(async {
            let stop = Control::clone(&control);
            let played = Rc::clone(&stream);
            glib::spawn_future_local(async move {
                let deadline = Instant::now() + Duration::from_secs(10);
                while played.handed.get() < count && Instant::now() < deadline {
                    glib::timeout_future(WATCH_POLL / 4).await;
                }
                let done = played.handed.get() >= count;
                stop.set(if done { Command::Done } else { Command::Cancel });
            });
            let report = |p, _| watching.set(p);
            run_live(&bridge, &*stream, Rect::new(0, 0, 400, 300), options, &control, report).await
        })
        .expect("a capture");
        assert_eq!(shot.ended, None, "Done ended it");
        assert_eq!(seen.get().frames, 6, "every step of the hand was placed");
        assert_eq!(shot.frame, typed(400, 300 + 230, 0), "the stitch is the page");
        assert_eq!(bridge.scroll_grabs(), 0, "not one frame was grabbed");
        assert_eq!(bridge.scroll_ends().len(), 1, "the session is always ended");
    }

    /// D112 through the loop, the way it was found (2026-09-24): a conversation standing at
    /// its foot, a capture left going down over it, and a hand that scrolls it up, comes
    /// part of the way back to read something again, and flicks back to where it began.
    /// Every frame is placed, the stitch is the page, and only the steps up added to it.
    #[test]
    fn a_page_scrolled_by_hand_against_the_capture_is_followed_both_ways() {
        let tops = [230u32, 212, 189, 153, 110, 62, 0, 60, 150, 230];
        let count = tops.len();
        let frames = tops.iter().map(|&top| typed(400, 300, top)).collect();
        let stream = Rc::new(Played::new(frames, false));
        let bridge = NullBridge::default();
        let options = Options::default();
        assert_eq!(options.direction, ScrollDirection::Down, "left going down");
        let control = Control::new();
        let seen = Rc::new(Cell::new(Progress::default()));
        let (watching, troubled) = (Rc::clone(&seen), Rc::new(Cell::new(false)));
        let worried = Rc::clone(&troubled);
        let shot = run_on(async {
            let stop = Control::clone(&control);
            let played = Rc::clone(&stream);
            glib::spawn_future_local(async move {
                let deadline = Instant::now() + Duration::from_secs(10);
                while played.handed.get() < count && Instant::now() < deadline {
                    glib::timeout_future(WATCH_POLL / 4).await;
                }
                let done = played.handed.get() >= count;
                stop.set(if done { Command::Done } else { Command::Cancel });
            });
            let report = |p: Progress, _| {
                worried.set(worried.get() || p.trouble);
                watching.set(p);
            };
            run_live(&bridge, &*stream, Rect::new(0, 0, 400, 300), options, &control, report).await
        })
        .expect("a capture");
        assert!(!troubled.get(), "nothing was ever too big");
        assert_eq!(seen.get().frames, 6, "the six steps up, and not the way back");
        assert_eq!(shot.frame, typed(400, 300 + 230, 0), "the stitch is the page");
    }

    /// A live view the compositor closes mid-capture ends the capture with what it had,
    /// rather than with an error, or with a loop waiting for frames that will not come.
    #[test]
    fn a_live_view_that_ends_keeps_what_was_stitched() {
        let tops = [0u32, 25, 60];
        let stream = Played::new(tops.iter().map(|&top| typed(400, 300, top)).collect(), true);
        let bridge = NullBridge::default();
        let options = Options::default();
        let control = Control::new();
        let rect = Rect::new(0, 0, 400, 300);
        let shot = run_on(run_live(&bridge, &stream, rect, options, &control, |_, _| {}))
            .expect("a capture");
        assert_eq!(shot.frame, typed(400, 360, 0), "what was stitched is kept");
        assert_eq!(bridge.scroll_ends().len(), 1, "the session is always ended");
    }

    #[test]
    fn a_cancelled_capture_still_ends_the_session() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let at = dir.path();
        let frames = settled(&[ruled(at, "c0.png", 0, 26), ruled(at, "c1.png", 174, 26)]);
        let bridge = NullBridge::default().with_scroll_frames(frames);
        let control = Control::new();
        control.set(Command::Cancel);
        let rect = Rect::new(0, 0, 400, 300);
        let outcome = run_on(run(&bridge, rect, Options::default(), &control, |_, _| {}));
        assert_eq!(outcome.err().as_deref(), Some("cancelled"));
        assert_eq!(bridge.scroll_ends().len(), 1, "cancelled is still ended");
    }
}
