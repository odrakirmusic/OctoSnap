// SPDX-License-Identifier: GPL-3.0-or-later
//! The app's half of `spec/06`'s recorder, at GIF size (M5, D68).
//!
//! The extension does not hand the app pixels for a recording the way it does for a
//! screenshot; it hands it a *request* -- a rectangle on a monitor -- and the app owns the
//! whole recording from there: the Mutter ScreenCast session, the GStreamer graph and the
//! reel the frames are kept in all live in [`octosnap_media`], driven from here. This
//! module is the glue: it turns a request into a running [`Recording`], keeps the panel
//! timer, the red frame and the controls pill in step with it, and on Stop files the
//! recording through the same [`CaptureFlow`](crate::flow::CaptureFlow) a screenshot goes
//! through -- so a GIF gets a card, an after-recording plan and a place in the history, all
//! for free.
//!
//! The capture Stop files is `<id>.gif`, but no GIF has been written: the frames are in
//! `<id>.reel` beside it, and [`render`] writes the GIF from them when something needs the
//! file (D113).
//!
//! Only one recording runs at a time. A second `record` while one is live is ignored, and
//! `stop` on nothing is a no-op, so the panel item, the shortcut and the pill can all fire
//! without coordinating.

mod controls;
pub mod render;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use adw::prelude::*;
use tracing::{info, warn};

use octosnap_core::capture::fresh_id;
use octosnap_core::{CaptureMode, CaptureResult, Rect, SourceWindow};
use octosnap_media::gif::{GifPlan, GifSettings, StreamSource};
use octosnap_media::recorder::{Options, Recording, Targeting};
use octosnap_shell::{Cue, Placement, RecordingState, ShellBridge};

use crate::{SharedFlow, notify};
use crate::settings::Settings;
use controls::Pill;

/// How often the timer, the panel indicator and (when the stream ends on its own) the
/// finaliser are woken. Half a second is smooth enough for a clock that shows seconds and
/// far short of `spec/10` §6's UI budget.
const TICK: Duration = Duration::from_millis(500);
/// The gap the controls pill is asked to sit below the recorded rectangle. The extension
/// clamps it into the work area (`spec/10` §3.1); this is only the request.
const PILL_GAP: i32 = 8;
/// How long an armed recording waits to be begun before it gives its session back.
/// `rec-countdown` tops out at ten seconds; this is that with room for a slow arm.
const ARMED_FOR: Duration = Duration::from_secs(20);
/// How long `start` waits for an arming already under way before building its own.
///
/// The All-In-One toolbar has no countdown, so the extension's `arm-record` and `record`
/// arrive a fifth of a millisecond apart (2026-09-23). `start` found nothing armed yet and
/// opened a second session beside the one being armed: two screencasts of one monitor at
/// fifty frames a second, one of them throwing every frame away for twenty seconds (D103).
/// An arm takes about 300 ms warm; this is that with room for a cold one.
const ARM_WAIT: Duration = Duration::from_secs(3);
/// How often a waiting `start` looks again.
const ARM_POLL: Duration = Duration::from_millis(10);

/// `mark_recording`: Mutter shows its own indicator too, so a recording is visible even if
/// the extension's is not there yet (`spec/06` §4.4).
fn record_options() -> Options {
    Options { targeting: Targeting::NodeId, mark_recording: true }
}

/// What the extension (or, later, the CLI) asks the app to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordRequest {
    /// The selection, in logical stage coordinates.
    pub rect: Rect,
    /// The connector the selection is on, when the caller knows it. `None` resolves from
    /// the rectangle's position.
    pub display: Option<String>,
    /// A per-recording override of `rec-cursor`; `None` uses the setting.
    pub cursor: Option<bool>,
    /// A per-recording override of `gif-fps`, from the All-In-One toolbar's recording
    /// row (D101); zero is the screen's rate. `None` uses the setting.
    pub fps: Option<u32>,
    /// A per-recording override of `gif-max-width`; zero is no cap.
    pub max_width: Option<u32>,
    /// A per-recording override of `gif-quality`.
    pub quality: Option<u8>,
}

