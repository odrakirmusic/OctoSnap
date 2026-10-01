// SPDX-License-Identifier: GPL-3.0-or-later

//! One Quick Access Overlay card: a transparent window holding a thumbnail
//! (`spec/04` §1, §9).
//!
//! The window is never `present()`ed, which is the whole point of the thing. `spec/04`'s
//! first principle is "it never steals focus": a card that took focus would interrupt
//! whatever the user was typing at the moment their screenshot finished, which is exactly
//! the moment they are least expecting it. `docs/spikes/01-02` measured that
//! `set_visible(true)` plus the extension's `make_above` leaves `focus_changed: false`.
//!
//! What this file does *not* decide: where the card goes (the extension, via
//! `PlaceWindow`), how big it is (`core::qao::card_size`), or what its buttons do (the
//! host). It builds a window and reports what the user did to it.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use adw::prelude::*;
use octosnap_core::qao::{
    self, AutoClose, CardId, CardKind, CloseTimer, Controls, EditAction, Edge, Holds,
    PrimaryAction, Size,
};
use octosnap_core::{CaptureResult, Rect};
use tracing::{debug, info, warn};

/// Marks the card the extension has lent the keyboard to (`spec/04` §3).
const FOCUS_CLASS: &str = "octosnap-focused";

/// `spec/04` §8: hover controls fade in over 100 ms.
const HOVER_MS: u32 = 100;

/// `spec/04` §8: arrival is 180 ms, ease-out-cubic.
const ARRIVE_MS: u32 = 180;

/// `spec/04` §8: close is 180 ms, ease-in-quad.
const CLOSE_MS: u32 = 180;

/// `spec/04` §8's "250 ms then close" for the copy tick.
const COPY_TICK_MS: u32 = 250;

/// `spec/00` §9's budget: capture to card.
const BUDGET_MS: u64 = 300;

/// How often the auto-close timer is sampled. Ten times a second: fine enough for the
/// hairline to look continuous, coarse enough that a screenful of cards is not a
/// wakeup source. The tick is fixed rather than derived from the interval so that a
/// 600 s card and a 5 s card animate their hairlines at the same smoothness.
const TICK_MS: u32 = 100;

/// How often a card following a recording's render reads its progress (D113): the export
/// bar's own rate, fine enough to look continuous.
const RENDER_TICK: std::time::Duration = std::time::Duration::from_millis(100);

/// Why a card went away, which decides what happens to its file (`spec/04` §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// The × , Ctrl+W, or middle-click.
    UserClosed,
    /// The interval in `spec/04` §6 ran out.
    Timeout,
    /// An action that closes the card behind it: Copy, Save, Annotate, a successful drag.
    ActionTaken,
    /// Pin took the capture to the screen (`spec/07` §3).
    ///
    /// Its own reason and not `ActionTaken`, because this is the one close that does not
    /// end the capture's time on screen -- the pin holds it now and hands it back when it
    /// closes, so the overlay must not also file it in history (`docs/decisions.md` D47).
    Pinned,
    /// The stack ran out of room and this was the oldest (`Stack::evict_for`).
    Evicted,
    /// "Close all".
    CloseAll,
    /// Trash took the capture, inside `spec/04` §3's 3 s undo window.
    ///
    /// Its own reason for `Pinned`'s reason turned inside out: the capture is on its way
    /// *out*, and the undo toast is the one way back. Filing it in history as well would
    /// give one capture two ways back, and "Restore recently closed" would hand out a
    /// card for a file already in the bin.
    Trashed,
    /// Annotate took the document a closed editor left on this card back into an editor
    /// (D108). The card's picture is a render of that document, superseded the moment the
    /// document is open again -- the editor's next close leaves a new one -- so it is not
    /// a capture anyone will look for in the history.
    Reopened,
}

impl CloseReason {
    /// Whether the user left the card without doing anything with its capture: closed it,
    /// or let it time out or be pushed off the stack. What a recording's plan held for
    /// its card runs then; anything the user did with it took its place (D113).
    #[must_use]
    pub const fn leaves_it_untouched(self) -> bool {
        matches!(self, Self::UserClosed | Self::Timeout | Self::Evicted | Self::CloseAll)
    }

    /// Whether the card should animate on its way out.
    ///
    /// "Close all" and eviction do not: six cards each running their own 180 ms slide is
    /// a scattering effect rather than a dismissal, and the evicted card is leaving to
    /// make room for one that is arriving in the same frame.
    #[must_use]
    pub const fn animates(self) -> bool {
        matches!(
            self,
            Self::UserClosed
                | Self::Timeout
                | Self::ActionTaken
                | Self::Pinned
                | Self::Trashed
                | Self::Reopened
        )
    }

    /// Whether the capture goes to history when the card goes (`spec/04` §7).
    ///
    /// Written as a question the reason answers rather than as a comparison at the one
    /// call site: `spec/04` §7 says "any reason", so every exception has to be visible
    /// from the enum rather than buried in the overlay's dismissal path.
    ///
    /// Both exceptions are the same rule seen from opposite ends -- the capture already
    /// has exactly one way back, and history would be a second. A pin holds it and gives
    /// it back when it closes (`docs/decisions.md` D47); a trashed capture is held by its
    /// undo toast for three seconds and then gone (D50).
    #[must_use]
    pub const fn files_in_history(self) -> bool {
        !matches!(self, Self::Pinned | Self::Trashed | Self::Reopened)
    }
}

/// What a card asks of whatever owns it.
///
/// A trait rather than callbacks on the struct because every one of these needs the
/// *stack*, not the card: copying closes the card, which reflows its neighbours, which
/// the card cannot do to itself. Keeping it abstract also keeps the card free of the
/// bridge's type parameter.
pub trait CardHost {
    fn copy(&self, id: CardId);
    /// `spec/04` §4's "Open in Text recognition (OCR)", which `spec/07` §2.1 names as one
    /// of the file sources: read this card's PNG and copy what it says.
    fn recognise(&self, id: CardId);
    fn save(&self, id: CardId);
    /// The context menu's Save As…, which always asks where, whatever "Ask where to save"
    /// says. The item used to run `save`, which with that setting off -- its default --
    /// saved without asking (D136).
    fn save_as(&self, id: CardId);
    /// `spec/04` §3's Space on a still: the capture in the desktop's image viewer (D137).
    fn preview(&self, id: CardId);
    fn annotate(&self, id: CardId);
    fn pin(&self, id: CardId);
    fn trash(&self, id: CardId);
    fn close(&self, id: CardId, reason: CloseReason);
    /// The pointer entered or left. Drives both the focus policy and the timer hold.
    fn hovered(&self, id: CardId, entered: bool);
    /// `spec/04` §3: scroll down over a card hides every card until the next capture.
    fn hide_all(&self);
    /// A context menu opened or closed, which holds the timer (`spec/04` §6).
    fn menu(&self, id: CardId, open: bool);
    /// The card's surface has been configured, so the extension can place it.
    fn ready(&self, id: CardId);
    /// A drag out of this card finished with a successful drop. `keep` is true when the
    /// user held Alt (`spec/04` §3: "Alt-drag (Pin cards) keeps the source").
    fn dropped(&self, id: CardId, keep: bool);
}

pub struct Card {
    pub id: CardId,
    pub capture: CaptureResult,
    window: gtk::ApplicationWindow,
    body: gtk::Widget,
    scrim: gtk::Widget,
    hairline: gtk::Widget,
    /// `spec/04` §8's copy tick. Named `copied` and not `tick`, because `tick` on this
    /// struct is already the auto-close timer's GLib source and two of them would be a
    /// trap for the next reader.
    copied: gtk::Widget,
    /// The card without its shadow band -- what the user sees and what the stack measures.
    card_size: Size,
    /// Which six controls this card offers. Kept because the centre pill is Save on one
    /// card and Trash on another, and the button that was built has to dispatch to the
    /// action it was labelled with.
    controls: Controls,
    timer: RefCell<CloseTimer>,
    tick: RefCell<Option<glib::SourceId>>,
    hover_animation: RefCell<Option<adw::TimedAnimation>>,
    /// The arrival or close animation, held for its lifetime.
    ///
    /// Held, not leaked. `std::mem::forget` on a playing animation looks harmless -- it is
    /// one small object and libadwaita keeps it alive anyway -- but it is one per capture,
    /// forever, and `spec/11` M7's leak pass would find a pile of them with no way to tell
    /// which card each belonged to. Owning it here means it dies with its card.
    transition: RefCell<Option<adw::TimedAnimation>>,
    /// Set once the card is on its way out, so a timer tick that fires during the close
    /// animation cannot ask the host to close it a second time.
    closing: Cell<bool>,
    /// How long the card held its arrival for the fly to land, for the budget line: a
    /// card that held was ready first, and one that did not arrived after the fly.
    waited_ms: Cell<u32>,
    /// `spec/04` §1's duration-and-size badge, when this card has one. It says how far
    /// the GIF has got while a recording's is being written (D113).
    badge: Option<gtk::Label>,
    /// The timer following that render, while one is.
    render_follow: RefCell<Option<glib::SourceId>>,
    host: Weak<dyn CardHost>,
}

