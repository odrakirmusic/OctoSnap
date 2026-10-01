// SPDX-License-Identifier: GPL-3.0-or-later

//! How far the page moved between two frames, and which rows did not move with it.
//!
//! `spec/07` §1.2, the middle of the loop:
//!
//! > detect FIXED rows (identical across F0..Fi at the same y: sticky headers/footers)
//! > and exclude them -- 1-D normalized cross-correlation of signatures over
//! > d in [0, rect.height]; refine +-2 px with full-pixel SSD -- accept if NCC > 0.85 and
//! > d > 0
//!
//! One reading had to be settled before any of this could be written. The spec also says
//! "append Fi[d:] to the canvas", which would make `d` the size of the *overlap*; but it
//! then says "if d == 0 for 2 iterations -> end of content", and a zero overlap is a page
//! that jumped, not a page that ended. `d` is therefore the **scroll distance**: the
//! content at row `y` of the previous frame is at row `y - d` of this one, the new rows
//! are the last `d` of the body, and `d == 0` means nothing moved. Everything downstream
//! follows that reading, and the tests below state it in both directions.

use std::ops::Range;

use crate::frame::Frame;
use crate::signature::{Row, Signatures, SEGMENT};

/// `spec/07` §1.2: "accept if NCC > 0.85".
pub const ACCEPT: f64 = 0.85;

/// `spec/07` §1.2: "refine +-2 px with full-pixel SSD".
const REFINE: u32 = 2;

/// How much of the body two frames must share before a score means anything.
///
/// [P]. Without a floor the best correlation is always the largest `d`, because two rows
/// correlate perfectly with themselves and a one-row overlap is two rows.
///
/// A tenth, not the quarter this started at. A quarter is right for the loop `spec/07`
/// §1.2 describes, where a 0.6-height step leaves 40 % of the body shared -- but `spec/07`
/// §1.1 item 5's manual mode is driven by a person, and a person flicks a wheel as far as
/// they like. An 842 px flick over a 1 000 px selection is 16 % shared, which was outside
/// the search entirely: the matcher answered with the best offset it was *allowed* to
/// consider, the pixels were never asked because the answer looked plausible, and a
/// 12 000 px capture came back with nine lines missing at every seam (2026-09-15). A tenth
/// leaves a hundred rows to correlate over, which is five lines of ordinary type, and
/// [`FIT`] is what stops the short overlaps from lying.
const MIN_OVERLAP: f64 = 0.1;

/// Under this, a run of rows has no shape to correlate and the score comes from the means.
const FLAT: f64 = 1e-6;

/// Scores this close together are the same score, and the step we asked for wins.
const TIE: f64 = 1e-3;

/// How many tied peaks the pixels are asked about.
///
/// [P]. Eight line pitches either side of the step covers any page whose type is set on a
/// grid; past that the frames share too little for another peak to be the real one.
const CONTENDERS: usize = 8;

/// Rows per sample when the pixels are separating candidates rather than refining one.
///
/// [P]. The candidates are a line pitch or more apart -- fourteen rows at the smallest
/// type anyone reads -- so every eighth row sees the difference, and costs an eighth.
const COARSE: u32 = 8;

/// The rows of a page that stay where they are while the rest of it scrolls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Fixed {
    /// Sticky rows at the top, drawn once from the first frame.
    pub header: u32,
    /// Sticky rows at the bottom, drawn once at the end from the last frame.
    pub footer: u32,
}

/// The most of a frame that can be sticky before the word stops meaning anything.
///
/// [P]. Two frames of a page that did not scroll are identical from top to bottom, and
/// without a cap that reads as "the whole frame is a sticky header" -- after which there
/// is no body left to correlate and the capture stops with no reason to give. Two fifths
/// each leaves a fifth of the frame as body in the worst case, which is enough to score.
const MOST: f64 = 0.4;

/// How far the overlap may differ before it is not the same content, as mean squared
/// difference per channel over the band.
///
/// [P]. At the offset the page really moved by, the overlap is *the same pixels* -- so the
/// number here is not a similarity threshold but a "did anything at all go wrong" one, and
/// there is a factor of ten of daylight either side of it. Thirty levels per channel
/// covers a redrawn scrollbar, a caret, a hover state and any amount of anti-aliasing; a
/// frame placed one line out puts ink where the ground was and scores in the thousands.
///
/// It is what turns "the correlation is confident and wrong" into a miss, which is
/// `spec/07` §1.2's "retry with a smaller step" and, in manual mode, a line on the pill.
const FIT: f64 = 900.0;

/// How much worse over everything a candidate may fit than the best of them before what
/// changed is no longer allowed to speak for it.
///
/// [P]. [`Changed`] compares a pair of rows only where both of its places changed, which
/// is what lets a banner stand over the page -- and it also lets a wrong offset leave out
/// its own mistakes. Where the type of one frame falls on the leading of the other, ink is
/// compared with ground that did not change and none of it counts: over a page the
/// compositor resampled, an offset half a line out was judged on the seven pairs of
/// segments that were left, fitted them exactly and beat the truth (2026-09-24), while
/// over everything it differed by twenty-five times what the truth did. A banner costs the
/// true offset its own rows over everything and every other offset those rows and more,
/// and on the fixtures that never put the truth more than a quarter behind the best -- so
/// eight times is room to spare. Nothing that fits under [`FIT`] over everything is ruled
/// out by this, because a page that fits that well at an offset is a page that is there.
const WORSE: f64 = 8.0;

/// How far two rows may differ and still be the same sticky row, as mean squared
/// difference per channel over the band.
///
/// [P]. Not zero: a sticky header is redrawn every frame and a shadow, a focus ring or a
/// subpixel-positioned label moves a few levels between redraws. Four is a mean difference
/// of two levels in 255, which no glyph moving anywhere stays under.
const STUCK: f64 = 4.0;

/// The fewest columns that must differ between two frames for them to be a scroll.
///
/// [P]. A caret is two pixels wide and a spinner a few dozen; a scroll moves every column
/// that has anything in it. A twentieth of the band, and never fewer than eight columns
/// -- or the whole band, when it is narrower than that.
fn enough(band: u32) -> u32 {
    (band / 20).max(8).min(band.max(1))
}

/// The part of `band` where the two frames differ at all, trimmed from both ends: the
/// columns a scroll can be measured in.
///
/// A sidebar, the frame of a window or the margin beside a centred column of text is the
/// same pixels in both frames while the page beside it scrolls -- and inside the band that
/// is worse than nothing to the matcher. At the offset the page really moved, those
/// columns disagree on every row, and a sidebar of text a fifth of the width wide is
/// enough to push the difference past [`FIT`] and turn the right answer into a miss. So
/// the ends that did not move are cut off before anything is measured (D105). Only the
/// ends: a still column in the middle of a page is rarer, and trimming it would need a
/// mask rather than a range.
///
/// `None` when the change is not the shape of a scroll: nothing changed at all, fewer
/// than [`enough`] columns did, or what changed is a **patch** -- inside a quarter of the
/// frame's height *and* half the band's width. A caret, a spinner, a clock, an animation
/// playing in the page is a patch; measured on the patch alone, the matcher is asked where
/// a picture of a spinner scrolled to, and its answer is a miss that tells a user who has
/// not scrolled at all that their scroll was too big (D105). Both dimensions, because a
/// scroll can change little: an hour grid moved by whole hours changes only the hours
/// whose appointments differ, which can be one row of events -- short, but as wide as the
/// day. Every other row of a changed column is compared, so a caret sixteen rows tall is
/// seen and a scroll never escapes it.
#[must_use]
pub fn moving(previous: &Frame, current: &Frame, band: Range<u32>) -> Option<Range<u32>> {
    let height = previous.height().min(current.height());
    let (from, to) = ((band.start as usize) * 4, (band.end as usize) * 4);
    let differs = |y: u32| previous.row(y).get(from..to) != current.row(y).get(from..to);
    let top = (0..height).find(|&y| differs(y))?;
    let bottom = (0..height).rev().find(|&y| differs(y))?;
    let changed = |x: u32| {
        let at = (x as usize) * 4;
        (top..=bottom)
            .step_by(2)
            .any(|y| previous.row(y).get(at..at + 3) != current.row(y).get(at..at + 3))
    };
    let flags: Vec<bool> = band.clone().map(changed).collect();
    let count = u32::try_from(flags.iter().filter(|&&moved| moved).count()).unwrap_or(0);
    if count < enough(band.end.saturating_sub(band.start)) {
        return None;
    }
    let first = u32::try_from(flags.iter().position(|&moved| moved)?).ok()?;
    let last = u32::try_from(flags.iter().rposition(|&moved| moved)?).ok()?;
    let span = band.end.saturating_sub(band.start);
    if bottom + 1 - top < (height / 4).max(1) && last + 1 - first < (span / 2).max(1) {
        return None;
    }
    Some(band.start + first..band.start + last + 1)
}

