// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/07` §1.2's row signatures: what a row of the page looks like, in two numbers.
//!
//! > use a central column band (rect.width x 0.5, excluding 12 % margins) to avoid
//! > scrollbars -- build row signatures (mean luminance + horizontal gradient energy per
//! > row)
//!
//! Two numbers and not the row's pixels, because the offset search compares every row
//! against every candidate shift: on a 1920x1200 frame that is 1.4 million comparisons,
//! which is nothing at two floats each and twenty-three megabytes of memory traffic at
//! 7680 bytes each. The pixels come back for the +-2 px refine, where there are five
//! candidates instead of twelve hundred.
//!
//! Why *two* numbers rather than the mean alone: a line of text and the blank line under
//! it can have the same mean luminance -- the text is thin -- and a page of prose is
//! mostly pairs of rows that differ only in whether anything is happening horizontally.
//! The gradient energy is exactly "is anything happening horizontally", and it is what
//! stops the matcher sliding a paragraph by one line.

use std::ops::Range;

use crate::frame::Frame;

/// One row, as the matcher sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    /// Mean Rec. 709 luminance over the band, 0.0 to 1.0.
    pub mean: f32,
    /// Mean absolute difference between neighbouring pixels' luminance over the band.
    pub energy: f32,
}

impl Row {
    /// Whether two rows are the same row of the page.
    ///
    /// Used only for the fixed-row test, where the question is "did this row not move",
    /// and where a false *yes* costs a duplicated sticky header and a false *no* costs
    /// nothing. The tolerance is one 255th -- a single quantisation step -- so a row that
    /// is genuinely identical survives a re-encode and a row that changed does not.
    #[must_use]
    pub fn same(self, other: Self) -> bool {
        const STEP: f32 = 1.0 / 255.0;
        (self.mean - other.mean).abs() <= STEP && (self.energy - other.energy).abs() <= STEP
    }
}

/// How wide a segment of a row is, in pixels: a glyph or two.
///
/// A row is also kept in segments, so that two rows can be compared over only some of
/// their columns -- the ones where each frame differs from the other at its own place
/// ([`crate::align::Changed`]) -- without reading the pixels again for every pair.
pub const SEGMENT: u32 = 16;

/// Every row of one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Signatures {
    rows: Vec<Row>,
    band: Range<u32>,
    /// How many segments of [`SEGMENT`] columns the band is read in; the last may be short.
    segments: usize,
    /// Per row, running totals over its segments -- `segments + 1` of them, the first zero
    /// -- of the luminance and of the energy.
    luminance: Vec<f32>,
    energy: Vec<f32>,
    /// Per row, the step into each segment from the one before it: part of the running
    /// energy, and not part of a run that begins there.
    inward: Vec<f32>,
}

impl Signatures {
    /// The signatures of a frame's rows, over the band the margins leave.
    #[must_use]
    pub fn of(frame: &Frame) -> Self {
        Self::over(frame, band(frame.width()))
    }

