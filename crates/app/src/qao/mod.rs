// SPDX-License-Identifier: GPL-3.0-or-later

//! The Quick Access Overlay: the stack of cards a capture lands in (`spec/04`).
//!
//! `spec/04` §9 calls this the `QaoStack`. It owns every card, decides where each one
//! sits, and is the only thing that talks to the extension about window geometry -- a
//! card cannot place itself, because its position depends on its neighbours.
//!
//! The arithmetic is not here. `core::qao` holds the stack model, the sizes and the
//! timer, all of it pure and tested without a display; this file is the part that has to
//! touch GTK and D-Bus, and it is deliberately thin enough to read as plumbing.

mod card;
mod style;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use gtk::gdk;
use gtk::glib;
use adw::prelude::*;
use octosnap_core::qao::{
    AutoClose, CardId, CardKind, Edge, PendingTrash, RecentlyClosed, SHADOW_MARGIN, Size,
    Slot, Stack, TRASH_GRACE_MS,
};
use octosnap_core::{CaptureResult, Rect};
use octosnap_shell::{Placement, PlacementMonitor, ShellBridge};
use tracing::{debug, info, warn};

pub use card::{CardConfig, CardHost, CloseReason};

use crate::flow::CaptureFlow;
use crate::notify;

/// `spec/08` §4's Quick Access settings, as the overlay uses them.
#[derive(Debug, Clone, Copy)]
pub struct QaoConfig {
    pub edge: Edge,
    /// `spec/08` §1's "Move to active screen": the card follows the pointer's display.
    pub follow_pointer: bool,
    pub size_step: u8,
    pub auto_close: AutoClose,
    /// `qao-close-after-drag`.
    pub close_after_drag: bool,
    /// `qao-shortcuts`.
    pub shortcuts: bool,
    /// `qao-ask-destination`: Save opens a chooser instead of writing straight out.
    pub ask_destination: bool,
}

impl Default for QaoConfig {
    fn default() -> Self {
        Self {
            edge: Edge::default(),
            follow_pointer: true,
            size_step: octosnap_core::qao::DEFAULT_SIZE_STEP,
            auto_close: AutoClose::default(),
            close_after_drag: true,
            shortcuts: true,
            ask_destination: false,
        }
    }
}

/// One card's bookkeeping.
struct Entry {
    card: Rc<card::Card>,
    /// Where the window actually landed, which is not always where it was asked to go.
    landed: Rect,
    /// The copy the after-capture plan wrote out, when it wrote one.
    ///
    /// Held here rather than on the card because it is not something the card draws --
    /// `already_saved` decides that -- and because it is the overlay that has to name a
    /// file when Trash is pressed (`spec/04` §3). A `bool` cannot name one.
    saved_to: Option<std::path::PathBuf>,
    /// The document a closed editor left on this card, when its picture is a render of
    /// one (D108): what the card's Annotate opens instead of the flattened picture.
    held: Option<Held>,
}

/// An edited document parked on a card, and where its capture came from.
struct Held {
    document: Box<crate::editor::actions::Document>,
    origin: Origin,
}

/// Where an editor's capture came from, which decides where it goes back to (D108).
///
/// An editor is the stack's capture on loan, the way a pin is (D47): × hands it back as
/// a card and Final Close files it. Unless it was never the stack's to lend.
#[derive(Debug, Clone)]
enum Origin {
    /// The stack lent it -- a card closed for the editor, or the capture never had one
    /// yet -- and gets it back, with the copy the after-capture plan wrote, if any.
    Stack { capture: Box<CaptureResult>, saved_to: Option<std::path::PathBuf> },
    /// Something else keeps it -- a pin, or a project file -- and nothing goes back.
    Kept,
}

/// A capture on its way to the trash, held for the length of `spec/04` §3's undo window.
struct Doomed {
    capture: CaptureResult,
    /// The copy in the pictures folder, when the after-capture plan made one. This is the
    /// file `spec/04` §3 means by "the saved file"; the spool copy goes with it.
    saved_to: Option<std::path::PathBuf>,
}

pub struct Qao<B: ShellBridge + 'static> {
    app: adw::Application,
    flow: Rc<CaptureFlow<B>>,
    config: RefCell<QaoConfig>,
    stack: RefCell<Stack>,
    entries: RefCell<HashMap<CardId, Entry>>,
    /// The capture and the copy it had on disk, because a restored card has to offer the
    /// same primary action the closed one did.
    ///
    /// The fallback for a session without a history store; with one, `spec/04` §7's
    /// "card closes → move to history" is the store's, and this ring stays empty.
    recent: RefCell<RecentlyClosed<(CaptureResult, Option<std::path::PathBuf>)>>,
    /// `spec/07` §4's store, installed by `main` the way `pins` is: a peer built after
    /// the overlay, that the overlay files into when a card goes.
    history: RefCell<Option<Rc<crate::history::History>>>,
    next_id: Cell<CardId>,
    /// `spec/04` §5's hide/show all. Persists "until toggled or a new capture arrives".
    hidden: Cell<bool>,
    /// Every card stepped aside for a capture that would otherwise have them in it (D107).
    /// Not `hidden`, which is the user's and which a new capture undoes: this is undone by
    /// the capture that asked for it, when it ends.
    aside: Cell<bool>,
    /// The card that currently has the keyboard on loan (`spec/04` §3).
    focused: Cell<Option<CardId>>,
    /// Cards on their way out, held only until their close animation ends.
    closing: RefCell<Vec<Rc<card::Card>>>,
    /// Where a card's Pin button sends its capture (`spec/07` §3).
    ///
    /// Installed after construction rather than taken as an argument, because the two are
    /// peers: both need the capture flow, neither owns the other, and a card that is
    /// pinned closes itself, so the overlay has to be the one holding the reference.
    pins: RefCell<Option<Rc<crate::pin::Pins<B>>>>,
    /// Captures whose deletion is inside `spec/04` §3's undo window.
    pending_trash: RefCell<PendingTrash<Doomed>>,
    /// A handle to `self`, so work can be deferred onto the main loop from `&self`.
    ///
    /// `CardHost`'s methods take `&self` -- a card holds `Weak<dyn CardHost>` and cannot
    /// hand back a typed `Rc<Qao<B>>` -- but closing a card has to be deferred by one
    /// turn, and a deferred closure needs an owner it can upgrade. Recovering the `Rc`
    /// from the entries map would be circular, so it is kept here, set once in `new`.
    me: RefCell<std::rc::Weak<Self>>,
}

impl<B: ShellBridge + 'static> std::fmt::Debug for Qao<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Qao")
            .field("cards", &self.stack.borrow().len())
            .field("hidden", &self.hidden.get())
            .finish_non_exhaustive()
    }
}