/// Where two frames differ at their own place, segment by segment of each row: what
/// [`place`] compares a pair of rows over at any offset but zero.
///
/// Whatever stood still while the page moved under it is the same pixels at the same place
/// in both frames -- a notification, a toast, a floating button, the cards of earlier
/// captures, the pill of a capture whose selection fills the screen -- and it agrees with
/// itself at offset zero and with nothing at the offset the page really moved. Over
/// flush-left type, where little else is there to outvote it, a banner turned eleven
/// placements in twelve into misses (2026-09-23, D105). Looking for it was tried first:
/// cells with detail in them that held still, grown out by the banner's colour, left out
/// by their rows or cut out by their columns. The first full-screen capture in which the
/// pill, a banner and three cards stood over the page at once found what that misses --
/// the pill's preview is a flat dark panel with no detail in it, a card showing a blank
/// capture is a white one, and a button lit up under the pointer is neither the same nor
/// the page -- and not one frame of it was placed (D107).
///
/// So nothing is looked for. A pair of rows `d` apart is compared over the segments that
/// changed at *both* of its places, and what stood still is left out whatever it looks
/// like. Nothing the scroll says is lost by that: page that is the same at its own place
/// in both frames is page that repeats itself exactly as far as it moved -- blank ground,
/// mostly -- and it agrees at the true offset whether it is compared or not.
///
/// Offset zero is the exception, and is compared whole. There, what changed is exactly
/// what disagrees; and "the page did not move" is the answer every other has to beat.
#[derive(Debug, Clone)]
pub struct Changed {
    /// Words of bits per row: one bit per segment of the band, set where the frames differ.
    words: usize,
    bits: Vec<u64>,
}

impl Changed {
    /// The segments of `band` in which `previous` and `current` differ, row by row.
    #[must_use]
    pub fn between(previous: &Frame, current: &Frame, band: Range<u32>) -> Self {
        let height = previous.height().min(current.height()) as usize;
        let columns = band.end.saturating_sub(band.start);
        let segments = columns.div_ceil(SEGMENT);
        let words = (segments as usize).div_ceil(64);
        let mut bits = vec![0u64; height * words];
        for (y, row) in bits.chunks_mut(words.max(1)).enumerate().take(height) {
            let (a, b) = (previous.row(y as u32), current.row(y as u32));
            for segment in 0..segments {
                let from = (band.start + segment * SEGMENT) as usize * 4;
                let to = (band.start + (segment + 1) * SEGMENT).min(band.end) as usize * 4;
                if a.get(from..to) != b.get(from..to) {
                    row[(segment / 64) as usize] |= 1 << (segment % 64);
                }
            }
        }
        Self { words, bits }
    }

    fn row(&self, y: u32) -> &[u64] {
        let from = (y as usize) * self.words;
        self.bits.get(from..from + self.words).unwrap_or(&[])
    }

    /// The runs of segments that changed both at row `y` and at row `there`, each as long
    /// as it goes.
    fn both(&self, y: u32, there: u32) -> Runs<'_> {
        Runs::new(self.row(y), self.row(there))
    }
}

/// The runs of bits set in both of two rows of bits, each joined across the words it spans.
struct Runs<'a> {
    a: &'a [u64],
    b: &'a [u64],
    word: usize,
    bits: u64,
}

impl<'a> Runs<'a> {
    fn new(a: &'a [u64], b: &'a [u64]) -> Self {
        let mut runs = Self { a, b, word: 0, bits: 0 };
        runs.bits = runs.load(0);
        runs
    }

    fn load(&self, word: usize) -> u64 {
        match (self.a.get(word), self.b.get(word)) {
            (Some(a), Some(b)) => a & b,
            _ => 0,
        }
    }
}

/// `bits` without the lowest `count` of them.
const fn above(bits: u64, count: u32) -> u64 {
    if count >= 64 { 0 } else { bits & (u64::MAX << count) }
}

impl Iterator for Runs<'_> {
    type Item = Range<usize>;

    fn next(&mut self) -> Option<Range<usize>> {
        let words = self.a.len().min(self.b.len());
        while self.bits == 0 {
            self.word += 1;
            if self.word >= words {
                return None;
            }
            self.bits = self.load(self.word);
        }
        let first = self.bits.trailing_zeros();
        let start = self.word * 64 + first as usize;
        let ones = (!(self.bits >> first)).trailing_zeros();
        if first + ones < 64 {
            self.bits = above(self.bits, first + ones);
            return Some(start..self.word * 64 + (first + ones) as usize);
        }
        // To the top of this word, and on into the next for as long as they are set.
        loop {
            self.word += 1;
            if self.word >= words {
                self.bits = 0;
                return Some(start..self.word * 64);
            }
            let bits = self.load(self.word);
            let ones = (!bits).trailing_zeros();
            if ones < 64 {
                self.bits = above(bits, ones);
                return Some(start..self.word * 64 + ones as usize);
            }
        }
    }
}

impl Fixed {
    /// The sticky runs between two frames: the rows that are the same at the same `y`.
    ///
    /// Only the runs that reach an edge count. A blank gap in the middle of a page is
    /// identical in both frames too, and treating it as fixed would drop the content that
    /// scrolled through it; a sticky header is by construction against the top and a
    /// sticky footer against the bottom.
    #[must_use]
    pub fn between(previous: &Frame, previous_rows: &Signatures, current: &Frame, current_rows: &Signatures) -> Self {
        let height = previous_rows.len().min(current_rows.len());
        if height == 0 {
            return Self::default();
        }
        let most = ((f64::from(height) * MOST) as u32).max(1);
        let band = current_rows.band();
        // Both, and the pixels are the half that means it. Two numbers a row cannot tell
        // one line of an evenly set table from the line two below it, so a page like that
        // scrolled by a whole number of line pitches reads as sticky from the edge inwards
        // -- and the mask is settled once and held for the rest of the capture. A row that
        // is really sticky is the *same row*, so that is what is asked.
        let same = |y: u32| match (previous_rows.get(y), current_rows.get(y)) {
            (Some(a), Some(b)) => {
                a.same(b) && fit(previous, current, &band, None, &(y..y + 1), 0, 1).whole <= STUCK
            }
            _ => false,
        };
        let header = (0..most).take_while(|y| same(*y)).count();
        let footer = (0..most).take_while(|y| same(height - 1 - *y)).count();
        let candidate = Self {
            header: u32::try_from(header).unwrap_or(0),
            footer: u32::try_from(footer).unwrap_or(0),
        };
        // A mask you cannot check is a mask not to trust. Two frames of a blank margin
        // are identical top to bottom, and the runs that leaves say "sticky header" about
        // rows that are only empty -- which would then be frozen for the rest of the
        // capture, because the mask is settled once. If what is left between the runs has
        // no shape of its own, there is nothing sticky here: there is nothing at all.
        let body = candidate.body(height);
        if body.end <= body.start || !previous_rows.varies(body.clone()) || !current_rows.varies(body) {
            return Self::default();
        }
        candidate
    }

