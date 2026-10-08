// SPDX-License-Identifier: GPL-3.0-or-later

//! `CaptureFlow`: applies the `ACT-01` plan to a capture that has arrived.
//!
//! `octosnap_core::actions` decides *what* should happen; this decides *how*, and does
//! it. Keeping the two apart is what lets the decision be tested exhaustively without a
//! session bus, a display or a filesystem, which is where the interesting rules live.
//!
//! Generic over the bridge rather than boxed, so `spec/10` §11's "`NullBridge` drives the
//! app through capture -> card -> actions without a shell" is a plain unit test.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;
use octosnap_core::actions::{AfterAction, ClipboardPlan, ConfirmModifiers, Plan, Policy, resolve};
use octosnap_core::request::CaptureRequest;
use octosnap_core::history::Kind;
use octosnap_core::savepath::{
    DEFAULT_JPEG_QUALITY, ImageFormat, recording_dir, save_as_target, screenshot_dir, unique_path,
};
use octosnap_core::{CaptureResult, Rect, filename};
use octosnap_shell::{BridgeError, Cue, ShellBridge};
use tracing::{info, warn};

use crate::encode::{self, EncodeError};
use crate::recording::render;

/// `ACT-06` and `ACT-07` settings. Defaults are `spec/08`'s.
#[derive(Debug, Clone)]
pub struct SaveConfig {
    /// `screenshot-folder`; `None` means XDG Pictures/Screenshots.
    pub folder: Option<PathBuf>,
    /// `recording-folder`; `None` means XDG Videos/Screencasts. A recording saves here,
    /// with its own file extension, rather than into the screenshot folder as a PNG.
    pub recording_folder: Option<PathBuf>,
    /// `shot-format`: what a screenshot is saved as, by every Save (D164).
    pub format: ImageFormat,
    /// `shot-jpg-quality`, 1 to 100.
    pub jpeg_quality: u8,
    pub template: String,
    pub counter_start: u32,
    pub counter_width: u8,
    pub utc: bool,
    /// `CAP-16`: append `@2x` when the capture is denser than 1x.
    pub retina_suffix: bool,
    /// Where a GIF the clipboard names is kept (D165); `None` means the app's cache,
    /// `$XDG_CACHE_HOME/octosnap/clipboard`. Not a setting: a test points it at its own
    /// directory, as it does the two folders above.
    pub clipboard_folder: Option<PathBuf>,
}

impl Default for SaveConfig {
    fn default() -> Self {
        Self {
            folder: None,
            recording_folder: None,
            format: ImageFormat::default(),
            jpeg_quality: DEFAULT_JPEG_QUALITY,
            template: filename::DEFAULT_TEMPLATE.to_owned(),
            counter_start: 1,
            counter_width: 1,
            utc: false,
            retina_suffix: true,
            clipboard_folder: None,
        }
    }
}

/// What the flow actually did, for logging and for the caller's UI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    pub copied: bool,
    pub saved_to: Option<PathBuf>,
    /// Actions in the plan that this build cannot perform yet.
    pub deferred: Vec<AfterAction>,
    /// True when a card was actually put on screen.
    ///
    /// **"Was shown", not "was asked for."** `notify::describe` stays silent when this is
    /// true, on the sound rule that the card *is* the notification and saying it twice is
    /// noise -- but that rule only holds if the card exists. It does not until `M2`, and
    /// setting this from the plan meant the default `['show-overlay', 'copy']` produced a
    /// capture with **no card and no notification**: a shutter, an animation, and no way
    /// to tell whether anything had been written. `docs/decisions.md` D27.
    pub card_shown: bool,
    /// True when the plan's `pin` put the capture on the screen (`spec/07` §3.1's
    /// "after-capture action" source). As visible as a card, and reported the same way.
    pub pinned: bool,
    /// What a text capture came to, and `None` for every capture that is not one.
    pub recognised: Option<Recognised>,
    /// A recording's GIF rather than a screenshot, which is what a notification calls it.
    pub recording: bool,
}

impl Outcome {
    /// Whether this was a read that found no language pack, and now waits for one (D171).
    #[must_use]
    pub fn waits_for_pack(&self) -> bool {
        matches!(&self.recognised, Some(Recognised::Failed(why)) if why == NO_PACK)
    }
}

/// `spec/07` §2.1's text capture, as far as the user is concerned.
///
/// Three outcomes and not a `Result<String, String>`, because "there was no text in it"
/// is not a failure -- it is the right answer for a photograph, and the notification for
/// it says something different from the one for a model that would not load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recognised {
    /// On the clipboard. The string is the first line, which is what §2.1 previews.
    Copied(String),
    /// Read, and there was nothing to read.
    Nothing,
    /// Not read. The message is already phrased for the user.
    Failed(String),
}

/// What a text capture says when there is nothing installed to read it with.
///
/// Phrased as the missing step rather than as a failure, and it names the size. Since
/// D171 it is also the sign, matched by value, that the read now waits for a pack: no
/// notification carries it any more, because Settings opens on the packs instead
/// (`notify::capture_outcome`), and the editor says it in its own words.
pub const NO_PACK: &str =
    "No language pack is installed. Settings > Advanced has them, from about 13 MB.";

/// How long a read that found no pack waits for one (D171).
///
/// Long enough to choose a pack and download it, with time over for a slow connection.
/// Not open-ended: a pack installed from Settings an afternoon later would otherwise read
/// a capture the user has long forgotten and replace whatever is on the clipboard by then.
pub const WAITS_FOR: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// A read that found no language pack, kept so that installing one finishes it (D171).
///
/// The file is kept as it was asked for, and nothing about the capture is copied: a text
/// capture's file stays in the spool with its twin (`spec/10` §8), and a card's, a pin's
/// or the editor's render stays beside it. Only a file that is gone by the time the pack
/// arrives is dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiting {
    pub path: PathBuf,
    pub rect: Option<Rect>,
    pub linebreaks: Option<bool>,
    pub asked: std::time::Instant,
}

impl Waiting {
    /// Whether this read should still happen at `now`, and if not, why not.
    ///
    /// # Errors
    /// The reason it is dropped, for the log.
    pub fn due(&self, now: std::time::Instant, exists: bool) -> Result<(), &'static str> {
        if now.saturating_duration_since(self.asked) > WAITS_FOR {
            return Err("it waited longer than a pack takes to install");
        }
        if !exists {
            return Err("its file is gone");
        }
        Ok(())
    }
}

/// What puts a capture on screen as a card, and reports whether one appeared.
///
/// A callback rather than a direct dependency on `qao::Qao`, because the overlay owns a
/// `CaptureFlow` (it needs Copy and Save) and the flow has to show cards. Injecting the
/// shower breaks that cycle and keeps the flow testable: `spec/10` §11's "drive the app
/// through capture -> card -> actions without a shell" installs a recorder here.
///
/// The `bool` is load-bearing. `Outcome::card_shown` records what happened, not what was
/// planned (`docs/decisions.md` D27), so a shower that fails hands the capture back to
/// `notify` rather than leaving the user with a shutter sound and nothing else.
/// The saved copy's path rather than a bare "it was saved", because the card's Trash
/// button has to name a file (`spec/04` §3) and `bool` cannot. It stays `Option` because
/// most captures are never written out.
pub type CardShower = Box<dyn Fn(&CaptureResult, Option<&std::path::Path>) -> bool>;

/// Opens `spec/05`'s editor on a capture, for the plan's `annotate` action.
///
/// Installed the way [`CardShower`] is and for the same reason: the editor needs the
/// overlay's `EditorActions`, which are built from this flow, so the flow cannot open one
/// itself without a cycle. `octosnap capture --action annotate` used to log the action
/// as "not implemented" while the card's pencil opened the very same editor.
pub type EditorOpener = Box<dyn Fn(&CaptureResult)>;

/// Pins a capture where it was taken, for the plan's `pin` action (`ACT-01`, `spec/07`
/// §3.1). Installed like [`CardShower`], answers like it: whether a pin appeared. The
/// saved copy's path travels with the capture for the card the pin hands back (D47).
pub type Pinner = Box<dyn Fn(&CaptureResult, Option<&std::path::Path>) -> bool>;

/// Puts `docs/decisions.md` D96's indicator on screen over an area being read, and
/// answers with the way to take it down again.
///
/// Installed like [`CardShower`] and for the same reason -- the indicator is a GTK window
/// the extension has to place, which is nothing the flow knows about. The `Rect` is the
/// area the text was taken from, and `None` when the read is of a file rather than of a
/// capture (`spec/07` §2.1's `capture-text?filepath=`), which has no place on screen to
/// point at.
///
/// `Option` on the way out as well: a build with no overlay, or a compositor that will
/// not place the window, leaves the read with nothing to take down.
///
/// An `Rc` where the other three are a `Box`, because this is the only one that is not
/// called while the flow is being asked: it is handed to a timer that outlives the call
/// and may never run it at all.
pub type ReadingShower = Rc<dyn Fn(Option<Rect>) -> Option<crate::ocr::reading::Hide>>;

