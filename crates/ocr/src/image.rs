// SPDX-License-Identifier: GPL-3.0-or-later

//! The image an engine is given, and the two measurements taken off it.
//!
//! `spec/07` §2.2 asks for three things before any engine sees the pixels -- "upscale 2x
//! for small text (< 14 px), grayscale, adaptive threshold for dark-mode UIs (invert if
//! mean luminance < 0.4)" -- and only the greyscale survived contact with the models
//! (`docs/decisions.md` D82). The threshold would throw away the antialiasing a neural
//! recogniser was trained on; the inversion measurably *loses* text; and the upscale is the
//! detector's business, because it is the only part that knows what its model wants to see.
//!
//! What is left here is [`Gray`]: the luminance, the resampling both halves of the engine
//! need, and [`Gray::line_height`], which is how the detector decides its scale.

use crate::{Bounds, OcrError};

/// A row needs this many ink pixels to count as carrying type. [P]
///
/// Two rather than one, so a stray pixel from a border or a compression artefact does not
/// turn the whole image into one tall line and talk `line_height` out of its answer.
const INK: usize = 2;

/// An 8-bit greyscale image: what an engine is actually given.
#[derive(Clone, PartialEq, Eq)]
pub struct Gray {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

// Its `Debug` is its size. The derived one is a screenful of pixels per assertion.
impl std::fmt::Debug for Gray {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Gray({}x{})", self.width, self.height)
    }
}