impl<B: ShellBridge + 'static> Qao<B> {
    pub fn new(app: &adw::Application, flow: Rc<CaptureFlow<B>>, config: QaoConfig) -> Rc<Self> {
        install_style(app);
        let qao = Rc::new(Self {
            app: app.clone(),
            flow,
            config: RefCell::new(config),
            stack: RefCell::new(Stack::new()),
            entries: RefCell::new(HashMap::new()),
            recent: RefCell::new(RecentlyClosed::new(10)),
            history: RefCell::new(None),
            next_id: Cell::new(1),
            hidden: Cell::new(false),
            aside: Cell::new(false),
            focused: Cell::new(None),
            closing: RefCell::new(Vec::new()),
            pins: RefCell::new(None),
            pending_trash: RefCell::new(PendingTrash::new()),
            me: RefCell::new(std::rc::Weak::new()),
        });
        *qao.me.borrow_mut() = Rc::downgrade(&qao);
        qao
    }

    /// A weak handle to `self`, for deferring work onto the main loop from `&self`.
    fn weak(&self) -> std::rc::Weak<Self> {
        self.me.borrow().clone()
    }

    /// The close animation ended: tear the window down and let the card go.
    fn finish_close(&self, id: CardId) {
        let mut closing = self.closing.borrow_mut();
        let Some(index) = closing.iter().position(|c| c.id == id) else { return };
        let card = closing.remove(index);
        drop(closing);
        card.destroy();
    }

    /// Tells the overlay where Pin should send a capture -- and the pins the way back.
    ///
    /// Both directions are wired here because the overlay is the owner: it holds the
    /// `Rc`, so the return trip has to be the weak half or the pair would outlive the
    /// session. A pin is the stack's capture on loan (`docs/decisions.md` D47), and this
    /// is the loan's other end.
    pub fn set_pins(self: &Rc<Self>, pins: Rc<crate::pin::Pins<B>>) {
        let qao = Rc::downgrade(self);
        pins.set_unpinned(Box::new(move |capture, already_saved| {
            let Some(qao) = qao.upgrade() else { return };
            // A capture arriving at the stack is a capture arriving at the stack, whether
            // it came from the shutter or back from the screen -- including `show`'s
            // eviction, which is what keeps "Close all pinned" from overflowing the edge.
            if !qao.show(capture, already_saved) {
                warn!("a closed pin could not go back to the overlay");
            }
        }));
        *self.pins.borrow_mut() = Some(pins);
    }

    /// Installs `spec/07` §4's history store, where closed cards go from now on.
    pub fn set_history(&self, history: Rc<crate::history::History>) {
        *self.history.borrow_mut() = Some(history);
    }

    /// Every capture file a card on screen still needs, for the history janitor: a spool
    /// file a window holds must not be swept out from under it. A card carrying a
    /// document needs the capture the document is drawn on as well as its own picture.
    #[must_use]
    pub fn open_paths(&self) -> Vec<std::path::PathBuf> {
        let entries = self.entries.borrow();
        let bases = entries.values().filter_map(|e| e.held.as_ref());
        entries
            .values()
            .map(|e| e.card.capture.path.clone())
            .chain(bases.map(|held| held.document.capture.path.clone()))
            .collect()
    }

    pub fn update_config(&self, config: QaoConfig) {
        *self.config.borrow_mut() = config;
    }

    #[must_use]
    pub fn card_count(&self) -> usize {
        self.stack.borrow().len()
    }

    /// Puts a capture on screen as a card. Returns whether one actually appeared.
    ///
    /// The boolean is the whole point of the signature. `Outcome::card_shown` records
    /// "was shown", not "was asked for" (`docs/decisions.md` D27), so a card that fails
    /// to appear falls back to a notification and the user is never left with a shutter
    /// sound and nothing else.
    pub fn show(
        self: &Rc<Self>,
        capture: &CaptureResult,
        saved_to: Option<&std::path::Path>,
    ) -> bool {
        self.show_held(capture, saved_to, None).is_some()
    }

    /// [`Self::show`], and then the card's Copy or Save As.
    ///
    /// For `spec/07` §4.2's Copy and Save As on a recording whose GIF is still frames:
    /// writing it can take a minute, and a card's badge is where that is shown (D113). From
    /// the strip it was a minute of nothing, so the capture comes back to the stack and its
    /// card does it, badge and all. `false` when no card could be shown.
    pub fn show_and(
        self: &Rc<Self>,
        capture: &CaptureResult,
        saved_to: Option<&std::path::Path>,
        then: CardThen,
    ) -> bool {
        let Some(id) = self.show_held(capture, saved_to, None) else { return false };
        match then {
            // Not the card's Copy, so no Alt: the shortcut that took the capture can
            // still be held, and every default one has Alt in it.
            CardThen::Copy => self.copy_then(id, false),
            CardThen::SaveAs => self.choose_and_save(id),
        }
        true
    }

    /// `spec/04` §3's "if 'ask for destination' is on, a file chooser opens; card closes",
    /// and [`Self::show_and`]'s Save As. The card stays up until the chooser is answered,
    /// because a cancelled save that had already closed the card would have thrown the
    /// capture away for the sake of a dialog the user changed their mind about.
    fn choose_and_save(&self, id: CardId) {
        let Some(capture) = self.capture_of(id) else {
            warn!(?id, "no capture behind the card to save");
            return;
        };
        info!(?id, "asking where to save the card");
        let flow = Rc::clone(&self.flow);
        // A GIF still in frames starts writing now, while the user picks a name, and the
        // badge counts it (D113).
        if crate::recording::render::pending(&capture.path)
            && let Some(card) = self.card(id)
        {
            card.follow_render();
        }
        let (qao, app) = (self.weak(), self.app.clone());
        glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder()
                .title(save_title(&capture))
                .initial_name(flow.suggested_name(&capture))
                .modal(false)
                .build();
            match dialog.save_future(None::<&gtk::Window>).await {
                Ok(file) => {
                    let Some(path) = file.path() else {
                        warn!("the chooser returned a file with no path");
                        return;
                    };
                    match flow.save_capture_as(&capture, &path).await {
                        Ok(path) => info!(path = %path.display(), "card saved as"),
                        Err(e) => {
                            warn!("card save-as failed: {e}");
                            notify::action_failed(&app, "Save", &e.to_string());
                            return;
                        }
                    }
                    if let Some(qao) = qao.upgrade() {
                        qao.close(id, CloseReason::ActionTaken);
                    }
                }
                // Dismissed. Not a failure, and the card is deliberately left alone.
                Err(e) => info!("the save chooser was dismissed: {e}"),
            }
        });
    }

    /// [`Self::show`], for a card that carries the document its picture was rendered
    /// from (D108). The card's id, or `None` when there is none to show.
    fn show_held(
        self: &Rc<Self>,
        capture: &CaptureResult,
        saved_to: Option<&std::path::Path>,
        held: Option<Held>,
    ) -> Option<CardId> {
        // `spec/04` §5: hide-all "persists until toggled or a new capture arrives".
        if self.hidden.replace(false) {
            self.set_all_hidden(self.aside.get());
        }

        let config = *self.config.borrow();
        let id = self.next_id.get();
        self.next_id.set(id + 1);

        let host: Rc<dyn CardHost> = self.clone();
        let card = card::Card::new(
            &self.app,
            id,
            capture.clone(),
            CardConfig {
                size_step: config.size_step,
                auto_close: config.auto_close,
                kind: card_kind(capture),
                already_saved: saved_to.is_some(),
                shortcuts: config.shortcuts,
                arrive_after_ms: remaining_flight(capture),
            },
            &host,
        );

        let size = card.card_size();
        if size.width <= 0 || size.height <= 0 {
            warn!(id, "a capture with no area cannot become a card");
            card.destroy();
            return None;
        }

        // Room first, so the newcomer is placed into a stack that has already collapsed
        // rather than being placed and then immediately reflowed.
        self.evict_for(size.height);

        self.stack.borrow_mut().push_newest(Slot { id, height: size.height });
        self.entries.borrow_mut().insert(
            id,
            Entry {
                card: Rc::clone(&card),
                landed: Rect::empty(),
                saved_to: saved_to.map(std::path::Path::to_path_buf),
                held,
            },
        );

        // No reflow here. The new card has no configured surface yet, and placing a
        // window Mutter has not finished mapping makes it assert -- see
        // `Card::wait_for_surface`. The card calls `ready` when it is placeable, and that
        // is what reflows the stack.
        card.start_timer();
        info!(id, cards = self.stack.borrow().len(), "card shown");
        Some(id)
    }

    fn evict_for(self: &Rc<Self>, height: i32) {
        let work_height = self.work_area_height();
        let doomed = self.stack.borrow().evict_for(work_height, height);
        for id in doomed {
            info!(id, "closing the oldest card to make room");
            self.close(id, CloseReason::Evicted);
        }
    }

    /// The work area the stack is measured against.
    ///
    /// Read from GDK rather than from the extension: this is only used to decide how many
    /// cards fit, the answer changes with the monitor the pointer is on, and a D-Bus round
    /// trip on the capture path to learn a number that is about to be approximate anyway
    /// would cost more than it is worth. The extension still owns the actual placement,
    /// where the exact work area does matter.
    fn work_area_height(&self) -> i32 {
        let Some(display) = gdk::Display::default() else { return 1080 };
        let monitors = display.monitors();
        let mut best = 0;
        for index in 0..monitors.n_items() {
            if let Some(monitor) = monitors.item(index).and_downcast::<gdk::Monitor>() {
                best = best.max(monitor.geometry().height());
            }
        }
        if best > 0 { best } else { 1080 }
    }

    /// Reflows on the next main-loop turn.
    ///
    /// Used when a card *leaves*, where every remaining window is already configured and
    /// the only reason to defer is to let the caller's signal handler return first. An
    /// arriving card cannot use this: it has no surface yet, and `ready` is its signal.
    fn schedule_reflow(self: &Rc<Self>) {
        let qao = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(qao) = qao.upgrade() {
                qao.reflow();
            }
        });
    }

    /// Places every card at its current slot (`spec/04` §5's stack shift).
    ///
    /// Every card, not just the ones that moved: the placements are computed from a
    /// running sum, so a card whose neighbour changed height has moved even though
    /// nothing about it did. Asking the extension to place a window that is already in
    /// the right place is one D-Bus call that returns the same rect, which is cheaper
    /// than tracking which ones are stale and getting it wrong.
    fn reflow(self: &Rc<Self>) {
        let placements = self.stack.borrow().placements();
        if placements.is_empty() {
            return;
        }
        let config = *self.config.borrow();

        for (id, offset) in placements {
            let found = self.entries.borrow().get(&id).and_then(|entry| {
                entry
                    .card
                    .object_path()
                    .map(|path| (path, entry.card.card_size(), entry.landed.is_empty()))
            });
            let Some((path, card, first_time)) = found else {
                warn!(id, "card has no D-Bus object path; it cannot be placed");
                continue;
            };

            // `spec/04` §5: a card that is already on screen *slides* to its new slot,
            // whether the stack grew below it or collapsed under it. A card being placed
            // for the first time has nowhere to slide from -- animating it would send it
            // travelling from wherever Mutter happened to map it.
            let animate_ms = if first_time { 0 } else { STACK_SHIFT_MS };

            let placement = placement_for(offset, card, config);
            let qao = Rc::downgrade(self);
            glib::spawn_future_local(async move {
                let Some(qao) = qao.upgrade() else { return };
                match qao.flow.bridge().place_window(&path, "qao", &placement, animate_ms).await {
                    Ok(landed) => {
                        if let Some(entry) = qao.entries.borrow_mut().get_mut(&id) {
                            entry.landed = landed;
                        }
                        debug!(id, ?landed, "card placed");
                    }
                    // Not fatal. An unplaced card is a card wherever Mutter put it, which
                    // is worse than the corner but far better than no card at all -- and
                    // the capture is still reachable from it.
                    Err(e) => warn!(id, "could not place the card: {e}"),
                }
            });
        }
    }

    /// Where a card ended up, for the extension's fly animation and for tests.
    #[must_use]
    pub fn card_rect(&self, id: CardId) -> Option<Rect> {
        self.entries.borrow().get(&id).map(|e| card::card_rect(e.landed))
    }

    fn capture_of(&self, id: CardId) -> Option<CaptureResult> {
        self.entries.borrow().get(&id).map(|e| e.card.capture.clone())
    }

    /// `CardHost::close` for a card whose recording has just been saved: once the close has
    /// filed it, the spool copy -- the GIF, and the frames it was written from -- goes at
    /// once rather than at the janitor's next run (D113). Not for a drag's close: a drop
    /// target may still be reading the file.
    fn close_saved(&self, id: CardId) {
        if !self.entries.borrow().contains_key(&id) {
            return;
        }
        let qao = self.weak();
        glib::idle_add_local_once(move || {
            if let Some(qao) = qao.upgrade() {
                qao.dismiss(id, CloseReason::ActionTaken);
                let history = qao.history.borrow().clone();
                if let Some(history) = history {
                    history.sweep_now();
                }
            }
        });
    }

    fn card(&self, id: CardId) -> Option<Rc<card::Card>> {
        self.entries.borrow().get(&id).map(|e| Rc::clone(&e.card))
    }

    /// `spec/04` §5's "Close all".
    pub fn close_all(self: &Rc<Self>) {
        for id in self.stack.borrow().ids() {
            self.close(id, CloseReason::CloseAll);
        }
    }

    /// `spec/04` §5's "Save all".
    pub fn save_all(self: &Rc<Self>) {
        for id in self.stack.borrow().ids() {
            self.save(id);
        }
    }

    /// `spec/04` §5's hide/show all overlays.
    pub fn toggle_hidden(&self) {
        let hidden = !self.hidden.get();
        self.hidden.set(hidden);
        self.set_all_hidden(hidden || self.aside.get());
        info!(hidden, "overlay visibility toggled");
    }

    /// Steps every card aside while a capture reads `rect`, if any of them is in it.
    ///
    /// `spec/07` §1's scrolling capture reads the screen for as long as it runs, and new
    /// rows come off the foot of every frame -- where the stack stands -- so a card in the
    /// selection was stitched into every step of the capture: the first full-screen one
    /// had three of them in it a dozen times over (2026-09-23, D107). Cards outside the
    /// selection stay put. Returns whether they went; [`Qao::step_back`] brings them back.
    pub fn step_aside(&self, rect: Rect) -> bool {
        let inside = self
            .entries
            .borrow()
            .values()
            .any(|entry| card::card_rect(entry.landed).overlap_area(rect) > 0);
        if !inside || self.aside.replace(true) {
            return false;
        }
        self.set_all_hidden(true);
        info!(cards = self.stack.borrow().len(), "cards stepped aside for a capture");
        true
    }

    /// Brings back what [`Qao::step_aside`] stepped aside, as the user had left it.
    pub fn step_back(&self) {
        if self.aside.replace(false) {
            self.set_all_hidden(self.hidden.get());
            info!("cards back after the capture");
        }
    }

    fn set_all_hidden(&self, hidden: bool) {
        for entry in self.entries.borrow().values() {
            entry.card.set_hidden(hidden);
        }
    }

    /// `spec/04` §3's "Restore recently closed".
    ///
    /// Returns whether anything came back, so a shortcut pressed with nothing to restore
    /// can say so rather than appearing to do nothing.
    pub fn restore_recent(self: &Rc<Self>) -> bool {
        // From the store when there is one: the card that closed last is its most recently
        // filed entry, and taking it out puts the files back in the spool (D47: a capture
        // is in exactly one place). The in-memory ring is the session without a store.
        let history = self.history.borrow().clone();
        let restored = match &history {
            Some(history) => history.last_filed().and_then(|entry| history.take(&entry.id)),
            None => self.recent.borrow_mut().pop(),
        };
        let Some((capture, saved_to)) = restored else {
            info!("nothing recently closed to restore");
            return false;
        };
        // Restored as a fresh card rather than with its old id: it is going back on top
        // of the stack, not into the hole it left, and reusing the id would make a stale
        // reference from before the close resolve to the new card.
        //
        // The saved copy comes back with it. A restored card that had already been
        // written out has to offer Trash, not a second Save -- the same rule the pin's
        // return trip follows, and for the same reason: this is the card that closed, not
        // a new capture.
        self.show(&capture, saved_to.as_deref())
    }

    /// `spec/04` §7: "card closes (any reason) → move to history … unless saved and
    /// 'keep history' is off".
    ///
    /// A card that closed because an action took its file -- Annotate opened the editor
    /// on it, a drag handed its URI to another application -- is filed with the spool
    /// copy left in place, because that other reader still has the path; the history's
    /// janitor removes the copy once no window holds it. Every other close removes the
    /// spool copy now: the history has the file, and the spool is for captures with a card.
    ///
    /// `rendering` keeps the spool copy too: a recording whose plan's held outputs are
    /// about to write its GIF from its frames (D113).
    fn file_in_history(
        &self,
        capture: &CaptureResult,
        saved_to: Option<&std::path::Path>,
        reason: CloseReason,
        rendering: bool,
    ) {
        let Some(history) = self.history.borrow().clone() else {
            self.recent.borrow_mut().push((capture.clone(), saved_to.map(Path::to_path_buf)));
            return;
        };
        let in_use_elsewhere = reason == CloseReason::ActionTaken || rendering;
        if history.files(saved_to.is_some()) {
            history.file(capture, saved_to, None, in_use_elsewhere);
        } else {
            info!(path = %capture.path.display(), "closed without filing: saved, and history does not keep saved captures");
            if !in_use_elsewhere {
                for path in [&capture.path, &capture.meta_path] {
                    if let Err(e) = std::fs::remove_file(path)
                        && e.kind() != std::io::ErrorKind::NotFound
                    {
                        warn!(path = %path.display(), "could not remove the spool copy: {e}");
                    }
                }
                octosnap_media::reel::remove(&octosnap_media::reel::beside(&capture.path));
            }
        }
    }

    /// Shows `spec/04` §8's tick on a card, then closes it.
    fn confirm_copied_kept(&self, id: CardId) {
        if let Some(card) = self.card(id) {
            card.confirm_copied_kept();
        }
    }

    /// Copies a card's capture; `keep` is `spec/04` §3's Alt, which leaves the card up.
    fn copy_then(&self, id: CardId, keep: bool) {
        let Some(capture) = self.capture_of(id) else { return };
        // A recording whose GIF is still frames is written first, and the badge says how
        // far it has got (D113).
        if let Some(card) = self.card(id) {
            card.follow_render();
        }
        let flow = Rc::clone(&self.flow);
        let qao = self.weak();
        glib::spawn_future_local(async move {
            let result = flow.copy(&capture).await;
            let Some(qao) = qao.upgrade() else { return };
            match result {
                Ok(()) if keep => {
                    info!(?id, "copied, and the card stays (Alt)");
                    qao.confirm_copied_kept(id);
                }
                Ok(()) => qao.confirm_copied(id),
                Err(e) => {
                    warn!("card copy failed: {e}");
                    notify::action_failed(&qao.app, "Copy", &e.to_string());
                }
            }
        });
    }

    fn confirm_copied(self: &Rc<Self>, id: CardId) {
        let Some(card) = self.entries.borrow().get(&id).map(|e| Rc::clone(&e.card)) else {
            // Closed while the clipboard was being written -- the timer, "close all", or
            // the user. The copy still happened, which is the part that mattered.
            return;
        };
        let qao = self.weak();
        card.confirm_copied(move || {
            if let Some(qao) = qao.upgrade() {
                qao.close(id, CloseReason::ActionTaken);
            }
        });
    }

    /// The grace period ran out. Moves the files and takes the toast down.
    fn carry_out_trash(self: &Rc<Self>, token: u64) {
        let Some(doomed) = self.pending_trash.borrow_mut().take(token) else {
            // Undo got there first, which is the whole point of the window.
            return;
        };
        self.app.withdraw_notification(&notify::trash_id(token));

        // The saved copy first, because it is the one `spec/04` §3 names and the one the
        // user can find again. The spool copy goes too: leaving it would mean a capture
        // the user sent to the trash is still sitting in the cache, and `spec/04` §7 is
        // explicit that a card "only ever references one file".
        //
        // Only the saved copy is trashed when there is one. Two entries in the bin for
        // one screenshot is a worse answer than one, and the spool file is OctoSnap's own
        // temporary -- the recoverable copy is the one in the pictures folder.
        //
        // A recording's reel goes the way its GIF would: to the bin when it was never
        // saved, since until then the frames are the recording (D113). A render still
        // writing the GIF is stopped first.
        crate::recording::render::cancel(&doomed.capture.path);
        let reel = octosnap_media::reel::beside(&doomed.capture.path);
        let reel = reel.is_dir().then_some(reel);
        for (path, keep) in [
            (doomed.saved_to.as_deref(), true),
            (Some(doomed.capture.path.as_path()), doomed.saved_to.is_none()),
            (reel.as_deref(), doomed.saved_to.is_none()),
            (Some(doomed.capture.meta_path.as_path()), false),
        ] {
            let Some(path) = path else { continue };
            let file = gtk::gio::File::for_path(path);
            let result = if keep { file.trash(gtk::gio::Cancellable::NONE) } else {
                file.delete(gtk::gio::Cancellable::NONE)
            };
            match result {
                Ok(()) => info!(path = %path.display(), trashed = keep, "trash carried out"),
                Err(e) => warn!(path = %path.display(), "could not remove the file: {e}"),
            }
        }
    }

    /// The Undo button on the toast. Brings the card back with its capture intact.
    pub fn undo_trash(self: &Rc<Self>, token: u64) -> bool {
        let Some(doomed) = self.pending_trash.borrow_mut().take(token) else {
            info!(token, "the undo window for that capture has closed");
            return false;
        };
        self.app.withdraw_notification(&notify::trash_id(token));
        info!(token, "trash undone");
        self.show(&doomed.capture, doomed.saved_to.as_deref())
    }

    /// The newest pending deletion, for an undo with no token to offer.
    pub fn undo_trash_newest(self: &Rc<Self>) -> bool {
        let Some(doomed) = self.pending_trash.borrow_mut().take_newest() else {
            info!("nothing is waiting to be trashed");
            return false;
        };
        info!("trash undone");
        self.show(&doomed.capture, doomed.saved_to.as_deref())
    }

    /// Drops a card, animating it out first when the reason calls for one.
    fn dismiss(self: &Rc<Self>, id: CardId, reason: CloseReason) {
        let Some(entry) = self.entries.borrow_mut().remove(&id) else {
            // Already gone. The timer firing on the same turn as a click is routine.
            return;
        };
        self.stack.borrow_mut().remove(id);
        if self.focused.get() == Some(id) {
            self.focused.set(None);
        }

        // D113: what a recording's plan held for this card runs now if the user left it
        // untouched, and is dropped if they did anything with it.
        let held = self.flow.release(&entry.card.capture.path);
        let runs = held.filter(|_| reason.leaves_it_untouched());
        if held.is_some() && runs.is_none() {
            info!(?reason, "the card was acted on; what the plan held for it is dropped");
        }

        // `spec/04` §7: "card closes (any reason) → move to history ... Restore recently
        // closed → back to a card". The history store itself is `spec/07` §4 and M4; the
        // in-session half of that promise works now.
        //
        // Pinning is the one exception, and it is less a special case than the definition
        // of a pin (`docs/decisions.md` D47): the capture has not closed, it has moved to
        // the screen, and the pin's own close is what brings it back. Filing it here as
        // well would let "Restore recently closed" put a second card in the stack for a
        // capture that is still pinned in front of it.
        if reason.files_in_history() {
            let capture = &entry.card.capture;
            self.file_in_history(capture, entry.saved_to.as_deref(), reason, runs.is_some());
        }
        if let Some(held) = runs {
            let (flow, app) = (Rc::clone(&self.flow), self.app.clone());
            let history = self.history.borrow().clone();
            let capture = entry.card.capture.clone();
            glib::spawn_future_local(async move {
                let outcome = flow.run_held(&capture, held).await;
                let enabled = crate::settings::Settings::load().notifications_enabled();
                notify::recording_saved(&app, &outcome, enabled);
                // The GIF is written: the history keeps it rather than a week of frames.
                if let Some(history) = history {
                    history.sweep_now();
                }
            });
        }
        // A card carrying a document carried the capture it is drawn on too, and the
        // document goes with the card -- so that capture goes where its own card's close
        // would have sent it (D108). Annotate takes the document out before it closes the
        // card, which is the one way the capture stays on loan.
        if let Some(held) = &entry.held {
            self.file_original(&held.origin);
        }

        let card = Rc::clone(&entry.card);
        if reason.animates() {
            // The overlay holds the card for the animation's duration. It cannot be the
            // animation that holds it: the animation belongs to the card, so a callback
            // owning the card would close a reference cycle and every closed card would
            // outlive the session with its window and its texture still allocated.
            self.closing.borrow_mut().push(Rc::clone(&card));
            let qao = self.weak();
            let edge = self.config.borrow().edge;
            card.slide_out(edge, move || {
                if let Some(qao) = qao.upgrade() {
                    qao.finish_close(id);
                }
            });
        } else {
            card.destroy();
        }

        self.schedule_reflow();
        info!(id, ?reason, remaining = self.stack.borrow().len(), "card closed");
    }

    /// What `spec/05` §7's bottom bar asks of the capture flow.
    ///
    /// Built here rather than in the editor because the flow is generic over the shell
    /// bridge and the editor is not -- and making it generic would put a type parameter on
    /// every widget in the module for the sake of four calls. Each closure does what the
    /// matching *card* button does, so an annotated image reaches the clipboard, the
    /// filename template, the `{n}` counter and `last_saved` by the same route an
    /// unannotated one does.
    fn editor_actions(&self, origin: Origin) -> crate::editor::EditorActions {
        let app = self.app.clone();
        let copy_flow = Rc::clone(&self.flow);
        let save_flow = Rc::clone(&self.flow);
        let as_path_flow = Rc::clone(&self.flow);
        let pin_app = app.clone();
        let pinner = self.pins.borrow().clone();
        let refresher = self.pins.borrow().clone();
        let pick_flow = Rc::clone(&self.flow);
        let read_flow = Rc::clone(&self.flow);

        crate::editor::EditorActions {
            copy: Box::new(move |capture, done| {
                let flow = Rc::clone(&copy_flow);
                let app = app.clone();
                glib::spawn_future_local(async move {
                    let outcome = match flow.copy(&capture).await {
                        Ok(()) => {
                            info!("copied an annotated image");
                            Ok(None)
                        }
                        Err(e) => {
                            warn!("annotated copy failed: {e}");
                            notify::action_failed(&app, "Copy", &e.to_string());
                            Err(e.to_string())
                        }
                    };
                    done(outcome);
                });
            }),
            save: Box::new(move |capture, done| {
                let flow = Rc::clone(&save_flow);
                glib::spawn_future_local(async move {
                    let outcome = match flow.save_capture(&capture).await {
                        Ok(path) => {
                            info!(path = %path.display(), "saved an annotated image");
                            Ok(Some(path))
                        }
                        Err(e) => {
                            warn!("annotated save failed: {e}");
                            Err(e.to_string())
                        }
                    };
                    done(outcome);
                });
            }),
            pin: Box::new(move |capture| {
                let Some(pins) = pinner.clone() else {
                    notify::unavailable(&pin_app, "Pin to the Screen", "pinned screenshots");
                    return false;
                };
                // `None` for the saved path: an annotated render has not been saved
                // anywhere the user chose, so a pin closed later puts the *capture* back
                // in the stack rather than claiming a file that does not exist.
                let pinned = pins.pin(&capture, None);
                if pinned {
                    info!("pinned an annotated image");
                }
                pinned
            }),
            save_as_path: Box::new(move |capture, path, done| {
                let flow = Rc::clone(&as_path_flow);
                glib::spawn_future_local(async move {
                    let outcome = match flow.save_capture_as(&capture, &path).await {
                        Ok(path) => {
                            info!(path = %path.display(), "saved an annotated image as");
                            Ok(Some(path))
                        }
                        Err(e) => {
                            warn!("annotated save-as failed: {e}");
                            Err(e.to_string())
                        }
                    };
                    done(outcome);
                });
            }),
            refresh_pins: Box::new(move |sources, capture| {
                let Some(pins) = refresher.clone() else { return };
                let paths: Vec<&std::path::Path> =
                    sources.iter().map(std::path::PathBuf::as_path).collect();
                pins.refresh(&paths, &capture);
            }),
            // `spec/10` §3.1's `PickColor`, through the same bridge every other call
            // takes. An extension that predates the method answers `Unsupported`, which
            // the editor turns into the canvas pipette rather than a fault.
            // `spec/07` §2.1's Copy Text, by the same route the card's entry takes -- the
            // editor hands over its *rendered* document, so the crop and the redactions
            // are part of what gets read.
            recognise: Box::new(move |capture, done| {
                let flow = Rc::clone(&read_flow);
                glib::spawn_future_local(async move {
                    done(flow.read_text(&capture.path, None, None).await);
                });
            }),
            pick_screen_color: Box::new(move |done| {
                let flow = Rc::clone(&pick_flow);
                glib::spawn_future_local(async move {
                    let answer = match flow.bridge().pick_color().await {
                        Ok(Some(c)) => Ok(Some(octosnap_scene::Rgba::new(c.r, c.g, c.b, 1.0))),
                        Ok(None) => Ok(None),
                        Err(e) if e.is_version_mismatch() => {
                            Err(crate::editor::actions::PickFailure::Unsupported)
                        }
                        Err(e) => Err(crate::editor::actions::PickFailure::Other(e.to_string())),
                    };
                    done(answer);
                });
            }),
            closed: self.closed_action(origin),
        }
    }

    /// Where an editor's close goes (D108): to [`Self::editor_closed`], a turn later.
    ///
    /// The turn is `hold_until_closed`'s: its handler lets go of the editor after this
    /// one runs, and until it has, the capture is still an open editor's -- which is the
    /// question [`Self::file_original`] asks before it files anything.
    fn closed_action(&self, origin: Origin) -> Box<dyn Fn(crate::editor::actions::Closed)> {
        let qao = self.weak();
        Box::new(move |closed| {
            let qao = qao.clone();
            let origin = origin.clone();
            glib::idle_add_local_once(move || {
                if let Some(qao) = qao.upgrade() {
                    qao.editor_closed(closed, origin);
                }
            });
        })
    }

    /// What a closed editor left behind (D108).
    ///
    /// × puts the capture back in the stack: as it was when nothing changed, or as the
    /// render of the document with the document parked on the card. Final Close files
    /// whatever was rendered, and the capture the editor was lent, in the history.
    fn editor_closed(self: &Rc<Self>, closed: crate::editor::actions::Closed, origin: Origin) {
        use crate::editor::actions::{Closed, Returned};
        match closed {
            Closed::Preview(Returned::Unchanged) => {
                if let Origin::Stack { capture, saved_to } = &origin {
                    self.return_to_stack(capture, saved_to.as_deref());
                }
            }
            Closed::Preview(Returned::Rendered(render)) => {
                if !self.show(&render, None) {
                    warn!("the edited GIF could not go back to the stack");
                }
                self.file_original(&origin);
            }
            Closed::Preview(Returned::Edited(document)) => {
                let render = document.render.clone();
                let saved_to = document.saved_to.clone();
                let held = Held { document, origin };
                if self.show_held(&render, saved_to.as_deref(), Some(held)).is_none() {
                    warn!("the edited capture could not go back to the stack");
                }
            }
            Closed::Final(returned) => {
                match returned {
                    Returned::Unchanged => {}
                    Returned::Rendered(render) => {
                        self.file_in_history(&render, None, CloseReason::UserClosed, false);
                    }
                    Returned::Edited(document) => self.file_in_history(
                        &document.render,
                        document.saved_to.as_deref(),
                        CloseReason::UserClosed,
                        false,
                    ),
                }
                self.file_original(&origin);
            }
        }
    }

    /// Puts a capture an editor had back in the stack as a card (D108).
    ///
    /// Out of the history first when the card that opened the editor filed it there on
    /// its way out -- with the copy it had on disk, so the card offers what it offered
    /// before -- because a capture is in exactly one place (D47).
    fn return_to_stack(self: &Rc<Self>, capture: &CaptureResult, saved_to: Option<&Path>) {
        let history = self.history.borrow().clone();
        let taken = history.as_ref().and_then(|history| {
            let id = octosnap_core::history::id_of(capture)?;
            history.get(&id)?;
            history.take(&id)
        });
        let (mut back, saved_to) = match taken {
            Some(taken) => taken,
            None if crate::recording::render::present(&capture.path) => {
                (capture.clone(), saved_to.map(Path::to_path_buf))
            }
            None => {
                warn!(path = %capture.path.display(), "the capture is gone; no card goes back");
                return;
            }
        };
        // Not a capture arriving: no fly-in to wait for, and no budget line to log.
        back.confirmed_at = None;
        back.animation_ms = 0;
        if !self.show(&back, saved_to.as_deref()) {
            warn!(path = %back.path.display(), "the capture could not go back to the stack");
        }
    }

    /// Files the capture an editor was lent, once nothing shows it any more (D108).
    ///
    /// Not when the history has it already -- a card's close filed it on the way into
    /// the editor -- and with the spool copy left in place when some other window still
    /// shows the file, for the janitor to clear later, the way an action's close does.
    fn file_original(&self, origin: &Origin) {
        let Origin::Stack { capture, saved_to } = origin else { return };
        if !crate::recording::render::present(&capture.path) {
            return;
        }
        let filed = self.history.borrow().as_ref().is_some_and(|history| {
            octosnap_core::history::id_of(capture).is_some_and(|id| history.get(&id).is_some())
        });
        if filed {
            return;
        }
        let reason = if self.held_elsewhere(&capture.path) {
            CloseReason::ActionTaken
        } else {
            CloseReason::UserClosed
        };
        self.file_in_history(capture, saved_to.as_deref(), reason, false);
    }

    /// Whether a card, a pin or an editor still shows `path`.
    fn held_elsewhere(&self, path: &Path) -> bool {
        let pinned = self.pins.borrow().as_ref().is_some_and(|pins| {
            pins.open_paths().iter().any(|open| open == path)
        });
        pinned
            || self.open_paths().iter().any(|open| open == path)
            || open_editor_paths().iter().any(|open| open == path)
    }
}