pub struct CaptureFlow<B: ShellBridge> {
    bridge: B,
    /// Behind a `RefCell` so a settings change takes effect on a running service. The
    /// Preferences dialog is expected to change these while captures are happening, and
    /// rebuilding the flow instead would reset the counter below.
    policy: RefCell<Policy>,
    /// `after-recording`, applied instead of `policy` when the capture is a recording
    /// (`spec/08` §5). A GIF's default plan is show-overlay and save, not copy.
    recording_policy: RefCell<Policy>,
    save: RefCell<SaveConfig>,
    /// `ACT-07`'s `{n}`. Runtime-only for now; `spec/08` persists it.
    counter: Cell<u32>,
    /// The most recent capture, for `spec/10` §3.2's `copy-last` / `save-last` /
    /// `annotate-last` actions and the panel menu items that invoke them.
    ///
    /// The capture lives in the spool, so remembering the path is enough and nothing is
    /// held in memory. It survives a settings change, which is why it sits here rather
    /// than being derived from the spool's newest file: "last" means the last one *this
    /// session took*, not the newest file on disk, and those differ the moment a second
    /// OctoSnap or a stale spool entry exists.
    last: RefCell<Option<CaptureResult>>,
    /// Where the last capture was written, if it was. Distinct from `last`: a capture
    /// that was only copied has no file outside the spool, and offering to reveal a
    /// spool file would show the user a temporary directory.
    last_saved: RefCell<Option<PathBuf>>,
    /// Installed by the app once the Quick Access Overlay exists. `None` means this build
    /// has no card, and the plan's `show-overlay` is reported as deferred.
    card_shower: RefCell<Option<CardShower>>,
    /// Installed beside the card shower. `None` means the plan's `annotate` is deferred.
    editor_opener: RefCell<Option<EditorOpener>>,
    pinner: RefCell<Option<Pinner>>,
    /// Installed beside the card shower. `None` means a slow read says nothing.
    reading_shower: RefCell<Option<ReadingShower>>,
    /// The Copy and Save a recording's plan holds back while its card is up, by the
    /// capture's path (D113): see [`Self::release`].
    held: RefCell<HashMap<PathBuf, Held>>,
    /// The last read that found no language pack, until a pack is installed (D171).
    waiting: RefCell<Option<Waiting>>,
}

/// What a recording's plan held back for its card: the outputs that need its GIF written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Held {
    pub copy: bool,
    pub save: bool,
}

impl<B: ShellBridge + std::fmt::Debug> std::fmt::Debug for CaptureFlow<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureFlow")
            .field("bridge", &self.bridge)
            .field("policy", &self.policy)
            .field("counter", &self.counter)
            .field("has_card_shower", &self.card_shower.borrow().is_some())
            .finish_non_exhaustive()
    }
}

impl<B: ShellBridge> CaptureFlow<B> {
    pub fn new(bridge: B, policy: Policy, save: SaveConfig) -> Self {
        let counter = Cell::new(save.counter_start);
        Self {
            bridge,
            policy: RefCell::new(policy),
            recording_policy: RefCell::new(Policy::default()),
            save: RefCell::new(save),
            counter,
            last: RefCell::new(None),
            last_saved: RefCell::new(None),
            card_shower: RefCell::new(None),
            editor_opener: RefCell::new(None),
            pinner: RefCell::new(None),
            reading_shower: RefCell::new(None),
            held: RefCell::default(),
            waiting: RefCell::new(None),
        }
    }

    /// Installs the thing that shows cards (`spec/04`).
    pub fn set_card_shower(&self, shower: CardShower) {
        *self.card_shower.borrow_mut() = Some(shower);
    }

    /// Installs the thing that opens the editor (`spec/05`).
    pub fn set_pinner(&self, pinner: Pinner) {
        *self.pinner.borrow_mut() = Some(pinner);
    }

    pub fn set_editor_opener(&self, opener: EditorOpener) {
        *self.editor_opener.borrow_mut() = Some(opener);
    }

    /// Installs the thing that says a read is under way (D96).
    pub fn set_reading_shower(&self, shower: ReadingShower) {
        *self.reading_shower.borrow_mut() = Some(shower);
    }

    /// Installs `after-recording`, the plan a recording runs instead of the screenshot
    /// one. Set once at startup and again on every settings change, beside `update`.
    pub fn set_recording_policy(&self, policy: Policy) {
        *self.recording_policy.borrow_mut() = policy;
    }

    /// Asks the extension to start a capture (`spec/10` §3.1 `BeginCapture`).
    ///
    /// The result does not come back here: it arrives later as the `handle-capture`
    /// action, which is the cold-activation-safe path (`docs/spikes/13`). So this returns
    /// the handle and nothing else, and the two halves of one capture are connected only
    /// by the extension's own bookkeeping. That is the contract, not a shortcut.
    pub async fn begin(&self, request: &CaptureRequest) -> Result<String, BridgeError> {
        let handle = self.bridge.begin_capture(request).await?;
        info!(mode = request.mode.as_wire(), handle, "capture started");
        Ok(handle)
    }

    /// The shell bridge, for callers that need the extension directly.
    ///
    /// The Quick Access Overlay needs `PlaceWindow` and `FocusWindow`, which are not
    /// capture-flow concerns and would only be pass-through methods here. Exposing the
    /// bridge is honest about that; wrapping it would suggest the flow had an opinion
    /// about window geometry, which it does not.
    #[must_use]
    pub const fn bridge(&self) -> &B {
        &self.bridge
    }

    /// One of the app's sounds (D134), if `ui-sounds` is on.
    ///
    /// Fire-and-forget on the extension's side, and a failure is only a log line: a sound
    /// daemon that is not there must not turn a copy or a pin into a failure.
    pub async fn cue(&self, cue: Cue) {
        if !crate::settings::ui_sounds() {
            return;
        }
        if let Err(e) = self.bridge.play_sound(cue).await {
            warn!(cue = %cue.wire(), "could not play a sound: {e}");
        }
    }

    /// The last capture this session took, if any.
    #[must_use]
    pub fn last_capture(&self) -> Option<CaptureResult> {
        self.last.borrow().clone()
    }

    /// Where the last capture was saved outside the spool, if it was.
    #[must_use]
    pub fn last_saved(&self) -> Option<PathBuf> {
        self.last_saved.borrow().clone()
    }

    /// Whether the most recent capture was a recording, for what `save-last` calls it.
    #[must_use]
    pub fn last_is_recording(&self) -> bool {
        self.last.borrow().as_ref().is_some_and(is_recording)
    }

    /// `spec/10` §3.2's `copy-last`: puts the most recent capture back on the clipboard.
    pub async fn copy_last(&self) -> Result<PathBuf, SaveError> {
        let capture = self.last_capture().ok_or(SaveError::NothingCaptured)?;
        self.to_clipboard(&capture).await?;
        info!(path = %capture.path.display(), "copied the last capture again");
        self.cue(Cue::Copied).await;
        Ok(capture.path)
    }

    /// `spec/10` §3.2's `save-last`: writes the most recent capture to the export
    /// location, even if the after-capture set never asked for a file.
    ///
    /// Saving twice deliberately produces a second file rather than reporting the first:
    /// `unique_path` is what stops a re-save from overwriting, and a user who presses
    /// Save twice has asked for it. The alternative -- silently doing nothing the second
    /// time -- looks like the shortcut is broken.
    pub async fn save_last(&self) -> Result<PathBuf, SaveError> {
        let capture = self.last_capture().ok_or(SaveError::NothingCaptured)?;
        let path = self.save(&capture).await?;
        *self.last_saved.borrow_mut() = Some(path.clone());
        info!(path = %path.display(), "saved the last capture on request");
        Ok(path)
    }

    /// Copies one capture to the clipboard, for a card's Copy (`spec/04` §3), and every
    /// other Copy the user presses: the editor's, a pin's, the history's.
    ///
    /// Distinct from `copy_last`: a stack holds several cards and the one the user
    /// clicked is very often not the newest. Sharing `copy_last`'s "the last capture"
    /// lookup would copy the wrong image whenever more than one card is open, which is
    /// the case `spec/04` §5 exists for.
    ///
    /// These copies tink and the after-capture copy does not. That one is part of the
    /// capture, whose shutter has just sounded.
    pub async fn copy(&self, capture: &CaptureResult) -> Result<(), SaveError> {
        self.to_clipboard(capture).await?;
        info!(path = %capture.path.display(), "copied a card to the clipboard");
        self.cue(Cue::Copied).await;
        Ok(())
    }

    /// Writes one capture to the export location, for a card's Save (`spec/04` §3).
    ///
    /// Records it as `last_saved` so "Show in Files" and `reveal-last` point at the file
    /// the user just made rather than at an older one.
    pub async fn save_capture(&self, capture: &CaptureResult) -> Result<PathBuf, SaveError> {
        let path = self.save(capture).await?;
        *self.last_saved.borrow_mut() = Some(path.clone());
        info!(path = %path.display(), "saved a card");
        Ok(path)
    }