impl std::fmt::Debug for Card {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Card")
            .field("id", &self.id)
            .field("path", &self.capture.path)
            .field("card_size", &self.card_size)
            .finish_non_exhaustive()
    }
}

/// Everything the card needs that is not the capture itself.
#[derive(Debug, Clone, Copy)]
pub struct CardConfig {
    pub size_step: u8,
    pub auto_close: AutoClose,
    pub kind: CardKind,
    /// True when the after-capture plan already wrote the file out, which turns Save into
    /// Trash (`spec/04` §1).
    pub already_saved: bool,
    /// `spec/08` §4's `qao-shortcuts`.
    pub shortcuts: bool,
    /// How long the extension's fly animation still has to run (`spec/03` §7 step 5).
    /// The card is mapped immediately -- placement needs a surface -- but stays invisible
    /// until the capture has finished flying into the slot.
    pub arrive_after_ms: u32,
}

impl Default for CardConfig {
    fn default() -> Self {
        Self {
            size_step: qao::DEFAULT_SIZE_STEP,
            auto_close: AutoClose::default(),
            kind: CardKind::Image,
            already_saved: false,
            shortcuts: true,
            arrive_after_ms: 0,
        }
    }
}

impl Card {
    /// Builds the window and maps it. Does **not** place it: the caller does that once
    /// the window has a surface, because only then does it have a size to place.
    pub fn new(
        app: &adw::Application,
        id: CardId,
        capture: CaptureResult,
        config: CardConfig,
        host: &Rc<dyn CardHost>,
    ) -> Rc<Self> {
        let card_size = qao::card_size(config.size_step);
        let band = qao::SHADOW_MARGIN;

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title(format!("octosnap-qao:{id}"))
            .decorated(false)
            .resizable(false)
            .deletable(false)
            // `spec/04` §9's "one transparent, undecorated window per card", sized to the
            // card plus the band its shadow is drawn into. Not screen-sized: `spec/04` §2
            // is explicit that copying the reference app's screen-sized windows would be
            // "a straight performance loss" on Wayland, where every one of them would be
            // a full-screen blend behind everything else.
            .default_width(card_size.width + 2 * band)
            .default_height(card_size.height + 2 * band)
            .build();
        window.add_css_class("octosnap-card");

        let controls = Controls { kind: config.kind, already_saved: config.already_saved };
        let (body, scrim, hairline, copied, badge) = build_body(&capture, card_size, controls);

        // Invisible from the first frame, not from the first `map`. Two reasons, and the
        // second one cost a debugging session:
        //
        // 1. A card that is opaque until `map` runs shows one frame of itself at full
        //    size before the arrival animation starts it small and transparent.
        // 2. Changing a widget property inside `connect_map` queues a redraw, and GTK
        //    then commits content to a Wayland surface the compositor has not configured
        //    yet -- which Mutter reports as "Buggy client committed initial non-empty
        //    content without acknowledging configuration". `docs/decisions.md` D33.
        body.set_opacity(0.0);
        body.set_can_target(false);

        // A `Fixed` so the card can be translated and scaled inside its window without
        // the window itself moving. Moving a Wayland toplevel per animation frame means a
        // D-Bus round trip to the extension per frame; transforming inside the surface is
        // free. It also means the animation is bounded by the shadow band -- see the
        // note on `slide_out`.
        let stage = gtk::Fixed::new();
        stage.put(&body, f64::from(band), f64::from(band));
        // Pinned to the window's size, and this is not cosmetic. `GtkFixed` measures its
        // children *including* their transforms, so animating the card's scale changed
        // what the window asked to be -- GTK renegotiated the toplevel's size mid-frame
        // and committed before acknowledging the configure that came back. Mutter
        // reported that on every capture as "Buggy client committed initial non-empty
        // content without acknowledging configuration". Measured: with the transform
        // removed the warning disappears entirely, so the transform is the cause and the
        // size request is the fix. `docs/decisions.md` D33.
        stage.set_size_request(
            card_size.width + 2 * band,
            card_size.height + 2 * band,
        );

        window.set_child(Some(&stage));

        let card = Rc::new(Self {
            id,
            capture,
            window: window.clone(),
            body: body.clone().upcast(),
            scrim,
            hairline,
            copied,
            card_size,
            controls,
            timer: RefCell::new(CloseTimer::new(config.auto_close)),
            tick: RefCell::new(None),
            hover_animation: RefCell::new(None),
            transition: RefCell::new(None),
            closing: Cell::new(false),
            waited_ms: Cell::new(0),
            badge,
            render_follow: RefCell::new(None),
            host: Rc::downgrade(host),
        });

        card.wire_hover(&body);
        card.wire_clicks(&body);
        card.wire_scroll(&body);
        card.wire_drag(&body);
        if config.shortcuts {
            card.wire_shortcuts();
        }
        card.wire_buttons();

        {
            // `Weak`, like every other handler on this window. A strong `Rc` here closed
            // the cycle `card -> window -> map handler -> card`, and `destroy()` then
            // unrealized a window nothing could finalize: every closed card kept its
            // texture -- two megabytes per capture, for the life of the service. The host
            // holds the card while it is mapped, so the upgrade cannot fail while the
            // handler has anything to do (D60).
            let card = Rc::downgrade(&card);
            let delay = config.arrive_after_ms;
            window.connect_map(move |_| {
                if let Some(card) = card.upgrade() {
                    card.on_mapped(delay);
                }
            });
        }

        // `set_visible`, never `present()`: `spec/01` §5's "never focus-steal", measured
        // in `docs/spikes/01-02`.
        window.set_visible(true);

        card
    }

    /// The GTK object path the extension addresses this window by.
    ///
    /// `spec/01` §3.3 documents `/org/octosnap/App/window/N`; the real prefix is derived
    /// from the application id, so it is `/io/github/odrakirmusic/OctoSnap/window/N`
    /// (measured in `docs/spikes/01-02`). Built from the app's own path rather than
    /// hard-coded, so a rename of the application id cannot leave the two halves
    /// disagreeing silently.
    #[must_use]
    pub fn object_path(&self) -> Option<String> {
        let app = self.window.application()?;
        let base = app.dbus_object_path()?;
        Some(format!("{base}/window/{}", self.window.id()))
    }

    /// The card's size without the shadow band -- the number the stack works in.
    #[must_use]
    pub const fn card_size(&self) -> Size {
        self.card_size
    }

    /// `spec/04` §5's hide/show all.
    pub fn set_hidden(&self, hidden: bool) {
        self.window.set_visible(!hidden);
    }

    /// `spec/04` §3's "subtle ring": the card is holding the keyboard.
    ///
    /// Set by the overlay when the extension confirms the loan, rather than derived from
    /// a GTK focus state -- see the note in `style.rs` for why every CSS route to this
    /// was wrong.
    pub fn set_focus_ring(&self, focused: bool) {
        if focused {
            self.body.add_css_class(FOCUS_CLASS);
        } else {
            self.body.remove_css_class(FOCUS_CLASS);
        }
    }

    /// Starts the auto-close countdown. Separate from construction because the card
    /// should not begin dying before it has finished arriving.
    pub fn start_timer(self: &Rc<Self>) {
        // No source at all when nothing counts down. A 10 Hz timeout per card that can
        // only ever answer "not yet" is a wakeup every 100 ms for as long as the card is
        // up -- and with `AutoClose::default()` now "never" (D38), that would be every
        // card, all the time.
        if !self.timer.borrow().counts_down() {
            self.update_hairline();
            return;
        }
        let card = Rc::downgrade(self);
        let source = glib::timeout_add_local(
            std::time::Duration::from_millis(u64::from(TICK_MS)),
            move || {
                let Some(card) = card.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                card.on_tick()
            },
        );
        if let Some(previous) = self.tick.replace(Some(source)) {
            previous.remove();
        }
    }