impl<B: ShellBridge + 'static> Qao<B> {
    /// Opens `spec/05`'s editor on a capture: the card's pencil and the plan's
    /// `annotate` action both land here.
    pub fn open_editor(&self, capture: &CaptureResult) {
        self.open_editor_with(capture, None);
    }

    /// [`Self::open_editor`], knowing the copy the after-capture plan wrote, so the card
    /// the editor's × leaves behind offers what the capture's own card did (D108).
    pub fn open_editor_with(&self, capture: &CaptureResult, saved_to: Option<&Path>) {
        let origin = Origin::Stack {
            capture: Box::new(capture.clone()),
            saved_to: saved_to.map(Path::to_path_buf),
        };
        self.open_editor_from(capture, origin);
    }

    /// [`Self::open_editor`] for a pin's Annotate: the pin keeps the capture, so the
    /// editor has nothing to give back but what it made (D108).
    pub fn open_editor_for_pin(&self, capture: &CaptureResult) {
        self.open_editor_from(capture, Origin::Kept);
    }

    fn open_editor_from(&self, capture: &CaptureResult, origin: Origin) {
        // A GIF's editor is its own (`gif_editor`): a preview and a trim, not a canvas.
        // Branching here rather than at each caller means the card's corner, its
        // double-click and Ctrl+E, the history strip, `annotate-last`, `annotate-file`
        // and the plan's `annotate` all reach it without knowing there are two editors.
        // By the file rather than the card kind, so a GIF opened from disk (`external`,
        // which `Kind::of` files as its own kind) is not flattened to its first frame.
        if octosnap_core::history::is_gif(&capture.path) {
            self.open_gif_editor(capture, origin);
            return;
        }
        let editor =
            crate::editor::Editor::open(&self.app, capture, self.editor_actions(origin));

        // The editor is kept alive by a handler on its own window, and nothing else.
        //
        // The overlay deliberately does not hold a list of open editors: it would have to
        // prune it, and an editor that outlived its window or a window that outlived its
        // editor are both worse than the reference being owned by the one object whose
        // lifetime already matches. GTK keeps a mapped window alive; the window keeps this
        // handler; the handler keeps the `Rc` -- and **lets go of it when the window
        // closes**. The first version held it for the closure's whole life, and the
        // closure lives as long as the window object, which the editor itself holds a
        // reference to: a cycle, so every closed editor stayed in memory with its canvas
        // and the capture's texture. `spec/05` §11 item 10 is the test that said so (D60).
        let window = editor.window().clone();
        hold_until_closed(editor, &window, capture.path.clone());
        self.close_cards_for(&capture.path);
    }

    /// Closes any card showing `path`, now that an editor has it.
    ///
    /// `spec/04` §3 is "Opens the editor with the file; **card closes**", and the card's
    /// own pencil used to be the only route that did it. Every other way in -- the
    /// after-capture plan's `annotate`, `annotate-last`, `annotate-file`, the history
    /// strip, Ctrl+E -- went round the card, so a plan of `[show-overlay, annotate]` put a
    /// card up *and* an editor over it, and closing the editor uncovered a card for a
    /// capture the user had just finished with. The rule belongs here, where every route
    /// passes, rather than at one of them.
    ///
    /// By path rather than by id, because the routes that need this do not have one: they
    /// were handed a capture, not a card. Called only once the editor's window is up, for
    /// the pencil's own reason -- an editor that failed to open behind a card that had
    /// already gone would leave the capture in the spool with no handle to it.
    fn close_cards_for(&self, path: &std::path::Path) {
        let ids: Vec<CardId> = self
            .entries
            .borrow()
            .iter()
            .filter(|(_, entry)| entry.card.capture.path == path)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            debug!(id, "closing the card an editor has taken over");
            self.close(id, CloseReason::ActionTaken);
        }
    }

    /// `spec/04` §1's Trim on a GIF card: the GIF editor, with the same three routes out
    /// an annotated image has. A trim is rendered to `<stem>-trim.gif` beside the
    /// original, the way the canvas exports `-annotated.png`, and handed to the flow as
    /// the capture's file, so the filename template and `last saved` hold; like that
    /// export it is derived rather than a capture, and the janitor clears it after a day.
    fn open_gif_editor(&self, capture: &CaptureResult, origin: Origin) {
        let copy_flow = Rc::clone(&self.flow);
        let copy_app = self.app.clone();
        let save_flow = Rc::clone(&self.flow);
        let as_path_flow = Rc::clone(&self.flow);
        let actions = crate::gif_editor::GifEditorActions {
            copy: Box::new(move |capture, done| {
                let flow = Rc::clone(&copy_flow);
                let app = copy_app.clone();
                glib::spawn_future_local(async move {
                    let outcome = match flow.copy(&capture).await {
                        Ok(()) => {
                            info!("copied a GIF from its editor");
                            Ok(None)
                        }
                        Err(e) => {
                            warn!("GIF copy failed: {e}");
                            notify::action_failed(&app, "Copy", &e.to_string());
                            Err(e.to_string())
                        }
                    };
                    done(outcome);
                });
            }),
            save: Box::new(move |capture, done| {
                let flow = Rc::clone(&save_flow);
                glib::spawn_future_local(async move {
                    let outcome = match flow.save_capture(&capture).await {
                        Ok(path) => {
                            info!(path = %path.display(), "saved a GIF from its editor");
                            Ok(Some(path))
                        }
                        Err(e) => {
                            warn!("GIF save failed: {e}");
                            Err(e.to_string())
                        }
                    };
                    done(outcome);
                });
            }),
            save_as_path: Box::new(move |capture, path, done| {
                let flow = Rc::clone(&as_path_flow);
                glib::spawn_future_local(async move {
                    let outcome = match flow.save_capture_as(&capture, &path).await {
                        Ok(path) => Ok(Some(path)),
                        Err(e) => {
                            warn!("GIF save-as failed: {e}");
                            Err(e.to_string())
                        }
                    };
                    done(outcome);
                });
            }),
            closed: self.closed_action(origin),
        };
        match crate::gif_editor::GifEditor::open(&self.app, capture, actions) {
            Ok(editor) => {
                let window = editor.window().clone();
                hold_until_closed(editor, &window, capture.path.clone());
                self.close_cards_for(&capture.path);
            }
            Err(e) => {
                warn!(path = %capture.path.display(), "could not open the GIF editor: {e}");
                notify::action_failed(&self.app, "Trim", &e);
            }
        }
    }

    /// Opens a document a closed editor parked on a card, where it left off (D108).
    fn reopen(&self, held: Held) {
        let Held { document, origin } = held;
        let path = document.capture.path.clone();
        let editor =
            crate::editor::Editor::reopen(&self.app, *document, self.editor_actions(origin));
        let window = editor.window().clone();
        hold_until_closed(editor, &window, path);
    }

    /// `spec/05` §8: opens a `.octosnap` project with its objects editable.
    ///
    /// Through the overlay rather than from `actions.rs` directly, because opening an
    /// editor needs [`Self::editor_actions`] -- the four closures the bottom bar is built
    /// from -- and those are made from the capture flow this owns. The same reason a
    /// card's Annotate button goes through here.
    ///
    /// The base image is extracted **beside the project file's own spool entry**, not into
    /// the project's directory: `spec/05` §8 makes the container self-contained precisely
    /// so that opening one does not write next to the user's file.
    pub fn open_project(self: &Rc<Self>, path: &std::path::Path) -> bool {
        let into = glib::user_cache_dir()
            .join("octosnap")
            .join("projects")
            .join(path.file_stem().unwrap_or_default());
        let opened = match octosnap_scene::project::read(path, &into) {
            Ok(opened) => opened,
            Err(e) => {
                warn!(path = %path.display(), "could not open the project: {e}");
                notify::action_failed(&self.app, "Open project", &e.to_string());
                return false;
            }
        };

        // A capture describing the *extracted* base, so the bottom bar's filename template,
        // clipboard route and pin position all work as they do for any other document.
        #[allow(clippy::cast_possible_truncation)]
        let rect = octosnap_core::Rect {
            x: 0,
            y: 0,
            width: (opened.manifest.canvas.width / opened.manifest.scale.max(f64::EPSILON))
                .round() as i32,
            height: (opened.manifest.canvas.height / opened.manifest.scale.max(f64::EPSILON))
                .round() as i32,
        };
        let capture = octosnap_core::CaptureResult {
            path: opened.base.clone(),
            meta_path: opened.base.with_extension("json"),
            mode: octosnap_core::capture::CaptureMode::Area,
            rect,
            scale: opened.manifest.scale,
            display: String::new(),
            cursor_rect: None,
            source_window: octosnap_core::capture::SourceWindow::default(),
            // A project's base carries whatever alpha the capture had -- §8 stores the
            // "original capture (alpha preserved)" -- and the crop that made the canvas
            // bigger than it is exactly the case that needs the flag.
            window_alpha: opened.manifest.canvas != opened.scene.base.bounds(),
            timestamp: opened.manifest.created.saturating_mul(1_000_000),
            confirmed_at: None,
            animation_ms: 0,
            requested_action: None,
            modifiers: 0,
            external: false,
            linebreaks: None,
            duration_ms: None,
        };

        info!(
            path = %path.display(),
            objects = opened.scene.len(),
            thumbnail = opened.has_thumbnail,
            "project opened"
        );
        let editor = crate::editor::Editor::open_with(
            &self.app,
            &capture,
            self.editor_actions(Origin::Kept),
            Some(opened.scene),
        );
        let window = editor.window().clone();
        hold_until_closed(editor, &window, capture.path.clone());
        true
    }
}

