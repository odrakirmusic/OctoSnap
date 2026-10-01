// SPDX-License-Identifier: GPL-3.0-or-later

//! Geometry in the one coordinate system the D-Bus boundary is allowed to use.
//!
//! `spec/01` §1 is unambiguous: the extension speaks **stage/logical** coordinates plus a
//! scale, and physical pixels exist only inside image files and the renderer. Every rect
//! here is therefore logical. The type carries no scale of its own precisely so that a
//! physical rect cannot be passed where a logical one is expected without saying so.

use serde::{Deserialize, Serialize};

/// A rectangle in logical stage coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    #[must_use]
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self { x, y, width, height }
    }

    /// A rect with no area. Distinct from "no rect at all", which is `Option::None`.
    #[must_use]
    pub const fn empty() -> Self {
        Self::new(0, 0, 0, 0)
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    /// How much of this rect lies inside `other`, in square logical pixels.
    ///
    /// The one honest way to ask "which monitor is this on" when a rect may straddle two.
    /// Containment is not enough -- a capture that reaches a seam is inside neither -- and
    /// "the biggest monitor" is not an answer to the question at all, which is the mistake
    /// this replaced (`docs/decisions.md` D50).
    ///
    /// `i64` because two 4K monitors' worth of area overflows `i32` at 2 073 600 000 000.
    #[must_use]
    pub fn overlap_area(&self, other: Self) -> i64 {
        let w = (self.x + self.width).min(other.x + other.width) - self.x.max(other.x);
        let h = (self.y + self.height).min(other.y + other.height) - self.y.max(other.y);
        if w <= 0 || h <= 0 { 0 } else { i64::from(w) * i64::from(h) }
    }

    /// Converts to physical pixels at `scale`, **rounding to nearest, as Mutter does**.
    ///
    /// This is the only sanctioned logical → physical conversion, and it lives here so it
    /// can be tested rather than repeated at each call site.
    ///
    /// It rounded *outward* until 2026-09-08, on the reasoning that a selection should
    /// never lose an edge pixel. That reasoning is sound and the rule was still wrong,
    /// because the compositor does not share it: `screenshot_area` rounds to nearest, so
    /// ceiling here made the twin claim a size the PNG did not have. Measured across
    /// every scale the hardware offers -- at 1.6666666269302368 a 200 px logical width
    /// came out 333 px and this function predicted 334.
    ///
    /// Worse, the scales Mutter reports are `float32` approximations of rationals, so
    /// ceiling turns their error into whole pixels: 1440 × 1.3333333730697632 is
    /// 1920.00005, and a 1920 px panel was being described as **1921** px wide.
    /// Rounding absorbs that error instead of amplifying it.
    ///
    /// `docs/decisions.md` D23.
    #[must_use]
    pub fn to_physical(&self, scale: f64) -> (u32, u32) {
        let w = (f64::from(self.width) * scale).round().max(0.0);
        let h = (f64::from(self.height) * scale).round().max(0.0);
        (w as u32, h as u32)
    }
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// The question `overlap_area` exists to answer: a capture on the smaller of two
    /// monitors must resolve to that monitor, not to the bigger one (D50).
    #[test]
    fn overlap_picks_the_monitor_a_rect_is_actually_on() {
        let small = Rect::new(0, 0, 960, 600);
        let large = Rect::new(960, 0, 1920, 1200);
        let capture = Rect::new(240, 174, 240, 142);
        assert_eq!(capture.overlap_area(small), 240 * 142);
        assert_eq!(capture.overlap_area(large), 0);
    }

    #[test]
    fn a_rect_across_a_seam_belongs_to_whichever_side_holds_more_of_it() {
        let left = Rect::new(0, 0, 960, 600);
        let right = Rect::new(960, 0, 1920, 1200);
        // 100 px on the left of the seam, 300 on the right.
        let straddling = Rect::new(860, 100, 400, 200);
        assert_eq!(straddling.overlap_area(left), 100 * 200);
        assert_eq!(straddling.overlap_area(right), 300 * 200);
        assert!(straddling.overlap_area(right) > straddling.overlap_area(left));
    }

    #[test]
    fn no_overlap_is_zero_rather_than_negative() {
        let a = Rect::new(0, 0, 100, 100);
        assert_eq!(a.overlap_area(Rect::new(200, 200, 100, 100)), 0);
        assert_eq!(a.overlap_area(Rect::new(100, 0, 100, 100)), 0, "touching is not overlapping");
        assert_eq!(a.overlap_area(Rect::empty()), 0);
    }

    #[test]
    fn empty_is_empty_and_zero_area_rects_are_too() {
        assert!(Rect::empty().is_empty());
        assert!(Rect::new(10, 10, 0, 50).is_empty());
        assert!(Rect::new(10, 10, 50, 0).is_empty());
        assert!(!Rect::new(0, 0, 1, 1).is_empty());
    }

    #[test]
    fn physical_conversion_at_integer_scale() {
        assert_eq!(Rect::new(0, 0, 512, 384).to_physical(2.0), (1024, 768));
        assert_eq!(Rect::new(0, 0, 512, 384).to_physical(1.0), (512, 384));
    }

    /// No scale is the common case and none is the edge case, so the whole ladder the
    /// hardware offers is here. 1536x960 logical is exactly 1920x1200 physical at 1.25.
    #[test]
    fn physical_conversion_at_every_fractional_scale() {
        assert_eq!(Rect::new(0, 0, 1536, 960).to_physical(1.25), (1920, 1200));
        assert_eq!(Rect::new(0, 0, 1280, 800).to_physical(1.5), (1920, 1200));
        assert_eq!(Rect::new(0, 0, 960, 600).to_physical(2.0), (1920, 1200));
    }

    /// The reported scales are `float32` approximations of rationals, and the exact
    /// values Mutter hands out are used here deliberately. 1440 × 4/3 is exactly 1920,
    /// but 1440 × 1.3333333730697632 is 1920.00005 -- which a ceiling turns into a
    /// 1921 px description of a 1920 px panel. D23.
    #[test]
    fn physical_conversion_absorbs_the_float32_error_in_a_reported_scale() {
        assert_eq!(Rect::new(0, 0, 1440, 900).to_physical(1.333_333_373_069_763_2), (1920, 1200));
        assert_eq!(Rect::new(0, 0, 1152, 720).to_physical(1.666_666_626_930_236_8), (1920, 1200));
    }

    /// Rounding to nearest, matching the compositor. Measured, not chosen: at these
    /// scales `screenshot_area` produced exactly these sizes, and a ceiling produced one
    /// pixel more. D23.
    #[test]
    fn physical_conversion_matches_what_the_compositor_produces() {
        assert_eq!(Rect::new(0, 0, 101, 101).to_physical(1.25), (126, 126));
        assert_eq!(Rect::new(0, 0, 201, 149).to_physical(1.25), (251, 186));
        assert_eq!(Rect::new(0, 0, 200, 150).to_physical(1.333_333_373_069_763_2), (267, 200));
        assert_eq!(Rect::new(0, 0, 200, 150).to_physical(1.666_666_626_930_236_8), (333, 250));
        assert_eq!(Rect::new(0, 0, 201, 149).to_physical(1.666_666_626_930_236_8), (335, 248));
    }
}
