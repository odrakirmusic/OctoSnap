// SPDX-License-Identifier: GPL-3.0-or-later

//! The Quick Access Overlay's arithmetic and its state machine (`spec/04`).
//!
//! Everything here is pure. The card is a GTK window and the stack is placed by the
//! extension, but *which* card is where, how big it is, when its timer may run and what
//! happens when it closes are all decisions that need no display to make and no display
//! to test -- and they are the decisions that go wrong. `spec/10` §11 asks for the app to
//! be drivable "through capture -> card -> actions without a shell"; this module is the
//! part of that path that can be exercised at full speed.
//!
//! Two numbers here also exist in the extension, in `slot.ts`. That duplication is
//! deliberate and load-bearing: the extension needs the card's size to know where to fly
//! the capture *before* the app has built a window, and the app needs it to size that
//! window. They have to agree exactly or the capture lands next to its card instead of
//! becoming it. [`SIZE_TABLE`] is the shared contract, and
//! `extension/src/overlay/slot.test.ts` asserts the same table from the other side.

use std::collections::VecDeque;

use crate::geometry::Rect;

/// `spec/04` §2, measured: distance from the screen edge, logical pixels.
pub const MARGIN: i32 = 16;

/// `spec/04` §2, measured: distance between stacked cards.
pub const GAP: i32 = 8;

/// The longest side of a card at the default size, from `spec/04` §1's measurement of
/// "≈ 207 × 125 pt for a 16:10 source".
pub const CARD_MAX: i32 = 207;

/// The card's shape: **the same for every capture**, from the same measurement.
///
/// `spec/04` §1 reads the 207 × 125 card as the capture's own shape scaled down, and §2
/// builds on that -- "a card takes its size from the capture", which is why cards in one
/// stack were allowed different heights. Riccardo asked for the opposite on 2026-09-08:
/// "make all preview blobs in the bottom left the same size, regardless of the true
/// picture size."
///
/// It is the better answer, and the reason is what a stack of cards is *for*. A card is a
/// handle to a capture, not a preview of its geometry: you reach for the second one down.
/// When every card is a different shape, "the second one down" is at a different place
/// every time, and a stack of a screenshot, a tall window and a thin strip is a ragged
/// column with nothing to aim at. Uniform cards make the stack a list.
///
/// The thumbnail therefore *fills* the card and is cropped, which is what `spec/04` §9
/// asked for all along: `Gtk.Picture` with `content-fit: cover`. That instruction only
/// makes sense for a card whose shape does not already match its capture.
/// `docs/decisions.md` D43.
pub const CARD_ASPECT: (i32, i32) = (207, 125);

/// The transparent band around the card that its drop shadow is drawn into
/// (`spec/04` §9). The window is this much bigger than the card on every side, and that
/// band is click-through.
///
/// **Equal to [`MARGIN`], and it may never exceed it.** The card has to sit 16 pt from
/// the screen edge because that is what `spec/04` §2 measured, so the window around it
/// starts exactly at the work area's edge. One pixel wider and the window would have to
/// begin outside the work area, where Mutter's silent clamp (`docs/spikes/01-02`) would
/// pull it back and take the card with it -- the measured margin would quietly become
/// whatever the clamp decided.
///
/// This also settles what the 8 pt inter-card gap measures. It is the distance between
/// two *cards*, not two windows, so consecutive windows overlap by `2 * SHADOW_MARGIN -
/// GAP`. Overlapping is free here: the bands are transparent, and each window's input
/// region is only its card (`spec/04` §9), so no card can swallow a click meant for its
/// neighbour.
pub const SHADOW_MARGIN: i32 = MARGIN;

/// The five-step overlay size slider (`spec/04` §1 [V]: "the Quick Access settings expose
/// size as a five-step slider rather than three named sizes").
///
/// `spec/08` §4 still lists `qao-size` as small/medium/large from documentation [D]. The
/// screenshot is later evidence than the documentation and wins; the three named sizes
/// map onto steps 2, 3 and 4 for anyone reading the old table.
pub const SIZE_STEPS: u8 = 5;

/// The default step: the one whose longest side is [`CARD_MAX`], because that is the size
/// the measurement in `spec/04` §1 was taken at.
pub const DEFAULT_SIZE_STEP: u8 = 3;

/// Longest side in logical pixels for each step, index = step - 1.
///
/// Geometric rather than linear, at roughly 1.32× per step. A linear ladder spends most
/// of its range on sizes that look the same: the difference between 190 and 207 pt is not
/// a setting anyone would move a slider for, while the difference between 120 and 350 is
/// the difference between a stamp and a preview. Five steps have to cover that whole
/// range, so each step multiplies.
pub const SIZE_TABLE: [i32; SIZE_STEPS as usize] = [120, 158, CARD_MAX, 273, 360];

/// The longest side for a size step, clamped into range.
///
/// Clamped rather than validated: this reads a GSettings value that a user can set with
/// `gsettings set` to anything the range allows, and a card that refuses to have a size
/// is worse than a card that is the nearest legal one.
#[must_use]
pub fn longest_side(step: u8) -> i32 {
    let index = usize::from(step.clamp(1, SIZE_STEPS)) - 1;
    SIZE_TABLE[index]
}

/// A width and height in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: i32,
    pub height: i32,
}

impl Size {
    #[must_use]
    pub const fn new(width: i32, height: i32) -> Self {
        Self { width, height }
    }
}

/// The size of every card at this step. The capture does not get a say -- see
/// [`CARD_ASPECT`].
#[must_use]
pub fn card_size(step: u8) -> Size {
    let long = longest_side(step);
    let (aw, ah) = CARD_ASPECT;
    #[allow(clippy::cast_possible_truncation)]
    let short = (f64::from(long) * f64::from(ah) / f64::from(aw)).round() as i32;
    Size::new(long.max(1), short.max(1))
}

