// SPDX-License-Identifier: GPL-3.0-or-later

//! DBNet detection: where the text is.
//!
//! The numbers are the model's own, read out of the `inference.yml` PaddleX ships beside
//! it rather than invented here:
//!
//! ```yaml
//! DetResizeForTest: { resize_long: 960 }
//! NormalizeImage:   { mean: [0.485, 0.456, 0.406], std: [0.229, 0.224, 0.225] }
//! DBPostProcess:    { thresh: 0.3, box_thresh: 0.6, max_candidates: 1000, unclip_ratio: 1.5 }
//! ```

use crate::{Bounds, Gray};

/// How tall a line's box should be when the detector sees it. [P]
///
/// The model's own `inference.yml` says `resize_long: 960`, which is a rule for a
/// photograph of a document -- a 4000 px phone picture of a page, where 960 leaves the
/// type around thirty rows tall. Applied to a screenshot it is the wrong way round: a
/// 1920 px screen already has its type at fourteen rows, and halving it to seven is how a
/// full-screen capture comes back with six lines on it. So the *line* is what is fitted,
/// not the side, and thirty-ish rows is what `resize_long` was aiming at all along.
///
/// Measured on a 1920x1200 desktop screenshot: at 11 px a box the read is unusable, at
/// 22 px it is half there, at 30 px every label and button on it comes back.
const TALL: u32 = 30;
/// The most pixels worth handing the detector. Detection costs about 0.4 us a pixel on an
/// ordinary laptop, so this is `spec/10` §7's 1.5 s with the recogniser's share left over.
const AREA: f64 = 4_200_000.0;
/// And the most worth handing the first, throwaway look.
///
/// A seventh of a megapixel, measured rather than assumed (D95). The claim this whole
/// two-pass shape rests on is that the median box height does not move when the scale
/// does, and it holds further down than half a megapixel ever needed: over a wall of
/// 14 px type, a page of prose, a strip of overlay text and a 900 px page, dropping the
/// scout from 500 000 px to 150 000 found the **same number of boxes every time** and
/// answered within a tenth of the same scale, while the pass itself went from 200 ms to
/// 60 ms. Below about 100 000 the answers start to wander -- one page asked for 1.58
/// where the full look said 1.36 -- so this is the floor and not a target to keep
/// lowering.
const SCOUT: f64 = 150_000.0;
/// How far the image may be scaled either way. A page that needs more than this is not
/// going to be read by scaling it further.
const RANGE: (f64, f64) = (0.4, 4.0);
/// How far off the scale has to be before it is worth looking again.
const ENOUGH: f64 = 1.2;
/// And how many boxes the first look has to have found for its answer to mean anything.
const FEW: usize = 3;
/// PaddleOCR's `min_size`: a region thinner than this either way is a speck.
const SPECK: usize = 3;
/// The model's stride: both sides have to be a multiple of it.
const STRIDE: u32 = 32;
/// `thresh`: a pixel of the probability map is text above this.
const INK: f32 = 0.3;
/// `box_thresh`: a box whose mean probability is below this is not text.
const SURE: f32 = 0.6;
/// `unclip_ratio`: DBNet is trained to answer with a shrunken box, and this is how much of
/// it to give back.
const UNCLIP: f64 = 1.5;
/// `max_candidates`.
const MOST: usize = 1000;
/// ImageNet normalisation, which is what the model was trained with.
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const DEVIATION: [f32; 3] = [0.229, 0.224, 0.225];

/// The size to hand the model for a given scale, clamped to what is worth running.
#[must_use]
pub fn fit(width: u32, height: u32, wanted: f64) -> (u32, u32, f64) {
    let mut ratio = wanted.clamp(RANGE.0, RANGE.1);
    let area = f64::from(width) * f64::from(height) * ratio * ratio;
    if area > AREA {
        ratio *= (AREA / area).sqrt();
    }
    let round = |side: u32| {
        let scaled = (f64::from(side) * ratio).round() as u32;
        (scaled.div_ceil(STRIDE) * STRIDE).max(STRIDE)
    };
    (round(width), round(height), ratio)
}

/// The first, cheap look.
///
/// `spec/07` §2.2 wants small text upscaled, and the size of the text is the one thing
/// nobody knows before the read. The page cannot be asked -- a threshold and a count of
/// inked rows answers "one line, two hundred rows tall" for a contact sheet and for any
/// capture with a filled panel on it, and that answer is the whole capture's scale. The
/// detector can: it is the thing whose job is finding lines, its answer about *how tall*
/// they are is the same at every scale, and at a seventh of a megapixel it costs 60 ms.
#[must_use]
pub fn scout(width: u32, height: u32) -> (u32, u32, f64) {
    let area = f64::from(width) * f64::from(height);
    fit(width, height, if area > SCOUT { (SCOUT / area).sqrt() } else { 1.0 })
}

