// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §4.7's Smart Highlighter: the band of text a stroke snaps to.
//!
//! > while dragging, detect the text line under the stroke: take a horizontal band of the
//! > *base image* ±(3×size) around the stroke's average y, compute per-row luminance
//! > variance, find the contiguous rows with high variance (the text line), and snap the
//! > stroke to a straight horizontal band covering those rows with 2 units of margin.
//! > Ctrl held disables snapping. Falls back to the freehand stroke if no line is found.
//!
//! The whole of that is here, as arithmetic over a straight RGBA buffer -- which is what
//! makes it testable: a synthetic page with one dark line in it is four lines of setup,
//! and "did the stroke snap to the line" becomes an assertion about two numbers rather
//! than a squint at a screenshot.
//!
//! Two decisions the spec leaves open, and the reasons for them.
//!
//! **Only the rows under the stroke are measured.** §4.7 says "detect the text line under
//! the stroke", and a variance taken across the whole image width would be dominated by
//! whatever else is on that row of the screen -- a sidebar, a window edge, the desktop.
//! The stroke's own horizontal span is the question the user asked.
//!
//! **A line has to be *bounded* by the band to count.** The rows are accepted against a
//! threshold relative to the busiest row in the band, so the detector needs no absolute
//! idea of what a screenshot looks like; but a run that reaches both the top and the
//! bottom of the searched band has not been detected, it has merely filled the search.
//! That is what a photograph looks like, and what a font too big for the current size
//! looks like, and in both cases §4.7's answer is the freehand stroke.

use crate::geometry::Point;
use crate::redact::Pixels;

/// `spec/05` §4.7: "with 2 units of margin".
pub const MARGIN: f64 = 2.0;

/// How busy a row has to be, against the busiest row in the band, to count as text.
///
/// [M], and **measured** rather than guessed: 5 % is what reproduces the ink extents of
/// real antialiased text. A row through the x-height of a line has twenty times the
/// variance of a row through the tops of its ascenders alone, so anything near a half or a
/// third finds the middle of the line and calls that the line -- a 22 px font came out as
/// a five-unit band, and the band moved every time the size control changed the window it
/// was measured in. At a twentieth the question becomes "is there any ink in this row",
/// which is the question that has a stable answer: the gaps between lines have none.
const RELATIVE: f64 = 0.05;

/// The least variance the busiest row may have and still be called text.
///
/// Luminance runs 0 to 1, so this is a standard deviation of about 1.8 %: four or five
/// levels out of 255. Below it the band is flat -- a wallpaper, a title bar, a blank
/// margin -- and the relative threshold alone would happily "detect" its noise.
const FLOOR: f64 = 0.0003;

/// The least §4.7's `±(3 × size)` may come to, in document units.
///
/// The same shape of fix as `style::ARROWHEAD_FLOOR`, and for the same reason. The reach
/// is proportional so that the size control means "look further"; but at level 1 the
/// proportion is ±3 units, which is narrower than any line of text ever is -- the window
/// lands *inside* the glyphs, finds ink from edge to edge, and correctly reports that it
/// has detected nothing. A floor of twelve units is a line's worth of rows, so every size
/// starts from a window that can contain a line, and the ratio takes over from there.
const MIN_REACH: f64 = 12.0;

/// How much further to look when the first search was cut off by its own edge.
///
/// A run that reaches one end of the window has been ended by the *search*, not by the
/// page: the line goes on past what was measured. That happens whenever the stroke's
/// average y sits nearer one edge of its line than the reach is deep, which is most
/// strokes -- and it makes the band a function of where in the line the pen happened to
/// travel, which is exactly the instability the threshold was fixed to remove. So the
/// question is asked once more from a window three times as deep, which is four line
/// heights at the floor and enough for any line the first window nearly held.
const WIDEN: f64 = 3.0;

