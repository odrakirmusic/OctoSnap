// SPDX-License-Identifier: GPL-3.0-or-later

//! `ANN-32` (D187): a circle that shows part of the picture enlarged, joined to it by a line.
//!
//! A magnifier is a lens and the spot it enlarges (`Geometry::Magnifier`): the lens a circle
//! of `radius` about `center`, the spot the circle `zoom` times smaller about `source`. The
//! lens shows the picture under the spot -- the base image and the picture's own objects,
//! redactions included, so a lens over a redaction shows the redaction and never what it
//! hides -- `zoom` times larger, its middle the spot's middle. The marks are not in it: a
//! lens shows the picture, and one that showed the marks would show itself.
//!
//! The spot is a ring on the picture, joined to the lens by a line from edge to edge along
//! the line between their middles. A spot under its own lens -- a magnifier placed with a
//! click -- is hidden by it, and the lens enlarges what is beneath it, as a loupe does.
//!
//! The spot belongs to the picture and the lens to the marks: a move, an arrow key or a
//! paste moves the lens and leaves the spot on what it enlarges; the spot is the ninth
//! handle, as a callout's tip is (`ANN-29`). What moves the picture -- a cut, a turn, a
//! resize -- moves both.

use crate::geometry::{Bounds, Point, Segment};

/// What the options row offers, and `[` and `]` step through.
pub const ZOOMS: [f64; 4] = [1.5, 2.0, 3.0, 4.0];
/// What a new magnifier enlarges by.
pub const DEFAULT_ZOOM: f64 = 2.0;
/// A new lens's radius, in document units: half as wide again as the largest badge.
pub const RADIUS: f64 = 64.0;
/// The least a lens is, so a resize cannot make one vanish.
pub const LEAST_RADIUS: f64 = 8.0;

/// The spot's radius: the lens's, `zoom` times smaller.
#[must_use]
pub fn spot_radius(radius: f64, zoom: f64) -> f64 {
    radius / zoom.max(1.0)
}

/// The lens's box, without its ring.
#[must_use]
pub fn lens(center: Point, radius: f64) -> Bounds {
    Bounds::new(center.x - radius, center.y - radius, radius * 2.0, radius * 2.0)
}

/// The spot's box.
#[must_use]
pub fn spot(source: Point, radius: f64, zoom: f64) -> Bounds {
    let spot = spot_radius(radius, zoom);
    Bounds::new(source.x - spot, source.y - spot, spot * 2.0, spot * 2.0)
}

/// The line from the spot's ring to the lens's, along the line between their middles;
/// `None` while the two circles overlap, and there is nothing between them to join.
#[must_use]
pub fn link(center: Point, radius: f64, source: Point, zoom: f64) -> Option<Segment> {
    let spot = spot_radius(radius, zoom);
    let length = center.distance_to(source);
    if length <= radius + spot {
        return None;
    }
    let (ux, uy) = ((center.x - source.x) / length, (center.y - source.y) / length);
    Some(Segment::new(
        source.offset(ux * spot, uy * spot),
        center.offset(-ux * radius, -uy * radius),
    ))
}

/// Where a point of the picture shows in the lens: `zoom` times as far from the lens's
/// middle as it is from the spot's.
#[must_use]
pub fn through(center: Point, source: Point, zoom: f64, point: Point) -> Point {
    Point::new(center.x + (point.x - source.x) * zoom, center.y + (point.y - source.y) * zoom)
}

/// Whether `point` is on the magnifier, `reach` being half its ring and the tolerance: in
/// the lens, on the spot's ring or on the line. Not inside the spot, which is the picture
/// showing through a ring -- a click there is for what is beneath it, as inside a hollow
/// ellipse.
#[must_use]
pub fn hits(
    center: Point,
    radius: f64,
    source: Point,
    zoom: f64,
    point: Point,
    reach: f64,
) -> bool {
    if center.distance_to(point) <= radius + reach {
        return true;
    }
    let spot = spot_radius(radius, zoom);
    if (source.distance_to(point) - spot).abs() <= reach {
        return true;
    }
    link(center, radius, source, zoom).is_some_and(|line| line.distance_to(point) <= reach)
}

/// Whether `point` is on the spot's ring and off the lens: a press there takes the spot,
/// to move what the lens enlarges, where a press on the lens moves the lens.
#[must_use]
pub fn on_spot(
    center: Point,
    radius: f64,
    source: Point,
    zoom: f64,
    point: Point,
    reach: f64,
) -> bool {
    center.distance_to(point) > radius + reach
        && (source.distance_to(point) - spot_radius(radius, zoom)).abs() <= reach
}

