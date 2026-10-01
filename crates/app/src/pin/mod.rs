// SPDX-License-Identifier: GPL-3.0-or-later

//! Pinned screenshots: `spec/07` §3.
//!
//! A pin is a capture left on the screen exactly where it was taken, at 1:1, above
//! everything, on every workspace — "so it looks like the screen froze there". It is the
//! other half of what the Quick Access Overlay is for: the card is what you do with a
//! capture, the pin is what you do when the capture *is* the thing you need to keep
//! looking at while you work somewhere else.
//!
//! `spec/11` schedules this for M4. It is here because the card's Pin button existed and
//! did nothing, and a control that reports its own absence is only better than a broken
//! one for as long as nobody wants to press it.
//!
//! The arithmetic is `core::pin`. This is the window.

mod window;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::glib;
use octosnap_core::{CaptureResult, Rect};
use octosnap_shell::{Cue, ShellBridge};
use tracing::{debug, info};

pub use window::{PinHost, PinId};

use crate::flow::CaptureFlow;

/// What a pin does with its capture when it closes.
///
/// A closure rather than a typed handle to the overlay, for the same reason the flow's
/// card shower is one: the overlay *owns* the pins (`Qao::pins`), so the way back has to
/// be the weak half of the pair or neither would ever be freed. It also keeps this file
/// free of `spec/04` -- a pin knows it has a capture to give back, not who wants it.
///
/// The path is the saved copy's, and it travels with the capture in both directions: the
/// card that comes back is the card that was pinned, so it has to offer the same primary
/// action it offered before -- and Trash can still name the file it means (`spec/04` §3),
/// which a bare `bool` could not.
type Unpinned = Box<dyn Fn(&CaptureResult, Option<&std::path::Path>)>;

/// One pin's bookkeeping.
struct Entry {
    pin: Rc<window::Pin>,
    /// The copy on disk, when there is one. Tracked here rather than on the window
    /// because the pin's own Save is what creates it, and the card the pin goes back to
    /// has to know -- both to draw Trash instead of Save and to name the file.
    saved: RefCell<Option<std::path::PathBuf>>,
}

/// Every pinned screenshot on the screen.
///
/// Separate from `Qao` rather than folded into it, because the two have opposite
/// lifetimes and opposite geometry: cards are transient, stacked and placed by the
/// overlay; pins are permanent until dismissed, independent, and placed by their own
/// capture's rect. The only thing they share is that a Wayland client cannot position
/// either of them.
pub struct Pins<B: ShellBridge + 'static> {
    app: adw::Application,
    flow: Rc<CaptureFlow<B>>,
    pins: RefCell<HashMap<PinId, Entry>>,
    next_id: Cell<PinId>,
    /// `spec/07` §3.1's "Hide/Show pinned".
    hidden: Cell<bool>,
    /// Where a closed pin's capture goes, installed by `Qao::set_pins`.
    unpinned: RefCell<Option<Unpinned>>,
    me: RefCell<std::rc::Weak<Self>>,
}

impl<B: ShellBridge + 'static> std::fmt::Debug for Pins<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pins")
            .field("count", &self.pins.borrow().len())
            .field("hidden", &self.hidden.get())
            .finish_non_exhaustive()
    }
}