/// The straight band a freehand stroke snapped to, in document units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    /// The middle of the band: where the snapped stroke runs.
    pub y: f64,
    pub left: f64,
    pub right: f64,
    /// How thick the stroke is drawn, which is the detected rows plus [`MARGIN`] a side.
    pub height: f64,
}

impl Band {
    /// The two points a snapped stroke is made of.
    #[must_use]
    pub fn points(self) -> [Point; 2] {
        [Point::new(self.left, self.y), Point::new(self.right, self.y)]
    }
}

/// `spec/05` §4.7's detection, over the base image's pixels.
///
/// `scale` is how many buffer pixels make one document unit, the same convention
/// [`crate::redact`] uses: a 2x capture hands over twice as many rows per unit and the
/// answer still comes back in units.
///
/// `reach` is §4.7's `3 × size` -- the highlighter's own drawn width -- so the *size*
/// control is how far the snap looks, subject to [`MIN_REACH`]. Asking for a bigger band
/// is how a user snaps to a heading; the freehand stroke is what they get if they ask for
/// too small a one.
///
/// `None` means no line was found, and §4.7 is explicit about what to do with that: keep
/// the stroke the user drew.
#[must_use]
pub fn snap(image: &Pixels, scale: f64, points: &[Point], reach: f64) -> Option<Band> {
    if points.len() < 2 || scale <= 0.0 || reach <= 0.0 || image.width == 0 || image.height == 0 {
        return None;
    }
    let (mut left, mut right) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut sum = 0.0;
    for point in points {
        left = left.min(point.x);
        right = right.max(point.x);
        sum += point.y;
    }
    let middle = sum / points.len() as f64;
    let span = right - left;
    if !span.is_finite() || span <= 0.0 || !middle.is_finite() {
        return None;
    }

    let reach = reach.max(MIN_REACH);
    let first = search(image, scale, left, right, middle, reach);
    let (top, bottom, _) = match first {
        // Cut off by the window rather than by the page. See [`WIDEN`]. If the wider
        // window answers worse -- busy from end to end, which a big enough window over a
        // page eventually is -- the first answer stands.
        Some((_, _, true)) => search(image, scale, left, right, middle, reach * WIDEN).or(first)?,
        // A clean run, or nothing at all: the reach is the reach, and §4.7's answer to
        // nothing is the stroke the user drew.
        clean => clean?,
    };

    // The run is a range of rows, and a row covers the unit interval it was sampled from:
    // row `n` starts at `n / scale` and ends at `(n + 1) / scale`.
    let top = top as f64 / scale - MARGIN;
    let bottom = (bottom + 1) as f64 / scale + MARGIN;
    Some(Band { y: (top + bottom) / 2.0, left, right, height: bottom - top })
}

/// One pass: the rows of the line under the stroke, and whether the window ended it.
///
/// The two buffer rows are inclusive, and the flag says the run reached one end of what
/// was measured -- so the caller knows the answer is "as much of the line as I looked at"
/// rather than "the line".
fn search(
    image: &Pixels,
    scale: f64,
    left: f64,
    right: f64,
    middle: f64,
    reach: f64,
) -> Option<(usize, usize, bool)> {
    let rows = rows(image, scale, left, right, middle, reach)?;
    let peak = rows.values.iter().copied().fold(0.0_f64, f64::max);
    if peak < FLOOR {
        return None;
    }
    // Relative to the busiest row, but never below the floor: a window over a nearly blank
    // region has a peak of almost nothing, and a twentieth of almost nothing would promote
    // the sensor noise in it to a line of text.
    let run = run_around(&rows.values, rows.centre, (peak * RELATIVE).max(FLOOR))?;
    let (low, high) = (run.0 == 0, run.1 + 1 == rows.values.len());
    // Unbounded: the band is busy from end to end, so nothing was *detected*. See the
    // module note.
    if low && high {
        return None;
    }
    Some((rows.first + run.0, rows.first + run.1, low || high))
}