    /// The name a capture would be saved under, without saving it.
    ///
    /// Needed by `spec/04` §3's "ask for destination" chooser, which has to be pre-filled
    /// with the templated name -- the point of the template is that the user does not
    /// have to think of a name, and a chooser that opens on "Untitled" throws that away.
    /// Does **not** advance `ACT-07`'s `{n}`: the counter belongs to files that were
    /// actually written, and a cancelled chooser must not consume one.
    #[must_use]
    pub fn suggested_name(&self, capture: &CaptureResult) -> String {
        let config = self.save.borrow().clone();
        let (width, height) = capture.rect.to_physical(capture.scale);
        let recording = is_recording(capture);
        let context = filename::Context {
            timestamp: chrono_from_micros(capture.timestamp),
            use_utc: config.utc,
            counter: self.counter.get(),
            counter_width: config.counter_width,
            app: capture.source_window.app_name.clone(),
            window: capture.source_window.title.clone(),
            capture_type: capture_type(recording),
            width,
            height,
            retina_suffix: !recording && config.retina_suffix && capture.scale > 1.0,
        };
        let extension = if recording {
            recording_extension(capture)
        } else {
            still_format(capture, config.format).extension().to_owned()
        };
        format!("{}.{}", filename::render(&config.template, &context), extension)
    }

    /// `shot-format` as it stands, for a chooser the flow does not open itself: the
    /// editor's Save As names its file with it.
    #[must_use]
    pub fn image_format(&self) -> ImageFormat {
        self.save.borrow().format
    }

    /// Writes a capture to a path the user chose (`spec/04` §3's "ask for destination").
    ///
    /// Separate from [`Self::save_capture`] rather than a flag on it, because the two
    /// differ in more than the destination: this one does not consult the template, does
    /// not touch the `{n}` counter, and must not apply `unique_path` -- the user picked
    /// that name, and quietly writing `name-1.png` instead would be the one thing a
    /// chooser is supposed to make impossible.
    ///
    /// A screenshot is written in the format its name asks for, whatever `shot-format`
    /// says (D164). The two cases where the name is not kept are the ones where keeping
    /// it would mean bytes it does not describe: a name with no format in it gains the
    /// configured one's extension, and a format too small for the picture gives way to
    /// PNG. The answer is the path written, which is how the caller learns of either.
    pub async fn save_capture_as(
        &self,
        capture: &CaptureResult,
        destination: &Path,
    ) -> Result<PathBuf, SaveError> {
        render::ensure(&capture.path).await.map_err(SaveError::Render)?;
        let written = if is_recording(capture) {
            copy_file(&capture.path, destination, true).await?;
            destination.to_path_buf()
        } else {
            let config = self.save.borrow().clone();
            let size = encode::peek(&capture.path).1;
            let target = save_as_target(
                destination,
                config.format,
                |format| size.is_none_or(|(w, h)| format.for_size(w, h) == format),
                |p| p.exists(),
            );
            if target.path != destination {
                info!(
                    chosen = %destination.display(),
                    path = %target.path.display(),
                    "the chosen name could not hold the picture's format"
                );
            }
            write_still(&capture.path, &target.path, target.format, config.jpeg_quality, target.chosen)
                .await?;
            target.path
        };
        *self.last_saved.borrow_mut() = Some(written.clone());
        info!(path = %written.display(), "saved a card to a chosen path");
        Ok(written)
    }

    /// Applies changed settings in place.
    ///
    /// The counter is only reset when its configured start actually changed, so editing
    /// an unrelated setting does not make the next `{n}` collide with an existing file.
    pub fn update(&self, policy: Policy, recording_policy: Policy, save: SaveConfig) {
        if save.counter_start != self.save.borrow().counter_start {
            self.counter.set(save.counter_start);
        }
        *self.policy.borrow_mut() = policy;
        *self.recording_policy.borrow_mut() = recording_policy;
        *self.save.borrow_mut() = save;
    }

    /// Resolves and runs the plan for one capture.
    ///
    /// Nothing here aborts the whole flow: a failed copy must not stop a save, and a
    /// failed save must not lose the capture -- which is still in the spool with its twin
    /// either way (`spec/10` §8). Failures are reported, not propagated.
    pub async fn handle(&self, capture: &CaptureResult) -> Outcome {
        // Before the plan, and instead of it. `spec/07` §2.1's text capture has its own
        // after-capture path -- sound, clipboard, notification -- and none of `ACT-01`'s
        // applies to it: a PNG of a paragraph is not what the user asked for, so saving
        // it into the screenshot folder or putting a card up for it would leave them
        // deleting a file after every quotation they copied.
        if capture.mode == octosnap_core::CaptureMode::Ocr {
            return self.recognise(capture).await;
        }

        let modifiers = decode_modifiers(capture.modifiers);
        // Cloned rather than held across the awaits below: a settings change landing
        // mid-capture must not panic on an outstanding borrow.
        let policy = if is_recording(capture) {
            self.recording_policy.borrow().clone()
        } else {
            self.policy.borrow().clone()
        };
        let plan = resolve(&policy, capture.requested_action, modifiers);
        info!(actions = ?plan.actions, "resolved after-capture plan");

        let mut outcome = Outcome { recording: is_recording(capture), ..Outcome::default() };

        // D113: a recording's GIF is written from its frames when something needs the
        // file, and a card is where the user says what that is -- Save, Copy, or Trim
        // first and then either. So a plan that puts up a card holds its own Copy and
        // Save back until the card goes without the user having done any of those
        // ([`Self::release`]), rather than writing a full-size GIF the user is about to
        // trim: at the screen's rate gifski takes a minute over ten seconds of it.
        let holds = is_recording(capture)
            && render::pending(&capture.path)
            && plan.has(AfterAction::ShowOverlay)
            && self.card_shower.borrow().is_some();
        let outputs = Held { copy: should_copy_now(&plan), save: plan.writes_to_export_location() };

        if outputs.copy && !holds {
            // Not held, so a GIF still in frames is written now, before the clipboard is
            // handed a file that is not there yet.
            match self.to_clipboard(capture).await {
                Ok(()) => {
                    outcome.copied = true;
                    info!(path = %capture.path.display(), "copied to the clipboard");
                }
                Err(e) => warn!("could not copy to the clipboard: {e}"),
            }
        }

        if outputs.save && !holds {
            match self.save(capture).await {
                Ok(path) => {
                    info!(path = %path.display(), "saved");
                    outcome.saved_to = Some(path);
                }
                Err(e) => warn!("could not save the capture: {e}"),
            }
        }

        // The card last, and only now: `spec/04` §1 shows Trash in place of Save "when
        // the after-capture action already saved the file", which is not known until the
        // save above has either happened or failed. Building the card first would give a
        // Save button to a capture that is already on disk.
        //
        // `pin` before the card, and instead of it: a pin *is* the capture's place on the
        // screen (D47), so a plan that asks for both gets the pin, and the card the pin
        // hands back when it closes. Two handles to one capture would be the thing D47
        // exists to prevent.
        if plan.has(AfterAction::Pin) {
            match self.pinner.borrow().as_ref() {
                Some(pin) => {
                    outcome.pinned = pin(capture, outcome.saved_to.as_deref());
                    if !outcome.pinned {
                        warn!("the capture could not be pinned; falling back to the card");
                    }
                }
                None => outcome.deferred.push(AfterAction::Pin),
            }
        }

        // `card_shown` is the result of trying, not of planning. D27.
        if plan.has(AfterAction::ShowOverlay) && !outcome.pinned {
            let shower = self.card_shower.borrow();
            match shower.as_ref() {
                Some(show) => {
                    outcome.card_shown = show(capture, outcome.saved_to.as_deref());
                    if !outcome.card_shown {
                        warn!("the overlay refused the capture; falling back to a notification");
                    }
                }
                None => outcome.deferred.push(AfterAction::ShowOverlay),
            }
        }

        if holds && outputs != Held::default() {
            if outcome.card_shown {
                info!(
                    path = %capture.path.display(),
                    ?outputs,
                    "the plan's outputs wait for the card"
                );
                self.held.borrow_mut().insert(capture.path.clone(), outputs);
            } else {
                // No card after all: nothing to wait for.
                let done = self.run_held(capture, outputs).await;
                outcome.copied = done.copied;
                outcome.saved_to = done.saved_to;
            }
        }

        // `annotate`, after the card for the same reason the card comes after the save:
        // the editor's bottom bar describes the capture as already carded or saved.
        if plan.has(AfterAction::Annotate) {
            match self.editor_opener.borrow().as_ref() {
                Some(open) => open(capture),
                None => outcome.deferred.push(AfterAction::Annotate),
            }
        }

        for action in &plan.actions {
            if matches!(action, AfterAction::Upload | AfterAction::AskName | AfterAction::OpenEditor) {
                outcome.deferred.push(*action);
            }
        }
        if !outcome.deferred.is_empty() {
            info!(deferred = ?outcome.deferred, "actions not implemented in this build");
        }

        // Recorded after the plan has run, so `save-last` on a capture that was already
        // saved writes a second file rather than reporting the first.
        *self.last.borrow_mut() = Some(capture.clone());
        *self.last_saved.borrow_mut() = outcome.saved_to.clone();

        outcome
    }

    /// Takes back what a recording's plan held for its card, if it held anything (D113).
    ///
    /// The card calls this when it closes. A card the user did nothing with -- closed,
    /// timed out, pushed off the stack -- runs what the plan held ([`Self::run_held`]),
    /// because the plan said so and nobody said otherwise. Anything the user did with it
    /// -- Save, Copy, Trim, a drag, Trash -- was the user saying otherwise, and what the
    /// plan held is dropped.
    pub fn release(&self, path: &Path) -> Option<Held> {
        self.held.borrow_mut().remove(path)
    }