    /// The signatures of a frame's rows over the columns `band`, which the stitcher
    /// narrows for each pair of frames to the columns that moved
    /// ([`crate::align::moving`]).
    #[must_use]
    pub fn over(frame: &Frame, band: Range<u32>) -> Self {
        let width = frame.width();
        let band = band.start.min(width)..band.end.min(width).max(band.start.min(width));
        let columns = (band.end - band.start) as usize;
        let segments = columns.div_ceil(SEGMENT as usize);
        let height = frame.height() as usize;
        let mut rows = Vec::with_capacity(height);
        let mut luminance = vec![0.0f32; height * (segments + 1)];
        let mut energy = vec![0.0f32; height * (segments + 1)];
        let mut inward = vec![0.0f32; height * segments];
        for y in 0..frame.height() {
            let at = y as usize;
            let (from, to) = ((band.start as usize) * 4, (band.end as usize) * 4);
            let pixels = frame.row(y).get(from..to).unwrap_or(&[]);
            let (pixels, _) = pixels.as_chunks::<4>();
            let (mut sum, mut steps) = (0.0f32, 0.0f32);
            let mut before: Option<f32> = None;
            for (segment, cells) in pixels.chunks(SEGMENT as usize).enumerate() {
                for (i, pixel) in cells.iter().enumerate() {
                    let l = luminance_of(pixel);
                    sum += l;
                    if let Some(before) = before {
                        let step = (l - before).abs();
                        steps += step;
                        if i == 0 {
                            inward[at * segments + segment] = step;
                        }
                    }
                    before = Some(l);
                }
                luminance[at * (segments + 1) + segment + 1] = sum;
                energy[at * (segments + 1) + segment + 1] = steps;
            }
            rows.push(if pixels.is_empty() {
                Row { mean: 0.0, energy: 0.0 }
            } else {
                let count = pixels.len() as f32;
                Row { mean: sum / count, energy: steps / count }
            });
        }
        Self { rows, band, segments, luminance, energy, inward }
    }

    /// Row `y` read over the segments `run`: its luminance and its energy, summed, and
    /// without the step into the run's first segment -- which is an edge with a pixel the
    /// run leaves out, and so no part of it.
    #[must_use]
    pub fn read(&self, y: u32, run: &Range<usize>) -> (f32, f32) {
        let totals = (y as usize) * (self.segments + 1);
        let (from, to) = (totals + run.start, totals + run.end.min(self.segments));
        let first = self.inward.get((y as usize) * self.segments + run.start).copied();
        let (Some(&start), Some(&end)) = (self.luminance.get(from), self.luminance.get(to))
        else {
            return (0.0, 0.0);
        };
        let steps = self.energy[to] - self.energy[from] - first.unwrap_or(0.0);
        (end - start, steps)
    }

    /// How many columns the segments `run` hold.
    #[must_use]
    pub fn width(&self, run: &Range<usize>) -> u32 {
        let columns = self.band.end - self.band.start;
        let end = u32::try_from(run.end).unwrap_or(u32::MAX).saturating_mul(SEGMENT);
        let start = u32::try_from(run.start).unwrap_or(u32::MAX).saturating_mul(SEGMENT);
        end.min(columns).saturating_sub(start.min(columns))
    }

    /// How many segments each row is read in.
    #[must_use]
    pub fn segments(&self) -> usize {
        self.segments
    }

