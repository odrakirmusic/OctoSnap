// SPDX-License-Identifier: GPL-3.0-or-later

//! Properties of the overlay's geometry over the whole scale ladder (ENG-03, D173).
//!
//! The unit tests in `geometry.rs` and `selection.rs` pin the cases someone thought of.
//! These state what has to hold for every rect, every drag and every scale, and proptest
//! looks for the case that breaks it. A failure comes back shrunk to its smallest form,
//! and proptest keeps it in `proptest-regressions/` beside this file, so it is tried
//! first on every run after.
//!
//! No scale is the default case. The ladder below is every rung the harnesses use, with
//! the two that Mutter reports as `float32` approximations written as Mutter reports them,
//! and any scale from 1 to 4 is drawn as well.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use octosnap_core::Rect;
use octosnap_core::selection::{
    Direction, HANDLE_HIT_RADIUS, Handle, Hit, Modifiers, clamp_to_monitor, dominant_monitor,
    handle_centre, hit_test, is_click, nudge, rect_from_drag, resize_by_handle, resize_by_key,
    set_height, set_width,
};
use proptest::prelude::*;

const LADDER: [f64; 10] =
    [1.0, 1.25, 1.333_333_373_069_763_2, 1.5, 1.666_666_626_930_236_8, 1.75, 2.0, 2.25, 2.5, 3.0];

/// Coordinates as far as a desk of several large monitors reaches, either side of the
/// origin, since a monitor left of or above the primary has negative coordinates.
fn coord() -> impl Strategy<Value = i32> {
    -40_000..40_000i32
}

fn size() -> impl Strategy<Value = i32> {
    0..16_000i32
}

fn rect() -> impl Strategy<Value = Rect> {
    (coord(), coord(), size(), size()).prop_map(|(x, y, w, h)| Rect::new(x, y, w, h))
}

fn point() -> impl Strategy<Value = (i32, i32)> {
    (coord(), coord())
}

fn scale() -> impl Strategy<Value = f64> {
    prop_oneof![prop::sample::select(LADDER.to_vec()), 1.0..4.0f64]
}

fn handle() -> impl Strategy<Value = Handle> {
    prop::sample::select(Handle::ALL.to_vec())
}

fn direction() -> impl Strategy<Value = Direction> {
    prop::sample::select(vec![Direction::Left, Direction::Right, Direction::Up, Direction::Down])
}

fn modifiers() -> impl Strategy<Value = Modifiers> {
    (any::<bool>(), any::<bool>())
        .prop_map(|(aspect_lock, from_centre)| Modifiers { aspect_lock, from_centre })
}

/// A ratio a person can lock: from a tall 1:10 to a wide 10:1.
fn ratio() -> impl Strategy<Value = f64> {
    0.1..10.0f64
}

fn area(r: Rect) -> i64 {
    i64::from(r.width.max(0)) * i64::from(r.height.max(0))
}

fn contains(outer: Rect, inner: Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x + inner.width <= outer.x + outer.width
        && inner.y + inner.height <= outer.y + outer.height
}