    /// The rows between the two sticky runs: `[start, end)`.
    #[must_use]
    pub const fn body(self, height: u32) -> std::ops::Range<u32> {
        let start = if self.header < height { self.header } else { height };
        let end = if self.footer < height - start { height - self.footer } else { start };
        start..end
    }
}

/// Where the page moved to, and how sure the matcher is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Match {
    /// The scroll distance in rows.
    pub offset: u32,
    /// The normalised cross-correlation at that distance, -1.0 to 1.0.
    pub score: f64,
}

impl Match {
    /// `spec/07` §1.2's accept rule, minus the `d > 0` half -- a zero offset is a real
    /// answer ("the page did not move") that the loop counts rather than rejects.
    ///
    /// The correlation alone, however well the pixels fit. Taking the pixels' word when
    /// they were close was tried (2026-09-23) and measured out: over an hour grid with a
    /// patch drawn on it they preferred two hours to the true three by 80 to 100, and a
    /// grid is exactly the page on which the correlation's threshold is what stands between
    /// the matcher and a wrong answer (D105).
    #[must_use]
    pub fn trusted(self) -> bool {
        self.score > ACCEPT
    }
}

/// How far `current` scrolled past `previous`, by the signatures alone.
///
/// `hint` is the step the caller asked the page to scroll. It decides nothing -- a page
/// that ignored the scroll, or scrolled twice as far, is found anyway -- and is used only
/// to break ties, which a blank stretch of page produces by the hundred: every offset
/// correlates perfectly with every other, and the only thing that distinguishes them is
/// what we asked for.
///
/// This is `spec/07` §1.2's step as written. [`place`] is what the stitcher calls, because
/// evenly set type ties too often for a tie-break to be a guess worth making.
#[must_use]
pub fn locate(
    previous: &Signatures,
    current: &Signatures,
    fixed: Fixed,
    hint: u32,
) -> Option<Match> {
    contenders(previous, current, fixed, hint).into_iter().next()
}

/// The largest offset a body this deep can be searched for.
///
/// [`MIN_OVERLAP`] of the body has to be left over for a score to mean anything, so this
/// is the ceiling on every answer below -- and the reason a caller might want it is that
/// an answer *at* the ceiling is usually the truth being outside it.
#[must_use]
pub fn reach(span: u32) -> u32 {
    span.saturating_sub(((f64::from(span) * MIN_OVERLAP) as u32).max(1))
}

/// Every offset the correlation cannot tell apart from its best, nearest the hint first.
///
/// [P]. A page of evenly set type -- a table, a code listing, a terminal, a list view --
/// correlates with itself at *every* line pitch, so the single best peak is one of a dozen
/// that score the same to three decimal places and the tie-break decides the capture. That
/// showed up the first time this ran against a real editor (2026-09-15): the matcher
/// answered with the step it had been given, every time, and the stitch repeated fourteen
/// lines at every seam while looking perfectly confident. The peaks are collected here and
/// [`place`] asks the pixels which one is real.
#[must_use]
pub fn contenders(
    previous: &Signatures,
    current: &Signatures,
    fixed: Fixed,
    hint: u32,
) -> Vec<Match> {
    tied(&peaks(previous, current, None, fixed), hint)
}

/// Where along a capture a frame's body could be, when it is nowhere near the frame before
/// it: the correlation of the body's rows with every stretch of the capture's as long as
/// it, each local maximum, strongest first -- `offset` is the row of the capture the
/// stretch starts at.
///
/// A person scrolling back to where they began gets there in a flick, and the frame that
/// lands shares nothing with the one before it and everything with what is captured
/// (2026-09-24). The signatures find the stretch; the pixels, asked by [`place`] about a
/// frame made of it, say whether the page is there.
#[must_use]
pub fn anywhere(capture: &[Row], body: &[Row]) -> Vec<Match> {
    let span = body.len();
    if span == 0 || capture.len() < span {
        return Vec::new();
    }
    let scores = spread(capture.len() - span + 1, |starts| {
        let mut series = Series::default();
        starts
            .map(|start| {
                series.clear();
                for (a, b) in body.iter().zip(&capture[start..start + span]) {
                    series.push((a.mean, a.energy), (b.mean, b.energy));
                }
                series.score(0)
            })
            .collect()
    });
    let mut maxima: Vec<Match> = scores
        .iter()
        .enumerate()
        .filter(|&(at, &score)| {
            scores.get(at.wrapping_sub(1)).is_none_or(|&before| score >= before)
                && scores.get(at + 1).is_none_or(|&after| score > after)
        })
        .map(|(at, &score)| Match { offset: at as u32, score })
        .collect();
    maxima.sort_by(|a, b| b.score.total_cmp(&a.score));
    maxima
}

/// Every peak of the correlation over the body, offset by offset: each local maximum, and
/// not merely the shoulder of the peak next door.
///
/// With the pair's [`Changed`] segments, every offset but zero also scores over only what
/// changed, and keeps the better of the two readings. The whole reading is the one that
/// knows a page with little on it that changed -- an hour grid moved by whole hours, whose
/// only changes are its appointments -- and the other is the one a banner cannot argue
/// with; either can find the offset, and the pixels judge whatever they propose.
fn peaks(
    previous: &Signatures,
    current: &Signatures,
    changed: Option<&Changed>,
    fixed: Fixed,
) -> Vec<Match> {
    let height = previous.len().min(current.len());
    let body = fixed.body(height);
    let span = body.end.saturating_sub(body.start);
    if span == 0 {
        return Vec::new();
    }
    let last = reach(span);
    // What changed leaves a score to take, but not one made of a handful of rows: half the
    // overlap a score is otherwise allowed, and never fewer than two.
    let least = (((f64::from(span) * MIN_OVERLAP) as u32) / 2).max(2);
    let scores = spread(last as usize + 1, |offsets| {
        let mut series = Series::default();
        offsets
            .map(|offset| {
                let offset = offset as u32;
                let overlap = span - offset;
                let whole = correlate(previous, current, body.start, offset, overlap, &mut series);
                match changed {
                    Some(changed) if offset > 0 => whole.max(correlate_changed(
                        (previous, current, changed),
                        (body.start, offset, overlap),
                        least,
                        &mut series,
                    )),
                    _ => whole,
                }
            })
            .collect()
    });
    scores
        .iter()
        .enumerate()
        .filter(|&(at, &score)| {
            scores.get(at.wrapping_sub(1)).is_none_or(|&before| score >= before)
                && scores.get(at + 1).is_none_or(|&after| score >= after)
        })
        .map(|(at, &score)| Match { offset: at as u32, score })
        .collect()
}

/// [`contenders`] from peaks already found.
fn tied(maxima: &[Match], hint: u32) -> Vec<Match> {
    let Some(best) = maxima.iter().map(|peak| peak.score).reduce(f64::max) else {
        return Vec::new();
    };
    let mut peaks: Vec<Match> =
        maxima.iter().copied().filter(|peak| peak.score + TIE >= best).collect();
    peaks.sort_by_key(|peak| peak.offset.abs_diff(hint));
    let mut kept: Vec<Match> = peaks.iter().copied().take(CONTENDERS).collect();
    // Two answers are never thinned away, however far they sit from the step. The
    // correlation's own best is one. Zero is the other, and it is the one that was lost:
    // a page that has stopped moving scores a perfect 1.0 at *every* offset, so "the
    // peaks nearest the step" is a window that does not contain the answer, and a capture
    // that had ended grew by another half-frame of content it had already shown
    // (2026-09-15).
    for peak in peaks.iter().copied().filter(|peak| peak.offset == 0 || peak.score >= best) {
        if !kept.iter().any(|seen| seen.offset == peak.offset) {
            kept.push(peak);
        }
    }
    kept.sort_by_key(|peak| peak.offset.abs_diff(hint));
    kept
}