/// Keeps an editor alive exactly as long as its window, and not a moment longer.
///
/// The `Rc` sits in a handler on the window's `close-request` and is dropped by that
/// handler. **Not** `destroy`: GTK 4 emits `destroy` from the widget's dispose, which
/// only runs once every reference is gone -- and the editor holds one, so a handler that
/// waited for `destroy` to release the editor would wait for itself (D60). `close-request`
/// is the signal for "this window is finished": Ctrl+W, the title bar's button and the
/// `close` action all arrive through it, and the editor's own handler on it, connected
/// first, has already saved the window size.
///
/// Everything else that refers back to the editor -- every button, every shortcut, the
/// canvas's listeners -- holds a `Weak`, so this is the last strong reference and the
/// editor goes with it; `Editor::drop` logs the moment.
/// Generic over the editor, because there are two (`spec/05`'s canvas and the GIF
/// editor) and the janitor's question -- which files are on screen -- is the same for both.
fn hold_until_closed<T: 'static>(held: Rc<T>, window: &adw::ApplicationWindow, showing: std::path::PathBuf) {
    let any: Rc<dyn std::any::Any> = held.clone();
    EDITORS.with(|editors| editors.borrow_mut().push((Rc::downgrade(&any), showing)));
    let held = RefCell::new(Some(held));
    window.connect_close_request(move |_| {
        let released = held.borrow_mut().take().is_some();
        debug!(released, "editor window closing");
        glib::Propagation::Proceed
    });
}

