// SPDX-License-Identifier: GPL-3.0-or-later

//! Selection geometry for the capture overlay: `CAP-02`, specified in `spec/03` §5.1.
//!
//! All of it is pure and works in **logical** stage coordinates (`spec/01` §1). It lives
//! here rather than in the extension for two reasons: `spec/10` §7 caps any extension
//! callback at 2 ms and this is the arithmetic that runs on every pointer motion, and
//! `spec/11` asks for the geometry to be unit-tested before any UI exists. The extension
//! will call an equivalent of this in TypeScript; these tests are the reference for what
//! it must agree with.

use crate::geometry::Rect;

/// `spec/03` §5.1: "Click without drag (movement < 4 px)".
pub const CLICK_THRESHOLD: i32 = 4;

/// `spec/03` §4: handles are 8x8 px with a 20 px hit area, so ±10 around the centre.
pub const HANDLE_HIT_RADIUS: i32 = 10;

/// `spec/03` §5.1: "arrow keys nudge 1 px, Shift+arrows 10 px".
pub const STEP: i32 = 1;
pub const STEP_FAST: i32 = 10;

/// The eight resize handles, in the order `spec/03` §4 lists them for a released
/// selection. Corner handles keep the opposite corner fixed; edge handles keep the
/// opposite edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl Handle {
    pub const ALL: [Self; 8] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Right,
        Self::BottomRight,
        Self::Bottom,
        Self::BottomLeft,
        Self::Left,
    ];

    const fn moves_left(self) -> bool {
        matches!(self, Self::TopLeft | Self::Left | Self::BottomLeft)
    }
    const fn moves_right(self) -> bool {
        matches!(self, Self::TopRight | Self::Right | Self::BottomRight)
    }
    const fn moves_top(self) -> bool {
        matches!(self, Self::TopLeft | Self::Top | Self::TopRight)
    }
    const fn moves_bottom(self) -> bool {
        matches!(self, Self::BottomLeft | Self::Bottom | Self::BottomRight)
    }
}

/// What a press at a given point should start doing.
///
/// `spec/03` §5.1 fixes the precedence: "inside selection → move, on a handle → resize,
/// anywhere else → begin a new selection". Handles are tested first because their hit
/// areas overhang the selection edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Handle(Handle),
    Inside,
    Outside,
}

/// Modifier state during a drag (`spec/03` §5.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// Shift: lock the aspect ratio.
    pub aspect_lock: bool,
    /// Alt: treat the anchor as the centre rather than a corner.
    pub from_centre: bool,
}

/// Corner positions of each handle, for drawing and hit-testing.
#[must_use]
pub fn handle_centre(rect: Rect, handle: Handle) -> (i32, i32) {
    let (left, top) = (rect.x, rect.y);
    let (right, bottom) = (rect.x + rect.width, rect.y + rect.height);
    let (mid_x, mid_y) = (rect.x + rect.width / 2, rect.y + rect.height / 2);

    match handle {
        Handle::TopLeft => (left, top),
        Handle::Top => (mid_x, top),
        Handle::TopRight => (right, top),
        Handle::Right => (right, mid_y),
        Handle::BottomRight => (right, bottom),
        Handle::Bottom => (mid_x, bottom),
        Handle::BottomLeft => (left, bottom),
        Handle::Left => (left, mid_y),
    }
}

/// Classifies a press. `rect` is the current selection, or `None` when there is none.
#[must_use]
pub fn hit_test(rect: Option<Rect>, point: (i32, i32)) -> Hit {
    let Some(rect) = rect else {
        return Hit::Outside;
    };

    for handle in Handle::ALL {
        let (hx, hy) = handle_centre(rect, handle);
        if (point.0 - hx).abs() <= HANDLE_HIT_RADIUS && (point.1 - hy).abs() <= HANDLE_HIT_RADIUS {
            return Hit::Handle(handle);
        }
    }

    if point.0 >= rect.x
        && point.0 <= rect.x + rect.width
        && point.1 >= rect.y
        && point.1 <= rect.y + rect.height
    {
        return Hit::Inside;
    }

    Hit::Outside
}

