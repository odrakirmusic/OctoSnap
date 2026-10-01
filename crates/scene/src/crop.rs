// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §4.11's crop, as arithmetic: the aspect presets, the snapping, the rect a
//! drag produces, and the colour an expansion is filled with.
//!
//! The one thing to understand before reading it is what a crop *is* in this document
//! model, and `docs/decisions.md` D52 states it: **`Scene::canvas` is what gets exported
//! and `Scene::base` is one thing drawn inside it.** So a crop is not an operation on the
//! capture -- no pixels are resampled, nothing is thrown away, and §4.11's "non-destructive
//! in the project (stored as canvas rect)" falls out for free. It is one
//! [`Command::CanvasChange`](crate::Command::CanvasChange), and undo is one press.
//!
//! That is also why dragging beyond the image can *expand* the canvas rather than being
//! clamped to it: the canvas was never required to be the base image's rect. The new area
//! has nothing behind it, so §4.11 fills it with "the detected background colour (median
//! of the border pixels) or transparent" -- [`median_color`] is the detection, and the
//! fill is a background object (`spec/05` §5.3 group 0, below the base image), which
//! keeps it undoable and inside `objects.json` rather than in a side channel.

use crate::geometry::{Bounds, Point};
use crate::handle::{self, Handle};
use crate::scene::Scene;
use crate::style::Rgba;

/// How close an edge has to come to a target before it snaps, in image units.
///
/// In the document's units and not the widget's, so the *distance on screen* changes with
/// the zoom -- which is the right way round: zoomed in, the user is placing an edge
/// precisely and a snap that reached across forty screen pixels would fight them.
pub const SNAP_DISTANCE: f64 = 6.0;

/// `spec/05` §4.11's aspect presets, in the order the menu lists them.
///
/// The list is §4.11's: "Free, Original, 1:1, 4:3, 3:2, 16:9, 16:10, 5:4, 9:16, custom".
/// The first one is labelled **Freeform** rather than "Free" because that is what
/// `editor-crop-mode.png` shows in the menu button, and the screenshot is evidence where
/// the prose is a summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Aspect {
    #[default]
    Freeform,
    Original,
    Square,
    FourThree,
    ThreeTwo,
    SixteenNine,
    SixteenTen,
    FiveFour,
    NineSixteen,
    Custom,
}

impl Aspect {
    /// All ten, in `spec/05` §4.11's order.
    pub const ALL: [Self; 10] = [
        Self::Freeform,
        Self::Original,
        Self::Square,
        Self::FourThree,
        Self::ThreeTwo,
        Self::SixteenNine,
        Self::SixteenTen,
        Self::FiveFour,
        Self::NineSixteen,
        Self::Custom,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Freeform => "Freeform",
            Self::Original => "Original",
            Self::Square => "1:1",
            Self::FourThree => "4:3",
            Self::ThreeTwo => "3:2",
            Self::SixteenNine => "16:9",
            Self::SixteenTen => "16:10",
            Self::FiveFour => "5:4",
            Self::NineSixteen => "9:16",
            Self::Custom => "Custom",
        }
    }

    /// The preset's ratio as the pair a person would type, or `None` for the two that
    /// have no fixed numbers.
    #[must_use]
    pub const fn numbers(self) -> Option<(f64, f64)> {
        match self {
            Self::Square => Some((1.0, 1.0)),
            Self::FourThree => Some((4.0, 3.0)),
            Self::ThreeTwo => Some((3.0, 2.0)),
            Self::SixteenNine => Some((16.0, 9.0)),
            Self::SixteenTen => Some((16.0, 10.0)),
            Self::FiveFour => Some((5.0, 4.0)),
            Self::NineSixteen => Some((9.0, 16.0)),
            Self::Freeform | Self::Original | Self::Custom => None,
        }
    }

    /// The preset whose ratio is `numbers`, if one is.
    ///
    /// What makes §4.11's swap button land on a *preset* rather than always on Custom:
    /// swapping 16:9 gives 9:16, which is in the list, and swapping 4:3 gives 3:4, which
    /// is not.
    #[must_use]
    pub fn preset_for(numbers: (f64, f64)) -> Option<Self> {
        let wanted = numbers.0 / numbers.1;
        Self::ALL.into_iter().find(|aspect| {
            aspect.numbers().is_some_and(|(w, h)| (w / h - wanted).abs() < 1e-6)
        })
    }
}