    /// Runs the outputs a plan held for a card: the GIF written from its frames first, then
    /// copied and saved. Failures are logged and left out of the outcome, as the plan's own
    /// are.
    pub async fn run_held(&self, capture: &CaptureResult, held: Held) -> Outcome {
        let mut outcome = Outcome::default();
        if held.copy {
            match self.copy(capture).await {
                Ok(()) => outcome.copied = true,
                Err(e) => warn!("could not copy the recording: {e}"),
            }
        }
        if held.save {
            match self.save(capture).await {
                Ok(path) => {
                    info!(path = %path.display(), "saved what the plan held for the card");
                    *self.last_saved.borrow_mut() = Some(path.clone());
                    outcome.saved_to = Some(path);
                }
                Err(e) => warn!("could not save the recording: {e}"),
            }
        }
        outcome
    }

    /// `spec/07` §2.1: "Select an area -> sound -> recognized text is copied".
    ///
    /// In that order, and the order is the point. The sound is the first thing the user
    /// gets after a read that took the better part of a second, so it goes before the
    /// D-Bus round trip that copies rather than after it; the notification comes last,
    /// from the caller, because only it knows whether notifications are on.
    ///
    /// The capture's PNG stays in the spool and is not saved, carded or pinned. It is
    /// still on disk with its twin (`spec/10` §8), which is what makes a failed read
    /// recoverable -- the same file can be read again once a pack is installed.
    async fn recognise(&self, capture: &CaptureResult) -> Outcome {
        let outcome = self.read_text(&capture.path, Some(capture.rect), capture.linebreaks).await;
        *self.last.borrow_mut() = Some(capture.clone());
        outcome
    }

    /// Finishes the read that found no pack, now that one is installed (D171).
    ///
    /// `None` when there is nothing to finish: no read waited, it waited too long or its
    /// file is gone, or there is still no pack to read with -- an install that brought a
    /// script and not the detection model, which keeps the read for the next one. A read
    /// that happens is the read it would have been, with its own rect and line breaks, and
    /// a text capture's `copy-last` already points at it from the first attempt.
    pub async fn read_waiting(&self) -> Option<Outcome> {
        let waiting = self.waiting.borrow_mut().take()?;
        if !crate::ocr::ready() {
            info!(path = %waiting.path.display(), "a read still waits for a language pack");
            *self.waiting.borrow_mut() = Some(waiting);
            return None;
        }
        if let Err(why) = waiting.due(std::time::Instant::now(), waiting.path.exists()) {
            info!(path = %waiting.path.display(), why, "dropped the read that waited for a pack");
            return None;
        }
        info!(path = %waiting.path.display(), "reading what waited for a language pack");
        Some(self.read_text(&waiting.path, waiting.rect, waiting.linebreaks).await)
    }

    /// Whether a read is waiting for a pack and would still be read if one came now,
    /// which is what Settings says when it opens on the packs (D171).
    pub fn is_waiting(&self) -> bool {
        let now = std::time::Instant::now();
        self.waiting
            .borrow()
            .as_ref()
            .is_some_and(|waiting| waiting.due(now, waiting.path.exists()).is_ok())
    }

    /// The read waiting for a pack, for the tests.
    #[cfg(test)]
    fn waiting(&self) -> Option<Waiting> {
        self.waiting.borrow().clone()
    }

    /// The same read, on a file the user named rather than on a capture.
    ///
    /// `spec/07` §2.1's "also works on a file (`capture-text?filepath=`)". One
    /// implementation for both, because the only difference between them is where the
    /// PNG came from -- and `copy-last` must not start pointing at a file the user asked
    /// to *read*, which is why the capture path sets `last` and this one does not.
    ///
    /// `rect` is where on screen the text came from, for D96's indicator. It is the one
    /// thing the two callers do not share: a file has no place on screen to point at.
    pub async fn read_text(
        &self,
        path: &Path,
        rect: Option<Rect>,
        linebreaks: Option<bool>,
    ) -> Outcome {
        // `spec/08` §1's setting, overridden for this one read when the capture came
        // from `capture-text-single-line` or from `capture-text?linebreaks=`.
        let mut config = crate::settings::ocr_config();
        if let Some(keep) = linebreaks {
            config.breaks = crate::ocr::breaks(keep);
        }
        // Asked before the read rather than after it, because the engine's answer for a
        // machine with no models is an empty page -- which is indistinguishable from a
        // capture of a photograph, and tells the user to try a different area when what
        // they need is a 13 MB download.
        if !crate::ocr::ready() {
            warn!("a text capture arrived with no language pack installed");
            // Kept for the pack the user is about to be shown (D171). A later read that
            // also finds none takes its place: the newest is the one they are waiting on.
            *self.waiting.borrow_mut() = Some(Waiting {
                path: path.to_owned(),
                rect,
                linebreaks,
                asked: std::time::Instant::now(),
            });
            return Outcome {
                recognised: Some(Recognised::Failed(NO_PACK.to_owned())),
                ..Outcome::default()
            };
        }
        // Armed before the read and dropped the moment it lands. Most reads are over
        // before this has anything to do; the ones that are not are the reason it exists
        // (D96).
        let reading = self.reading_shower.borrow().as_ref().map(|shower| {
            let shower = Rc::clone(shower);
            crate::ocr::reading::Reading::start(move || shower(rect))
        });
        let read = crate::ocr::read(path, config.script).await;
        // The indicator comes down here rather than at the end of the function: the
        // sound, the clipboard and the notification are the read's *result*, and none of
        // them waits on the thing that said the result was coming.
        drop(reading);
        let read = match read {
            Ok(read) => read,
            Err(why) => {
                warn!(path = %path.display(), "could not read the capture: {why}");
                return Outcome { recognised: Some(Recognised::Failed(why)), ..Outcome::default() };
            }
        };
        if read.is_empty() {
            info!(path = %path.display(), "read a capture with no text in it");
            return Outcome { recognised: Some(Recognised::Nothing), ..Outcome::default() };
        }

        let text = read.text(config.breaks);
        let first = read.first_line().unwrap_or_default().to_owned();
        if config.sounds {
            // Fire-and-forget on the extension's side; a missing sound daemon must not
            // turn a successful read into a failure.
            if let Err(e) = self.bridge.play_sound(Cue::TextCopied).await {
                warn!("could not play the text-copied sound: {e}");
            }
        }

        let mut outcome = Outcome::default();
        crate::editor::tools::forget_copied_objects("the app copied text");
        match self.bridge.set_clipboard_text(&text).await {
            Ok(()) => {
                outcome.copied = true;
                info!(characters = text.len(), "copied recognised text");
            }
            // Still worth showing: the window has a Copy button, and losing the text
            // because the clipboard would not take it would be the worse answer.
            Err(e) => warn!("could not copy the recognised text: {e}"),
        }
        crate::ocr::remember(read, config);
        outcome.recognised = Some(Recognised::Copied(first));
        outcome
    }

    /// `ACT-06`: renders the name, resolves the folder, picks a non-colliding path, copies.
    /// A recording whose GIF is still frames is written first (D113), before the name
    /// takes a `{n}`: a render that fails must not use one up.
    /// Every copy's way to the clipboard: the card's, the editors', a pin's, the history's,
    /// `copy-last` and the after-capture plan's.
    ///
    /// A recording whose GIF is still frames is written first (D113). Then the shell is
    /// handed a file, and what it does with it depends on what it is (D165): a PNG's pixels
    /// are read at once and the file can go, but a GIF goes on the clipboard as its file,
    /// which is read when it is pasted -- after the card that held it has closed and its
    /// spool copy has been filed away. So a GIF is copied first to a place of the
    /// clipboard's own ([`Self::clipboard_copy`]).
    async fn to_clipboard(&self, capture: &CaptureResult) -> Result<(), SaveError> {
        render::ensure(&capture.path).await.map_err(SaveError::Render)?;
        let path = if octosnap_core::history::is_gif(&capture.path) {
            self.clipboard_copy(capture).await?
        } else {
            capture.path.clone()
        };
        crate::editor::tools::forget_copied_objects("the app copied an image");
        self.bridge.set_clipboard_image(&path).await?;
        Ok(())
    }