/// Where the page moved to: the correlation proposes, the pixels dispose.
///
/// The signatures are two numbers a row and cannot tell one line of a table from the next;
/// the pixels can, and a coarse pass over every eighth row is enough to separate candidates
/// a whole line pitch apart. Only the winner is refined, which is what keeps this inside
/// `spec/10` §7's 120 ms: a coarse pass between two rows costs about a quarter of a full
/// one, and a frame of 1.8 Mpx was placed in 17 ms on average, 31 at worst (2026-09-24).
#[must_use]
pub fn place(
    previous: &Frame,
    previous_rows: &Signatures,
    current: &Frame,
    current_rows: &Signatures,
    fixed: Fixed,
    hint: u32,
) -> Option<Match> {
    fitted(previous, previous_rows, current, current_rows, fixed, hint).map(|(found, _)| found)
}

/// [`place`], and how far the overlap differs at its answer: the mean squared difference
/// per channel that was held to [`FIT`], over what changed.
///
/// For a caller with two answers to choose between -- the page placed going forwards and
/// going back ([`crate::Stitcher::both_ways`]) -- and no other way to say which of them the
/// pixels liked better. Nearer is not it: going back cannot say "one row on", and says
/// "nought" instead, which a one-row slip of sparse type fits well inside [`FIT`]
/// (2026-09-24).
#[must_use]
pub fn fitted(
    previous: &Frame,
    previous_rows: &Signatures,
    current: &Frame,
    current_rows: &Signatures,
    fixed: Fixed,
    hint: u32,
) -> Option<(Match, f64)> {
    let height = previous_rows.len().min(current_rows.len());
    let body = fixed.body(height);
    let band = current_rows.band();
    let changed = Changed::between(previous, current, band.clone());
    let maxima = peaks(previous_rows, current_rows, Some(&changed), fixed);
    let mut peaks = tied(&maxima, hint);
    // And the strongest peaks, tied or not. Anything in the band that does not scroll with
    // the page -- a toast, an animation, a scrollbar's thumb -- disagrees at the offset the
    // page really moved and nowhere in particular elsewhere, so it can cost the true peak
    // its tie with a spurious one and take it out of the running before the pixels, which
    // would have picked it, are ever asked (D105). They are asked about these as about the
    // rest, and the order below -- nearest the hint first, replaced only by something
    // strictly better -- is unchanged, so a strong peak that does not fit costs one coarse
    // comparison and nothing else.
    let mut strongest = maxima;
    strongest.sort_by(|a, b| b.score.total_cmp(&a.score));
    for peak in strongest.into_iter().take(CONTENDERS) {
        if !peaks.iter().any(|seen| seen.offset == peak.offset) {
            peaks.push(peak);
        }
    }
    // Only a peak the correlation vouches for can be the answer -- the stitcher takes no
    // other ([`Match::trusted`]) -- so no other is let near the pixels. Over a short overlap
    // the pixels see little, and what they see can be blank ground that fits exactly: a
    // peak at 352 rows of 396, scored 0.66, beat a nudge of under a row that scored 0.98,
    // and a scroll nobody could have made smaller was a miss (2026-09-24). Zero stays
    // whatever it scored, because it is judged on its own terms below.
    peaks.retain(|peak| peak.offset == 0 || peak.trusted());
    peaks.sort_by_key(|peak| peak.offset.abs_diff(hint));
    // "The page did not move" is the answer to beat, not one candidate among the rest.
    //
    // A page that has stopped is the *same picture* as the one before it, so every offset
    // a rule apart scores a perfect correlation **and** a zero difference -- the step we
    // asked for included -- and "nearest the step" then told a capture standing at the
    // foot of a page that it had scrolled. Google Calendar's day view found it
    // (2026-09-16, D94): an hour grid is a dark ground ruled every 58 px, three of those
    // rules came to the 175 rows the tie-break liked best, and the capture appended the
    // same three hours twenty times over without one of them ever being new.
    //
    // Zero cannot be beaten on a page that stopped -- its difference is exactly zero --
    // so seeding with it and keeping the strict `<` below is the whole fix: a candidate
    // has to be *better* than "nothing moved", not merely as good.
    //
    // Over a body with no shape at all the seed is dropped, because there the pixels are
    // not saying "nothing moved", they are saying nothing: a capture begun on a white
    // margin has two identical frames and no evidence either way, and the rows the step
    // asks for are the rows it would have got. The same reading as [`Fixed::between`]'s
    // own guard, and the same reason -- an answer you cannot check is not an answer.
    //
    // And "did not move" includes a page that moved less than half a row. A page the
    // compositor resampled, nudged by a touchpad, is no row of the frame before but a
    // blend of two, and judged at nought alone the nudge was a miss that told the user
    // their scroll was too big (2026-09-24). Nearer the next row than this one, it moved,
    // and a scroll has to beat nought exactly as before.
    let sure = previous_rows.varies(body.clone()) && current_rows.varies(body.clone());
    let mut chosen: Option<(Match, Fit)> =
        peaks.iter().copied().find(|peak| sure && peak.offset == 0).map(|peak| {
            let nudged = between(previous, current, &band, None, &body, 0, COARSE);
            let still = match nudged.after {
                false => nudged.fit,
                true => fit(previous, current, &band, None, &body, 0, COARSE),
            };
            (peak, still)
        });
    // Nearest the hint first, and only ever replaced by something strictly better, so a
    // stretch of page the pixels cannot separate either keeps the step we asked for. Every
    // offset but zero is judged by what changed ([`Changed`]); zero is judged whole, and
    // so has to be beaten by a scroll that explains what changed better than "nothing
    // moved" explains everything.
    //
    // Each from the row before it to the row after, and judged where it fits best in
    // that: a page the compositor resampled lands between rows ([`between`]), and judged
    // on the row alone the truth is judged half a row from where it is.
    let peaks: Vec<Match> = peaks.into_iter().filter(|peak| peak.offset != 0).collect();
    let tries: Vec<(usize, u32)> = peaks
        .iter()
        .enumerate()
        .flat_map(|(at, peak)| {
            [peak.offset - 1, peak.offset].into_iter().map(move |row| (at, row))
        })
        .collect();
    let fits = spread(tries.len(), |at| {
        at.map(|at| between(previous, current, &band, Some(&changed), &body, tries[at].1, COARSE))
            .map(|landing| landing.fit)
            .collect()
    });
    let least = fits.iter().map(|fit: &Fit| fit.whole).fold(f64::MAX, f64::min);
    let mut best: Vec<Option<Fit>> = vec![None; peaks.len()];
    for (&(at, _), error) in tries.iter().zip(fits) {
        if error.stray(least) {
            continue;
        }
        if best[at].is_none_or(|seen| error.better(seen)) {
            best[at] = Some(error);
        }
    }
    for (peak, error) in peaks.into_iter().zip(best) {
        let Some(error) = error else { continue };
        if chosen.as_ref().is_none_or(|&(_, seen)| error.better(seen)) {
            chosen = Some((peak, error));
        }
    }
    let (found, error) = chosen?;
    // The signatures said these rows line up; the pixels say whether they are the same
    // rows. A page of type lines up at every line pitch, so without this the answer is
    // only ever as good as the range it was allowed to search. A scroll is judged where
    // it settled, between rows if that is where it went, and not where the correlation
    // put it: a page resampled between rows fits no row well.
    if found.offset == 0 {
        return (error.changed <= FIT).then_some((found, error.changed));
    }
    let (offset, error) = settle(previous, current, &band, &changed, &body, found.offset);
    (error.changed <= FIT).then_some((Match { offset, ..found }, error.changed))
}

/// `spec/07` §1.2's "+-2 px with full-pixel SSD", over the same band the signatures used
/// and over what changed ([`Changed`]).
///
/// The signatures are two numbers a row, which is enough to find the line of text but not
/// enough to tell it from the line above at sub-row accuracy -- a page of 16 px type
/// correlates nearly as well one row out as it does on the nose. The pixels settle it, and
/// only five candidates ever reach them. Never zero for a scroll of a row or more: "the
/// page did not move" is [`place`]'s to answer, compared whole, and not a neighbour of the
/// scroll it found.
///
/// To the fraction of a row, for a page the compositor resampled ([`between`]) -- and
/// back to the row nearest where that landed, which for a scroll of under half a row is
/// nought.
#[must_use]
pub fn refine(
    previous: &Frame,
    current: &Frame,
    band: &Range<u32>,
    changed: &Changed,
    body: Range<u32>,
    offset: u32,
) -> u32 {
    settle(previous, current, band, changed, &body, offset).0
}