/// What `spec/05` §4.11 fills a canvas expansion with.
///
/// §2's Crop row calls the control "expand-canvas colour" and §4.11 gives the two
/// automatic answers -- "the detected background colour (median of the border pixels) or
/// transparent" -- so the third is the one a person picks by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FillKind {
    /// [`median_color`] of the base image's border.
    #[default]
    Detected,
    Transparent,
    Custom,
}

impl FillKind {
    pub const ALL: [Self; 3] = [Self::Detected, Self::Transparent, Self::Custom];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            // Short, because they sit on the one toolbar row that has to fit beside
            // twelve other controls (D57); the swatch next to the menu says "colour".
            Self::Detected => "Detected",
            Self::Transparent => "Transparent",
            Self::Custom => "Custom",
        }
    }
}

/// The expand-canvas choice: which kind, and the colour the custom one holds.
///
/// The colour is kept even while another kind is chosen, so switching to Transparent and
/// back does not lose it -- the same reason the aspect menu keeps [`Crop::custom`] while
/// a preset is selected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Expand {
    pub kind: FillKind,
    pub color: Rgba,
}

impl Default for Expand {
    /// White, because a custom fill is offered for the case where the detected colour is
    /// wrong -- which on a screenshot is nearly always a page that should have been white.
    fn default() -> Self {
        Self { kind: FillKind::default(), color: Rgba::new(1.0, 1.0, 1.0, 1.0) }
    }
}

impl Expand {
    /// The colour to fill with, or `None` for transparent.
    ///
    /// `detected` is what the canvas measured off the base image's border; `None` from it
    /// means there was nothing to measure -- no texture, or a zero-size image -- and the
    /// honest answer then is transparent rather than an invented colour.
    #[must_use]
    pub fn resolve(self, detected: Option<Rgba>) -> Option<Rgba> {
        match self.kind {
            FillKind::Detected => detected,
            FillKind::Transparent => None,
            FillKind::Custom => Some(self.color),
        }
    }
}

/// `spec/05` §4.11's crop mode: the rect being dragged and the four controls around it.
///
/// Held by the editor rather than by the scene, and that is the point of "Enter applies,
/// Esc cancels": until Enter the document has not changed at all, so cancelling is
/// dropping this struct rather than undoing anything.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crop {
    pub rect: Bounds,
    pub aspect: Aspect,
    /// The two numbers behind [`Aspect::Custom`], which §4.11 shows as "a value field".
    pub custom: (f64, f64),
    /// §4.11's lock, which only has a job while the aspect is Freeform: it pins whatever
    /// ratio the rect happens to have, so a rect sized by eye can then be moved and
    /// resized without losing its proportions. With a preset chosen, the preset is
    /// already the constraint.
    pub locked: bool,
    /// §4.11's snapping, the `snapInAnnotateCrop` preference [V].
    pub snap: bool,
    /// §2's "expand-canvas colour", used only when the rect leaves the image.
    pub expand: Expand,
}

impl Crop {
    /// Enters crop mode on the whole canvas.
    ///
    /// The canvas and not the base image: entering crop twice must not silently undo a
    /// previous crop's expansion (D52).
    #[must_use]
    pub fn new(canvas: Bounds, snap: bool) -> Self {
        Self {
            rect: canvas.normalised(),
            aspect: Aspect::Freeform,
            custom: (16.0, 9.0),
            locked: false,
            snap,
            expand: Expand::default(),
        }
    }

    /// Re-enters crop mode on `canvas`, keeping every control where the user left it.
    ///
    /// Only the rect resets. Choosing 16:9, cropping, and going back into crop mode to
    /// adjust it must not silently return to Freeform.
    #[must_use]
    pub fn again(self, canvas: Bounds) -> Self {
        Self { rect: canvas.normalised(), ..self }
    }

    /// The ratio the rect is constrained to, if any.
    ///
    /// `original` is the base image's rect, which is what [`Aspect::Original`] means --
    /// the image as it was captured, not the canvas as a previous crop left it.
    #[must_use]
    pub fn ratio(&self, original: Bounds) -> Option<f64> {
        let ratio = match self.aspect {
            Aspect::Freeform => {
                if self.locked { self.rect.width / self.rect.height } else { return None }
            }
            Aspect::Original => original.width / original.height,
            Aspect::Custom => self.custom.0 / self.custom.1,
            other => {
                let (w, h) = other.numbers()?;
                w / h
            }
        };
        (ratio.is_finite() && ratio > 0.0).then_some(ratio)
    }

