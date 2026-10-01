// SPDX-License-Identifier: GPL-3.0-or-later

//! Resizing by a handle: the eight grips, and where a rectangle goes when one is dragged.
//!
//! Its own module rather than part of `crop.rs`, because two features in `spec/05` §4 ask
//! the same question. §4.11's crop rect is dragged by "L-shaped corner brackets and short
//! bar handles at the edge midpoints", and §4.1's selection chrome has "eight handles for
//! a resizable object" -- including §4.5's "drag a corner handle for any size in between",
//! which is how a text object gets a font size that is not one of the thirteen presets.
//! Both are the same arithmetic over a [`Bounds`], so it is written once and tested once.
//!
//! Two rules hold throughout and are worth stating before the code:
//!
//! **The opposite side does not move.** Dragging the left edge changes `x` and `width`
//! and leaves the right edge exactly where it was. That sounds obvious and is the thing
//! that goes wrong when a resize is implemented as "scale about the centre" -- the corner
//! the user is not touching creeps, and a rect snapped to the image's edge unsnaps
//! itself.
//!
//! **A rect never inverts.** Dragging the left edge past the right one stops at
//! [`MIN_EXTENT`] rather than flipping the rect inside out. Flipping is defensible for a
//! drawing tool -- `Bounds::from_corners` allows exactly that while a shape is being
//! drawn -- but a crop rect that turns itself inside out under the pointer is a puzzle,
//! and a text object that does is a bug.

use crate::geometry::{Bounds, Point};

/// The smallest a resize will leave either extent, in image units.
///
/// Not zero, and not one: a crop rect of a single pixel is not a thing anyone wants and a
/// zero-extent one divides by itself in the aspect arithmetic. Sixteen is about the
/// smallest rect whose eight handles are still individually hittable at 100 %.
pub const MIN_EXTENT: f64 = 16.0;

/// One of `spec/05` §4.1's eight handles.
///
/// Named for where it sits rather than for what it does, because both callers draw them:
/// the crop rect puts brackets on the corners and bars on the edge midpoints (§4.11), and
/// the selection chrome puts a square on all eight (§4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Left,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Handle {
    /// All eight, in reading order -- which is also the order the chrome draws them.
    pub const ALL: [Self; 8] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Left,
        Self::Right,
        Self::BottomLeft,
        Self::Bottom,
        Self::BottomRight,
    ];

    /// The four corners, which `spec/05` §4.11 draws as L-shaped brackets.
    pub const CORNERS: [Self; 4] =
        [Self::TopLeft, Self::TopRight, Self::BottomLeft, Self::BottomRight];

    /// The four edge midpoints, which §4.11 draws as short bars.
    pub const EDGES: [Self; 4] = [Self::Top, Self::Left, Self::Right, Self::Bottom];

    /// Which way the handle points from the middle, per axis, in `-1..=1`.
    ///
    /// Everything else here is derived from this pair, which is why the eight arms appear
    /// exactly once: the anchor is the opposite side, the moved edges are the non-zero
    /// axes, and the cursor is the diagonal it lies on.
    #[must_use]
    pub const fn direction(self) -> (i8, i8) {
        match self {
            Self::TopLeft => (-1, -1),
            Self::Top => (0, -1),
            Self::TopRight => (1, -1),
            Self::Left => (-1, 0),
            Self::Right => (1, 0),
            Self::BottomLeft => (-1, 1),
            Self::Bottom => (0, 1),
            Self::BottomRight => (1, 1),
        }
    }

    #[must_use]
    pub const fn is_corner(self) -> bool {
        let (dx, dy) = self.direction();
        dx != 0 && dy != 0
    }

    /// Where the handle sits on a rectangle.
    #[must_use]
    pub fn position(self, bounds: Bounds) -> Point {
        let r = bounds.normalised();
        let (dx, dy) = self.direction();
        Point::new(edge(r.x, r.x + r.width, dx), edge(r.y, r.y + r.height, dy))
    }

    /// The point a drag of this handle leaves alone: the opposite corner, or the middle
    /// of the opposite edge.
    ///
    /// This is the whole reason a resize is not a scale. Dragging `TopLeft` pins
    /// `BottomRight`; dragging `Right` pins the left edge and, because an aspect-locked
    /// resize has to grow the *other* axis somewhere, the vertical middle of it.
    #[must_use]
    pub fn anchor(self, bounds: Bounds) -> Point {
        let r = bounds.normalised();
        let (dx, dy) = self.direction();
        // The opposite side, so the sign is negated.
        Point::new(edge(r.x, r.x + r.width, -dx), edge(r.y, r.y + r.height, -dy))
    }

    /// The CSS cursor name for the handle, for `gdk_cursor_new_from_name`.
    #[must_use]
    pub const fn cursor(self) -> &'static str {
        match self {
            Self::TopLeft => "nw-resize",
            Self::Top => "n-resize",
            Self::TopRight => "ne-resize",
            Self::Left => "w-resize",
            Self::Right => "e-resize",
            Self::BottomLeft => "sw-resize",
            Self::Bottom => "s-resize",
            Self::BottomRight => "se-resize",
        }
    }
}