    fn on_tick(self: &Rc<Self>) -> glib::ControlFlow {
        // Every `Break` below drops the handle first. `SourceId::remove` **panics** on a
        // source that has already finished -- and because it is called from a GLib
        // callback, the panic crosses an `extern "C"` frame and aborts the process rather
        // than unwinding. So a card closing on its own timer took the whole app down with
        // it, ten seconds after every capture. `docs/decisions.md` D32.
        if self.closing.get() {
            self.tick.take();
            return glib::ControlFlow::Break;
        }
        let fired = self.timer.borrow_mut().tick(TICK_MS);
        self.update_hairline();
        if fired {
            self.tick.take();
            if let Some(host) = self.host.upgrade() {
                host.close(self.id, CloseReason::Timeout);
            }
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    }

    fn update_hairline(&self) {
        let timer = self.timer.borrow();
        // Absent entirely when nothing is counting down, which is now the default. Asking
        // `progress()` is not enough: "never" reports 0.0, the same as "just arrived", so
        // the bar was drawn at full width and never moved.
        if !timer.counts_down() {
            self.hairline.set_visible(false);
            return;
        }
        let remaining = 1.0 - timer.progress();
        #[allow(clippy::cast_possible_truncation)]
        let width = (f64::from(self.card_size.width) * remaining).round() as i32;
        self.hairline.set_visible(width > 0);
        self.hairline.set_size_request(width.max(1), 2);
    }

    /// Sets one of `spec/04` §6's holds and re-syncs the timer.
    pub fn set_hold(&self, edit: impl FnOnce(&mut Holds)) {
        let mut timer = self.timer.borrow_mut();
        let mut holds = timer.holds();
        edit(&mut holds);
        timer.set_holds(holds);
    }

    /// `spec/04` §3: "Card appears | Fly-in from the capture rect (extension animation)
    /// **then** card fades in at its slot".
    ///
    /// The window is mapped straight away because it needs a surface before the extension
    /// can place it -- but it is transparent and click-through until the capture has
    /// finished flying into it, or the card would be sitting in the corner watching its
    /// own arrival.
    fn on_mapped(self: &Rc<Self>, delay_ms: u32) {
        self.wait_for_surface();
        self.waited_ms.set(delay_ms);

        if delay_ms == 0 {
            self.arrive();
            return;
        }
        let card = Rc::downgrade(self);
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(u64::from(delay_ms)),
            move || {
                if let Some(card) = card.upgrade()
                    && !card.closing.get()
                {
                    card.arrive();
                }
            },
        );
    }

    /// Waits for the compositor to configure the surface, then sets the input region and
    /// tells the host the card can be placed.
    ///
    /// Both of those looked safe to do from `map`, and both were wrong. GTK maps the
    /// window before the Wayland surface has been configured, so acting at map time
    /// produced two complaints from Mutter on every single capture:
    ///
    /// ```text
    /// meta_window_set_stack_position_no_sync: assertion 'window->stack_position >= 0' failed
    /// Buggy client (io.github.odrakirmusic.OctoSnap) committed initial non-empty
    ///   content without acknowledging configuration, working around.
    /// ```
    ///
    /// The first is the extension's `make_above` reaching a window Mutter has not yet put
    /// in its stack; the second is this process committing a surface it had not acked.
    /// Neither was fatal -- Mutter says "working around" and means it -- but "the
    /// compositor logs an assertion failure every time you take a screenshot" is not a
    /// state to ship, and a worked-around race is still a race.
    ///
    /// The signal to wait for is the **first frame**, not the first configure.
    ///
    /// `layout` was the obvious candidate and it is too early by one frame: the surface
    /// has a size GTK agreed to, but Mutter has no buffer yet, so `get_frame_rect()`
    /// still answers `0x0`. Placement computed against a zero-size window put the card at
    /// `y = 1200` on a 1200 px display -- exactly one card-height below the bottom edge,
    /// which is what "anchor the bottom of a window with no height" means.
    ///
    /// `after-paint` fires once GTK has painted and committed, which is the first moment
    /// the compositor can answer questions about this window. The handler disconnects
    /// itself, because it fires for every frame afterwards.
    fn wait_for_surface(self: &Rc<Self>) {
        let Some(surface) = self.window.surface() else {
            warn!(id = self.id, "card has no surface at map time");
            return;
        };
        let clock = surface.frame_clock();
        let card = Rc::downgrade(self);
        let handler = Rc::new(RefCell::new(None));
        let once = Rc::clone(&handler);
        *handler.borrow_mut() = Some(clock.connect_after_paint(move |clock| {
            if let Some(id) = once.borrow_mut().take() {
                clock.disconnect(id);
            } else {
                return;
            }
            let Some(card) = card.upgrade() else { return };
            card.apply_input_region();
            if let Some(host) = card.host.upgrade() {
                host.ready(card.id);
            }
        }));
    }

    /// `spec/04` §9: "Input region = the card's rounded rect (so the shadow margin is
    /// click-through)".
    ///
    /// Without this the window would swallow clicks in a 16 px band all around the card,
    /// including the band between two stacked cards -- so the gap between cards would be
    /// a dead zone rather than the desktop, and clicking "behind" a card near its corner
    /// would do nothing at all. That is the kind of bug users describe as "the desktop
    /// stops working in that corner".
    ///
    /// The rectangle comes from `qao::input_region` rather than from this file's own
    /// arithmetic, and `qao::reachable` is the statement that the controls have to fall
    /// inside it. The band's width was never the problem -- it was correct throughout
    /// D39. What was missing was anything tying it to where the body is drawn.
    fn apply_input_region(&self) {
        let Some(surface) = self.window.surface() else {
            warn!(id = self.id, "card has no surface at map time; input region not set");
            return;
        };
        let region = qao::input_region(self.card_size);
        surface.set_input_region(Some(&gtk::cairo::Region::create_rectangle(
            &gtk::cairo::RectangleInt::new(region.x, region.y, region.width, region.height),
        )));
    }

    /// Reads back where the card's controls actually landed, once the frame that placed
    /// them has been laid out and painted.
    ///
    /// The invariant is about the *drawn* control, not the one this file asked for, and
    /// D39 is the reason that distinction matters: every number in `build_scrim` was
    /// right, every number in `apply_input_region` was right, and three of the four
    /// corner controls still took no clicks, because a transform between them moved the
    /// body out from under the region. Nothing that reasons about intent can see that.
    /// Measuring can, and turns it into a line in the log.
    ///
    /// Measured against the `GtkFixed`, not against the body. The Fixed is pinned to the
    /// window's own size, so its coordinate space is the surface's -- the space
    /// `set_input_region` speaks. Bounds taken inside the body would have been correct all
    /// through the bug, because the body is the thing that moved.
    ///
    /// After paint, not `add_tick_callback`, for the reason spelled out in
    /// `Pin::verify_unlock_control`: a tick runs in the frame clock's update phase, before
    /// layout, so it reads the geometry of the previous frame.
    fn verify_controls(self: &Rc<Self>) {
        let Some(surface) = self.window.surface() else { return };
        // The `GtkFixed` from `new`, reached the way `set_card_transform` reaches it
        // rather than through a field of its own, so there is one answer to "what is the
        // body's parent" instead of two that can drift.
        let Some(stage) = self.body.parent() else { return };
        let card = Rc::downgrade(self);
        let handler = Rc::new(RefCell::new(None));
        let once = Rc::clone(&handler);
        *handler.borrow_mut() = Some(surface.frame_clock().connect_after_paint(move |clock| {
            let Some(id) = once.borrow_mut().take() else { return };
            clock.disconnect(id);
            let Some(card) = card.upgrade() else { return };
            if card.closing.get() {
                return;
            }
            let region = qao::input_region(card.card_size);
            let mut drawn = Vec::new();
            let mut dead = Vec::new();
            // `button_actions` rather than a list of corners: a control that gains an
            // action gains this check with it, which is the only way a table like that
            // stays the single place a button is declared.
            for (name, _) in button_actions() {
                let Some(widget) = find_named(&card.body, name) else { continue };
                let Some(bounds) = widget.compute_bounds(&stage) else { continue };
                let rect = enclosing_rect(bounds);
                let reading = format!("{name} {},{} {}x{}", rect.x, rect.y, rect.width, rect.height);
                if !qao::reachable(card.card_size, rect) {
                    dead.push(reading.clone());
                }
                drawn.push(reading);
            }
            if drawn.is_empty() {
                // `Tier::Bare`: a card too small for any control at all. Said out loud so
                // that a silent absence cannot be read as a clean reading.
                debug!(id = card.id, ?region, "card draws no controls at this size");
            } else if dead.is_empty() {
                debug!(
                    id = card.id,
                    ?region,
                    controls = %drawn.join("; "),
                    "card controls reachable"
                );
            } else {
                warn!(
                    id = card.id,
                    ?region,
                    unreachable = %dead.join("; "),
                    "card controls unreachable: a control outside the input region takes no clicks"
                );
            }
        }));
        // A frame has to actually happen for `after-paint` to fire, and a card at rest
        // asks for none. The arrival's last transform queues one, but whether libadwaita
        // emits `done` before or after that frame's paint is its business rather than
        // ours -- and a verifier that only runs when some other redraw happens to come
        // along is a verifier that says nothing on the quiet path. One redraw is cheaper
        // than that uncertainty.
        self.body.queue_draw();
    }