impl<B: ShellBridge + 'static> Pins<B> {
    pub fn new(app: &adw::Application, flow: Rc<CaptureFlow<B>>) -> Rc<Self> {
        window::install_style();
        let pins = Rc::new(Self {
            app: app.clone(),
            flow,
            pins: RefCell::new(HashMap::new()),
            next_id: Cell::new(1),
            hidden: Cell::new(false),
            unpinned: RefCell::new(None),
            me: RefCell::new(std::rc::Weak::new()),
        });
        *pins.me.borrow_mut() = Rc::downgrade(&pins);
        pins
    }

    fn weak(&self) -> std::rc::Weak<Self> {
        self.me.borrow().clone()
    }

    /// Installs the way back to the overlay (`docs/decisions.md` D47).
    ///
    /// Without it a pin still closes, and its capture is then reachable only as the file
    /// in the spool -- which is why this is set once at startup rather than being
    /// optional behaviour.
    pub fn set_unpinned(&self, unpinned: Unpinned) {
        *self.unpinned.borrow_mut() = Some(unpinned);
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.pins.borrow().len()
    }

    /// The capture files pinned windows are showing, for the history janitor.
    #[must_use]
    pub fn open_paths(&self) -> Vec<std::path::PathBuf> {
        self.pins.borrow().values().map(|e| e.pin.capture().path).collect()
    }

    /// Puts a capture on the screen where it was taken, because the user asked for it: a
    /// card's pin, the editor's, the history's, a file's. Returns whether one appeared,
    /// and tinks when it did (D134).
    pub fn pin(
        self: &Rc<Self>,
        capture: &CaptureResult,
        saved_to: Option<&std::path::Path>,
    ) -> bool {
        let pinned = self.pin_as_captured(capture, saved_to);
        if pinned {
            let flow = Rc::clone(&self.flow);
            glib::spawn_future_local(async move { flow.cue(Cue::Pinned).await });
        }
        pinned
    }

    /// [`Self::pin`] without the sound, for `ACT-01`'s after-capture pin, which is part of a
    /// capture whose shutter has just sounded.
    ///
    /// `saved_to` travels with the capture and comes back with it: the pin does nothing
    /// with it, but the card this capture returns to when the pin closes has to offer
    /// Trash rather than a second Save if the file is already on disk -- and Trash needs
    /// the path, not the fact.
    pub fn pin_as_captured(
        self: &Rc<Self>,
        capture: &CaptureResult,
        saved_to: Option<&std::path::Path>,
    ) -> bool {
        // A new pin makes the hidden ones visible again, the same way a new capture ends
        // the overlay's hide-all: the alternative is a pin that was asked for and cannot
        // be seen, with nothing on screen to explain why.
        if self.hidden.replace(false) {
            self.set_all_hidden(false);
        }

        let id = self.next_id.get();
        self.next_id.set(id + 1);

        let host: Rc<dyn PinHost> = self.clone();
        let Some(pin) = window::Pin::new(&self.app, id, capture.clone(), &host) else {
            return false;
        };
        self.pins.borrow_mut().insert(
            id,
            Entry { pin, saved: RefCell::new(saved_to.map(std::path::Path::to_path_buf)) },
        );
        info!(id, saved = saved_to.is_some(), pins = self.pins.borrow().len(), "pinned");
        true
    }

    /// `spec/05` §4.11: "A pinned window showing this image updates after crop."
    ///
    /// Answers how many pins took it. The identity is the *source* capture's path,
    /// because that is what the editor and the pin have in common: the pin was created
    /// either from the card that opened the editor -- same path -- or from the editor's
    /// own Pin button, whose path is the render. Both are passed, so a second crop
    /// updates the pin the first one created.
    pub fn refresh(self: &Rc<Self>, sources: &[&std::path::Path], updated: &CaptureResult) -> usize {
        let matching: Vec<PinId> = self
            .ids()
            .into_iter()
            .filter(|id| {
                self.pins.borrow().get(id).is_some_and(|entry| {
                    let showing = entry.pin.capture().path;
                    sources.iter().any(|source| showing == **source)
                })
            })
            .collect();
        let mut updated_count = 0;
        for id in matching {
            let pin = self.pins.borrow().get(&id).map(|entry| Rc::clone(&entry.pin));
            if let Some(pin) = pin
                && pin.set_capture(updated.clone())
            {
                updated_count += 1;
            }
        }
        if updated_count > 0 {
            info!(pins = updated_count, "pinned copies updated");
        }
        updated_count
    }

    /// `spec/07` §3.1's "Close all pinned".
    pub fn close_all(self: &Rc<Self>) {
        for id in self.ids() {
            self.close(id);
        }
    }

    /// `spec/07` §3.1's "Hide/Show pinned".
    ///
    /// `set_visible(false)`, not opacity 0: `spec/07` §3.1 records a fix for exactly that
    /// mistake — "hidden pins must not remain clickable" [D 4.7.5]. An invisible window
    /// that still swallows clicks is a dead patch of desktop nobody can explain.
    pub fn toggle_hidden(self: &Rc<Self>) {
        let hidden = !self.hidden.get();
        self.hidden.set(hidden);
        if hidden {
            self.hide_all();
        } else {
            self.set_all_hidden(false);
        }
        info!(hidden, pins = self.pins.borrow().len(), "pinned visibility toggled");
    }

    /// Hides every pin, having first asked each one where it is.
    ///
    /// Showing a pin places it from scratch at the rect the app remembers, because hiding
    /// it destroyed the toplevel and Mutter's record of where it was. That remembered rect
    /// is right for every move the app made itself: a placement and an arrow key each
    /// answer with where they landed, and `remember` keeps the answer.
    ///
    /// It is wrong for the one move the app never hears about. `spec/07` §3.1 grants "move
    /// by dragging anywhere" alongside the arrow keys, and `GtkWindowHandle` hands a drag
    /// to the compositor, which runs the whole grab and reports nothing back — so a dragged
    /// pin sat somewhere the app had no idea about, and the next Hide/Show snapped it back
    /// to wherever the app had last moved it.
    ///
    /// So the app asks, once, at the only moment the answer matters and the window is still
    /// there to give it. A delta of nothing is the question: `MoveWindowBy` moves the frame
    /// to where it already is and returns the rect, which is the only reading anyone can
    /// take of a position the compositor owns. A reading that fails is not fatal — the
    /// remembered rect still holds every move the app made — so the pin is hidden either
    /// way.
    fn hide_all(self: &Rc<Self>) {
        let open: Vec<(PinId, Rc<window::Pin>)> =
            self.pins.borrow().iter().map(|(id, e)| (*id, Rc::clone(&e.pin))).collect();

        for (id, pin) in open {
            let Some(path) = pin.object_path() else {
                pin.set_hidden(true);
                continue;
            };
            let flow = Rc::clone(&self.flow);
            let pins = self.weak();
            glib::spawn_future_local(async move {
                let located = flow.bridge().move_window_by(&path, 0, 0).await;
                let Some(pins) = pins.upgrade() else { return };
                // The user can ask for the pins back while the question is in flight. The
                // answer then describes a window that is already gone, and acting on it
                // would both record a rect Mutter never used and hide a pin that has just
                // been shown.
                if !pins.hidden.get() {
                    return;
                }
                let pin = pins.pins.borrow().get(&id).map(|e| Rc::clone(&e.pin));
                let Some(pin) = pin else { return };
                match located {
                    Ok(at) => {
                        debug!(id, ?at, "pin located before hiding");
                        pin.remember(at);
                    }
                    Err(e) => debug!(id, "could not locate the pin before hiding: {e}"),
                }
                pin.set_hidden(true);
            });
        }
    }

    /// Only ever called with `false` — showing is the half that needs no question asked.
    ///
    /// Hiding goes through `hide_all` instead, because a pin has to be read before its
    /// window stops existing. Passing `true` here would hide the pins and lose every
    /// position the app did not itself cause, which is the bug D48 is about.
    fn set_all_hidden(&self, hidden: bool) {
        for entry in self.pins.borrow().values() {
            entry.pin.set_hidden(hidden);
        }
    }

    /// Keeps a pin's idea of where it is in step with the compositor's.
    ///
    /// Free to do: both moves already answer with a landed rect and already log it. Not
    /// doing it was the whole bug — the rect went into the log and nowhere else, so the
    /// pin's own idea of where it was never left the rect its capture came from.
    fn remember(pins: &std::rc::Weak<Self>, id: PinId, at: Rect) {
        let Some(pins) = pins.upgrade() else { return };
        let pin = pins.pins.borrow().get(&id).map(|e| Rc::clone(&e.pin));
        if let Some(pin) = pin {
            pin.remember(at);
        }
    }

    /// Asks for the pin's shadow, if it has one, once the pin has been placed (D132).
    fn shade_placed(pins: &std::rc::Weak<Self>, id: PinId, path: &str) {
        let Some(pins) = pins.upgrade() else { return };
        let look = pins.pins.borrow().get(&id).and_then(|e| e.pin.shadow_look());
        if let Some((radius, opacity)) = look {
            pins.shade(id, path.to_owned(), radius, opacity);
        }
    }

    fn ids(&self) -> Vec<PinId> {
        self.pins.borrow().keys().copied().collect()
    }

    fn capture_of(&self, id: PinId) -> Option<CaptureResult> {
        self.pins.borrow().get(&id).map(|entry| entry.pin.capture())
    }
}

