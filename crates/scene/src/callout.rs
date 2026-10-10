// SPDX-License-Identifier: GPL-3.0-or-later

//! `ANN-29` (D182): a counter or a text with a tail that points at something.
//!
//! The tail is one stored point, its tip (`Geometry::Counter::tail`, `Geometry::Text::tail`).
//! Its shape is worked out here, from the body it leaves and the tip it reaches, so the
//! canvas, a vector export and the hit test agree on it:
//!
//! - A badge's tail is a wedge in the badge's colour, its base inside the disc, so the disc
//!   drawn over it hides the join.
//! - A plated label's is a wedge in the plate's colour, its base along the edge the tail
//!   leaves by and inset so the plate hides it, kept clear of the rounded corners.
//! - A label with no plate has nothing to hide a join under, so its tail is a line in the
//!   ink's colour from where it leaves the label's box, with a dot on the tip.
//!
//! A tip inside the body draws nothing: there is nothing to point out of.

use crate::geometry::{Bounds, Point, Segment};

/// Half a badge's wedge where it starts, against the radius.
const BADGE_HALF: f64 = 0.55;
/// How far into the disc the wedge's base is, against the radius. The base's corners are
/// then √(0.45² + 0.55²) ≈ 0.71 of the radius from the middle: inside the disc.
const BADGE_INSET: f64 = 0.45;
/// Half a plate's wedge, against the plate's shorter side.
const PLATE_HALF: f64 = 0.3;
/// The most a plate's wedge is, against the font size: a tall block of text would
/// otherwise grow a tail as wide as a paragraph.
const PLATE_HALF_MOST_EM: f64 = 0.6;

/// A badge's tail: the triangle to fill, the base's two corners and then the tip.
#[must_use]
pub fn badge_wedge(center: Point, radius: f64, tip: Point) -> Option<[Point; 3]> {
    let length = center.distance_to(tip);
    if length <= radius {
        return None;
    }
    let (ux, uy) = ((tip.x - center.x) / length, (tip.y - center.y) / length);
    let base = center.offset(ux * radius * BADGE_INSET, uy * radius * BADGE_INSET);
    let half = radius * BADGE_HALF;
    Some([base.offset(-uy * half, ux * half), base.offset(uy * half, -ux * half), tip])
}

/// Half the base of a plate's wedge, for a plate `body` around glyphs of `font_size`.
#[must_use]
pub fn plate_half(body: Bounds, font_size: f64) -> f64 {
    let body = body.normalised();
    (body.width.min(body.height) * PLATE_HALF).min(font_size * PLATE_HALF_MOST_EM).max(1.0)
}

/// A plated label's tail: the triangle to fill, the base's two corners and then the tip.
///
/// `body` is the plate, `corner` its corner radius and `half` half the base -- see
/// [`plate_half`].
#[must_use]
pub fn plate_wedge(body: Bounds, corner: f64, half: f64, tip: Point) -> Option<[Point; 3]> {
    let (exit, (nx, ny)) = exit(body, tip)?;
    let body = body.normalised();
    // Along its edge, the base keeps `corner + half` from either end, or sits in the middle
    // of an edge too short for that.
    let margin = corner + half;
    let clear = |value: f64, low: f64, high: f64| {
        if high - low >= 2.0 * margin {
            value.clamp(low + margin, high - margin)
        } else {
            (low + high) / 2.0
        }
    };
    let horizontal_edge = nx == 0.0;
    let middle = if horizontal_edge {
        Point::new(clear(exit.x, body.x, body.x + body.width), exit.y)
    } else {
        Point::new(exit.x, clear(exit.y, body.y, body.y + body.height))
    };
    // Inset by its own half-width, so the plate covers the base; never past the middle.
    let depth = if horizontal_edge { body.height } else { body.width } / 2.0;
    let inset = half.min(depth);
    let base = middle.offset(-nx * inset, -ny * inset);
    let (tx, ty) = (-ny, nx);
    Some([base.offset(tx * half, ty * half), base.offset(-tx * half, -ty * half), tip])
}

/// A plain label's tail: the line from where it leaves the label's box to the tip.
#[must_use]
pub fn line(body: Bounds, tip: Point) -> Option<Segment> {
    exit(body, tip).map(|(exit, _)| Segment::new(exit, tip))
}

/// The width of a plain label's tail, for glyphs of `font_size`: a tenth of it, so a
/// heading points with a heavier line than a footnote.
#[must_use]
pub fn line_width(font_size: f64) -> f64 {
    (font_size / 10.0).max(1.5)
}

/// The radius of the dot on a plain label's tip.
#[must_use]
pub fn dot_radius(font_size: f64) -> f64 {
    line_width(font_size) * 2.0
}

/// Where the line from the body's middle to the tip leaves the body, and the outward normal
/// of the edge it leaves by. `None` when the tip is inside the body, or the body is empty.
#[must_use]
pub fn exit(body: Bounds, tip: Point) -> Option<(Point, (f64, f64))> {
    let body = body.normalised();
    if body.is_empty() || body.contains(tip) {
        return None;
    }
    let middle = body.center();
    let (dx, dy) = (tip.x - middle.x, tip.y - middle.y);
    // How far along the line each pair of edges is; the nearer is the one it crosses.
    let to_side = if dx == 0.0 { f64::INFINITY } else { body.width / 2.0 / dx.abs() };
    let to_end = if dy == 0.0 { f64::INFINITY } else { body.height / 2.0 / dy.abs() };
    let along = to_side.min(to_end);
    let exit = middle.offset(dx * along, dy * along);
    let normal = if to_side <= to_end { (dx.signum(), 0.0) } else { (0.0, dy.signum()) };
    Some((exit, normal))
}

