// SPDX-License-Identifier: GPL-3.0-or-later

//! Which object the pointer is over (`spec/05` §6: "hit testing uses object geometry,
//! never pixels").
//!
//! Geometry and not pixels for three reasons, in order of weight. It works at any zoom,
//! because there is nothing to sample. It gives the right answer for a hollow shape --
//! clicking inside an unfilled rectangle selects what is *behind* it, which is what the
//! shape looks like. And it needs no render, so a click costs no frame.

use crate::geometry::{Bounds, CURVE_LEGS, Point, Segment, quad_flatten};
use crate::object::{Geometry, Object, ObjectId, ObjectKind};
use crate::scene::Scene;

/// How far from a stroke still counts as on it, in image pixels.
///
/// A hairline at level 1 is one pixel wide and nobody can click a one-pixel line. The
/// tolerance is added to half the stroke, so a heavy stroke is easier to hit than a thin
/// one -- which matches what the user sees -- and a thin one is still reachable.
///
/// Image pixels, which is the right unit for the document and the wrong one for a
/// pointer: at 40 % zoom four image pixels are not even two on screen. So this is the
/// *default*, and [`topmost_within`] takes the tolerance the editor has worked out from
/// its zoom -- the scene stays ignorant of zoom, as `spec/05` §6 wants, and the click
/// target stays the same size on screen.
pub const TOLERANCE: f64 = 4.0;

/// The topmost object at `point`, or `None`.
///
/// Topmost by *render order*, not by `z`: what the user clicks is what they can see, and
/// `spec/05` §5.3 puts counters above everything regardless of z. Anything else would
/// mean clicking a counter and selecting the rectangle behind it.
///
/// Locked objects are skipped. `spec/05` §5.1 has a `locked` flag and the only thing it
/// can usefully mean is "not the thing I am reaching for".
#[must_use]
pub fn topmost(scene: &Scene, point: Point) -> Option<ObjectId> {
    topmost_within(scene, point, TOLERANCE)
}

/// [`topmost`], with the stroke tolerance the caller wants -- see [`TOLERANCE`].
#[must_use]
pub fn topmost_within(scene: &Scene, point: Point, tolerance: f64) -> Option<ObjectId> {
    topmost_measured(scene, point, tolerance, &|_| None)
}

/// [`topmost_within`], with the caller's own measurement of the objects it can measure.
///
/// A text object's box is a Pango layout, which only a widget with a font map has;
/// [`Object::bounds`] estimates it at 0.6 em a character, and an estimate is a hit box the
/// user cannot see. `measure` answers `Some` for the objects the caller can measure and
/// `None` for the rest, which are hit by their geometry as usual.
#[must_use]
pub fn topmost_measured(
    scene: &Scene,
    point: Point,
    tolerance: f64,
    measure: &dyn Fn(&Object) -> Option<Bounds>,
) -> Option<ObjectId> {
    scene.render_order().into_iter().rev().find(|id| {
        scene.get(*id).is_some_and(|o| {
            !o.locked
                && match measure(o) {
                    Some(bounds) => bounds.normalised().contains(point),
                    None => hits_within(o, point, tolerance),
                }
        })
    })
}

/// Every object at `point`, topmost first. For a right-click menu that offers a choice.
#[must_use]
pub fn all_at(scene: &Scene, point: Point) -> Vec<ObjectId> {
    scene
        .render_order()
        .into_iter()
        .rev()
        .filter(|id| scene.get(*id).is_some_and(|o| !o.locked && hits(o, point)))
        .collect()
}

/// Whether one object is under the pointer.
#[must_use]
pub fn hits(object: &Object, point: Point) -> bool {
    hits_within(object, point, TOLERANCE)
}