thread_local! {
    /// The open editors and the file each shows, weakly: `hold_until_closed` owns them,
    /// this only answers the history janitor's question. Pruned on every read.
    static EDITORS: RefCell<Vec<(std::rc::Weak<dyn std::any::Any>, std::path::PathBuf)>> =
        const { RefCell::new(Vec::new()) };
}

/// The capture files open editors are showing, for the history janitor.
#[must_use]
pub fn open_editor_paths() -> Vec<std::path::PathBuf> {
    EDITORS.with(|editors| {
        let mut editors = editors.borrow_mut();
        editors.retain(|(editor, _)| editor.strong_count() > 0);
        editors.iter().map(|(_, path)| path.clone()).collect()
    })
}

/// The card's own view of its owner (`spec/04` §3's behaviour table).
impl<B: ShellBridge + 'static> CardHost for Qao<B> {
    /// `spec/04` §3: "Copies image + file URI; card closes with a 'copied' tick". The image
    /// or the file, as D165 has it: a screenshot's pixels, a GIF's file.
    ///
    /// The tick, and the closing, wait for the clipboard to actually take the image.
    ///
    /// This used to close the card the moment the button was pressed and let the copy
    /// resolve behind it, which put the two in exactly the wrong order: on a copy that
    /// failed the user got a card that vanished and an empty clipboard, with the reason
    /// in a journal they are not reading. Reported from hardware on 2026-09-09 as "the
    /// card preview just disappears". The clipboard turned out to be working -- 283530
    /// bytes of `image/png`, measured with `wl-paste` -- so what was actually missing was
    /// the feedback. Both halves are here: the tick says it worked, and a failure now
    /// **keeps the card**, because a capture is not something to throw away on the
    /// strength of an action that did not happen. `qao-ask-destination` already worked
    /// this way, and the pin's Save already recorded success rather than the click.
    fn copy(&self, id: CardId) {
        // Read now, as the drop reads it: the pill's click, the menu's item and
        // Ctrl+Alt+C all come here with Alt down or not (D137).
        self.copy_then(id, card::alt_held());
    }

    fn preview(&self, id: CardId) {
        let Some(path) = self
            .entries
            .borrow()
            .get(&id)
            .map(|e| e.saved_to.clone().unwrap_or_else(|| e.card.capture.path.clone()))
        else {
            return;
        };
        info!(?id, path = %path.display(), "previewing the card in the image viewer");
        let uri = gio::File::for_path(&path).uri();
        if let Err(e) = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>) {
            warn!("could not open the capture in the image viewer: {e}");
            notify::action_failed(&self.app, "Preview", &e.to_string());
        }
    }

    /// `spec/04` §4's "Open in Text recognition (OCR)", and `spec/07` §2.1's card source.
    ///
    /// The card stays where it is. A read is not an after-capture action -- the capture is
    /// already on screen and already the user's -- so this only puts its text on the
    /// clipboard and notifies, exactly as a text capture does.
    fn recognise(&self, id: CardId) {
        let Some(capture) = self.capture_of(id) else { return };
        let flow = Rc::clone(&self.flow);
        let app = self.app.clone();
        glib::spawn_future_local(async move {
            let outcome = flow.read_text(&capture.path, None, None).await;
            notify::capture_outcome(
                &app,
                &outcome,
                crate::settings::Settings::load().notifications_enabled(),
            );
        });
    }

    fn save_as(&self, id: CardId) {
        self.choose_and_save(id);
    }

    fn save(&self, id: CardId) {
        if self.config.borrow().ask_destination {
            self.choose_and_save(id);
            return;
        }
        let Some(capture) = self.capture_of(id) else { return };
        let flow = Rc::clone(&self.flow);
        // A recording whose GIF is still frames: the card stays, its badge counting the
        // render, until the file is written and saved -- gone at once, it would leave a
        // minute with nothing on screen to say a GIF is on its way (D113).
        let writes_first = crate::recording::render::pending(&capture.path);
        if writes_first && let Some(card) = self.card(id) {
            card.follow_render();
        }

        let app = self.app.clone();
        if writes_first {
            let qao = self.weak();
            glib::spawn_future_local(async move {
                match flow.save_capture(&capture).await {
                    Ok(path) => {
                        info!(path = %path.display(), "card saved");
                        if let Some(qao) = qao.upgrade() {
                            qao.close_saved(id);
                        }
                    }
                    Err(e) => {
                        warn!("card save failed: {e}");
                        notify::action_failed(&app, "Save", &e.to_string());
                    }
                }
            });
            return;
        }
        glib::spawn_future_local(async move {
            match flow.save_capture(&capture).await {
                Ok(path) => info!(path = %path.display(), "card saved"),
                Err(e) => {
                    warn!("card save failed: {e}");
                    notify::action_failed(&app, "Save", &e.to_string());
                }
            }
        });
        self.close(id, CloseReason::ActionTaken);
    }

    /// `spec/04` §3: "Annotate (Ctrl+E) | Opens the editor with the file; card closes."
    ///
    /// The close is `open_editor`'s: this card shows the capture the editor is opening, so
    /// `close_cards_for` takes it -- and takes it only once the editor's window is up.
    ///
    /// A card an editor left behind opens its document instead (D108): the objects are
    /// still objects and Ctrl+Z still undoes, because the card's picture was only ever a
    /// render of them.
    fn annotate(&self, id: CardId) {
        let held = self.entries.borrow_mut().get_mut(&id).and_then(|entry| entry.held.take());
        if let Some(held) = held {
            debug!(id, "annotate: the card's document goes back into an editor");
            let render = held.document.render.clone();
            self.reopen(held);
            // The render is superseded by the document it was made from. Its twin goes,
            // so the janitor treats the picture as derived and clears it in a day rather
            // than filing it: a drop target may still be reading it.
            if let Err(e) = std::fs::remove_file(&render.meta_path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                warn!(path = %render.meta_path.display(), "could not retire the render: {e}");
            }
            self.close(id, CloseReason::Reopened);
            return;
        }
        let Some((capture, saved_to)) = self
            .entries
            .borrow()
            .get(&id)
            .map(|entry| (entry.card.capture.clone(), entry.saved_to.clone()))
        else {
            return;
        };
        debug!(id, "annotate");
        self.open_editor_with(&capture, saved_to.as_deref());
    }

    // The controls whose feature has not landed. They say so rather than doing nothing: a
    // button that only writes to the journal is indistinguishable from a broken one, and
    // the card stays open so nothing is lost either way.
    /// `spec/04` §3: "Pin | Creates a pinned window at the capture location; card closes."
    ///
    /// The card's saved state goes with the capture, because the pin can hand it back
    /// (`docs/decisions.md` D47) and the card that comes back should be this card.
    fn pin(&self, id: CardId) {
        let Some((capture, saved_to)) = self
            .entries
            .borrow()
            .get(&id)
            .map(|entry| (entry.card.capture.clone(), entry.saved_to.clone()))
        else {
            return;
        };
        let Some(pins) = self.pins.borrow().clone() else {
            notify::unavailable(&self.app, "Pin to the Screen", "pinned screenshots");
            return;
        };
        debug!(id, "pin");
        if pins.pin(&capture, saved_to.as_deref()) {
            self.close(id, CloseReason::Pinned);
        }
    }

    /// `spec/04` §3: "Moves the saved file to the trash after a 3 s undo toast."
    ///
    /// The toast **is** the grace period: nothing is deleted while it is up. The obvious
    /// alternative -- trash immediately, restore on undo -- cannot be built, because GIO
    /// can put a file in the XDG trash and offers no way to take it back out, so "undo"
    /// would mean telling the user to go and find it in Files. Three seconds of doing
    /// nothing makes the undo exact.
    ///
    /// The toast itself is a notification with a button rather than an `AdwToast`, which
    /// needs a window the app does not have while a card is up. That is not a compromise:
    /// `notify.rs` already routes notification buttons back to GApplication actions, the
    /// notification is the platform's toast for an app with no window on screen, and it
    /// survives the card closing -- which an in-card toast could not, since the card is
    /// the thing being dismissed.
    fn trash(&self, id: CardId) {
        let Some((capture, saved_to)) = self
            .entries
            .borrow()
            .get(&id)
            .map(|entry| (entry.card.capture.clone(), entry.saved_to.clone()))
        else {
            return;
        };
        let name = saved_to
            .as_deref()
            .unwrap_or(capture.path.as_path())
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "the screenshot".to_owned());

        let token = self.pending_trash.borrow_mut().push(Doomed { capture, saved_to });
        debug!(id, token, "trash");
        notify::trashed(&self.app, token, &name);

        // No `SourceId` to keep and none to cancel, which is deliberate: `SourceId::remove`
        // panics on a source that has already fired, from inside a GLib callback, and that
        // aborts rather than unwinds -- it is what took the whole app down in D32. So the
        // timeout is left to fire and the *queue* decides whether anything is still there
        // to act on. Undo removes the entry; the timeout then finds nothing and returns.
        let qao = self.weak();
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(u64::from(TRASH_GRACE_MS)),
            move || {
                if let Some(qao) = qao.upgrade() {
                    qao.carry_out_trash(token);
                }
            },
        );

        // `CloseReason::Trashed`, not `UserClosed`: the capture must not also go to
        // "Restore recently closed", or one capture would have two ways back and the
        // second would hand out a card for a file already in the bin.
        self.close(id, CloseReason::Trashed);
    }


    fn close(&self, id: CardId, reason: CloseReason) {
        // Deferred by one turn so this can be called from inside a card's own signal
        // handler -- a button click, a gesture -- without destroying the widget the
        // handler is still returning through.
        if !self.entries.borrow().contains_key(&id) {
            return;
        }
        let qao = self.weak();
        glib::idle_add_local_once(move || {
            if let Some(qao) = qao.upgrade() {
                qao.dismiss(id, reason);
            }
        });
    }

    fn hovered(&self, id: CardId, entered: bool) {
        debug!(id, entered, "card hover");
        let Some(path) = self
            .entries
            .borrow()
            .get(&id)
            .and_then(|entry| entry.card.object_path())
        else {
            return;
        };

        // `spec/04` §3's focus policy. Only the compositor can do this: a Wayland client
        // cannot focus itself, and only the compositor knows what had focus before.
        if entered {
            self.focused.set(Some(id));
        } else if self.focused.get() == Some(id) {
            self.focused.set(None);
        } else {
            // The pointer left a card that was not the one holding the keyboard, which
            // happens when it crossed straight onto a neighbour. Restoring here would
            // take focus off the card the pointer is now over.
            return;
        }

        let flow = Rc::clone(&self.flow);
        let qao = self.weak();
        glib::spawn_future_local(async move {
            let moved = match flow.bridge().focus_window(&path, entered).await {
                Ok(moved) => moved,
                Err(e) => {
                    debug!("could not move focus for a card: {e}");
                    false
                }
            };
            // The ring follows what actually happened. Leaving it on a card whose focus
            // request failed would promise a keyboard the card has not got, and Ctrl+C
            // would then do nothing while the card looked ready for it.
            if let Some(qao) = qao.upgrade()
                && let Some(entry) = qao.entries.borrow().get(&id)
            {
                entry.card.set_focus_ring(entered && moved);
            }
        });
    }

    fn hide_all(&self) {
        self.toggle_hidden();
    }

    fn menu(&self, id: CardId, open: bool) {
        debug!(id, open, "card menu");
    }

    /// `spec/04` §3: "on successful drop, close the card if 'delete after dragging' is
    /// on. Alt-drag (Pin cards) keeps the source."
    fn dropped(&self, id: CardId, keep: bool) {
        let close = self.config.borrow().close_after_drag && !keep;
        info!(id, keep, close, "card dropped");
        if close {
            self.close(id, CloseReason::ActionTaken);
        }
    }

    fn ready(&self, id: CardId) {
        debug!(id, "card surface configured");
        if let Some(qao) = self.weak().upgrade() {
            qao.reflow();
        }
    }
}