    /// The copy of a GIF that the clipboard names (D165): `clipboard/<id>/<name>.gif` in
    /// the app's cache, under the name Save would give it, since that is the name a paste
    /// into Files or a chat shows.
    ///
    /// It lasts until the next GIF is copied, which takes the folder's earlier copies away:
    /// a page that took the file from a paste may read it later than the paste, so it is
    /// not removed the moment the clipboard changes, and the cache is where a file that can
    /// be made again belongs. Its own directory per capture, so two GIFs of one name never
    /// meet. A copy rather than a link: the file the clipboard names must not change under
    /// it if the spool's is ever rewritten in place.
    async fn clipboard_copy(&self, capture: &CaptureResult) -> Result<PathBuf, SaveError> {
        let root = self
            .save
            .borrow()
            .clipboard_folder
            .clone()
            .unwrap_or_else(|| glib::user_cache_dir().join("octosnap").join("clipboard"));
        let id = capture
            .path
            .file_stem()
            .map_or_else(octosnap_core::capture::fresh_id, |s| s.to_string_lossy().into_owned());
        let dir = root.join(&id);
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy() != id
                    && let Err(e) = std::fs::remove_dir_all(entry.path())
                {
                    warn!(path = %entry.path().display(), "could not remove an earlier clipboard copy: {e}");
                }
            }
        }
        if let Err(e) = gio::File::for_path(&dir).make_directory_with_parents(gio::Cancellable::NONE)
            && !e.matches(gio::IOErrorEnum::Exists)
        {
            return Err(SaveError::Directory(dir, e));
        }
        let destination = dir.join(self.suggested_name(capture));
        gio::File::for_path(&capture.path)
            .copy_future(
                &gio::File::for_path(&destination),
                gio::FileCopyFlags::OVERWRITE,
                glib::Priority::DEFAULT,
            )
            .0
            .await
            .map_err(|e| SaveError::Copy(destination.clone(), e))?;
        Ok(destination)
    }

    async fn save(&self, capture: &CaptureResult) -> Result<PathBuf, SaveError> {
        render::ensure(&capture.path).await.map_err(SaveError::Render)?;
        let config = self.save.borrow().clone();
        let (physical_width, physical_height) = capture.rect.to_physical(capture.scale);
        let recording = is_recording(capture);

        let context = filename::Context {
            timestamp: chrono_from_micros(capture.timestamp),
            use_utc: config.utc,
            counter: self.counter.get(),
            counter_width: config.counter_width,
            app: capture.source_window.app_name.clone(),
            window: capture.source_window.title.clone(),
            capture_type: capture_type(recording),
            width: physical_width,
            height: physical_height,
            // `CAP-16`: only when the capture really is denser than 1x, so a 100 %
            // monitor does not produce files claiming to be @2x. A GIF is never @2x: it
            // is scaled to `gif-max-width`, not to the monitor's density.
            retina_suffix: !recording && config.retina_suffix && capture.scale > 1.0,
        };
        let stem = filename::render(&config.template, &context);
        self.counter.set(self.counter.get().saturating_add(1));

        // A recording saves into the recording folder, keeping the extension the spool
        // file already has (`gif` in M5); a screenshot into the screenshot folder as its
        // configured image format (D164).
        let (dir, extension, format): (PathBuf, String, Option<ImageFormat>) = if recording {
            (
                recording_dir(config.recording_folder.as_deref(), &videos_dir()),
                recording_extension(capture),
                None,
            )
        } else {
            let format = still_format(capture, config.format);
            (
                screenshot_dir(config.folder.as_deref(), &pictures_dir()),
                format.extension().to_owned(),
                Some(format),
            )
        };
        let destination = unique_path(&dir, &stem, &extension, |p| p.exists());

        // Creating the directory is one syscall chain and has no async variant, so it
        // stays sync. The write below is the part that matters: spec/10 §6 caps UI-thread
        // work at 4 ms and a 4K PNG is megabytes, so that one is a future -- and an encode
        // is a worker's.
        if let Err(e) = gio::File::for_path(&dir).make_directory_with_parents(gio::Cancellable::NONE)
            && !e.matches(gio::IOErrorEnum::Exists)
        {
            return Err(SaveError::Directory(dir, e));
        }

        match format {
            Some(format) => {
                write_still(&capture.path, &destination, format, config.jpeg_quality, false).await?;
            }
            None => copy_file(&capture.path, &destination, false).await?,
        }
        Ok(destination)
    }
}

/// The format a screenshot is saved in: `configured`, or PNG when that cannot hold the
/// picture -- a scrolling capture past WebP's 16384 rows (D164). The size is the file's
/// own, from its header, because the capture's recorded size can round differently.
fn still_format(capture: &CaptureResult, configured: ImageFormat) -> ImageFormat {
    match encode::peek(&capture.path).1 {
        Some((width, height)) => configured.for_size(width, height),
        None => configured,
    }
}

/// Writes the screenshot at `source` to `destination` as `format` (D164).
///
/// A copy when the file is that format already, which is every PNG save as it always
/// was. An encode otherwise, on a worker: a 5K JPEG is a tenth of a second of CPU, and
/// `spec/10` §7 gives the main loop a frame.
async fn write_still(
    source: &Path,
    destination: &Path,
    format: ImageFormat,
    quality: u8,
    replace: bool,
) -> Result<(), SaveError> {
    if encode::peek(source).0 == Some(format) {
        return copy_file(source, destination, replace).await;
    }
    let started = std::time::Instant::now();
    let (from, to) = (source.to_path_buf(), destination.to_path_buf());
    gio::spawn_blocking(move || encode::transcode(&from, &to, format, quality, replace))
        .await
        .unwrap_or(Err(EncodeError::Panicked))
        .map_err(|e| SaveError::Encode(destination.to_path_buf(), e))?;
    info!(
        path = %destination.display(),
        format = format.as_wire(),
        ms = started.elapsed().as_millis(),
        "encoded a screenshot"
    );
    Ok(())
}

/// `source` to `destination` as it is. `replace` writes over a file already there, which
/// only a Save As may.
async fn copy_file(source: &Path, destination: &Path, replace: bool) -> Result<(), SaveError> {
    let flags = if replace { gio::FileCopyFlags::OVERWRITE } else { gio::FileCopyFlags::NONE };
    gio::File::for_path(source)
        .copy_future(&gio::File::for_path(destination), flags, glib::Priority::DEFAULT)
        .0
        .await
        .map_err(|e| SaveError::Copy(destination.to_path_buf(), e))
}

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("nothing has been captured yet in this session")]
    NothingCaptured,
    #[error("the GIF could not be written: {0}")]
    Render(String),
    #[error("could not create {0}: {1}")]
    Directory(PathBuf, glib::Error),
    #[error("could not write {0}: {1}")]
    Copy(PathBuf, glib::Error),
    #[error("could not write {0}: {1}")]
    Encode(PathBuf, EncodeError),
    #[error("the shell bridge failed: {0}")]
    Bridge(#[from] BridgeError),
}

/// [`CaptureFlow::cue`] for a caller that is neither async nor holding the flow: a window
/// of the app's own, like the text window's Copy.
pub fn play_cue(cue: Cue) {
    if let Some(flow) = crate::capture_flow() {
        gtk::glib::spawn_future_local(async move { flow.cue(cue).await });
    }
}

/// `CAP-15`: the modifier mask the extension captured at confirm.
///
/// The values are `Clutter.ModifierType`: Ctrl is bit 2 and Shift bit 0.
fn decode_modifiers(mask: u32) -> ConfirmModifiers {
    const SHIFT_MASK: u32 = 1 << 0;
    const CONTROL_MASK: u32 = 1 << 2;
    ConfirmModifiers {
        ctrl: mask & CONTROL_MASK != 0,
        shift: mask & SHIFT_MASK != 0,
    }
}

/// True when the image should go on the clipboard *now*.
///
/// `ACT-01`'s Copy+Upload rule means "copy the link when the upload finishes" must not
/// also put the image there first.
fn should_copy_now(plan: &Plan) -> bool {
    matches!(
        plan.clipboard,
        Some(ClipboardPlan::Image | ClipboardPlan::ImageThenLink)
    )
}

fn pictures_dir() -> PathBuf {
    glib::user_special_dir(glib::UserDirectory::Pictures)
        .unwrap_or_else(|| glib::home_dir().join("Pictures"))
}

fn videos_dir() -> PathBuf {
    glib::user_special_dir(glib::UserDirectory::Videos)
        .unwrap_or_else(|| glib::home_dir().join("Videos"))
}

/// Whether a capture files as a recording (`spec/07` §4.1's GIF or video kind), which is
/// what sends it to the recording folder rather than the screenshot one. A GIF opened from
/// disk (`import`, `external`) files as a `File` in the history but saves as the GIF it
/// is: the recording folder, the recording template, `.gif` -- never a `.png` name on GIF
/// bytes.
fn is_recording(capture: &CaptureResult) -> bool {
    matches!(Kind::of(capture), Kind::Gif | Kind::Video) || octosnap_core::history::is_gif(&capture.path)
}

fn capture_type(recording: bool) -> filename::CaptureType {
    if recording {
        filename::CaptureType::Recording
    } else {
        filename::CaptureType::Screenshot
    }
}

/// The extension the spool file already carries, lower-cased; `gif` if it somehow has
/// none. Keeps a saved recording the same format the encoder wrote.
fn recording_extension(capture: &CaptureResult) -> String {
    capture
        .path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "gif".to_owned())
}

