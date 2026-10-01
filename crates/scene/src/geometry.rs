// SPDX-License-Identifier: GPL-3.0-or-later

//! Canvas geometry: floating point, and in the image's own coordinates.
//!
//! Deliberately **not** `core::geometry::Rect`, which is `i32` and documents itself as
//! logical *stage* coordinates. Both halves of that are wrong here. An annotation lives
//! in the *image*, so it survives the window being moved, resized or zoomed, and it is
//! fractional at every stage: a drag lands between pixels, `spec/05` §9's zoom is
//! continuous, an arrow's control point is a third of the way along a curve, and
//! `spec/05` §11 item 2 asks for an export at 2x to match the canvas at 100 % -- which
//! only holds if the object was never rounded to the screen it was drawn on.
//!
//! Keeping the two types distinct means a stage rect cannot be passed where a canvas one
//! is expected without saying so, which is the same argument `core::geometry` makes for
//! keeping logical and physical apart.

use serde::{Deserialize, Serialize};

/// A point in image coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub fn offset(self, dx: f64, dy: f64) -> Self {
        Self::new(self.x + dx, self.y + dy)
    }

    #[must_use]
    pub fn distance_to(self, other: Self) -> f64 {
        let (dx, dy) = (other.x - self.x, other.y - self.y);
        dx.hypot(dy)
    }
}

/// An axis-aligned rectangle in image coordinates.
///
/// May have negative width or height while a drag is in progress -- the user is allowed
/// to draw up and to the left -- so anything that reasons about edges calls
/// [`Bounds::normalised`] first rather than assuming.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Bounds {
    #[must_use]
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }

    /// The rectangle two dragged corners describe, in either order.
    #[must_use]
    pub fn from_corners(a: Point, b: Point) -> Self {
        Self::new(a.x.min(b.x), a.y.min(b.y), (b.x - a.x).abs(), (b.y - a.y).abs())
    }

    /// Positive width and height, with the origin moved to the top-left.
    #[must_use]
    pub fn normalised(self) -> Self {
        let x = if self.width < 0.0 { self.x + self.width } else { self.x };
        let y = if self.height < 0.0 { self.y + self.height } else { self.y };
        Self::new(x, y, self.width.abs(), self.height.abs())
    }

    #[must_use]
    pub fn contains(self, point: Point) -> bool {
        let r = self.normalised();
        point.x >= r.x && point.x <= r.x + r.width && point.y >= r.y && point.y <= r.y + r.height
    }

    /// Grown by `by` on every side. Negative shrinks, and never past empty.
    #[must_use]
    pub fn inflated(self, by: f64) -> Self {
        let r = self.normalised();
        Self::new(
            r.x - by,
            r.y - by,
            (r.width + 2.0 * by).max(0.0),
            (r.height + 2.0 * by).max(0.0),
        )
    }

    /// The smallest rectangle holding both.
    ///
    /// Used to build an object's bounds from its parts -- a text run plus its plate, a
    /// path's points, the union of the spotlights `spec/05` §5.3 masks with.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        let (a, b) = (self.normalised(), other.normalised());
        let x = a.x.min(b.x);
        let y = a.y.min(b.y);
        Self::new(
            x,
            y,
            (a.x + a.width).max(b.x + b.width) - x,
            (a.y + a.height).max(b.y + b.height) - y,
        )
    }

    #[must_use]
    pub fn center(self) -> Point {
        let r = self.normalised();
        Point::new(r.x + r.width / 2.0, r.y + r.height / 2.0)
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.width == 0.0 || self.height == 0.0
    }

    /// Whether `point` is on the ellipse inscribed in these bounds.
    ///
    /// Here rather than in `hit.rs` because it is the shape's own definition, and both
    /// the ellipse tool and the elliptical spotlight (`spec/05` §4.8) need it.
    #[must_use]
    pub fn ellipse_contains(self, point: Point) -> bool {
        let r = self.normalised();
        if r.width <= 0.0 || r.height <= 0.0 {
            return false;
        }
        let c = r.center();
        let nx = (point.x - c.x) / (r.width / 2.0);
        let ny = (point.y - c.y) / (r.height / 2.0);
        nx * nx + ny * ny <= 1.0
    }
}

/// A straight span, and the reason hit testing needs no pixels.
///
/// `spec/05` §6: "Hit testing uses object geometry, never pixels." A line, an arrow's
/// shaft and each leg of a freehand path are all this, so the distance-to-a-stroke
/// question is answered once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub start: Point,
    pub end: Point,
}

impl Segment {
    #[must_use]
    pub const fn new(start: Point, end: Point) -> Self {
        Self { start, end }
    }

    /// Shortest distance from `point` to the segment -- not to the infinite line.
    ///
    /// The difference matters: clicking a long way past the end of a short arrow, but on
    /// the line it would have followed, must not select it.
    #[must_use]
    pub fn distance_to(self, point: Point) -> f64 {
        let (dx, dy) = (self.end.x - self.start.x, self.end.y - self.start.y);
        let length_squared = dx * dx + dy * dy;
        if length_squared == 0.0 {
            return self.start.distance_to(point);
        }
        // The projection's position along the segment, clamped to its ends.
        let t = (((point.x - self.start.x) * dx + (point.y - self.start.y) * dy)
            / length_squared)
            .clamp(0.0, 1.0);
        let nearest = Point::new(self.start.x + t * dx, self.start.y + t * dy);
        nearest.distance_to(point)
    }

    #[must_use]
    pub fn bounds(self) -> Bounds {
        Bounds::from_corners(self.start, self.end)
    }
}