/// [`hits`], with the stroke tolerance the caller wants.
#[must_use]
pub fn hits_within(object: &Object, point: Point, tolerance: f64) -> bool {
    let reach = object.style.stroke_width() / 2.0 + tolerance;
    match &object.geometry {
        // An arrow's shaft plus its head. A curved shaft is hit along the *curve* --
        // sixteen legs of it -- and not along its two control legs, which the first
        // version used on the theory that they were "within a stroke" of the curve. They
        // are not: a quadratic passes half-way between its chord and its control point,
        // so at the bend the legs were a quarter of the bow away from the arrow and the
        // control point -- selection chrome -- was clickable while the visible middle was
        // not. Reported from hardware as "at the bend the hitbox disappears" (D57).
        //
        // The head reaches further than the shaft (`style::arrowhead_reach`), and a click
        // on its wing is a click on the arrow: the reach is widened to most of that, so
        // the heavy head a level-6 arrow carries is as clickable as it looks.
        Geometry::Arrow { start, end, ctrl, .. } => {
            let reach = reach.max(crate::style::arrowhead_reach(object.style.stroke_width()) * 0.75);
            match ctrl {
                Some(c) => quad_flatten(*start, *c, *end, CURVE_LEGS)
                    .windows(2)
                    .any(|leg| Segment::new(leg[0], leg[1]).distance_to(point) <= reach),
                None => Segment::new(*start, *end).distance_to(point) <= reach,
            }
        }
        Geometry::Line { start, end } => Segment::new(*start, *end).distance_to(point) <= reach,
        // A filled rectangle is hit anywhere inside; an outlined one only on its edge,
        // because that is what it looks like -- see the note at the top of this file.
        Geometry::Rect { bounds, filled, .. } => {
            if *filled {
                bounds.inflated(reach).contains(point)
            } else {
                on_rect_edge(*bounds, point, reach)
            }
        }
        Geometry::Ellipse { bounds } => {
            bounds.inflated(reach).ellipse_contains(point)
                && !bounds.inflated(-reach).ellipse_contains(point)
        }
        // Text, redactions, images and spotlights are solid: every one of them is
        // something the user drew *over* the image, so its whole area belongs to it.
        Geometry::Text { .. } => object.bounds().contains(point),
        Geometry::Redact { bounds, .. } | Geometry::Image { bounds, .. } => {
            bounds.contains(point)
        }
        Geometry::Spotlight { shape, bounds, .. } => match shape {
            crate::style::SpotlightShape::Ellipse => bounds.ellipse_contains(point),
            _ => bounds.contains(point),
        },
        Geometry::Path { points, .. } => points
            .windows(2)
            .any(|leg| Segment::new(leg[0], leg[1]).distance_to(point) <= reach)
            // A single-point path is a dot the pencil put down without moving.
            || points.len() == 1 && points[0].distance_to(point) <= reach,
        Geometry::Counter { center, radius, .. } => center.distance_to(point) <= radius + reach,
        // Neither is a thing on the canvas: the background is the canvas and a crop is a
        // transform of it. Both are edited through their own mode, never by being clicked.
        Geometry::Background { .. } | Geometry::Crop { .. } => false,
    }
}

/// Within `reach` of a rectangle's outline, inside or out.
fn on_rect_edge(bounds: Bounds, point: Point, reach: f64) -> bool {
    bounds.inflated(reach).contains(point) && !bounds.inflated(-reach).contains(point)
}

/// Every object the `marquee` touches, in render order.
///
/// Bounds and not shape, deliberately: a rubber-band selection is about the region the
/// user swept, and asking whether a hollow ellipse's *outline* is enclosed would leave
/// out the one in the middle of the band.
///
/// **Touching and not enclosing** (D99). The rule was full containment -- both
/// corners of an object inside the band -- which is the strictest one there is, and it
/// reads as a feature that does not work: a band swept across four arrows selects none of
/// them, because a band that crosses an object does not contain it. The user has to guess
/// every object's extent and sweep wider than all of them at once, on a canvas where the
/// objects are what they are looking at. Intersection is what a sweep means.
#[must_use]
pub fn within(scene: &Scene, marquee: Bounds) -> Vec<ObjectId> {
    let region = marquee.normalised();
    scene
        .render_order()
        .into_iter()
        .filter(|id| {
            scene.get(*id).is_some_and(|o| {
                !o.locked
                    && !matches!(o.kind(), ObjectKind::Background | ObjectKind::Crop)
                    && touches(region, o.bounds().normalised())
            })
        })
        .collect()
}