/// Whether `point` is on a tail, within `reach`. Along the line from the body's middle to
/// the tip, which a wedge and a line both follow.
#[must_use]
pub fn hits(body: Bounds, tip: Point, point: Point, reach: f64) -> bool {
    !body.normalised().contains(tip)
        && Segment::new(body.normalised().center(), tip).distance_to(point) <= reach
}

/// Where Add Pointer puts a tip: down and to the right of the body, far enough out to be
/// seen and grabbed, or towards another corner when that one is off the canvas.
#[must_use]
pub fn default_tip(body: Bounds, canvas: Bounds) -> Point {
    let body = body.normalised();
    let middle = body.center();
    let out = (body.width.min(body.height) * 1.5).max(40.0);
    let (hx, hy) = (body.width / 2.0 + out * 0.6, body.height / 2.0 + out);
    let corners = [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)];
    let tips = corners.map(|(sx, sy)| middle.offset(sx * hx, sy * hy));
    let canvas = canvas.normalised();
    tips.into_iter().find(|tip| canvas.contains(*tip)).unwrap_or(tips[0])
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "a failing assertion should panic")]
mod tests {
    use super::*;

    fn inside_disc(center: Point, radius: f64, point: Point) -> bool {
        center.distance_to(point) < radius
    }

    #[test]
    fn a_badge_s_wedge_starts_inside_the_disc_and_ends_on_the_tip() {
        let center = Point::new(50.0, 50.0);
        let tip = Point::new(150.0, 120.0);
        let [a, b, end] = badge_wedge(center, 20.0, tip).expect("a tail");
        assert!(inside_disc(center, 20.0, a) && inside_disc(center, 20.0, b));
        assert_eq!(end, tip);
        // A tip inside the badge points at nothing.
        assert!(badge_wedge(center, 20.0, Point::new(55.0, 55.0)).is_none());
    }

    #[test]
    fn a_plate_s_wedge_leaves_by_the_edge_facing_the_tip() {
        let plate = Bounds::new(0.0, 0.0, 200.0, 40.0);
        // Below: the base is on the bottom edge, inset, and clear of the corners.
        let [a, b, _] = plate_wedge(plate, 8.0, 10.0, Point::new(60.0, 140.0)).expect("a tail");
        assert!(plate.contains(a) && plate.contains(b), "{a:?} {b:?}");
        assert!((a.y - 30.0).abs() < 1e-9 && (b.y - 30.0).abs() < 1e-9);
        // Far to the right: the base is on the right edge.
        let [a, b, _] = plate_wedge(plate, 8.0, 10.0, Point::new(600.0, 30.0)).expect("a tail");
        assert!((a.x - 190.0).abs() < 1e-9 && (b.x - 190.0).abs() < 1e-9, "{a:?} {b:?}");
        assert!(((a.y + b.y) / 2.0 - 22.0).abs() < 1e-9, "{a:?} {b:?}");
    }

    #[test]
    fn a_plate_s_wedge_keeps_clear_of_the_corners() {
        let plate = Bounds::new(0.0, 0.0, 200.0, 40.0);
        // A tip low and far to the left leaves by the bottom edge near its left corner, and
        // the base stays inside the corner's round.
        let [a, b, _] = plate_wedge(plate, 8.0, 10.0, Point::new(-5.0, 45.0)).expect("a tail");
        assert!(a.x.min(b.x) >= 8.0 - 1e-9, "{a:?} {b:?}");
    }

    #[test]
    fn a_plain_label_s_line_leaves_from_its_box() {
        let label = Bounds::new(0.0, 0.0, 100.0, 20.0);
        let line = line(label, Point::new(50.0, 100.0)).expect("a tail");
        assert_eq!(line.start, Point::new(50.0, 20.0));
        assert!(super::line(label, Point::new(10.0, 10.0)).is_none());
    }

    #[test]
    fn a_tail_is_hit_along_its_length_and_not_past_its_tip() {
        let body = Bounds::new(0.0, 0.0, 40.0, 40.0);
        let tip = Point::new(20.0, 200.0);
        assert!(hits(body, tip, Point::new(22.0, 120.0), 4.0));
        assert!(!hits(body, tip, Point::new(20.0, 220.0), 4.0));
        assert!(!hits(body, tip, Point::new(60.0, 120.0), 4.0));
    }

    #[test]
    fn a_new_tip_is_on_the_canvas() {
        let canvas = Bounds::new(0.0, 0.0, 400.0, 300.0);
        let tip = default_tip(Bounds::new(20.0, 20.0, 40.0, 40.0), canvas);
        assert!(tip.x > 60.0 && tip.y > 60.0, "down and to the right: {tip:?}");
        // In the bottom-right corner it turns up and to the left.
        let tip = default_tip(Bounds::new(340.0, 240.0, 40.0, 40.0), canvas);
        assert!(canvas.contains(tip) && tip.x < 340.0 && tip.y < 240.0, "{tip:?}");
    }
}