fn chrono_from_micros(micros: u64) -> chrono::DateTime<chrono::Utc> {
    let secs = (micros / 1_000_000) as i64;
    let nanos = ((micros % 1_000_000) * 1_000) as u32;
    chrono::DateTime::from_timestamp(secs, nanos).unwrap_or_else(chrono::Utc::now)
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use std::path::Path;
    use std::rc::Rc;

    use chrono::TimeZone;
    use octosnap_core::actions::{AfterAction, CopyUploadBehavior};
    use octosnap_core::{CaptureMode, Rect, SourceWindow};
    use octosnap_shell::NullBridge;

    use super::*;

    /// `spec/10` §11: "`NullBridge` drives the app through capture -> card -> actions
    /// without a shell." No session bus, no compositor, no extension.
    ///
    /// A **fresh** context per test, not `MainContext::default()`: the default one is a
    /// process-wide singleton that only one thread may own at a time, and cargo runs
    /// these in parallel, so sharing it fails with "already acquired by another thread".
    fn run<F: std::future::Future>(future: F) -> F::Output {
        glib::MainContext::new().block_on(future)
    }

    fn capture_in(dir: &Path, modifiers: u32) -> CaptureResult {
        let png = dir.join("01SOURCE.png");
        // Not a real PNG; nothing in a PNG save decodes it, it only gets copied. A JPEG or
        // a WebP save does, and [`still_in`] is for those.
        std::fs::write(&png, b"\x89PNG\r\n\x1a\n fake").expect("write source");
        CaptureResult {
            path: png.clone(),
            meta_path: png.with_extension("json"),
            mode: CaptureMode::Area,
            rect: Rect::new(0, 0, 512, 384),
            scale: 1.25,
            display: "eDP-1".to_owned(),
            cursor_rect: None,
            source_window: SourceWindow::default(),
            window_alpha: false,
            // Derived rather than a magic number: the literal 1_757_251_200_000_000 that
            // was here reads like 2026-09-07 and is actually 2025-09-07, which a test
            // asserting on a rendered date duly caught.
            timestamp: chrono::Utc
                .with_ymd_and_hms(2026, 9, 7, 13, 4, 5)
                .single()
                .expect("a valid instant")
                .timestamp_micros() as u64,
            // No confirmation stamp: nothing in these tests is about the 300 ms budget,
            // and `None` is what a twin written before that field existed carries.
            confirmed_at: None,
            animation_ms: 0,
            requested_action: None,
            modifiers,
            external: false,
            linebreaks: None,
            project: None,
            duration_ms: None,
        }
    }

    /// [`capture_in`] with a real `width` x `height` PNG behind it, see-through down its
    /// left edge when `alpha`, as a window capture's shadow is.
    fn still_in(dir: &Path, width: u32, height: u32, alpha: bool) -> CaptureResult {
        let capture = capture_in(dir, 0);
        encode::tests::write_png(&capture.path, width, height, alpha);
        capture
    }

    /// Both folders in `dir`: a test that saves a recording must never reach the real
    /// XDG Videos folder, which is where `recording_folder: None` goes.
    fn save_config_in(dir: &Path) -> SaveConfig {
        SaveConfig {
            folder: Some(dir.to_path_buf()),
            recording_folder: Some(dir.join("recordings")),
            template: "Shot {yyyy}-{MM}-{dd} {n}".to_owned(),
            utc: true,
            clipboard_folder: Some(dir.join("clipboard")),
            ..SaveConfig::default()
        }
    }

    /// A finished GIF recording sitting in the spool: `Record` mode, a `.gif` file, a
    /// length for the badge.
    fn recording_capture_in(dir: &Path) -> CaptureResult {
        let gif = dir.join("01REC.gif");
        std::fs::write(&gif, b"GIF89a fake").expect("write source");
        CaptureResult {
            meta_path: gif.with_extension("json"),
            path: gif,
            mode: CaptureMode::Record,
            duration_ms: Some(3_200),
            ..capture_in(dir, 0)
        }
    }

    /// A recording that has just stopped (D113): its twin says `01REC.gif`, and the GIF is
    /// still the frames in `01REC.reel` beside it.
    fn stopped_recording_in(dir: &Path) -> CaptureResult {
        let capture = recording_capture_in(dir);
        std::fs::remove_file(&capture.path).expect("no GIF yet");
        let frames = octosnap_media::reel::beside(&capture.path);
        crate::recording::render::tests::write_reel(&frames, 3);
        capture
    }

    /// D113: a plan that puts a card up for a recording still in frames holds its Copy and
    /// Save for that card, which takes them back when it closes -- rather than writing a
    /// full-size GIF the user may be about to trim.
    #[test]
    fn a_recordings_card_holds_the_plans_copy_and_save_until_it_closes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bridge = NullBridge::default();
        let flow = CaptureFlow::new(bridge.clone(), Policy::default(), save_config_in(dir.path()));
        flow.set_recording_policy(Policy {
            configured: vec![AfterAction::ShowOverlay, AfterAction::Copy, AfterAction::Save],
            ..Policy::default()
        });
        flow.set_card_shower(Box::new(|_, _| true));
        let capture = stopped_recording_in(dir.path());

        let outcome = run(flow.handle(&capture));

        assert!(outcome.card_shown);
        assert!(!outcome.copied && outcome.saved_to.is_none(), "{outcome:?}");
        assert!(bridge.clipboard_calls().is_empty());
        assert!(!capture.path.exists(), "no GIF was written for a card to trim");
        assert_eq!(flow.release(&capture.path), Some(Held { copy: true, save: true }));
        assert_eq!(flow.release(&capture.path), None, "released once");
    }

    /// With no card to wait for, the plan runs at once: the GIF is written from its frames
    /// before the clipboard or the recording folder is handed it.
    #[test]
    fn a_recording_with_no_card_is_written_then_copied_and_saved() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bridge = NullBridge::default();
        let flow = CaptureFlow::new(bridge.clone(), Policy::default(), save_config_in(dir.path()));
        flow.set_recording_policy(Policy {
            configured: vec![AfterAction::Copy, AfterAction::Save],
            ..Policy::default()
        });
        let capture = stopped_recording_in(dir.path());

        let outcome = run(flow.handle(&capture));

        assert!(capture.path.is_file(), "the GIF was written from its frames");
        assert!(outcome.copied);
        // The clipboard's own copy of it (D165), under the name the save below gets too.
        let copied = dir.path().join("clipboard/01REC/Shot 2026-09-07 1.gif");
        assert_eq!(bridge.clipboard_calls(), vec![copied.clone()]);
        assert_eq!(std::fs::read(&copied).expect("copied"), std::fs::read(&capture.path).expect("written"));
        let saved = outcome.saved_to.expect("saved");
        assert_eq!(saved.file_name(), copied.file_name());
        assert!(saved.is_file());
        assert_eq!(saved.extension().and_then(|e| e.to_str()), Some("gif"));
        assert!(flow.release(&capture.path).is_none(), "nothing was held");
    }

    /// D165: a GIF goes on the clipboard as its file, and that file is one of the
    /// clipboard's own, under the name Save gives it. It is read when it is pasted, which is
    /// after a Copy has closed the card and the card's closing has taken the spool copy.
    #[test]
    fn a_gif_is_copied_as_a_file_that_outlives_the_spool_copy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bridge = NullBridge::default();
        let flow = CaptureFlow::new(bridge.clone(), Policy::default(), save_config_in(dir.path()));
        let capture = recording_capture_in(dir.path());

        run(flow.copy(&capture)).expect("copied");

        let calls = bridge.clipboard_calls();
        assert_eq!(calls, vec![dir.path().join("clipboard/01REC/Shot 2026-09-07 1.gif")]);
        std::fs::remove_file(&capture.path).expect("the card closed and filed the spool copy away");
        assert_eq!(std::fs::read(&calls[0]).expect("still there to paste"), b"GIF89a fake");
    }

    /// D165: the clipboard's folder keeps the GIF it names and no older one, and a PNG,
    /// whose pixels the shell reads at once, is handed over as it is.
    #[test]
    fn the_next_gif_takes_the_last_ones_copy_and_a_png_needs_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bridge = NullBridge::default();
        let flow = CaptureFlow::new(bridge.clone(), Policy::default(), save_config_in(dir.path()));
        let first = recording_capture_in(dir.path());
        let later = dir.path().join("01LATER.gif");
        std::fs::write(&later, b"GIF89a later").expect("write source");
        let second = CaptureResult { path: later.clone(), meta_path: later.with_extension("json"), ..first.clone() };
        let shot = capture_in(dir.path(), 0);

        run(flow.copy(&first)).expect("first GIF");
        run(flow.copy(&shot)).expect("a screenshot");
        let kept = bridge.clipboard_calls()[0].clone();
        assert!(kept.is_file(), "a screenshot's copy leaves the GIF's alone");
        run(flow.copy(&second)).expect("second GIF");

        let calls = bridge.clipboard_calls();
        assert_eq!(calls[1], shot.path);
        assert!(!kept.exists(), "the first GIF's copy went when the second was copied");
        assert_eq!(std::fs::read(&calls[2]).expect("the second's copy"), b"GIF89a later");
        let left: Vec<_> = std::fs::read_dir(dir.path().join("clipboard"))
            .expect("the folder")
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, vec![std::ffi::OsString::from("01LATER")]);
    }

    /// `spec/08` §5: a recording saves into the recording folder, not the screenshot one,
    /// and keeps the extension the encoder wrote rather than the screenshot format.
    #[test]
    fn a_recording_saves_into_the_recording_folder_as_a_gif() {
        let dir = tempfile::tempdir().expect("tempdir");
        let shots = dir.path().join("shots");
        let recordings = dir.path().join("recordings");
        let save = SaveConfig {
            folder: Some(shots.clone()),
            recording_folder: Some(recordings.clone()),
            template: "Take {n}".to_owned(),
            ..SaveConfig::default()
        };
        let flow = CaptureFlow::new(NullBridge::default(), Policy::default(), save);

        let capture = recording_capture_in(dir.path());
        let saved = run(flow.save_capture(&capture)).expect("saved");

        assert!(saved.starts_with(&recordings), "a GIF belongs under the recording folder, got {saved:?}");
        assert!(!saved.starts_with(&shots), "and not under the screenshot folder");
        assert_eq!(saved.extension().and_then(|e| e.to_str()), Some("gif"));
    }

    /// The chooser's pre-filled name for a GIF is a `.gif`, never the screenshot format,
    /// and never `@2x` -- a GIF is scaled to a width, not to a monitor's density.
    #[test]
    fn a_recordings_suggested_name_is_a_gif() {
        let dir = tempfile::tempdir().expect("tempdir");
        let save = SaveConfig { template: "Take".to_owned(), ..save_config_in(dir.path()) };
        let flow = CaptureFlow::new(NullBridge::default(), Policy::default(), save);
        let capture = recording_capture_in(dir.path());
        assert_eq!(flow.suggested_name(&capture), "Take.gif");
    }

    /// A GIF opened from disk (`import`, `external`) files as a `File`, but it saves as the
    /// GIF it is: the recording folder, the recording template, `.gif` -- never `.png`
    /// bytes-be-damned, which is what `Kind::External` alone would have given it.
    #[test]
    fn an_opened_gif_saves_as_a_gif_into_the_recording_folder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let recordings = dir.path().join("recordings");
        let save = SaveConfig {
            recording_folder: Some(recordings.clone()),
            template: "Take".to_owned(),
            ..save_config_in(dir.path())
        };
        let flow = CaptureFlow::new(NullBridge::default(), Policy::default(), save);
        let capture = CaptureResult { external: true, ..recording_capture_in(dir.path()) };
        assert_eq!(flow.suggested_name(&capture), "Take.gif");
        let saved = run(flow.save_capture(&capture)).expect("saved");
        assert!(saved.starts_with(&recordings), "got {saved:?}");
        assert_eq!(saved.extension().and_then(|e| e.to_str()), Some("gif"));
    }

    #[test]
    fn the_default_policy_copies_and_does_not_write_a_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bridge = NullBridge::default();
        let flow = CaptureFlow::new(bridge.clone(), Policy::default(), save_config_in(dir.path()));
        let capture = capture_in(dir.path(), 0);

        let outcome = run(flow.handle(&capture));

        assert!(outcome.copied);
        assert_eq!(bridge.clipboard_calls(), vec![capture.path.clone()]);
        assert_eq!(outcome.saved_to, None);
        // No card shower is installed here, so the plan's card is deferred rather than
        // silently claimed -- which is what lets `notify` speak up instead. D27.
        assert!(!outcome.card_shown);
        assert!(outcome.deferred.contains(&AfterAction::ShowOverlay));
    }

    #[test]
    fn a_save_policy_writes_the_templated_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = Policy {
            configured: vec![AfterAction::Save],
            ..Policy::default()
        };
        let flow = CaptureFlow::new(NullBridge::default(), policy, save_config_in(dir.path()));
        let capture = capture_in(dir.path(), 0);

        let outcome = run(flow.handle(&capture));

        let saved = outcome.saved_to.expect("saved");
        assert_eq!(
            saved.file_name().expect("name").to_string_lossy(),
            // scale is 1.25, so CAP-16's @2x suffix applies.
            "Shot 2026-09-07 1@2x.png"
        );
        assert!(saved.exists());
        assert!(!outcome.copied);
    }

    /// `CAP-15`: Ctrl held at confirm copies whatever the settings say. The mask value is
    /// `Clutter.ModifierType.CONTROL_MASK`.
    #[test]
    fn ctrl_at_confirm_copies_even_under_a_save_only_policy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bridge = NullBridge::default();
        let policy = Policy {
            configured: vec![AfterAction::Save],
            ..Policy::default()
        };
        let flow = CaptureFlow::new(bridge.clone(), policy, save_config_in(dir.path()));

        let outcome = run(flow.handle(&capture_in(dir.path(), 1 << 2)));

        assert!(outcome.copied);
        assert_eq!(bridge.clipboard_calls().len(), 1);
        assert!(outcome.saved_to.is_some());
    }

    /// `ACT-01`'s Copy+Upload rule: with `LinkOnly` the image must not be put on the
    /// clipboard first, or the user pastes a picture when they asked for a URL.
    #[test]
    fn link_only_does_not_copy_the_image() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bridge = NullBridge::default();
        let policy = Policy {
            configured: vec![AfterAction::Copy, AfterAction::Upload],
            copy_upload: CopyUploadBehavior::LinkOnly,
            ..Policy::default()
        };
        let flow = CaptureFlow::new(bridge.clone(), policy, save_config_in(dir.path()));

        let outcome = run(flow.handle(&capture_in(dir.path(), 0)));

        assert!(!outcome.copied);
        assert!(bridge.clipboard_calls().is_empty());
        assert!(outcome.deferred.contains(&AfterAction::Upload));
    }

    /// A second capture must not overwrite the first when the template has no seconds.
    #[test]
    fn a_second_capture_with_the_same_name_does_not_overwrite() {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = Policy {
            configured: vec![AfterAction::Save],
            ..Policy::default()
        };
        // Counter width 1 and a fixed template, so both renders collide on the name.
        let save = SaveConfig {
            template: "Shot".to_owned(),
            retina_suffix: false,
            ..save_config_in(dir.path())
        };
        let flow = CaptureFlow::new(NullBridge::default(), policy, save);
        let capture = capture_in(dir.path(), 0);

        let first = run(flow.handle(&capture)).saved_to.expect("first");
        let second = run(flow.handle(&capture)).saved_to.expect("second");

        assert_ne!(first, second);
        assert!(first.exists() && second.exists());
        assert_eq!(second.file_name().expect("n").to_string_lossy(), "Shot (2).png");
    }

    /// A missing extension must not lose the capture: it is in the spool with its twin,
    /// and the save half still has to happen.
    /// D27's contract, now that there is something to show: `card_shown` is the answer the
    /// overlay gave, not the plan's intention.
    #[test]
    fn card_shown_is_what_the_overlay_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let flow = CaptureFlow::new(NullBridge::default(), Policy::default(), save_config_in(dir.path()));
        let shown = Rc::new(Cell::new(false));
        {
            let shown = Rc::clone(&shown);
            flow.set_card_shower(Box::new(move |_, _| {
                shown.set(true);
                true
            }));
        }

        let outcome = run(flow.handle(&capture_in(dir.path(), 0)));

        assert!(shown.get(), "the plan asked for a card, so the shower must have been called");
        assert!(outcome.card_shown);
        assert!(
            !outcome.deferred.contains(&AfterAction::ShowOverlay),
            "a card that appeared is not a deferred action"
        );
    }

    /// The failure that D27 was written for: an overlay that cannot put a card up must
    /// leave the capture visible some other way, not claim it succeeded.
    #[test]
    fn an_overlay_that_refuses_falls_back_to_a_notification() {
        let dir = tempfile::tempdir().expect("tempdir");
        let flow = CaptureFlow::new(NullBridge::default(), Policy::default(), save_config_in(dir.path()));
        flow.set_card_shower(Box::new(|_, _| false));

        let outcome = run(flow.handle(&capture_in(dir.path(), 0)));

        assert!(!outcome.card_shown);
        assert!(
            crate::notify::describe(&outcome).is_some(),
            "with no card on screen the user has to be told something"
        );
    }

    /// `spec/04` §1: Trash replaces Save "when the after-capture action already saved the
    /// file", so the card has to be built after the save has actually happened.
    #[test]
    fn the_card_is_told_which_file_was_already_saved() {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = Policy {
            configured: vec![AfterAction::ShowOverlay, AfterAction::Save],
            ..Policy::default()
        };
        let flow = CaptureFlow::new(NullBridge::default(), policy, save_config_in(dir.path()));
        let saved = Rc::new(Cell::new(false));
        {
            let saved = Rc::clone(&saved);
            flow.set_card_shower(Box::new(move |_, saved_to| {
                saved.set(saved_to.is_some());
                true
            }));
        }

        let outcome = run(flow.handle(&capture_in(dir.path(), 0)));

        assert!(outcome.saved_to.is_some());
        assert!(saved.get(), "the card must be given the path of the file that exists");
    }

    /// `spec/04` §3's chooser opens on the templated name, and asking for it must not
    /// consume a `{n}`: a cancelled chooser that had advanced the counter would leave a
    /// gap in the numbering with no file to account for it.
    #[test]
    fn the_suggested_name_does_not_advance_the_counter() {
        let dir = tempfile::tempdir().expect("tempdir");
        let save = SaveConfig {
            template: "Shot {n}".to_owned(),
            ..save_config_in(dir.path())
        };
        let flow = CaptureFlow::new(NullBridge::default(), Policy::default(), save);
        let capture = capture_in(dir.path(), 0);

        // `@2x` because the sample capture is at 1.25 (`CAP-16`). The chooser has to
        // offer the same name the automatic save would write, suffix included, or the
        // two paths produce differently-named files from one setting.
        assert_eq!(flow.suggested_name(&capture), "Shot 1@2x.png");
        assert_eq!(flow.suggested_name(&capture), "Shot 1@2x.png", "asking twice is not saving");

        // A real save does advance it, so the next suggestion moves on.
        run(flow.save_capture(&capture)).expect("save");
        assert_eq!(flow.suggested_name(&capture), "Shot 2@2x.png");
    }

    /// Save-as writes exactly where it was told. `unique_path` is deliberately not applied:
    /// the user picked that name in a chooser that already asked about overwriting, and
    /// quietly writing `name-1.png` instead is the one thing a chooser should make
    /// impossible.
    #[test]
    fn save_as_honours_the_chosen_path_exactly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let flow = CaptureFlow::new(NullBridge::default(), Policy::default(), save_config_in(dir.path()));
        let capture = capture_in(dir.path(), 0);
        let chosen = dir.path().join("somewhere else.png");

        let written = run(flow.save_capture_as(&capture, &chosen)).expect("save as");
        assert_eq!(written, chosen);
        assert!(chosen.exists());
        assert_eq!(flow.last_saved(), Some(chosen.clone()));

        // And again, over the top, because the chooser has already asked.
        let again = run(flow.save_capture_as(&capture, &chosen)).expect("save as again");
        assert_eq!(again, chosen);
        assert_eq!(std::fs::read_dir(dir.path()).expect("read dir").count(), 2);
    }

    /// D164: `shot-format` is what a Save writes -- the bytes, not only the name. After a
    /// capture by the plan, and from a card, a pin or the editor, which all save through
    /// [`CaptureFlow::save_capture`].
    #[test]
    fn every_save_writes_the_configured_format() {
        for format in [ImageFormat::Png, ImageFormat::Jpg, ImageFormat::Webp] {
            let dir = tempfile::tempdir().expect("tempdir");
            let policy = Policy {
                configured: vec![AfterAction::Save],
                ..Policy::default()
            };
            let save = SaveConfig { format, jpeg_quality: 80, ..save_config_in(dir.path()) };
            let flow = CaptureFlow::new(NullBridge::default(), policy, save);
            let capture = still_in(dir.path(), 64, 48, true);

            let after_capture = run(flow.handle(&capture)).saved_to.expect("saved by the plan");
            let from_a_card = run(flow.save_capture(&capture)).expect("saved from a card");

            for saved in [after_capture, from_a_card] {
                encode::tests::assert_is(&saved, format);
            }
            let suggested = flow.suggested_name(&capture);
            assert!(suggested.ends_with(&format!(".{}", format.extension())), "{suggested}");
        }
    }

    /// D164: Save As writes what the chosen name asks for, whatever the setting says, and
    /// a name that asks for nothing gains the setting's extension rather than holding
    /// bytes it does not describe. A card's, the history's and the editor's Save As all
    /// come here.
    #[test]
    fn save_as_writes_the_format_its_name_asks_for() {
        let dir = tempfile::tempdir().expect("tempdir");
        let save = SaveConfig { format: ImageFormat::Jpg, ..save_config_in(dir.path()) };
        let flow = CaptureFlow::new(NullBridge::default(), Policy::default(), save);
        let capture = still_in(dir.path(), 64, 48, true);

        for (name, format) in [
            ("a.png", ImageFormat::Png),
            ("b.webp", ImageFormat::Webp),
            ("c.JPEG", ImageFormat::Jpg),
            ("d.jpg", ImageFormat::Jpg),
        ] {
            let chosen = dir.path().join(name);
            let written = run(flow.save_capture_as(&capture, &chosen)).expect("save as");
            assert_eq!(written, chosen);
            encode::tests::assert_is(&written, format);
        }

        let written = run(flow.save_capture_as(&capture, &dir.path().join("notes v1.2")))
            .expect("save as with no format in the name");
        assert_eq!(written, dir.path().join("notes v1.2.jpg"));
        encode::tests::assert_is(&written, ImageFormat::Jpg);
        assert_eq!(flow.last_saved(), Some(written));
    }

    /// D164: a WebP holds 16384 rows and a scrolling capture can have more. It is saved as
    /// a PNG under a PNG's name rather than failed, or written as a PNG called `.webp`.
    #[test]
    fn a_capture_too_long_for_its_format_saves_as_png() {
        let dir = tempfile::tempdir().expect("tempdir");
        let save = SaveConfig { format: ImageFormat::Webp, ..save_config_in(dir.path()) };
        let flow = CaptureFlow::new(NullBridge::default(), Policy::default(), save);
        let capture = still_in(dir.path(), 2, 16_385, false);

        assert!(flow.suggested_name(&capture).ends_with(".png"));
        let saved = run(flow.save_capture(&capture)).expect("save");
        encode::tests::assert_is(&saved, ImageFormat::Png);

        let chosen = dir.path().join("long.webp");
        let written = run(flow.save_capture_as(&capture, &chosen)).expect("save as");
        assert_eq!(written, dir.path().join("long.png"));
        encode::tests::assert_is(&written, ImageFormat::Png);
        assert!(!chosen.exists());
    }

    #[test]
    fn a_failing_bridge_does_not_stop_the_save() {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = Policy {
            configured: vec![AfterAction::Copy, AfterAction::Save],
            ..Policy::default()
        };
        let bridge = NullBridge::default().unavailable("extension not enabled");
        let flow = CaptureFlow::new(bridge, policy, save_config_in(dir.path()));

        let outcome = run(flow.handle(&capture_in(dir.path(), 0)));

        assert!(!outcome.copied);
        assert!(outcome.saved_to.is_some(), "the save must still happen");
    }

    /// D96's threshold, from the flow's side. The read here is over in microseconds --
    /// there is no such file, and on a machine with no language pack it does not even get
    /// that far -- so the indicator must never appear, whichever of the two happened.
    #[test]
    fn a_read_that_fails_at_once_never_says_it_is_reading() {
        let dir = tempfile::tempdir().expect("tempdir");
        let flow = CaptureFlow::new(NullBridge::default(), Policy::default(), save_config_in(dir.path()));
        let asked = Rc::new(Cell::new(0u32));
        {
            let asked = Rc::clone(&asked);
            flow.set_reading_shower(Rc::new(move |_| {
                asked.set(asked.get() + 1);
                None
            }));
        }

        let missing = dir.path().join("no-such-capture.png");
        let outcome = run(async {
            let outcome = flow.read_text(&missing, None, None).await;
            // Well past the threshold, so a timer that was still armed has fired by now.
            glib::timeout_future(std::time::Duration::from_millis(900)).await;
            outcome
        });

        assert!(matches!(outcome.recognised, Some(Recognised::Failed(_))), "{outcome:?}");
        assert_eq!(asked.get(), 0, "a read that failed at once still put something on screen");
    }

    fn waiting_since(asked: std::time::Instant) -> Waiting {
        Waiting { path: PathBuf::from("/spool/a.png"), rect: None, linebreaks: None, asked }
    }

    /// D171: a pack installed while the read waits finishes it.
    #[test]
    fn a_read_that_waited_for_a_pack_is_due_while_its_file_is_there() {
        let asked = std::time::Instant::now();
        let waiting = waiting_since(asked);
        assert_eq!(waiting.due(asked, true), Ok(()));
        assert_eq!(waiting.due(asked + std::time::Duration::from_secs(90), true), Ok(()));
        assert_eq!(waiting.due(asked + WAITS_FOR, true), Ok(()), "the window is inclusive");
    }

    /// D171: a pack installed an afternoon later does not read a forgotten capture over
    /// whatever is on the clipboard by then.
    #[test]
    fn a_read_that_waited_too_long_is_dropped() {
        let asked = std::time::Instant::now();
        let later = asked + WAITS_FOR + std::time::Duration::from_secs(1);
        assert!(waiting_since(asked).due(later, true).is_err());
    }

    /// D171: a card closed into the history moves its file, and the janitor files a
    /// cardless spool file; either way there is nothing left to read.
    #[test]
    fn a_read_whose_file_is_gone_is_dropped() {
        let asked = std::time::Instant::now();
        assert_eq!(waiting_since(asked).due(asked, false), Err("its file is gone"));
    }

    /// D171: an install with no read waiting reads nothing, so installing a second pack
    /// from Settings does not touch the clipboard.
    #[test]
    fn an_install_with_nothing_waiting_reads_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let flow = CaptureFlow::new(NullBridge::default(), Policy::default(), save_config_in(dir.path()));
        assert!(flow.waiting().is_none());
        assert_eq!(run(flow.read_waiting()), None);
    }

    /// D171, from the flow's side, on whichever machine runs it: a read that found no pack
    /// is kept for one, and a read that found one keeps nothing.
    #[test]
    fn a_read_waits_exactly_when_there_is_no_pack() {
        let dir = tempfile::tempdir().expect("tempdir");
        let flow = CaptureFlow::new(NullBridge::default(), Policy::default(), save_config_in(dir.path()));
        let file = dir.path().join("capture.png");
        let rect = Rect { x: 4, y: 8, width: 120, height: 40 };

        let outcome = run(flow.read_text(&file, Some(rect), Some(false)));

        let no_pack = matches!(&outcome.recognised, Some(Recognised::Failed(why)) if why == NO_PACK);
        assert_eq!(no_pack, !crate::ocr::ready(), "{outcome:?}");
        match flow.waiting() {
            Some(waiting) => {
                assert!(no_pack, "a read kept waiting although it was read: {outcome:?}");
                assert_eq!(waiting.path, file);
                assert_eq!(waiting.rect, Some(rect));
                assert_eq!(waiting.linebreaks, Some(false));
            }
            None => assert!(!no_pack, "a read that found no pack was not kept for one"),
        }
    }
}
