// SPDX-License-Identifier: GPL-3.0-or-later

//! What a pencil stroke held still at its end was meant to be (`ANN-28`, D177): a straight
//! line, a rectangle or an ellipse, or nothing, in which case it stays the stroke it is.
//!
//! Arithmetic on the stroke's points and nothing else, so every case is a test. The rules
//! are deliberately few, because a wrong guess costs the user an undo and a second try:
//!
//! - **A line** when every point lies close to the chord between the stroke's ends.
//! - **A closed shape** only when the ends meet and the stroke goes once round the middle,
//!   then whichever of the rectangle and the ellipse its bounds describe lies closer to the
//!   points, and only when it lies close.
//!   The two are told apart by where the stroke goes near the corners: a rectangle's go
//!   into them, an ellipse's cut across them at 0.29 of the half-width from the box.
//! - **Nothing** for anything smaller than [`MIN_SIZE`], an open curve, or a scribble.
//!
//! A rectangle is axis-aligned, as the rectangle tool's are.

use crate::geometry::{Bounds, Point, Segment};

/// The smallest shape a held stroke becomes, on the diagonal: a hold on a tick or a dot is
/// a pause, not a request.
pub const MIN_SIZE: f64 = 16.0;

/// How far from the chord a stroke may stray and still be a line, as a share of its length.
const LINE_SLACK: f64 = 0.06;

/// How far apart a closed shape's ends may be, as a share of its bounds' diagonal.
const CLOSED_GAP: f64 = 0.2;

/// How far, on average, the points may lie from the shape they are taken for, as a share
/// of its bounds' diagonal.
const FIT: f64 = 0.05;

/// How much of a turn round its middle a closed shape's stroke has to make. A zigzag whose
/// ends meet lies on its bounds' edges as well as a box does, and goes nowhere round.
const ROUND: f64 = 0.8;

/// A shape a held stroke was taken for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    Line { start: Point, end: Point },
    Rect { bounds: Bounds },
    Ellipse { bounds: Bounds },
}

impl Shape {
    /// The shape's name, for a log line.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Line { .. } => "line",
            Self::Rect { .. } => "rectangle",
            Self::Ellipse { .. } => "ellipse",
        }
    }
}

/// The shape `points` were meant to be, or `None` to leave them a stroke.
#[must_use]
pub fn recognise(points: &[Point]) -> Option<Shape> {
    let (&first, &last) = (points.first()?, points.last()?);
    if points.len() < 3 || points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
        return None;
    }
    let bounds = points
        .iter()
        .fold(Bounds::from_corners(first, first), |b, &p| b.union(Bounds::from_corners(p, p)));
    let diagonal = bounds.width.hypot(bounds.height);
    if diagonal < MIN_SIZE {
        return None;
    }

    let chord = Segment::new(first, last);
    let length = first.distance_to(last);
    if length >= MIN_SIZE {
        let furthest = points.iter().map(|&p| chord.distance_to(p)).fold(0.0, f64::max);
        if furthest <= LINE_SLACK * length {
            return Some(Shape::Line { start: first, end: last });
        }
    }

    // A closed shape, with room inside it: a thin loop is a scribble along a line.
    if length > CLOSED_GAP * diagonal || bounds.width.min(bounds.height) < MIN_SIZE / 2.0 {
        return None;
    }
    if turn(bounds.center(), points).abs() < ROUND * std::f64::consts::TAU {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let count = points.len() as f64;
    let rect_error = points.iter().map(|&p| off_rect(bounds, p)).sum::<f64>() / count;
    let ellipse_error = points.iter().map(|&p| off_ellipse(bounds, p)).sum::<f64>() / count;
    let (shape, error) = if rect_error <= ellipse_error {
        (Shape::Rect { bounds }, rect_error)
    } else {
        (Shape::Ellipse { bounds }, ellipse_error)
    };
    (error <= FIT * diagonal).then_some(shape)
}

/// How far round `centre` the stroke goes, in radians, one way counted against the other.
fn turn(centre: Point, points: &[Point]) -> f64 {
    let angle = |p: &Point| (p.y - centre.y).atan2(p.x - centre.x);
    points
        .windows(2)
        .map(|pair| {
            let step = angle(&pair[1]) - angle(&pair[0]);
            // Across the ray at ±π the difference wraps: the short way round is the step.
            if step > std::f64::consts::PI {
                step - std::f64::consts::TAU
            } else if step < -std::f64::consts::PI {
                step + std::f64::consts::TAU
            } else {
                step
            }
        })
        .sum()
}