    /// The ratio as a pair, for the value field and for the swap.
    #[must_use]
    pub fn numbers(&self, original: Bounds) -> (f64, f64) {
        match self.aspect {
            Aspect::Custom => self.custom,
            Aspect::Original => (original.width, original.height),
            Aspect::Freeform => (self.rect.width, self.rect.height),
            other => other.numbers().unwrap_or((self.rect.width, self.rect.height)),
        }
    }

    /// Chooses a preset, refitting the rect to it.
    ///
    /// The rect has to change: the user picked 16:9 while looking at a square, and a menu
    /// that only affected the *next* drag would look broken. Fitted about the centre, and
    /// inside the rect rather than around it ([`handle::fit_to_ratio`]), so choosing an
    /// aspect on a crop that fills the image does not push it off the edge.
    pub fn set_aspect(&mut self, aspect: Aspect, original: Bounds) {
        self.aspect = aspect;
        if let Some(ratio) = self.ratio(original) {
            self.rect = handle::fit_to_ratio(self.rect, ratio);
        }
    }

    /// Sets [`Aspect::Custom`]'s two numbers and refits.
    pub fn set_custom(&mut self, numbers: (f64, f64), original: Bounds) {
        self.custom = numbers;
        self.aspect = Aspect::Custom;
        if let Some(ratio) = self.ratio(original) {
            self.rect = handle::fit_to_ratio(self.rect, ratio);
        }
    }

    /// §4.11's swap-orientation button: transposes the ratio.
    ///
    /// Lands on a preset when the transposed ratio is one (16:9 becomes 9:16) and on
    /// Custom when it is not (4:3 becomes 3:4). Freeform transposes the *rect*, which is
    /// the only thing there is to transpose when no ratio is set.
    pub fn swap_orientation(&mut self, original: Bounds) {
        let (w, h) = self.numbers(original);
        if self.aspect == Aspect::Freeform && !self.locked {
            // Nothing is constrained, so the button turns the rectangle on its side.
            let centre = self.rect.center();
            self.rect = Bounds::new(centre.x - h / 2.0, centre.y - w / 2.0, h, w);
            return;
        }
        match Aspect::preset_for((h, w)) {
            Some(preset) => self.set_aspect(preset, original),
            None => self.set_custom((h, w), original),
        }
    }

    /// §4.11's reset: the whole image again, unconstrained.
    pub fn reset(&mut self, original: Bounds) {
        self.rect = original.normalised();
        self.aspect = Aspect::Freeform;
        self.locked = false;
    }

    /// Where a drag in progress puts the rect.
    ///
    /// Snapping is applied to the pointer *before* the resize and the ratio *after*, and
    /// that order is the only one that is stable: a rect snapped to the image's edge and
    /// then corrected for 16:9 keeps the snapped edge and moves the free one, whereas
    /// snapping the finished rect would break the ratio the user asked for.
    #[must_use]
    pub fn drag_to(&self, drag: &Drag, to: Point, targets: &Targets, original: Bounds) -> Bounds {
        let ratio = self.ratio(original);
        match *drag {
            Drag::Resize { handle, start } => {
                let (dx, dy) = handle.direction();
                let at = if self.snap {
                    Point::new(
                        if dx == 0 { to.x } else { snapped(to.x, &targets.x) },
                        if dy == 0 { to.y } else { snapped(to.y, &targets.y) },
                    )
                } else {
                    to
                };
                match ratio {
                    Some(ratio) => handle::resize_to_ratio(start, handle, at, ratio),
                    None => handle::resize(start, handle, at),
                }
            }
            Drag::Move { start, from } => {
                let moved = Bounds::new(
                    start.x + to.x - from.x,
                    start.y + to.y - from.y,
                    start.width,
                    start.height,
                );
                if !self.snap {
                    return moved;
                }
                let (dx, dy) = snap_translation(moved, targets);
                Bounds::new(moved.x + dx, moved.y + dy, moved.width, moved.height)
            }
            Drag::New { from } => {
                let fresh = Bounds::from_corners(from, to);
                match ratio {
                    // A new rect is dragged by its bottom-right corner from its origin,
                    // which is what makes the ratio apply the same way a resize does.
                    Some(ratio) => handle::resize_to_ratio(
                        Bounds::new(from.x, from.y, handle::MIN_EXTENT, handle::MIN_EXTENT),
                        Handle::BottomRight,
                        to,
                        ratio,
                    ),
                    None => fresh,
                }
            }
        }
    }
}