/// [`refine`], and how well the overlap fits where it settled.
///
/// The fit is where the page went, between rows if that is where it went; the answer is
/// the row nearest it. The fit is what [`FIT`] judges, and a page resampled between rows
/// fits no row well. The answer is what the canvas is cut by, and the half row a seam
/// rounds to is less than the compositor already blurred.
fn settle(
    previous: &Frame,
    current: &Frame,
    band: &Range<u32>,
    changed: &Changed,
    body: &Range<u32>,
    offset: u32,
) -> (u32, Fit) {
    let span = body.end.saturating_sub(body.start);
    let first = offset.saturating_sub(REFINE);
    let last = (offset + REFINE).min(span.saturating_sub(1));
    // Each row to the next, over the rows either side of the correlation's answer. Seeded
    // with its own and only ever replaced by something strictly better, so a stretch of
    // page where every candidate scores the same -- a blank margin, a solid rule -- keeps
    // the offset the correlation already agreed on rather than sliding to whichever
    // candidate happened to be tried first.
    let rows: Vec<u32> =
        std::iter::once(offset).chain((first..last).filter(|&row| row != offset)).collect();
    let fits = spread(rows.len(), |at| {
        at.map(|at| between(previous, current, band, Some(changed), body, rows[at], 1)).collect()
    });
    let least = fits.iter().map(|landing: &Landing| landing.fit.whole).fold(f64::MAX, f64::min);
    let mut tried = rows.into_iter().zip(fits).filter(|(_, landing)| !landing.fit.stray(least));
    let Some(mut best) = tried.next() else {
        return (offset, Fit { whole: f64::MAX, changed: f64::MAX });
    };
    for (row, landing) in tried {
        if landing.fit.better(best.1.fit) {
            best = (row, landing);
        }
    }
    let (row, landing) = best;
    (row + u32::from(landing.after), landing.fit)
}

/// How well the overlap agrees at one candidate offset, as mean squared difference per
/// channel: over everything, and over what changed at both places of each pair of rows
/// ([`Changed`]).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fit {
    whole: f64,
    /// The whole again where nothing changed at both places of any pair: no evidence
    /// either way, and the whole is what is left to judge by.
    changed: f64,
}

impl Fit {
    /// Far worse over everything than `least`, the best any candidate beside it managed
    /// there: not the scroll, however well what changed lines up ([`WORSE`]).
    fn stray(self, least: f64) -> bool {
        self.whole > FIT && self.whole > WORSE * least
    }

    /// Better over what changed, or as good there and better over everything.
    ///
    /// What changed first, because that is what a banner cannot spoil. Everything as the
    /// tie-break, because what changed can be too little to decide by: an hour grid moved
    /// by whole hours changes only where its appointments differ, and one row out it fits
    /// the insides of those as perfectly as it does on the nose -- while its rules, which
    /// are the same at their own place in both frames and so left out, line up at one of
    /// the two and not the other. A tie is exact: two fits that differ at all are not one.
    fn better(self, other: Self) -> bool {
        self.changed < other.changed || (self.changed == other.changed && self.whole < other.whole)
    }
}

/// [`Fit`] at one candidate offset.
///
/// `stride` is how many rows one sample stands for: one for the refinement, [`COARSE`]
/// when the question is only which of several candidates is the right neighbourhood.
/// Without `changed`, both readings are the whole one.
fn fit(
    previous: &Frame,
    current: &Frame,
    band: &Range<u32>,
    changed: Option<&Changed>,
    body: &Range<u32>,
    offset: u32,
    stride: u32,
) -> Fit {
    let segments = band.end.saturating_sub(band.start).div_ceil(SEGMENT);
    let set = |bits: &[u64], segment: u32| {
        bits.get((segment / 64) as usize).is_some_and(|word| word >> (segment % 64) & 1 == 1)
    };
    let (mut whole, mut all) = (0u64, 0usize);
    let (mut kept, mut some) = (0u64, 0usize);
    for y in (body.start..body.end.saturating_sub(offset)).step_by(stride.max(1) as usize) {
        let (here, there) = (current.row(y), previous.row(y + offset));
        let both = changed.map(|changed| (changed.row(y), changed.row(y + offset)));
        for segment in 0..segments {
            let from = band.start + segment * SEGMENT;
            let to = (from + SEGMENT).min(band.end);
            let span = (from as usize) * 4..(to as usize) * 4;
            let (Some(a), Some(b)) = (here.get(span.clone()), there.get(span)) else {
                continue;
            };
            let mut sum = 0u64;
            for (a, b) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
                for channel in 0..3 {
                    let d = i32::from(a[channel]) - i32::from(b[channel]);
                    sum += u64::from(d.unsigned_abs() * d.unsigned_abs());
                }
            }
            let count = 3 * (to - from) as usize;
            whole += sum;
            all += count;
            if both.is_some_and(|(mine, theirs)| set(mine, segment) && set(theirs, segment)) {
                kept += sum;
                some += count;
            }
        }
    }
    let whole = if all == 0 { f64::MAX } else { whole as f64 / (all as f64) };
    let changed = if some == 0 { whole } else { kept as f64 / (some as f64) };
    Fit { whole, changed }
}

/// Where between row `offset` of `previous` and the row after it the overlap fits best, and
/// how well it fits there.
///
/// A page the compositor resamples does not scroll by whole rows of the screen. A window
/// drawn at twice the scale and shown at 1.25 -- what a toolkit or an Electron app without
/// fractional scaling gets -- moves in rows of its own that are five eighths of ours, and
/// every row of the frame is then a blend of the two rows of the frame before that it
/// falls between. Measured at the nearest whole row, the right answer differed by 500 to
/// 1 050 over what changed, past [`FIT`], and a real capture of a chat window stitched
/// seven frames of 214 and answered every scroll after them with "that scroll was too big"
/// (2026-09-24).
///
/// The blend is linear in how far it goes, so the difference squared is a parabola in it,
/// and where that is lowest comes out of three sums in one pass ([`Sums`]) rather than out
/// of trying fractions: a quarter of a row is still an eighth from the truth at worst, and
/// type sharp enough is a thousand out at an eighth. Both readings are taken where what
/// changed fits best.
fn between(
    previous: &Frame,
    current: &Frame,
    band: &Range<u32>,
    changed: Option<&Changed>,
    body: &Range<u32>,
    offset: u32,
    stride: u32,
) -> Landing {
    let segments = band.end.saturating_sub(band.start).div_ceil(SEGMENT);
    let set = |bits: &[u64], segment: u32| {
        bits.get((segment / 64) as usize).is_some_and(|word| word >> (segment % 64) & 1 == 1)
    };
    let (mut whole, mut kept) = (Sums::default(), Sums::default());
    for y in (body.start..body.end.saturating_sub(offset + 1)).step_by(stride.max(1) as usize) {
        let (here, there) = (current.row(y), previous.row(y + offset));
        let next = previous.row(y + offset + 1);
        let both = changed.map(|changed| (changed.row(y), changed.row(y + offset)));
        for segment in 0..segments {
            let from = band.start + segment * SEGMENT;
            let to = (from + SEGMENT).min(band.end);
            let span = (from as usize) * 4..(to as usize) * 4;
            let (Some(a), Some(b), Some(c)) =
                (here.get(span.clone()), there.get(span.clone()), next.get(span))
            else {
                continue;
            };
            let sums = Sums::of(a, b, c);
            whole.add(sums);
            if both.is_some_and(|(mine, theirs)| set(mine, segment) && set(theirs, segment)) {
                kept.add(sums);
            }
        }
    }
    let judged = if kept.count == 0 { whole } else { kept };
    let part = judged.lowest();
    let fit = Fit {
        whole: whole.at(part),
        changed: if kept.count == 0 { whole.at(part) } else { kept.at(part) },
    };
    Landing { fit, after: whole.at(1.0) < whole.at(0.0) }
}