/// `low`, `high` or the midpoint, by the sign of `side`.
fn edge(low: f64, high: f64, side: i8) -> f64 {
    match side {
        i8::MIN..=-1 => low,
        0 => f64::midpoint(low, high),
        1..=i8::MAX => high,
    }
}

/// What the pointer is over: a handle, the inside of the rect, or neither.
///
/// Ordered as it is tested -- a handle first, because the handles at the corners overlap
/// the inside and the user reaching for one has not asked to move the rect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grip {
    Handle(Handle),
    Inside,
    Outside,
}

/// The handle under `at`, within `tolerance` of its position.
///
/// Corners win ties, and they win by being tested first rather than by a distance
/// comparison: on a rect narrower than twice the tolerance the corner and the edge
/// midpoint are within tolerance of each other, and resizing two edges at once is the
/// more likely intent when the target is that small.
#[must_use]
pub fn handle_at(bounds: Bounds, at: Point, tolerance: f64) -> Option<Handle> {
    Handle::CORNERS
        .into_iter()
        .chain(Handle::EDGES)
        .find(|handle| handle.position(bounds).distance_to(at) <= tolerance)
}

/// [`handle_at`], falling back to whether the point is inside the rect.
#[must_use]
pub fn grip_at(bounds: Bounds, at: Point, tolerance: f64) -> Grip {
    match handle_at(bounds, at, tolerance) {
        Some(handle) => Grip::Handle(handle),
        None if bounds.contains(at) => Grip::Inside,
        None => Grip::Outside,
    }
}

/// Drags `handle` to `to`, keeping the opposite side where it is.
///
/// Never inverts: an extent squeezed below [`MIN_EXTENT`] stops there, with the anchor
/// still fixed, so the rect shrinks to the minimum and stays put rather than flipping
/// through itself.
#[must_use]
pub fn resize(bounds: Bounds, handle: Handle, to: Point) -> Bounds {
    let r = bounds.normalised();
    let (dx, dy) = handle.direction();
    let (mut left, mut right) = (r.x, r.x + r.width);
    let (mut top, mut bottom) = (r.y, r.y + r.height);

    if dx < 0 {
        left = to.x.min(right - MIN_EXTENT);
    } else if dx > 0 {
        right = to.x.max(left + MIN_EXTENT);
    }
    if dy < 0 {
        top = to.y.min(bottom - MIN_EXTENT);
    } else if dy > 0 {
        bottom = to.y.max(top + MIN_EXTENT);
    }
    Bounds::new(left, top, right - left, bottom - top)
}

/// [`resize`], with the result forced to `ratio` (width over height).
///
/// The three cases differ in *which* extent the drag decides, and each is the behaviour
/// that keeps the pointer on the handle:
///
/// - a **corner** decides both, so the ratio is applied to whichever axis the drag made
///   larger relative to it -- the rect covers the free-form drag rather than fitting
///   inside it, which is what stops a diagonal drag leaving the pointer behind;
/// - a **left or right** edge decides the width, and the height follows, growing equally
///   above and below the anchor edge's midpoint;
/// - a **top or bottom** edge decides the height, and the width follows.
#[must_use]
pub fn resize_to_ratio(bounds: Bounds, handle: Handle, to: Point, ratio: f64) -> Bounds {
    if !(ratio.is_finite() && ratio > 0.0) {
        return resize(bounds, handle, to);
    }
    let free = resize(bounds, handle, to).normalised();
    let (dx, dy) = handle.direction();

    let (mut width, mut height) = if dx != 0 && dy != 0 {
        // Cover, not fit: take the larger of the two demands.
        let width = free.width.max(free.height * ratio);
        (width, width / ratio)
    } else if dx != 0 {
        (free.width, free.width / ratio)
    } else {
        (free.height * ratio, free.height)
    };

    // The minimum applies to both axes at once, so it is enforced on the height and the
    // width is re-derived -- clamping them independently would break the ratio.
    if height < MIN_EXTENT {
        height = MIN_EXTENT;
        width = height * ratio;
    }
    if width < MIN_EXTENT {
        width = MIN_EXTENT;
        height = width / ratio;
    }

    let anchor = handle.anchor(bounds);
    Bounds::new(
        place(anchor.x, width, dx),
        place(anchor.y, height, dy),
        width,
        height,
    )
}