/// The per-row luminance variance of the band, and where in it the stroke was.
struct Measured {
    /// The buffer row the first value was measured from.
    first: usize,
    /// Which value the stroke's average y fell on.
    centre: usize,
    values: Vec<f64>,
}

fn rows(
    image: &Pixels,
    scale: f64,
    left: f64,
    right: f64,
    middle: f64,
    reach: f64,
) -> Option<Measured> {
    let to_px = |v: f64| v * scale;
    let clamp_x = |v: f64| v.clamp(0.0, image.width as f64);
    let clamp_y = |v: f64| v.clamp(0.0, image.height as f64);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let x0 = clamp_x(to_px(left).floor()) as usize;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let x1 = clamp_x(to_px(right).ceil()) as usize;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let y0 = clamp_y(to_px(middle - reach).floor()) as usize;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let y1 = clamp_y(to_px(middle + reach).ceil()) as usize;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let centre = clamp_y(to_px(middle).floor()) as usize;
    let values = (y0..y1).map(|y| variance(image, y, x0, x1)).collect();
    Some(Measured { first: y0, centre: centre.saturating_sub(y0).min(y1 - y0 - 1), values })
}

/// One row's luminance variance across `x0..x1`.
///
/// Rec. 709 luminance on straight, unpremultiplied bytes, weighted by alpha so a
/// transparent region reads as flat rather than as whatever colour happens to be stored
/// under its zero alpha.
fn variance(image: &Pixels, y: usize, x0: usize, x1: usize) -> f64 {
    let n = (x1 - x0) as f64;
    let (mut sum, mut squares) = (0.0, 0.0);
    for x in x0..x1 {
        let [r, g, b, a] = image.at(x, y);
        let alpha = f64::from(a) / 255.0;
        let luma = (0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b))
            / 255.0
            * alpha;
        sum += luma;
        squares += luma * luma;
    }
    let mean = sum / n;
    (squares / n - mean * mean).max(0.0)
}

