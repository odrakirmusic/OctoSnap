// SPDX-License-Identifier: GPL-3.0-or-later

//! `ANN-34` (D185): a pen's pressure as the pencil's width.
//!
//! A stroke drawn with a pen keeps the pressure at each of its points beside them, 0 to 1
//! (`Geometry::Path::pressure`). Its width at a point is the pencil's width scaled by that:
//! a quarter of it at the lightest touch, the width itself at half pressure, and up to one
//! and three quarters pressed hard. A mouse and a finger have no pressure, and their
//! strokes are one width, as they always were.
//!
//! A stroke whose width changes along it is not a line stroked with one pen, so it is drawn
//! as an area: a disc at each sample of its centre line, and between each two the band
//! that joins them. They are one fill, each wound the same way round, so where they overlap
//! is covered once and a translucent colour is no darker at the joins. The samples follow
//! the curve the pencil's smoothing draws through the points -- quadratics through the
//! midpoints -- so smoothing means the same with a pen as without one.

use crate::geometry::{Bounds, Point};

/// The width's share at the lightest touch, and what pressing hard adds to it.
const LIGHTEST: f64 = 0.25;
const PRESSED: f64 = 1.5;

/// About how far apart the samples of a smoothed stroke are, in document units: the
/// spacing the pencil keeps its points at (`tool::RESAMPLE_SPACING`).
const SPACING: f64 = 2.0;

/// The pressure as it is stored: to a hundredth, which is finer than a hand holds a pen
/// steady, and keeps `objects.json` short.
#[must_use]
pub fn stored(pressure: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)]
    let rounded = ((pressure.clamp(0.0, 1.0) * 100.0).round() / 100.0) as f32;
    rounded
}

/// The stroke's width where the pen pressed `pressure`, for a pencil `width` wide.
#[must_use]
pub fn width_at(width: f64, pressure: f32) -> f64 {
    width * PRESSED.mul_add(f64::from(pressure).clamp(0.0, 1.0), LIGHTEST)
}

/// The widest the stroke gets.
#[must_use]
pub fn widest(width: f64, pressure: &[f32]) -> f64 {
    pressure.iter().map(|p| width_at(width, *p)).fold(width * LIGHTEST, f64::max)
}

/// One piece of a pressed stroke's area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Piece {
    /// The disc around a sample.
    Disc { center: Point, radius: f64 },
    /// The band between two samples' discs: its four corners, clockwise on the screen
    /// (y down), which is the way round a disc is drawn too.
    Band([Point; 4]),
}

/// A sample of the stroke's centre line, and its half-width there.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Sample {
    at: Point,
    radius: f64,
}

/// The area of a stroke through `points`, pressed `pressure` (one for each point), for a
/// pencil `width` wide. Empty when the two do not pair up, which nothing writes.
#[must_use]
pub fn pieces(points: &[Point], pressure: &[f32], width: f64, smoothing: bool) -> Vec<Piece> {
    if points.is_empty() || points.len() != pressure.len() {
        return Vec::new();
    }
    let samples: Vec<Sample> = if smoothing && points.len() >= 3 {
        smoothed(points, pressure, width)
    } else {
        points
            .iter()
            .zip(pressure)
            .map(|(at, p)| Sample { at: *at, radius: width_at(width, *p) / 2.0 })
            .collect()
    };
    let mut out = Vec::with_capacity(samples.len() * 2);
    for (index, sample) in samples.iter().enumerate() {
        out.push(Piece::Disc { center: sample.at, radius: sample.radius });
        if let Some(band) = samples.get(index + 1).and_then(|next| band(*sample, *next)) {
            out.push(Piece::Band(band));
        }
    }
    out
}

/// The box the pieces cover.
#[must_use]
pub fn bounds(pieces: &[Piece]) -> Option<Bounds> {
    pieces
        .iter()
        .map(|piece| match piece {
            Piece::Disc { center, radius } => {
                Bounds::new(center.x - radius, center.y - radius, radius * 2.0, radius * 2.0)
            }
            Piece::Band(corners) => corners[1..]
                .iter()
                .fold(Bounds::new(corners[0].x, corners[0].y, 0.0, 0.0), |b, p| {
                    b.union(Bounds::new(p.x, p.y, 0.0, 0.0))
                }),
        })
        .reduce(Bounds::union)
}