    /// `spec/04` §8: "Arrival (after fly-in) | 180 ms | ease-out-cubic | opacity 0→1,
    /// scale 0.96→1, y +8→0".
    fn arrive(self: &Rc<Self>) {
        self.report_budget();
        let body = self.body.clone();
        body.set_opacity(0.0);
        body.set_can_target(true);

        let target = adw::CallbackAnimationTarget::new({
            let body = body.clone();
            let size = self.card_size;
            move |t| {
                body.set_opacity(t);
                let scale = 0.96 + 0.04 * t;
                let lift = 8.0 * (1.0 - t);
                set_card_transform(&body, size, scale, (0.0, lift));
            }
        });

        let animation = adw::TimedAnimation::builder()
            .widget(&body)
            .value_from(0.0)
            .value_to(1.0)
            .duration(ARRIVE_MS)
            .easing(adw::Easing::EaseOutCubic)
            .target(&target)
            .build();
        // Measured when the arrival has settled, which is the only frame worth measuring:
        // the transform is at its final value and the card will sit there for the rest of
        // its life. It is also the frame D39 broke -- the first animation frame is what
        // moved the body, so a check that ran before this one would have passed.
        //
        // `Weak`, for the reason `slide_out` spells out: the animation is owned by the
        // card, so a callback owning the card back would keep every closed card's window
        // and texture alive for the session.
        {
            let card = Rc::downgrade(self);
            animation.connect_done(move |_| {
                if let Some(card) = card.upgrade() {
                    // Re-asserted here, not only from `after-paint`.
                    //
                    // The region is what decides whether the card can be clicked at all,
                    // and the first application races the commit Mutter complains about
                    // in D33 -- "committed initial non-empty content without
                    // acknowledging configuration, working around", which is still in the
                    // journal for every card. Whether a region set across that commit
                    // survives it is not something a client can read back: GDK exposes no
                    // getter, so there is nothing to check and nothing to log.
                    //
                    // So it is stated twice instead. Once as early as possible, so the
                    // card is usable the moment it lands, and once on the frame the
                    // arrival settles -- which is past every configure and every
                    // workaround. Setting an input region is idempotent and costs one
                    // call; a card that is born click-through costs a user who thinks the
                    // buttons do not work.
                    card.apply_input_region();
                    card.verify_controls();
                }
            });
        }
        animation.play();
        *self.transition.borrow_mut() = Some(animation);
    }

    /// `spec/00` §9's budget: 300 ms from the capture to the card, measured end to end.
    ///
    /// Logged here because this is the frame the card starts becoming visible on, and
    /// logged **at all** because nobody could measure it. `spec/04` §10 item 1 asks for
    /// the card "within 300 ms" and the honest answer for two milestones was "not
    /// measured": the shell knows when the user stopped deciding and the app knows when
    /// the card appeared, and no single process could see both ends. Asking a person to
    /// judge it is not a measurement either -- as the report on 2026-09-09 put it, "as a
    /// human, I cannot test this. I may notice a delay above near 1000 ms+, but not like
    /// that."
    ///
    /// So the shell stamps `confirmed_at` and this subtracts it. Everything before that
    /// instant is a person choosing a region and is deliberately excluded, which is the
    /// same line `flow.ts` draws for its own figure.
    ///
    /// What the budget is judged on is the fly's start, where `spec/10` §7's row ends
    /// ("incl. sound + fly animation start"): the card lands with the fly, 400 ms of
    /// deliberate motion later, so a card inside 300 ms could only be a card with no fly.
    /// The shell stamps `timestamp` as the fly sets off (`flow.ts`, D127), so that part is
    /// `timestamp` less `confirmed_at`. The card's own arrival is printed beside it, with
    /// `waited_ms` saying whether it was on time for the landing: a card that held its
    /// arrival was ready first, and one that held nothing came after the fly had landed.
    fn report_budget(&self) {
        let Some(confirmed_at) = self.capture.confirmed_at else {
            debug!(id = self.id, "no confirmation stamp; the twin predates the budget line");
            return;
        };
        let now = glib::real_time();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let elapsed_ms = (now.saturating_sub(confirmed_at as i64) / 1000) as u64;
        let fly_ms = u64::from(self.capture.animation_ms);
        let flew_after_ms = self.capture.timestamp.saturating_sub(confirmed_at) / 1000;
        // With no fly, the card is the first thing the user sees.
        let seen_ms = if fly_ms > 0 { flew_after_ms } else { elapsed_ms };
        // `info` and not `debug`: this is the one number `spec/00` §9 asks for, and a
        // figure that only appears under a debug filter is a figure nobody reads.
        info!(
            id = self.id,
            elapsed_ms,
            flew_after_ms,
            fly_ms,
            waited_ms = self.waited_ms.get(),
            budget_ms = BUDGET_MS,
            within = seen_ms <= BUDGET_MS,
            "capture to card"
        );
    }

    /// `spec/04` §8's copy tick: "check icon scales 0.6->1", 250 ms, "then close".
    ///
    /// Shown *after* the clipboard has actually taken the image, which is the whole point
    /// of it existing. Reported from hardware on 2026-09-09 -- "this doesn't work, the
    /// card preview just disappears!" -- and the clipboard was fine: the image was there,
    /// 283530 bytes of `image/png`, measured with `wl-paste`. What was missing was any
    /// sign that it had happened, so a control that worked read as one that did not.
    ///
    /// `done` must not hold an `Rc<Card>`, for `slide_out`'s reason: the animation is
    /// owned by the card, so a callback owning the card back would keep every copied
    /// card's window and texture alive for the session.
    pub fn confirm_copied(self: &Rc<Self>, done: impl Fn() + 'static) {
        // A card already on its way out gets no tick -- and `done` is still called, or the
        // caller would be waiting for a close that never came.
        if self.closing.get() {
            done();
            return;
        }
        // The scrim goes, so the tick is the only thing on the thumbnail. The hover
        // controls have just been used and leaving them up under a confirmation reads as
        // an offer to press them again.
        self.scrim.set_opacity(0.0);
        self.copied.set_opacity(1.0);

        // With animations off, libadwaita plays the tick in no time and calls `done` from
        // inside `play`, and the card was gone before the tick had been drawn: the
        // confirmation this exists for never reached the screen (D129). So the tick is
        // shown at its full size and held for the animation's length instead. Reduced
        // motion is fewer things moving, not fewer things said.
        if !adw::is_animations_enabled(&self.copied) {
            glib::timeout_add_local_once(
                std::time::Duration::from_millis(u64::from(COPY_TICK_MS)),
                done,
            );
            return;
        }

        // 0.6 -> 1, from `spec/04` §8, animated as the icon's **pixel size**.
        //
        // Not a `gsk::Transform`, which is how the card's own arrival scales: that goes
        // through `GtkFixed::set_child_transform`, and the tick lives in the body's
        // `GtkOverlay` where there is no such thing -- `GtkImage` has no transform
        // property either, so setting one would silently do nothing. A symbolic icon
        // re-renders crisply at any size, so its pixel size *is* its scale, and one
        // 250 ms one-shot on one small icon costs nothing.
        let full = self.copied.property::<i32>("pixel-size").max(1);
        let target = adw::CallbackAnimationTarget::new({
            let copied = self.copied.clone();
            move |t| {
                let scale = 0.6 + 0.4 * t;
                #[allow(clippy::cast_possible_truncation)]
                let px = (f64::from(full) * scale).round() as i32;
                copied.set_property("pixel-size", px.max(1));
            }
        });
        let animation = adw::TimedAnimation::builder()
            .widget(&self.copied)
            .value_from(0.0)
            .value_to(1.0)
            .duration(COPY_TICK_MS)
            .easing(adw::Easing::EaseOutQuad)
            .target(&target)
            .build();
        animation.connect_done(move |_| done());
        animation.play();
        *self.transition.borrow_mut() = Some(animation);
    }

    /// Copy with Alt held: the tick, and then the card as it was, still up (D137).
    pub fn confirm_copied_kept(self: &Rc<Self>) {
        let card = Rc::downgrade(self);
        self.confirm_copied(move || {
            if let Some(card) = card.upgrade() {
                card.copied.set_opacity(0.0);
                let hovered = card.timer.borrow().holds().hovered;
                card.set_scrim(hovered);
            }
        });
    }