    #[must_use]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    #[must_use]
    pub fn len(&self) -> u32 {
        u32::try_from(self.rows.len()).unwrap_or(0)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The columns these signatures were measured over, which the pixel refine reuses so
    /// the two stages are looking at the same part of the page.
    #[must_use]
    pub fn band(&self) -> Range<u32> {
        self.band.clone()
    }

    #[must_use]
    pub fn get(&self, y: u32) -> Option<Row> {
        self.rows.get(y as usize).copied()
    }

    /// Whether the rows in `range` differ from one another at all.
    ///
    /// A blank stretch of page -- a margin, the gap under the last paragraph, a terminal
    /// with nothing in it -- has rows that are all the same, and every question about it
    /// has every answer. Callers ask this before trusting one.
    #[must_use]
    pub fn varies(&self, range: std::ops::Range<u32>) -> bool {
        let rows: Vec<Row> = range.filter_map(|y| self.get(y)).collect();
        if rows.len() < 2 {
            return false;
        }
        let n = rows.len() as f32;
        let spread = |pick: fn(Row) -> f32| {
            let mean = rows.iter().map(|r| pick(*r)).sum::<f32>() / n;
            rows.iter().map(|r| (pick(*r) - mean).powi(2)).sum::<f32>() / n
        };
        const FLAT: f32 = 1e-6;
        spread(|r| r.mean) > FLAT || spread(|r| r.energy) > FLAT
    }
}

/// The columns the matcher reads: nearly all of them.
///
/// `spec/07` §1.2 asks for "a central column band (rect.width x 0.5, excluding 12 %
/// margins)", and this was that band until 2026-09-23 -- when it was the reason a real
/// capture could not be stitched at all (D105). The page was a dialog of left-aligned
/// text, and its middle half was dark ground with the end of a long line crossing it
/// every few hundred rows: the matcher, looking only there, saw blank matched against
/// blank at almost every offset, and on the user's own frame it placed a 200-row scroll at
/// 178, 285-row answers for 300, and 630 for anything when told to expect 643 -- each one
/// *accepted*, because blank matched against blank scores a perfect fit. Most of what
/// anyone scrolls is set flush left: settings, lists, logs, code, chat.
///
/// So the band is the whole width less a sliver at each side: a fiftieth on the left for
/// a clipped window edge, and on the right a twenty-fifth but never under 16 px, which is
/// the width of a scrollbar whose thumb moves against the page. What else in the band
/// stands still while the page scrolls -- a sidebar, a margin, the chrome of the window --
/// is trimmed pair by pair by [`crate::align::moving`], which can see it; a fixed margin
/// here cannot. A frame too narrow for margins keeps every column, because one column of
/// signal beats a band of nothing.
#[must_use]
pub fn band(width: u32) -> Range<u32> {
    if width < 4 {
        return 0..width;
    }
    let left = (width / 50).max(2);
    let right = (width / 25).max(16).min(width / 4);
    left..width - right
}

/// Rec. 709 luminance, ignoring alpha: a grabbed frame is opaque, and a translucent one
/// would be the compositor's own blend rather than something to un-multiply here.
fn luminance_of(pixel: &[u8; 4]) -> f32 {
    let r = f32::from(pixel[0]);
    let g = f32::from(pixel[1]);
    let b = f32::from(pixel[2]);
    (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_band_is_nearly_the_whole_width_and_never_empty() {
        // A fiftieth off the left, a twenty-fifth off the right.
        assert_eq!(band(1920), 38..1844);
        assert_eq!(band(1541), 30..1480);
        // At least 16 px for the scrollbar, and never more than a quarter.
        assert_eq!(band(100), 2..84);
        assert_eq!(band(40), 2..30);
        // Too narrow for margins: every column, rather than nothing to correlate.
        assert_eq!(band(3), 0..3);
        assert_eq!(band(1), 0..1);
    }

    /// The page that retired the centred half (D105): type set flush left on a dark
    /// ground, as a dialog's list of fields is. The middle half of that frame is blank.
    #[test]
    fn text_set_flush_left_is_inside_the_band() {
        let (width, height) = (400u32, 40u32);
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                // Five rows of glyphs and three of leading, in the first quarter only.
                let ink = (20..90).contains(&x) && y % 8 < 5 && (x / 3) % 2 == 0;
                let tone = if ink { 220u8 } else { 30 };
                pixels.extend_from_slice(&[tone, tone, tone, 255]);
            }
        }
        let frame = Frame::new(width, height, pixels).expect("a frame");
        assert!(Signatures::of(&frame).varies(0..height), "the text is what the band reads");
        let half = (width / 4)..(width - width / 4);
        assert!(!Signatures::over(&frame, half).varies(0..height), "the old band read nothing");
    }