impl Gray {
    /// # Errors
    /// If the pixels are not exactly `width * height` of them, or either is zero.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, OcrError> {
        let wanted = (width as usize) * (height as usize);
        if width == 0 || height == 0 || pixels.len() != wanted {
            return Err(OcrError::Size { width, height, pixels: pixels.len() });
        }
        Ok(Self { width, height, pixels })
    }

    /// Rec. 709 luminance of an RGBA buffer, ignoring alpha -- a grabbed frame is opaque,
    /// and a translucent one would be the compositor's own blend rather than something to
    /// un-multiply here. The same formula `crates/stitch` compares rows with.
    ///
    /// # Errors
    /// If the buffer is not exactly four bytes per pixel.
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Self, OcrError> {
        let wanted = (width as usize) * (height as usize) * 4;
        if width == 0 || height == 0 || rgba.len() != wanted {
            return Err(OcrError::Size { width, height, pixels: rgba.len() });
        }
        let pixels = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                let l = 0.2126 * f32::from(p[0])
                    + 0.7152 * f32::from(p[1])
                    + 0.0722 * f32::from(p[2]);
                l.round().clamp(0.0, 255.0) as u8
            })
            .collect();
        Self::new(width, height, pixels)
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    #[must_use]
    pub fn at(&self, x: u32, y: u32) -> u8 {
        self.pixels
            .get((y as usize) * (self.width as usize) + (x as usize))
            .copied()
            .unwrap_or(0)
    }

    /// Mean luminance, 0.0 to 1.0.
    #[must_use]
    pub fn mean(&self) -> f64 {
        if self.pixels.is_empty() {
            return 0.0;
        }
        let total: u64 = self.pixels.iter().map(|&p| u64::from(p)).sum();
        total as f64 / (self.pixels.len() as f64 * 255.0)
    }

    #[must_use]
    pub fn inverted(&self) -> Self {
        Self {
            width: self.width,
            height: self.height,
            pixels: self.pixels.iter().map(|p| 255 - p).collect(),
        }
    }

    /// Twice the size.
    ///
    /// Nearest-neighbour would be cheaper and is what a doubling usually gets, but a
    /// doubled screen glyph is exactly the case where it is wrong: the antialiasing that
    /// tells the recogniser where the stroke ends becomes a staircase, which is a shape the
    /// model has never been trained on.
    #[must_use]
    pub fn doubled(&self) -> Self {
        self.resized(self.width * 2, self.height * 2)
    }

    /// Any size, bilinear. Both ways: the recogniser wants every line it is given scaled
    /// to exactly 48 rows, which is as often down as up.
    #[must_use]
    pub fn resized(&self, width: u32, height: u32) -> Self {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) == (self.width, self.height) {
            return self.clone();
        }
        let (ratio_x, ratio_y) = (
            f64::from(self.width) / f64::from(width),
            f64::from(self.height) / f64::from(height),
        );
        let mut pixels = Vec::with_capacity((width as usize) * (height as usize));
        for y in 0..height {
            // The half-pixel offset both ways: destination pixel `y` covers the source
            // around `(y + 0.5) * ratio`, and source pixel centres sit at `y + 0.5`.
            let (y0, fy) = split((f64::from(y) + 0.5) * ratio_y - 0.5, self.height);
            let y1 = (y0 + 1).min(self.height - 1);
            for x in 0..width {
                let (x0, fx) = split((f64::from(x) + 0.5) * ratio_x - 0.5, self.width);
                let x1 = (x0 + 1).min(self.width - 1);
                let top = mix(self.at(x0, y0), self.at(x1, y0), fx);
                let bottom = mix(self.at(x0, y1), self.at(x1, y1), fx);
                pixels.push(mix_f(top, bottom, fy));
            }
        }
        Self { width, height, pixels }
    }

    /// The part of the image inside `bounds`, clamped to what is there.
    ///
    /// `None` when the box and the image do not meet -- a detector answering about rows
    /// that are not in the image is a bug, but it is the caller's to see rather than this
    /// one's to paper over with a blank crop.
    #[must_use]
    pub fn crop(&self, bounds: Bounds) -> Option<Self> {
        let left = bounds.left().clamp(0, self.width as i32) as u32;
        let top = bounds.top().clamp(0, self.height as i32) as u32;
        let right = bounds.right().clamp(0, self.width as i32) as u32;
        let bottom = bounds.bottom().clamp(0, self.height as i32) as u32;
        if right <= left || bottom <= top {
            return None;
        }
        let mut pixels = Vec::with_capacity(((right - left) as usize) * ((bottom - top) as usize));
        for y in top..bottom {
            let from = (y as usize) * (self.width as usize);
            pixels.extend_from_slice(self.pixels.get(from + left as usize..from + right as usize)?);
        }
        Self::new(right - left, bottom - top, pixels).ok()
    }

    /// The height of a line of type on this page, in pixels, or `None` if nothing on it
    /// looks like type.
    ///
    /// `spec/07` §2.2 wants small text upscaled, and nobody can measure text before it is
    /// read -- which is the order the clause implies and cannot have. So the page is asked
    /// instead: threshold it, find the runs of rows that carry ink, and take their median
    /// height. On a page of type those runs *are* the lines, and the median shrugs off the
    /// rule, the border and the one line of dashes.
    #[must_use]
    pub fn line_height(&self) -> Option<u32> {
        let cut = self.otsu();
        // Which side of the cut is the ink is decided by counting, not by assuming it is
        // the dark one. A terminal is light type on a dark page, and the version of this
        // that assumed dark ink answered "the whole image is one line" for every one of
        // them -- which is the detector's scale, so it was the whole capture's answer.
        let dark = self.pixels.iter().filter(|&&p| p <= cut).count();
        let ink_is_dark = dark * 2 <= self.pixels.len();
        let mut runs: Vec<u32> = Vec::new();
        let mut run = 0u32;
        for y in 0..self.height {
            let start = (y as usize) * (self.width as usize);
            let end = start + self.width as usize;
            let inked = self.pixels.get(start..end).map_or(0, |row| {
                row.iter().filter(|&&p| if ink_is_dark { p <= cut } else { p > cut }).count()
            });
            if inked >= INK {
                run += 1;
            } else if run > 0 {
                runs.push(run);
                run = 0;
            }
        }
        if run > 0 {
            runs.push(run);
        }
        runs.sort_unstable();
        runs.get(runs.len() / 2).copied()
    }

    /// Otsu's threshold: the grey that splits the histogram into the two groups with the
    /// least variance within them. The textbook answer to "which of these pixels are ink",
    /// and it needs no constant of its own, which is why it is here rather than a number.
    fn otsu(&self) -> u8 {
        let mut histogram = [0u64; 256];
        for &p in &self.pixels {
            histogram[p as usize] += 1;
        }
        let total = self.pixels.len() as f64;
        let sum: f64 = histogram.iter().enumerate().map(|(i, &n)| i as f64 * n as f64).sum();
        let (mut below, mut weighted, mut best, mut cut) = (0.0f64, 0.0f64, -1.0f64, 0u8);
        for (value, &count) in histogram.iter().enumerate() {
            below += count as f64;
            if below == 0.0 || below >= total {
                continue;
            }
            weighted += value as f64 * count as f64;
            let above = total - below;
            let spread = below * above * (weighted / below - (sum - weighted) / above).powi(2);
            if spread > best {
                best = spread;
                cut = value as u8;
            }
        }
        cut
    }
}