/// Whether two normalised rectangles overlap at all.
///
/// Zero-sized counts: a horizontal line has no height and a vertical one no width, and
/// both are objects a band swept over them plainly touched.
fn touches(region: Bounds, bounds: Bounds) -> bool {
    region.x <= bounds.x + bounds.width
        && bounds.x <= region.x + region.width
        && region.y <= bounds.y + bounds.height
        && bounds.y <= region.y + region.height
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::background::BackgroundParams;
    use crate::scene::Base;
    use crate::style::{CounterStyle, Rgba, Style};

    fn scene() -> Scene {
        Scene::new(Base::new("base.png", 1000.0, 1000.0, 1.0))
    }

    fn style(size: u8) -> Style {
        Style::new(Rgba::new(1.0, 0.0, 0.0, 1.0), size, false)
    }

    fn object(z: i32, geometry: Geometry) -> Object {
        Object::new(z, 0, style(1), geometry)
    }

    #[test]
    fn a_thin_line_is_still_clickable() {
        let line = object(0, Geometry::Line {
            start: Point::new(0.0, 100.0),
            end: Point::new(200.0, 100.0),
        });
        // Level 1 is a one-pixel stroke: without the tolerance this would need the exact
        // pixel.
        assert!(hits(&line, Point::new(100.0, 102.0)));
        assert!(!hits(&line, Point::new(100.0, 120.0)));
        // And not past the end, which is `Segment::distance_to`'s doing.
        assert!(!hits(&line, Point::new(400.0, 100.0)));
    }

    /// The editor widens the tolerance as it zooms out, so a hairline stays clickable
    /// at 40 % -- `TOLERANCE` is the default, not the rule.
    #[test]
    fn the_tolerance_is_the_callers_to_widen() {
        let line = object(0, Geometry::Line {
            start: Point::new(0.0, 100.0),
            end: Point::new(200.0, 100.0),
        });
        let far = Point::new(100.0, 100.0 + TOLERANCE + 5.0);
        assert!(!hits(&line, far));
        assert!(hits_within(&line, far, TOLERANCE + 6.0));
        let mut scene = scene();
        scene.add(line);
        assert_eq!(topmost(&scene, far), None);
        assert!(topmost_within(&scene, far, TOLERANCE + 6.0).is_some());
    }

    #[test]
    fn a_heavier_stroke_is_easier_to_hit() {
        let thin = Object::new(0, 0, style(1), Geometry::Line {
            start: Point::new(0.0, 0.0),
            end: Point::new(100.0, 0.0),
        });
        let thick = Object::new(0, 0, style(6), Geometry::Line {
            start: Point::new(0.0, 0.0),
            end: Point::new(100.0, 0.0),
        });
        let at = Point::new(50.0, 9.0);
        assert!(!hits(&thin, at));
        assert!(hits(&thick, at), "a 13 px stroke reaches 6.5 px plus the tolerance");
    }

    /// The behaviour the "geometry, never pixels" rule buys: an outlined rectangle is
    /// hollow, so clicking the middle reaches what is behind it.
    #[test]
    fn an_outlined_rectangle_is_hollow_and_a_filled_one_is_not() {
        let bounds = Bounds::new(100.0, 100.0, 200.0, 200.0);
        let outlined = object(0, Geometry::Rect { bounds, filled: false, radius: 0.0 });
        let filled = object(0, Geometry::Rect { bounds, filled: true, radius: 0.0 });
        let middle = Point::new(200.0, 200.0);
        assert!(!hits(&outlined, middle));
        assert!(hits(&filled, middle));
        // Both are hit on the edge.
        let edge = Point::new(100.0, 200.0);
        assert!(hits(&outlined, edge));
        assert!(hits(&filled, edge));
    }

    #[test]
    fn an_ellipse_is_hit_on_its_outline_not_in_its_middle() {
        let e = object(0, Geometry::Ellipse { bounds: Bounds::new(0.0, 0.0, 200.0, 100.0) });
        assert!(hits(&e, Point::new(0.0, 50.0)), "the left of the outline");
        assert!(!hits(&e, Point::new(100.0, 50.0)), "the centre is empty");
        assert!(!hits(&e, Point::new(5.0, 5.0)), "the bounding box's corner is outside it");
    }

    #[test]
    fn a_pencil_stroke_is_hit_along_any_of_its_legs() {
        let path = object(0, Geometry::Path {
            points: vec![Point::new(0.0, 0.0), Point::new(100.0, 0.0), Point::new(100.0, 100.0)],
            smoothing: true,
            highlighter: false,
            band: None,
        });
        assert!(hits(&path, Point::new(50.0, 1.0)), "the first leg");
        assert!(hits(&path, Point::new(99.0, 50.0)), "the second");
        assert!(!hits(&path, Point::new(50.0, 50.0)), "the space the corner encloses");
    }

    /// A click without a drag: the pencil put down one point and it must still be
    /// selectable.
    #[test]
    fn a_single_point_stroke_is_a_dot_that_can_be_hit() {
        let dot = object(0, Geometry::Path {
            points: vec![Point::new(40.0, 40.0)],
            smoothing: false,
            highlighter: false,
            band: None,
        });
        assert!(hits(&dot, Point::new(41.0, 41.0)));
        assert!(!hits(&dot, Point::new(80.0, 80.0)));
    }

    #[test]
    fn a_counter_is_a_disc() {
        let c = object(0, Geometry::Counter {
            center: Point::new(100.0, 100.0),
            number: 3,
            style: CounterStyle::Arabic,
            radius: 12.0,
        });
        assert!(hits(&c, Point::new(100.0, 100.0)));
        assert!(hits(&c, Point::new(110.0, 100.0)));
        assert!(!hits(&c, Point::new(140.0, 100.0)));
    }

    /// Topmost is by *render order*: a counter drawn first is still what the click gets,
    /// because it is what the user can see.
    #[test]
    fn the_click_goes_to_what_is_visible_not_to_the_highest_z() {
        let mut s = scene();
        let counter = object(0, Geometry::Counter {
            center: Point::new(100.0, 100.0),
            number: 1,
            style: CounterStyle::Arabic,
            radius: 20.0,
        });
        let rect = object(99, Geometry::Rect {
            bounds: Bounds::new(50.0, 50.0, 100.0, 100.0),
            filled: true,
            radius: 0.0,
        });
        let counter_id = counter.id;
        s.add(counter);
        s.add(rect);
        assert_eq!(topmost(&s, Point::new(100.0, 100.0)), Some(counter_id));
    }

    #[test]
    fn a_locked_object_is_not_what_the_pointer_is_reaching_for() {
        let mut s = scene();
        let mut locked = object(10, Geometry::Rect {
            bounds: Bounds::new(0.0, 0.0, 500.0, 500.0),
            filled: true,
            radius: 0.0,
        });
        locked.locked = true;
        let under = object(0, Geometry::Rect {
            bounds: Bounds::new(0.0, 0.0, 500.0, 500.0),
            filled: true,
            radius: 0.0,
        });
        let under_id = under.id;
        s.add(under);
        s.add(locked);
        assert_eq!(topmost(&s, Point::new(100.0, 100.0)), Some(under_id));
    }

    #[test]
    fn nothing_under_the_pointer_is_none() {
        let mut s = scene();
        s.add(object(0, Geometry::Line {
            start: Point::new(0.0, 0.0),
            end: Point::new(10.0, 0.0),
        }));
        assert_eq!(topmost(&s, Point::new(900.0, 900.0)), None);
    }

    /// The background and the crop are not things on the canvas, so a click never lands
    /// on them -- they are edited through their own modes.
    #[test]
    fn the_background_and_the_crop_take_no_clicks() {
        let mut s = scene();
        s.add(object(0, Geometry::Background { params: BackgroundParams::default(), source: None }));
        s.add(object(1, Geometry::Crop { canvas_rect: Bounds::new(0.0, 0.0, 500.0, 500.0) }));
        assert_eq!(topmost(&s, Point::new(100.0, 100.0)), None);
        assert!(within(&s, Bounds::new(-10.0, -10.0, 2000.0, 2000.0)).is_empty());
    }

    #[test]
    fn all_at_lists_every_object_under_the_pointer_topmost_first() {
        let mut s = scene();
        let bounds = Bounds::new(0.0, 0.0, 200.0, 200.0);
        let low = object(0, Geometry::Rect { bounds, filled: true, radius: 0.0 });
        let high = object(1, Geometry::Rect { bounds, filled: true, radius: 0.0 });
        let (low_id, high_id) = (low.id, high.id);
        s.add(low);
        s.add(high);
        assert_eq!(all_at(&s, Point::new(100.0, 100.0)), vec![high_id, low_id]);
    }

    /// A marquee takes what it touches, and takes it by bounds -- so the ellipse in the
    /// middle of the band is included rather than skipped for having a hollow centre, and
    /// so is the rectangle the band only crosses.
    ///
    /// It used to take only what it fully enclosed (D99), which is why the
    /// `straddling` object below is now expected and was not before: a band swept across
    /// a row of arrows selected none of them, and that reads as a feature that is missing
    /// rather than as a rule that is strict.
    #[test]
    fn a_marquee_takes_what_it_touches() {
        let mut s = scene();
        let inside = object(0, Geometry::Ellipse { bounds: Bounds::new(100.0, 100.0, 50.0, 50.0) });
        let straddling = object(1, Geometry::Rect {
            bounds: Bounds::new(250.0, 100.0, 200.0, 50.0),
            filled: false,
            radius: 0.0,
        });
        let (inside_id, straddling_id) = (inside.id, straddling.id);
        s.add(inside);
        s.add(straddling);
        let both = vec![inside_id, straddling_id];
        assert_eq!(within(&s, Bounds::new(50.0, 50.0, 250.0, 250.0)), both);
        // And a backwards marquee is the same marquee.
        assert_eq!(within(&s, Bounds::new(300.0, 300.0, -250.0, -250.0)), both);
        // A band that reaches neither still takes neither.
        assert!(within(&s, Bounds::new(0.0, 0.0, 10.0, 10.0)).is_empty());
    }

    /// The band is a sweep, not a click: an object it merely grazes is one the user drew
    /// their rectangle over, and a line with no thickness in one axis is still an object.
    #[test]
    fn a_band_that_grazes_an_edge_has_touched_it() {
        let mut s = scene();
        let line = object(0, Geometry::Line {
            start: Point::new(100.0, 100.0),
            end: Point::new(300.0, 100.0),
        });
        let id = line.id;
        s.add(line);
        assert_eq!(within(&s, Bounds::new(0.0, 0.0, 100.0, 100.0)), vec![id], "edge to edge");
        assert!(within(&s, Bounds::new(0.0, 0.0, 99.0, 99.0)).is_empty(), "short of it");
    }

    /// D57: the hit box is the curve the user sees -- its bend included -- and not the
    /// control point, which is chrome.
    #[test]
    fn an_arrows_curve_is_hit_along_the_curve_itself_and_not_at_its_control_point() {
        let curved = object(0, Geometry::Arrow {
            start: Point::new(0.0, 0.0),
            end: Point::new(200.0, 0.0),
            ctrl: Some(Point::new(100.0, 100.0)),
            style: crate::style::ArrowStyle::Curved,
            head: crate::style::ArrowHead::default(),
        });
        // The bend: half-way between the chord and the control point.
        assert!(hits(&curved, Point::new(100.0, 50.0)), "the visible middle of the arc");
        assert!(hits(&curved, Point::new(50.0, 37.5)), "a quarter of the way along");
        assert!(!hits(&curved, Point::new(100.0, 100.0)), "the control point is not the arrow");
        assert!(!hits(&curved, Point::new(100.0, 0.0)), "the chord it bows away from");
        // And the bounds are the curve's, not the control point's.
        assert!(curved.bounds().y + curved.bounds().height < 80.0, "{:?}", curved.bounds());
    }

    /// The caller's measurement wins over the estimate for the objects it can measure.
    #[test]
    fn a_measured_object_is_hit_by_its_measured_box() {
        let mut scene = scene();
        let label = object(0, Geometry::Text {
            pos: Point::new(100.0, 100.0),
            text: "ok".to_owned(),
            style: crate::style::TextStyle::Standard,
            font_size: 20.0,
            align: crate::object::TextAlign::Start,
            width: None,
        });
        let id = label.id;
        scene.add(label);
        // The estimate is about 24 x 24; a real layout says the label is far wider.
        let far = Point::new(180.0, 110.0);
        assert_eq!(topmost_within(&scene, far, TOLERANCE), None);
        let measured = topmost_measured(&scene, far, TOLERANCE, &|o| {
            matches!(o.geometry, Geometry::Text { .. }).then(|| Bounds::new(100.0, 100.0, 120.0, 24.0))
        });
        assert_eq!(measured, Some(id));
    }
}