/// The pencil's smoothed curve (`nodes::smooth_path` in the app): from the first point, a
/// quadratic to the middle of each next segment with the point between as its control,
/// then a line to the last point. The half-width runs along each piece the same way, so a
/// change of pressure is as smooth as the line.
fn smoothed(points: &[Point], pressure: &[f32], width: f64) -> Vec<Sample> {
    let radius = |p: f32| width_at(width, p) / 2.0;
    let mid = |a: Point, b: Point| Point::new(f64::midpoint(a.x, b.x), f64::midpoint(a.y, b.y));
    let mut out = vec![Sample { at: points[0], radius: radius(pressure[0]) }];
    let mut from = out[0];
    for i in 1..points.len() - 1 {
        let control = Sample { at: points[i], radius: radius(pressure[i]) };
        let to = Sample {
            at: mid(points[i], points[i + 1]),
            radius: f64::midpoint(control.radius, radius(pressure[i + 1])),
        };
        let reach = from.at.distance_to(control.at) + control.at.distance_to(to.at);
        let steps = steps_over(reach);
        for step in 1..=steps {
            #[allow(clippy::cast_precision_loss)]
            let t = step as f64 / steps as f64;
            out.push(quadratic(from, control, to, t));
        }
        from = to;
    }
    let last = Sample { at: points[points.len() - 1], radius: radius(pressure[points.len() - 1]) };
    let steps = steps_over(from.at.distance_to(last.at));
    for step in 1..=steps {
        #[allow(clippy::cast_precision_loss)]
        let t = step as f64 / steps as f64;
        out.push(Sample {
            at: Point::new(
                (last.at.x - from.at.x).mul_add(t, from.at.x),
                (last.at.y - from.at.y).mul_add(t, from.at.y),
            ),
            radius: (last.radius - from.radius).mul_add(t, from.radius),
        });
    }
    out
}

/// How many samples a piece of curve this long gets: one per [`SPACING`], at least one.
fn steps_over(length: f64) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let steps = (length / SPACING).ceil().clamp(1.0, 64.0) as usize;
    steps
}

fn quadratic(a: Sample, b: Sample, c: Sample, t: f64) -> Sample {
    let (u, v) = (1.0 - t, t);
    let (wa, wb, wc) = (u * u, 2.0 * u * v, v * v);
    Sample {
        at: Point::new(
            wa * a.at.x + wb * b.at.x + wc * c.at.x,
            wa * a.at.y + wb * b.at.y + wc * c.at.y,
        ),
        radius: wa * a.radius + wb * b.radius + wc * c.radius,
    }
}

/// The band from one sample's disc to the next's, or `None` when one disc holds the other
/// and the discs alone are the stroke there.
fn band(a: Sample, b: Sample) -> Option<[Point; 4]> {
    let length = a.at.distance_to(b.at);
    if length <= (a.radius - b.radius).abs() || length <= f64::EPSILON {
        return None;
    }
    let (nx, ny) = (-(b.at.y - a.at.y) / length, (b.at.x - a.at.x) / length);
    let corners = [
        a.at.offset(-nx * a.radius, -ny * a.radius),
        b.at.offset(-nx * b.radius, -ny * b.radius),
        b.at.offset(nx * b.radius, ny * b.radius),
        a.at.offset(nx * a.radius, ny * a.radius),
    ];
    Some(clockwise(corners))
}

/// Twice the signed area: positive when the corners go clockwise on a screen whose y
/// points down.
fn signed_area(corners: &[Point]) -> f64 {
    corners
        .iter()
        .zip(corners.iter().cycle().skip(1))
        .map(|(p, q)| p.x.mul_add(q.y, -(q.x * p.y)))
        .sum()
}