impl RecordRequest {
    /// Parses `record`'s `a{sv}` payload. `None` for a payload with no rectangle, or one
    /// that asks for a format this build does not have (video is M9).
    ///
    /// The overrides are clamped to what the settings page would accept, not rejected: a
    /// request that says `fps: 60` is a request from a caller that has not heard the GIF
    /// format cannot play that fast, and it gets the ceiling rather than nothing.
    #[must_use]
    pub fn from_variant(payload: &glib::Variant) -> Option<Self> {
        use octosnap_media::gif::GIF_FPS_MAX;
        use octosnap_shell::variant;
        let dict = glib::VariantDict::new(Some(payload));
        let rect = variant::rect(&dict, "rect")?;
        let display = variant::string(&dict, "display").filter(|s| !s.is_empty());
        let cursor = variant::bool_(&dict, "cursor");
        let format = variant::string(&dict, "record-format").unwrap_or_default();
        if !format.is_empty() && !format.eq_ignore_ascii_case("gif") {
            warn!(format, "this build records GIFs only; video is M9");
            return None;
        }
        let fps = variant::i32_(&dict, "fps")
            .map(|n| u32::try_from(n).unwrap_or(0).min(GIF_FPS_MAX));
        let max_width = variant::i32_(&dict, "max-width")
            .map(|n| u32::try_from(n).unwrap_or(0).min(7680));
        let quality = variant::i32_(&dict, "quality")
            .map(|n| u8::try_from(n.clamp(1, 100)).unwrap_or(80));
        Some(Self { rect, display, cursor, fps, max_width, quality })
    }

    /// Lays the overrides over the user's settings: what the request said wins, and what
    /// it left out is left alone (D101).
    pub fn apply_to(&self, gif: &mut GifSettings) {
        if let Some(cursor) = self.cursor {
            gif.cursor = cursor;
        }
        if let Some(fps) = self.fps {
            gif.fps = fps;
        }
        if let Some(max_width) = self.max_width {
            gif.max_width = max_width;
        }
        if let Some(quality) = self.quality {
            gif.quality = quality;
        }
    }
}

/// One recording in progress, and the on-screen things that go with it.
#[derive(Debug)]
struct Active {
    recording: Recording,
    /// `None` for a whole-monitor recording, whose controls have nowhere outside the area
    /// to go and so fold into the panel (`spec/06` §3, acceptance item 3).
    pill: Option<Rc<Pill>>,
    /// The red frame's rectangle, kept so Stop and Trash can take it away.
    frame: Rect,
    /// The monitor the recording came from, for the GIF's history record.
    display: String,
    scale: f64,
    started: Instant,
}

/// A recording built, playing and throwing its frames away, waiting for the countdown.
///
/// `spec/06` §3 puts a countdown in front of every recording, and the extension runs it.
/// Arming turns those three seconds from dead time into the recorder's whole setup, so
/// that when the countdown reaches zero there is nothing left to do but keep the frames
/// (D70).
#[derive(Debug)]
struct Armed {
    recording: Recording,
    request: RecordRequest,
    plan: GifPlan,
    source: StreamSource,
    path: PathBuf,
    /// Which arming this is. A guard timer disarms only its own, so beginning a recording
    /// does not need the timer's `SourceId` back (D32).
    run: u64,
}

/// What a recording needs before a session is opened: where it comes from, at what size,
/// and where it goes.
struct Prepared {
    plan: GifPlan,
    source: StreamSource,
    path: PathBuf,
}

/// The recorder. One per app; reached through [`crate::recorder`].
#[derive(Debug)]
pub struct Recorder {
    app: adw::Application,
    flow: SharedFlow,
    active: RefCell<Option<Active>>,
    /// A recording set up during the countdown, waiting to be begun.
    armed: RefCell<Option<Armed>>,
    /// Bumped by every arm and every disarm, so a guard timer that fires late finds a
    /// generation that is no longer its own and does nothing.
    arming: Cell<u64>,
    /// Armings under way right now, so a `start` that overtakes one waits for it rather
    /// than opening a second session beside it (D103).
    arming_now: Cell<u32>,
    /// A `start` is between its first check and the recording it builds, so a second
    /// `start` in that window is refused rather than building a second recording.
    starting: Cell<bool>,
    ticker: RefCell<Option<glib::SourceId>>,
}