/// The part of a card's window that takes clicks, in window coordinates.
///
/// `spec/04` §9: "Input region = the card's rounded rect (so the shadow margin is
/// click-through)". A rectangle rather than a rounded one, matching what the app actually
/// sets: the corner controls are inset from the card's edge by more than the card's own
/// corner radius, so rounding the region would remove no reachable pixel and would add a
/// second radius to keep in step with the stylesheet's.
///
/// The offset is [`SHADOW_MARGIN`] because that is where the card is *drawn* -- the window
/// is the card plus one band on every side. That single number is the whole contract
/// between this region and the widgets inside it, which is why it belongs in one place
/// rather than being arrived at twice.
#[must_use]
pub fn input_region(card: Size) -> Rect {
    Rect::new(SHADOW_MARGIN, SHADOW_MARGIN, card.width, card.height)
}

/// Whether a control drawn at `rect` can be clicked at all.
///
/// `rect` is in the card window's coordinates, with every transform between the control
/// and the window already applied -- which is the only form of the question worth asking.
/// A card's input region is [`input_region`] and nothing else, so a control outside it is
/// not a control: the click travels through to the desktop while the button sits there
/// looking pressable.
///
/// The card's counterpart to [`crate::pin::reachable_when_locked`], and it exists for the
/// same reason that one does. `docs/decisions.md` D39: the arrival animation's
/// `set_child_transform` *replaced* the position `gtk_fixed_put` had set, so the card's
/// body was drawn at the window's origin rather than one band in. The region was still
/// exactly right about where the card was supposed to be, the row of controls was still
/// exactly right about where it sat inside that card, and between the two the top-left
/// controls ended up over the click-through band -- dead, while the centre pills, further
/// in, kept working. Nothing in either computation was wrong; nothing in either
/// computation said the two had to agree.
#[must_use]
pub fn reachable(card: Size, rect: Rect) -> bool {
    let region = input_region(card);
    rect.x >= region.x
        && rect.y >= region.y
        && rect.x + rect.width <= region.x + region.width
        && rect.y + rect.height <= region.y + region.height
}

/// A source's shape fitted inside a `longest` square, **never enlarged**.
///
/// No longer what sizes a card -- cards are uniform now -- but still what the extension's
/// fly animation needs: it scales the captured image down toward the corner, and it has
/// to know how far. Not enlarging is a decision rather than an oversight, and it is the
/// same one `slot.ts` records: a 40×30 capture blown up to 207×155 would fly *backwards*,
/// growing on its way to the corner, which reads as "something opened" rather than "this
/// was filed away".
#[must_use]
pub fn fit(source: Rect, longest: i32) -> Size {
    let longest_source = source.width.max(source.height);
    if longest_source <= 0 || longest <= 0 {
        return Size::new(0, 0);
    }
    if longest_source <= longest {
        return Size::new(source.width.max(1), source.height.max(1));
    }
    let k = f64::from(longest) / f64::from(longest_source);
    Size::new(
        scale_axis(source.width, k),
        scale_axis(source.height, k),
    )
}

fn scale_axis(value: i32, k: f64) -> i32 {
    // `round`, matching `slot.ts`'s `Math.round`, and then a floor of 1: a 1000×3 capture
    // scaled to fit 207 gives 0.6 px of height, and a card with no height is invisible
    // rather than small.
    #[allow(clippy::cast_possible_truncation)]
    let scaled = (f64::from(value) * k).round() as i32;
    scaled.max(1)
}

/// Which screen edge the stack hugs (`spec/04` §2: "**Anchor is an edge, not a corner**").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Edge {
    #[default]
    Left,
    Right,
}

impl Edge {
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "left" => Self::Left,
            "right" => Self::Right,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

/// `spec/04` §6's auto-close interval. Zero is "never", which is the setting's own
/// encoding rather than a sentinel this module invented (`spec/08` §4: "`i`, 0 = never").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoClose(u32);

impl Default for AutoClose {
    /// **Never**, by the user's decision on 2026-09-08: "remove the auto-remove time
    /// when the preview blobs expire and disappear."
    ///
    /// `spec/04` §6 says "Default 10 s", marked `[M]` -- an inference, not a measurement
    /// of the reference app. An inference is exactly the kind of default a real user's
    /// preference should overrule, and this one had a foreseeable cost: a capture you
    /// glance away from is gone, and the only way back is a shortcut you have to know
    /// about. The interval is still there with all of `spec/04` §6's values; it is off
    /// until asked for. `docs/decisions.md` D38.
    fn default() -> Self {
        Self(0)
    }
}

impl AutoClose {
    /// The menu, in order (`spec/04` §6). `Never` is offered first because it is the
    /// choice that is qualitatively different from the rest.
    pub const CHOICES: [u32; 9] = [0, 5, 10, 15, 30, 60, 120, 300, 600];

    #[must_use]
    pub const fn seconds(self) -> u32 {
        self.0
    }

    #[must_use]
    pub const fn is_never(self) -> bool {
        self.0 == 0
    }

    /// Any non-negative value, snapped to the nearest offered choice.
    ///
    /// A value off the menu is not rejected: `spec/08`'s key is a plain integer, the menu
    /// is a UI convenience, and someone who sets 45 seconds by hand has expressed a
    /// preference that rounding to 30 honours better than falling back to the default.
    #[must_use]
    pub fn snapped(seconds: u32) -> Self {
        if seconds == 0 {
            return Self(0);
        }
        let nearest = Self::CHOICES
            .iter()
            .skip(1)
            .copied()
            .min_by_key(|c| c.abs_diff(seconds))
            .unwrap_or(10);
        Self(nearest)
    }

    /// The exact value, for a settings round trip that must not drift.
    #[must_use]
    pub const fn exact(seconds: u32) -> Self {
        Self(seconds)
    }
}

/// Why a card's auto-close timer is not running.
///
/// A set of reasons rather than a boolean, because they overlap: the pointer can leave a
/// card whose context menu is still open, and a single `paused` flag would then resume a
/// timer under an open menu. `spec/04` §6 lists four of them and they can all be true at
/// once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Holds {
    pub hovered: bool,
    pub menu_open: bool,
    pub uploading: bool,
    pub dragging: bool,
    /// A GIF being written for the card's Copy or Save (D113). Timed out before it is
    /// written, the card would take its "Writing GIF…" badge with it and leave nothing on
    /// screen to say a file is on its way.
    pub rendering: bool,
}