    /// `spec/04` §8's close: fade and shrink, then tell the host it may drop the card.
    ///
    /// `done` must not hold an `Rc<Card>`. The animation is owned by the card, so a
    /// callback that owned the card back would close a cycle through
    /// `card -> transition -> handler -> card`, and every closed card would keep its
    /// window and its texture alive for the rest of the session. The host keeps the card
    /// alive for the animation's duration instead, which it has to do anyway.
    ///
    /// The spec asks for a 24 px slide toward the nearest screen edge. That is a macOS
    /// shape: over there each card lives in a screen-sized window, so it can slide any
    /// distance inside its own surface. `spec/04` §2 deliberately rejected screen-sized
    /// windows here -- six full-screen blends is a straight performance loss on Wayland --
    /// and the consequence is that a card can only be animated within its own 16 px
    /// shadow band. So the slide is the band's width and the rest of the distance is
    /// spent on scale, which reads as the same gesture. `docs/decisions.md` D31.
    ///
    /// The nearest edge is the one the stack stands against, `toward`: a card is 16 px
    /// from it and the rest of the screen away from every other. It used to lift instead,
    /// up and away from the bottom it is anchored to, which read as the card being put
    /// back rather than sent off (D129).
    pub fn slide_out(self: &Rc<Self>, toward: Edge, done: impl Fn() + 'static) {
        if self.closing.replace(true) {
            return;
        }
        if let Some(source) = self.tick.take() {
            source.remove();
        }

        let body = self.body.clone();
        let side = match toward {
            Edge::Left => -1.0,
            Edge::Right => 1.0,
        };
        let target = adw::CallbackAnimationTarget::new({
            let body = body.clone();
            let size = self.card_size;
            move |t| {
                body.set_opacity(1.0 - t);
                let slide = side * f64::from(qao::SHADOW_MARGIN) * t;
                set_card_transform(&body, size, 1.0 - 0.10 * t, (slide, 0.0));
            }
        });

        let animation = adw::TimedAnimation::builder()
            .widget(&body)
            .value_from(0.0)
            .value_to(1.0)
            .duration(CLOSE_MS)
            .easing(adw::Easing::EaseInQuad)
            .target(&target)
            .build();
        animation.connect_done(move |_| done());
        animation.play();
        // Replaces the arrival animation, which has necessarily finished by now: a card
        // cannot be closed before it has been shown.
        *self.transition.borrow_mut() = Some(animation);
    }

    /// Tears the window down. Idempotent: the timer and a click can both arrive.
    pub fn destroy(&self) {
        if let Some(source) = self.tick.take() {
            source.remove();
        }
        // Released here rather than left to the drop, because an animation still playing
        // holds the widget it animates and the callback it was built with. Taking them
        // first makes the teardown order the same whether the card is dropped now or in
        // a moment.
        if let Some(animation) = self.transition.take() {
            animation.pause();
        }
        if let Some(animation) = self.hover_animation.take() {
            animation.pause();
        }
        self.window.set_visible(false);
        self.window.destroy();
    }

    fn wire_hover(self: &Rc<Self>, body: &gtk::Widget) {
        let motion = gtk::EventControllerMotion::new();
        {
            let card = Rc::downgrade(self);
            motion.connect_enter(move |_, _, _| {
                if let Some(card) = card.upgrade() {
                    card.set_scrim(true);
                    card.set_hold(|h| h.hovered = true);
                    if let Some(host) = card.host.upgrade() {
                        host.hovered(card.id, true);
                    }
                }
            });
        }
        {
            let card = Rc::downgrade(self);
            motion.connect_leave(move |_| {
                if let Some(card) = card.upgrade() {
                    card.set_scrim(false);
                    card.set_hold(|h| h.hovered = false);
                    if let Some(host) = card.host.upgrade() {
                        host.hovered(card.id, false);
                    }
                }
            });
        }
        body.add_controller(motion);
    }

    /// `spec/04` §8: "Hover controls | 100 ms | ease-out | opacity only".
    fn set_scrim(&self, shown: bool) {
        if let Some(previous) = self.hover_animation.take() {
            previous.pause();
        }
        let scrim = self.scrim.clone();
        let target = adw::PropertyAnimationTarget::new(&scrim, "opacity");
        let animation = adw::TimedAnimation::builder()
            .widget(&scrim)
            .value_from(scrim.opacity())
            .value_to(if shown { 1.0 } else { 0.0 })
            .duration(HOVER_MS)
            .easing(adw::Easing::EaseOutQuad)
            .target(&target)
            .build();
        animation.play();
        *self.hover_animation.borrow_mut() = Some(animation);
    }

    fn wire_clicks(self: &Rc<Self>, body: &gtk::Widget) {
        // Left: click focuses (which the window does on its own), double-click annotates.
        let primary = gtk::GestureClick::new();
        primary.set_button(gdk::BUTTON_PRIMARY);
        {
            let card = Rc::downgrade(self);
            primary.connect_pressed(move |_, clicks, _, _| {
                if clicks < 2 {
                    return;
                }
                if let Some(card) = card.upgrade()
                    && card.controls.has_edit()
                    && let Some(host) = card.host.upgrade()
                {
                    host.annotate(card.id);
                }
            });
        }
        body.add_controller(primary);

        // `spec/04` §3: "Middle-click | Close".
        let middle = gtk::GestureClick::new();
        middle.set_button(gdk::BUTTON_MIDDLE);
        {
            let card = Rc::downgrade(self);
            middle.connect_released(move |_, _, _, _| {
                if let Some(card) = card.upgrade()
                    && let Some(host) = card.host.upgrade()
                {
                    host.close(card.id, CloseReason::UserClosed);
                }
            });
        }
        body.add_controller(middle);

        let secondary = gtk::GestureClick::new();
        secondary.set_button(gdk::BUTTON_SECONDARY);
        {
            let card = Rc::downgrade(self);
            secondary.connect_pressed(move |_, _, x, y| {
                if let Some(card) = card.upgrade() {
                    card.open_menu(x, y);
                }
            });
        }
        body.add_controller(secondary);
    }