/// The scale the first look implies, or `None` if it was already close enough.
///
/// `boxes` are in the original image's pixels, which is what makes this work: the median
/// box height does not move when the scale does, so one number off a throwaway pass sets
/// the scale for the real one.
#[must_use]
pub fn again(boxes: &[Bounds], ratio: f64) -> Option<f64> {
    let mut heights: Vec<i32> = boxes.iter().map(|b| b.height).filter(|h| *h > 0).collect();
    if heights.len() < FEW {
        return None;
    }
    heights.sort_unstable();
    let median = f64::from(heights.get(heights.len() / 2).copied().unwrap_or(1)).max(1.0);
    let wanted = f64::from(TALL) / median;
    (wanted > ratio * ENOUGH || wanted * ENOUGH < ratio).then_some(wanted)
}

/// The image as the model wants it: NCHW, three channels of the same grey, normalised.
///
/// Three channels of the same thing because `spec/07` §2.2's pre-processing has already
/// made it grey, and a detector trained on photographs is looking for edges rather than
/// for colour.
#[must_use]
pub fn tensor(image: &Gray, width: u32, height: u32) -> Vec<f32> {
    let scaled = image.resized(width, height);
    let mut out = vec![0.0f32; 3 * (width as usize) * (height as usize)];
    for (channel, plane) in out.chunks_exact_mut((width as usize) * (height as usize)).enumerate() {
        let (mean, deviation) = (MEAN[channel], DEVIATION[channel]);
        for (value, pixel) in plane.iter_mut().zip(scaled.pixels()) {
            *value = (f32::from(*pixel) / 255.0 - mean) / deviation;
        }
    }
    out
}

/// The probability map's text regions, back in the original image's pixels.
///
/// DBNet answers with a map, not with boxes: every pixel's probability of being inside a
/// shrunken text region. The regions are the connected runs above `thresh`, their boxes are
/// those runs' extents, and `unclip_ratio` is how much of the shrinking to undo -- the
/// model was trained to answer small so that two lines of type do not touch.
///
/// The way back is **per axis**, and not the `ratio` the scale was asked for. [`fit`]
/// rounds each side up to the model's stride on its own, so a 1100 x 483 page asked for
/// at 1.667 is handed over as 1856 x 832 -- 1.687 across and 1.723 down, neither of them
/// the number requested. Undoing both with the requested one puts every box a little too
/// far down, and "a little" grows with `y`: 2 px at the top of that page and **16 px at
/// the foot of it**, which is half a line of type. The read degraded down the page, the
/// last lines came back as word-shaped nonsense and the URL on them was lost entirely --
/// while the same lines cropped out on their own read perfectly, because a shorter image
/// rounds less (2026-09-16, D95). The model sizes and the image's own are both here, so
/// the achieved scales are arithmetic rather than a parameter to be kept in step.
#[must_use]
pub fn boxes(map: &[f32], width: u32, height: u32, limit: (u32, u32)) -> Vec<Bounds> {
    let (width, height) = (width as usize, height as usize);
    if map.len() < width * height || width == 0 || height == 0 || limit.0 == 0 || limit.1 == 0 {
        return Vec::new();
    }
    let across = f64::from(limit.0) / width as f64;
    let down = f64::from(limit.1) / height as f64;
    let mut seen = vec![false; width * height];
    let mut stack: Vec<usize> = Vec::new();
    let mut found = Vec::new();
    for start in 0..width * height {
        if seen[start] || map[start] < INK {
            continue;
        }
        // Flood fill on an explicit stack. Recursion here is a stack overflow on a
        // screenshot of a wall of text, which is the case this exists for.
        let (mut left, mut right, mut top, mut bottom) = (width, 0usize, height, 0usize);
        seen[start] = true;
        stack.push(start);
        while let Some(at) = stack.pop() {
            let (x, y) = (at % width, at / width);
            left = left.min(x);
            right = right.max(x);
            top = top.min(y);
            bottom = bottom.max(y);
            let mut visit = |next: usize| {
                if !seen[next] && map[next] >= INK {
                    seen[next] = true;
                    stack.push(next);
                }
            };
            if x > 0 {
                visit(at - 1);
            }
            if x + 1 < width {
                visit(at + 1);
            }
            if y > 0 {
                visit(at - width);
            }
            if y + 1 < height {
                visit(at + width);
            }
        }
        if let Some(bounds) = accept(map, width, left, right, top, bottom, (across, down), limit) {
            found.push(bounds);
        }
        if found.len() >= MOST {
            break;
        }
    }
    found
}