/// Where a scroll landed between two rows: how well the overlap fits there, and whether
/// the second row is the nearer.
///
/// Nearer by the pixels rather than by the fraction. Two resampled frames are blurred
/// alike but not in step, so where the parabola is lowest can be a third of a row from
/// where the page went on type as sharp as it comes; the row that fits better is the one
/// to cut the canvas at either way. Better over everything, because what changed at both
/// places is read for the first row's pairs and would judge the second on the wrong ones.
#[derive(Debug, Clone, Copy)]
struct Landing {
    fit: Fit,
    after: bool,
}

/// A run of `current` against a blend of two runs of `previous`, as a parabola in how far
/// the blend goes. With `u` the difference from the first run and `v` the step from the
/// first to the second, the difference squared at `t` of the way is `u² - 2tuv + t²v²`.
#[derive(Debug, Clone, Copy, Default)]
struct Sums {
    uu: u64,
    uv: i64,
    vv: u64,
    count: usize,
}

impl Sums {
    fn of(a: &[u8], b: &[u8], c: &[u8]) -> Self {
        let mut sums = Self::default();
        let pixels = b.as_chunks::<4>().0.iter().zip(c.as_chunks::<4>().0);
        for (a, (b, c)) in a.as_chunks::<4>().0.iter().zip(pixels) {
            for channel in 0..3 {
                let u = i32::from(a[channel]) - i32::from(b[channel]);
                let v = i32::from(c[channel]) - i32::from(b[channel]);
                sums.uu += u64::from(u.unsigned_abs() * u.unsigned_abs());
                sums.uv += i64::from(u * v);
                sums.vv += u64::from(v.unsigned_abs() * v.unsigned_abs());
            }
            sums.count += 3;
        }
        sums
    }

    fn add(&mut self, other: Self) {
        self.uu += other.uu;
        self.uv += other.uv;
        self.vv += other.vv;
        self.count += other.count;
    }

    /// How far from the first run to the second the difference is least, nought to one.
    fn lowest(self) -> f64 {
        if self.vv == 0 { 0.0 } else { (self.uv as f64 / self.vv as f64).clamp(0.0, 1.0) }
    }

    /// The mean squared difference `part` of the way, or the most there is over nothing.
    fn at(self, part: f64) -> f64 {
        if self.count == 0 {
            return f64::MAX;
        }
        let (uu, uv, vv) = (self.uu as f64, self.uv as f64, self.vv as f64);
        ((uu - 2.0 * part * uv + part * part * vv) / self.count as f64).max(0.0)
    }
}

/// A pair of series of rows, both channels: the vectors a score is made of, kept between
/// offsets so that twelve hundred of them are not twelve hundred allocations.
#[derive(Default)]
struct Series {
    mine: (Vec<f64>, Vec<f64>),
    theirs: (Vec<f64>, Vec<f64>),
}

impl Series {
    fn clear(&mut self) {
        self.mine.0.clear();
        self.mine.1.clear();
        self.theirs.0.clear();
        self.theirs.1.clear();
    }

    fn push(&mut self, mine: (f32, f32), theirs: (f32, f32)) {
        self.mine.0.push(f64::from(mine.0));
        self.mine.1.push(f64::from(mine.1));
        self.theirs.0.push(f64::from(theirs.0));
        self.theirs.1.push(f64::from(theirs.1));
    }

    /// The mean of the two channels' correlations, or -1 -- which never wins -- for a
    /// score made of fewer than `least` pairs.
    ///
    /// Both, rather than one combined number, because they answer different questions and
    /// a row that matches on brightness but not on where its edges are is not the same
    /// row.
    fn score(&self, least: u32) -> f64 {
        if self.mine.0.len() < least as usize {
            return -1.0;
        }
        (ncc(&self.mine.0, &self.theirs.0) + ncc(&self.mine.1, &self.theirs.1)) / 2.0
    }
}

/// How well `overlap` rows of `current` from `from` agree with the rows of `previous`
/// `offset` further down, read whole.
fn correlate(
    previous: &Signatures,
    current: &Signatures,
    from: u32,
    offset: u32,
    overlap: u32,
    series: &mut Series,
) -> f64 {
    series.clear();
    let blank = crate::signature::Row { mean: 0.0, energy: 0.0 };
    for y in 0..overlap {
        let a = current.get(from + y).unwrap_or(blank);
        let b = previous.get(from + offset + y).unwrap_or(blank);
        series.push((a.mean, a.energy), (b.mean, b.energy));
    }
    series.score(0)
}

/// [`correlate`] over what changed: each pair of rows read over the segments in which the
/// frames differ at both of its places ([`Changed`]), and a pair with none of those left
/// out. `(from, offset, overlap)` as there.
fn correlate_changed(
    (previous, current, changed): (&Signatures, &Signatures, &Changed),
    (from, offset, overlap): (u32, u32, u32),
    least: u32,
    series: &mut Series,
) -> f64 {
    series.clear();
    for y in 0..overlap {
        let (here, there) = (from + y, from + offset + y);
        let mut width = 0u32;
        let (mut mine, mut theirs) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32));
        for run in changed.both(here, there) {
            let (a, b) = (current.read(here, &run), previous.read(there, &run));
            width += current.width(&run);
            mine = (mine.0 + a.0, mine.1 + a.1);
            theirs = (theirs.0 + b.0, theirs.1 + b.1);
        }
        if width == 0 {
            continue;
        }
        let n = width as f32;
        series.push((mine.0 / n, mine.1 / n), (theirs.0 / n, theirs.1 / n));
    }
    series.score(least)
}

/// `each` over `0..count`, split between as many threads as the machine has cores, and
/// the answers back in order.
///
/// Every offset's score, and every candidate's fit, is independent of the next, and a
/// full-screen frame has twelve hundred of the one: on a single core the matcher took a
/// hundred milliseconds a frame, which in manual mode is frames the user scrolled through
/// and nobody looked at. The items are dealt round-robin rather than in blocks, because
/// the small offsets share the most rows and cost the most.
fn spread<T: Send>(
    count: usize,
    each: impl Fn(std::iter::StepBy<Range<usize>>) -> Vec<T> + Sync,
) -> Vec<T> {
    let threads = std::thread::available_parallelism().map_or(1, usize::from).min(count);
    if threads <= 1 {
        return each((0..count).step_by(1));
    }
    let dealt: Vec<Vec<T>> = std::thread::scope(|scope| {
        let each = &each;
        let hands: Vec<_> = (0..threads)
            .map(|first| scope.spawn(move || each((first..count).step_by(threads))))
            .collect();
        hands
            .into_iter()
            .map(|hand| hand.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)))
            .collect()
    });
    let mut hands: Vec<std::vec::IntoIter<T>> = dealt.into_iter().map(Vec::into_iter).collect();
    (0..count).filter_map(|at| hands[at % threads].next()).collect()
}