/// `spec/04` §8: "Stack shift | 200 ms | ease-out-cubic | y translation of siblings".
const STACK_SHIFT_MS: u32 = 200;

/// The `PlaceWindow` request for a card at `offset` up the stack.
///
/// Split out from [`Qao::reflow`] because everything interesting about placement is in
/// here and none of it needs a window: `spec/10` §11's "drive the app through capture ->
/// card -> actions without a shell" can then check that a stack of three cards does not
/// overlap, against the `NullBridge`, with no display anywhere.
#[must_use]
pub fn placement_for(offset: i32, card: Size, config: QaoConfig) -> Placement {
    Placement::Stacked {
        edge: config.edge,
        offset,
        size: Size::new(card.width + 2 * SHADOW_MARGIN, card.height + 2 * SHADOW_MARGIN),
        // The window is bigger than the card by this band on every side; telling the
        // extension means the *card* lands at `spec/04` §2's measured 16 pt margin
        // rather than the window doing so and pushing the card inward.
        inset: SHADOW_MARGIN,
        monitor: if config.follow_pointer {
            // `spec/04` §10 item 1: "in the configured corner on the pointer's monitor".
            PlacementMonitor::Pointer
        } else {
            PlacementMonitor::Primary
        },
    }
}

/// How much of the extension's fly animation is still to run.
///
/// The extension starts the animation and notifies the app on the same turn
/// (`extension/src/flow.ts`), stamping `timestamp` at that moment, so the card knows both
/// when the flight began and how long it lasts. Subtracting the time the app has since
/// spent copying and saving is what keeps the card's fade-in tied to the capture landing
/// rather than to how slow the clipboard happened to be.
fn remaining_flight(capture: &CaptureResult) -> u32 {
    #[allow(clippy::cast_sign_loss)]
    let now_us = glib::real_time().max(0) as u64;
    let elapsed_ms = now_us.saturating_sub(capture.timestamp) / 1000;
    u32::try_from(u64::from(capture.animation_ms).saturating_sub(elapsed_ms)).unwrap_or(0)
}

