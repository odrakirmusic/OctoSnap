// SPDX-License-Identifier: GPL-3.0-or-later

//! A bridge that answers without a compositor.
//!
//! `spec/10` §11 wants integration tests that "drive the app through capture -> card ->
//! actions without a shell". Its defaults describe the current target machine
//! (1920x1200 at 1.25, see `docs/decisions.md`) so tests exercise the fractional-scale
//! path by default rather than the tidy 1.0 case that hides coordinate bugs.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use octosnap_core::protocol::PROTOCOL_VERSION;
use octosnap_core::qao::{self, Edge, Size};
use octosnap_core::request::CaptureRequest;
use octosnap_core::{Monitor, Rect, ScrollDirection};

use crate::bridge::{
    ClipboardImage, Cue, PickedColor, Placement, RecordingState, ShellBridge, ShellVersion,
};

/// One recorded `place_window`: the window's object path, its role, what was asked for
/// and where it landed.
pub type PlacementCall = (String, String, Placement, Rect);
use crate::error::BridgeError;

#[derive(Debug, Clone)]
pub struct NullBridge {
    version: ShellVersion,
    monitors: Vec<Monitor>,
    /// When set, every call fails with it, so degraded-mode paths can be tested.
    failure: Option<String>,
    /// Every path handed to `set_clipboard_image`, so a test can assert that the
    /// after-capture plan actually reached the clipboard rather than merely being
    /// computed. Shared so a clone of the bridge records into the same log.
    clipboard: Rc<RefCell<Vec<PathBuf>>>,
    /// Every string handed to `set_clipboard_text`, for the same reason.
    text: Rc<RefCell<Vec<String>>>,
    /// What `read_clipboard_image` answers (D169), and how often it was asked.
    clipboard_image: Rc<RefCell<Option<ClipboardImage>>>,
    clipboard_reads: Rc<RefCell<u32>>,
    /// Answer `read_clipboard_image` as an extension from before D169 does.
    clipboard_read_missing: bool,
    /// Every cue handed to `play_sound`, in order.
    sounds: Rc<RefCell<Vec<Cue>>>,
    /// Every request handed to `begin_capture`, so a test can assert that the panel
    /// menu, the CLI action and the URL handler all ask for the same thing.
    captures: Rc<RefCell<Vec<CaptureRequest>>>,
    cancels: Rc<RefCell<Vec<String>>>,
    /// What each window's frame size is, so `place_window` can answer with a rect the
    /// caller can stack against. Set by [`Self::set_window_size`].
    window_sizes: Rc<RefCell<HashMap<String, Size>>>,
    /// Every placement, with the rect it produced.
    placements: Rc<RefCell<Vec<PlacementCall>>>,
    focus_calls: Rc<RefCell<Vec<(String, bool)>>>,
    /// Every `set_window_shadow` call: the window, the radius and the opacity.
    shadow_calls: Rc<RefCell<Vec<(String, i32, f64)>>>,
    /// Every placement's requested slide duration, in order.
    animations: Rc<RefCell<Vec<(String, u32)>>>,
    /// What `pick_color` answers: a colour, or `None` for a user who pressed Escape.
    picked: Option<PickedColor>,
    pick_calls: Rc<RefCell<u32>>,
    /// Whether the (pretend) desktop icons are hidden; the null session always has some.
    desktop_hidden: Rc<RefCell<bool>>,
    /// Every `set_recording_state` call, in order, so a test can assert the app drove the
    /// indicator through idle → recording → processing → idle.
    recording_states: Rc<RefCell<Vec<(RecordingState, u32)>>>,
    /// Every `show_recording_frame` call, in order.
    recording_frames: Rc<RefCell<Vec<(Rect, bool)>>>,
    /// Every `show_scroll_frame` call, in order: the pill's object path and the selection.
    scroll_outlines: Rc<RefCell<Vec<(String, Rect)>>>,
    /// The frames `grab_frame` hands out, in order, and how far through them it is.
    ///
    /// The one place this bridge answers with something real rather than recording a
    /// call: a scrolling capture is a loop over frames, and a loop that is handed nothing
    /// cannot be tested at all. Past the end it repeats the last frame, which is exactly
    /// what a page nobody is scrolling looks like.
    scroll_frames: Rc<RefCell<Vec<PathBuf>>>,
    scroll_grabs: Rc<RefCell<usize>>,
    /// Every session started, with what it was started for.
    scroll_sessions: Rc<RefCell<Vec<(Rect, ScrollDirection)>>>,
    /// Every handle ended, so a test can assert the device was never left held.
    scroll_ends: Rc<RefCell<Vec<String>>>,
}