fn clockwise(mut corners: [Point; 4]) -> [Point; 4] {
    if signed_area(&corners) < 0.0 {
        corners.reverse();
    }
    corners
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn half_pressure_is_the_pencils_width() {
        assert_eq!(width_at(8.0, 0.5), 8.0);
        assert_eq!(width_at(8.0, 0.0), 2.0);
        assert_eq!(width_at(8.0, 1.0), 14.0);
        // A reading out of range is the nearest end of it.
        assert_eq!(width_at(8.0, 3.0), 14.0);
        assert_eq!(widest(8.0, &[0.1, 0.9, 0.3]), width_at(8.0, 0.9));
        assert_eq!(widest(8.0, &[]), 2.0);
    }

    #[test]
    fn a_pressure_is_kept_to_a_hundredth() {
        assert_eq!(stored(0.123_456), 0.12);
        assert_eq!(stored(0.996), 1.0);
        assert_eq!(stored(-0.5), 0.0);
        assert_eq!(serde_json::to_string(&stored(0.53)).unwrap(), "0.53");
    }

    #[test]
    fn a_stroke_is_a_disc_at_each_point_and_a_band_between() {
        let points = [Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(20.0, 0.0)];
        let pieces = pieces(&points, &[0.0, 0.5, 1.0], 4.0, false);
        let discs: Vec<f64> = pieces
            .iter()
            .filter_map(|p| match p {
                Piece::Disc { radius, .. } => Some(*radius),
                Piece::Band(_) => None,
            })
            .collect();
        assert_eq!(discs, vec![0.5, 2.0, 3.5]);
        assert_eq!(pieces.iter().filter(|p| matches!(p, Piece::Band(_))).count(), 2);
        // Every band is wound the way a disc is.
        for piece in &pieces {
            if let Piece::Band(corners) = piece {
                assert!(signed_area(corners) > 0.0, "{corners:?}");
            }
        }
    }

    /// Whichever way the stroke goes, its bands are wound clockwise, so the one fill
    /// covers each overlap once and leaves no hole.
    #[test]
    fn every_band_is_wound_the_same_way_whichever_way_the_stroke_goes() {
        let ways = [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0), (1.0, 1.0), (-1.0, 0.5)];
        for (dx, dy) in ways {
            let points: Vec<Point> = (0..5)
                .map(|i| Point::new(dx * 10.0 * f64::from(i), dy * 10.0 * f64::from(i)))
                .collect();
            for piece in pieces(&points, &[0.2, 0.4, 0.6, 0.8, 1.0], 6.0, true) {
                if let Piece::Band(corners) = piece {
                    assert!(signed_area(&corners) > 0.0, "{dx},{dy}: {corners:?}");
                }
            }
        }
    }

    #[test]
    fn a_smoothed_stroke_starts_and_ends_on_its_points() {
        let points = [Point::new(0.0, 0.0), Point::new(20.0, 20.0), Point::new(40.0, 0.0)];
        let pieces = pieces(&points, &[0.0, 0.5, 1.0], 4.0, true);
        let discs: Vec<(Point, f64)> = pieces
            .iter()
            .filter_map(|p| match p {
                Piece::Disc { center, radius } => Some((*center, *radius)),
                Piece::Band(_) => None,
            })
            .collect();
        assert_eq!(discs.first(), Some(&(Point::new(0.0, 0.0), 0.5)));
        assert_eq!(discs.last(), Some(&(Point::new(40.0, 0.0), 3.5)));
        // Sampled about every two units, and the width growing all the way.
        assert!(discs.len() > 20, "{}", discs.len());
        assert!(discs.windows(2).all(|w| w[1].1 >= w[0].1 - 1e-9));
        // The curve passes under its control point, as the pencil's does: never above it.
        assert!(discs.iter().all(|(c, _)| c.y <= 20.0));
    }

    #[test]
    fn pressures_that_do_not_pair_with_the_points_draw_nothing() {
        let points = [Point::new(0.0, 0.0), Point::new(10.0, 0.0)];
        assert!(pieces(&points, &[0.5], 4.0, false).is_empty());
        assert!(pieces(&[], &[], 4.0, false).is_empty());
    }

    #[test]
    fn a_disc_inside_the_next_needs_no_band() {
        let points = [Point::new(0.0, 0.0), Point::new(1.0, 0.0)];
        let pieces = pieces(&points, &[0.0, 1.0], 10.0, false);
        assert_eq!(pieces.len(), 2, "{pieces:?}");
        let covered = bounds(&pieces).unwrap();
        assert_eq!(covered, Bounds::new(-7.75, -8.75, 17.5, 17.5));
    }

    // --- the stroke as the document has it -------------------------------------------

    use crate::object::{Geometry, Object};
    use crate::style::{Rgba, Style};
    use crate::tool::{Draft, Modifiers, Tool, ToolSettings};

    /// A pencil stroke along y = 0 from 0 to 200, pressed lightly at its start and hard
    /// at its end.
    fn pressed(pressure: Option<Vec<f32>>) -> Object {
        Object::new(
            0,
            0,
            Style::new(Rgba::new(0.0, 0.0, 0.0, 1.0), 4, false),
            Geometry::Path {
                points: vec![Point::new(0.0, 0.0), Point::new(100.0, 0.0), Point::new(200.0, 0.0)],
                smoothing: false,
                highlighter: false,
                band: None,
                pressure,
            },
        )
    }

    #[test]
    fn a_pressed_stroke_is_as_wide_as_it_was_pressed() {
        let light = pressed(None);
        let hard = pressed(Some(vec![0.1, 0.5, 1.0]));
        let stroke = light.style.stroke_width();
        assert_eq!(light.bounds().height, stroke);
        assert_eq!(hard.bounds().height, width_at(stroke, 1.0));
        // Hit where it is wide, and not as far out where it is narrow.
        let out = crate::hit::TOLERANCE + width_at(stroke, 1.0) / 2.0 - 0.5;
        assert!(crate::hit::hits(&hard, Point::new(195.0, out)), "the pressed end");
        assert!(!crate::hit::hits(&hard, Point::new(5.0, out)), "the light end");
        assert!(!crate::hit::hits(&light, Point::new(195.0, out)), "an unpressed stroke");
    }

    /// A stroke from before, or a mouse's, writes no `pressure`, so its `objects.json` is
    /// what it was; a pen's keeps its hundredths.
    #[test]
    fn only_a_pressed_stroke_writes_a_pressure() {
        let plain = serde_json::to_string(&pressed(None).geometry).unwrap();
        assert!(!plain.contains("pressure"), "{plain}");
        let hard = pressed(Some(vec![0.1, 0.5, 1.0]));
        let json = serde_json::to_string(&hard.geometry).unwrap();
        assert!(json.contains(r#""pressure":[0.1,0.5,1.0]"#), "{json}");
        let back: Geometry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, hard.geometry);
    }

    fn pencil() -> Draft {
        Draft::begin(Tool::Pencil, ToolSettings::default(), Point::new(0.0, 0.0), Modifiers::none())
            .unwrap()
    }

    fn pressure_of(draft: &Draft) -> Option<Vec<f32>> {
        match draft.geometry(1) {
            Some(Geometry::Path { pressure, .. }) => pressure,
            other => panic!("not a path: {other:?}"),
        }
    }

    #[test]
    fn a_pen_presses_every_point_it_keeps() {
        let mut draft = pencil();
        draft.feel(Some(0.2));
        for (i, p) in [(1, 0.3), (2, 0.4), (3, 0.5)] {
            draft.extend(Point::new(f64::from(i) * 10.0, 0.0), Modifiers::none());
            draft.feel(Some(p));
        }
        assert_eq!(pressure_of(&draft), Some(vec![0.2, 0.3, 0.4, 0.5]));
        // Pressed harder without moving past the spacing: the last point is pressed again.
        draft.extend(Point::new(30.5, 0.0), Modifiers::none());
        draft.feel(Some(0.9));
        // And the tip leaving the surface as the button comes up is not a pressure.
        draft.feel(Some(0.0));
        assert_eq!(pressure_of(&draft), Some(vec![0.2, 0.3, 0.4, 0.9]));
        // Shift's ruler keeps the first point's and the line's end's.
        draft.extend(Point::new(80.0, 40.0), Modifiers::shift());
        draft.feel(Some(0.6));
        assert_eq!(pressure_of(&draft), Some(vec![0.2, 0.6]));
    }

    #[test]
    fn a_mouse_or_one_event_without_a_pressure_makes_the_stroke_one_width() {
        let mut mouse = pencil();
        mouse.feel(None);
        mouse.extend(Point::new(10.0, 0.0), Modifiers::none());
        mouse.feel(None);
        assert_eq!(pressure_of(&mouse), None);

        let mut mixed = pencil();
        mixed.feel(Some(0.5));
        mixed.extend(Point::new(10.0, 0.0), Modifiers::none());
        mixed.feel(None);
        mixed.extend(Point::new(20.0, 0.0), Modifiers::none());
        mixed.feel(Some(0.5));
        assert_eq!(pressure_of(&mixed), None);
    }

    #[test]
    fn only_the_pencil_is_pressed() {
        let mut marker = Draft::begin(
            Tool::Highlighter,
            ToolSettings::default(),
            Point::new(0.0, 0.0),
            Modifiers::none(),
        )
        .unwrap();
        marker.feel(Some(0.8));
        marker.extend(Point::new(40.0, 0.0), Modifiers::none());
        marker.feel(Some(0.8));
        assert_eq!(pressure_of(&marker), None);
    }
}