    /// A row read over a run of its segments is those columns and nothing beside them:
    /// the same two numbers as the columns read on their own, and no edge with the
    /// neighbour the run leaves out.
    #[test]
    fn a_run_of_segments_reads_as_its_own_columns() {
        let width = 5 * SEGMENT + 7;
        let mut pixels = Vec::with_capacity((width * 4) as usize);
        for x in 0..width {
            // Bright up to the second segment, then stripes: an edge right at the start of
            // the run, which is not the run's.
            let tone = if x < SEGMENT { 240u8 } else if (x / 3) % 2 == 0 { 20 } else { 140 };
            pixels.extend_from_slice(&[tone, tone, tone, 255]);
        }
        let frame = Frame::new(width, 1, pixels).expect("a row");
        let whole = Signatures::over(&frame, 0..width);
        assert_eq!(whole.segments(), 6, "the last one short");
        for run in [1..3usize, 1..6, 0..1, 5..6] {
            let columns = (run.start as u32) * SEGMENT..((run.end as u32) * SEGMENT).min(width);
            let alone = Signatures::over(&frame, columns.clone());
            let (sum, steps) = whole.read(0, &run);
            let count = whole.width(&run);
            assert_eq!(count, columns.end - columns.start, "{run:?}");
            let read = Row { mean: sum / count as f32, energy: steps / count as f32 };
            assert!(read.same(alone.rows()[0]), "{run:?}: {read:?} {:?}", alone.rows()[0]);
        }
        // All of it is the row as the band reads it.
        let (sum, steps) = whole.read(0, &(0..6));
        let all = Row { mean: sum / width as f32, energy: steps / width as f32 };
        assert!(all.same(whole.rows()[0]));
    }

    #[test]
    fn a_flat_row_has_no_energy_and_its_own_brightness() {
        let white = Signatures::of(&Frame::filled(8, 1, [255, 255, 255, 255]));
        let black = Signatures::of(&Frame::filled(8, 1, [0, 0, 0, 255]));
        assert!((white.rows()[0].mean - 1.0).abs() < 1e-6);
        assert!((black.rows()[0].mean - 0.0).abs() < 1e-6);
        assert!(white.rows()[0].energy.abs() < 1e-6);
    }

    #[test]
    fn two_rows_of_the_same_mean_are_told_apart_by_their_energy() {
        // Half black, half white: mean 0.5, one edge.
        let mut striped = vec![0u8; 8 * 4];
        let (pixels, _) = striped.as_chunks_mut::<4>();
        for pixel in pixels.iter_mut().skip(4) {
            pixel.copy_from_slice(&[255, 255, 255, 255]);
        }
        // Every pixel mid-grey: the same mean, no edge at all.
        let flat = [128u8, 128, 128, 255].repeat(8);
        let striped = Signatures::of(&Frame::new(8, 1, striped).expect("a row"));
        let flat = Signatures::of(&Frame::new(8, 1, flat.to_vec()).expect("a row"));
        assert!((striped.rows()[0].mean - flat.rows()[0].mean).abs() < 0.02);
        assert!(striped.rows()[0].energy > flat.rows()[0].energy + 0.1);
        assert!(!striped.rows()[0].same(flat.rows()[0]));
    }

    #[test]
    fn a_blank_run_of_rows_does_not_vary_and_a_page_does() {
        let blank = Signatures::of(&Frame::filled(32, 40, [250, 250, 250, 255]));
        assert!(!blank.varies(0..40));
        let page = Signatures::of(&crate::testing::window(32, 40, 0, 0));
        assert!(page.varies(0..40));
        // One row is not a run.
        assert!(!page.varies(3..4));
    }

    #[test]
    fn the_scrollbar_column_is_outside_the_band() {
        // A frame that differs only in its rightmost eighth -- where a scrollbar thumb
        // would move -- signs identically, which is the whole point of the band.
        let plain = Frame::filled(64, 2, [40, 40, 40, 255]);
        let mut with_bar = plain.pixels().to_vec();
        for y in 0..2usize {
            for x in 58..64usize {
                let at = (y * 64 + x) * 4;
                with_bar[at..at + 4].copy_from_slice(&[220, 220, 220, 255]);
            }
        }
        let with_bar = Frame::new(64, 2, with_bar).expect("a frame");
        let a = Signatures::of(&plain);
        let b = Signatures::of(&with_bar);
        assert!(a.rows()[0].same(b.rows()[0]));
    }
}