    /// `spec/04` §3: "Scroll down over the card (touchpad two-finger) | Hide temporarily
    /// until the next capture".
    fn wire_scroll(self: &Rc<Self>, body: &gtk::Widget) {
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        {
            let card = Rc::downgrade(self);
            scroll.connect_scroll(move |_, _, dy| {
                // Downward only, and past a threshold: a stray pixel of scroll while the
                // pointer crosses a card must not make the whole stack vanish.
                if dy > 0.5
                    && let Some(card) = card.upgrade()
                    && let Some(host) = card.host.upgrade()
                {
                    host.hide_all();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        body.add_controller(scroll);
    }

    /// `spec/04` §3: "Drag thumbnail | Starts a drag with `text/uri-list` + `image/png`;
    /// drag icon = the thumbnail".
    fn wire_drag(self: &Rc<Self>, body: &gtk::Widget) {
        let source = gtk::DragSource::new();
        source.set_actions(gdk::DragAction::COPY);

        {
            let card = Rc::downgrade(self);
            source.connect_prepare(move |_, _, _| {
                let card = card.upgrade()?;
                card.set_hold(|h| h.dragging = true);
                card.drag_content()
            });
        }
        {
            let card = Rc::downgrade(self);
            source.connect_drag_begin(move |source, drag| {
                let Some(card) = card.upgrade() else { return };
                if let Some(texture) = card.texture() {
                    // Hot spot at the pointer rather than a corner: the thumbnail under
                    // the cursor should feel picked up where it was grabbed.
                    let size = card.card_size;
                    source.set_icon(Some(&texture), size.width / 2, size.height / 2);
                }

                // `dnd-finished`, not `drag-end`. `spec/04` §3 closes the card "on
                // successful drop", and `drag-end` fires either way -- including when the
                // user drags out and lets go over the desktop, which would silently file
                // the capture away after a gesture that did nothing. `dnd-finished` is
                // the signal that means a receiver took it.
                let inner = Rc::downgrade(&card);
                drag.connect_dnd_finished(move |_| {
                    let Some(card) = inner.upgrade() else { return };
                    let keep = alt_held();
                    debug!(id = card.id, keep, "dropped");
                    if let Some(host) = card.host.upgrade() {
                        host.dropped(card.id, keep);
                    }
                });
            });
        }
        {
            let card = Rc::downgrade(self);
            source.connect_drag_end(move |_, _, delete| {
                let Some(card) = card.upgrade() else { return };
                card.set_hold(|h| h.dragging = false);
                debug!(id = card.id, delete, "drag ended");
                if let Some(host) = card.host.upgrade() {
                    host.hovered(card.id, false);
                }
            });
        }
        body.add_controller(source);
    }

    /// Writes a recording's GIF from its frames, or joins the render already doing so, and
    /// says how far it has got on the badge until it is done (D113). A card whose capture
    /// has nothing to write -- a screenshot, a GIF already written -- is left alone.
    ///
    /// The card starts the render itself, rather than waiting for the Save or Copy that
    /// asked for it to get there, so the badge has something to follow from the first tick.
    pub fn follow_render(self: &Rc<Self>) {
        use crate::recording::render;
        if render::start(&self.capture.path).is_none() || self.render_follow.borrow().is_some() {
            return;
        }
        self.set_hold(|h| h.rendering = true);
        let card = Rc::downgrade(self);
        let source = glib::timeout_add_local(RENDER_TICK, move || {
            let Some(card) = card.upgrade() else { return glib::ControlFlow::Break };
            let progress = render::progress_of(&card.capture.path);
            let Some(badge) = &card.badge else {
                return if progress.is_some() {
                    glib::ControlFlow::Continue
                } else {
                    card.render_follow.borrow_mut().take();
                    card.set_hold(|h| h.rendering = false);
                    glib::ControlFlow::Break
                };
            };
            match progress.map(|p| p.fraction()) {
                Some(Some(fraction)) => {
                    badge.set_text(&format!("Writing GIF\u{2026} {:.0} %", fraction * 100.0));
                }
                Some(None) => badge.set_text("Writing GIF\u{2026}"),
                None => {
                    // Written, or given up on: the badge goes back to what it says of the
                    // capture, which now has a file with a size.
                    if let Some(text) = badge_text(&card.capture) {
                        badge.set_text(&text);
                    }
                    card.render_follow.borrow_mut().take();
                    card.set_hold(|h| h.rendering = false);
                    return glib::ControlFlow::Break;
                }
            }
            glib::ControlFlow::Continue
        });
        *self.render_follow.borrow_mut() = Some(source);
    }

    /// The drag payload: the file first, the pixels second.
    ///
    /// Order is the contract. `ContentProvider::new_union` offers formats in the order
    /// given, and receivers take the first they understand -- so a file manager and a
    /// terminal get a path, while an image editor or a browser upload field that cannot
    /// use a path still finds `image/png`. Reversing this would drop a PNG into Nautilus
    /// as a new file rather than moving the one that exists.
    ///
    /// A recording whose GIF is still frames offers only the picture: there is no file to
    /// hand over yet, and a drag cannot wait a minute for one (D113).
    fn drag_content(&self) -> Option<gdk::ContentProvider> {
        let mut providers = Vec::with_capacity(2);
        if !crate::recording::render::pending(&self.capture.path) {
            let file = gio::File::for_path(&self.capture.path);
            providers.push(gdk::ContentProvider::for_value(&file.to_value()));
        }
        if let Some(texture) = self.texture() {
            providers.push(gdk::ContentProvider::for_value(&texture.to_value()));
        }
        Some(gdk::ContentProvider::new_union(&providers))
    }

    fn texture(&self) -> Option<gdk::Texture> {
        crate::history::thumbnail::texture_of(&self.capture.path)
    }

    /// `spec/04` §3's keyboard vocabulary, scoped to this window.
    ///
    /// Scoped, not global: these are the shortcuts that work *because* the pointer is
    /// over the card and the extension has lent it the keyboard. The global equivalents
    /// ("Copy last capture" and friends) are `spec/08` §2's, and belong to the app.
    fn wire_shortcuts(self: &Rc<Self>) {
        let controller = gtk::ShortcutController::new();
        controller.set_scope(gtk::ShortcutScope::Local);

        // No Ctrl+U: `spec/04` §3's Upload waits for a sharing provider (`spec/11` M8),
        // and a key that only says so is the control `spec/13` #19 rules out (D133).
        let bindings: [(&str, CardAction); 8] = [
            ("<Control>c", |c| c.ask(|host, id| host.copy(id))),
            // `spec/04` §3: Copy closes the card "unless Alt is held". The host reads Alt
            // when it copies, as it does for the pill; this is the chord that reaches it
            // from the keyboard (D137).
            ("<Control><Alt>c", |c| c.ask(|host, id| host.copy(id))),
            ("<Control>s", |c| c.ask(|host, id| host.save(id))),
            ("<Control>e", |c| c.ask(|host, id| host.annotate(id))),
            ("<Control>w", |c| c.ask(|host, id| host.close(id, CloseReason::UserClosed))),
            ("Escape", |c| c.ask(|host, id| host.close(id, CloseReason::UserClosed))),
            ("Delete", |c| c.ask(|host, id| host.trash(id))),
            // `spec/04` §3: "Space | Quick preview: open the file in the system image
            // viewer (Loupe) or a lightweight in-app previewer". For a GIF that previewer
            // is its editor, which opens playing; a still goes to the image viewer (D137).
            ("space", |c| {
                if c.controls.kind == CardKind::Gif {
                    c.ask(|host, id| host.annotate(id));
                } else {
                    c.ask(|host, id| host.preview(id));
                }
            }),
        ];

        for (accel, action) in bindings {
            // A card without an edit corner does not answer its shortcut either.
            if accel == "<Control>e" && !self.controls.has_edit() {
                continue;
            }
            let Some(trigger) = gtk::ShortcutTrigger::parse_string(accel) else {
                warn!("could not parse the card shortcut {accel}");
                continue;
            };
            let card = Rc::downgrade(self);
            let callback = gtk::CallbackAction::new(move |_, _| {
                if let Some(card) = card.upgrade() {
                    action(&card);
                }
                glib::Propagation::Stop
            });
            controller.add_shortcut(gtk::Shortcut::new(Some(trigger), Some(callback)));
        }

        self.window.add_controller(controller);
    }

    fn ask(self: &Rc<Self>, f: impl FnOnce(&Rc<dyn CardHost>, CardId)) {
        if let Some(host) = self.host.upgrade() {
            f(&host, self.id);
        }
    }

    fn wire_buttons(self: &Rc<Self>) {
        for (name, action) in button_actions() {
            let Some(button) = find_named(&self.body, name) else { continue };

            let Ok(button) = button.downcast::<gtk::Button>() else { continue };
            let card = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(card) = card.upgrade() {
                    action(&card);
                }
            });
        }
    }

    /// `spec/04` §4's context menu, trimmed to what this build can actually do.
    ///
    /// A menu that lists Rotate, Flip and Resize before any of them exists would be a
    /// menu of dead ends. `spec/04` §4's full order is preserved as sections so the items
    /// arrive in their documented places rather than being appended later -- which is why
    /// OCR is third in the first section and not last in the menu, now that it exists.
    fn open_menu(self: &Rc<Self>, x: f64, y: f64) {
        let menu = gio::Menu::new();

        let first = gio::Menu::new();
        if self.controls.has_edit() {
            let label = match self.controls.edit() {
                EditAction::Annotate => "Annotate",
                EditAction::Trim => "Trim",
            };
            first.append(Some(label), Some("card.annotate"));
        }
        if self.controls.has_pin() {
            first.append(Some("Pin to the Screen"), Some("card.pin"));
        }
        if self.controls.has_text() {
            first.append(Some("Copy Text"), Some("card.recognise"));
        }
        if first.n_items() > 0 {
            menu.append_section(None, &first);
        }

        let second = gio::Menu::new();
        second.append(Some("Save As…"), Some("card.save-as"));
        second.append(Some("Move to Trash"), Some("card.trash"));
        menu.append_section(None, &second);

        let last = gio::Menu::new();
        last.append(Some("Copy"), Some("card.copy"));
        last.append(Some("Close"), Some("card.close"));
        menu.append_section(None, &last);

        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(&self.body);
        popover.set_has_arrow(false);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));

        let group = gio::SimpleActionGroup::new();
        for (name, action) in menu_actions() {
            let entry = gio::SimpleAction::new(name, None);
            let card = Rc::downgrade(self);
            entry.connect_activate(move |_, _| {
                if let Some(card) = card.upgrade() {
                    action(&card);
                }
            });
            group.add_action(&entry);
        }
        self.body.insert_action_group("card", Some(&group));

        // An open menu holds the timer (`spec/04` §6), or the card would close out from
        // under the menu the user is reading.
        self.set_hold(|h| h.menu_open = true);
        if let Some(host) = self.host.upgrade() {
            host.menu(self.id, true);
        }
        {
            let card = Rc::downgrade(self);
            popover.connect_closed(move |popover| {
                if let Some(card) = card.upgrade() {
                    card.set_hold(|h| h.menu_open = false);
                    if let Some(host) = card.host.upgrade() {
                        host.menu(card.id, false);
                    }
                }
                // On the next turn: GTK closes the menu before it runs the item that was
                // clicked, and the item finds its action through this parent (D136).
                let popover = popover.clone();
                glib::idle_add_local_once(move || popover.unparent());
            });
        }
        popover.popup();
    }
}

/// What a control does when it is used. A plain function pointer rather than a boxed
/// closure: every one of these is a fixed call into the host, and the tables below are
/// `const`-shaped data.
type CardAction = fn(&Rc<Card>);