/// The contiguous run of above-threshold rows containing -- or nearest to -- `centre`.
///
/// Nearest rather than containing, because the stroke's average y lands between two lines
/// as often as on one: a user swiping along a line of text tends to start above it and
/// end below. Searching outwards from where they were is what makes the gesture forgiving
/// without making it wrong; a stroke nowhere near any text still finds nothing, because
/// there is nothing above the threshold to find.
fn run_around(values: &[f64], centre: usize, threshold: f64) -> Option<(usize, usize)> {
    let seed = if values.get(centre).is_some_and(|v| *v >= threshold) {
        centre
    } else {
        (1..values.len())
            .flat_map(|d| [centre.checked_sub(d), centre.checked_add(d)])
            .flatten()
            .find(|i| values.get(*i).is_some_and(|v| *v >= threshold))?
    };
    let mut start = seed;
    while start > 0 && values[start - 1] >= threshold {
        start -= 1;
    }
    let mut end = seed;
    while end + 1 < values.len() && values[end + 1] >= threshold {
        end += 1;
    }
    Some((start, end))
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A white page with a dark bar across rows `top..bottom`, which is what a line of
    /// text looks like to a variance detector: busy where the glyphs are, flat elsewhere.
    fn page(width: usize, height: usize, top: usize, bottom: usize) -> Pixels {
        let mut pixels = Pixels::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let dark = y >= top && y < bottom && x % 3 != 0;
                let v = if dark { 20 } else { 245 };
                let i = (y * width + x) * 4;
                pixels.rgba[i] = v;
                pixels.rgba[i + 1] = v;
                pixels.rgba[i + 2] = v;
                pixels.rgba[i + 3] = 255;
            }
        }
        pixels
    }

    fn stroke(y: f64) -> Vec<Point> {
        vec![Point::new(10.0, y), Point::new(190.0, y - 3.0), Point::new(380.0, y + 4.0)]
    }

    #[test]
    fn a_stroke_across_a_line_of_text_snaps_to_it() {
        let image = page(400, 100, 40, 54);
        let band = snap(&image, 1.0, &stroke(47.0), 24.0).expect("no line found");
        // The detected rows are 40..54, plus §4.7's two units of margin on each side.
        assert!((band.y - 47.0).abs() < 0.001, "middle: {}", band.y);
        assert!((band.height - 18.0).abs() < 0.001, "height: {}", band.height);
        assert!((band.left - 10.0).abs() < 0.001);
        assert!((band.right - 380.0).abs() < 0.001);
    }

    #[test]
    fn the_snapped_stroke_is_straight() {
        let image = page(400, 100, 40, 54);
        let band = snap(&image, 1.0, &stroke(47.0), 24.0).expect("no line found");
        let [a, b] = band.points();
        assert!((a.y - b.y).abs() < f64::EPSILON, "the band is not horizontal");
    }

    #[test]
    fn a_stroke_over_blank_paper_finds_nothing() {
        let image = page(400, 100, 0, 0);
        assert!(snap(&image, 1.0, &stroke(47.0), 24.0).is_none());
    }

    #[test]
    fn a_band_busy_from_edge_to_edge_is_not_a_line() {
        // Every row is noisy, which is what a photograph looks like. There is no line to
        // find, and inventing one would snap the stroke to the whole search band.
        let image = page(400, 100, 0, 100);
        assert!(snap(&image, 1.0, &stroke(47.0), 24.0).is_none());
    }

    #[test]
    fn a_stroke_that_missed_the_line_still_finds_it() {
        // Drawn nine units above the text, well inside the ±24 the size asks for.
        let image = page(400, 100, 40, 54);
        let band = snap(&image, 1.0, &stroke(33.0), 24.0).expect("no line found");
        assert!((band.y - 47.0).abs() < 0.001, "middle: {}", band.y);
    }

    #[test]
    fn a_line_beyond_the_reach_is_left_alone() {
        // A stroke drawn twenty units above the text, at a size whose reach -- floored at
        // MIN_REACH -- still does not get there. §4.7 ties the search to the size, and a
        // size too small for the distance finds nothing.
        let image = page(400, 100, 40, 54);
        assert!(snap(&image, 1.0, &stroke(20.0), 4.0).is_none());
    }

    #[test]
    fn the_reach_never_falls_below_a_lines_worth_of_rows() {
        // Level 1 asks for ±3, which is narrower than the line it is over: the window
        // would be all ink and the detector would rightly say it had found nothing. The
        // floor is what stops the thinnest highlighter being the one that never snaps.
        let image = page(400, 100, 40, 54);
        let band = snap(&image, 1.0, &stroke(47.0), 3.0).expect("no line found");
        assert!((band.height - 18.0).abs() < 0.001, "height: {}", band.height);
    }

    #[test]
    fn a_hidpi_capture_answers_in_document_units() {
        // The same page at 2x: twice the rows, and the same answer in units.
        let image = page(800, 200, 80, 108);
        let band = snap(&image, 2.0, &stroke(47.0), 24.0).expect("no line found");
        assert!((band.y - 47.0).abs() < 0.001, "middle: {}", band.y);
        assert!((band.height - 18.0).abs() < 0.001, "height: {}", band.height);
    }

    #[test]
    fn a_dot_is_not_a_stroke() {
        let image = page(400, 100, 40, 54);
        assert!(snap(&image, 1.0, &[Point::new(10.0, 47.0)], 24.0).is_none());
    }

    #[test]
    fn a_transparent_region_is_flat() {
        // Alpha zero with colour underneath: a window capture's surround. Weighting the
        // luminance by alpha is what stops it reading as a page of text.
        let mut image = page(400, 100, 40, 54);
        for pixel in image.rgba.as_chunks_mut::<4>().0 {
            pixel[3] = 0;
        }
        assert!(snap(&image, 1.0, &stroke(47.0), 24.0).is_none());
    }
}