impl Holds {
    /// No holds; a `const` twin of `Default` so [`CloseTimer::new`] can stay `const`.
    pub const NONE: Self = Self {
        hovered: false,
        menu_open: false,
        uploading: false,
        dragging: false,
        rendering: false,
    };

    #[must_use]
    pub const fn any(self) -> bool {
        self.hovered || self.menu_open || self.uploading || self.dragging || self.rendering
    }
}

/// A card's auto-close timer.
///
/// Holds elapsed time rather than a deadline. A deadline would have to be pushed forward
/// every time a hold is released, which loses the time already served -- hover for a
/// second, leave, and the card would restart its ten rather than finish its nine. The
/// progress hairline in `spec/04` §6 makes that difference visible, so it has to be right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloseTimer {
    interval: AutoClose,
    elapsed_ms: u32,
    holds: Holds,
}

impl CloseTimer {
    #[must_use]
    pub const fn new(interval: AutoClose) -> Self {
        Self { interval, elapsed_ms: 0, holds: Holds::NONE }
    }

    #[must_use]
    pub const fn holds(&self) -> Holds {
        self.holds
    }

    pub const fn set_holds(&mut self, holds: Holds) {
        self.holds = holds;
    }

    /// Advances by `ms` and reports whether the card should now close.
    ///
    /// Never fires while a hold is set, and never fires at all when the interval is
    /// "never" -- both checked here rather than by the caller, so a caller that forgets to
    /// stop ticking cannot close a card the user is hovering.
    pub fn tick(&mut self, ms: u32) -> bool {
        if self.interval.is_never() || self.holds.any() {
            return false;
        }
        self.elapsed_ms = self.elapsed_ms.saturating_add(ms);
        self.elapsed_ms >= self.interval.seconds().saturating_mul(1000)
    }

    /// Whether this card is counting down at all.
    ///
    /// The hairline asks, because `progress()` cannot answer it: "never" reports 0.0
    /// progress, which is indistinguishable from "just arrived" and drew a **full-width
    /// bar on every card, permanently**. The comment beside that code claimed it was
    /// hidden; the code did not do it. With "never" now the default, it would have been
    /// on every card in the product.
    #[must_use]
    pub const fn counts_down(&self) -> bool {
        !self.interval.is_never()
    }

    /// How much of the interval has run, 0.0 to 1.0, for the progress hairline.
    #[must_use]
    pub fn progress(&self) -> f64 {
        let total = self.interval.seconds().saturating_mul(1000);
        if total == 0 {
            return 0.0;
        }
        (f64::from(self.elapsed_ms) / f64::from(total)).clamp(0.0, 1.0)
    }
}

/// What a card is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CardKind {
    #[default]
    Image,
    /// `spec/04` §1: a GIF card shows the first frame with a duration-and-size badge, and
    /// offers Copy, Save, drag-out, Trash and **Trim** -- the GIF editor (preview, a
    /// range, save) in the corner Annotate has on an image card. No Pin, because a
    /// looping GIF is not sensibly pinned (M5, D68).
    Gif,
    /// `spec/04` §1: video cards swap *Annotate* for **Trim**, add a **GIF** action, and
    /// carry a duration + file size badge. M9 fills this in; the shape is here so the
    /// card widget is not rewritten to accept it.
    Video,
}

/// The six controls of `spec/04` §1's hover state, resolved for one card.
///
/// Computed rather than hard-coded because two of the six change identity: `spec/04` §1
/// says Trash "replaces *Save* when the after-capture action already saved the file"
/// [D 4.5], and a video card shows Trim where an image card shows Annotate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Controls {
    pub kind: CardKind,
    /// True when the file already exists outside the spool, so Save has nothing to do.
    pub already_saved: bool,
}

/// The centre pill that sits below Copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimaryAction {
    Save,
    Trash,
}

/// The bottom-left corner control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditAction {
    Annotate,
    Trim,
}

impl Controls {
    #[must_use]
    pub const fn primary(self) -> PrimaryAction {
        if self.already_saved { PrimaryAction::Trash } else { PrimaryAction::Save }
    }

    /// The bottom-left corner: Annotate draws on a still; Trim opens a recording's
    /// editor, GIF or video alike (`spec/04` §1's "swap *Annotate* for **Trim**").
    #[must_use]
    pub const fn edit(self) -> EditAction {
        match self.kind {
            CardKind::Image => EditAction::Annotate,
            CardKind::Gif | CardKind::Video => EditAction::Trim,
        }
    }

    /// Whether the top-right Pin corner is offered. A GIF is not pinned (D68).
    #[must_use]
    pub const fn has_pin(self) -> bool {
        !matches!(self.kind, CardKind::Gif)
    }

    /// Whether the bottom-left edit corner (Annotate or Trim) is offered. Every kind has
    /// one now that a GIF has its editor (M5); kept as a method so the card asks one
    /// question in one place, and so a kind without an editor can say so again.
    #[must_use]
    pub const fn has_edit(self) -> bool {
        matches!(self.kind, CardKind::Image | CardKind::Gif | CardKind::Video)
    }

    /// Whether the card carries `spec/04` §1's duration-and-size badge.
    #[must_use]
    pub const fn shows_badge(self) -> bool {
        matches!(self.kind, CardKind::Gif | CardKind::Video)
    }

    /// Whether `spec/04` §4's "Open in Text recognition (OCR)" is offered.
    ///
    /// Stills only. A recording's card holds the first frame as a thumbnail, not as a
    /// file the reader could open, and text in a moving picture is not one page of text.
    #[must_use]
    pub const fn has_text(self) -> bool {
        matches!(self.kind, CardKind::Image)
    }
}

/// A card's identity. Monotonic per session, so a closed card's id is never reused and a
/// stale reference resolves to nothing rather than to somebody else's card.
pub type CardId = u64;

/// One card's contribution to the stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub id: CardId,
    /// The **card's** height, with no shadow band.
    ///
    /// Card space rather than window space throughout, because that is the space
    /// `spec/04` §2's numbers were measured in: 16 pt from the edge and 8 pt between
    /// cards are distances between the things you can see. The window is derived from the
    /// card by [`SHADOW_MARGIN`], and the extension does that conversion at placement
    /// time so that only one side of the boundary has to know about it.
    pub height: i32,
}