/// The button names wired to host calls. One table, so a button that exists in the
/// widget tree but has no action is a missing row here rather than a silent no-op.
fn button_actions() -> [(&'static str, CardAction); 5] {
    [
        ("close", |c| c.ask(|h, id| h.close(id, CloseReason::UserClosed))),
        ("pin", |c| c.ask(|h, id| h.pin(id))),
        ("edit", |c| c.ask(|h, id| h.annotate(id))),
        ("copy", |c| c.ask(|h, id| h.copy(id))),
        ("primary", |c| match c.controls.primary() {
            PrimaryAction::Save => c.ask(|h, id| h.save(id)),
            PrimaryAction::Trash => c.ask(|h, id| h.trash(id)),
        }),
    ]
}

fn menu_actions() -> [(&'static str, CardAction); 7] {
    [
        ("annotate", |c| c.ask(|h, id| h.annotate(id))),
        ("pin", |c| c.ask(|h, id| h.pin(id))),
        ("recognise", |c| c.ask(|h, id| h.recognise(id))),
        ("save-as", |c| c.ask(|h, id| h.save_as(id))),
        ("trash", |c| c.ask(|h, id| h.trash(id))),
        ("copy", |c| c.ask(|h, id| h.copy(id))),
        ("close", |c| c.ask(|h, id| h.close(id, CloseReason::UserClosed))),
    ]
}

/// Scale about the card's centre, then move it by `offset`, **inside the shadow band**.
///
/// The band is part of the transform and has to be: `gtk_fixed_set_child_transform`
/// *replaces* the position `gtk_fixed_put` set, it does not compose with it. The first
/// version left it out, so the first animation frame silently moved the card from
/// `(16, 16)` to the window's origin -- and it stayed there.
///
/// That was not a cosmetic slip. The window still reserved 16 pt on every side for the
/// shadow, so the band existed only on the right and bottom, and the **input region** --
/// correctly set to the band -- no longer covered the card's top-left. Three of the four
/// corner controls sat outside it and took no clicks at all, while the centre pills
/// worked, which reads as "the corner buttons are broken" rather than "the card is in the
/// wrong place". `docs/decisions.md` D39.
///
/// Scale is about the card's centre rather than its origin: scaling about the origin
/// would slide the card toward its top-left as it grew, so the arrival would read as a
/// drift rather than a swell.
fn set_card_transform(body: &gtk::Widget, size: Size, scale: f64, offset: (f64, f64)) {
    let Some(parent) = body.parent().and_downcast::<gtk::Fixed>() else { return };
    let band = f64::from(qao::SHADOW_MARGIN);
    let half_w = f64::from(size.width) / 2.0;
    let half_h = f64::from(size.height) / 2.0;
    #[allow(clippy::cast_possible_truncation)]
    let transform = gtk::gsk::Transform::new()
        .translate(&gtk::graphene::Point::new(
            (band + half_w + offset.0) as f32,
            (band + half_h + offset.1) as f32,
        ))
        .scale(scale as f32, scale as f32)
        .translate(&gtk::graphene::Point::new(-half_w as f32, -half_h as f32));
    parent.set_child_transform(body, Some(&transform));
}

/// The smallest whole-pixel rect containing `bounds`.
///
/// Outward on every side rather than to the nearest pixel, because this feeds a
/// reachability check against a pixel-aligned input region: a control whose edge lands a
/// fraction outside the region has a strip that silently does nothing, and rounding
/// inward would report that as clean. At rest the card's transform is an integer
/// translation and there is no fraction to lose, so the widening costs nothing in the
/// case that matters and errs the right way in the ones that do not.
fn enclosing_rect(bounds: gtk::graphene::Rect) -> Rect {
    let left = bounds.x().floor();
    let top = bounds.y().floor();
    let right = (bounds.x() + bounds.width()).ceil();
    let bottom = (bounds.y() + bounds.height()).ceil();
    #[allow(clippy::cast_possible_truncation)]
    Rect::new(left as i32, top as i32, (right - left) as i32, (bottom - top) as i32)
}

/// Finds a named descendant. Used to wire buttons after the tree is built, which keeps
/// the layout function free of every callback the card needs.
fn find_named(root: &gtk::Widget, name: &str) -> Option<gtk::Widget> {
    if root.widget_name() == name {
        return Some(root.clone());
    }
    let mut child = root.first_child();
    while let Some(current) = child {
        if let Some(found) = find_named(&current, name) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}

/// The card's badge: the recording's length and the file's size (`spec/04` §1). `None`
/// when there is neither -- a card that shows a badge but has no numbers yet.
fn badge_text(capture: &CaptureResult) -> Option<String> {
    use octosnap_media::text;
    let bytes = std::fs::metadata(&capture.path).map(|m| m.len()).ok().filter(|&b| b > 0);
    match (capture.duration_ms, bytes) {
        (Some(ms), Some(bytes)) => Some(text::badge(ms, bytes)),
        (Some(ms), None) => Some(text::duration_label(ms)),
        (None, Some(bytes)) => Some(text::size_label(bytes)),
        (None, None) => None,
    }
}

/// `spec/04` §1's anatomy: a bare thumbnail at rest, six controls under a scrim on hover.
///
/// Returns the body, the scrim (whose opacity is the hover animation), the hairline, the
/// copy tick and the badge, if the card has one.
fn build_body(
    capture: &CaptureResult,
    size: Size,
    controls: Controls,
) -> (gtk::Widget, gtk::Widget, gtk::Widget, gtk::Widget, Option<gtk::Label>) {
    let picture = crate::history::thumbnail::picture_of(&capture.path);
    picture.set_content_fit(gtk::ContentFit::Cover);
    picture.set_can_shrink(true);

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&picture));
    overlay.set_size_request(size.width, size.height);
    overlay.add_css_class("octosnap-card-body");
    // `spec/04` §1's rounded corners. `Overflow::Hidden` is what clips the *children* to
    // the CSS border radius; the radius alone would round the background and leave the
    // thumbnail's square corners sticking out of it.
    overlay.set_overflow(gtk::Overflow::Hidden);
    overlay.set_focusable(true);

    let scrim = build_scrim(controls, size);
    scrim.set_opacity(0.0);
    overlay.add_overlay(&scrim);

    // `spec/04` §1's duration-and-size badge, always visible on a recording card (not
    // only on hover): the length is the recording's own, the size is the file on disk --
    // none yet for a recording whose GIF is still frames (D113).
    let mut badge = None;
    if controls.shows_badge()
        && let Some(text) = badge_text(capture)
    {
        let label = gtk::Label::new(Some(&text));
        label.add_css_class("octosnap-badge");
        label.set_halign(gtk::Align::End);
        label.set_valign(gtk::Align::Start);
        label.set_can_target(false);
        overlay.add_overlay(&label);
        badge = Some(label);
    }

    let hairline = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    hairline.add_css_class("octosnap-hairline");
    hairline.set_halign(gtk::Align::Start);
    hairline.set_valign(gtk::Align::End);
    hairline.set_size_request(size.width, 2);
    hairline.set_can_target(false);
    overlay.add_overlay(&hairline);

    // `spec/04` §8's copy tick, built now and shown for 250 ms when a copy lands.
    //
    // Built with the card rather than on demand because it appears at the moment the card
    // is about to close, and a widget added to a closing window is a frame that may never
    // come. `set_can_target(false)`: it is feedback, never a control.
    let tick = gtk::Image::from_icon_name(COPIED_ICON);
    tick.add_css_class("octosnap-card-tick");
    tick.set_pixel_size(TICK_PX.min(size.height / 2));
    tick.set_halign(gtk::Align::Center);
    tick.set_valign(gtk::Align::Center);
    tick.set_can_target(false);
    tick.set_opacity(0.0);
    overlay.add_overlay(&tick);

    (overlay.clone().upcast(), scrim.upcast(), hairline.upcast(), tick.upcast(), badge)
}

/// `spec/04` §8's copy tick: "check icon scales 0.6->1", 250 ms, "then close".
///
/// A stock Adwaita symbolic, and deliberately so for now. `docs/progress/M2.md` deferred
/// this feedback on the grounds that it "needs an icon set, which is M3's" -- and then a
/// hardware run reported its absence as a bug, because a control that gives no sign it
/// worked reads as broken whatever the reason. `object-select-symbolic` ships with the
/// platform, is a checkmark, and follows the theme. `spec/09` §4b's identity pass in M7
/// replaces it with OctoSnap's own, which is a swap of this constant.
const COPIED_ICON: &str = "object-select-symbolic";

/// The tick's size, capped to half the card so a step-1 card does not wear a tick larger
/// than itself.
const TICK_PX: i32 = 48;

/// Scrim margin, and the diameter of a corner control.
const SCRIM_PAD: i32 = 6;
const CORNER: i32 = 22;
const PILL_H: i32 = 22;
const PILL_GAP: i32 = 3;
const PILL_MAX_W: i32 = 88;

/// How much of `spec/04` §1's six-control layout fits on a card this size.
///
/// The layout was measured on a 207 × 125 card, which is the middle of a five-step size
/// slider running from 120 to 360 (`core::qao::SIZE_TABLE`). At the small end there is
/// simply not room: two corner rows and two stacked pills need 107 pt of height, and a
/// step-1 landscape card is 75 pt tall. Rendering them anyway does not fail cleanly --
/// GTK clips them, so the card grows a row of half-buttons and a truncated pill, which
/// looks like a rendering bug rather than a size the user chose.
///
/// So the controls thin out as the card does. What is never lost is the *card*: every
/// tier still drags, double-clicks to Annotate, middle-clicks to close, and takes the
/// keyboard shortcuts on hover, because those cost no pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    /// Four corners and both pills.
    Full,
    /// Copy and the primary pill only.
    Pills,
    /// A dim, and nothing else that would not be clipped.
    Bare,
}

