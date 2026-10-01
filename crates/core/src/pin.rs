// SPDX-License-Identifier: GPL-3.0-or-later

//! Pinned screenshots: the numbers and the rules (`spec/07` §3).
//!
//! A pin is "a capture that appears **exactly where it was captured** at 1:1, so it looks
//! like the screen froze there". Everything in this file is what that costs in arithmetic:
//! where the window goes, how far an arrow key moves it, what a scroll does to its
//! opacity, and which part of it stays clickable once it is locked.
//!
//! Pure, like `qao`, and for the same reason: a pin is a window nobody can photograph
//! while it is being tested, so the parts that can be checked without a display should be.

use crate::geometry::Rect;
use crate::qao::Size;

/// `spec/07` §3.1: "opacity with two-finger scroll / Ctrl+scroll (10-100 %)".
pub const OPACITY_MIN: f64 = 0.10;
pub const OPACITY_MAX: f64 = 1.00;

/// One notch of scroll. Ten steps across the range, which is coarse enough that a single
/// flick of a wheel makes a visible difference and fine enough to stop anywhere useful.
pub const OPACITY_STEP: f64 = 0.10;

/// `spec/07` §3.1: "arrow keys move 1 px, Shift 10 px".
pub const NUDGE: i32 = 1;
pub const NUDGE_FAST: i32 = 10;

/// The side of the square that stays reactive in lock mode.
///
/// `spec/07` §3.2: "lock mode via empty input region (keep a 24x24 unlock handle
/// region)". Without it a locked pin is unclosable by pointer -- the whole point of lock
/// mode is that clicks pass through, and that includes the click that would unlock it.
pub const UNLOCK_HANDLE: i32 = 24;

/// `spec/07` §3.1's rounded corners, which are the pin's own 8 px rather than a card's 10.
pub const RADIUS: i32 = 8;

/// The hover controls (`spec/07` §3.1's "small drag grip" and "lock icon on hover"): the
/// side of one button, the space between two, and how far the row sits in from the corner.
///
/// Sized off `UNLOCK_HANDLE` rather than picked separately, because a locked pin's only
/// button has to *be* the handle and a button that changes size when you lock it is a
/// button that moves under the pointer that just pressed it.
pub const CHROME_BUTTON: i32 = UNLOCK_HANDLE - 4;
pub const CHROME_GAP: i32 = 4;
pub const CHROME_MARGIN: i32 = 4;

/// `spec/07` §3.1's "resize from edges/corners keeping aspect", as a Wayland client can
/// have it: a **zoom** on the pin's own size, in steps, with the aspect kept by
/// construction. The compositor owns interactive resizes and offers no aspect
/// constraint, so a resize that kept the aspect could only ever be the client asking for
/// a new size -- which is this. One notch is 10 %, a quarter to four times the capture.
pub const ZOOM_STEP: f64 = 0.10;
pub const ZOOM_MIN: f64 = 0.25;
pub const ZOOM_MAX: f64 = 4.0;

/// Zoom after `notches` of Shift+scroll or +/- presses, clamped. Positive is larger.
#[must_use]
pub fn adjust_zoom(current: f64, notches: f64) -> f64 {
    // Snapped to the step, so ten notches out and ten back land on exactly 1.0 rather
    // than on 0.999 and a pin one pixel short of its capture.
    let steps = ((current + notches * ZOOM_STEP) / ZOOM_STEP).round();
    (steps * ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX)
}

/// The pin's size at a zoom, never smaller than a pixel a side.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn zoomed_size(base: Size, zoom: f64) -> Size {
    Size::new(
        ((f64::from(base.width) * zoom).round() as i32).max(1),
        ((f64::from(base.height) * zoom).round() as i32).max(1),
    )
}

/// The zoom that fits a pin inside the work area, and never enlarges it: `spec/07`
/// §3.1's "double-click toggles 1:1 / fit" [P]. A capture that already fits answers 1.0,
/// so the toggle has nothing to do on a pin the screen can hold.
#[must_use]
pub fn fit_zoom(base: Size, work_area: Rect) -> f64 {
    if base.width <= 0 || base.height <= 0 {
        return 1.0;
    }
    let by_width = f64::from(work_area.width) / f64::from(base.width);
    let by_height = f64::from(work_area.height) / f64::from(base.height);
    // Floored to the step, so a fitted pin is inside the work area rather than a pixel
    // over it, and lands on a zoom the +/- keys can reach.
    let fit = by_width.min(by_height).min(1.0);
    ((fit / ZOOM_STEP).floor() * ZOOM_STEP).clamp(ZOOM_MIN, 1.0)
}

/// How far an arrow key moves a pin.
#[must_use]
pub const fn nudge(fast: bool) -> i32 {
    if fast { NUDGE_FAST } else { NUDGE }
}