/// How many straight legs stand in for a curved arrow when it is hit-tested or measured.
///
/// Sixteen puts every leg within a fraction of a pixel of the curve at the sizes `spec/05`
/// §2 offers, and sixteen distance tests per motion event is nothing.
pub const CURVE_LEGS: usize = 16;

/// Points along the quadratic Bézier `p0 → ctrl → p2`, `legs + 1` of them.
///
/// This is the curve the renderer draws, and it is what the hit test and the bounds have
/// to be about (D57). The first version stood in two straight legs -- `p0 → ctrl` and
/// `ctrl → p2` -- and a quadratic never reaches its control point: at the bend the curve
/// is half-way between the chord and the control point, so a click on the visible arrow's
/// middle missed by up to a quarter of the bow, and the control point itself -- a piece of
/// selection chrome, not of the arrow -- was clickable. The user's rule is the right one:
/// "the hitbox should always be as per the object created by the tool … no invisible
/// elements such as UI".
#[must_use]
pub fn quad_flatten(p0: Point, ctrl: Point, p2: Point, legs: usize) -> Vec<Point> {
    let legs = legs.max(1);
    (0..=legs)
        .map(|i| {
            #[allow(clippy::cast_precision_loss)]
            let t = i as f64 / legs as f64;
            let (u, v) = (1.0 - t, t);
            Point::new(
                u * u * p0.x + 2.0 * u * v * ctrl.x + v * v * p2.x,
                u * u * p0.y + 2.0 * u * v * ctrl.y + v * v * p2.y,
            )
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// The flattened curve starts and ends where the arrow does, passes half-way to the
    /// control point at its bend, and never reaches the control point.
    #[test]
    fn a_flattened_curve_bends_half_way_to_its_control_point() {
        let points = quad_flatten(
            Point::new(0.0, 0.0),
            Point::new(100.0, 100.0),
            Point::new(200.0, 0.0),
            16,
        );
        assert_eq!(points.len(), 17);
        assert_eq!(points[0], Point::new(0.0, 0.0));
        assert_eq!(points[16], Point::new(200.0, 0.0));
        assert!((points[8].x - 100.0).abs() < 1e-9 && (points[8].y - 50.0).abs() < 1e-9);
        assert!(points.iter().all(|p| p.y < 60.0), "nothing near the control point");
    }

    /// The user is allowed to drag up and to the left, so a rect mid-gesture has negative
    /// extents and everything that reads its edges has to normalise first.
    #[test]
    fn a_backwards_drag_normalises_to_the_same_rectangle() {
        let forwards = Bounds::from_corners(Point::new(10.0, 20.0), Point::new(110.0, 70.0));
        let backwards = Bounds::from_corners(Point::new(110.0, 70.0), Point::new(10.0, 20.0));
        assert_eq!(forwards, backwards);
        assert_eq!(forwards, Bounds::new(10.0, 20.0, 100.0, 50.0));

        let negative = Bounds::new(110.0, 70.0, -100.0, -50.0);
        assert_eq!(negative.normalised(), forwards);
        assert!(negative.contains(Point::new(50.0, 40.0)), "contains must normalise too");
    }

    #[test]
    fn union_grows_to_hold_both_and_is_order_independent() {
        let a = Bounds::new(0.0, 0.0, 10.0, 10.0);
        let b = Bounds::new(20.0, 5.0, 10.0, 10.0);
        assert_eq!(a.union(b), Bounds::new(0.0, 0.0, 30.0, 15.0));
        assert_eq!(a.union(b), b.union(a));
        assert_eq!(a.union(a), a);
    }

    #[test]
    fn inflating_never_produces_a_negative_extent() {
        let thin = Bounds::new(0.0, 0.0, 4.0, 4.0);
        assert_eq!(thin.inflated(-10.0), Bounds::new(10.0, 10.0, 0.0, 0.0));
    }

    /// The whole reason `distance_to` is about the segment and not the line: a click far
    /// past the end of a short arrow is not on it.
    #[test]
    fn distance_is_to_the_segment_not_to_the_line_it_lies_on() {
        let seg = Segment::new(Point::new(0.0, 0.0), Point::new(10.0, 0.0));
        assert!((seg.distance_to(Point::new(5.0, 3.0)) - 3.0).abs() < 1e-9);
        // Beyond the end, so the distance is to the endpoint, not 3.
        assert!((seg.distance_to(Point::new(110.0, 3.0)) - 100.044_99).abs() < 1e-4);
        // On it.
        assert!(seg.distance_to(Point::new(5.0, 0.0)).abs() < 1e-9);
    }

    /// A zero-length segment is what a click-without-drag produces, and it must answer a
    /// distance rather than divide by zero.
    #[test]
    fn a_segment_of_no_length_measures_from_its_point() {
        let dot = Segment::new(Point::new(4.0, 4.0), Point::new(4.0, 4.0));
        assert!((dot.distance_to(Point::new(4.0, 9.0)) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn an_ellipse_is_inscribed_in_its_bounds() {
        let b = Bounds::new(0.0, 0.0, 200.0, 100.0);
        assert!(b.ellipse_contains(Point::new(100.0, 50.0)), "the centre");
        assert!(b.ellipse_contains(Point::new(199.0, 50.0)), "on the major axis");
        assert!(!b.ellipse_contains(Point::new(5.0, 5.0)), "the bounding box's corner");
        assert!(b.contains(Point::new(5.0, 5.0)), "which the box does hold");
        assert!(!Bounds::new(0.0, 0.0, 0.0, 10.0).ellipse_contains(Point::new(0.0, 5.0)));
    }
}