fn tier(size: Size) -> Tier {
    let full_h = 2 * SCRIM_PAD + 2 * CORNER + 2 * PILL_H + PILL_GAP;
    let pills_h = 2 * SCRIM_PAD + 2 * PILL_H + PILL_GAP;
    if size.height >= full_h && size.width >= 130 {
        Tier::Full
    } else if size.height >= pills_h && size.width >= 96 {
        Tier::Pills
    } else {
        Tier::Bare
    }
}

fn build_scrim(controls: Controls, size: Size) -> gtk::Box {
    let scrim = gtk::Box::new(gtk::Orientation::Vertical, 0);
    scrim.add_css_class("octosnap-scrim");
    // Inset with CSS padding, not widget margins. `spec/04` §1 has the whole thumbnail
    // darkening under the scrim; margins would leave a 6 pt band of undimmed screenshot
    // around it, with the scrim's own rounded corners floating inside the card's.

    let tier = tier(size);
    if tier == Tier::Bare {
        return scrim;
    }

    let (primary_label, destructive) = match controls.primary() {
        PrimaryAction::Save => ("Save", false),
        PrimaryAction::Trash => ("Trash", true),
    };
    // Narrow enough to sit inside the card with its margins, wide enough to be a target.
    let pill_width = PILL_MAX_W.min(size.width - 2 * SCRIM_PAD - 8).max(48);

    let pills = gtk::Box::new(gtk::Orientation::Vertical, PILL_GAP);
    pills.set_halign(gtk::Align::Center);
    pills.set_valign(gtk::Align::Center);
    pills.set_vexpand(true);
    pills.append(&pill("copy", "Copy", false, pill_width));
    pills.append(&pill("primary", primary_label, destructive, pill_width));

    if tier == Tier::Full {
        let (edit_icon, edit_tip) = match controls.edit() {
            EditAction::Annotate => ("document-edit-symbolic", "Annotate"),
            EditAction::Trim => ("edit-cut-symbolic", "Trim"),
        };

        let top = gtk::CenterBox::new();
        top.set_start_widget(Some(&corner("close", "window-close-symbolic", "Close", true)));
        if controls.has_pin() {
            top.set_end_widget(Some(&corner("pin", "view-pin-symbolic", "Pin to the Screen", false)));
        }

        // The bottom-right corner is `spec/04` §1's Upload, and empty until there is
        // somewhere to upload to (`spec/11` M8): a control that can only say the feature is
        // missing is the one `spec/13` #19 rules out (D133).
        let bottom = gtk::CenterBox::new();
        if controls.has_edit() {
            bottom.set_start_widget(Some(&corner("edit", edit_icon, edit_tip, false)));
        }

        scrim.append(&top);
        scrim.append(&pills);
        scrim.append(&bottom);
    } else {
        scrim.append(&pills);
    }
    scrim
}

fn corner(name: &str, icon: &str, tooltip: &str, destructive: bool) -> gtk::Button {
    let button = gtk::Button::new();
    let image = gtk::Image::from_icon_name(&resolve_icon(icon));
    // In code, not CSS: `-gtk-icon-size` is honoured inconsistently across themes, and a
    // 16 pt icon in a 22 pt button leaves no ring of button around it to aim at.
    image.set_pixel_size(CORNER - 8);
    button.set_child(Some(&image));
    button.set_widget_name(name);
    // Tooltips on the icons only. The pills say what they do, and a tooltip reading
    // "Save" under a button labelled Save is a second, larger label covering the card.
    button.set_tooltip_text(Some(tooltip));
    button.add_css_class("octosnap-corner");
    if destructive {
        button.add_css_class("destructive");
    }
    button.set_size_request(CORNER, CORNER);
    button.set_valign(gtk::Align::Start);
    button.set_halign(gtk::Align::Start);
    button
}

fn pill(name: &str, label: &str, destructive: bool, width: i32) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.set_widget_name(name);
    button.add_css_class("octosnap-pill");
    if destructive {
        button.add_css_class("destructive");
    }
    button.set_size_request(width, PILL_H);
    button
}

/// The first of these icon names the theme actually has.
///
/// Icon names drift between Adwaita releases and between distributions -- `view-pin-symbolic`
/// is not in every icon theme that ships GNOME 50 -- and GTK renders a missing icon as the
/// broken-image glyph rather than nothing. A card with a broken-image square in its corner
/// looks like a bug in the card, not a gap in the icon theme.
fn resolve_icon(preferred: &str) -> String {
    let fallbacks: &[&str] = match preferred {
        "view-pin-symbolic" => &["view-pin-symbolic", "pin-symbolic", "starred-symbolic"],
        "send-to-symbolic" => &["send-to-symbolic", "document-send-symbolic", "go-up-symbolic"],
        "document-edit-symbolic" => &["document-edit-symbolic", "edit-symbolic"],
        "edit-cut-symbolic" => &["edit-cut-symbolic", "edit-select-all-symbolic"],
        other => &[other],
    };

    let theme = gdk::Display::default().map(|d| gtk::IconTheme::for_display(&d));
    if let Some(theme) = theme {
        for name in fallbacks {
            if theme.has_icon(name) {
                return (*name).to_owned();
            }
        }
    }
    preferred.to_owned()
}

/// Whether Alt is down right now.
///
/// Read at the moment the drop completes rather than remembered from the drag's start,
/// because `spec/04` §3's Alt is a decision the user can still change mid-drag -- the same
/// way Alt-drag works in a file manager.
pub(super) fn alt_held() -> bool {
    gdk::Display::default()
        .and_then(|display| display.default_seat())
        .and_then(|seat| seat.keyboard())
        .is_some_and(|keyboard| keyboard.modifier_state().contains(gdk::ModifierType::ALT_MASK))
}

/// The rect a capture's card occupies, for the extension's fly animation to aim at.
#[must_use]
pub fn card_rect(landed: Rect) -> Rect {
    let band = qao::SHADOW_MARGIN;
    Rect::new(
        landed.x + band,
        landed.y + band,
        (landed.width - 2 * band).max(1),
        (landed.height - 2 * band).max(1),
    )
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use octosnap_core::qao::{SIZE_STEPS, card_size};

    use super::*;

    /// `spec/04` §7's "any reason" against D47's one exception.
    ///
    /// Worth asserting because the failure is silent and the wrong way round: a pinned
    /// card filed in history gives "Restore recently closed" a second card for a capture
    /// already on the screen, and both of them look correct on their own.
    #[test]
    fn only_pinning_keeps_a_closed_card_out_of_history() {
        for reason in [
            CloseReason::UserClosed,
            CloseReason::Timeout,
            CloseReason::ActionTaken,
            CloseReason::Evicted,
            CloseReason::CloseAll,
        ] {
            assert!(reason.files_in_history(), "{reason:?} ends a capture's time on screen");
        }
        assert!(
            !CloseReason::Pinned.files_in_history(),
            "a pinned capture has moved to the screen, not closed; the pin hands it back"
        );
        assert!(
            !CloseReason::Reopened.files_in_history(),
            "a reopened card's picture is a render the open document supersedes (D108)"
        );
    }

    /// Every step of the size slider. `tier` may thin the controls out but must never
    /// claim a layout fits a card it would be clipped by -- that is the failure it exists
    /// to prevent.
    #[test]
    fn every_size_step_gets_a_layout_that_fits() {
        for step in 1..=SIZE_STEPS {
            let size = card_size(step);
            let needed = match tier(size) {
                Tier::Full => 2 * SCRIM_PAD + 2 * CORNER + 2 * PILL_H + PILL_GAP,
                Tier::Pills => 2 * SCRIM_PAD + 2 * PILL_H + PILL_GAP,
                Tier::Bare => 0,
            };
            assert!(
                size.height >= needed,
                "step {step} -> {size:?} cannot hold {:?}",
                tier(size)
            );
        }
    }

    /// The measured card from `spec/04` §1 is the one the six-control layout was drawn
    /// for, so it had better be the tier that shows all six.
    #[test]
    fn the_default_card_shows_every_control() {
        assert_eq!(tier(card_size(octosnap_core::qao::DEFAULT_SIZE_STEP)), Tier::Full);
    }

    /// The small end of the slider, where the six-control layout does not fit.
    #[test]
    fn the_smallest_step_thins_the_controls_out() {
        let size = card_size(1);
        assert_eq!(size, Size::new(120, 72));
        assert_eq!(tier(size), Tier::Pills, "72 pt of height cannot hold two corner rows");
    }

    /// Pills are never wider than the card they sit in.
    #[test]
    fn the_pill_never_overflows_its_card() {
        for step in 1..=SIZE_STEPS {
            let size = card_size(step);
            if tier(size) == Tier::Bare {
                continue;
            }
            let width = PILL_MAX_W.min(size.width - 2 * SCRIM_PAD - 8).max(48);
            assert!(
                width + 2 * SCRIM_PAD <= size.width,
                "step {step}: a {width} pt pill does not fit a {} pt card",
                size.width
            );
        }
    }
}