/// One region: scored against `box_thresh`, unclipped, and put back in the caller's pixels.
#[allow(clippy::too_many_arguments)]
fn accept(
    map: &[f32],
    stride: usize,
    left: usize,
    right: usize,
    top: usize,
    bottom: usize,
    scale: (f64, f64),
    limit: (u32, u32),
) -> Option<Bounds> {
    // `box_score_fast`: the mean of the map over the region's box, not over the region --
    // a ragged region whose box is mostly empty is a smear rather than a line of type.
    let mut total = 0.0f64;
    let mut count = 0usize;
    for y in top..=bottom {
        for x in left..=right {
            total += f64::from(map.get(y * stride + x).copied().unwrap_or(0.0));
            count += 1;
        }
    }
    if count == 0 || total / (count as f64) < f64::from(SURE) {
        return None;
    }
    let (w, h) = ((right - left + 1) as f64, (bottom - top + 1) as f64);
    if w < SPECK as f64 || h < SPECK as f64 {
        return None;
    }
    // pyclipper's offset distance for a rectangle: area x ratio / perimeter.
    let grow = w * h * UNCLIP / (2.0 * (w + h));
    let (across, down) = scale;
    let x = ((left as f64 - grow) * across).round().max(0.0);
    let y = ((top as f64 - grow) * down).round().max(0.0);
    let far_x = ((right as f64 + 1.0 + grow) * across).round().min(f64::from(limit.0));
    let far_y = ((bottom as f64 + 1.0 + grow) * down).round().min(f64::from(limit.1));
    if far_x <= x || far_y <= y {
        return None;
    }
    Some(Bounds::new(x as i32, y as i32, (far_x - x) as i32, (far_y - y) as i32))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A probability map with a filled rectangle on it, as DBNet would answer.
    fn map(width: usize, height: usize, boxes: &[(usize, usize, usize, usize)]) -> Vec<f32> {
        let mut out = vec![0.0f32; width * height];
        for (x, y, w, h) in boxes {
            for row in *y..y + h {
                for column in *x..x + w {
                    if let Some(cell) = out.get_mut(row * width + column) {
                        *cell = 0.9;
                    }
                }
            }
        }
        out
    }

    #[test]
    fn a_size_the_model_is_given_is_always_a_multiple_of_its_stride() {
        for (width, height) in [(1920, 1200), (100, 37), (1, 1), (999, 1001)] {
            let (w, h, _) = fit(width, height, 1.0);
            assert_eq!((w % STRIDE, h % STRIDE), (0, 0), "{width}x{height}");
            assert!(w >= STRIDE && h >= STRIDE);
        }
    }

    #[test]
    fn a_scale_that_would_cost_more_than_the_budget_is_pulled_back() {
        // A 4K capture asked for at four times its size is 130 megapixels of detection.
        let (w, h, ratio) = fit(3840, 2160, 4.0);
        assert!(f64::from(w) * f64::from(h) <= AREA * 1.05, "{w}x{h}");
        assert!(ratio < 4.0);
        // And a small one is left where it was asked for.
        assert_eq!(fit(400, 300, 2.0).2, 2.0);
    }

    #[test]
    fn the_first_look_is_cheap_unless_shrinking_it_further_would_blind_it() {
        for (width, height) in [(1920, 1200), (600, 1000), (2000, 250)] {
            let (w, h, ratio) = scout(width, height);
            // Under the budget, or held off it by the floor -- a screen shrunk past
            // `RANGE.0` has 14 px type at five rows and the first look finds nothing at
            // all, which is the one way this pass can cost something and buy nothing.
            let cheap = f64::from(w) * f64::from(h) <= SCOUT * 1.2;
            assert!(cheap || ratio == RANGE.0, "{width}x{height} -> {w}x{h} at {ratio}");
            assert!(ratio <= 1.0);
        }
        // Anything already small enough is looked at as it is.
        assert_eq!(scout(400, 300).2, 1.0);
        // And a 4K screen stops at the floor rather than shrinking into illegibility --
        // which costs the first look more, and is still cheaper than a look at nothing.
        assert_eq!(scout(3840, 2160).2, RANGE.0);
    }

    #[test]
    fn the_second_look_happens_when_the_type_is_the_wrong_size_and_not_otherwise() {
        let small = [Bounds::new(0, 0, 100, 10); 5];
        let right = [Bounds::new(0, 0, 100, 30); 5];
        let huge = [Bounds::new(0, 0, 400, 120); 5];
        assert_eq!(again(&right, 1.0), None, "30 px at 1.0 is already what it wants");
        assert_eq!(again(&small, 1.0), Some(3.0), "10 px type wants trebling");
        assert_eq!(again(&huge, 1.0), Some(0.25), "and 120 px type wants quartering");
        // Nothing to go on is not an answer.
        assert_eq!(again(&[], 1.0), None);
        assert_eq!(again(&small[..2], 1.0), None);
    }

    #[test]
    fn a_region_of_the_map_becomes_a_box_rather_larger_than_itself() {
        // DBNet is trained to answer with a shrunken region, so the box has to grow.
        let found = boxes(&map(100, 60, &[(20, 20, 40, 10)]), 100, 60, (100, 60));
        assert_eq!(found.len(), 1, "{found:?}");
        let one = found[0];
        assert!(one.left() < 20 && one.top() < 20, "{one:?}");
        assert!(one.right() > 60 && one.bottom() > 30, "{one:?}");
        // And never off the edge of the image it came from.
        let corner = boxes(&map(100, 60, &[(0, 0, 40, 10)]), 100, 60, (100, 60));
        assert_eq!(corner.first().map(Bounds::left), Some(0));
        assert_eq!(corner.first().map(Bounds::top), Some(0));
    }

    #[test]
    fn two_regions_that_do_not_touch_are_two_boxes() {
        let found = boxes(&map(100, 60, &[(5, 5, 20, 6), (5, 40, 20, 6)]), 100, 60, (100, 60));
        assert_eq!(found.len(), 2, "{found:?}");
    }

    #[test]
    fn a_box_the_model_is_not_sure_of_is_not_a_box() {
        // A cross: joined up, so it is one region, but its own box is 97 % empty. A smear,
        // not a line of type.
        let mut cross = vec![0.0f32; 100 * 60];
        for step in 10..40 {
            cross[25 * 100 + step] = 0.9;
            cross[step * 100 + 25] = 0.9;
        }
        assert!(boxes(&cross, 100, 60, (100, 60)).is_empty());
    }

    #[test]
    fn a_speck_is_not_a_line_of_type() {
        let mut dust = vec![0.0f32; 100 * 60];
        for (x, y) in [(10, 10), (50, 30), (80, 50)] {
            dust[y * 100 + x] = 0.95;
        }
        assert!(boxes(&dust, 100, 60, (100, 60)).is_empty());
    }

    #[test]
    fn a_box_found_in_a_scaled_image_comes_back_in_the_captures_own_pixels() {
        // The map is half the size of the capture it was made from.
        let found = boxes(&map(100, 60, &[(20, 20, 40, 10)]), 100, 60, (200, 120));
        let one = found.first().copied().expect("a box");
        assert!(one.left() >= 20 && one.left() <= 40, "{one:?}");
        assert!(one.bottom() >= 60 && one.bottom() <= 80, "{one:?}");
    }

    /// D95. `fit` rounds each side up to the model's stride on its own, so the scale the
    /// image was handed over at is *two* numbers and neither is the one that was asked
    /// for. A map twice as wide and four times as tall as the capture puts a box at
    /// half its x and a quarter of its y -- and undoing both with one number put the
    /// bottom of a 483 px page sixteen rows out, which is half a line of type.
    #[test]
    fn a_map_stretched_differently_on_each_axis_comes_back_on_both() {
        let found = boxes(&map(100, 120, &[(20, 40, 40, 20)]), 100, 120, (50, 30));
        let one = found.first().copied().expect("a box");
        // x: 20..60 of 100 is 10..30 of 50. y: 40..60 of 120 is 10..15 of 30.
        assert!(one.left() >= 5 && one.left() <= 12, "{one:?}");
        assert!(one.right() >= 28 && one.right() <= 35, "{one:?}");
        assert!(one.top() >= 5 && one.top() <= 12, "{one:?}");
        assert!(one.bottom() >= 13 && one.bottom() <= 20, "{one:?}");
    }

    #[test]
    fn the_image_reaches_the_model_normalised_the_way_it_was_trained() {
        let white = Gray::new(64, 32, vec![255; 64 * 32]).expect("a page");
        let values = tensor(&white, 64, 32);
        assert_eq!(values.len(), 3 * 64 * 32);
        // (1.0 - mean) / deviation, per channel.
        for (channel, plane) in values.as_chunks::<{ 64 * 32 }>().0.iter().enumerate() {
            let wanted = (1.0 - MEAN[channel]) / DEVIATION[channel];
            let got = plane[0];
            assert!((got - wanted).abs() < 1e-5, "channel {channel}: {got} not {wanted}");
        }
    }
}