impl Default for NullBridge {
    fn default() -> Self {
        Self {
            version: ShellVersion {
                version: "0.0.0-null".to_owned(),
                protocol: PROTOCOL_VERSION,
            },
            monitors: vec![Monitor {
                index: 0,
                connector: "eDP-1".to_owned(),
                geometry: Rect::new(0, 0, 1536, 960),
                work_area: Rect::new(0, 0, 1536, 923),
                scale: 1.25,
                geometry_scale: 2,
                primary: true,
                current: true,
                refresh: Some(60.0),
            }],
            failure: None,
            clipboard: Rc::new(RefCell::new(Vec::new())),
            text: Rc::new(RefCell::new(Vec::new())),
            clipboard_image: Rc::new(RefCell::new(None)),
            clipboard_reads: Rc::new(RefCell::new(0)),
            clipboard_read_missing: false,
            sounds: Rc::new(RefCell::new(Vec::new())),
            captures: Rc::new(RefCell::new(Vec::new())),
            cancels: Rc::new(RefCell::new(Vec::new())),
            window_sizes: Rc::new(RefCell::new(HashMap::new())),
            placements: Rc::new(RefCell::new(Vec::new())),
            focus_calls: Rc::new(RefCell::new(Vec::new())),
            shadow_calls: Rc::new(RefCell::new(Vec::new())),
            animations: Rc::new(RefCell::new(Vec::new())),
            picked: None,
            pick_calls: Rc::new(RefCell::new(0)),
            desktop_hidden: Rc::new(RefCell::new(false)),
            recording_states: Rc::new(RefCell::new(Vec::new())),
            recording_frames: Rc::new(RefCell::new(Vec::new())),
            scroll_outlines: Rc::new(RefCell::new(Vec::new())),
            scroll_frames: Rc::new(RefCell::new(Vec::new())),
            scroll_grabs: Rc::new(RefCell::new(0)),
            scroll_sessions: Rc::new(RefCell::new(Vec::new())),
            scroll_ends: Rc::new(RefCell::new(Vec::new())),
        }
    }
}

impl NullBridge {
    #[must_use]
    pub fn with_monitors(mut self, monitors: Vec<Monitor>) -> Self {
        self.monitors = monitors;
        self
    }

    /// Makes every call report the extension as missing.
    #[must_use]
    pub fn unavailable(mut self, reason: impl Into<String>) -> Self {
        self.failure = Some(reason.into());
        self
    }

    #[must_use]
    pub fn with_protocol(mut self, protocol: u32) -> Self {
        self.version.protocol = protocol;
        self
    }

    /// Makes `pick_color` answer this colour rather than a cancel.
    #[must_use]
    pub fn with_picked_color(mut self, r: f64, g: f64, b: f64) -> Self {
        self.picked = Some(PickedColor { r, g, b });
        self
    }

    /// What the (pretend) clipboard holds for `read_clipboard_image`: a file the reader
    /// takes, as it takes the extension's.
    pub fn set_clipboard_image(&self, image: Option<ClipboardImage>) {
        *self.clipboard_image.borrow_mut() = image;
    }

    /// An extension from before D169, which has no `ReadClipboardImage`.
    #[must_use]
    pub const fn without_clipboard_read(mut self) -> Self {
        self.clipboard_read_missing = true;
        self
    }

    /// How many times the clipboard was read through the shell.
    #[must_use]
    pub fn clipboard_reads(&self) -> u32 {
        *self.clipboard_reads.borrow()
    }

    /// How many times the pipette asked the shell.
    #[must_use]
    pub fn pick_calls(&self) -> u32 {
        *self.pick_calls.borrow()
    }

    /// What was copied to the clipboard, in order.
    #[must_use]
    pub fn clipboard_calls(&self) -> Vec<PathBuf> {
        self.clipboard.borrow().clone()
    }