/// The scales Mutter could offer for a mode: every `width / logical` that also divides the
/// height into whole logical pixels, as the `float32` Mutter stores it.
fn mutter_scales(width: u32, height: u32) -> Vec<(f64, u32, u32)> {
    let mut out = Vec::new();
    for logical in (width / 4).max(1)..=width {
        let scale = f64::from(width as f32 / logical as f32);
        let logical_height = f64::from(height) / scale;
        if (logical_height - logical_height.round()).abs() < 1e-3 && logical_height >= 1.0 {
            out.push((scale, logical, logical_height.round() as u32));
        }
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    // --- Rect ---------------------------------------------------------------------------

    #[test]
    fn overlap_is_symmetric_and_bounded(a in rect(), b in rect()) {
        let ab = a.overlap_area(b);
        prop_assert_eq!(ab, b.overlap_area(a));
        prop_assert!(ab >= 0);
        prop_assert!(ab <= area(a).min(area(b)));
    }

    #[test]
    fn a_rect_inside_another_overlaps_it_by_its_whole_area(outer in rect(), fx in 0.0..1.0f64,
            fy in 0.0..1.0f64, fw in 0.0..1.0f64, fh in 0.0..1.0f64) {
        let w = (f64::from(outer.width) * fw) as i32;
        let h = (f64::from(outer.height) * fh) as i32;
        let x = outer.x + (f64::from(outer.width - w) * fx) as i32;
        let y = outer.y + (f64::from(outer.height - h) * fy) as i32;
        let inner = Rect::new(x, y, w, h);
        prop_assert_eq!(inner.overlap_area(outer), area(inner));
    }

    /// D23: rounding to nearest, as Mutter does, is the whole rule.
    #[test]
    fn physical_size_is_the_logical_size_times_the_scale_rounded(r in rect(), s in scale()) {
        let (w, h) = r.to_physical(s);
        prop_assert_eq!(f64::from(w), (f64::from(r.width) * s).round().max(0.0));
        prop_assert_eq!(f64::from(h), (f64::from(r.height) * s).round().max(0.0));
    }

    #[test]
    fn a_wider_rect_is_never_narrower_in_physical_pixels(r in rect(), more in 0..1000i32, s in scale()) {
        let wider = Rect::new(r.x, r.y, r.width + more, r.height);
        prop_assert!(wider.to_physical(s).0 >= r.to_physical(s).0);
    }

    /// D23's case, for every mode and every scale Mutter could offer for it: a whole monitor
    /// in logical pixels comes back as exactly the mode, though the scale is a `float32`.
    #[test]
    fn a_whole_monitor_converts_back_to_its_mode(width in 640..7_681u32, height in 480..4_321u32) {
        for (scale, logical_w, logical_h) in mutter_scales(width, height) {
            let monitor = Rect::new(0, 0, logical_w as i32, logical_h as i32);
            prop_assert_eq!(monitor.to_physical(scale), (width, height),
                "{}x{} at {} is {}x{} logical", width, height, scale, logical_w, logical_h);
        }
    }

    // --- Drawing a selection ------------------------------------------------------------

    #[test]
    fn a_plain_drag_spans_its_two_points(anchor in point(), pointer in point()) {
        let r = rect_from_drag(anchor, pointer, Modifiers::default(), None);
        prop_assert_eq!(r.x, anchor.0.min(pointer.0));
        prop_assert_eq!(r.y, anchor.1.min(pointer.1));
        prop_assert_eq!(r.width, (pointer.0 - anchor.0).abs());
        prop_assert_eq!(r.height, (pointer.1 - anchor.1).abs());
    }

    #[test]
    fn a_drag_never_has_a_negative_size(anchor in point(), pointer in point(), m in modifiers(),
            locked in proptest::option::of(ratio())) {
        let r = rect_from_drag(anchor, pointer, m, locked);
        prop_assert!(r.width >= 0 && r.height >= 0);
    }

    /// Alt: the press is the centre.
    #[test]
    fn an_alt_drag_is_centred_on_the_press(anchor in point(), pointer in point(),
            locked in proptest::option::of(ratio()), shift in any::<bool>()) {
        let m = Modifiers { aspect_lock: shift, from_centre: true };
        let r = rect_from_drag(anchor, pointer, m, locked);
        prop_assert_eq!(r.x + r.width / 2, anchor.0);
        prop_assert_eq!(r.y + r.height / 2, anchor.1);
    }

    /// Shift grows the smaller side, so the locked rect holds the plain one, at the ratio.
    #[test]
    fn a_shift_drag_holds_the_ratio_and_the_plain_drag(anchor in point(), pointer in point(),
            locked in ratio()) {
        let shift = Modifiers { aspect_lock: true, from_centre: false };
        let plain = rect_from_drag(anchor, pointer, Modifiers::default(), None);
        let r = rect_from_drag(anchor, pointer, shift, Some(locked));
        prop_assert!(r.width >= plain.width && r.height >= plain.height);
        if r.width > 0 && r.height > 0 && plain.height > 0 {
            // Rounding one side to a whole pixel moves the ratio by under a pixel's worth.
            let off = (f64::from(r.width) - locked * f64::from(r.height)).abs();
            prop_assert!(off <= locked.max(1.0), "{}x{} at {}", r.width, r.height, locked);
        }
    }

    // --- Changing it --------------------------------------------------------------------

    /// The edge a handle does not move stays where it was, as one edge of the result.
    #[test]
    fn a_handle_keeps_the_opposite_edge(r in rect(), h in handle(), pointer in point()) {
        let out = resize_by_handle(r, h, pointer, Modifiers::default(), None);
        prop_assert!(out.width >= 0 && out.height >= 0);
        let (left, right) = (out.x, out.x + out.width);
        let (top, bottom) = (out.y, out.y + out.height);
        match h {
            Handle::Right | Handle::TopRight | Handle::BottomRight => {
                prop_assert!(r.x == left || r.x == right)
            }
            Handle::Left | Handle::TopLeft | Handle::BottomLeft => {
                prop_assert!(r.x + r.width == left || r.x + r.width == right)
            }
            Handle::Top | Handle::Bottom => prop_assert!(left == r.x && right == r.x + r.width),
        }
        match h {
            Handle::Bottom | Handle::BottomLeft | Handle::BottomRight => {
                prop_assert!(r.y == top || r.y == bottom)
            }
            Handle::Top | Handle::TopLeft | Handle::TopRight => {
                prop_assert!(r.y + r.height == top || r.y + r.height == bottom)
            }
            Handle::Left | Handle::Right => prop_assert!(top == r.y && bottom == r.y + r.height),
        }
    }

    #[test]
    fn a_resize_never_has_a_negative_size(r in rect(), h in handle(), pointer in point(),
            m in modifiers(), locked in proptest::option::of(ratio())) {
        let out = resize_by_handle(r, h, pointer, m, locked);
        prop_assert!(out.width >= 0 && out.height >= 0);
    }

    #[test]
    fn a_nudge_and_its_opposite_cancel(r in rect(), d in direction(), step in 1..100i32) {
        let back = match d {
            Direction::Left => Direction::Right,
            Direction::Right => Direction::Left,
            Direction::Up => Direction::Down,
            Direction::Down => Direction::Up,
        };
        prop_assert_eq!(nudge(nudge(r, d, step), back, step), r);
    }

    /// `spec/03` §5.1: 1x1 is legal, zero is not a selection.
    #[test]
    fn a_key_resize_never_goes_below_one_pixel(r in rect(), d in direction(), step in 1..100i32) {
        let out = resize_by_key(Rect::new(r.x, r.y, r.width.max(1), r.height.max(1)), d, step);
        prop_assert!(out.width >= 1 && out.height >= 1);
        prop_assert_eq!((out.x, out.y), (r.x, r.y));
    }

    #[test]
    fn typed_sizes_keep_the_locked_ratio(r in rect(), typed in 1..16_000i32, locked in ratio()) {
        let w = set_width(r, typed, Some(locked));
        prop_assert_eq!(w.width, typed);
        prop_assert!(w.height >= 1);
        prop_assert!((f64::from(w.width) / locked - f64::from(w.height)).abs() <= 1.0);
        let h = set_height(r, typed, Some(locked));
        prop_assert_eq!(h.height, typed);
        prop_assert!(h.width >= 1);
        prop_assert!((f64::from(h.height) * locked - f64::from(h.width)).abs() <= 1.0);
        prop_assert_eq!((w.x, w.y, h.x, h.y), (r.x, r.y, r.x, r.y));
    }

    // --- Monitors -----------------------------------------------------------------------

    #[test]
    fn clamping_lands_inside_the_monitor_and_is_final(r in rect(), monitor in rect()) {
        prop_assume!(monitor.width > 0 && monitor.height > 0);
        let once = clamp_to_monitor(r, monitor);
        if r.width >= 0 && r.height >= 0 {
            prop_assert!(contains(monitor, once), "{:?} in {:?} gave {:?}", r, monitor, once);
        }
        prop_assert_eq!(clamp_to_monitor(once, monitor), once);
    }

    #[test]
    fn a_selection_already_inside_is_not_moved(monitor in rect(), fx in 0.0..1.0f64,
            fy in 0.0..1.0f64, fw in 0.0..1.0f64, fh in 0.0..1.0f64) {
        let w = (f64::from(monitor.width) * fw) as i32;
        let h = (f64::from(monitor.height) * fh) as i32;
        let x = monitor.x + (f64::from(monitor.width - w) * fx) as i32;
        let y = monitor.y + (f64::from(monitor.height - h) * fy) as i32;
        let inside = Rect::new(x, y, w, h);
        prop_assert_eq!(clamp_to_monitor(inside, monitor), inside);
    }

    #[test]
    fn the_dominant_monitor_holds_the_most_of_the_selection(r in rect(),
            monitors in prop::collection::vec(rect(), 0..5)) {
        match dominant_monitor(r, &monitors) {
            None => prop_assert!(monitors.iter().all(|m| r.overlap_area(*m) == 0)),
            Some(i) => {
                let best = r.overlap_area(monitors[i]);
                prop_assert!(best > 0);
                prop_assert!(monitors.iter().all(|m| r.overlap_area(*m) <= best));
            }
        }
    }

    // --- Pressing -----------------------------------------------------------------------

    /// On a selection big enough that no two handles' hit areas meet, each handle's own
    /// centre is that handle.
    #[test]
    fn each_handle_is_found_at_its_centre(x in coord(), y in coord(),
            w in (4 * HANDLE_HIT_RADIUS + 2)..16_000, h in (4 * HANDLE_HIT_RADIUS + 2)..16_000,
            which in handle()) {
        let r = Rect::new(x, y, w, h);
        prop_assert_eq!(hit_test(Some(r), handle_centre(r, which)), Hit::Handle(which));
    }

    #[test]
    fn with_no_selection_every_press_starts_one(p in point()) {
        prop_assert_eq!(hit_test(None, p), Hit::Outside);
    }

    #[test]
    fn a_click_is_symmetric(a in point(), b in point()) {
        prop_assert_eq!(is_click(a, b), is_click(b, a));
        prop_assert!(is_click(a, a));
    }
}

/// A random mode has few scales that divide it into whole logical pixels, and the modes
/// people own have many, so these are checked one by one: laptops, desktops, and the
/// panels that are known for odd sizes.
#[test]
fn every_scale_of_every_common_mode_converts_back_to_the_mode() {
    let modes = [
        (1366, 768),
        (1600, 900),
        (1920, 1080),
        (1920, 1200),
        (2256, 1504),
        (2560, 1440),
        (2560, 1600),
        (2880, 1800),
        (2880, 1920),
        (3000, 2000),
        (3024, 1964),
        (3200, 1800),
        (3440, 1440),
        (3840, 2160),
        (3840, 2400),
        (5120, 2160),
        (5120, 2880),
        (6016, 3384),
        (7680, 4320),
    ];
    let mut checked = 0;
    for (width, height) in modes {
        for (scale, logical_w, logical_h) in mutter_scales(width, height) {
            let monitor = Rect::new(0, 0, logical_w as i32, logical_h as i32);
            assert_eq!(monitor.to_physical(scale), (width, height), "{width}x{height} at {scale}");
            checked += 1;
        }
    }
    assert!(checked > 200, "only {checked} mode and scale pairs");
}