/// How far `p` lies from the nearest edge of `b`.
fn off_rect(b: Bounds, p: Point) -> f64 {
    let (left, right) = ((p.x - b.x).abs(), (b.x + b.width - p.x).abs());
    let (top, bottom) = ((p.y - b.y).abs(), (b.y + b.height - p.y).abs());
    left.min(right).min(top).min(bottom)
}

/// About how far `p` lies from the ellipse inscribed in `b`: along the ray from the centre,
/// which is exact on the axes and close enough between them to rank two fits.
fn off_ellipse(b: Bounds, p: Point) -> f64 {
    let (a, c) = (b.width / 2.0, b.height / 2.0);
    let centre = b.center();
    let (dx, dy) = (p.x - centre.x, p.y - centre.y);
    let r = (dx / a).hypot(dy / c);
    if r == 0.0 {
        return a.min(c);
    }
    // The point on the ellipse along the same ray is (dx, dy) / r.
    let along = dx.hypot(dy);
    (along - along / r).abs()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    /// A hand's wobble: a fixed, small, uneven offset, so a test fails the same way twice.
    fn wobble(i: usize, by: f64) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let i = i as f64;
        by * ((i * 1.7).sin() + 0.5 * (i * 0.37).cos())
    }

    fn line(from: Point, to: Point, steps: usize, by: f64) -> Vec<Point> {
        (0..=steps)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let t = i as f64 / steps as f64;
                Point::new(
                    from.x + t * (to.x - from.x),
                    from.y + t * (to.y - from.y) + wobble(i, by),
                )
            })
            .collect()
    }

    /// Round a box from its top-left corner, `gap` short of where it started.
    fn rect(b: Bounds, by: f64, gap: f64) -> Vec<Point> {
        let corners = [
            Point::new(b.x, b.y),
            Point::new(b.x + b.width, b.y),
            Point::new(b.x + b.width, b.y + b.height),
            Point::new(b.x, b.y + b.height),
            Point::new(b.x, b.y + gap),
        ];
        let mut points = Vec::new();
        for pair in corners.windows(2) {
            let mut leg = line(pair[0], pair[1], 40, 0.0);
            leg.pop();
            points.extend(leg);
        }
        points.push(corners[4]);
        points
            .into_iter()
            .enumerate()
            .map(|(i, p)| Point::new(p.x + wobble(i, by), p.y + wobble(i + 7, by)))
            .collect()
    }

    /// Round an ellipse, `missing` of a turn short of where it started.
    fn ellipse(b: Bounds, by: f64, missing: f64) -> Vec<Point> {
        let c = b.center();
        (0..=120)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let t = (i as f64 / 120.0) * TAU * (1.0 - missing);
                let r = 1.0 + wobble(i, by) / (b.width / 2.0);
                Point::new(c.x + r * b.width / 2.0 * t.cos(), c.y + r * b.height / 2.0 * t.sin())
            })
            .collect()
    }

    #[test]
    fn a_wobbly_straight_stroke_is_a_line_between_its_ends() {
        let points = line(Point::new(10.0, 20.0), Point::new(310.0, 140.0), 80, 3.0);
        let Some(Shape::Line { start, end }) = recognise(&points) else { panic!("not a line") };
        assert_eq!((start, end), (points[0], *points.last().unwrap()));
    }

    #[test]
    fn a_loop_round_a_box_is_a_rectangle_of_its_bounds() {
        let b = Bounds::new(50.0, 40.0, 300.0, 180.0);
        let Some(Shape::Rect { bounds }) = recognise(&rect(b, 2.5, 12.0)) else {
            panic!("not a rectangle")
        };
        assert!(
            (bounds.x - b.x).abs() < 6.0 && (bounds.width - b.width).abs() < 12.0,
            "{bounds:?}"
        );
    }

    #[test]
    fn a_loop_round_an_oval_is_an_ellipse_of_its_bounds() {
        let b = Bounds::new(0.0, 0.0, 260.0, 140.0);
        let Some(Shape::Ellipse { bounds }) = recognise(&ellipse(b, 2.5, 0.04)) else {
            panic!("not an ellipse")
        };
        assert!((bounds.width - b.width).abs() < 12.0, "{bounds:?}");
    }

    #[test]
    fn a_circle_and_a_square_are_told_apart_at_every_size() {
        for side in [24.0, 60.0, 200.0, 900.0] {
            let b = Bounds::new(10.0, 10.0, side, side);
            assert!(matches!(recognise(&rect(b, 0.0, 0.0)), Some(Shape::Rect { .. })), "{side}");
            assert!(
                matches!(recognise(&ellipse(b, 0.0, 0.0)), Some(Shape::Ellipse { .. })),
                "{side}"
            );
        }
    }

    #[test]
    fn an_open_curve_stays_a_stroke() {
        let b = Bounds::new(0.0, 0.0, 200.0, 200.0);
        // Half an ellipse: its ends are a diameter apart.
        assert_eq!(recognise(&ellipse(b, 0.0, 0.5)), None);
        // An L: two legs of a box.
        let mut l = line(Point::new(0.0, 0.0), Point::new(0.0, 200.0), 40, 0.0);
        l.extend(line(Point::new(0.0, 200.0), Point::new(200.0, 200.0), 40, 0.0));
        assert_eq!(recognise(&l), None);
    }

    #[test]
    fn a_scribble_stays_a_stroke() {
        // A zigzag between two edges that comes back to where it began: every corner of it
        // is on its bounds, as a box's are.
        let corner = |i: i32| Point::new(f64::from(i) * 10.0, if i % 2 == 0 { 0.0 } else { 120.0 });
        let mut z = Vec::new();
        for i in 0..19 {
            z.extend(line(corner(i), corner(i + 1), 30, 0.0));
        }
        z.extend(line(corner(19), Point::new(0.0, 10.0), 60, 0.0));
        assert_eq!(recognise(&z), None);
    }

    #[test]
    fn a_small_or_short_stroke_stays_a_stroke() {
        assert_eq!(recognise(&line(Point::new(0.0, 0.0), Point::new(10.0, 6.0), 6, 0.0)), None);
        assert_eq!(recognise(&ellipse(Bounds::new(0.0, 0.0, 10.0, 8.0), 0.0, 0.0)), None);
        assert_eq!(recognise(&[Point::new(0.0, 0.0), Point::new(100.0, 0.0)]), None);
        assert_eq!(recognise(&[]), None);
    }

    /// Round the corners given, back to the first.
    fn polygon(corners: &[Point]) -> Vec<Point> {
        let mut points = Vec::new();
        for (i, &from) in corners.iter().enumerate() {
            let to = corners[(i + 1) % corners.len()];
            let mut leg = line(from, to, 40, 0.0);
            leg.pop();
            points.extend(leg);
        }
        points.push(corners[0]);
        points
    }

    #[test]
    fn a_diamond_or_a_triangle_is_neither_shape() {
        // A diamond's corners are on the middles of its bounds' edges, where an ellipse's
        // are too, and it cuts across the box's corners as an ellipse does: closer to an
        // ellipse than a box is, and still not close.
        let diamond = [
            Point::new(100.0, 0.0),
            Point::new(200.0, 100.0),
            Point::new(100.0, 200.0),
            Point::new(0.0, 100.0),
        ];
        assert_eq!(recognise(&polygon(&diamond)), None);
        let triangle = [Point::new(0.0, 200.0), Point::new(200.0, 200.0), Point::new(100.0, 0.0)];
        assert_eq!(recognise(&polygon(&triangle)), None);
    }

    #[test]
    fn a_thin_loop_is_a_scribble_along_a_line() {
        let b = Bounds::new(0.0, 0.0, 300.0, 4.0);
        assert_eq!(recognise(&rect(b, 0.0, 0.0)), None);
    }

    #[test]
    fn a_line_with_a_hook_at_its_end_stays_a_stroke() {
        let mut points = line(Point::new(0.0, 0.0), Point::new(300.0, 0.0), 60, 0.0);
        points.extend(line(Point::new(300.0, 0.0), Point::new(280.0, 40.0), 8, 0.0));
        assert_eq!(recognise(&points), None);
    }

    #[test]
    fn nothing_that_is_not_a_number_is_a_shape() {
        let points = [Point::new(0.0, 0.0), Point::new(f64::NAN, 3.0), Point::new(100.0, 0.0)];
        assert_eq!(recognise(&points), None);
    }
}