/// Holds a counter up for as long as it lives: an arming counts as under way on every path
/// out of `arm`, the early returns included.
struct Counted<'a>(&'a Cell<u32>);

impl<'a> Counted<'a> {
    fn new(count: &'a Cell<u32>) -> Self {
        count.set(count.get() + 1);
        Self(count)
    }
}

impl Drop for Counted<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

/// Holds a flag up for as long as it lives.
struct Raised<'a>(&'a Cell<bool>);

impl Drop for Raised<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl Recorder {
    #[must_use]
    pub fn new(app: &adw::Application, flow: SharedFlow) -> Rc<Self> {
        Rc::new(Self {
            app: app.clone(),
            flow,
            active: RefCell::new(None),
            armed: RefCell::new(None),
            arming: Cell::new(0),
            arming_now: Cell::new(0),
            starting: Cell::new(false),
            ticker: RefCell::new(None),
        })
    }

    /// True while a recording is running, for the actions that should not start a second.
    #[must_use]
    pub fn is_recording(&self) -> bool {
        self.active.borrow().is_some()
    }

    /// Sets a recording up without starting it, for the extension to begin when its
    /// countdown reaches zero.
    ///
    /// Everything expensive is here: resolving the monitor, planning the crop and the
    /// scale, opening the ScreenCast session, waiting for the PipeWire node, building the
    /// graph, starting the encoder, and waiting for the stream to negotiate. Measured on
    /// the target machine that is 266 ms warm and 504 ms cold -- and it used to run after
    /// the countdown, in front of a user who had already been made to wait three seconds
    /// (D70). Armed, it runs *inside* the countdown.
    ///
    /// Quiet on failure, deliberately: arming is speculative, and `start` does the whole
    /// thing again from scratch and tells the user if it fails then. Nothing here is
    /// promised to anyone.
    pub async fn arm(self: &Rc<Self>, request: RecordRequest) {
        if self.is_recording() || self.armed.borrow().is_some() {
            return;
        }
        let _under_way = Counted::new(&self.arming_now);
        let generation = self.arming.get();
        let at = Instant::now();
        let prepared = match self.prepare(&request).await {
            Ok(prepared) => prepared,
            Err(e) => {
                warn!("could not prepare the arming: {e}");
                return;
            }
        };
        let Some(connection) = self.app.dbus_connection() else {
            return;
        };
        let reel = octosnap_media::reel::beside(&prepared.path);
        let recording =
            match Recording::arm(&connection, &prepared.plan, &reel, record_options()).await {
                Ok(recording) => recording,
                Err(e) => {
                    warn!("could not arm the recording: {e}");
                    octosnap_media::reel::remove(&reel);
                    return;
                }
            };
        // The countdown may have ended while the session was being opened -- a zero-second
        // countdown, or a `record` from the CLI. `start` waits for an arming under way, but
        // not for ever; one that gave up bumped the generation and built its own, and then
        // this one is a session held over a recording nobody asked for.
        if self.arming.get() != generation || self.is_recording() || self.armed.borrow().is_some() {
            info!("the arming was overtaken; giving the session back");
            glib::spawn_future_local(async move { recording.trash().await });
            return;
        }
        let run = self.arming.get() + 1;
        self.arming.set(run);
        let Prepared { plan, source, path } = prepared;
        *self.armed.borrow_mut() = Some(Armed { recording, request, plan, source, path, run });

        // An armed recording holds a session open and lights Mutter's indicator, so it
        // must not outlive the countdown that is supposed to end it. The extension cancels
        // its own; this catches the one whose extension went away mid-countdown.
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(ARMED_FOR, move || {
            if let Some(recorder) = weak.upgrade() {
                recorder.disarm_run(run, "it was never begun");
            }
        });
        info!(took_ms = ms(at.elapsed()), "recording armed");
    }

    /// Throws away a recording armed but never begun -- the countdown was cancelled, or
    /// the overlay was closed.
    pub fn disarm(self: &Rc<Self>) {
        self.disarm_run(self.arming.get(), "it was cancelled");
    }