    /// Every string put on the clipboard, in order.
    #[must_use]
    pub fn clipboard_text(&self) -> Vec<String> {
        self.text.borrow().clone()
    }

    /// Every cue asked for, in order.
    #[must_use]
    pub fn sounds(&self) -> Vec<Cue> {
        self.sounds.borrow().clone()
    }

    /// What was asked of `begin_capture`, in order.
    #[must_use]
    pub fn capture_calls(&self) -> Vec<CaptureRequest> {
        self.captures.borrow().clone()
    }

    /// Handles passed to `cancel_capture`, in order.
    #[must_use]
    pub fn cancel_calls(&self) -> Vec<String> {
        self.cancels.borrow().clone()
    }

    /// Tells the bridge how big a window is, so `place_window` can answer for it.
    ///
    /// The real extension reads this off `Meta.Window.get_frame_rect()`; here the test
    /// has to say, because there is no window.
    pub fn set_window_size(&self, object_path: &str, size: Size) {
        self.window_sizes.borrow_mut().insert(object_path.to_owned(), size);
    }

    /// Every `place_window` call with the rect it produced, in order.
    #[must_use]
    pub fn placement_calls(&self) -> Vec<PlacementCall> {
        self.placements.borrow().clone()
    }

    /// Every `focus_window` call, in order.
    #[must_use]
    pub fn focus_calls(&self) -> Vec<(String, bool)> {
        self.focus_calls.borrow().clone()
    }

    /// Every `set_window_shadow` call, in order.
    #[must_use]
    pub fn shadow_calls(&self) -> Vec<(String, i32, f64)> {
        self.shadow_calls.borrow().clone()
    }

    /// Every placement's requested slide duration, in order.
    #[must_use]
    pub fn animation_calls(&self) -> Vec<(String, u32)> {
        self.animations.borrow().clone()
    }

    /// Every `set_recording_state` call, in order.
    #[must_use]
    pub fn recording_state_calls(&self) -> Vec<(RecordingState, u32)> {
        self.recording_states.borrow().clone()
    }

    /// The PNGs `grab_frame` will hand out, in order. Past the end it repeats the last.
    #[must_use]
    pub fn with_scroll_frames(self, frames: Vec<PathBuf>) -> Self {
        *self.scroll_frames.borrow_mut() = frames;
        self
    }

    /// Every scroll-assist session started, with the rect and direction it asked for.
    #[must_use]
    pub fn scroll_sessions(&self) -> Vec<(Rect, ScrollDirection)> {
        self.scroll_sessions.borrow().clone()
    }

    /// Every handle `end_scroll_assist` was called with.
    #[must_use]
    pub fn scroll_ends(&self) -> Vec<String> {
        self.scroll_ends.borrow().clone()
    }

    /// How many frames `grab_frame` has handed out, the repeats of the last one included.
    ///
    /// What a test waiting for "every frame I listed has been offered" reads, since a
    /// frame the loop declines never reaches its progress callback (D98).
    #[must_use]
    pub fn scroll_grabs(&self) -> usize {
        *self.scroll_grabs.borrow()
    }

    /// Every `show_recording_frame` call, in order.
    #[must_use]
    pub fn recording_frame_calls(&self) -> Vec<(Rect, bool)> {
        self.recording_frames.borrow().clone()
    }

    /// Every `show_scroll_frame` call, in order.
    #[must_use]
    pub fn scroll_outline_calls(&self) -> Vec<(String, Rect)> {
        self.scroll_outlines.borrow().clone()
    }

    fn guard(&self) -> Result<(), BridgeError> {
        match &self.failure {
            Some(reason) => Err(BridgeError::Unavailable(reason.clone())),
            None => Ok(()),
        }
    }
}

impl ShellBridge for NullBridge {
    async fn version(&self) -> Result<ShellVersion, BridgeError> {
        self.guard()?;
        Ok(self.version.clone())
    }

    async fn set_spool(&self, _dir: &Path) -> Result<(), BridgeError> {
        self.guard()
    }

    async fn recent_log(&self) -> Result<Vec<String>, BridgeError> {
        self.guard()?;
        Ok(vec!["INFO null bridge: no extension, no log".to_owned()])
    }

    async fn monitors(&self) -> Result<Vec<Monitor>, BridgeError> {
        self.guard()?;
        Ok(self.monitors.clone())
    }