/// The origin of an extent grown from `anchor` in direction `side`.
fn place(anchor: f64, extent: f64, side: i8) -> f64 {
    match side {
        i8::MIN..=-1 => anchor - extent,
        0 => anchor - extent / 2.0,
        1..=i8::MAX => anchor,
    }
}

/// Grows or shrinks `bounds` to `ratio` without moving its centre.
///
/// What the aspect menu does to a rect that is already there (`spec/05` §4.11): choosing
/// 16:9 has to change the rect the user is looking at, and there is no handle to anchor
/// it, so the centre stays put. Fits *inside* the current rect rather than covering it,
/// so switching aspect on a crop that fills the image does not push it off the edge.
#[must_use]
pub fn fit_to_ratio(bounds: Bounds, ratio: f64) -> Bounds {
    if !(ratio.is_finite() && ratio > 0.0) {
        return bounds.normalised();
    }
    let r = bounds.normalised();
    let centre = r.center();
    let (width, height) = if r.width / ratio <= r.height {
        (r.width, r.width / ratio)
    } else {
        (r.height * ratio, r.height)
    };
    let width = width.max(MIN_EXTENT);
    let height = height.max(MIN_EXTENT);
    Bounds::new(centre.x - width / 2.0, centre.y - height / 2.0, width, height)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn rect() -> Bounds {
        Bounds::new(100.0, 100.0, 400.0, 200.0)
    }

    fn close(a: Bounds, b: Bounds) -> bool {
        (a.x - b.x).abs() < 1e-9
            && (a.y - b.y).abs() < 1e-9
            && (a.width - b.width).abs() < 1e-9
            && (a.height - b.height).abs() < 1e-9
    }

    /// The rule the whole module exists to keep: the side you are not dragging does not
    /// move. A resize written as a scale about the centre passes nothing here.
    #[test]
    fn the_opposite_side_never_moves() {
        let r = rect();
        for handle in Handle::ALL {
            let dragged = resize(r, handle, Point::new(220.0, 160.0));
            let anchor = handle.anchor(r);
            let after = handle.anchor(dragged);
            assert!(
                (anchor.x - after.x).abs() < 1e-9 && (anchor.y - after.y).abs() < 1e-9,
                "{handle:?} moved its anchor from {anchor:?} to {after:?}",
            );
        }
    }

    #[test]
    fn an_edge_handle_changes_one_axis_only() {
        let r = rect();
        let taller = resize(r, Handle::Bottom, Point::new(9999.0, 400.0));
        assert!(close(taller, Bounds::new(100.0, 100.0, 400.0, 300.0)), "{taller:?}");

        let narrower = resize(r, Handle::Left, Point::new(300.0, 9999.0));
        assert!(close(narrower, Bounds::new(300.0, 100.0, 200.0, 200.0)), "{narrower:?}");
    }

    /// Dragging an edge past its opposite stops at the minimum instead of turning the
    /// rect inside out. A crop rect that inverts under the pointer is unusable.
    #[test]
    fn a_rect_stops_at_the_minimum_rather_than_inverting() {
        let r = rect();
        let squeezed = resize(r, Handle::Left, Point::new(4000.0, 0.0));
        assert!(squeezed.width > 0.0 && squeezed.height > 0.0);
        assert!((squeezed.width - MIN_EXTENT).abs() < 1e-9, "{squeezed:?}");
        // Still anchored to the right edge it was dragged towards.
        assert!((squeezed.x + squeezed.width - 500.0).abs() < 1e-9, "{squeezed:?}");

        let flattened = resize(r, Handle::Top, Point::new(0.0, 4000.0));
        assert!((flattened.height - MIN_EXTENT).abs() < 1e-9, "{flattened:?}");
        assert!((flattened.y + flattened.height - 300.0).abs() < 1e-9, "{flattened:?}");
    }

    #[test]
    fn a_ratio_holds_however_the_handle_is_dragged() {
        let r = rect();
        for handle in Handle::ALL {
            for to in [
                Point::new(0.0, 0.0),
                Point::new(640.0, 480.0),
                Point::new(110.0, 105.0),
                Point::new(-300.0, 700.0),
            ] {
                let sized = resize_to_ratio(r, handle, to, 16.0 / 9.0);
                assert!(
                    (sized.width / sized.height - 16.0 / 9.0).abs() < 1e-9,
                    "{handle:?} to {to:?} gave {sized:?}",
                );
                assert!(sized.width >= MIN_EXTENT && sized.height >= MIN_EXTENT);
            }
        }
    }

    /// An aspect-locked *edge* drag has to put the other axis somewhere, and the only
    /// choice that does not make the rect walk away from the pointer is symmetrically
    /// about the anchor edge's middle.
    #[test]
    fn a_locked_edge_drag_grows_the_other_axis_symmetrically() {
        let r = Bounds::new(0.0, 0.0, 100.0, 100.0);
        // 200 wide at 4:1 is 50 tall, so 25 comes off the top and 25 off the bottom.
        let wider = resize_to_ratio(r, Handle::Right, Point::new(200.0, 0.0), 4.0);
        assert!(close(wider, Bounds::new(0.0, 25.0, 200.0, 50.0)), "{wider:?}");
        // The left edge is still at zero and the vertical middle is still at 50.
        assert!((wider.x).abs() < 1e-9);
        assert!((wider.center().y - 50.0).abs() < 1e-9);
    }

    /// A corner drag covers the free-form rect rather than fitting inside it, which is
    /// what keeps the pointer on the handle instead of ahead of it.
    #[test]
    fn a_locked_corner_drag_covers_the_free_drag() {
        let r = Bounds::new(0.0, 0.0, 100.0, 100.0);
        // A drag that asks for 300 x 100 at 1:1 becomes 300 x 300, not 100 x 100.
        let grown = resize_to_ratio(r, Handle::BottomRight, Point::new(300.0, 100.0), 1.0);
        assert!(close(grown, Bounds::new(0.0, 0.0, 300.0, 300.0)), "{grown:?}");
    }

    #[test]
    fn fitting_a_ratio_keeps_the_centre_and_stays_inside() {
        let r = Bounds::new(0.0, 0.0, 400.0, 400.0);
        let wide = fit_to_ratio(r, 2.0);
        assert!(close(wide, Bounds::new(0.0, 100.0, 400.0, 200.0)), "{wide:?}");
        assert!(
            (wide.center().x - r.center().x).abs() < 1e-9
                && (wide.center().y - r.center().y).abs() < 1e-9,
        );
        // Inside, not covering: the width is unchanged and the height shrank.
        assert!(wide.width <= r.width && wide.height <= r.height);
    }

    #[test]
    fn a_ratio_that_is_not_a_number_falls_back_to_a_free_resize() {
        let r = rect();
        let to = Point::new(220.0, 160.0);
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(close(resize_to_ratio(r, Handle::TopLeft, to, bad), resize(r, Handle::TopLeft, to)));
            assert!(close(fit_to_ratio(r, bad), r));
        }
    }

    #[test]
    fn a_handle_is_found_within_tolerance_and_corners_win() {
        let r = rect();
        assert_eq!(handle_at(r, Point::new(100.0, 100.0), 6.0), Some(Handle::TopLeft));
        assert_eq!(handle_at(r, Point::new(303.0, 99.0), 6.0), Some(Handle::Top));
        assert_eq!(handle_at(r, Point::new(250.0, 200.0), 6.0), None);

        // Small enough that the corner and the edge midpoint are both in range.
        let tiny = Bounds::new(0.0, 0.0, MIN_EXTENT, MIN_EXTENT);
        assert_eq!(
            handle_at(tiny, Point::new(0.0, 8.0), 10.0),
            Some(Handle::TopLeft),
            "a corner is the likelier intent on a rect this small",
        );
    }

    #[test]
    fn a_grip_is_a_handle_then_the_inside_then_nothing() {
        let r = rect();
        assert_eq!(grip_at(r, Point::new(500.0, 300.0), 6.0), Grip::Handle(Handle::BottomRight));
        assert_eq!(grip_at(r, Point::new(300.0, 200.0), 6.0), Grip::Inside);
        assert_eq!(grip_at(r, Point::new(50.0, 50.0), 6.0), Grip::Outside);
    }

    /// The eight handles are eight distinct points, and each one's anchor is the point
    /// diagonally or orthogonally opposite it -- checked by construction rather than by
    /// eight hand-written pairs, so a wrong sign in `direction` cannot hide.
    #[test]
    fn every_handle_has_its_own_position_and_the_opposite_anchor() {
        let r = rect();
        let mut seen: Vec<(f64, f64)> = Vec::new();
        for handle in Handle::ALL {
            let p = handle.position(r);
            assert!(!seen.contains(&(p.x, p.y)), "{handle:?} duplicates a position");
            seen.push((p.x, p.y));
            let a = handle.anchor(r);
            let centre = r.center();
            // Position and anchor are mirror images about the centre.
            assert!((p.x + a.x - 2.0 * centre.x).abs() < 1e-9, "{handle:?}");
            assert!((p.y + a.y - 2.0 * centre.y).abs() < 1e-9, "{handle:?}");
        }
        assert_eq!(seen.len(), 8);
    }
}