/// Normalised cross-correlation of two equal-length series.
///
/// A series with no variance -- a blank stretch of page, where every row is the same
/// white -- has no shape to correlate, so the answer comes from whether the two are the
/// same flat value. Without that case a blank page scores `0/0` and every offset is as
/// bad as every other, which is how a scroll through a long empty margin stops dead.
fn ncc(a: &[f64], b: &[f64]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let n = a.len() as f64;
    let mean_a = a.iter().sum::<f64>() / n;
    let mean_b = b.iter().sum::<f64>() / n;
    let mut top = 0.0;
    let mut var_a = 0.0;
    let mut var_b = 0.0;
    for (x, y) in a.iter().zip(b) {
        let (dx, dy) = (x - mean_a, y - mean_b);
        top += dx * dy;
        var_a += dx * dx;
        var_b += dy * dy;
    }
    match (var_a < FLAT, var_b < FLAT) {
        (true, true) => {
            if (mean_a - mean_b).abs() < FLAT.sqrt() { 1.0 } else { 0.0 }
        }
        (true, false) | (false, true) => 0.0,
        (false, false) => top / (var_a * var_b).sqrt(),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testing::{looking, window, with_sidebar, Look, RULE};

    /// The runs of segments that changed at both places: joined across the words they
    /// span, split where either place stood still, and none where nothing changed.
    #[test]
    fn what_changed_at_both_places_is_read_in_runs() {
        let runs = |a: &[u64], b: &[u64]| Runs::new(a, b).collect::<Vec<_>>();
        assert_eq!(runs(&[0b1110_0110], &[u64::MAX]), vec![1..3, 5..8]);
        assert_eq!(runs(&[0b1110_0110], &[0b0100_0110]), vec![1..3, 6..7]);
        assert_eq!(runs(&[u64::MAX << 60, 0b111], &[u64::MAX, u64::MAX]), vec![60..67]);
        assert_eq!(runs(&[u64::MAX, u64::MAX], &[u64::MAX, u64::MAX]), vec![0..128]);
        assert_eq!(runs(&[1 << 63, 0], &[u64::MAX, u64::MAX]), vec![63..64]);
        assert_eq!(runs(&[0, 1], &[0, 1]), vec![64..65]);
        assert!(runs(&[0, 0], &[u64::MAX, u64::MAX]).is_empty());
        assert!(runs(&[], &[]).is_empty());
    }

    /// D107: whatever stands still over the page while it scrolls is left out of the
    /// comparison, whatever it looks like -- a notification with text in it, a flat dark
    /// panel with none (the pill's preview), a blank white card, a toolbar the height of
    /// the page -- and all of them at once, as the first full-screen capture had them.
    #[test]
    fn what_stood_still_over_the_scroll_is_left_out_whatever_it_looks_like() {
        use crate::testing::{with_banner, with_panel, with_toolbar};
        let band = crate::signature::band(480);
        let crowd: [(&str, &dyn Fn(Frame) -> Frame); 5] = [
            ("a banner", &|f| with_banner(&f, 150, 30, 180, 60)),
            ("a dark panel", &|f| with_panel(&f, 330, 0, 140, 200, 30)),
            ("a blank card", &|f| with_panel(&f, 10, 480, 120, 150, 250)),
            ("a toolbar", &|f| with_toolbar(&f, 200, 40, 64, 600)),
            ("all of them", &|f| {
                let f = with_banner(&f, 150, 30, 180, 60);
                let f = with_panel(&f, 330, 0, 140, 200, 30);
                with_panel(&f, 10, 480, 120, 150, 250)
            }),
        ];
        for (what, over) in crowd {
            for shift in [24u32, 90, 200, 330] {
                let a = over(looking(Look::Flush, 480, 700, 0, 11));
                let b = over(looking(Look::Flush, 480, 700, shift, 11));
                let columns = moving(&a, &b, band.clone()).expect("the page moved");
                let sa = Signatures::over(&a, columns.clone());
                let sb = Signatures::over(&b, columns);
                let fixed = Fixed::between(&a, &sa, &b, &sb);
                let found = place(&a, &sa, &b, &sb, fixed, shift).filter(|found| found.trusted());
                assert_eq!(found.map(|found| found.offset), Some(shift), "{what}, shift {shift}");
                // And with no idea how far it went.
                let found = place(&a, &sa, &b, &sb, fixed, 0).filter(|found| found.trusted());
                let case = format!("{what}, shift {shift}, no hint");
                assert_eq!(found.map(|found| found.offset), Some(shift), "{case}");
            }
        }
    }

    /// The columns that stood still are cut off the ends of the band, and a pair with
    /// nothing moving has no columns at all.
    #[test]
    fn the_columns_that_stood_still_are_trimmed_from_the_ends() {
        let a = with_sidebar(&looking(Look::Prose, 400, 300, 0, 5), 80, 5);
        let b = with_sidebar(&looking(Look::Prose, 400, 300, 60, 5), 80, 5);
        let band = crate::signature::band(400);
        let columns = moving(&a, &b, band.clone()).expect("the page moved");
        assert_eq!(columns.start, 80, "the sidebar is trimmed");
        assert!(columns.end > 350, "{columns:?}: the page is kept");
        assert_eq!(moving(&a, &a, band), None, "nothing moved");
    }

    #[test]
    fn a_scrolled_page_is_found_at_the_distance_it_scrolled() {
        let first = window(64, 200, 0, 0);
        for step in [1u32, 17, 60, 120] {
            let second = window(64, 200, step, 0);
            let (a, b) = (Signatures::of(&first), Signatures::of(&second));
            let found = locate(&a, &b, Fixed::default(), step).expect("a body to match over");
            assert!(found.trusted(), "step {step} scored {}", found.score);
            assert_eq!(found.offset, step, "step {step}");
        }
    }

    #[test]
    fn the_offset_is_the_distance_the_content_moved_up() {
        // The reading this whole module rests on: content at row y of the first frame is
        // at row y - d of the second.
        let first = window(64, 120, 0, 0);
        let second = window(64, 120, 30, 0);
        assert_eq!(second.row(10), first.row(40));
        let found = locate(&Signatures::of(&first), &Signatures::of(&second), Fixed::default(), 30)
            .expect("a match");
        assert_eq!(found.offset, 30);
    }

    #[test]
    fn a_page_that_did_not_move_scores_a_zero_offset_rather_than_a_guess() {
        let first = window(64, 120, 0, 0);
        let found = locate(&Signatures::of(&first), &Signatures::of(&first), Fixed::default(), 72)
            .expect("a match");
        assert_eq!(found.offset, 0);
        assert!(found.trusted());
    }

    #[test]
    fn an_unrelated_frame_is_not_trusted() {
        let first = window(64, 200, 0, 0);
        let other = window(64, 200, 0, 777);
        let found = locate(&Signatures::of(&first), &Signatures::of(&other), Fixed::default(), 120)
            .expect("a match");
        assert!(!found.trusted(), "scored {}", found.score);
    }

    #[test]
    fn a_sticky_header_is_found_and_kept_out_of_the_match() {
        let first = window(64, 200, 0, 0);
        let second = window(64, 200, 50, 0);
        // Paste the first 24 rows of the first frame back over the second: a header that
        // does not scroll with the page.
        let mut pixels = second.pixels().to_vec();
        let stride = 64 * 4;
        pixels[..24 * stride].copy_from_slice(&first.pixels()[..24 * stride]);
        let second = Frame::new(64, 200, pixels).expect("a frame");
        let (a, b) = (Signatures::of(&first), Signatures::of(&second));
        let fixed = Fixed::between(&first, &a, &second, &b);
        assert_eq!(fixed.header, 24);
        assert_eq!(fixed.footer, 0);
        let found = locate(&a, &b, fixed, 50).expect("a body to match over");
        assert!(found.trusted(), "scored {}", found.score);
        assert_eq!(found.offset, 50);
    }

    #[test]
    fn a_sticky_footer_is_found_at_the_bottom() {
        let first = window(64, 200, 0, 0);
        let second = window(64, 200, 40, 0);
        let mut pixels = second.pixels().to_vec();
        let stride = 64 * 4;
        pixels[(200 - 18) * stride..].copy_from_slice(&first.pixels()[(200 - 18) * stride..]);
        let second = Frame::new(64, 200, pixels).expect("a frame");
        let fixed = Fixed::between(&first, &Signatures::of(&first), &second, &Signatures::of(&second));
        assert_eq!(fixed.footer, 18);
        assert_eq!(fixed.header, 0);
    }

    #[test]
    fn two_identical_frames_do_not_report_the_whole_page_as_sticky() {
        let first = window(64, 200, 0, 0);
        let fixed = Fixed::between(&first, &Signatures::of(&first), &first, &Signatures::of(&first));
        assert_eq!(fixed.header, 80);
        assert_eq!(fixed.footer, 80);
        assert_eq!(fixed.body(200), 80..120);
    }

    /// A page that has stopped is a perfect 1.0 at every offset, so the peaks nearest the
    /// step are all of them -- and the answer, zero, is the one the window leaves out.
    #[test]
    fn a_page_that_did_not_move_is_placed_at_zero_whatever_step_was_asked_for() {
        let first = window(64, 200, 0, 0);
        let rows = Signatures::of(&first);
        assert!(
            contenders(&rows, &rows, Fixed::default(), 120).iter().any(|peak| peak.offset == 0),
            "zero has to survive the thinning",
        );
        let found = place(&first, &rows, &first, &rows, Fixed::default(), 120).expect("a match");
        assert_eq!(found.offset, 0);
    }

    /// The listing again: scrolled by a whole number of line pitches, every row's two
    /// numbers match the row at the same `y` and the signatures alone call the edges
    /// sticky. Thirty rows of a page frozen for the rest of a capture, from a page that
    /// has no sticky anything.
    #[test]
    fn evenly_set_type_is_not_a_sticky_run_however_well_its_numbers_line_up() {
        // 310 wide, because the listing ties only over a band holding a whole number of
        // its 48 px patterns, and 310's band (6..294) holds six.
        let first = looking(Look::Listing, 310, 200, 0, 6);
        let second = looking(Look::Listing, 310, 200, 40, 6);
        let (a, b) = (Signatures::of(&first), Signatures::of(&second));
        assert!(a.get(150).is_some_and(|row| b.get(150).is_some_and(|other| row.same(other))));
        let fixed = Fixed::between(&first, &a, &second, &b);
        assert_eq!(fixed.header, 0, "{fixed:?}");
        // Rows 180..191 are the last line of type in the frame, and rows 191..200 the
        // blank leading under it. The blank rows really are the same rows in both frames
        // and calling them fixed costs nothing; the line of type above them is content,
        // and the signatures alone handed over thirty rows -- reaching into it.
        assert!(fixed.body(200).end > 190, "{fixed:?} reaches into a line of type");
    }

    /// D94, as the page that found it. An hour grid is a dark ground ruled at a fixed
    /// pitch: shift it by a whole number of rules and it is *the same pixels*, so the
    /// correlation says 1.0 and the difference says 0.0 at every one of them -- and at
    /// zero, which is the truth. Whichever peak sat nearest the step used to win.
    #[test]
    fn a_ruled_page_that_stopped_is_placed_at_zero_and_not_at_the_step() {
        let frame = looking(Look::Ruled, 240, 400, 0, 0);
        let rows = Signatures::of(&frame);
        let peaks = contenders(&rows, &rows, Fixed::default(), 3 * RULE);
        assert!(peaks.len() > 2, "an hour grid should tie at every rule: {peaks:?}");
        assert!(peaks.iter().all(|peak| peak.offset.is_multiple_of(RULE)), "{peaks:?}");
        // The signatures alone still answer with whatever the caller asked for.
        let told = locate(&rows, &rows, Fixed::default(), 3 * RULE);
        assert_eq!(told.map(|found| found.offset), Some(3 * RULE));
        // The pixels cannot separate them either -- and that is precisely why zero wins.
        for hint in [0, RULE, 3 * RULE, 10 * RULE, 581] {
            let found = place(&frame, &rows, &frame, &rows, Fixed::default(), hint);
            assert_eq!(found.map(|m| m.offset), Some(0), "hint {hint}");
        }
    }

    /// The other half of D94: zero is the answer to beat only where there is something to
    /// be sure about. A capture begun on a white margin has two identical frames and no
    /// evidence either way, and the rows the step asks for are the rows it would have
    /// got -- so there the step keeps its word, exactly as it did before.
    #[test]
    fn a_body_with_no_shape_in_it_still_takes_the_step_at_its_word() {
        let blank = Frame::filled(64, 200, [252, 252, 252, 255]);
        let rows = Signatures::of(&blank);
        assert!(!rows.varies(0..200), "a white margin has no shape to be sure about");
        let found = place(&blank, &rows, &blank, &rows, Fixed::default(), 120).expect("a match");
        assert_eq!(found.offset, 120);
    }

    #[test]
    fn the_refine_keeps_a_correct_offset_and_pulls_a_near_one_back() {
        let first = window(64, 200, 0, 0);
        let second = window(64, 200, 44, 0);
        let band = Signatures::of(&first).band();
        let changed = Changed::between(&first, &second, band.clone());
        assert_eq!(refine(&first, &second, &band, &changed, 0..200, 44), 44);
        assert_eq!(refine(&first, &second, &band, &changed, 0..200, 45), 44);
        assert_eq!(refine(&first, &second, &band, &changed, 0..200, 42), 44);
    }

    #[test]
    fn a_blank_page_answers_with_the_step_it_was_asked_for() {
        let blank = Frame::filled(64, 200, [255, 255, 255, 255]);
        let sig = Signatures::of(&blank);
        let found = locate(&sig, &sig, Fixed::default(), 90).expect("a match");
        // Every offset correlates perfectly; the hint is the only thing that separates
        // them, and the loop's own `d == 0` counter is what ends a blank page.
        assert!(found.trusted());
        assert_eq!(found.offset, 90);
    }

    /// The defect a real editor found on 2026-09-15, stated as a test: on a listing whose
    /// every line weighs the same, the signatures tie at every line pitch and `locate`
    /// answers with whichever peak is nearest the step it was told. `place` asks the
    /// pixels and gets the page's own answer.
    #[test]
    fn the_pixels_break_a_tie_the_signatures_cannot() {
        let (first, second) = (looking(Look::Listing, 240, 200, 0, 6), looking(Look::Listing, 240, 200, 40, 6));
        let (a, b) = (Signatures::of(&first), Signatures::of(&second));
        // Five peaks a line pitch apart, all of them the same score to three decimals.
        let peaks = contenders(&a, &b, Fixed::default(), 120);
        assert!(peaks.len() > 1, "a page this even should tie: {peaks:?}");
        assert!(peaks.iter().all(|peak| peak.offset.is_multiple_of(20)), "{peaks:?}");
        // Told the page moved 120 when it moved 40: the signatures believe the step.
        assert_eq!(locate(&a, &b, Fixed::default(), 120).map(|found| found.offset), Some(120));
        assert_eq!(place(&first, &a, &second, &b, Fixed::default(), 120).map(|found| found.offset), Some(40));
    }

    /// A page the compositor resampled scrolls by fractions of a row, and every row of type
    /// in the frame is then a blend of two rows of the frame before. Measured at the nearest
    /// whole row the right answer differed by more than [`FIT`], and a real chat window
    /// stitched seven frames of 214 (2026-09-24). Placed between its rows it is found, at
    /// the row nearest where it went -- over type set light, dark and flush left, and with
    /// or without a step to go by.
    #[test]
    fn a_page_the_compositor_resampled_is_placed_between_its_rows() {
        use crate::testing::resampled;
        let band = crate::signature::band(480);
        for look in [Look::Prose, Look::Terminal, Look::Flush] {
            // In eighths of a row: where a window drawn at 2 and shown at 1.25 lands.
            for shift in [13u32, 46, 101, 211, 333, 811, 1_203] {
                let a = resampled(look, 480, 400, 3, 21);
                let b = resampled(look, 480, 400, 3 + shift, 21);
                let columns = moving(&a, &b, band.clone()).expect("the page moved");
                let sa = Signatures::over(&a, columns.clone());
                let sb = Signatures::over(&b, columns);
                let fixed = Fixed::between(&a, &sa, &b, &sb);
                for hint in [0, shift / 8] {
                    let found = place(&a, &sa, &b, &sb, fixed, hint);
                    let case = format!("{look:?}, {shift} eighths, hint {hint}: {found:?}");
                    let found = found.filter(|found| found.trusted()).expect(&case);
                    assert!((found.offset * 8).abs_diff(shift) <= 4, "{case}");
                }
            }
        }
    }
}