/// The ordered stack of cards against one edge.
///
/// Newest first, which is the order the geometry wants: `spec/04` §2's stack is anchored
/// at the bottom and grows upward, so index 0 is the card nearest the anchor and every
/// later card is pushed away from it. Keeping the vector in that order means an insert is
/// at the front and the offsets are a running sum, with no reversal anywhere.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stack {
    slots: Vec<Slot>,
}

impl Stack {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    #[must_use]
    pub fn ids(&self) -> Vec<CardId> {
        self.slots.iter().map(|s| s.id).collect()
    }

    /// Puts a new card nearest the anchor, pushing the rest away from it.
    pub fn push_newest(&mut self, slot: Slot) {
        self.slots.insert(0, slot);
    }

    /// Drops a card and lets its neighbours collapse the gap (`spec/04` §5).
    ///
    /// Returns whether it was there, so a double close -- the timer firing on the same
    /// turn as a click, which is exactly what a 10 s timer and an impatient user produce
    /// -- is a no-op rather than a panic or a stale reflow.
    pub fn remove(&mut self, id: CardId) -> bool {
        let before = self.slots.len();
        self.slots.retain(|s| s.id != id);
        self.slots.len() != before
    }

    /// A card's height changed (a thumbnail loaded, a video card gained its badge).
    pub fn resize(&mut self, id: CardId, height: i32) -> bool {
        for slot in &mut self.slots {
            if slot.id == id {
                slot.height = height;
                return true;
            }
        }
        false
    }

    /// The distance from the anchor to each card, in the same order as [`Self::ids`].
    ///
    /// This is the number `PlaceWindow`'s `offset` option takes, and the reason it is a
    /// distance rather than an index: cards take their size from their captures, so the
    /// third card's position depends on how tall the first two happened to be.
    #[must_use]
    pub fn offsets(&self) -> Vec<i32> {
        let mut running = 0;
        let mut out = Vec::with_capacity(self.slots.len());
        for slot in &self.slots {
            out.push(running);
            running += slot.height + GAP;
        }
        out
    }

    /// Every card with the offset it should now sit at.
    #[must_use]
    pub fn placements(&self) -> Vec<(CardId, i32)> {
        self.ids().into_iter().zip(self.offsets()).collect()
    }

    /// The height the visible stack occupies, margins included.
    #[must_use]
    pub fn extent(&self) -> i32 {
        if self.slots.is_empty() {
            return 0;
        }
        let total: i32 = self.slots.iter().map(|s| s.height).sum::<i32>()
            + GAP * (i32::try_from(self.slots.len()).unwrap_or(i32::MAX) - 1);
        total + 2 * MARGIN
    }

    /// Whether the whole stack still fits between the work area's margins.
    #[must_use]
    pub fn fits(&self, work_area_height: i32) -> bool {
        self.extent() <= work_area_height
    }

    /// The cards, oldest first, that must go for a card of `height` to fit.
    ///
    /// `spec/04` §2 saw six cards at once and recorded that there is no "+N more"
    /// collapse, so there is nothing to fold away -- but six default-size cards need
    /// 846 pt and a 1366×768 laptop offers 736, so on a small display the stack does run
    /// out of room. Something has to give, and the choice is between three bad options
    /// and one honest one:
    ///
    /// - Let it overflow. Mutter clamps the top window onto its neighbour and the stack
    ///   silently starts lying about how many captures there are.
    /// - Shrink the cards. The size slider stops meaning anything at exactly the moment
    ///   the user would notice.
    /// - Refuse the new card. The capture the user just took is the one they care about.
    /// - **Close the oldest**, which is what this returns.
    ///
    /// Closing is not losing: `spec/04` §7 sends a closed card to history either way, and
    /// "Restore recently closed" brings it back. So an evicted card has taken exactly the
    /// path it would have taken had its auto-close timer fired a moment earlier.
    #[must_use]
    pub fn evict_for(&self, work_area_height: i32, height: i32) -> Vec<CardId> {
        let mut trial = self.clone();
        trial.push_newest(Slot { id: CardId::MAX, height });
        let mut evicted = Vec::new();
        // Never evict the newcomer itself: a card taller than the whole work area is
        // placed and clamped rather than refused, because refusing it would throw away
        // the capture the user just asked for.
        while trial.slots.len() > 1 && !trial.fits(work_area_height) {
            if let Some(oldest) = trial.slots.pop() {
                evicted.push(oldest.id);
            }
        }
        evicted
    }
}

/// `spec/04` §3's "Restore recently closed".
///
/// A bounded queue of what was on screen, so the shortcut can bring back the last card
/// even after the stack has emptied. Bounded because this is a convenience, not the
/// History window (`spec/07` §4) -- the file is in history either way, and an unbounded
/// list here would be a second, worse history that nobody can see.
#[derive(Debug, Clone)]
pub struct RecentlyClosed<T> {
    items: VecDeque<T>,
    capacity: usize,
}