/// One crop gesture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Drag {
    /// A handle, with the rect as it was at the press so the drag is absolute.
    Resize { handle: Handle, start: Bounds },
    /// The inside of the rect: `spec/05` §4.11's crop rect can be moved as a whole.
    Move { start: Bounds, from: Point },
    /// A drag that began outside the rect, which draws a new one.
    New { from: Point },
}

/// The edges a crop rect snaps to (`spec/05` §4.11).
///
/// > Snapping to image edges, other objects' bounds, and detected content edges when
/// > "snap" is on.
///
/// Two of the three are exact and are here. **Detected content edges** are not: they need
/// a pass over the pixels to find where the content stops, which is `spec/05` §4.13's
/// Auto Balance trick applied to a different question, and it belongs with that code in
/// M6 rather than being guessed at here.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Targets {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
}

impl Targets {
    /// The image's edges, the canvas's, and every object's bounds.
    ///
    /// The canvas as well as the base image, because after one crop they differ and both
    /// are meaningful: the image's edge is where the picture stops and the canvas's is
    /// where the export does.
    #[must_use]
    pub fn of(scene: &Scene) -> Self {
        let mut targets = Self::default();
        for rect in [scene.base.bounds(), scene.canvas] {
            targets.push(rect);
        }
        for object in scene.objects() {
            let bounds = object.bounds();
            if !bounds.is_empty() {
                targets.push(bounds);
            }
        }
        targets.x.sort_by(f64::total_cmp);
        targets.y.sort_by(f64::total_cmp);
        targets.x.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        targets.y.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        targets
    }

    fn push(&mut self, rect: Bounds) {
        let r = rect.normalised();
        self.x.push(r.x);
        self.x.push(r.x + r.width);
        self.y.push(r.y);
        self.y.push(r.y + r.height);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.x.is_empty() && self.y.is_empty()
    }
}

/// The nearest target within [`SNAP_DISTANCE`], or the value unchanged.
#[must_use]
pub fn snapped(value: f64, targets: &[f64]) -> f64 {
    targets
        .iter()
        .copied()
        .filter(|target| (target - value).abs() <= SNAP_DISTANCE)
        .min_by(|a, b| (a - value).abs().total_cmp(&(b - value).abs()))
        .unwrap_or(value)
}

/// The nudge that puts one of `rect`'s edges on a target, per axis.
///
/// Both edges are candidates and the smaller correction wins, so dragging a crop rect
/// towards the image's right edge snaps when its *right* edge arrives -- and dragging it
/// left snaps on its left edge -- without the caller having to say which.
#[must_use]
pub fn snap_translation(rect: Bounds, targets: &Targets) -> (f64, f64) {
    let r = rect.normalised();
    (
        best_nudge(&[r.x, r.x + r.width], &targets.x),
        best_nudge(&[r.y, r.y + r.height], &targets.y),
    )
}

fn best_nudge(edges: &[f64], targets: &[f64]) -> f64 {
    edges
        .iter()
        .map(|edge| snapped(*edge, targets) - edge)
        .filter(|nudge| nudge.abs() > 0.0)
        .min_by(|a, b| a.abs().total_cmp(&b.abs()))
        .unwrap_or(0.0)
}

/// Whether a crop rect reaches outside the base image, so `spec/05` §11 item 7's
/// expansion applies.
#[must_use]
pub fn expands(rect: Bounds, base: Bounds) -> bool {
    let (r, b) = (rect.normalised(), base.normalised());
    // A whisker of tolerance: a rect snapped to the image's edge is at the edge, and
    // floating-point arithmetic can put it a fraction of a unit outside.
    const EPSILON: f64 = 1e-6;
    r.x < b.x - EPSILON
        || r.y < b.y - EPSILON
        || r.x + r.width > b.x + b.width + EPSILON
        || r.y + r.height > b.y + b.height + EPSILON
}