/// Applies an aspect ratio by **growing** the smaller axis.
///
/// Growing rather than shrinking means the selection always contains the pointer's
/// dominant extent, which is what makes Shift feel like it is following the drag instead
/// of fighting it.
fn apply_ratio(width: i32, height: i32, ratio: f64) -> (i32, i32) {
    if ratio <= 0.0 {
        return (width, height);
    }
    let (w, h) = (f64::from(width), f64::from(height));
    if h == 0.0 || w / h >= ratio {
        // Width dominates: derive the height from it.
        (width, (w / ratio).round() as i32)
    } else {
        (((h * ratio).round()) as i32, height)
    }
}

/// Builds a selection from a drag (`spec/03` §5.1).
///
/// `anchor` is where the press happened, `pointer` where it is now. `locked_ratio` is
/// width/height, supplied by the caller because `spec/03` §5.1 says the ratio is either
/// the one at the moment Shift was pressed or the one locked in the toolbar -- the
/// geometry does not get to choose.
#[must_use]
pub fn rect_from_drag(
    anchor: (i32, i32),
    pointer: (i32, i32),
    modifiers: Modifiers,
    locked_ratio: Option<f64>,
) -> Rect {
    let mut dx = (pointer.0 - anchor.0).abs();
    let mut dy = (pointer.1 - anchor.1).abs();

    if modifiers.aspect_lock
        && let Some(ratio) = locked_ratio
    {
        (dx, dy) = apply_ratio(dx, dy, ratio);
    }

    let sign_x = if pointer.0 < anchor.0 { -1 } else { 1 };
    let sign_y = if pointer.1 < anchor.1 { -1 } else { 1 };

    if modifiers.from_centre {
        // Alt: the anchor is the centre, so the drag distance is a half-extent and the
        // rect grows in both directions at once.
        Rect::new(anchor.0 - dx, anchor.1 - dy, dx * 2, dy * 2)
    } else {
        let x = if sign_x < 0 { anchor.0 - dx } else { anchor.0 };
        let y = if sign_y < 0 { anchor.1 - dy } else { anchor.1 };
        Rect::new(x, y, dx, dy)
    }
}

/// Resizes by dragging a handle (`spec/03` §5.1).
///
/// Corner handles keep the opposite corner fixed, edge handles the opposite edge. With
/// `from_centre` the rect grows symmetrically about its own centre instead.
#[must_use]
pub fn resize_by_handle(
    rect: Rect,
    handle: Handle,
    pointer: (i32, i32),
    modifiers: Modifiers,
    locked_ratio: Option<f64>,
) -> Rect {
    let (mut left, mut top) = (rect.x, rect.y);
    let (mut right, mut bottom) = (rect.x + rect.width, rect.y + rect.height);

    if handle.moves_left() {
        left = pointer.0;
    }
    if handle.moves_right() {
        right = pointer.0;
    }
    if handle.moves_top() {
        top = pointer.1;
    }
    if handle.moves_bottom() {
        bottom = pointer.1;
    }

    // Dragging a handle past the opposite edge flips the rect rather than producing a
    // negative size.
    let mut out = Rect::new(
        left.min(right),
        top.min(bottom),
        (right - left).abs(),
        (bottom - top).abs(),
    );

    if modifiers.aspect_lock
        && let Some(ratio) = locked_ratio
    {
        let (w, h) = apply_ratio(out.width, out.height, ratio);
        // Grow away from the fixed edge so the anchor stays put.
        if handle.moves_left() {
            out.x -= w - out.width;
        }
        if handle.moves_top() {
            out.y -= h - out.height;
        }
        out.width = w;
        out.height = h;
    }

    if modifiers.from_centre {
        let (cx, cy) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
        let half_w = (out.width.max(1)) / 2;
        let half_h = (out.height.max(1)) / 2;
        out = Rect::new(cx - half_w, cy - half_h, half_w * 2, half_h * 2);
    }

    out
}