    async fn begin_capture(&self, request: &CaptureRequest) -> Result<String, BridgeError> {
        self.guard()?;
        self.captures.borrow_mut().push(request.clone());
        // A stable, obviously-fake handle: a test asserting on it is asserting on the
        // call having happened, which is the only thing this bridge can promise.
        Ok(format!("null-{}", self.captures.borrow().len()))
    }

    async fn cancel_capture(&self, handle: &str) -> Result<(), BridgeError> {
        self.guard()?;
        self.cancels.borrow_mut().push(handle.to_owned());
        Ok(())
    }

    async fn set_clipboard_image(&self, path: &Path) -> Result<(), BridgeError> {
        self.guard()?;
        self.clipboard.borrow_mut().push(path.to_path_buf());
        Ok(())
    }

    async fn read_clipboard_image(&self) -> Result<Option<ClipboardImage>, BridgeError> {
        self.guard()?;
        *self.clipboard_reads.borrow_mut() += 1;
        if self.clipboard_read_missing {
            return Err(BridgeError::Unsupported { method: "ReadClipboardImage" });
        }
        Ok(self.clipboard_image.borrow().clone())
    }

    async fn set_clipboard_text(&self, text: &str) -> Result<(), BridgeError> {
        self.guard()?;
        self.text.borrow_mut().push(text.to_owned());
        Ok(())
    }

    async fn play_sound(&self, cue: Cue) -> Result<(), BridgeError> {
        self.guard()?;
        self.sounds.borrow_mut().push(cue);
        Ok(())
    }

    /// Runs the same arithmetic the extension would, against the first monitor's work
    /// area, and reports the result as the landed rect.
    ///
    /// Not a stub returning zeroes: `spec/10` §11 wants the app driven "through capture ->
    /// card -> actions without a shell", and a card stack whose every member lands at the
    /// origin would let a placement bug pass every test in the suite. Computing it here
    /// means a test can assert that three cards do not overlap -- which is the property
    /// that actually matters -- without a compositor. The clamping half is not modelled:
    /// this bridge cannot know what Mutter would refuse, and pretending to would be a
    /// second, wrong implementation of the thing under test.
    async fn place_window(
        &self,
        object_path: &str,
        role: &str,
        placement: &Placement,
        animate_ms: u32,
    ) -> Result<Rect, BridgeError> {
        self.guard()?;
        // Where a window lands does not depend on how it got there, so the duration is
        // recorded rather than acted on -- a test asserting that a reflow animates is
        // asserting about the request, which is all this side of the boundary owns.
        self.animations.borrow_mut().push((object_path.to_owned(), animate_ms));
        // The size the caller declared, exactly as the extension prefers it, falling back
        // to whatever a test registered for this window.
        let size = match *placement {
            Placement::Stacked { size, .. } if size.width > 0 && size.height > 0 => size,
            _ => self
                .window_sizes
                .borrow()
                .get(object_path)
                .copied()
                .unwrap_or(Size::new(
                    207 + 2 * qao::SHADOW_MARGIN,
                    129 + 2 * qao::SHADOW_MARGIN,
                )),
        };

        let work = self
            .monitors
            .first()
            .map_or_else(|| Rect::new(0, 0, 1920, 1080), |m| m.work_area);

        let landed = match *placement {
            Placement::At { x, y } => Rect::new(x, y, size.width, size.height),
            Placement::Stacked { edge, offset, inset, .. } => {
                let band = inset.clamp(0, qao::MARGIN);
                let outer = qao::MARGIN - band;
                let x = match edge {
                    Edge::Left => work.x + outer,
                    Edge::Right => work.x + work.width - outer - size.width,
                };
                let y = work.y + work.height - outer - offset - size.height;
                Rect::new(x, y, size.width, size.height)
            }
        };

        self.placements
            .borrow_mut()
            .push((object_path.to_owned(), role.to_owned(), *placement, landed));
        Ok(landed)
    }

