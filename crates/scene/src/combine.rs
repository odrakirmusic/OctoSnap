// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §4.14's "add image / combine", as the arithmetic of where a picture lands.
//!
//! > Drop or open an image: inserted as an **image object** with move/resize handles; drop
//! > zones at the four canvas edges extend the canvas and place the image beside the
//! > existing content ("stitch").
//!
//! Five answers, then: one for a drop in the middle of the picture and one for each edge.
//! The middle is an object on top of what is there; an edge is a *stitch*, which means the
//! canvas grows by exactly the room the picture needs and the picture is scaled to meet
//! the edge it was dropped on -- a screenshot stitched to the right of another has to be
//! the same height or the join is a step.
//!
//! The canvas is allowed to grow **backwards**. `Scene::canvas` has had an origin of its
//! own since `spec/05` §4.11's crop learnt to leave the image (D52, §11 item 7), so a drop
//! on the left edge moves the canvas's x rather than moving every object on it -- which is
//! what keeps a stitch one `CanvasChange` and one `Add` instead of an edit to everything.
//!
//! [`place`] answers in the coordinates it is given. A document has two once §4.13's
//! background moves the picture into a margin (D89), and [`insert`] chooses between them:
//! in the middle the picture lands on the picture, and at an edge it is stitched beside
//! the picture's frame, which is laid out again around both -- one `Change` of the
//! background more in the step (D191).

use crate::command::Command;
use crate::geometry::{Bounds, Point};
use crate::object::Geometry;
use crate::scene::Scene;

/// How wide the edge bands are, as a fraction of the canvas's shorter side.
///
/// [M]. A sixth is big enough to aim at with a file under the cursor and small enough that
/// the middle of a picture is unambiguously the middle: on a 1920 × 1080 canvas the bands
/// are 180 px, and the inner region is still 1560 × 720.
pub const BAND: f64 = 1.0 / 6.0;

/// The most of the canvas a picture dropped in the middle is allowed to cover.
///
/// Dropping a 5K photo onto a 1280 × 720 screenshot should leave the screenshot visible;
/// dropping a small logo should not blow it up. So this is a cap and never a target.
pub const MAX_INSIDE: f64 = 0.6;

/// One of `spec/05` §4.14's five landing places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    /// On top of the picture: an ordinary image object.
    Inside,
    Left,
    Right,
    Top,
    Bottom,
}

impl Zone {
    /// The four §4.14 calls "drop zones at the four canvas edges".
    pub const EDGES: [Self; 4] = [Self::Left, Self::Right, Self::Top, Self::Bottom];

    #[must_use]
    pub const fn is_edge(self) -> bool {
        !matches!(self, Self::Inside)
    }

    /// The strip the canvas highlights while a file is over this zone.
    ///
    /// Empty for [`Self::Inside`], which is not a strip: the whole picture is the target
    /// and drawing a band around it would say the opposite.
    #[must_use]
    pub fn band(self, canvas: Bounds) -> Bounds {
        let c = canvas.normalised();
        let thickness = c.width.min(c.height) * BAND;
        match self {
            Self::Inside => Bounds::new(c.x, c.y, 0.0, 0.0),
            Self::Left => Bounds::new(c.x, c.y, thickness, c.height),
            Self::Right => Bounds::new(c.x + c.width - thickness, c.y, thickness, c.height),
            Self::Top => Bounds::new(c.x, c.y, c.width, thickness),
            Self::Bottom => Bounds::new(c.x, c.y + c.height - thickness, c.width, thickness),
        }
    }
}