    fn disarm_run(self: &Rc<Self>, run: u64, why: &str) {
        let armed = {
            let mut slot = self.armed.borrow_mut();
            match slot.as_ref() {
                Some(armed) if armed.run == run => slot.take(),
                _ => None,
            }
        };
        let Some(armed) = armed else {
            return;
        };
        self.arming.set(self.arming.get() + 1);
        info!(why, "disarming the recording");
        // `trash` stops the session and aborts the encoder, which removes the spool file
        // it had opened: an arming that came to nothing leaves nothing behind.
        let recording = armed.recording;
        glib::spawn_future_local(async move { recording.trash().await });
    }

    /// Starts recording `request`: begins the armed recording if there is one for this
    /// rectangle, otherwise builds one now. Puts the frame and the pill on screen and
    /// lights the panel indicator. Any failure leaves nothing running and tells the user.
    pub async fn start(self: &Rc<Self>, request: RecordRequest) {
        if self.is_recording() || self.starting.replace(true) {
            warn!("a recording is already running; ignoring the request");
            return;
        }
        let _starting = Raised(&self.starting);

        // An arming still under way is, nearly always, the recording this request is for:
        // the All-In-One toolbar sends `arm-record` and `record` back to back. Waiting for
        // it costs what building a second one would, and leaves one session instead of two
        // (D103).
        let deadline = Instant::now() + ARM_WAIT;
        while self.arming_now.get() > 0 && Instant::now() < deadline {
            glib::timeout_future(ARM_POLL).await;
        }
        if self.arming_now.get() > 0 {
            // Too slow to wait for: tell it so, and build this one from scratch.
            warn!("an arming is still under way; recording without it");
            self.arming.set(self.arming.get() + 1);
        }

        // The armed one, if it is for this rectangle. A request that does not match what
        // was armed (the CLI asking for somewhere else while an overlay counts down) is
        // built from scratch and the armed one is thrown away, rather than recording the
        // wrong rectangle very quickly.
        let armed = {
            let mut slot = self.armed.borrow_mut();
            match slot.as_ref() {
                Some(armed) if armed.request == request => slot.take(),
                _ => None,
            }
        };
        if armed.is_none() && self.armed.borrow().is_some() {
            self.disarm();
        }

        let (recording, plan, source, path) = match armed {
            Some(armed) => {
                self.arming.set(self.arming.get() + 1);
                // The whole of "start recording", for a recording that is already running
                // and discarding: one atomic store, and the next frame is frame zero.
                armed.recording.begin();
                (armed.recording, armed.plan, armed.source, armed.path)
            }
            None => {
                let prepared = match self.prepare(&request).await {
                    Ok(prepared) => prepared,
                    Err(e) => {
                        warn!("could not prepare the recording: {e}");
                        notify::tell(&self.app, "recording", "Could not record", &e, true);
                        return;
                    }
                };
                let Some(connection) = self.app.dbus_connection() else {
                    warn!("the app has no bus connection; cannot open a ScreenCast session");
                    return;
                };
                let recording = match Recording::start(
                    &connection,
                    &prepared.plan,
                    &octosnap_media::reel::beside(&prepared.path),
                    record_options(),
                )
                .await
                {
                    Ok(recording) => recording,
                    Err(e) => {
                        warn!("could not start the recording: {e}");
                        notify::tell(&self.app, "recording", "Could not record", &e.to_string(), true);
                        return;
                    }
                };
                let Prepared { plan, source, path } = prepared;
                (recording, plan, source, path)
            }
        };

        let bridge = self.flow.bridge().clone();
        if let Err(e) = bridge.show_recording_frame(request.rect, true).await {
            warn!("could not show the recording frame: {e}");
        }
        if let Err(e) = bridge.set_recording_state(RecordingState::Recording, 0).await {
            warn!("could not set the recording state: {e}");
        }

        let whole_monitor = plan.is_whole_monitor();
        *self.active.borrow_mut() = Some(Active {
            recording,
            pill: None,
            frame: request.rect,
            display: source.connector.clone(),
            scale: source.scale,
            started: Instant::now(),
        });

        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local(TICK, move || match weak.upgrade() {
            Some(recorder) => {
                recorder.tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        *self.ticker.borrow_mut() = Some(id);

        info!(path = %path.display(), width = plan.output.width, height = plan.output.height, "recording started");
        // Spawned: the pill below is the user's control, and it should not wait on a sound.
        {
            let flow = Rc::clone(&self.flow);
            glib::spawn_future_local(async move { flow.cue(Cue::RecordStart).await });
        }

        // `spec/06` §3: the pill sits outside the area. A whole-monitor recording has no
        // outside, so its controls are the panel's alone (acceptance item 3) -- unless
        // there is no panel and no shortcut to stop it either. Then the pill goes in the
        // monitor's corner and into the recording, which is better than a recording
        // nothing can stop.
        //
        // Built *after* the recording is running, not before: it is a GTK window plus a
        // `PlaceWindow` round trip, about 120 ms cold, and none of it is the recording.
        // The clock now starts when the frames do, and the pill arrives a moment later
        // with the right time already on it.
        let stranded = whole_monitor && !stoppable_elsewhere();
        if stranded {
            info!("no panel indicator and no stop shortcut: the pill goes in the recording");
        }
        if !whole_monitor || stranded {
            let recorder = Rc::clone(self);
            let rect = request.rect;
            glib::spawn_future_local(async move {
                let pill = recorder.build_pill(rect, &source).await;
                match recorder.active.borrow_mut().as_mut() {
                    Some(active) => {
                        pill.set_elapsed(active.started.elapsed());
                        active.pill = Some(pill);
                    }
                    // Stopped while the pill was being placed: it has no recording to
                    // control, and must not be left on screen.
                    None => pill.close(),
                }
            });
        }
    }

    /// The monitor, the settings, the plan and the capture's name: everything a session
    /// needs, and nothing that needs a session. Shared by `arm` and `start`. The frames go
    /// to the reel beside the name, and the GIF to the name when it is asked for (D113).
    ///
    /// `Err` is a sentence for the user, but it is the *caller* that decides whether to say
    /// it: the same failure is worth a notification from `start`, which the user asked for,
    /// and worth only a log line from `arm`, which they did not.
    async fn prepare(self: &Rc<Self>, request: &RecordRequest) -> Result<Prepared, String> {
        let Some(source) = self.source_for(request).await else {
            return Err("OctoSnap could not find the monitor to record.".to_owned());
        };

        let settings = Settings::load();
        let mut gif = settings.gif_settings();
        request.apply_to(&mut gif);
        let plan = GifPlan::new(request.rect, &source, gif).map_err(|e| e.to_string())?;

        let spool = crate::history::History::default_spool();
        std::fs::create_dir_all(&spool)
            .map_err(|e| format!("the spool {} could not be created: {e}", spool.display()))?;
        let path = spool.join(format!("{}.gif", fresh_id()));
        Ok(Prepared { plan, source, path })
    }

    /// Asked for by the pill's Stop, the `stop-recording` action and the panel item. Spawns
    /// the async stop so the button handler stays synchronous.
    pub fn request_stop(self: &Rc<Self>) {
        let recorder = Rc::clone(self);
        glib::spawn_future_local(async move { recorder.stop().await });
    }

    /// The pill's Trash. `spec/06` §9 item 5 wants a confirmation, because a discard cannot
    /// be undone the way a card's Trash can.
    pub fn request_trash(self: &Rc<Self>) {
        let parent = self.active.borrow().as_ref().and_then(|a| a.pill.as_ref().map(|p| p.window().clone()));
        let dialog = adw::AlertDialog::builder()
            .heading("Discard the recording?")
            .body("The recording will be deleted. This cannot be undone.")
            .build();
        dialog.add_response("cancel", "Keep recording");
        dialog.add_response("discard", "Discard");
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let recorder = Rc::clone(self);
        dialog.connect_response(None, move |_, response| {
            if response == "discard" {
                let recorder = Rc::clone(&recorder);
                glib::spawn_future_local(async move { recorder.trash().await });
            }
        });
        dialog.present(parent.as_ref().map(|w| w.upcast_ref::<gtk::Widget>()));
    }

    /// `spec/06` §3 Stop: the graph drains to EOS, the last frames are kept, the session is
    /// stopped, and the recording goes through the after-recording flow. The indicator
    /// shows `Processing` until the flow has put up its card -- a moment, now that nothing
    /// is encoded at Stop (D113).
    async fn stop(self: &Rc<Self>) {
        self.stop_ticker();
        let Some(active) = self.active.borrow_mut().take() else {
            return;
        };
        let bridge = self.flow.bridge().clone();
        let elapsed_ms = ms(active.started.elapsed());
        if let Err(e) = bridge.set_recording_state(RecordingState::Processing, elapsed_ms).await {
            warn!("could not set the recording state: {e}");
        }
        if let Err(e) = bridge.show_recording_frame(active.frame, false).await {
            warn!("could not hide the recording frame: {e}");
        }
        if let Some(pill) = &active.pill {
            pill.close();
        }

        let Active { recording, frame, display, scale, .. } = active;
        let gif = recording.path().with_extension("gif");
        let stopped = recording.stop().await;
        // `spec/09` §4: the stop sound once the stream has ended, so that it is in no
        // recording, and before the card, which is the answer to it.
        self.flow.cue(Cue::RecordStop).await;
        match stopped {
            Ok(kept) => {
                info!(
                    reel = %kept.dir.display(),
                    taken = kept.taken,
                    kept = kept.kept,
                    bytes = kept.bytes,
                    "recorded"
                );
                let capture = build_capture(gif, kept.duration, frame, scale, display);
                write_twin(&capture);
                let outcome = self.flow.handle(&capture).await;
                // `ACT-05`: a plan with no card, or a card the stack refused, leaves
                // nothing on screen to say where the recording went.
                notify::capture_outcome(
                    &self.app,
                    &outcome,
                    Settings::load().notifications_enabled(),
                );
            }
            Err(e) => {
                warn!("the recording failed: {e}");
                notify::tell(&self.app, "recording", "The recording failed", &e.to_string(), true);
            }
        }
        if let Err(e) = bridge.set_recording_state(RecordingState::Idle, 0).await {
            warn!("could not clear the recording state: {e}");
        }
    }

    /// `spec/06` §3 Trash: everything torn down, the file deleted, nothing filed.
    async fn trash(self: &Rc<Self>) {
        self.stop_ticker();
        let Some(active) = self.active.borrow_mut().take() else {
            return;
        };
        let bridge = self.flow.bridge().clone();
        if let Err(e) = bridge.set_recording_state(RecordingState::Idle, 0).await {
            warn!("could not clear the recording state: {e}");
        }
        if let Err(e) = bridge.show_recording_frame(active.frame, false).await {
            warn!("could not hide the recording frame: {e}");
        }
        if let Some(pill) = &active.pill {
            pill.close();
        }
        let Active { recording, .. } = active;
        recording.trash().await;
        info!("recording discarded");
    }

    /// The half-second wake: advances the pill's clock and the panel timer, and if the
    /// stream has ended on its own -- the compositor closed the session, the monitor went
    /// away -- finalises the recording rather than leaving a dead pill on screen.
    fn tick(self: &Rc<Self>) {
        let (elapsed, ended) = {
            let active = self.active.borrow();
            let Some(a) = active.as_ref() else {
                return;
            };
            let elapsed = a.started.elapsed();
            if let Some(pill) = &a.pill {
                pill.set_elapsed(elapsed);
            }
            (elapsed, a.recording.has_ended())
        };

        let bridge = self.flow.bridge().clone();
        let elapsed_ms = ms(elapsed);
        glib::spawn_future_local(async move {
            let _ = bridge.set_recording_state(RecordingState::Recording, elapsed_ms).await;
        });

        if ended {
            info!("the stream ended on its own; finalising the recording");
            self.request_stop();
        }
    }

    fn stop_ticker(&self) {
        if let Some(id) = self.ticker.borrow_mut().take() {
            id.remove();
        }
    }

    /// The monitor to record: the named connector, else the one the selection sits on,
    /// else the current or first monitor.
    async fn source_for(&self, request: &RecordRequest) -> Option<StreamSource> {
        let monitors = match self.flow.bridge().monitors().await {
            Ok(monitors) => monitors,
            Err(e) => {
                warn!("could not read the monitors: {e}");
                return None;
            }
        };
        let monitor = request
            .display
            .as_ref()
            .and_then(|connector| monitors.iter().find(|m| m.connector == *connector))
            .or_else(|| monitors.iter().find(|m| contains_center(&m.geometry, &request.rect)))
            .or_else(|| monitors.iter().find(|m| m.current))
            .or_else(|| monitors.first())?;
        Some(StreamSource {
            connector: monitor.connector.clone(),
            rect: monitor.geometry,
            scale: monitor.scale,
            refresh: monitor.refresh,
        })
    }

    /// Builds the controls pill and asks the extension to place it just below the recorded
    /// rectangle, outside it.
    async fn build_pill(self: &Rc<Self>, rect: Rect, _source: &StreamSource) -> Rc<Pill> {
        let stop = {
            let weak = Rc::downgrade(self);
            move || {
                if let Some(recorder) = weak.upgrade() {
                    recorder.request_stop();
                }
            }
        };
        let trash = {
            let weak = Rc::downgrade(self);
            move || {
                if let Some(recorder) = weak.upgrade() {
                    recorder.request_trash();
                }
            }
        };
        let pill = Pill::new(&self.app, stop, trash);

        if let Some(path) = pill.object_path() {
            // Just below the area. For a whole monitor that is off the screen, and
            // `PlaceWindow` keeps a window on it (`spec/10` §3.1), so the pill lands in the
            // monitor's bottom-left corner.
            let placement = Placement::At { x: rect.x, y: rect.y + rect.height + PILL_GAP };
            match self.flow.bridge().place_window(&path, "recorder", &placement, 0).await {
                Ok(landed) => info!(?landed, "controls pill placed"),
                Err(e) => warn!("could not place the controls pill: {e}"),
            }
        } else {
            warn!("the controls pill has no object path; it cannot be placed");
        }
        pill
    }

}

/// Whether something other than the pill can stop a recording: the panel indicator's Stop,
/// or the `recording-stop` shortcut. Unknown counts as yes -- with no extension settings to
/// read there is no extension, and no recording either.
fn stoppable_elsewhere() -> bool {
    let Some(settings) = crate::settings::extension_settings() else { return true };
    let has = |key: &str| settings.settings_schema().is_some_and(|s| s.has_key(key));
    let indicator = !has("show-indicator") || settings.boolean("show-indicator");
    let shortcut = has("recording-stop") && !settings.strv("recording-stop").is_empty();
    indicator || shortcut
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if let Some(id) = self.ticker.borrow_mut().take() {
            id.remove();
        }
    }
}

/// Milliseconds of a duration as the wire's `u32`, saturating rather than wrapping on a
/// recording longer than 49 days.
fn ms(d: Duration) -> u32 {
    u32::try_from(d.as_millis()).unwrap_or(u32::MAX)
}

/// True when the monitor's rectangle holds the selection's centre.
fn contains_center(monitor: &Rect, selection: &Rect) -> bool {
    let cx = selection.x + selection.width / 2;
    let cy = selection.y + selection.height / 2;
    cx >= monitor.x
        && cx < monitor.x + monitor.width
        && cy >= monitor.y
        && cy < monitor.y + monitor.height
}

/// The recording as a capture: `Record` mode, the GIF's name, its length in the badge.
/// Logical `rect` and `scale` are the selection's, so the history and a restore know what
/// was recorded. The GIF itself is written from the reel beside it when it is needed.
fn build_capture(
    path: PathBuf,
    duration: Duration,
    rect: Rect,
    scale: f64,
    display: String,
) -> CaptureResult {
    let now = glib::real_time().unsigned_abs();
    CaptureResult {
        meta_path: path.with_extension("json"),
        path,
        mode: CaptureMode::Record,
        rect,
        scale,
        display,
        cursor_rect: None,
        source_window: SourceWindow::default(),
        window_alpha: false,
        timestamp: now,
        confirmed_at: Some(now),
        // No fly animation: a recording does not shrink from a selection into a card.
        animation_ms: 0,
        requested_action: None,
        modifiers: 0,
        duration_ms: Some(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)),
        external: false,
        linebreaks: None,
        project: None,
    }
}

/// Writes the recording's twin, so the spool entry is self-describing the way a
/// screenshot's is (`spec/10` §8) even if the app dies before the card closes.
fn write_twin(capture: &CaptureResult) {
    match serde_json::to_vec_pretty(capture) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(&capture.meta_path, bytes) {
                warn!(path = %capture.meta_path.display(), "could not write the twin: {e}");
            }
        }
        Err(e) => warn!("could not serialise the twin: {e}"),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// Builds an `a{sv}` payload the way the `record` action receives it.
    fn payload(build: impl FnOnce(&glib::VariantDict)) -> glib::Variant {
        let dict = glib::VariantDict::new(None);
        build(&dict);
        dict.end()
    }