fn split(source: f64, limit: u32) -> (u32, f64) {
    if source <= 0.0 {
        return (0, 0.0);
    }
    let whole = (source.floor() as u32).min(limit - 1);
    (whole, source - f64::from(whole))
}

fn mix(a: u8, b: u8, t: f64) -> f64 {
    f64::from(a) + (f64::from(b) - f64::from(a)) * t
}

fn mix_f(a: f64, b: f64, t: f64) -> u8 {
    (a + (b - a) * t).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Bands of `ink` rows of type on a `pitch` row pitch, dark on light.
    fn typeset(width: u32, height: u32, ink: u32, pitch: u32) -> Gray {
        let mut pixels = Vec::with_capacity((width as usize) * (height as usize));
        for y in 0..height {
            for x in 0..width {
                let on = y % pitch < ink && x % 3 == 0;
                pixels.push(if on { 30 } else { 240 });
            }
        }
        Gray::new(width, height, pixels).expect("a page")
    }

    #[test]
    fn greyscale_is_the_luminance_the_stitcher_uses() {
        let rgba = vec![255, 255, 255, 255, 0, 0, 0, 255];
        let gray = Gray::from_rgba(2, 1, &rgba).expect("a strip");
        assert_eq!(gray.pixels(), &[255, 0]);
        // Rec. 709: green carries most of it, blue almost none.
        let green = Gray::from_rgba(1, 1, &[0, 255, 0, 255]).expect("a pixel");
        let blue = Gray::from_rgba(1, 1, &[0, 0, 255, 255]).expect("a pixel");
        assert!(green.at(0, 0) > blue.at(0, 0) * 8);
    }

    #[test]
    fn a_buffer_that_is_not_the_size_it_says_is_refused() {
        assert!(Gray::new(2, 2, vec![0; 3]).is_err());
        assert!(Gray::from_rgba(2, 2, &[0; 8]).is_err());
        assert!(Gray::new(0, 4, Vec::new()).is_err());
    }

    #[test]
    fn a_line_of_type_is_measured_by_the_rows_that_carry_ink() {
        assert_eq!(typeset(100, 200, 14, 20).line_height(), Some(14));
        assert_eq!(typeset(100, 200, 8, 12).line_height(), Some(8));
        assert_eq!(typeset(100, 200, 30, 44).line_height(), Some(30));
    }

    #[test]
    fn a_page_with_nothing_on_it_has_no_line_height() {
        let blank = Gray::new(10, 10, vec![250; 100]).expect("a page");
        assert_eq!(blank.line_height(), None);
    }

    #[test]
    fn turning_a_page_over_turns_its_mean_over_with_it() {
        let light = typeset(100, 200, 14, 20);
        let dark = light.inverted();
        let (up, down) = (light.mean(), dark.mean());
        assert!((up + down - 1.0).abs() < 1e-9, "{up} and {down} should add to one");
        // And the type is the same height either way, which is what the detector asks.
        assert_eq!(light.line_height(), dark.line_height());
    }

    #[test]
    fn a_crop_is_the_part_of_the_page_inside_the_box_and_nothing_where_there_is_none() {
        let page = typeset(40, 40, 14, 20);
        let crop = page.crop(Bounds::new(10, 10, 8, 6)).expect("a crop");
        assert_eq!((crop.width(), crop.height()), (8, 6));
        assert_eq!(crop.at(0, 0), page.at(10, 10));
        // Clamped to the page, and `None` when the box and the page do not meet at all.
        assert_eq!(page.crop(Bounds::new(36, 36, 20, 20)).map(|c| c.width()), Some(4));
        assert!(page.crop(Bounds::new(60, 0, 10, 10)).is_none());
        assert!(page.crop(Bounds::new(0, 0, 0, 10)).is_none());
    }

    #[test]
    fn doubling_keeps_the_page_the_page() {
        let page = typeset(40, 40, 14, 20);
        let doubled = page.doubled();
        assert_eq!((doubled.width(), doubled.height()), (80, 80));
        // The lines are twice as tall, and the page is neither darker nor lighter for it.
        assert_eq!(doubled.line_height(), Some(28));
        let (before, after) = (page.mean(), doubled.mean());
        assert!((after - before).abs() < 0.02, "{before} became {after}");
    }

    #[test]
    fn otsu_puts_the_ink_on_one_side_of_the_cut_and_the_paper_on_the_other() {
        // Which is all `line_height` asks of it: ink is `<= cut` and paper is above it.
        let cut = typeset(100, 100, 14, 20).otsu();
        assert!((30..240).contains(&cut), "{cut}");
    }
}