/// Which zone a point in the canvas is in.
///
/// Corners belong to whichever edge the point is nearer to, so the two bands that overlap
/// there do not need to be drawn as an L -- and a drop in a corner does something rather
/// than nothing.
#[must_use]
pub fn zone_at(canvas: Bounds, at: Point) -> Zone {
    let c = canvas.normalised();
    if c.width <= 0.0 || c.height <= 0.0 {
        return Zone::Inside;
    }
    let thickness = c.width.min(c.height) * BAND;
    let (left, right) = (at.x - c.x, c.x + c.width - at.x);
    let (top, bottom) = (at.y - c.y, c.y + c.height - at.y);
    let nearest = left.min(right).min(top).min(bottom);
    if nearest >= thickness {
        return Zone::Inside;
    }
    if nearest == left {
        Zone::Left
    } else if nearest == right {
        Zone::Right
    } else if nearest == top {
        Zone::Top
    } else {
        Zone::Bottom
    }
}

/// Where a dropped picture goes: its rect, and the canvas it needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Insert {
    /// The image object's bounds.
    pub image: Bounds,
    /// The canvas afterwards, which differs from the one handed in only for an edge drop.
    pub canvas: Bounds,
}

/// `spec/05` §4.14's placement, for a picture of `width` × `height` pixels.
///
/// `at` is where the pointer let go, which only the inside case uses -- an edge stitch
/// lands against its edge and is centred on the other axis, because "beside the existing
/// content" has one answer and it is not wherever the mouse happened to be.
#[must_use]
pub fn place(canvas: Bounds, zone: Zone, at: Point, width: f64, height: f64) -> Option<Insert> {
    let c = canvas.normalised();
    if !(width > 0.0 && height > 0.0 && c.width > 0.0 && c.height > 0.0) {
        return None;
    }
    let aspect = width / height;
    Some(match zone {
        Zone::Inside => {
            // Natural size, capped so a large picture cannot bury the document.
            let cap = (c.width * MAX_INSIDE) / width;
            let cap = cap.min((c.height * MAX_INSIDE) / height).min(1.0);
            let (w, h) = (width * cap, height * cap);
            let x = (at.x - w / 2.0).clamp(c.x, c.x + c.width - w);
            let y = (at.y - h / 2.0).clamp(c.y, c.y + c.height - h);
            Insert { image: Bounds::new(x, y, w, h), canvas: c }
        }
        // The stitch: the picture matches the edge it joins, and the canvas grows by
        // exactly what is left over.
        Zone::Left | Zone::Right => {
            let h = c.height;
            let w = h * aspect;
            let x = if matches!(zone, Zone::Left) { c.x - w } else { c.x + c.width };
            Insert {
                image: Bounds::new(x, c.y, w, h),
                canvas: Bounds::new(c.x.min(x), c.y, c.width + w, c.height),
            }
        }
        Zone::Top | Zone::Bottom => {
            let w = c.width;
            let h = w / aspect;
            let y = if matches!(zone, Zone::Top) { c.y - h } else { c.y + c.height };
            Insert {
                image: Bounds::new(c.x, y, w, h),
                canvas: Bounds::new(c.x, c.y.min(y), c.width, c.height + h),
            }
        }
    })
}

/// An insert into a document: where the picture goes, and what else has to change for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Insertion {
    /// The image object's bounds, in the picture's coordinates like every object's.
    pub image: Bounds,
    /// The rest of the step beside the image's own `Add`: the background a stitch laid out
    /// again, and the canvas it grew.
    pub edits: Vec<Command>,
}

/// Where a picture let go in the middle can land, in the canvas's coordinates: the picture as
/// the canvas shows it, no more of it than the canvas shows, or the whole canvas when no
/// background frames the picture ([`Scene::frame`]). What a drop in the middle highlights.
#[must_use]
pub fn room(scene: &Scene) -> Bounds {
    let canvas = scene.canvas.normalised();
    match scene.frame() {
        Some(_) => scene.picture_in_canvas().intersection(canvas).unwrap_or(canvas),
        None => canvas,
    }
}