/// `spec/05` §4.11's "detected background colour (median of the border pixels)".
///
/// Per channel, and the median rather than the mean for the reason §4.11 implies by
/// choosing it: a screenshot's border is usually one flat colour with a few pixels of
/// something else -- a window's rounded corner, a shadow, one letter of a title. The mean
/// takes a tint from those and the median ignores them.
///
/// Alpha is included, so a capture whose border is transparent detects *transparent* --
/// which is §4.11's other option ("or transparent") arrived at from the same rule rather
/// than as a separate branch.
#[must_use]
pub fn median_color(samples: &[Rgba]) -> Option<Rgba> {
    if samples.is_empty() {
        return None;
    }
    let channel = |get: fn(&Rgba) -> f64| {
        let mut values: Vec<f64> = samples.iter().map(get).collect();
        values.sort_by(f64::total_cmp);
        values.get(values.len() / 2).copied().unwrap_or(0.0)
    };
    Some(Rgba::new(
        channel(|c| c.r),
        channel(|c| c.g),
        channel(|c| c.b),
        channel(|c| c.a),
    ))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::object::{Geometry, Object};
    use crate::scene::Base;
    use crate::style::Style;

    fn base() -> Bounds {
        Bounds::new(0.0, 0.0, 1600.0, 900.0)
    }

    fn crop() -> Crop {
        Crop::new(base(), true)
    }

    /// `spec/05` §4.11's list, in its order, with the labels a menu shows.
    #[test]
    fn the_aspect_menu_is_the_spec_list_in_order() {
        let labels: Vec<&str> = Aspect::ALL.iter().map(|a| a.label()).collect();
        assert_eq!(
            labels,
            ["Freeform", "Original", "1:1", "4:3", "3:2", "16:9", "16:10", "5:4", "9:16", "Custom"],
        );
    }

    #[test]
    fn freeform_constrains_nothing_until_the_lock_is_on() {
        let mut c = crop();
        assert_eq!(c.ratio(base()), None);
        c.rect = Bounds::new(0.0, 0.0, 300.0, 100.0);
        c.locked = true;
        assert_eq!(c.ratio(base()), Some(3.0));
    }

    #[test]
    fn original_is_the_base_images_ratio_and_not_the_canvass() {
        let mut c = crop();
        // A previous crop left the canvas square; Original still means 16:9.
        c.rect = Bounds::new(0.0, 0.0, 400.0, 400.0);
        c.set_aspect(Aspect::Original, base());
        assert!((c.ratio(base()).unwrap() - 16.0 / 9.0).abs() < 1e-9);
        assert!((c.rect.width / c.rect.height - 16.0 / 9.0).abs() < 1e-9);
    }

    /// Choosing a preset has to change the rect the user is looking at, and fit inside it
    /// so a crop that fills the image is not pushed off the edge.
    #[test]
    fn choosing_an_aspect_refits_the_rect_inside_itself() {
        let mut c = crop();
        c.set_aspect(Aspect::Square, base());
        assert!((c.rect.width - c.rect.height).abs() < 1e-9, "{:?}", c.rect);
        assert!(c.rect.width <= 1600.0 && c.rect.height <= 900.0);
        // Centred on what it replaced.
        assert!((c.rect.center().x - 800.0).abs() < 1e-9);
        assert!((c.rect.center().y - 450.0).abs() < 1e-9);
    }

    /// The swap lands on a preset when the transpose is one, and on Custom when it is
    /// not. That is the whole reason `preset_for` exists.
    #[test]
    fn swapping_orientation_prefers_a_preset_and_falls_back_to_custom() {
        let mut c = crop();
        c.set_aspect(Aspect::SixteenNine, base());
        c.swap_orientation(base());
        assert_eq!(c.aspect, Aspect::NineSixteen);
        c.swap_orientation(base());
        assert_eq!(c.aspect, Aspect::SixteenNine);

        c.set_aspect(Aspect::FourThree, base());
        c.swap_orientation(base());
        assert_eq!(c.aspect, Aspect::Custom, "3:4 is not in the list");
        assert!((c.custom.0 / c.custom.1 - 3.0 / 4.0).abs() < 1e-9);
        assert!((c.rect.width / c.rect.height - 3.0 / 4.0).abs() < 1e-9);
    }

    #[test]
    fn swapping_a_freeform_rect_turns_it_on_its_side() {
        let mut c = crop();
        c.rect = Bounds::new(100.0, 200.0, 400.0, 100.0);
        c.swap_orientation(base());
        assert_eq!(c.aspect, Aspect::Freeform);
        assert!((c.rect.width - 100.0).abs() < 1e-9 && (c.rect.height - 400.0).abs() < 1e-9);
        // About the centre, so it does not walk off.
        assert!((c.rect.center().x - 300.0).abs() < 1e-9);
        assert!((c.rect.center().y - 250.0).abs() < 1e-9);
    }

    #[test]
    fn reset_is_the_whole_image_unconstrained() {
        let mut c = crop();
        c.set_aspect(Aspect::Square, base());
        c.locked = true;
        c.reset(base());
        assert_eq!(c.rect, base());
        assert_eq!(c.aspect, Aspect::Freeform);
        assert!(!c.locked);
    }

    #[test]
    fn a_move_drag_keeps_the_size_and_a_resize_keeps_the_anchor() {
        let mut c = crop();
        c.snap = false;
        c.rect = Bounds::new(100.0, 100.0, 400.0, 200.0);
        let moved = c.drag_to(
            &Drag::Move { start: c.rect, from: Point::new(200.0, 150.0) },
            Point::new(260.0, 190.0),
            &Targets::default(),
            base(),
        );
        assert_eq!(moved, Bounds::new(160.0, 140.0, 400.0, 200.0));

        let resized = c.drag_to(
            &Drag::Resize { handle: Handle::TopLeft, start: c.rect },
            Point::new(150.0, 120.0),
            &Targets::default(),
            base(),
        );
        assert_eq!(resized, Bounds::new(150.0, 120.0, 350.0, 180.0));
    }

    /// The order matters: the pointer snaps, then the ratio corrects the other axis. A
    /// rect snapped to the image edge and then made 16:9 keeps the snapped edge.
    #[test]
    fn a_snap_survives_the_aspect_correction() {
        let mut c = crop();
        c.set_aspect(Aspect::SixteenNine, base());
        c.rect = Bounds::new(400.0, 300.0, 320.0, 180.0);
        let targets = Targets { x: vec![0.0, 1600.0], y: vec![0.0, 900.0] };
        // Dragging the right edge to within snapping distance of the image's edge.
        let dragged = c.drag_to(
            &Drag::Resize { handle: Handle::Right, start: c.rect },
            Point::new(1597.0, 0.0),
            &targets,
            base(),
        );
        assert!((dragged.x + dragged.width - 1600.0).abs() < 1e-9, "{dragged:?}");
        assert!((dragged.width / dragged.height - 16.0 / 9.0).abs() < 1e-9, "{dragged:?}");
    }

    #[test]
    fn snapping_is_off_when_the_preference_is() {
        let mut c = crop();
        c.snap = false;
        let targets = Targets { x: vec![0.0], y: vec![0.0] };
        let dragged = c.drag_to(
            &Drag::Resize { handle: Handle::TopLeft, start: Bounds::new(100.0, 100.0, 400.0, 200.0) },
            Point::new(3.0, 3.0),
            &targets,
            base(),
        );
        assert_eq!(dragged, Bounds::new(3.0, 3.0, 497.0, 297.0));
    }

    #[test]
    fn a_moved_rect_snaps_on_whichever_edge_arrives_first() {
        let targets = Targets { x: vec![0.0, 1600.0], y: vec![0.0, 900.0] };
        // Left edge 4 units in: it snaps to zero.
        let (dx, dy) = snap_translation(Bounds::new(4.0, 500.0, 200.0, 100.0), &targets);
        assert!((dx + 4.0).abs() < 1e-9, "{dx}");
        assert!(dy.abs() < 1e-9, "no y target is close");

        // Right edge 3 units past: it snaps back.
        let (dx, _) = snap_translation(Bounds::new(1403.0, 0.0, 200.0, 100.0), &targets);
        assert!((dx + 3.0).abs() < 1e-9, "{dx}");

        // Nothing within range.
        let (dx, dy) = snap_translation(Bounds::new(700.0, 400.0, 200.0, 100.0), &targets);
        assert!(dx.abs() < f64::EPSILON && dy.abs() < f64::EPSILON);
    }

    #[test]
    fn the_snap_targets_are_the_image_the_canvas_and_the_objects() {
        let mut scene = Scene::new(Base::new("base.png", 1600.0, 900.0, 1.0));
        scene.canvas = Bounds::new(-50.0, 0.0, 1700.0, 900.0);
        scene.add(Object::new(
            1,
            0,
            Style::default(),
            Geometry::Rect {
                bounds: Bounds::new(200.0, 300.0, 100.0, 50.0),
                radius: 0.0,
                filled: false,
            },
        ));
        let targets = Targets::of(&scene);
        // The object's own bounds, which are the *visible* ones -- half a stroke wider
        // than its geometry on every side. That is what should snap: the user is lining
        // the crop up with the edge they can see, not with the centre of the outline.
        let drawn = scene.objects().first().map(Object::bounds).unwrap();
        for expected in [-50.0, 0.0, drawn.x, drawn.x + drawn.width, 1600.0, 1650.0] {
            assert!(
                targets.x.iter().any(|t| (t - expected).abs() < 1e-9),
                "{expected} missing from {:?}",
                targets.x,
            );
        }
        for expected in [0.0, drawn.y, drawn.y + drawn.height, 900.0] {
            assert!(targets.y.iter().any(|t| (t - expected).abs() < 1e-9), "{expected} missing");
        }
        // Sorted and deduplicated, because two objects sharing an edge is normal.
        assert!(targets.x.windows(2).all(|w| w[0] < w[1]), "{:?}", targets.x);
    }

    #[test]
    fn expansion_is_only_reported_when_the_rect_really_leaves_the_image() {
        assert!(!expands(base(), base()), "the whole image is not an expansion");
        assert!(!expands(Bounds::new(10.0, 10.0, 100.0, 100.0), base()));
        assert!(expands(Bounds::new(-1.0, 0.0, 100.0, 100.0), base()));
        assert!(expands(Bounds::new(1500.0, 0.0, 200.0, 100.0), base()));
        assert!(expands(Bounds::new(0.0, 800.0, 100.0, 200.0), base()));
    }

    /// The median and not the mean, and the test is the case that distinguishes them: a
    /// flat border with a few odd pixels in it.
    #[test]
    fn the_border_colour_is_the_median_so_a_few_odd_pixels_do_not_tint_it() {
        let grey = Rgba::new(0.5, 0.5, 0.5, 1.0);
        let mut samples = vec![grey; 9];
        samples.push(Rgba::new(1.0, 0.0, 0.0, 1.0));
        samples.push(Rgba::new(0.0, 0.0, 1.0, 1.0));
        let detected = median_color(&samples).unwrap();
        assert!((detected.r - 0.5).abs() < 1e-9, "{detected:?}");
        assert!((detected.g - 0.5).abs() < 1e-9);
        assert!((detected.b - 0.5).abs() < 1e-9);
        assert_eq!(median_color(&[]), None);
    }

    /// The three expand choices, including the case that has no answer: nothing was
    /// measured, so the fill is transparent rather than an invented colour.
    #[test]
    fn the_expand_choice_resolves_to_a_colour_or_to_nothing() {
        let detected = Rgba::new(0.2, 0.3, 0.4, 1.0);
        let custom = Rgba::new(1.0, 0.0, 0.0, 1.0);
        let expand = Expand { kind: FillKind::Detected, color: custom };
        assert_eq!(expand.resolve(Some(detected)), Some(detected));
        assert_eq!(expand.resolve(None), None, "nothing measured means transparent");
        assert_eq!(
            Expand { kind: FillKind::Transparent, color: custom }.resolve(Some(detected)),
            None,
        );
        assert_eq!(
            Expand { kind: FillKind::Custom, color: custom }.resolve(Some(detected)),
            Some(custom),
            "the custom colour wins over the detected one",
        );
        // Kept while another kind is chosen, so switching away and back does not lose it.
        assert_eq!(expand.color, custom);
    }

    /// Re-entering crop mode keeps the controls and resets only the rect.
    #[test]
    fn re_entering_crop_mode_keeps_every_control() {
        let mut first = crop();
        first.set_aspect(Aspect::SixteenTen, base());
        first.snap = false;
        first.locked = true;
        first.expand = Expand { kind: FillKind::Transparent, ..Expand::default() };

        let again = first.again(Bounds::new(0.0, 0.0, 800.0, 600.0));
        assert_eq!(again.rect, Bounds::new(0.0, 0.0, 800.0, 600.0));
        assert_eq!(again.aspect, Aspect::SixteenTen);
        assert_eq!(again.expand.kind, FillKind::Transparent);
        assert!(again.locked && !again.snap);
    }

    /// §4.11's "or transparent" is the same rule, not a second branch: a capture whose
    /// border is transparent detects a transparent fill.
    #[test]
    fn a_transparent_border_detects_a_transparent_fill() {
        let clear = Rgba::new(0.0, 0.0, 0.0, 0.0);
        let detected = median_color(&[clear, clear, clear]).unwrap();
        assert!(detected.a.abs() < f64::EPSILON);
    }
}