/// Clamps a selection inside one monitor.
///
/// `spec/01` §1: a selection spanning two differently-scaled monitors is refused, so
/// clamping is always to a single monitor's rect. Position is moved before size is
/// reduced, so dragging off the edge slides the selection rather than shrinking it --
/// except when it genuinely does not fit.
#[must_use]
pub fn clamp_to_monitor(rect: Rect, monitor: Rect) -> Rect {
    let width = rect.width.min(monitor.width);
    let height = rect.height.min(monitor.height);

    let x = rect.x.clamp(monitor.x, monitor.x + monitor.width - width);
    let y = rect.y.clamp(monitor.y, monitor.y + monitor.height - height);

    Rect::new(x, y, width, height)
}

/// Which monitor a selection belongs to: the one containing the largest share of it.
///
/// Used to decide the single monitor a selection is clamped to, and which monitor's
/// toolbar and scale apply.
#[must_use]
pub fn dominant_monitor(rect: Rect, monitors: &[Rect]) -> Option<usize> {
    monitors
        .iter()
        .enumerate()
        .map(|(i, m)| (i, overlap_area(rect, *m)))
        .filter(|(_, area)| *area > 0)
        .max_by_key(|(_, area)| *area)
        .map(|(i, _)| i)
}

fn overlap_area(a: Rect, b: Rect) -> i64 {
    let x = (a.x + a.width).min(b.x + b.width) - a.x.max(b.x);
    let y = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y);
    if x <= 0 || y <= 0 {
        0
    } else {
        i64::from(x) * i64::from(y)
    }
}

/// Arrow-key direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Moves the selection (`spec/03` §5.1: arrows nudge 1 px, Shift+arrows 10 px).
#[must_use]
pub fn nudge(rect: Rect, direction: Direction, step: i32) -> Rect {
    let (dx, dy) = match direction {
        Direction::Left => (-step, 0),
        Direction::Right => (step, 0),
        Direction::Up => (0, -step),
        Direction::Down => (0, step),
    };
    Rect::new(rect.x + dx, rect.y + dy, rect.width, rect.height)
}

/// Resizes the selection from its bottom-right (`spec/03` §5.1: Ctrl+arrows resize by
/// 1 px, Ctrl+Shift by 10).
///
/// A selection never shrinks below 1x1: `spec/03` §5.1 says 1x1 is legal, and zero is not
/// a selection at all.
#[must_use]
pub fn resize_by_key(rect: Rect, direction: Direction, step: i32) -> Rect {
    let (dw, dh) = match direction {
        Direction::Left => (-step, 0),
        Direction::Right => (step, 0),
        Direction::Up => (0, -step),
        Direction::Down => (0, step),
    };
    Rect::new(
        rect.x,
        rect.y,
        (rect.width + dw).max(1),
        (rect.height + dh).max(1),
    )
}

/// Sets an exact width, anchored at the top-left (`spec/03` §5.1: "typing into the W/H
/// fields sets exact sizes anchored at the top-left; the lock keeps the ratio").
#[must_use]
pub fn set_width(rect: Rect, width: i32, locked_ratio: Option<f64>) -> Rect {
    let width = width.max(1);
    let height = match locked_ratio {
        Some(ratio) if ratio > 0.0 => ((f64::from(width) / ratio).round() as i32).max(1),
        _ => rect.height,
    };
    Rect::new(rect.x, rect.y, width, height)
}