/// `spec/05` §4.14's insert into `scene`, for a picture of `width` × `height` let go at `at`,
/// in the canvas's coordinates, which are a pointer's and the canvas's centre's.
///
/// The picture's bounds are in the picture's coordinates, which are every object's (D89).
/// With no background the two are one, and with the crop's own fill too (D52). On one
/// §4.13's panel laid out, the picture is a margin in, perhaps smaller, and over a crop the
/// crop is where it was in the capture (D191):
///
/// - In the middle, it lands on the [`room`], taken into the picture's coordinates, and is
///   capped and kept inside it there. Placed in the canvas's coordinates it was a margin
///   off the middle, and over a crop often where the frame shows nothing.
/// - At an edge, it is stitched beside the picture's frame and matched to its side, and the
///   background is laid out again around the two, as a cut lays it out again around what is
///   left (D183), so the frame, its corners and its shadow hold both (D166). Beside the
///   canvas, it was outside the frame and clipped away, and the canvas grew in its stead.
#[must_use]
pub fn insert(scene: &Scene, zone: Zone, at: Point, width: f64, height: f64) -> Option<Insertion> {
    let placement = scene.placement();
    if !zone.is_edge() {
        let room = placement.invert_bounds(room(scene));
        let placed = place(room, zone, placement.invert(at), width, height)?;
        return Some(Insertion { image: placed.image, edits: Vec::new() });
    }
    let Some((source, params)) = scene.frame() else {
        // The picture's coordinates are the canvas's, and the canvas grows.
        let placed = place(scene.canvas, zone, at, width, height)?;
        let mut edits = Vec::new();
        if placed.canvas != scene.canvas {
            edits.push(Command::CanvasChange { before: scene.canvas, after: placed.canvas });
        }
        return Some(Insertion { image: placed.image, edits });
    };
    // Beside the frame, which grows to hold both: `place`'s canvas is the rect laid out next.
    let placed = place(source.normalised(), zone, at, width, height)?;
    let (id, _) = scene.background()?;
    let was = scene.get(id)?;
    let mut now = was.clone();
    // The rect grows, and the rest stays: the crop's fill it keeps paints the whole rect,
    // behind the stitched picture too (D192).
    if let Geometry::Background { source, .. } = &mut now.geometry {
        *source = Some(placed.canvas);
    }
    let canvas = crate::background::layout(placed.canvas, params).canvas;
    let mut edits =
        vec![Command::Change { id, before: Box::new(was.clone()), after: Box::new(now) }];
    if canvas != scene.canvas {
        edits.push(Command::CanvasChange { before: scene.canvas, after: canvas });
    }
    Some(Insertion { image: placed.image, edits })
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    const CANVAS: Bounds = Bounds::new(0.0, 0.0, 1200.0, 600.0);

    #[test]
    fn the_middle_of_the_picture_is_the_middle() {
        assert_eq!(zone_at(CANVAS, Point::new(600.0, 300.0)), Zone::Inside);
    }

    #[test]
    fn each_edge_has_a_band() {
        // A sixth of 600 is 100.
        assert_eq!(zone_at(CANVAS, Point::new(20.0, 300.0)), Zone::Left);
        assert_eq!(zone_at(CANVAS, Point::new(1190.0, 300.0)), Zone::Right);
        assert_eq!(zone_at(CANVAS, Point::new(600.0, 10.0)), Zone::Top);
        assert_eq!(zone_at(CANVAS, Point::new(600.0, 590.0)), Zone::Bottom);
        assert_eq!(zone_at(CANVAS, Point::new(101.0, 300.0)), Zone::Inside);
    }

    #[test]
    fn a_corner_belongs_to_the_edge_it_is_nearer() {
        // Ten from the top and forty from the left: the top.
        assert_eq!(zone_at(CANVAS, Point::new(40.0, 10.0)), Zone::Top);
        assert_eq!(zone_at(CANVAS, Point::new(10.0, 40.0)), Zone::Left);
    }

    #[test]
    fn a_stitch_to_the_right_matches_the_height_and_grows_the_canvas() {
        let out = place(CANVAS, Zone::Right, Point::new(1190.0, 300.0), 800.0, 400.0)
            .expect("no placement");
        // 800 x 400 at 600 tall is 1200 x 600.
        assert_eq!(out.image, Bounds::new(1200.0, 0.0, 1200.0, 600.0));
        assert_eq!(out.canvas, Bounds::new(0.0, 0.0, 2400.0, 600.0));
    }

    #[test]
    fn a_stitch_to_the_left_grows_the_canvas_backwards() {
        let out =
            place(CANVAS, Zone::Left, Point::new(10.0, 300.0), 300.0, 600.0).expect("no placement");
        assert_eq!(out.image, Bounds::new(-300.0, 0.0, 300.0, 600.0));
        assert_eq!(out.canvas, Bounds::new(-300.0, 0.0, 1500.0, 600.0));
        // Nothing already on the canvas has to move, which is the point of letting the
        // origin go negative.
        assert_eq!(out.canvas.x + out.canvas.width, CANVAS.x + CANVAS.width);
    }

    #[test]
    fn a_stitch_below_matches_the_width() {
        let out = place(CANVAS, Zone::Bottom, Point::new(600.0, 590.0), 1200.0, 300.0)
            .expect("no placement");
        assert_eq!(out.image, Bounds::new(0.0, 600.0, 1200.0, 300.0));
        assert_eq!(out.canvas, Bounds::new(0.0, 0.0, 1200.0, 900.0));
    }

    #[test]
    fn a_stitch_keeps_the_pictures_aspect() {
        for zone in Zone::EDGES {
            let out =
                place(CANVAS, zone, Point::new(0.0, 0.0), 1600.0, 900.0).expect("no placement");
            let aspect = out.image.width / out.image.height;
            assert!((aspect - 16.0 / 9.0).abs() < 1e-9, "{zone:?} sheared it: {aspect}");
        }
    }

    #[test]
    fn a_drop_in_the_middle_is_centred_on_the_pointer_and_leaves_the_canvas_alone() {
        let out = place(CANVAS, Zone::Inside, Point::new(600.0, 300.0), 200.0, 100.0)
            .expect("no placement");
        assert_eq!(out.image, Bounds::new(500.0, 250.0, 200.0, 100.0));
        assert_eq!(out.canvas, CANVAS);
    }

    #[test]
    fn a_huge_picture_dropped_inside_is_capped_rather_than_burying_the_document() {
        let out = place(CANVAS, Zone::Inside, Point::new(600.0, 300.0), 5120.0, 2880.0)
            .expect("no placement");
        assert!(out.image.width <= CANVAS.width * MAX_INSIDE + 1e-9, "{:?}", out.image);
        assert!(out.image.height <= CANVAS.height * MAX_INSIDE + 1e-9, "{:?}", out.image);
        let aspect = out.image.width / out.image.height;
        assert!((aspect - 5120.0 / 2880.0).abs() < 1e-9, "sheared: {aspect}");
    }

    #[test]
    fn a_drop_near_an_edge_stays_inside_the_canvas() {
        let out = place(CANVAS, Zone::Inside, Point::new(10.0, 10.0), 200.0, 100.0)
            .expect("no placement");
        let inside = out.image.x >= CANVAS.x - 1e-9 && out.image.y >= CANVAS.y - 1e-9;
        assert!(inside, "{:?}", out.image);
    }

    #[test]
    fn a_picture_with_no_size_is_refused() {
        assert!(place(CANVAS, Zone::Inside, Point::new(0.0, 0.0), 0.0, 100.0).is_none());
    }

    /// A capture 400 by 300 with a background around `source` at a margin of `padding`.
    fn on_a_background(source: Bounds, padding: f64) -> Scene {
        let params = crate::background::BackgroundParams { padding, ..Default::default() };
        crate::scene::laid_out(400.0, 300.0, source, params)
    }

    /// `scene` after an insert of a `width` by `height` picture let go at `at`, and the
    /// picture's bounds. One step, which undone gives back the document it was.
    fn inserted(scene: &Scene, zone: Zone, at: Point, width: f64, height: f64) -> (Scene, Bounds) {
        use crate::object::{Geometry, Object};
        let insertion = insert(scene, zone, at, width, height).expect("no insert");
        let file = "assets/a.png".to_owned();
        let geometry = Geometry::Image { bounds: insertion.image, file };
        let image = Object::new(scene.top_z(), 0, crate::style::Style::default(), geometry);
        let mut commands = vec![Command::Add(image)];
        commands.extend(insertion.edits);
        let mut after = scene.clone();
        let mut history = crate::command::History::default();
        assert!(history.apply(&mut after, Command::Batch(commands)));
        let mut undone = after.clone();
        assert!(history.undo(&mut undone));
        assert_eq!(&undone, scene, "one undo is not the document it was");
        (after, insertion.image)
    }

    #[test]
    fn without_a_background_an_insert_is_placed_on_the_canvas() {
        let scene = Scene::new(crate::scene::Base::new("base.png", 1200.0, 600.0, 1.0));
        let (after, image) = inserted(&scene, Zone::Inside, scene.canvas.center(), 200.0, 100.0);
        assert_eq!(image, Bounds::new(500.0, 250.0, 200.0, 100.0));
        assert_eq!(after.canvas, scene.canvas);
        let (after, image) = inserted(&scene, Zone::Right, Point::new(1190.0, 300.0), 800.0, 400.0);
        assert_eq!(image, Bounds::new(1200.0, 0.0, 1200.0, 600.0));
        assert_eq!(after.canvas, Bounds::new(0.0, 0.0, 2400.0, 600.0));
    }

    /// D191: on a background a picture let go in the middle lands in the middle of the
    /// picture, at most six tenths of it, in the picture's coordinates like every object.
    #[test]
    fn on_a_background_a_paste_lands_in_the_middle_of_the_picture() {
        // A margin of 100: the canvas is 600 by 500, the picture in it at (100, 100).
        let scene = on_a_background(Bounds::new(0.0, 0.0, 400.0, 300.0), 100.0);
        let (after, image) = inserted(&scene, Zone::Inside, scene.canvas.center(), 100.0, 50.0);
        assert_eq!(image, Bounds::new(150.0, 125.0, 100.0, 50.0));
        assert_eq!(after.canvas, scene.canvas, "nothing grew");
        // A large one is capped by the picture, not the canvas, and kept inside it.
        let (_, image) = inserted(&scene, Zone::Inside, Point::new(590.0, 490.0), 1000.0, 500.0);
        assert_eq!(image, Bounds::new(160.0, 180.0, 240.0, 120.0));
    }

    /// D191: over a crop the picture is the crop, wherever it was in the capture.
    #[test]
    fn over_a_crop_a_paste_lands_on_the_crop() {
        // Cropped to (100, 50, 200, 150), then a margin of 20: the canvas is 240 by 190.
        let scene = on_a_background(Bounds::new(100.0, 50.0, 200.0, 150.0), 20.0);
        let (_, image) = inserted(&scene, Zone::Inside, scene.canvas.center(), 100.0, 50.0);
        assert_eq!(image, Bounds::new(150.0, 100.0, 100.0, 50.0));
        // Let go on the margin, it is kept to the crop.
        let (_, image) = inserted(&scene, Zone::Inside, Point::new(5.0, 95.0), 100.0, 50.0);
        assert_eq!(image, Bounds::new(100.0, 100.0, 100.0, 50.0));
    }

    /// D191: a stitch on a background goes beside the picture, and the background is laid
    /// out again around the two, so its frame holds both (D166).
    #[test]
    fn on_a_background_a_stitch_goes_beside_the_picture_and_the_background_holds_both() {
        let scene = on_a_background(Bounds::new(0.0, 0.0, 400.0, 300.0), 100.0);
        let (after, image) = inserted(&scene, Zone::Right, Point::new(590.0, 250.0), 200.0, 100.0);
        // 200 by 100, at the picture's 300 high, is 600 by 300.
        assert_eq!(image, Bounds::new(400.0, 0.0, 600.0, 300.0));
        let (source, _) = after.background_source().expect("a background");
        assert_eq!(source, Bounds::new(0.0, 0.0, 1000.0, 300.0));
        assert_eq!(after.canvas, Bounds::new(0.0, 0.0, 1200.0, 500.0));
        assert_eq!(after.placement().apply_bounds(image), Bounds::new(500.0, 100.0, 600.0, 300.0));
    }

    /// D191: a stitch to the left of a crop on a background moves nothing in the picture: the
    /// background's source grows backwards, and the layout puts it all at the margin.
    #[test]
    fn a_stitch_left_of_a_crop_grows_the_background_s_source_backwards() {
        let scene = on_a_background(Bounds::new(100.0, 50.0, 200.0, 150.0), 20.0);
        let (after, image) = inserted(&scene, Zone::Left, Point::new(5.0, 95.0), 100.0, 50.0);
        // 100 by 50, at the crop's 150 high, is 300 by 150.
        assert_eq!(image, Bounds::new(-200.0, 50.0, 300.0, 150.0));
        let (source, _) = after.background_source().expect("a background");
        assert_eq!(source, Bounds::new(-200.0, 50.0, 500.0, 150.0));
        assert_eq!(after.canvas, Bounds::new(0.0, 0.0, 540.0, 190.0));
        assert_eq!(after.placement().apply_bounds(image), Bounds::new(20.0, 20.0, 300.0, 150.0));
    }

    /// The fill a crop beyond the image leaves lays nothing out, and the picture's coordinates
    /// are the canvas's: a stitch grows the canvas as it does with no background (D191).
    #[test]
    fn with_the_crop_s_own_fill_a_stitch_grows_the_canvas() {
        use crate::object::{Geometry, Object};
        let mut scene = Scene::new(crate::scene::Base::new("base.png", 400.0, 300.0, 1.0));
        scene.canvas = Bounds::new(-50.0, 0.0, 450.0, 300.0);
        let grey = crate::style::Rgba::new(0.5, 0.5, 0.5, 1.0);
        let params = crate::background::BackgroundParams::plain(grey);
        let geometry = Geometry::Background { params, source: None, fill: None };
        scene.add(Object::new(-1, 0, crate::style::Style::default(), geometry));
        let (after, image) = inserted(&scene, Zone::Right, Point::new(395.0, 150.0), 200.0, 100.0);
        assert_eq!(image, Bounds::new(400.0, 0.0, 600.0, 300.0));
        assert_eq!(after.canvas, Bounds::new(-50.0, 0.0, 1050.0, 300.0));
        let fill = after.objects().iter().find(|o| o.z == -1).expect("the fill");
        assert_eq!(fill, scene.objects().iter().find(|o| o.z == -1).expect("the fill"));
    }

    /// D192: a stitch beside a background the panel laid out over the crop's fill keeps the
    /// fill, which paints the grown rect, behind the stitched picture too.
    #[test]
    fn a_stitch_keeps_the_crop_s_fill_the_background_kept() {
        use crate::object::Geometry;
        let grey = crate::style::Rgba::new(0.5, 0.5, 0.5, 1.0);
        // A crop 200 past the capture's right edge, then a margin of 100.
        let mut scene = on_a_background(Bounds::new(0.0, 0.0, 600.0, 300.0), 100.0);
        let mut background = scene.objects()[0].clone();
        if let Geometry::Background { fill, .. } = &mut background.geometry {
            *fill = Some(grey);
        }
        scene.replace(background);
        let (after, image) = inserted(&scene, Zone::Right, Point::new(790.0, 250.0), 200.0, 100.0);
        assert_eq!(image, Bounds::new(600.0, 0.0, 600.0, 300.0));
        assert_eq!(after.frame_fill(), Some((Bounds::new(0.0, 0.0, 1200.0, 300.0), grey)));
    }
}