impl<B: ShellBridge + 'static> PinHost for Pins<B> {
    fn place(&self, id: PinId, path: String, placement: octosnap_shell::Placement) {
        let flow = Rc::clone(&self.flow);
        let pins = self.weak();
        glib::spawn_future_local(async move {
            match flow.bridge().place_window(&path, "pin", &placement, 0).await {
                // The path is here rather than only in the error because it is the handle
                // anything outside the app has on this window -- including a test that
                // wants to move a pin the way a drag does, behind the app's back.
                Ok(landed) => {
                    debug!(id, %path, ?landed, "pin placed");
                    Self::remember(&pins, id, landed);
                    // Once placed, so the compositor has the window to draw around: a pin
                    // shown again, zoomed or cropped is placed again, and asked again.
                    Self::shade_placed(&pins, id, &path);
                }
                Err(e) => tracing::warn!(id, "could not place the pin: {e}"),
            }
        });
    }

    fn shade(&self, id: PinId, path: String, radius: i32, opacity: f64) {
        let flow = Rc::clone(&self.flow);
        glib::spawn_future_local(async move {
            // Debug, not a warning: an extension from before D132 has no such method, and
            // a pin without a shadow is still a pin.
            match flow.bridge().set_window_shadow(&path, radius, opacity).await {
                Ok(()) => debug!(id, radius, opacity, "pin shadow set"),
                Err(e) => debug!(id, "could not set the pin's shadow: {e}"),
            }
        });
    }

    fn nudge(&self, id: PinId, path: String, dx: i32, dy: i32) {
        let flow = Rc::clone(&self.flow);
        let pins = self.weak();
        glib::spawn_future_local(async move {
            match flow.bridge().move_window_by(&path, dx, dy).await {
                Ok(landed) => {
                    debug!(id, ?landed, "pin nudged");
                    Self::remember(&pins, id, landed);
                }
                Err(e) => tracing::warn!(id, "could not nudge the pin: {e}"),
            }
        });
    }

    fn hovered(&self, id: PinId, path: String, entered: bool) {
        // A pin that has already gone still emits a leave as its window is torn down, and
        // asking the extension to focus a window that no longer exists is an error the
        // log would carry on every close.
        if !self.pins.borrow().contains_key(&id) {
            return;
        }
        debug!(id, entered, "pin hover");
        let flow = Rc::clone(&self.flow);
        glib::spawn_future_local(async move {
            if let Err(e) = flow.bridge().focus_window(&path, entered).await {
                debug!(id, "could not move focus for a pin: {e}");
            }
        });
    }

    fn copy(&self, id: PinId) {
        let Some(capture) = self.capture_of(id) else { return };
        let flow = Rc::clone(&self.flow);
        glib::spawn_future_local(async move {
            if let Err(e) = flow.copy(&capture).await {
                tracing::warn!("copying a pin failed: {e}");
            }
        });
    }

    fn save(&self, id: PinId) {
        let Some(capture) = self.capture_of(id) else { return };
        let flow = Rc::clone(&self.flow);
        let pins = self.weak();
        glib::spawn_future_local(async move {
            match flow.save_capture(&capture).await {
                Ok(path) => {
                    info!(path = %path.display(), "pin saved");
                    // Recorded on success, not on the click: a save that failed has put
                    // nothing on disk, and the card this pin goes back to would then
                    // offer Trash for a file the user never got.
                    if let Some(pins) = pins.upgrade()
                        && let Some(entry) = pins.pins.borrow().get(&id)
                    {
                        *entry.saved.borrow_mut() = Some(path);
                    }
                }
                Err(e) => tracing::warn!("saving a pin failed: {e}"),
            }
        });
    }

    /// `spec/07` §3.1's Annotate, through the overlay's editor opener like a card's
    /// pencil. The pin stays: `PIN-08` is the editor updating it after a crop.
    fn annotate(&self, id: PinId) {
        let Some(capture) = self.capture_of(id) else { return };
        match crate::overlay() {
            Some(overlay) => {
                overlay.open_editor_for_pin(&capture);
                info!(id, "editor opened from a pin");
            }
            None => tracing::warn!(id, "no overlay to open the editor with"),
        }
    }

    /// `spec/07` §2.1's pin source. The pin does not move, close or change: a read puts
    /// text on the clipboard and says so in a notification, and nothing else.
    fn recognise(&self, id: PinId) {
        let Some(capture) = self.capture_of(id) else { return };
        let flow = Rc::clone(&self.flow);
        let app = self.app.clone();
        glib::spawn_future_local(async move {
            let outcome = flow.read_text(&capture.path, Some(capture.rect), None).await;
            crate::notify::capture_outcome(
                &app,
                &outcome,
                crate::settings::Settings::load().notifications_enabled(),
            );
        });
    }

    /// The capture left through the grip (`PIN-05`). With Alt the pin stays; otherwise it
    /// closes **into the history** rather than back to the stack -- the user has just put
    /// the capture where they wanted it, so a card asking again would be noise. The spool
    /// copy stays for the janitor: the drop target may still be reading the file.
    fn dragged_out(&self, id: PinId, keep: bool) {
        if keep {
            return;
        }
        let pins = self.weak();
        glib::idle_add_local_once(move || {
            let Some(pins) = pins.upgrade() else { return };
            let Some(entry) = pins.pins.borrow_mut().remove(&id) else { return };
            entry.pin.destroy();
            if let Some(history) = crate::history() {
                history.file(&entry.pin.capture(), entry.saved.borrow().as_deref(), None, true);
            }
            info!(id, remaining = pins.pins.borrow().len(), "pin closed after a drag-out");
        });
    }

    /// `spec/07` §3.1's close -- which is also the capture's way back to the overlay.
    ///
    /// `spec/04` §3's Pin closes the card, so while a pin is up the capture is on the
    /// screen and nowhere else: it is the stack's capture on loan, not a copy of it. The
    /// close has to give it back, or the control that ends a pin would be the same
    /// control that quietly throws a capture away (`docs/decisions.md` D47).
    ///
    /// Every route in, including "Close all pinned", because there is no reading of
    /// "close" on which a capture should evaporate.
    fn close(&self, id: PinId) {
        // Deferred a turn, so this can be called from inside the pin's own signal handler
        // without destroying the widget the handler is returning through.
        let pins = self.weak();
        glib::idle_add_local_once(move || {
            let Some(pins) = pins.upgrade() else { return };
            let Some(entry) = pins.pins.borrow_mut().remove(&id) else { return };
            entry.pin.destroy();
            info!(id, remaining = pins.pins.borrow().len(), "pin closed");

            // After the window is down, so one capture is never in two places at once.
            match pins.unpinned.borrow().as_ref() {
                Some(unpinned) => {
                    unpinned(&entry.pin.capture(), entry.saved.borrow().as_deref());
                }
                // Only reachable if `Qao::set_pins` never ran, which would mean there was
                // no overlay to pin from either. Worth a line rather than a silent drop.
                None => tracing::warn!(id, "a closed pin has nowhere to go back to"),
            }
        });
    }

    fn close_all_pins(&self) {
        for id in self.ids() {
            self.close(id);
        }
    }
}