    /// Moves the window's last known rect, so a test can nudge a pin and see where it
    /// would be. Not clamped: this bridge cannot know what Mutter would refuse.
    async fn move_window_by(
        &self,
        object_path: &str,
        dx: i32,
        dy: i32,
    ) -> Result<Rect, BridgeError> {
        self.guard()?;
        let mut placements = self.placements.borrow_mut();
        let last = placements
            .iter()
            .rev()
            .find(|(path, _, _, _)| path == object_path)
            .map(|(_, _, _, rect)| *rect)
            .unwrap_or_else(|| Rect::new(0, 0, 0, 0));
        let moved = Rect::new(last.x + dx, last.y + dy, last.width, last.height);
        placements.push((object_path.to_owned(), "move".to_owned(), Placement::At { x: moved.x, y: moved.y }, moved));
        Ok(moved)
    }

    async fn pick_color(&self) -> Result<Option<PickedColor>, BridgeError> {
        self.guard()?;
        *self.pick_calls.borrow_mut() += 1;
        Ok(self.picked)
    }

    async fn desktop_icons(&self, what: &str) -> Result<(bool, bool), BridgeError> {
        self.guard()?;
        let mut hidden = self.desktop_hidden.borrow_mut();
        *hidden = match what {
            "hide" => true,
            "show" => false,
            _ => !*hidden,
        };
        Ok((*hidden, true))
    }

    async fn set_window_shadow(
        &self,
        object_path: &str,
        radius: i32,
        opacity: f64,
    ) -> Result<(), BridgeError> {
        self.guard()?;
        self.shadow_calls.borrow_mut().push((object_path.to_owned(), radius, opacity));
        Ok(())
    }

    async fn focus_window(&self, object_path: &str, focus: bool) -> Result<bool, BridgeError> {
        self.guard()?;
        self.focus_calls.borrow_mut().push((object_path.to_owned(), focus));
        Ok(true)
    }

    async fn set_recording_state(
        &self,
        state: RecordingState,
        elapsed_ms: u32,
    ) -> Result<(), BridgeError> {
        self.guard()?;
        self.recording_states.borrow_mut().push((state, elapsed_ms));
        Ok(())
    }

    async fn set_update_offered(&self, _offered: bool) -> Result<(), BridgeError> {
        self.guard()
    }

    async fn show_recording_frame(&self, rect: Rect, visible: bool) -> Result<(), BridgeError> {
        self.guard()?;
        self.recording_frames.borrow_mut().push((rect, visible));
        Ok(())
    }

    async fn show_scroll_frame(&self, object_path: &str, rect: Rect) -> Result<(), BridgeError> {
        self.guard()?;
        self.scroll_outlines.borrow_mut().push((object_path.to_owned(), rect));
        Ok(())
    }

    async fn start_scroll_assist(&self, rect: Rect, direction: ScrollDirection) -> Result<String, BridgeError> {
        self.guard()?;
        self.scroll_sessions.borrow_mut().push((rect, direction));
        *self.scroll_grabs.borrow_mut() = 0;
        Ok(format!("null-scroll-{}", self.scroll_sessions.borrow().len()))
    }

    async fn grab_frame(&self, _handle: &str) -> Result<PathBuf, BridgeError> {
        self.guard()?;
        let frames = self.scroll_frames.borrow();
        let Some(last) = frames.len().checked_sub(1) else {
            return Err(BridgeError::MalformedReply {
                method: "GrabFrame",
                detail: "this NullBridge was given no frames to hand out".to_owned(),
            });
        };
        let mut at = self.scroll_grabs.borrow_mut();
        let frame = frames[(*at).min(last)].clone();
        *at += 1;
        // Copied, because the real one hands out a **new file every time** -- the
        // extension writes each grab into `$XDG_RUNTIME_DIR` and the app deletes it as
        // soon as it has read it (`scrolling::read`). A bridge that answered with the
        // same path twice would have its second answer already deleted, which is a
        // property of this bridge and not of anything it is standing in for.
        let copy = frame.with_file_name(format!("null-grab-{at}.png"));
        std::fs::copy(&frame, &copy).map_err(|why| BridgeError::MalformedReply {
            method: "GrabFrame",
            detail: format!("{}: {why}", frame.display()),
        })?;
        Ok(copy)
    }

    async fn end_scroll_assist(&self, handle: &str) -> Result<(), BridgeError> {
        self.guard()?;
        self.scroll_ends.borrow_mut().push(handle.to_owned());
        Ok(())
    }
}