    #[test]
    fn a_record_request_needs_a_rectangle() {
        let p = payload(|d| d.insert_value("display", &"eDP-1".to_variant()));
        assert!(RecordRequest::from_variant(&p).is_none());
    }

    #[test]
    fn a_record_request_reads_the_rect_display_and_cursor() {
        let p = payload(|d| {
            d.insert_value("rect", &(10i32, 20i32, 512i32, 384i32).to_variant());
            d.insert_value("display", &"eDP-1".to_variant());
            d.insert_value("cursor", &false.to_variant());
        });
        let request = RecordRequest::from_variant(&p).expect("a request");
        assert_eq!(request.rect, Rect::new(10, 20, 512, 384));
        assert_eq!(request.display.as_deref(), Some("eDP-1"));
        assert_eq!(request.cursor, Some(false));
    }

    #[test]
    fn a_record_request_leaves_absent_options_to_the_settings() {
        let p = payload(|d| d.insert_value("rect", &(0i32, 0i32, 100i32, 100i32).to_variant()));
        let request = RecordRequest::from_variant(&p).expect("a request");
        assert_eq!(request.display, None);
        assert_eq!(request.cursor, None);
    }

    /// D101: the toolbar's recording row travels as three more optional keys, each
    /// clamped to the settings page's range rather than refused -- sixty is a caller that
    /// has not heard the GIF ceiling, and it gets fifty.
    #[test]
    fn a_record_request_reads_the_gif_overrides_and_clamps_them() {
        let p = payload(|d| {
            d.insert_value("rect", &(0i32, 0i32, 100i32, 100i32).to_variant());
            d.insert_value("fps", &60i32.to_variant());
            d.insert_value("max-width", &640i32.to_variant());
            d.insert_value("quality", &130i32.to_variant());
        });
        let request = RecordRequest::from_variant(&p).expect("a request");
        assert_eq!(request.fps, Some(octosnap_media::gif::GIF_FPS_MAX));
        assert_eq!(request.max_width, Some(640));
        assert_eq!(request.quality, Some(100));
    }