/// Opacity after `notches` of scroll, clamped to the documented range.
///
/// Positive is more opaque. Clamped rather than wrapped: a pin that jumps from 10 % back
/// to full because one notch went too far is a pin that vanished and then shouted.
#[must_use]
pub fn adjust_opacity(current: f64, notches: f64) -> f64 {
    (current + notches * OPACITY_STEP).clamp(OPACITY_MIN, OPACITY_MAX)
}

/// The region of a locked pin that still takes clicks, in window coordinates.
///
/// Top-right, per `spec/07` §3.1 ("only a small unlock handle at the top-right remains
/// reactive"), and clamped so a pin smaller than the handle is entirely reactive rather
/// than entirely unclickable -- a 20x20 pin with a 24x24 handle placed at `width - 24`
/// would otherwise sit at `x = -4` and take no clicks at all.
#[must_use]
pub fn unlock_handle(size: Size) -> Rect {
    let side = UNLOCK_HANDLE.min(size.width).min(size.height).max(1);
    Rect::new((size.width - side).max(0), 0, side, side)
}

/// Whether a control drawn at `rect` can still be pressed once the pin is locked.
///
/// Lock mode's one invariant, and the one it broke. A locked pin's input region is
/// `unlock_handle` and nothing else, so a control outside that square is not a control at
/// all: the click travels through to whatever is underneath while the button sits there
/// looking pressable. The first version put Lock and Close side by side in the corner, and
/// only the outer of the two -- Close -- fell inside the region. Locking a pin therefore
/// left a close button standing exactly where the unlock button appeared to be, and no way
/// out but the keyboard.
#[must_use]
pub fn reachable_when_locked(size: Size, rect: Rect) -> bool {
    let handle = unlock_handle(size);
    rect.x >= handle.x
        && rect.y >= handle.y
        && rect.x + rect.width <= handle.x + handle.width
        && rect.y + rect.height <= handle.y + handle.height
}

