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

use crate::geometry::{Bounds, Point};

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
            let out = place(CANVAS, zone, Point::new(0.0, 0.0), 1600.0, 900.0)
                .expect("no placement");
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
}