/// The next of [`ZOOMS`] up or down from `zoom`, or `zoom` itself at either end.
#[must_use]
pub fn step(zoom: f64, up: bool) -> f64 {
    let next = if up {
        ZOOMS.iter().copied().find(|z| *z > zoom + 1e-9)
    } else {
        ZOOMS.iter().rev().copied().find(|z| *z < zoom - 1e-9)
    };
    next.unwrap_or(zoom)
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "a failing assertion should panic")]
mod tests {
    use super::*;

    fn close(a: Point, b: Point) -> bool {
        a.distance_to(b) < 1e-9
    }

    #[test]
    fn the_spot_is_the_lens_made_zoom_times_smaller() {
        assert!((spot_radius(64.0, 2.0) - 32.0).abs() < 1e-9);
        assert!((spot_radius(60.0, 3.0) - 20.0).abs() < 1e-9);
        // Never larger than the lens: a zoom under 1 would shrink, which is not a magnifier.
        assert!((spot_radius(64.0, 0.5) - 64.0).abs() < 1e-9);
        let b = spot(Point::new(100.0, 50.0), 64.0, 2.0);
        assert_eq!(b, Bounds::new(68.0, 18.0, 64.0, 64.0));
        assert_eq!(lens(Point::new(10.0, 20.0), 5.0), Bounds::new(5.0, 15.0, 10.0, 10.0));
    }

    /// The spot's middle shows at the lens's, and the spot's edge at the lens's edge: the
    /// lens is exactly the spot, enlarged.
    #[test]
    fn the_spot_fills_the_lens() {
        let (center, source, zoom, radius) =
            (Point::new(300.0, 100.0), Point::new(50.0, 80.0), 2.0, 64.0);
        assert!(close(through(center, source, zoom, source), center));
        let edge = source.offset(spot_radius(radius, zoom), 0.0);
        assert!(close(through(center, source, zoom, edge), center.offset(radius, 0.0)));
        let above = source.offset(0.0, -spot_radius(radius, zoom));
        assert!(close(through(center, source, zoom, above), center.offset(0.0, -radius)));
    }

    #[test]
    fn the_line_joins_the_rings_edge_to_edge() {
        let (center, source) = (Point::new(200.0, 0.0), Point::new(0.0, 0.0));
        let line = link(center, 64.0, source, 2.0).expect("apart, so joined");
        assert!(close(line.start, Point::new(32.0, 0.0)), "{line:?}");
        assert!(close(line.end, Point::new(136.0, 0.0)), "{line:?}");
        // Touching, and overlapping: nothing to join.
        assert!(link(Point::new(96.0, 0.0), 64.0, source, 2.0).is_none());
        assert!(link(Point::new(10.0, 10.0), 64.0, source, 2.0).is_none());
        assert!(link(source, 64.0, source, 2.0).is_none(), "a loupe has no line");
    }

    #[test]
    fn the_lens_the_ring_and_the_line_are_hit_and_the_spot_is_not() {
        let (center, source, zoom, radius, reach) =
            (Point::new(300.0, 0.0), Point::new(0.0, 0.0), 2.0, 64.0, 4.0);
        let on = |p: Point| hits(center, radius, source, zoom, p, reach);
        assert!(on(center), "the middle of the lens");
        assert!(on(center.offset(0.0, 66.0)), "its ring, within the reach");
        assert!(!on(center.offset(0.0, 70.0)), "past it");
        assert!(on(source.offset(32.0, 0.0)) && on(source.offset(0.0, -34.0)), "the spot's ring");
        assert!(!on(source), "inside the spot is the picture");
        assert!(on(Point::new(150.0, 2.0)), "the line");
        assert!(!on(Point::new(150.0, 20.0)), "beside it");
        // The spot's ring is the spot's; the lens and the line are the lens's.
        let spot = |p: Point| on_spot(center, radius, source, zoom, p, reach);
        assert!(spot(source.offset(0.0, 33.0)));
        assert!(!spot(center) && !spot(Point::new(150.0, 0.0)) && !spot(source));
        // A loupe's spot is under its lens, and the lens takes the press.
        assert!(!on_spot(source, radius, source, zoom, source.offset(32.0, 0.0), reach));
    }

    #[test]
    fn the_keys_step_along_the_ladder_and_stop_at_its_ends() {
        assert!((step(2.0, true) - 3.0).abs() < 1e-9);
        assert!((step(2.0, false) - 1.5).abs() < 1e-9);
        assert!((step(4.0, true) - 4.0).abs() < 1e-9);
        assert!((step(1.5, false) - 1.5).abs() < 1e-9);
        // Off the ladder, the next one on it.
        assert!((step(2.5, true) - 3.0).abs() < 1e-9);
        assert!((step(2.5, false) - 2.0).abs() < 1e-9);
    }
}