/// Where a pin opens: exactly the rect its capture came from.
///
/// `spec/07` §3.1's "exactly where it was captured", with one adjustment the spec does not
/// spell out but its own words require. A capture taken at the very edge of a display puts
/// the pin's frame partly outside the work area, where Mutter's silent clamp moves it --
/// and "it looks like the screen froze there" is exactly the illusion a few pixels of
/// drift breaks. So the origin is clamped here, where the caller can see it happen,
/// rather than by the compositor where nobody can.
#[must_use]
pub fn opening_rect(capture: Rect, work_area: Rect) -> Rect {
    let width = capture.width.min(work_area.width).max(1);
    let height = capture.height.min(work_area.height).max(1);
    Rect::new(
        capture.x.clamp(work_area.x, work_area.x + work_area.width - width),
        capture.y.clamp(work_area.y, work_area.y + work_area.height - height),
        width,
        height,
    )
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn opacity_moves_by_a_notch_and_stops_at_the_ends() {
        assert!((adjust_opacity(1.0, -1.0) - 0.9).abs() < 1e-9);
        assert!((adjust_opacity(0.5, 1.0) - 0.6).abs() < 1e-9);
        // `spec/07` §3.1's 10-100 % range, at both ends.
        assert!((adjust_opacity(0.15, -5.0) - OPACITY_MIN).abs() < 1e-9);
        assert!((adjust_opacity(0.95, 5.0) - OPACITY_MAX).abs() < 1e-9);
    }

    /// A pin that scrolls to invisible cannot be scrolled back, because there is nothing
    /// left to put the pointer on.
    #[test]
    fn opacity_never_reaches_zero() {
        let mut opacity = 1.0;
        for _ in 0..100 {
            opacity = adjust_opacity(opacity, -1.0);
        }
        assert!(opacity >= OPACITY_MIN);
    }

    #[test]
    fn shift_makes_an_arrow_key_ten_times_bigger() {
        assert_eq!(nudge(false), 1);
        assert_eq!(nudge(true), 10);
    }

    #[test]
    fn the_unlock_handle_sits_in_the_top_right() {
        let handle = unlock_handle(Size::new(400, 300));
        assert_eq!(handle, Rect::new(400 - UNLOCK_HANDLE, 0, UNLOCK_HANDLE, UNLOCK_HANDLE));
    }

    /// A pin smaller than its own unlock handle must stay clickable, or locking it is a
    /// one-way door.
    #[test]
    fn a_tiny_pin_is_entirely_its_own_unlock_handle() {
        let handle = unlock_handle(Size::new(20, 12));
        assert_eq!(handle, Rect::new(8, 0, 12, 12));
        assert!(handle.x >= 0 && handle.width > 0 && handle.height > 0);
    }

    /// The regression that named this function. A 20 px button 4 px in from the corner is
    /// reachable; the one beside it, 4 px further left, is not -- and the second was Lock.
    #[test]
    fn only_the_corner_control_survives_lock_mode() {
        let size = Size::new(400, 300);
        let outer = Rect::new(
            size.width - CHROME_MARGIN - CHROME_BUTTON,
            CHROME_MARGIN,
            CHROME_BUTTON,
            CHROME_BUTTON,
        );
        let inner = Rect::new(outer.x - CHROME_GAP - CHROME_BUTTON, outer.y, outer.width, outer.height);
        assert!(reachable_when_locked(size, outer));
        assert!(!reachable_when_locked(size, inner));
    }

    /// So the control lock mode leaves behind is the handle itself, at every pin size --
    /// including one smaller than the handle, where the square shrinks to fit.
    #[test]
    fn the_unlock_control_is_the_reactive_square() {
        for size in [Size::new(400, 300), Size::new(48, 40), Size::new(20, 12), Size::new(1, 1)] {
            assert!(
                reachable_when_locked(size, unlock_handle(size)),
                "the handle must be reachable at {size:?}"
            );
        }
    }

    #[test]
    fn a_pin_opens_where_its_capture_was() {
        let work = Rect::new(0, 32, 1920, 1168);
        assert_eq!(opening_rect(Rect::new(300, 200, 400, 300), work), Rect::new(300, 200, 400, 300));
    }

    /// `spec/07` §3.1's illusion is that the screen froze. A pin Mutter had to shove back
    /// inside would be a few pixels off from the thing it is pretending to be.
    #[test]
    fn a_pin_at_the_edge_is_pulled_inside_before_the_compositor_does_it() {
        let work = Rect::new(0, 32, 1920, 1168);
        // A capture flush with the bottom-right corner of a 1920x1200 panel.
        let at_edge = opening_rect(Rect::new(1620, 1000, 400, 300), work);
        assert_eq!(at_edge.x + at_edge.width, 1920);
        assert_eq!(at_edge.y + at_edge.height, 1200);

        // And one that starts above the panel.
        let under_panel = opening_rect(Rect::new(10, 0, 400, 300), work);
        assert_eq!(under_panel.y, 32);
    }

    #[test]
    fn a_capture_larger_than_the_screen_is_shrunk_rather_than_placed_off_it() {
        let work = Rect::new(0, 32, 1920, 1168);
        let huge = opening_rect(Rect::new(0, 0, 4000, 3000), work);
        assert_eq!(huge, Rect::new(0, 32, 1920, 1168));
    }

    #[test]
    fn zoom_steps_are_snapped_and_clamped() {
        assert!((adjust_zoom(1.0, -1.0) - 0.9).abs() < 1e-9);
        assert!((adjust_zoom(0.9, 1.0) - 1.0).abs() < 1e-9);
        let mut zoom = 1.0;
        for _ in 0..5 {
            zoom = adjust_zoom(zoom, -1.0);
        }
        assert!((zoom - 0.5).abs() < 1e-9, "five out is exactly 0.5, got {zoom}");
        for _ in 0..5 {
            zoom = adjust_zoom(zoom, 1.0);
        }
        assert!((zoom - 1.0).abs() < 1e-9, "and five back is exactly 1.0, got {zoom}");
        assert!((adjust_zoom(0.3, -1.0) - ZOOM_MIN).abs() < 1e-9);
        assert!((adjust_zoom(3.95, 1.0) - ZOOM_MAX).abs() < 1e-9);
    }

    #[test]
    fn a_zoomed_size_keeps_the_aspect_and_never_vanishes() {
        assert_eq!(zoomed_size(Size::new(400, 300), 0.5), Size::new(200, 150));
        assert_eq!(zoomed_size(Size::new(400, 300), 1.0), Size::new(400, 300));
        assert_eq!(zoomed_size(Size::new(3, 2), 0.25), Size::new(1, 1));
    }

    #[test]
    fn fit_zoom_shrinks_only_what_does_not_fit() {
        let work = Rect::new(0, 0, 1536, 928);
        assert!((fit_zoom(Size::new(700, 450), work) - 1.0).abs() < 1e-9, "already fits");
        // 3000 wide in a 1536 work area: 0.512, floored to the step.
        assert!((fit_zoom(Size::new(3000, 1000), work) - 0.5).abs() < 1e-9);
        // Taller than wide: the height is the constraint. 928 / 2000 = 0.464 -> 0.4.
        assert!((fit_zoom(Size::new(1000, 2000), work) - 0.4).abs() < 1e-9);
        assert!((fit_zoom(Size::new(0, 10), work) - 1.0).abs() < 1e-9, "no area, no fit");
    }
}