/// Sets an exact height, anchored at the top-left.
#[must_use]
pub fn set_height(rect: Rect, height: i32, locked_ratio: Option<f64>) -> Rect {
    let height = height.max(1);
    let width = match locked_ratio {
        Some(ratio) if ratio > 0.0 => ((f64::from(height) * ratio).round() as i32).max(1),
        _ => rect.width,
    };
    Rect::new(rect.x, rect.y, width, height)
}

/// True when a press-release pair was a click rather than a drag (`spec/03` §5.1).
#[must_use]
pub fn is_click(anchor: (i32, i32), release: (i32, i32)) -> bool {
    (release.0 - anchor.0).abs() < CLICK_THRESHOLD
        && (release.1 - anchor.1).abs() < CLICK_THRESHOLD
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers { aspect_lock: false, from_centre: false };
    const ALT: Modifiers = Modifiers { aspect_lock: false, from_centre: true };
    const SHIFT: Modifiers = Modifiers { aspect_lock: true, from_centre: false };

    // --- drag normalisation --------------------------------------------------

    #[test]
    fn a_drag_down_right_spans_anchor_to_pointer() {
        assert_eq!(
            rect_from_drag((100, 100), (300, 250), NONE, None),
            Rect::new(100, 100, 200, 150)
        );
    }

    /// Dragging up and left must produce the same rect as dragging down and right
    /// between the same two points. Getting this wrong gives negative sizes, which is
    /// the most common selection bug there is.
    #[test]
    fn dragging_in_any_direction_gives_the_same_rect() {
        let expected = Rect::new(100, 100, 200, 150);
        assert_eq!(rect_from_drag((300, 250), (100, 100), NONE, None), expected);
        assert_eq!(rect_from_drag((100, 250), (300, 100), NONE, None), expected);
        assert_eq!(rect_from_drag((300, 100), (100, 250), NONE, None), expected);
    }

    #[test]
    fn a_zero_length_drag_is_an_empty_rect_not_a_negative_one() {
        let r = rect_from_drag((50, 50), (50, 50), NONE, None);
        assert_eq!(r, Rect::new(50, 50, 0, 0));
        assert!(r.is_empty());
    }

    #[test]
    fn alt_draws_from_the_centre() {
        // Anchor is the centre, so a 100x50 pull becomes a 200x100 rect around it.
        assert_eq!(
            rect_from_drag((200, 200), (300, 250), ALT, None),
            Rect::new(100, 150, 200, 100)
        );
    }

    #[test]
    fn alt_is_symmetric_regardless_of_drag_direction() {
        let a = rect_from_drag((200, 200), (300, 250), ALT, None);
        let b = rect_from_drag((200, 200), (100, 150), ALT, None);
        assert_eq!(a, b);
    }

    #[test]
    fn shift_locks_the_ratio_by_growing_the_smaller_axis() {
        // 16:9 with a wide, shallow drag: the height grows to match the width.
        let r = rect_from_drag((0, 0), (320, 10), SHIFT, Some(16.0 / 9.0));
        assert_eq!(r.width, 320);
        assert_eq!(r.height, 180);

        // A tall, narrow drag: the width grows instead.
        let r = rect_from_drag((0, 0), (10, 180), SHIFT, Some(16.0 / 9.0));
        assert_eq!(r.width, 320);
        assert_eq!(r.height, 180);
    }

    #[test]
    fn shift_without_a_ratio_does_nothing() {
        assert_eq!(
            rect_from_drag((0, 0), (200, 30), SHIFT, None),
            Rect::new(0, 0, 200, 30)
        );
    }

    #[test]
    fn a_square_lock_keeps_the_dominant_axis() {
        let r = rect_from_drag((0, 0), (200, 40), SHIFT, Some(1.0));
        assert_eq!((r.width, r.height), (200, 200));
    }

    // --- hit testing --------------------------------------------------------

    #[test]
    fn handles_win_over_the_interior() {
        let rect = Rect::new(100, 100, 200, 200);
        // Exactly on the top-left corner.
        assert_eq!(hit_test(Some(rect), (100, 100)), Hit::Handle(Handle::TopLeft));
        // Just inside it, still within the 20 px hit area.
        assert_eq!(hit_test(Some(rect), (108, 108)), Hit::Handle(Handle::TopLeft));
        // Well inside: a move.
        assert_eq!(hit_test(Some(rect), (200, 200)), Hit::Inside);
    }

    /// The hit area overhangs the edge, so a press just *outside* a corner still grabs
    /// the handle. Without this, resizing a selection whose corner is at the screen edge
    /// is impossible.
    #[test]
    fn the_handle_hit_area_overhangs_the_selection() {
        let rect = Rect::new(100, 100, 200, 200);
        assert_eq!(hit_test(Some(rect), (92, 92)), Hit::Handle(Handle::TopLeft));
        assert_eq!(hit_test(Some(rect), (89, 89)), Hit::Outside);
    }

    #[test]
    fn every_handle_is_reachable_at_its_own_centre() {
        let rect = Rect::new(100, 100, 200, 200);
        for handle in Handle::ALL {
            let point = handle_centre(rect, handle);
            assert_eq!(
                hit_test(Some(rect), point),
                Hit::Handle(handle),
                "{handle:?} not reachable at {point:?}"
            );
        }
    }

    #[test]
    fn a_press_with_no_selection_starts_a_new_one() {
        assert_eq!(hit_test(None, (10, 10)), Hit::Outside);
    }

    #[test]
    fn a_press_outside_starts_a_new_selection() {
        let rect = Rect::new(100, 100, 200, 200);
        assert_eq!(hit_test(Some(rect), (500, 500)), Hit::Outside);
    }

    // --- handle resize ------------------------------------------------------

    #[test]
    fn a_corner_handle_keeps_the_opposite_corner_fixed() {
        let rect = Rect::new(100, 100, 200, 200);
        let out = resize_by_handle(rect, Handle::TopLeft, (150, 120), NONE, None);
        // Bottom-right stays at (300, 300).
        assert_eq!(out, Rect::new(150, 120, 150, 180));
        assert_eq!((out.x + out.width, out.y + out.height), (300, 300));
    }

    #[test]
    fn an_edge_handle_moves_only_its_own_edge() {
        let rect = Rect::new(100, 100, 200, 200);
        let out = resize_by_handle(rect, Handle::Right, (400, 999), NONE, None);
        assert_eq!(out, Rect::new(100, 100, 300, 200));
    }

    /// Dragging a handle past the opposite edge must flip the rect, not produce a
    /// negative width that every downstream calculation then mishandles.
    #[test]
    fn dragging_a_handle_past_the_far_edge_flips_the_rect() {
        let rect = Rect::new(100, 100, 200, 200);
        let out = resize_by_handle(rect, Handle::Left, (400, 100), NONE, None);
        assert_eq!(out.x, 300);
        assert_eq!(out.width, 100);
        assert!(!out.is_empty());
    }

    #[test]
    fn a_ratio_locked_corner_resize_keeps_the_anchor_corner() {
        let rect = Rect::new(100, 100, 200, 200);
        let out = resize_by_handle(rect, Handle::BottomRight, (400, 210), SHIFT, Some(1.0));
        // Top-left is the anchor and must not move.
        assert_eq!((out.x, out.y), (100, 100));
        assert_eq!(out.width, out.height);
    }

    // --- clamping and monitor choice ----------------------------------------

    #[test]
    fn clamping_slides_a_selection_back_inside() {
        let monitor = Rect::new(0, 0, 1536, 960);
        let out = clamp_to_monitor(Rect::new(1500, 900, 200, 200), monitor);
        assert_eq!(out, Rect::new(1336, 760, 200, 200));
    }

    #[test]
    fn clamping_shrinks_only_what_cannot_fit() {
        let monitor = Rect::new(0, 0, 1536, 960);
        let out = clamp_to_monitor(Rect::new(-50, -50, 5000, 5000), monitor);
        assert_eq!(out, monitor);
    }

    /// Monitors do not start at the origin once there is more than one, and the second
    /// one's offset is in logical units (M0 spike 8).
    #[test]
    fn clamping_respects_a_monitor_offset() {
        let second = Rect::new(1536, 0, 1280, 720);
        let out = clamp_to_monitor(Rect::new(1500, 100, 200, 200), second);
        assert_eq!(out, Rect::new(1536, 100, 200, 200));
    }

    #[test]
    fn the_dominant_monitor_is_the_one_holding_most_of_the_selection() {
        let monitors = [Rect::new(0, 0, 1536, 960), Rect::new(1536, 0, 1280, 720)];
        // Straddling, but mostly on the right-hand monitor.
        assert_eq!(dominant_monitor(Rect::new(1436, 100, 400, 200), &monitors), Some(1));
        // Straddling, but mostly on the left.
        assert_eq!(dominant_monitor(Rect::new(1200, 100, 400, 200), &monitors), Some(0));
        assert_eq!(dominant_monitor(Rect::new(10, 10, 100, 100), &monitors), Some(0));
    }

    #[test]
    fn a_selection_on_no_monitor_has_no_dominant_one() {
        let monitors = [Rect::new(0, 0, 1536, 960)];
        assert_eq!(dominant_monitor(Rect::new(9000, 9000, 10, 10), &monitors), None);
    }

    // --- keyboard -----------------------------------------------------------

    #[test]
    fn arrows_move_and_ctrl_arrows_resize() {
        let rect = Rect::new(100, 100, 200, 200);
        assert_eq!(nudge(rect, Direction::Right, STEP), Rect::new(101, 100, 200, 200));
        assert_eq!(nudge(rect, Direction::Up, STEP_FAST), Rect::new(100, 90, 200, 200));
        assert_eq!(resize_by_key(rect, Direction::Right, STEP), Rect::new(100, 100, 201, 200));
        assert_eq!(resize_by_key(rect, Direction::Down, STEP_FAST), Rect::new(100, 100, 200, 210));
    }

    /// 1x1 is a legal selection per spec/03 §5.1; 0x0 is not, and shrinking must stop.
    #[test]
    fn keyboard_resize_stops_at_one_pixel() {
        let rect = Rect::new(100, 100, 1, 1);
        let out = resize_by_key(rect, Direction::Left, STEP_FAST);
        assert_eq!((out.width, out.height), (1, 1));
    }

    // --- exact sizes --------------------------------------------------------

    #[test]
    fn typing_a_size_anchors_at_the_top_left() {
        let rect = Rect::new(100, 100, 200, 200);
        assert_eq!(set_width(rect, 640, None), Rect::new(100, 100, 640, 200));
        assert_eq!(set_height(rect, 480, None), Rect::new(100, 100, 200, 480));
    }

    #[test]
    fn a_locked_ratio_drives_the_other_field() {
        let rect = Rect::new(0, 0, 100, 100);
        let out = set_width(rect, 1920, Some(16.0 / 9.0));
        assert_eq!((out.width, out.height), (1920, 1080));
        let out = set_height(rect, 1080, Some(16.0 / 9.0));
        assert_eq!((out.width, out.height), (1920, 1080));
    }

    #[test]
    fn a_typed_size_of_zero_becomes_one() {
        let rect = Rect::new(0, 0, 100, 100);
        assert_eq!(set_width(rect, 0, None).width, 1);
        assert_eq!(set_height(rect, -5, None).height, 1);
    }

    // --- click vs drag ------------------------------------------------------

    #[test]
    fn a_short_movement_is_a_click() {
        assert!(is_click((100, 100), (102, 101)));
        assert!(!is_click((100, 100), (104, 100)));
        assert!(!is_click((100, 100), (100, 105)));
    }
}