    /// What the request said wins; what it left out is the setting, untouched.
    #[test]
    fn overrides_lay_over_the_settings_and_leave_the_rest_alone() {
        let p = payload(|d| {
            d.insert_value("rect", &(0i32, 0i32, 100i32, 100i32).to_variant());
            d.insert_value("fps", &0i32.to_variant());
            d.insert_value("cursor", &false.to_variant());
        });
        let request = RecordRequest::from_variant(&p).expect("a request");
        let mut gif = GifSettings { fps: 24, max_width: 1024, quality: 90, cursor: true };
        request.apply_to(&mut gif);
        assert_eq!(gif, GifSettings { fps: 0, max_width: 1024, quality: 90, cursor: false });
    }

    #[test]
    fn a_record_request_records_only_gifs() {
        let mp4 = payload(|d| {
            d.insert_value("rect", &(0i32, 0i32, 100i32, 100i32).to_variant());
            d.insert_value("record-format", &"mp4".to_variant());
        });
        assert!(RecordRequest::from_variant(&mp4).is_none(), "video is M9");

        let gif = payload(|d| {
            d.insert_value("rect", &(0i32, 0i32, 100i32, 100i32).to_variant());
            d.insert_value("record-format", &"GIF".to_variant());
        });
        assert!(RecordRequest::from_variant(&gif).is_some(), "gif, in any case, is fine");
    }

    #[test]
    fn the_selection_is_recorded_on_the_monitor_its_centre_is_on() {
        let left = Rect::new(0, 0, 1920, 1200);
        let right = Rect::new(1920, 0, 1920, 1200);
        let on_the_right = Rect::new(2400, 400, 200, 200);
        assert!(!contains_center(&left, &on_the_right));
        assert!(contains_center(&right, &on_the_right));
    }
}