impl<T> RecentlyClosed<T> {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self { items: VecDeque::new(), capacity: capacity.max(1) }
    }

    pub fn push(&mut self, item: T) {
        self.items.push_front(item);
        while self.items.len() > self.capacity {
            self.items.pop_back();
        }
    }

    /// The most recently closed card, removed from the list.
    pub fn pop(&mut self) -> Option<T> {
        self.items.pop_front()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl<T> Default for RecentlyClosed<T> {
    fn default() -> Self {
        Self::new(10)
    }
}

/// `spec/04` §3's "Moves the saved file to the trash after a 3 s undo toast".
pub const TRASH_GRACE_MS: u32 = 3000;

/// The captures whose deletion has been asked for and not yet carried out.
///
/// The toast **is** the grace period, so nothing is deleted while it is up. `spec/04` §3
/// puts the undo before the move, and the alternative -- trash first, restore on undo --
/// cannot be honoured: GIO can put a file in the XDG trash and offers no way to take it
/// back out, so "undo" would mean asking the user to open Files. Waiting costs three
/// seconds of nothing and makes the undo exact.
///
/// A queue rather than one slot, because trashing two cards inside one grace period is an
/// ordinary thing to do and the second must not silently cancel the first. Each entry is
/// named by a token rather than by its position, because the entry ahead of it can be
/// carried out while this one is still waiting -- so an index is not a name.
#[derive(Debug, Clone)]
pub struct PendingTrash<T> {
    items: Vec<Pending<T>>,
    next: u64,
}

#[derive(Debug, Clone)]
struct Pending<T> {
    token: u64,
    item: T,
}

impl<T> PendingTrash<T> {
    #[must_use]
    pub fn new() -> Self {
        Self { items: Vec::new(), next: 1 }
    }

    /// Queues a capture for deletion, answering with the token that names this one.
    pub fn push(&mut self, item: T) -> u64 {
        let token = self.next;
        self.next += 1;
        self.items.push(Pending { token, item });
        token
    }

    /// Removes one entry and hands it over. What happens to it next is the caller's
    /// intent, not this type's: the grace period taking it means delete, the undo button
    /// taking it means restore.
    ///
    /// `None` is not an error. A click landing in the same turn as the timeout is
    /// ordinary, and whichever arrives second must find nothing rather than acting twice
    /// on one capture.
    pub fn take(&mut self, token: u64) -> Option<T> {
        let at = self.items.iter().position(|pending| pending.token == token)?;
        Some(self.items.remove(at).item)
    }

    /// The newest entry, for an undo that has no token to offer.
    pub fn take_newest(&mut self) -> Option<T> {
        self.items.pop().map(|pending| pending.item)
    }

    /// Everything still waiting, oldest first, emptying the queue.
    ///
    /// For shutdown: a pending deletion at quit has to resolve one way or the other, and
    /// the one the user can still undo is the one that has not happened.
    pub fn drain(&mut self) -> Vec<T> {
        self.items.drain(..).map(|pending| pending.item).collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl<T> Default for PendingTrash<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// `spec/04` §1's measurement, exactly: "≈ 207 × 125 pt".
    #[test]
    fn the_default_step_matches_the_measurement() {
        assert_eq!(card_size(DEFAULT_SIZE_STEP), Size::new(207, 125));
    }

    /// The point of D43: the capture's shape does not reach the card.
    #[test]
    fn every_capture_gets_the_same_card() {
        let shapes = [
            Rect::new(0, 0, 1600, 1000),
            Rect::new(0, 0, 100, 4000),
            Rect::new(0, 0, 4000, 100),
            Rect::new(0, 0, 7, 7),
        ];
        let first = card_size(DEFAULT_SIZE_STEP);
        for shape in shapes {
            let _ = shape;
            assert_eq!(card_size(DEFAULT_SIZE_STEP), first);
        }
        // And at every step the shape is the measured one, within a pixel of rounding.
        for step in 1..=SIZE_STEPS {
            let size = card_size(step);
            let expected = f64::from(size.width) * 125.0 / 207.0;
            assert!(
                (f64::from(size.height) - expected).abs() <= 0.5,
                "step {step}: {size:?} is not 207:125"
            );
        }
    }


    /// `spec/04` §9's region: the card, one shadow band in from the window's origin.
    #[test]
    fn the_input_region_is_the_card_inside_its_shadow_band() {
        let card = card_size(DEFAULT_SIZE_STEP);
        assert_eq!(input_region(card), Rect::new(SHADOW_MARGIN, SHADOW_MARGIN, 207, 125));
        // Which is the window minus one band on every side, at every step.
        for step in 1..=SIZE_STEPS {
            let card = card_size(step);
            let region = input_region(card);
            let window = Size::new(card.width + 2 * SHADOW_MARGIN, card.height + 2 * SHADOW_MARGIN);
            assert_eq!(region.x + region.width + SHADOW_MARGIN, window.width);
            assert_eq!(region.y + region.height + SHADOW_MARGIN, window.height);
        }
    }

    /// The scrim's inset and the side of a corner control, from `qao::card`'s `SCRIM_PAD`
    /// and `CORNER`. Duplicated here rather than hoisted into this crate because the card
    /// does not place its controls by arithmetic at all -- a `GtkCenterBox` does it -- so
    /// there is no number for core to own. These two only have to be *plausible* for the
    /// tests below, which are about the band, not about the button.
    const PAD: i32 = 6;
    const BTN: i32 = 22;

    /// `spec/04` §1's four corner positions, for a card body drawn with its top-left at
    /// `origin` in window coordinates. The bottom-right one is Upload's, empty on the card
    /// until there is somewhere to upload to (D133); the band has to reach it all the same.
    fn corner_controls(origin: (i32, i32), card: Size) -> [(&'static str, Rect); 4] {
        let (ox, oy) = origin;
        let right = ox + card.width - PAD - BTN;
        let bottom = oy + card.height - PAD - BTN;
        [
            ("close", Rect::new(ox + PAD, oy + PAD, BTN, BTN)),
            ("pin", Rect::new(right, oy + PAD, BTN, BTN)),
            ("edit", Rect::new(ox + PAD, bottom, BTN, BTN)),
            ("upload", Rect::new(right, bottom, BTN, BTN)),
        ]
    }

    /// Where the card is supposed to be: every control inside the region, at every step
    /// big enough to draw them.
    #[test]
    fn a_card_drawn_in_its_own_band_can_be_clicked_everywhere() {
        for step in 1..=SIZE_STEPS {
            let card = card_size(step);
            for (name, rect) in corner_controls((SHADOW_MARGIN, SHADOW_MARGIN), card) {
                assert!(
                    reachable(card, rect),
                    "step {step}: {name} at {rect:?} is outside {:?}",
                    input_region(card)
                );
            }
        }
    }

    /// D39, exactly as it happened. `set_child_transform` replaced the position
    /// `gtk_fixed_put` had set, so the body was drawn at the **window's origin** instead of
    /// one band in. The region was still the card's own rect and still correct about it;
    /// the controls were still correctly placed inside the body. Between them, the two
    /// controls along the top and the two along the left fell into the click-through band.
    ///
    /// Which is what made it so hard to read: the top-left corner is out by a whole band on
    /// both axes, so *three* of the four corners went dead -- and the centre pills, further
    /// in than any of them, kept working perfectly. It reads as "the corner buttons are
    /// broken", not as "the card is in the wrong place".
    #[test]
    fn a_card_drawn_at_the_window_origin_loses_its_corner_controls() {
        let card = card_size(DEFAULT_SIZE_STEP);
        let dead: Vec<&str> = corner_controls((0, 0), card)
            .into_iter()
            .filter(|(_, rect)| !reachable(card, *rect))
            .map(|(name, _)| name)
            .collect();
        assert_eq!(dead, ["close", "pin", "edit"], "the corners a shifted body loses");

        // And the pills, which is why the card did not look misplaced. The centre of a
        // 207x125 card is 16 px from nothing.
        let pill = Rect::new(SHADOW_MARGIN + 60, SHADOW_MARGIN + 51, 88, 22);
        let shifted = Rect::new(pill.x - SHADOW_MARGIN, pill.y - SHADOW_MARGIN, pill.width, pill.height);
        assert!(reachable(card, shifted), "the pills survived the same shift");
    }

    /// One pixel is enough, in any direction. A control that is *mostly* inside the region
    /// has a strip along one edge that silently does nothing, and a button that works when
    /// you aim well is worse to use than one that never works at all.
    #[test]
    fn a_control_one_pixel_over_any_edge_is_not_reachable() {
        let card = card_size(DEFAULT_SIZE_STEP);
        let region = input_region(card);
        let inside = Rect::new(region.x, region.y, BTN, BTN);
        assert!(reachable(card, inside));
        assert!(!reachable(card, Rect::new(inside.x - 1, inside.y, inside.width, inside.height)));
        assert!(!reachable(card, Rect::new(inside.x, inside.y - 1, inside.width, inside.height)));

        let far = Rect::new(
            region.x + region.width - BTN,
            region.y + region.height - BTN,
            BTN,
            BTN,
        );
        assert!(reachable(card, far));
        assert!(!reachable(card, Rect::new(far.x + 1, far.y, far.width, far.height)));
        assert!(!reachable(card, Rect::new(far.x, far.y + 1, far.width, far.height)));
    }

    /// A control that fills the card exactly is reachable, and one that fills the *window*
    /// is not -- which is the shape of every "why is the shadow band eating my clicks"
    /// question. The band is 16 px of window that belongs to the desktop.
    #[test]
    fn the_band_belongs_to_the_desktop() {
        let card = card_size(DEFAULT_SIZE_STEP);
        assert!(reachable(card, input_region(card)));
        assert!(!reachable(
            card,
            Rect::new(0, 0, card.width + 2 * SHADOW_MARGIN, card.height + 2 * SHADOW_MARGIN)
        ));
    }

    /// The same table the extension's `slot.ts` uses for the fly animation. If these two
    /// drift, the capture flies to a rect that is not where its card appears.
    #[test]
    fn fit_matches_the_extensions_card_size() {
        // Cases copied from `extension/src/overlay/slot.test.ts`.
        assert_eq!(fit(Rect::new(0, 0, 1600, 1000), 207), Size::new(207, 129));
        assert_eq!(fit(Rect::new(0, 0, 1000, 1600), 207), Size::new(129, 207));
        assert_eq!(fit(Rect::new(0, 0, 400, 400), 207), Size::new(207, 207));
        // Never enlarged.
        assert_eq!(fit(Rect::new(0, 0, 40, 30), 207), Size::new(40, 30));
    }

    #[test]
    fn a_card_never_loses_an_axis_entirely() {
        // 3 px of height scaled by 207/1000 is 0.62, which rounds to 1, not 0.
        let size = fit(Rect::new(0, 0, 1000, 3), 207);
        assert_eq!(size.width, 207);
        assert_eq!(size.height, 1);
    }

    #[test]
    fn a_degenerate_capture_has_no_card() {
        assert_eq!(fit(Rect::new(0, 0, 0, 0), 207), Size::new(0, 0));
    }

    #[test]
    fn the_size_slider_is_monotonic_and_clamped() {
        for step in 1..SIZE_STEPS {
            assert!(
                longest_side(step) < longest_side(step + 1),
                "step {step} is not smaller than {}",
                step + 1
            );
        }
        // Out of range in both directions rather than panicking on an index.
        assert_eq!(longest_side(0), SIZE_TABLE[0]);
        assert_eq!(longest_side(99), SIZE_TABLE[SIZE_STEPS as usize - 1]);
        assert_eq!(longest_side(DEFAULT_SIZE_STEP), CARD_MAX);
    }

    /// The user asked for cards that stay until they are dismissed (D38).
    #[test]
    fn cards_do_not_close_themselves_by_default() {
        assert!(AutoClose::default().is_never());
        let mut timer = CloseTimer::new(AutoClose::default());
        assert!(!timer.counts_down());
        for _ in 0..10_000 {
            assert!(!timer.tick(1000), "a default card must never close itself");
        }
    }

    #[test]
    fn auto_close_snaps_to_the_menu_but_keeps_never() {
        assert!(AutoClose::snapped(0).is_never());
        assert_eq!(AutoClose::snapped(45).seconds(), 30);
        assert_eq!(AutoClose::snapped(11).seconds(), 10);
        assert_eq!(AutoClose::snapped(100_000).seconds(), 600);
        // 1 second is closer to 5 than to never: snapping must not silently disable
        // auto-close for someone who asked for a very short one.
        assert_eq!(AutoClose::snapped(1).seconds(), 5);
        // The default is asserted in `cards_do_not_close_themselves_by_default`; it is
        // not 10 any more, and snapping has nothing to do with it either way.
    }

    #[test]
    fn the_timer_fires_once_the_interval_has_run() {
        let mut timer = CloseTimer::new(AutoClose::exact(10));
        for _ in 0..99 {
            assert!(!timer.tick(100));
        }
        assert!(timer.tick(100), "10 s of 100 ms ticks should fire");
    }

    /// The point of holding elapsed time rather than a deadline: hovering must not give
    /// the card its whole interval back.
    #[test]
    fn a_hold_pauses_rather_than_restarting() {
        let mut timer = CloseTimer::new(AutoClose::exact(10));
        for _ in 0..90 {
            timer.tick(100);
        }
        assert!((timer.progress() - 0.9).abs() < 1e-9);

        timer.set_holds(Holds { hovered: true, ..Holds::default() });
        for _ in 0..100 {
            assert!(!timer.tick(100), "a hovered card must never close");
        }
        assert!((timer.progress() - 0.9).abs() < 1e-9, "the hold must not consume time");

        timer.set_holds(Holds::default());
        for _ in 0..9 {
            assert!(!timer.tick(100));
        }
        assert!(timer.tick(100), "the remaining second, not another ten");
    }

    #[test]
    fn never_never_fires() {
        let mut timer = CloseTimer::new(AutoClose::exact(0));
        for _ in 0..10_000 {
            assert!(!timer.tick(1000));
        }
        assert_eq!(timer.progress(), 0.0);
        assert!(!timer.counts_down(), "a hairline that never moves looks broken");
    }

    /// Overlapping holds: the reason `Holds` is a set and not a boolean.
    #[test]
    fn leaving_a_card_with_its_menu_open_does_not_resume_it() {
        let mut timer = CloseTimer::new(AutoClose::exact(5));
        timer.set_holds(Holds { hovered: true, menu_open: true, ..Holds::default() });
        assert!(!timer.tick(10_000));
        // Pointer leaves; the menu is still up.
        timer.set_holds(Holds { hovered: false, menu_open: true, ..Holds::default() });
        assert!(!timer.tick(10_000));
        // Menu dismissed.
        timer.set_holds(Holds::default());
        assert!(timer.tick(5_000));
    }

    #[test]
    fn controls_swap_save_for_trash_once_the_file_exists() {
        let fresh = Controls { kind: CardKind::Image, already_saved: false };
        assert_eq!(fresh.primary(), PrimaryAction::Save);
        assert_eq!(fresh.edit(), EditAction::Annotate);

        let saved = Controls { kind: CardKind::Image, already_saved: true };
        assert_eq!(saved.primary(), PrimaryAction::Trash);

        let video = Controls { kind: CardKind::Video, already_saved: false };
        assert_eq!(video.edit(), EditAction::Trim);
    }

    fn slot(id: CardId, height: i32) -> Slot {
        Slot { id, height }
    }

    #[test]
    fn the_stack_grows_from_the_anchor_outward() {
        let mut stack = Stack::new();
        stack.push_newest(slot(1, 100));
        stack.push_newest(slot(2, 50));
        stack.push_newest(slot(3, 200));

        // Newest first: 3 is nearest the anchor.
        assert_eq!(stack.ids(), vec![3, 2, 1]);
        assert_eq!(stack.offsets(), vec![0, 200 + GAP, 200 + GAP + 50 + GAP]);
    }

    /// `spec/04` §10 item 5: "closing the middle one collapses the gap".
    #[test]
    fn closing_the_middle_card_collapses_the_gap() {
        let mut stack = Stack::new();
        stack.push_newest(slot(1, 100));
        stack.push_newest(slot(2, 50));
        stack.push_newest(slot(3, 200));

        assert!(stack.remove(2));
        assert_eq!(stack.ids(), vec![3, 1]);
        assert_eq!(stack.offsets(), vec![0, 200 + GAP]);
    }

    #[test]
    fn closing_a_card_twice_is_not_an_error() {
        let mut stack = Stack::new();
        stack.push_newest(slot(1, 100));
        assert!(stack.remove(1));
        assert!(!stack.remove(1), "the timer and a click can race");
        assert!(stack.is_empty());
    }

    /// An index could not do this, which is why `PlaceWindow` takes a distance.
    ///
    /// A card's offset is the sum of the heights *between it and the anchor*, which in a
    /// newest-first stack means the cards taken after it. So the same index sits at two
    /// different distances depending on what arrived later, and the oldest card's
    /// position is the one that moves most.
    #[test]
    fn offsets_depend_on_heights_not_on_position() {
        let mut tall = Stack::new();
        tall.push_newest(slot(1, 50));
        tall.push_newest(slot(2, 300));

        let mut short = Stack::new();
        short.push_newest(slot(1, 50));
        short.push_newest(slot(2, 30));

        // Index 0 is the anchor in both: nothing is below it.
        assert_eq!(tall.offsets()[0], 0);
        assert_eq!(short.offsets()[0], 0);

        // Index 1 is card 1 in both -- same card, same index, different distance,
        // because the card that landed on top of it is not the same height.
        assert_eq!(tall.offsets()[1], 300 + GAP);
        assert_eq!(short.offsets()[1], 30 + GAP);
        assert_ne!(tall.offsets()[1], short.offsets()[1]);
    }

    #[test]
    fn a_resized_card_moves_its_neighbours() {
        let mut stack = Stack::new();
        stack.push_newest(slot(1, 100));
        stack.push_newest(slot(2, 100));
        assert_eq!(stack.offsets(), vec![0, 108]);

        assert!(stack.resize(2, 40));
        assert_eq!(stack.offsets(), vec![0, 48]);
        assert!(!stack.resize(99, 10));
    }

    /// `spec/04` §2: "Six captures produced six cards, all visible at once. There is no
    /// '+N more' collapse." So six has to fit, and on the smallest display anyone is
    /// likely to have this on -- not just on a large one.
    #[test]
    fn six_cards_fit_a_normal_work_area() {
        let mut stack = Stack::new();
        for id in 1..=6 {
            stack.push_newest(slot(id, 129));
        }
        // 1080p less GNOME's top panel.
        assert!(stack.fits(1080 - 32), "six default-size cards must fit a 1080p work area");
        // 6 * 129 + 5 * 8 + 2 * 16
        assert_eq!(stack.extent(), 846);
    }

    /// The same six cards on a 1366x768 laptop, where they do not fit. The stack must
    /// say so rather than overflow, and the answer is the oldest card, not the newest.
    #[test]
    fn a_small_screen_evicts_the_oldest_card_rather_than_overflowing() {
        let mut stack = Stack::new();
        for id in 1..=5 {
            stack.push_newest(slot(id, 129));
        }
        let work_area = 768 - 32;
        assert!(stack.fits(work_area), "five fit");

        // The sixth does not: 846 > 736, so card 1 -- the oldest -- goes.
        assert_eq!(stack.evict_for(work_area, 129), vec![1]);

        // A very tall card takes several with it.
        assert_eq!(stack.evict_for(work_area, 400), vec![1, 2, 3]);
    }

    /// A capture bigger than the screen is still the capture the user just took.
    #[test]
    fn an_oversized_card_is_never_the_one_evicted() {
        let mut stack = Stack::new();
        stack.push_newest(slot(1, 129));
        // Taller than the work area on its own.
        assert_eq!(stack.evict_for(400, 5_000), vec![1]);
        assert_eq!(Stack::new().evict_for(400, 5_000), Vec::<CardId>::new());
    }

    /// The reason [`SHADOW_MARGIN`] is capped at [`MARGIN`]: a wider band would put the
    /// window outside the work area, and Mutter's silent clamp would move the card.
    #[test]
    fn the_shadow_band_never_pushes_a_window_out_of_the_work_area() {
        const {
            assert!(
                SHADOW_MARGIN <= MARGIN,
                "a shadow band wider than the margin would be clamped by Mutter"
            );
        }
    }

    #[test]
    fn the_stack_reports_when_it_has_run_out_of_room() {
        let mut stack = Stack::new();
        for id in 1..=6 {
            stack.push_newest(slot(id, 200));
        }
        // 6 * 200 + 5 * 8 + 32 = 1272
        assert!(stack.fits(1272));
        assert!(!stack.fits(1271));
        assert!(Stack::new().fits(0), "an empty stack always fits");
    }

    /// The undo button has no token to offer, so it takes the newest -- and the newest
    /// is the one whose toast is on screen.
    #[test]
    fn undo_takes_back_the_capture_the_toast_is_about() {
        let mut pending = PendingTrash::new();
        pending.push("first");
        pending.push("second");
        assert_eq!(pending.take_newest(), Some("second"));
        assert_eq!(pending.take_newest(), Some("first"));
        assert_eq!(pending.take_newest(), None);
    }

    /// Two cards trashed inside one grace period. The second must not cancel the first,
    /// which is the whole reason this is a queue and not a slot.
    #[test]
    fn a_second_trash_does_not_swallow_the_first() {
        let mut pending = PendingTrash::new();
        let first = pending.push("first");
        let second = pending.push("second");
        assert_ne!(first, second, "each pending deletion needs its own name");
        assert_eq!(pending.take(first), Some("first"), "the older one is still there");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending.take(second), Some("second"));
    }

    /// A click and a timeout landing in the same turn. Whichever is second finds nothing,
    /// so one capture cannot be both deleted and restored.
    #[test]
    fn a_capture_resolves_exactly_once() {
        let mut pending = PendingTrash::new();
        let token = pending.push("only");
        assert_eq!(pending.take(token), Some("only"));
        assert_eq!(pending.take(token), None, "the second caller must find it gone");
        assert!(pending.is_empty());
    }

    /// A token names an entry, not a position. The entry ahead can be carried out while
    /// this one is still waiting, and an index would then point at the wrong capture.
    #[test]
    fn a_token_survives_the_entry_in_front_of_it_going() {
        let mut pending = PendingTrash::new();
        let first = pending.push("first");
        let second = pending.push("second");
        let third = pending.push("third");
        assert_eq!(pending.take(first), Some("first"));
        assert_eq!(pending.take(third), Some("third"));
        assert_eq!(pending.take(second), Some("second"), "the middle one, by name");
    }

    /// At quit, a deletion that has not happened yet has not happened: the file stays.
    #[test]
    fn draining_hands_back_everything_still_waiting() {
        let mut pending = PendingTrash::new();
        pending.push("first");
        pending.push("second");
        assert_eq!(pending.drain(), vec!["first", "second"]);
        assert!(pending.is_empty());
        assert!(pending.drain().is_empty());
    }

    /// `spec/04` §3's number, asserted so a change to it is a deliberate one.
    #[test]
    fn the_grace_period_is_the_three_seconds_the_spec_asks_for() {
        assert_eq!(TRASH_GRACE_MS, 3000);
    }

    #[test]
    fn recently_closed_is_newest_first_and_bounded() {
        let mut recent = RecentlyClosed::new(3);
        for id in 1..=5 {
            recent.push(id);
        }
        assert_eq!(recent.len(), 3);
        assert_eq!(recent.pop(), Some(5));
        assert_eq!(recent.pop(), Some(4));
        assert_eq!(recent.pop(), Some(3));
        assert_eq!(recent.pop(), None, "2 and 1 fell off the end");
        assert!(recent.is_empty());
    }

    #[test]
    fn edge_round_trips_the_wire() {
        for edge in [Edge::Left, Edge::Right] {
            assert_eq!(Edge::from_wire(edge.as_wire()), Some(edge));
        }
        assert_eq!(Edge::from_wire("bottom-left"), None, "corners are not edges");
        assert_eq!(Edge::default(), Edge::Left);
    }

    /// `spec/04` §1: a recording card swaps Annotate for Trim; a GIF's editor is M5's,
    /// so it has the corner too. A GIF is still not pinned (D68).
    #[test]
    fn a_gif_card_offers_trim_but_not_pin() {
        let gif = Controls { kind: CardKind::Gif, already_saved: false };
        assert!(gif.has_edit());
        assert_eq!(gif.edit(), EditAction::Trim);
        assert!(!gif.has_pin());
        assert!(gif.shows_badge());

        let video = Controls { kind: CardKind::Video, already_saved: false };
        assert_eq!(video.edit(), EditAction::Trim);
        assert!(video.has_pin());

        let image = Controls { kind: CardKind::Image, already_saved: false };
        assert!(image.has_edit());
        assert_eq!(image.edit(), EditAction::Annotate);
        assert!(image.has_pin());
        assert!(!image.shows_badge());
    }
}