/// The card the capture files as (`spec/04` §1). Only screenshots and GIFs reach the
/// overlay in M5; a video's card is M9's, but the mapping is written whole so it needs no
/// revisiting then.
/// What [`Qao::show_and`] has the card do once it is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardThen {
    Copy,
    SaveAs,
}

/// A save chooser's title, by what is being saved.
pub fn save_title(capture: &CaptureResult) -> &'static str {
    match card_kind(capture) {
        CardKind::Image => "Save Screenshot",
        CardKind::Gif => "Save GIF",
        CardKind::Video => "Save Video",
    }
}

fn card_kind(capture: &CaptureResult) -> CardKind {
    match octosnap_core::history::Kind::of(capture) {
        octosnap_core::history::Kind::Gif => CardKind::Gif,
        octosnap_core::history::Kind::Video => CardKind::Video,
        // A GIF opened from disk (`import`) is a GIF card too: the badge, Trim, no Pin.
        octosnap_core::history::Kind::External if octosnap_core::history::is_gif(&capture.path) => {
            CardKind::Gif
        }
        _ => CardKind::Image,
    }
}

fn install_style(app: &adw::Application) {
    let _ = app;
    let Some(display) = gdk::Display::default() else {
        warn!("no display; the overlay stylesheet was not installed");
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&style::sheet());
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use octosnap_core::Monitor;
    use octosnap_core::qao::card_size;
    use octosnap_shell::NullBridge;

    use super::*;

    /// A fresh context per test, matching `flow.rs`: the default one is shared, and a
    /// test that blocks on it can deadlock against another test's pending work.
    fn run<F: std::future::Future>(future: F) -> F::Output {
        glib::MainContext::new().block_on(future)
    }

    fn monitor(width: i32, height: i32, panel: i32) -> Monitor {
        Monitor {
            index: 0,
            connector: "eDP-1".to_owned(),
            geometry: Rect::new(0, 0, width, height),
            work_area: Rect::new(0, panel, width, height - panel),
            scale: 1.0,
            geometry_scale: 1,
            primary: true,
            current: true,
            refresh: None,
        }
    }

    fn window_size(card: Size) -> Size {
        Size::new(card.width + 2 * SHADOW_MARGIN, card.height + 2 * SHADOW_MARGIN)
    }

    /// `spec/04` §10 item 5: "Three captures stack correctly". Driven through the same
    /// `place_window` the real extension answers, with no display and no compositor.
    #[test]
    fn three_stacked_cards_do_not_overlap() {
        let bridge = NullBridge::default().with_monitors(vec![monitor(1920, 1200, 32)]);
        let config = QaoConfig::default();

        // Three cards. They are the same size now (D43), so the stack's stride is
        // constant -- but the offsets are still computed from heights rather than from an
        // index, because a video card may not be, and because `PlaceWindow` takes a
        // distance for that reason.
        let cards: Vec<Size> = (0..3).map(|_| card_size(config.size_step)).collect();

        let mut stack = Stack::new();
        for (index, size) in cards.iter().enumerate() {
            let id = index as CardId + 1;
            bridge.set_window_size(&format!("/w/{id}"), window_size(*size));
            stack.push_newest(Slot { id, height: size.height });
        }

        let landed: Vec<Rect> = run(async {
            let mut out = Vec::new();
            for (id, offset) in stack.placements() {
                let card = cards[usize::try_from(id).expect("small id") - 1];
                let rect = bridge
                    .place_window(&format!("/w/{id}"), "qao", &placement_for(offset, card, config), 0)
                    .await
                    .expect("the null bridge always places");
                out.push(rect);
            }
            out
        });

        // Card rects, not window rects: the windows overlap by design (their shadow
        // bands do), and the property that matters is what the user sees.
        let visible: Vec<Rect> = landed.iter().map(|r| card::card_rect(*r)).collect();

        for pair in visible.windows(2) {
            let (lower, upper) = (pair[0], pair[1]);
            let gap = lower.y - (upper.y + upper.height);
            assert_eq!(gap, octosnap_core::qao::GAP, "cards must clear each other by one gap");
        }

        // And the newest sits at the measured margin from the bottom-left corner.
        let work = monitor(1920, 1200, 32).work_area;
        assert_eq!(visible[0].x, work.x + octosnap_core::qao::MARGIN);
        assert_eq!(
            visible[0].y + visible[0].height,
            work.y + work.height - octosnap_core::qao::MARGIN
        );
    }

    /// Closing the middle card must close the gap, not leave a hole (`spec/04` §10 item 5).
    #[test]
    fn closing_the_middle_card_moves_the_one_above_it_down() {
        let bridge = NullBridge::default().with_monitors(vec![monitor(1920, 1200, 32)]);
        let config = QaoConfig::default();
        let card = card_size(config.size_step);

        let mut stack = Stack::new();
        for id in 1..=3 {
            bridge.set_window_size(&format!("/w/{id}"), window_size(card));
            stack.push_newest(Slot { id, height: card.height });
        }

        let place = |stack: &Stack, id: CardId| -> Rect {
            let offset = stack
                .placements()
                .into_iter()
                .find(|(candidate, _)| *candidate == id)
                .map(|(_, offset)| offset)
                .expect("the card is in the stack");
            run(bridge.place_window(&format!("/w/{id}"), "qao", &placement_for(offset, card, config), 0))
                .expect("the null bridge always places")
        };

        // Stack is [3, 2, 1] newest first, so card 1 is the top one.
        let before = place(&stack, 1);
        assert!(stack.remove(2));
        let after = place(&stack, 1);

        assert_eq!(
            after.y - before.y,
            card.height + octosnap_core::qao::GAP,
            "card 1 should fall by exactly the card that left"
        );
    }

    /// The right edge is the same layout mirrored, not a different one.
    #[test]
    fn the_right_edge_mirrors_the_left() {
        let bridge = NullBridge::default().with_monitors(vec![monitor(1920, 1200, 32)]);
        let card = card_size(QaoConfig::default().size_step);
        bridge.set_window_size("/w/1", window_size(card));

        let left = run(bridge.place_window(
            "/w/1",
            "qao",
            &placement_for(0, card, QaoConfig { edge: Edge::Left, ..QaoConfig::default() }),
            0,
        ))
        .expect("placed");
        let right = run(bridge.place_window(
            "/w/1",
            "qao",
            &placement_for(0, card, QaoConfig { edge: Edge::Right, ..QaoConfig::default() }),
            0,
        ))
        .expect("placed");

        assert_eq!(left.y, right.y, "only the side changes");
        let work = monitor(1920, 1200, 32).work_area;
        assert_eq!(
            card::card_rect(right).x + card.width,
            work.x + work.width - octosnap_core::qao::MARGIN
        );
    }

    /// The inset is not decoration: without it the card would land 16 pt further in than
    /// `spec/04` §2 measured, and every gap would be 48 pt instead of 8.
    /// `spec/04` §5's stack shift, and the one card it must not apply to.
    ///
    /// Worth a test because the failure is invisible in both directions: a missing slide
    /// just looks like a jump, and a slide on a first placement sends the card travelling
    /// from wherever Mutter happened to map it -- usually the middle of the screen.
    #[test]
    fn only_a_card_that_has_been_placed_before_slides() {
        let bridge = NullBridge::default().with_monitors(vec![monitor(1920, 1200, 32)]);
        let config = QaoConfig::default();
        let card = card_size(config.size_step);
        let placement = placement_for(0, card, config);

        run(bridge.place_window("/w/new", "qao", &placement, 0)).expect("placed");
        run(bridge.place_window("/w/old", "qao", &placement, STACK_SHIFT_MS)).expect("placed");

        assert_eq!(
            bridge.animation_calls(),
            vec![("/w/new".to_owned(), 0), ("/w/old".to_owned(), STACK_SHIFT_MS)]
        );
        // `spec/04` §8's figure, not one this file invented.
        assert_eq!(STACK_SHIFT_MS, 200);
    }

    #[test]
    fn the_shadow_band_is_declared_to_the_extension() {
        match placement_for(0, Size::new(207, 129), QaoConfig::default()) {
            Placement::Stacked { inset, .. } => assert_eq!(inset, SHADOW_MARGIN),
            other => panic!("a card is always stacked, got {other:?}"),
        }
    }

    #[test]
    fn following_the_pointer_is_a_setting_the_extension_is_told_about() {
        let card = Size::new(207, 129);
        let following = placement_for(0, card, QaoConfig { follow_pointer: true, ..QaoConfig::default() });
        let fixed = placement_for(0, card, QaoConfig { follow_pointer: false, ..QaoConfig::default() });
        match (following, fixed) {
            (
                Placement::Stacked { monitor: a, .. },
                Placement::Stacked { monitor: b, .. },
            ) => {
                assert_eq!(a, PlacementMonitor::Pointer);
                assert_eq!(b, PlacementMonitor::Primary);
            }
            other => panic!("a card is always stacked, got {other:?}"),
        }
    }
}
