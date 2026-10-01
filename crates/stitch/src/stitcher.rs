// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/07` §1.2's loop, minus the part that scrolls.
//!
//! The stitcher is handed frames and answers with what to do next. It never scrolls,
//! never waits, never grabs: those are the extension's, and keeping them out is what lets
//! the whole algorithm -- the retry, the two ways a capture ends, the sticky header drawn
//! once -- be a unit test over a synthetic page instead of a thing you confirm by
//! scrolling a README and squinting at the seams.
//!
//! The loop the caller runs:
//!
//! ```text
//! let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
//! stitcher.push(grab()?, step)?;              // F0: always Next::Scroll
//! loop {
//!     scroll(step); settle();
//!     match stitcher.push(grab()?, step)? {
//!         Next::Scroll  => continue,
//!         Next::Smaller => { step = step / 2; continue }
//!         Next::Stop(_) => break,
//!     }
//! }
//! let result = stitcher.finish()?;
//! ```

use std::ops::Range;

use crate::align::{self, Fixed};
use crate::frame::Frame;
use crate::signature::{Row, Signatures};
use crate::StitchError;

/// `spec/07` §1.2: "if 3 failures -> stop (end reached)".
const MISSES: u8 = 3;

/// `spec/07` §1.2: "if d == 0 for 2 iterations -> end of content".
const REPEATS: u8 = 2;

/// How many stretches of what is captured are asked about a frame nowhere near the one
/// before it ([`align::anywhere`]), besides the two ends.
///
/// [P]. The same eight the matcher asks about tied peaks, for the same reason: past the
/// best few, a stretch the signatures liked less is not where the page is.
const ELSEWHERE: usize = 8;

/// How many times better going back must fit than going forwards to be tried first, when
/// both have an answer ([`Stitcher::both_ways`]).
///
/// [P]. Twice. Answers the pixels liked about as well are answers they cannot tell apart,
/// and then the one to keep is the one the capture would have taken going only forwards.
/// Over a page resampled between rows, a nudge of under a row fitted "one row on" at 169
/// and "nought" going back at 156, and taking nought left the anchor on a frame from which
/// the next was misplaced by 244 rows (2026-09-24). A scroll back is not close: the one
/// that found this fitted at nought, where going forwards' best fitted at 706.
const CLEARER: f64 = 2.0;

/// How much of a frame's body must be rows it shares with what it is placed against, when
/// it is placed any way but forwards from the anchor ([`Stitcher::both_ways`]).
///
/// [P]. A half, where going forwards takes a tenth ([`align::reach`]). These are second
/// chances, asked only of a frame the ordinary answer turned down, and what a short
/// overlap agrees with is a short overlap: a flick's frame from far up a page was placed
/// below its foot, where nothing is, on the 139 rows of 507 it seemed to share with the
/// first frame -- the same rows it was found by, and so the same rows any check of it
/// read (2026-09-24). Every frame placed this way in the capture that found D112 was
/// wholly inside what was captured, or two rows from where it began.
const SHARED: f64 = 0.5;

/// Which way the content moves.
///
/// Four, and one algorithm. `spec/07` §1.2 ends "horizontal mode is the transpose", and
/// backwards is the flip: a frame turned on its side or turned over is a frame that scrolls
/// down, which is the only case anything below this line knows about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    /// The page scrolls down; the capture grows taller.
    #[default]
    Down,
    /// The page scrolls up; the capture grows upwards.
    Up,
    /// `spec/07` §1.1's horizontal mode; the capture grows wider.
    Right,
    /// Horizontal, backwards.
    Left,
}

impl Direction {
    /// Whether the capture grows sideways rather than downwards.
    #[must_use]
    pub const fn horizontal(self) -> bool {
        matches!(self, Self::Right | Self::Left)
    }

    /// Whether new content arrives before what is already captured.
    #[must_use]
    pub const fn backwards(self) -> bool {
        matches!(self, Self::Up | Self::Left)
    }

    /// The same axis, the other way.
    #[must_use]
    pub const fn reversed(self) -> Self {
        match self {
            Self::Down => Self::Up,
            Self::Up => Self::Down,
            Self::Right => Self::Left,
            Self::Left => Self::Right,
        }
    }
}

/// How big a scrolling capture is allowed to get.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// `spec/07` §1.1 item 4: "a warning appears when the result is very long
    /// (> 20 000 px)". The stitcher only reports it; the warning is the app's.
    pub warn_at: u32,
    /// `spec/07` §1.2's "cap total size", past which the capture stops itself.
    ///
    /// [P] 64 000 px. Three times the warning, so the warning is a warning and not a
    /// countdown, and still a file every viewer on the machine can open.
    pub stop_at: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self { warn_at: 20_000, stop_at: 64_000 }
    }
}

/// Why a scrolling capture stopped on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// The page stopped moving: `spec/07` §1.2's "end of content".
    Content,
    /// Three frames running that the matcher could not place.
    Unmatched,
    /// The size cap.
    Full,
}

/// What the caller should do after handing a frame over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Scroll again by the same step.
    Scroll,
    /// Scroll again by a smaller step: `spec/07` §1.2's "retry with a smaller step".
    Smaller,
    /// Stop, and finish.
    Stop(End),
}

/// The growing capture.
#[derive(Debug)]
pub struct Stitcher {
    direction: Direction,
    limits: Limits,
    /// The first frame, which is where the sticky header is drawn from and which is the
    /// whole result if the page turns out not to scroll at all.
    first: Option<Frame>,
    /// What the next frame is matched against, and where the sticky footer comes from.
    last: Option<Frame>,
    /// Settled by the first accepted match and held after that.
    fixed: Option<Fixed>,
    /// Everything between the sticky runs, in the algorithm's own orientation.
    body: Option<Frame>,
    /// Whether a frame the page went back for is placed as well ([`Self::both_ways`]).
    both: bool,
    /// The row of [`Self::body`] that the body of [`Self::last`] starts at: for a capture
    /// that only goes forwards, always one body's length short of its end.
    at: u32,
    /// Whether the rows added last went before what was already captured.
    grew_back: bool,
    /// The columns the capture began in, and its rows as the matcher sees them over those
    /// columns: kept as it grows, for looking along it ([`align::anywhere`]).
    columns: Option<Range<u32>>,
    rows: Vec<Row>,
    repeats: u8,
    misses: u8,
    growth: u32,
    /// How far the page went back on the last frame that went back.
    back: u32,
    accepted: u32,
}

impl Stitcher {
    #[must_use]
    pub const fn new(direction: Direction, limits: Limits) -> Self {
        Self {
            direction,
            limits,
            first: None,
            last: None,
            fixed: None,
            body: None,
            both: false,
            at: 0,
            grew_back: false,
            columns: None,
            rows: Vec::new(),
            repeats: 0,
            misses: 0,
            growth: 0,
            back: 0,
            accepted: 0,
        }
    }

    /// A capture somebody is driving: the page is followed whichever way it goes along
    /// the axis, and `direction` is only which way it is expected to (D112).
    ///
    /// `spec/07` §1.1 item 5's manual mode is a person with a wheel, and a person scrolls
    /// the way the page will go. A conversation stands at its foot, so the only way it
    /// goes is up -- and a capture set to go down answered every frame of it with "scroll
    /// was too big", smallest nudge included, because a page that moved the other way is
    /// no offset the matcher can find (2026-09-24). So:
    ///
    /// - until a frame has been placed, a page that went the other way turns the capture
    ///   round, and it goes on as though that had been its direction all along;
    /// - after that, going back over what is captured is placed without adding anything,
    ///   and going on past either end adds what is new at that end;
    /// - and a frame nowhere near the one before it, which is what a flick back to where
    ///   the capture began leaves, is looked for along everything captured, and placed
    ///   where it is found.
    ///
    /// Nothing that drives the page wants this: it scrolls one way, and a frame that
    /// seems to have come back is a match the loop should not believe.
    #[must_use]
    pub const fn both_ways(mut self) -> Self {
        self.both = true;
        self
    }

    /// Hands the stitcher a frame, and asks what to do next.
    ///
    /// `expected` is how far the caller asked the page to scroll. It is a tie-break and
    /// nothing more -- see [`crate::align::locate`] -- so manual mode, where nobody knows
    /// how far the user flicked, passes [`Stitcher::growth`] or zero and loses nothing but
    /// its footing on a blank stretch of page.
    pub fn push(&mut self, frame: Frame, expected: u32) -> Result<Next, StitchError> {
        // One algorithm, applied sideways and upside down (`spec/07` §1.2, "horizontal
        // mode is the transpose"). Everything below this line thinks in rows, downwards.
        let frame = self.facing(frame);
        if let Some(seen) = self.last.as_ref()
            && (frame.width() != seen.width() || frame.height() != seen.height())
        {
            return Err(StitchError::Mismatch { width: frame.width(), height: frame.height() });
        }
        let Some(previous) = self.last.take() else {
            self.first = Some(frame.clone());
            self.last = Some(frame);
            return Ok(Next::Scroll);
        };
        let current = frame;
        let height = current.height();
        // The columns this pair can be measured in: the band, less whatever stood still at
        // its ends -- a sidebar, a margin, the window's own frame (D105).
        let band = crate::signature::band(current.width());
        let columns = match align::moving(&previous, &current, band.clone()) {
            Some(columns) => columns,
            // Nothing moved but a caret, on a page with something on it: the answer is
            // zero, and there is no need to ask the matcher for it.
            None if Signatures::over(&current, band.clone()).varies(0..height) => {
                return Ok(self.unmoved(previous));
            }
            // Nothing moved on a page with nothing on it -- a capture begun on a margin.
            // That is no evidence either way, and the matcher already knows what to do
            // with none: take the step at its word. It gets the whole band to find that.
            None => band,
        };
        // Whatever held still in the middle of it while the page moved under it -- a
        // notification, a toast, the pill of a capture that fills the screen -- the
        // matcher leaves out by itself ([`align::Changed`], D107).
        let previous_rows = Signatures::over(&previous, columns.clone());
        let current_rows = Signatures::over(&current, columns.clone());
        let fixed = self.fixed.unwrap_or_else(|| {
            let found = Fixed::between(&previous, &previous_rows, &current, &current_rows);
            // A mask is settled once and held for the whole capture, so the one chance to
            // take it is also the one chance to refuse it -- and a run that leaves less
            // body than the caller says the page moved is about to clip the search below
            // the answer. The step has evidence the run does not: somebody asked the page
            // to go that far.
            //
            // An hour grid scrolled by exactly three hours found it (2026-09-16): every
            // edge row of the two frames was the same row, because on a periodic page
            // scrolled by a whole number of periods it genuinely is, and 66 rows of
            // header and 44 of footer were read off a page with neither. The search was
            // clipped at 171 where the page had moved 174, three rows went missing at the
            // seam, and the frame after that could not be placed at all -- three misses
            // and a capture that stopped with "the page stopped matching".
            //
            // Refusing costs a genuine sticky run being drawn at every seam, which is a
            // capture you can still read. Keeping one costs a capture.
            let body = found.body(height);
            let room = body.end.saturating_sub(body.start);
            if expected > 0 && align::reach(room) < expected {
                tracing::debug!(?found, room, expected, "a sticky run with no room to scroll in");
                Fixed::default()
            } else {
                found
            }
        });
        let forward =
            align::fitted(&previous, &previous_rows, &current, &current_rows, fixed, expected)
                .filter(|(found, _)| found.trusted());
        // The other way round, for a capture somebody is driving, and tried first when the
        // pixels liked it clearly better ([`CLEARER`]) -- or as well, as they do rows that
        // are the same, and nearer, with more of the frame in it. Forwards first took a code
        // block's twin 392 rows on, over the 97 rows the two shared, for a scroll of 82 rows
        // back that shared 407 (2026-09-24); nearer first took "nought" going back for a
        // page one row on, which going back cannot say. The forward answer is kept for when
        // the other does not hold, so no frame the capture could place going forwards is
        // lost to this.
        let back = self
            .both
            .then(|| {
                let hint = if self.back > 0 { self.back } else { expected };
                align::fitted(&current, &current_rows, &previous, &previous_rows, fixed, hint)
            })
            .flatten()
            .filter(|(found, _)| found.trusted());
        let first_back = match (forward, back) {
            (Some((on, on_fit)), Some((back, back_fit))) => {
                back_fit * CLEARER < on_fit || (back_fit == on_fit && back.offset < on.offset)
            }
            (Some(_), None) => false,
            (None, _) => true,
        };
        if forward.is_some() && back.is_some() {
            tracing::debug!(?forward, ?back, first_back, "both ways had an answer");
        }
        let (forward, back) = (forward.map(|(found, _)| found), back.map(|(found, _)| found));
        if self.both
            && first_back
            && let Some(placing) = self.backwards(back, &current, &current_rows, &columns, fixed)
        {
            return match placing {
                Placing::Still => Ok(self.unmoved(previous)),
                Placing::Turn(offset) => self.turn(previous, current, fixed, offset, &columns),
                Placing::At(at) => {
                    tracing::debug!(at, "a frame placed back along what is captured");
                    self.land(previous, current, fixed, at, &columns)
                }
            };
        }
        if let Some(found) = forward {
            // `place` has already refined it against the pixels.
            if found.offset == 0 {
                return Ok(self.unmoved(previous));
            }
            let at = i64::from(self.at) + i64::from(found.offset);
            return self.land(previous, current, fixed, at, &columns);
        }
        // Not this page, or not settled yet. The previous frame stays where it is, so the
        // smaller step is matched against the same anchor rather than against a frame
        // nobody trusted.
        self.last = Some(previous);
        self.misses += 1;
        tracing::debug!(misses = self.misses, "a frame the matcher could not place");
        Ok(if self.misses >= MISSES { Next::Stop(End::Unmatched) } else { Next::Smaller })
    }

    /// Where a frame is that the anchor places going back, or not at all ([`Self::find`]):
    /// nowhere, for a page that went back under half a row; the capture turned round, if
    /// nothing has been placed; or a row of what is captured.
    fn backwards(
        &self,
        back: Option<align::Match>,
        current: &Frame,
        current_rows: &Signatures,
        columns: &Range<u32>,
        fixed: Fixed,
    ) -> Option<Placing> {
        match back {
            Some(found) if found.offset == 0 => return Some(Placing::Still),
            Some(found) if self.fixed.is_none() => {
                let body = fixed.body(current.height());
                let span = body.end - body.start;
                return shares(span.saturating_sub(found.offset), span)
                    .then_some(Placing::Turn(found.offset));
            }
            _ => {}
        }
        let back = back.map(|found| i64::from(self.at) - i64::from(found.offset));
        self.find(current, current_rows, columns, fixed, back).map(Placing::At)
    }

    /// The frame's body is at row `at` of the capture's -- which is before its start if
    /// `at` is negative: it is the anchor now, and whatever of it lies past either end of
    /// what is captured is added at that end. For a capture that only goes forwards, the
    /// anchor is always at the end, and what is added is all of what is new.
    fn land(
        &mut self,
        previous: Frame,
        current: Frame,
        fixed: Fixed,
        at: i64,
        columns: &Range<u32>,
    ) -> Result<Next, StitchError> {
        let body = fixed.body(current.height());
        // The first accepted match is where the sticky runs are settled, and where the
        // first frame's body finally joins the canvas: until now there was no way to know
        // how much of it was header.
        if self.fixed.is_none() {
            self.fixed = Some(fixed);
            let first = self.first.as_ref().unwrap_or(&previous);
            let canvas = first.rows(body.start, body.end)?;
            if self.both {
                self.rows = Signatures::over(&canvas, columns.clone()).rows().to_vec();
                self.columns = Some(columns.clone());
            }
            self.body = Some(canvas);
            self.at = 0;
            tracing::debug!(header = fixed.header, footer = fixed.footer, "the page's sticky rows");
        }
        let span = body.end - body.start;
        let moved = at - i64::from(self.at);
        match u32::try_from(at) {
            Ok(at) => {
                let length = self.body.as_ref().map_or(0, Frame::height);
                let new = (at + span).saturating_sub(length).min(span);
                if new > 0 {
                    let fresh = current.rows(body.end - new, body.end)?;
                    if let Some(columns) = self.columns.clone() {
                        self.rows.extend_from_slice(Signatures::over(&fresh, columns).rows());
                    }
                    match self.body.as_mut() {
                        Some(canvas) => canvas.stack(&fresh)?,
                        None => self.body = Some(fresh),
                    }
                    self.grew_back = false;
                    self.accepted += 1;
                }
                self.at = at;
            }
            Err(_) => {
                let new = u32::try_from(at.unsigned_abs()).map_or(span, |new| new.min(span));
                // A copy of the whole capture, where going on is an append. It is only
                // paid by somebody who went on, came back, and kept going past the start.
                let mut fresh = current.rows(body.start, body.start + new)?;
                if let Some(columns) = self.columns.clone() {
                    let mut rows = Signatures::over(&fresh, columns).rows().to_vec();
                    rows.extend_from_slice(&self.rows);
                    self.rows = rows;
                }
                if let Some(canvas) = self.body.as_ref() {
                    fresh.stack(canvas)?;
                }
                self.body = Some(fresh);
                self.grew_back = true;
                self.accepted += 1;
                self.at = 0;
            }
        }
        let distance = u32::try_from(moved.unsigned_abs()).unwrap_or(u32::MAX);
        match moved.signum() {
            1 => self.growth = distance,
            -1 => self.back = distance,
            _ => {}
        }
        Ok(self.placed(current))
    }

    /// The page went the other way before anything was placed ([`Self::both_ways`]): the
    /// capture turns round, and the frame is its first step that way.
    ///
    /// The one answer the other way that nothing is captured yet to check against, so it
    /// is held to sharing half the frame ([`SHARED`]) and to nothing else.
    fn turn(
        &mut self,
        previous: Frame,
        current: Frame,
        fixed: Fixed,
        offset: u32,
        columns: &Range<u32>,
    ) -> Result<Next, StitchError> {
        self.direction = self.direction.reversed();
        tracing::debug!(direction = ?self.direction, "the page went the other way; turning round");
        // Turned over, which is all the other way ever is ([`Self::facing`]): row `y` is
        // row `height - 1 - y`, and what stood still at the top stands still at the bottom.
        self.first = self.first.take().map(|first| first.flip());
        let fixed = Fixed { header: fixed.footer, footer: fixed.header };
        self.land(previous.flip(), current.flip(), fixed, i64::from(offset), columns)
    }

    /// Where along what is captured a frame is that the anchor could not place going
    /// forwards, for a capture somebody is driving that has begun: the anchor's own answer
    /// going back, if it had one, and then each stretch [`align::anywhere`] likes best and
    /// both ends -- where a flick usually stops, and where a frame that went past them is
    /// found -- made into the frame the page showed there and put to [`align::place`].
    ///
    /// Every answer is held to all the rows it shares with the capture ([`Self::agrees`]),
    /// not only to the stretch it was found against. These are second chances for a frame
    /// the ordinary answer turned down, and one of them was wrong: a flick's frame, which
    /// belonged nowhere, fitted a tenth of the anchor going back and was placed 471 rows up
    /// a capture whose next two frames then added nothing (2026-09-24).
    fn find(
        &self,
        current: &Frame,
        current_rows: &Signatures,
        columns: &Range<u32>,
        fixed: Fixed,
        back: Option<i64>,
    ) -> Option<i64> {
        let canvas = self.body.as_ref()?;
        if let Some(at) = back.and_then(|at| self.agrees(current, current_rows, columns, fixed, at))
        {
            return Some(at);
        }
        let body = fixed.body(current.height());
        let last = canvas.height().checked_sub(body.end - body.start)?;
        // Read over the columns the capture began in, whatever this pair moved in, so the
        // capture's rows are read once as they arrive and never again.
        let mine = Signatures::over(current, self.columns.clone()?);
        let mine = mine.rows().get(body.start as usize..body.end as usize)?;
        // Only a stretch the correlation vouches for, as only a peak it vouches for is let
        // near the pixels in [`align::place`]: a frame of a page nobody has captured yet
        // costs the one pass, and not ten matches that were never going to agree. Each is
        // asked the ways worth asking: a stretch inside the capture both, and an end only
        // outwards -- what went past the end went on from it, and what went past the start
        // went back from it.
        let mut starts: Vec<(u32, bool, bool)> = align::anywhere(&self.rows, mine)
            .into_iter()
            .take_while(|found| found.trusted())
            .take(ELSEWHERE)
            .map(|found| (found.offset, true, true))
            .collect();
        // The end the anchor stands at has been asked already, both ways, as the anchor.
        for (end, on, back) in [(last, true, false), (0, false, true)] {
            if end != self.at && !starts.iter().any(|&(start, ..)| start == end) {
                starts.push((end, on, back));
            }
        }
        starts.into_iter().find_map(|(start, on, back)| {
            let there = framed(current, canvas, fixed, i64::from(start))?;
            let rows = Signatures::over(&there, columns.clone());
            let start = i64::from(start);
            let went_on = on
                .then(|| align::place(&there, &rows, current, current_rows, fixed, 0))
                .flatten()
                .filter(|found| found.trusted())
                .map(|found| i64::from(found.offset));
            // Nowhere to go and all of it inside the capture: that was the whole of the
            // check [`Self::agrees`] would make.
            if went_on == Some(0) && start <= i64::from(last) {
                return Some(start);
            }
            let at = went_on.map(|offset| start + offset).or_else(|| {
                back.then(|| align::place(current, current_rows, &there, &rows, fixed, 0))
                    .flatten()
                    .filter(|found| found.trusted())
                    .map(|found| start - i64::from(found.offset))
            })?;
            self.agrees(current, current_rows, columns, fixed, at)
        })
    }

    /// Whether the frame is at row `at` of the capture by all the rows it shares with it:
    /// the frame the page would have shown there, made of the capture's rows where it has
    /// them and the frame's own elsewhere, and asked of [`align::place`] against the frame.
    /// Where it is to the row, allowing the half a row either way a page the compositor
    /// resampled lands in ([`align::refine`]), or nothing.
    fn agrees(
        &self,
        current: &Frame,
        current_rows: &Signatures,
        columns: &Range<u32>,
        fixed: Fixed,
        at: i64,
    ) -> Option<i64> {
        let canvas = self.body.as_ref()?;
        let body = fixed.body(current.height());
        let span = i64::from(body.end - body.start);
        // What it shares is all a check of it can read: the rest of `there` is its own.
        let shared = (at + span).min(i64::from(canvas.height())) - at.max(0);
        if !shares(u32::try_from(shared).unwrap_or(0), body.end - body.start) {
            return None;
        }
        let there = framed(current, canvas, fixed, at)?;
        let rows = Signatures::over(&there, columns.clone());
        let nudge = align::place(&there, &rows, current, current_rows, fixed, 0)
            .filter(|found| found.trusted())
            .map(|found| i64::from(found.offset))
            .or_else(|| {
                align::place(current, current_rows, &there, &rows, fixed, 0)
                    .filter(|found| found.trusted())
                    .map(|found| -i64::from(found.offset))
            })?;
        (nudge.abs() <= 1).then_some(at + nudge)
    }

    /// A frame was placed: it is the anchor now.
    fn placed(&mut self, current: Frame) -> Next {
        self.repeats = 0;
        self.misses = 0;
        self.last = Some(current);
        if self.length() >= self.limits.stop_at { Next::Stop(End::Full) } else { Next::Scroll }
    }

    /// The page did not move: the anchor stays where it is, and two of these in a row are
    /// `spec/07` §1.2's end of content.
    fn unmoved(&mut self, previous: Frame) -> Next {
        self.last = Some(previous);
        self.repeats += 1;
        tracing::debug!(repeats = self.repeats, "the page did not move");
        if self.repeats >= REPEATS { Next::Stop(End::Content) } else { Next::Scroll }
    }

    /// The stitched capture as it stands, in the user's own orientation.
    ///
    /// The live preview strip (`spec/07` §1.1 item 3) is this, scaled down. Composed on
    /// demand rather than kept composed, because the sticky footer comes from whichever
    /// frame is the last one *so far* and changes under every step.
    #[must_use]
    pub fn composed(&self) -> Option<Frame> {
        let first = self.first.as_ref()?;
        let Some(body) = self.body.as_ref() else {
            // Nothing matched yet: one frame is a perfectly good screenshot, and handing
            // it back is kinder than an error for a user who pressed Done too early.
            return Some(self.upright(first.clone()));
        };
        let fixed = self.fixed.unwrap_or_default();
        let mut canvas = match fixed.header {
            0 => body.clone(),
            header => {
                let mut top = first.rows(0, header).ok()?;
                top.stack(body).ok()?;
                top
            }
        };
        if fixed.footer > 0
            && let Some(last) = self.last.as_ref()
        {
            let height = last.height();
            if let Ok(footer) = last.rows(height - fixed.footer, height) {
                canvas.stack(&footer).ok()?;
            }
        }
        Some(self.upright(canvas))
    }

    /// The finished capture.
    pub fn finish(self) -> Result<Frame, StitchError> {
        self.composed().ok_or(StitchError::Empty)
    }

    /// The size so far, in the user's orientation.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        let Some(first) = self.first.as_ref() else { return (0, 0) };
        let across = first.width();
        let along = self.length();
        if self.direction.horizontal() { (along, across) } else { (across, along) }
    }

    /// `spec/07` §1.1 item 4's "very long" result.
    #[must_use]
    pub fn long(&self) -> bool {
        self.length() >= self.limits.warn_at
    }

    /// How far the page moved on the last frame placed going forwards.
    #[must_use]
    pub const fn growth(&self) -> u32 {
        self.growth
    }

    /// How many frames past the first have added to the capture. A frame placed over
    /// what is already captured ([`Self::both_ways`]) is not one of them.
    #[must_use]
    pub const fn accepted(&self) -> u32 {
        self.accepted
    }

    /// The end the last rows were added at, as a direction: the capture's own, turned
    /// round if the page went the other way first, and turned again while it grows at its
    /// start ([`Self::both_ways`]). The end a window onto the growing capture shows (D77).
    #[must_use]
    pub const fn growing(&self) -> Direction {
        if self.grew_back { self.direction.reversed() } else { self.direction }
    }

    /// The sticky runs, once a match has settled them.
    #[must_use]
    pub const fn fixed(&self) -> Option<Fixed> {
        self.fixed
    }

    /// The length along the scroll, sticky runs included.
    fn length(&self) -> u32 {
        let Some(first) = self.first.as_ref() else { return 0 };
        let Some(body) = self.body.as_ref() else { return first.height() };
        let fixed = self.fixed.unwrap_or_default();
        body.height() + fixed.header + fixed.footer
    }

    /// Turned so the algorithm's "down" is the direction the page is going.
    fn facing(&self, frame: Frame) -> Frame {
        let frame = if self.direction.horizontal() { frame.transpose() } else { frame };
        if self.direction.backwards() { frame.flip() } else { frame }
    }

    /// Back the way the user is looking at it: [`Self::facing`] undone, in reverse.
    fn upright(&self, frame: Frame) -> Frame {
        let frame = if self.direction.backwards() { frame.flip() } else { frame };
        if self.direction.horizontal() { frame.transpose() } else { frame }
    }
}

/// What became of a frame placed any way but forwards from the anchor.
enum Placing {
    /// It went back under half a row: the page did not move.
    Still,
    /// Nothing was placed yet, and it went back this far: the capture turns round.
    Turn(u32),
    /// Its body is at this row of what is captured, before the start if negative.
    At(i64),
}

/// Whether `rows` of a body `span` long is enough of it for a second chance ([`SHARED`]).
fn shares(rows: u32, span: u32) -> bool {
    f64::from(rows) >= f64::from(span) * SHARED
}

/// The frame the page showed with its body at row `at` of the capture: `current`'s sticky
/// runs, which are the same in every frame, round the capture's rows from there -- and
/// `current`'s own rows for those of its body past either end of the capture.
fn framed(current: &Frame, canvas: &Frame, fixed: Fixed, at: i64) -> Option<Frame> {
    let height = current.height();
    let body = fixed.body(height);
    let mut pixels = Vec::with_capacity(current.pixels().len());
    for y in 0..body.start {
        pixels.extend_from_slice(current.row(y));
    }
    for y in body.clone() {
        let row = at + i64::from(y - body.start);
        match u32::try_from(row).ok().filter(|&row| row < canvas.height()) {
            Some(row) => pixels.extend_from_slice(canvas.row(row)),
            None => pixels.extend_from_slice(current.row(y)),
        }
    }
    for y in body.end..height {
        pixels.extend_from_slice(current.row(y));
    }
    Frame::new(current.width(), height, pixels).ok()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testing::{Look, looking, sticky, window, with_sidebar};

    /// Scrolls a synthetic page in `step` rows at a time and stitches it.
    fn run(width: u32, height: u32, step: u32, steps: u32) -> Stitcher {
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
        for i in 0..=steps {
            let frame = window(width, height, i * step, 0);
            let next = stitcher.push(frame, step).expect("a frame of the right size");
            if i > 0 {
                assert_eq!(next, Next::Scroll, "step {i}");
            }
        }
        stitcher
    }

    #[test]
    fn a_scrolled_page_stitches_to_exactly_the_rows_it_showed() {
        let stitcher = run(64, 200, 60, 3);
        // Four frames, three of them adding 60 rows: 200 + 180.
        assert_eq!(stitcher.size(), (64, 380));
        assert_eq!(stitcher.accepted(), 3);
        let result = stitcher.finish().expect("a result");
        let whole = window(64, 380, 0, 0);
        assert_eq!(result, whole, "the stitched page is the page");
    }

    #[test]
    fn a_page_that_stops_moving_ends_by_itself() {
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
        let frame = window(64, 200, 0, 0);
        assert_eq!(stitcher.push(frame.clone(), 120).expect("f0"), Next::Scroll);
        assert_eq!(stitcher.push(window(64, 200, 120, 0), 120).expect("f1"), Next::Scroll);
        // The page has hit its bottom: every further frame is the same one.
        let bottom = window(64, 200, 120, 0);
        assert_eq!(stitcher.push(bottom.clone(), 120).expect("f2"), Next::Scroll);
        assert_eq!(stitcher.push(bottom, 120).expect("f3"), Next::Stop(End::Content));
        assert_eq!(stitcher.size(), (64, 320));
    }

    /// D94, the whole way through: the page the user was capturing was Google Calendar's
    /// day view, whose grid is a dark ground ruled every hour and nothing else. Scrolled
    /// to the foot of the day it stopped moving, and the matcher -- which could not tell
    /// one rule from the next and was told the step it had asked for -- answered with
    /// three hours every time. Twenty of them went onto the canvas, the same three hours
    /// over and over, and because the offset was never zero the capture had no reason to
    /// end (2026-09-16).
    #[test]
    fn a_ruled_page_at_its_foot_stops_rather_than_appending_the_same_rows_for_ever() {
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
        let step = 3 * crate::testing::RULE;
        let foot = looking(Look::Ruled, 240, 400, 0, 0);
        assert_eq!(stitcher.push(foot.clone(), step).expect("f0"), Next::Scroll);
        assert_eq!(stitcher.push(foot.clone(), step).expect("f1"), Next::Scroll);
        assert_eq!(stitcher.push(foot, step).expect("f2"), Next::Stop(End::Content));
        assert_eq!(stitcher.size(), (240, 400), "not one row of it was new");
    }

    /// The other side of D94, and the one that had to be proved before the fix could be
    /// kept: a calendar that really *is* scrolling. The hour grid ties at every rule
    /// exactly as it did at the foot of the day, so zero is a tied contender on every
    /// frame -- but there are appointments on this part of the page, so the pixels can
    /// separate the hours and the step is taken at its word. Stitched to the page, row
    /// for row, at a step of three whole hours.
    ///
    /// (`fixed` comes out as a 160-row header and a 19-row footer here, on a page with no
    /// sticky anything: at a step that is a whole number of rules the edge rows of two
    /// frames genuinely are the same pixels, and no amount of looking at them can say
    /// otherwise. It is harmless -- the rows below a false header are covered by the body
    /// of the frames before it, so the canvas is still contiguous -- and the assertion
    /// below is what says so.)
    #[test]
    fn a_calendar_that_scrolls_is_stitched_although_its_grid_ties_at_every_hour() {
        let step = 3 * crate::testing::RULE;
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
        for i in 0..5u32 {
            let frame = looking(Look::Calendar, 240, 400, i * step, 9);
            assert_eq!(stitcher.push(frame, step).expect("a frame"), Next::Scroll, "frame {i}");
        }
        assert_eq!(stitcher.size(), (240, 400 + 4 * step));
        let result = stitcher.finish().expect("a result");
        let day = looking(Look::Calendar, 240, 400 + 4 * step, 0, 9);
        assert_eq!(result, day, "the stitch is the day");
    }

    /// The adversarial calendar: an hour grid scrolled by *exactly* three hours. On a
    /// periodic page moved by a whole number of periods the edge rows of two frames
    /// genuinely are the same rows, so `Fixed::between` reads a sticky run off a page
    /// that has none -- and no amount of looking at those rows can say otherwise, because
    /// they are identical pixels.
    ///
    /// What can be said is that the run leaves less body than the caller scrolled. This
    /// page reads 13 rows of header and 103 of footer, on a page with neither, which
    /// leaves 184 rows of body -- and a body that deep can only be searched to 166, while
    /// the page moved 174. Believed, it clipped the search below the answer: rows lost at
    /// the first seam, the frame after that unplaceable, and a capture that stopped with
    /// "the page stopped matching".
    #[test]
    fn a_sticky_run_with_no_room_to_scroll_in_is_not_believed() {
        let step = 3 * crate::testing::RULE;
        let a = looking(Look::Calendar, 240, 300, 0, 0);
        let b = looking(Look::Calendar, 240, 300, step, 0);
        let (sa, sb) = (Signatures::of(&a), Signatures::of(&b));
        let read = Fixed::between(&a, &sa, &b, &sb);
        assert_eq!((read.header, read.footer), (13, 103), "the run this page reads as");
        assert!(crate::align::reach(read.body(300).end - read.body(300).start) < step, "{read:?}");

        let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
        for i in 0..3u32 {
            let frame = looking(Look::Calendar, 240, 300, i * step, 0);
            assert_eq!(stitcher.push(frame, 180).expect("a frame"), Next::Scroll, "frame {i}");
        }
        assert_eq!(stitcher.fixed(), Some(Fixed::default()), "a page with no sticky anything");
        assert_eq!(stitcher.size(), (240, 300 + 2 * step), "and not one row short of it");
        let day = looking(Look::Calendar, 240, 300 + 2 * step, 0, 0);
        assert_eq!(stitcher.finish().expect("a result"), day, "the stitch is the day");
    }

    #[test]
    fn three_frames_the_matcher_cannot_place_stop_the_capture() {
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
        stitcher.push(window(64, 200, 0, 0), 120).expect("f0");
        for (i, seed) in [11u32, 22, 33].into_iter().enumerate() {
            let next = stitcher.push(window(64, 200, 0, seed), 120).expect("a stranger");
            let wanted = if i + 1 >= usize::from(MISSES) { Next::Stop(End::Unmatched) } else { Next::Smaller };
            assert_eq!(next, wanted, "stranger {i}");
        }
    }

    #[test]
    fn a_smaller_step_is_matched_against_the_frame_that_was_trusted() {
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
        stitcher.push(window(64, 200, 0, 0), 120).expect("f0");
        assert_eq!(stitcher.push(window(64, 200, 0, 99), 120).expect("a stranger"), Next::Smaller);
        // The retry lands on the real page again and is placed against F0, not against
        // the frame nobody trusted.
        assert_eq!(stitcher.push(window(64, 200, 60, 0), 60).expect("the retry"), Next::Scroll);
        assert_eq!(stitcher.size(), (64, 260));
        assert_eq!(stitcher.finish().expect("a result"), window(64, 260, 0, 0));
    }

    #[test]
    fn a_sticky_header_is_drawn_once() {
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
        for i in 0..4u32 {
            let frame = sticky(64, 200, i * 50, 0, 24, 0);
            stitcher.push(frame, 50).expect("a frame");
        }
        let fixed = stitcher.fixed().expect("a settled mask");
        assert_eq!((fixed.header, fixed.footer), (24, 0));
        // 200 + three lots of 50, and the header counted once inside that.
        assert_eq!(stitcher.size(), (64, 350));
        let result = stitcher.finish().expect("a result");
        // The header, then the page from row 24 on: nothing repeated, nothing missing.
        let mut wanted = window(64, 24, 0, 0);
        wanted.stack(&window(64, 326, 24, 0)).expect("same width");
        assert_eq!(result, wanted);
    }

    #[test]
    fn a_sticky_footer_is_drawn_once_at_the_end() {
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
        for i in 0..4u32 {
            stitcher.push(sticky(64, 200, i * 50, 0, 0, 18), 50).expect("a frame");
        }
        let fixed = stitcher.fixed().expect("a settled mask");
        assert_eq!((fixed.header, fixed.footer), (0, 18));
        assert_eq!(stitcher.size(), (64, 350));
        let result = stitcher.finish().expect("a result");
        // The body of the page, then the footer the last frame was still showing.
        let mut wanted = window(64, 332, 0, 0);
        wanted.stack(&window(64, 18, 182, 0)).expect("same width");
        assert_eq!(result, wanted);
    }

    #[test]
    fn a_horizontal_capture_is_the_same_algorithm_sideways() {
        let mut stitcher = Stitcher::new(Direction::Right, Limits::default());
        for i in 0..4u32 {
            // A page that scrolls right is the vertical page turned on its side.
            stitcher.push(window(64, 200, i * 60, 0).transpose(), 60).expect("a frame");
        }
        assert_eq!(stitcher.size(), (380, 64));
        assert_eq!(stitcher.finish().expect("a result"), window(64, 380, 0, 0).transpose());
    }

    #[test]
    fn a_capture_that_grows_upwards_is_the_same_algorithm_turned_over() {
        // The user starts at the bottom of the page and scrolls back up: each frame shows
        // rows *above* the last, and the result is the page the right way up.
        let mut stitcher = Stitcher::new(Direction::Up, Limits::default());
        for i in (0..4u32).rev() {
            stitcher.push(window(64, 200, i * 60, 0), 60).expect("a frame");
        }
        assert_eq!(stitcher.size(), (64, 380));
        assert_eq!(stitcher.finish().expect("a result"), window(64, 380, 0, 0));
    }

    /// Pushes a frame per top and asserts each was placed.
    fn follow(stitcher: &mut Stitcher, tops: &[u32], sideways: bool) {
        for &top in tops {
            let frame = window(64, 200, top, 0);
            let frame = if sideways { frame.transpose() } else { frame };
            let next = stitcher.push(frame, stitcher.growth()).expect("a frame");
            assert_eq!(next, Next::Scroll, "the frame at {top}");
        }
    }

    /// A conversation stands at its foot, so the capture set going down over it is only
    /// ever scrolled up: every frame of it used to be a miss (2026-09-24). Driven by hand,
    /// the capture turns round on the first frame that went the other way.
    #[test]
    fn a_page_scrolled_against_a_capture_somebody_is_driving_turns_it_round() {
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default()).both_ways();
        follow(&mut stitcher, &[240, 180, 120, 60, 0], false);
        assert_eq!(stitcher.growing(), Direction::Up);
        assert_eq!(stitcher.accepted(), 4);
        assert_eq!(stitcher.size(), (64, 440));
        assert_eq!(stitcher.finish().expect("a result"), window(64, 440, 0, 0));
    }

    #[test]
    fn a_sideways_capture_turns_round_the_same_way() {
        let mut stitcher = Stitcher::new(Direction::Right, Limits::default()).both_ways();
        follow(&mut stitcher, &[180, 120, 60, 0], true);
        assert_eq!(stitcher.growing(), Direction::Left);
        assert_eq!(stitcher.size(), (380, 64));
        assert_eq!(stitcher.finish().expect("a result"), window(64, 380, 0, 0).transpose());
    }

    /// Turned round, what stood still at the top of the frame is still drawn once at the
    /// top of the capture.
    #[test]
    fn a_capture_that_turns_round_keeps_its_sticky_header_on_top() {
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default()).both_ways();
        for top in [150u32, 100, 50, 0] {
            let next = stitcher.push(sticky(64, 200, top, 0, 24, 0), 50).expect("a frame");
            assert_eq!(next, Next::Scroll, "the frame at {top}");
        }
        assert_eq!(stitcher.size(), (64, 350));
        let mut wanted = window(64, 24, 0, 0);
        wanted.stack(&window(64, 326, 24, 0)).expect("same width");
        assert_eq!(stitcher.finish().expect("a result"), wanted);
    }

    /// Going back to read something again is placed, adds nothing, and does not lose the
    /// capture's footing: going on from there adds only what is new.
    #[test]
    fn going_back_over_what_is_captured_places_the_frame_without_adding_to_it() {
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default()).both_ways();
        follow(&mut stitcher, &[0, 60, 120, 180], false);
        assert_eq!((stitcher.size(), stitcher.accepted()), ((64, 380), 3));
        follow(&mut stitcher, &[150, 90, 150], false);
        assert_eq!((stitcher.size(), stitcher.accepted()), ((64, 380), 3), "nothing new");
        assert_eq!(stitcher.growing(), Direction::Down);
        follow(&mut stitcher, &[210, 270], false);
        assert_eq!((stitcher.size(), stitcher.accepted()), ((64, 470), 5));
        assert_eq!(stitcher.finish().expect("a result"), window(64, 470, 0, 0));
    }

    /// And going back past where the capture began adds what is before it, at the start.
    #[test]
    fn going_back_past_the_start_adds_what_is_before_it() {
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default()).both_ways();
        follow(&mut stitcher, &[200, 260, 200, 140, 80], false);
        assert_eq!(stitcher.growing(), Direction::Up);
        assert_eq!((stitcher.size(), stitcher.accepted()), ((64, 380), 3));
        follow(&mut stitcher, &[140, 200, 260, 320], false);
        assert_eq!(stitcher.growing(), Direction::Down);
        assert_eq!(stitcher.size(), (64, 440));
        assert_eq!(stitcher.finish().expect("a result"), window(64, 440, 80, 0));
    }

    /// A flick back to where the capture began lands nowhere near the frame before it, and
    /// on rows the capture already has: it is found there (2026-09-24), and going on from
    /// it is going on from where the page is.
    #[test]
    fn a_flick_back_to_where_the_capture_began_is_found_in_what_is_captured() {
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default()).both_ways();
        follow(&mut stitcher, &[400, 340, 280, 220, 160], false);
        assert_eq!((stitcher.size(), stitcher.accepted()), ((64, 440), 4));
        follow(&mut stitcher, &[400], false);
        assert_eq!((stitcher.size(), stitcher.accepted()), ((64, 440), 4), "nothing new");
        follow(&mut stitcher, &[440, 380], false);
        assert_eq!(stitcher.growing(), Direction::Down);
        assert_eq!(stitcher.size(), (64, 480));
        assert_eq!(stitcher.finish().expect("a result"), window(64, 480, 160, 0));
    }

    /// A flick past everything captured is still a miss -- there is nothing to find it in
    /// -- and the capture waits for the page to come back to rows it has.
    #[test]
    fn a_flick_past_everything_captured_is_a_miss_until_the_page_comes_back() {
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default()).both_ways();
        follow(&mut stitcher, &[0, 60, 120], false);
        assert_eq!(stitcher.push(window(64, 200, 700, 0), 60).expect("a flick"), Next::Smaller);
        follow(&mut stitcher, &[40, 100, 160], false);
        assert_eq!(stitcher.finish().expect("a result"), window(64, 360, 0, 0));
    }

    /// A loop that scrolls the page one way believes nothing that seems to have come back.
    #[test]
    fn a_capture_that_drives_the_page_does_not_follow_it_back() {
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
        stitcher.push(window(64, 200, 60, 0), 60).expect("f0");
        assert_eq!(stitcher.push(window(64, 200, 0, 0), 60).expect("f1"), Next::Smaller);
        assert_eq!(stitcher.growing(), Direction::Down);
        assert_eq!(stitcher.push(window(64, 200, 120, 0), 60).expect("f2"), Next::Scroll);
        assert_eq!(stitcher.finish().expect("a result"), window(64, 260, 60, 0));
    }

    #[test]
    fn the_cap_stops_a_capture_that_would_not() {
        let mut stitcher = Stitcher::new(Direction::Down, Limits { warn_at: 260, stop_at: 320 });
        assert!(!stitcher.long());
        stitcher.push(window(64, 200, 0, 0), 60).expect("f0");
        assert_eq!(stitcher.push(window(64, 200, 60, 0), 60).expect("f1"), Next::Scroll);
        assert!(stitcher.long());
        assert_eq!(stitcher.push(window(64, 200, 120, 0), 60).expect("f2"), Next::Stop(End::Full));
    }

    /// The case the bar page never puts to it: a page of type, where every second row is
    /// blank and the rows that are not repeat on a 20-row pitch. A matcher that leans on
    /// row-to-row novelty places this one a line out, and a line out is a README with a
    /// sentence missing.
    #[test]
    fn a_page_of_type_stitches_without_losing_or_repeating_a_line() {
        for look in [Look::Prose, Look::Terminal] {
            for step in [37u32, 60, 120, 140] {
                let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
                for i in 0..=4u32 {
                    stitcher.push(looking(look, 240, 200, i * step, 5), step).expect("a frame");
                }
                let height = 200 + 4 * step;
                assert_eq!(stitcher.size(), (240, height), "{look:?} in {step}s");
                let result = stitcher.finish().expect("a result");
                assert_eq!(result, looking(look, 240, height, 0, 5), "{look:?} in {step}s");
            }
        }
    }

    /// A page the compositor resampled, scrolled the way a wheel scrolls one at 1.25: in
    /// uneven steps that land between rows, two of them too small to be a scroll at all.
    /// Every frame is placed, every seam within half a row of where the page went, and the
    /// capture as long as the page went give or take those halves.
    #[test]
    fn a_page_the_compositor_resampled_stitches_without_a_miss() {
        use crate::testing::resampled;
        // In eighths of a row.
        let tops = [0u32, 5, 30, 95, 180, 181, 290, 455, 700, 703, 1_005, 1_391, 1_900, 2_417];
        for look in [Look::Prose, Look::Terminal] {
            let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
            let mut anchor = 0;
            for (i, &top) in tops.iter().enumerate() {
                let accepted = stitcher.accepted();
                let frame = resampled(look, 360, 240, top, 9);
                let next = stitcher.push(frame, stitcher.growth()).expect("a frame");
                let case = format!("{look:?}, frame {i} at {top} eighths");
                assert_eq!(next, Next::Scroll, "{case}");
                if i == 0 {
                    continue;
                }
                if stitcher.accepted() == accepted {
                    assert!(top - anchor < 8, "{case}: a scroll of a row or more was not placed");
                    continue;
                }
                let placed = stitcher.growth() * 8;
                assert!(placed.abs_diff(top - anchor) <= 4, "{case}: placed at {placed} eighths");
                anchor = top;
            }
            let (_, height) = stitcher.size();
            let went = 240 * 8 + anchor;
            let halves = stitcher.accepted() * 4;
            let case = format!("{look:?}: {height} rows for {went} eighths");
            assert!((height * 8).abs_diff(went) <= halves, "{case}");
        }
    }

    /// A flick too big to place is a miss, not a short append.
    ///
    /// It used to be a short append: the search stopped at three quarters of the body, so a
    /// scroll past that was answered with the best offset inside the range -- a wrong one,
    /// on a page of type, at an NCC the accept rule was happy with. Nine lines went missing
    /// at every seam of a twelve-thousand-pixel capture and nothing anywhere said so.
    #[test]
    fn a_scroll_past_the_search_is_a_miss_rather_than_a_short_append() {
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
        stitcher.push(looking(Look::Prose, 240, 200, 0, 4), 120).expect("the first frame");
        // 190 of 200: the frames share ten rows, which is under `MIN_OVERLAP`.
        assert_eq!(stitcher.push(looking(Look::Prose, 240, 200, 190, 4), 120).expect("a frame"), Next::Smaller);
        assert_eq!(stitcher.size(), (240, 200), "nothing may be appended from a frame nobody placed");
        // And a frame that *does* overlap is still placed afterwards, from the same anchor.
        assert_eq!(stitcher.push(looking(Look::Prose, 240, 200, 150, 4), 120).expect("a frame"), Next::Scroll);
        assert_eq!(stitcher.size(), (240, 350));
    }

    /// The case a real editor put to it on 2026-09-15, and the reason the matcher asks the
    /// pixels: a listing whose every line weighs the same to the row signatures. The
    /// correlation ties at every one of the twenty-row pitches, so a matcher that breaks
    /// ties towards the step it asked for answers with that step every time -- and the
    /// capture repeats fourteen lines at every seam while looking perfectly confident.
    ///
    /// The page is scrolled by a third of what the caller asks for, which is what a wheel
    /// does in manual mode: the hint is a guess about a page nobody is driving.
    #[test]
    fn an_even_listing_is_placed_by_its_pixels_and_not_by_the_step_we_asked_for() {
        for step in [40u32, 60, 100] {
            let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
            for i in 0..=4u32 {
                // Told 3 x step, moved step: every wrong peak is nearer the hint than the
                // right one.
                stitcher.push(looking(Look::Listing, 240, 200, i * step, 6), step * 3).expect("a frame");
            }
            let height = 200 + 4 * step;
            assert_eq!(stitcher.size(), (240, height), "in {step}s");
            assert_eq!(stitcher.finish().expect("a result"), looking(Look::Listing, 240, height, 0, 6), "in {step}s");
        }
    }

    /// A step that lands the frame entirely inside the blank leading between paragraphs
    /// still has the lines above and below it to go on.
    #[test]
    fn a_page_of_type_survives_a_step_that_is_a_whole_number_of_lines() {
        // 20 rows is exactly the pitch: every line of the new frame sits where a line of
        // the old one was, and only the words tell them apart.
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
        for i in 0..=5u32 {
            stitcher.push(looking(Look::Prose, 240, 200, i * 20, 3), 20).expect("a frame");
        }
        assert_eq!(stitcher.size(), (240, 300));
        assert_eq!(stitcher.finish().expect("a result"), looking(Look::Prose, 240, 300, 0, 3));
    }

    #[test]
    fn a_blank_page_is_not_mistaken_for_one_long_sticky_header() {
        // A capture that starts on a margin: two identical white frames, and nothing in
        // them to tell a sticky run from an empty one.
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
        let blank = Frame::filled(64, 200, [252, 252, 252, 255]);
        stitcher.push(blank.clone(), 120).expect("f0");
        assert_eq!(stitcher.push(blank, 120).expect("f1"), Next::Scroll);
        assert_eq!(stitcher.fixed(), Some(Fixed::default()));
        // The step is the only thing that says how far it went, and it is taken at its
        // word rather than frozen into a mask for the rest of the capture.
        assert_eq!(stitcher.size(), (64, 320));
    }

    /// D105, as the user's own frame showed it: type set flush left on a dark ground,
    /// scrolled by a line to half a frame, and the matcher told a distance nowhere near
    /// the truth. The centred band placed 200 at 178, 300 at 285, and anything at all at
    /// 630 when told 643 -- and took every one of them, because blank against blank fits
    /// perfectly. Here the stitch is the page, row for row, whatever it is told.
    #[test]
    fn type_set_flush_left_is_placed_where_it_scrolled_whatever_the_hint() {
        for shift in [24u32, 200, 300, 370] {
            for hint in [0u32, 420, 643] {
                let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
                stitcher.push(looking(Look::Flush, 480, 700, 0, 11), hint).expect("f0");
                let second = looking(Look::Flush, 480, 700, shift, 11);
                let next = stitcher.push(second, hint).expect("f1");
                assert_eq!(next, Next::Scroll, "shift {shift}, hint {hint}");
                assert_eq!(stitcher.growth(), shift, "shift {shift}, hint {hint}");
                let page = looking(Look::Flush, 480, 700 + shift, 0, 11);
                let stitched = stitcher.finish().expect("a result");
                assert_eq!(stitched, page, "shift {shift}, hint {hint}");
            }
        }
    }

    /// A sidebar a fifth of the width that stands still while the page beside it scrolls.
    /// Measured across the whole band, its rows disagree at every offset but zero and push
    /// the right one past the fit; trimmed off the end of the band, it costs nothing
    /// (D105). What the capture shows beside the sidebar is the page.
    #[test]
    fn a_sidebar_that_stands_still_does_not_hide_the_page_scrolling_beside_it() {
        let (width, height, side) = (400u32, 300u32, 80u32);
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
        for (i, top) in [0u32, 60, 130, 210].into_iter().enumerate() {
            let frame = with_sidebar(&looking(Look::Prose, width, height, top, 5), side, 5);
            assert_eq!(stitcher.push(frame, 0).expect("a frame"), Next::Scroll, "frame {i}");
        }
        assert_eq!(stitcher.size(), (width, height + 210));
        let result = stitcher.finish().expect("a result");
        let page = looking(Look::Prose, width, height + 210, 0, 5);
        let at = (side as usize) * 4;
        for y in 0..result.height() {
            assert_eq!(result.row(y)[at..], page.row(y)[at..], "row {y}");
        }
    }

    /// A caret blinking on a page nobody scrolled: two columns changed, which is not a
    /// scroll, and the answer is "the page did not move".
    #[test]
    fn a_caret_is_not_a_scroll() {
        let page = looking(Look::Prose, 240, 200, 0, 3);
        let mut blinked = page.pixels().to_vec();
        for y in 40..56u32 {
            for x in 100..102u32 {
                let at = ((y * 240 + x) * 4) as usize;
                blinked[at..at + 4].copy_from_slice(&[0, 0, 0, 255]);
            }
        }
        let blinked = Frame::new(240, 200, blinked).expect("a frame");
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
        stitcher.push(page, 60).expect("f0");
        assert_eq!(stitcher.push(blinked, 60).expect("f1"), Next::Scroll);
        assert_eq!(stitcher.size(), (240, 200), "not one row was added");
        assert_eq!(stitcher.accepted(), 0);
    }

    /// `frame` with a square patch of `tone` near its top right, inside the band: a toast,
    /// an animation, a thumb -- something drawn over the page that does not scroll with it.
    fn patched(frame: Frame, tone: u8) -> Frame {
        let width = frame.width();
        let mut pixels = frame.pixels().to_vec();
        for y in 0..40u32 {
            for x in width - 48..width - 8 {
                let at = ((y * width + x) * 4) as usize;
                pixels[at..at + 4].copy_from_slice(&[tone, tone, tone, 255]);
            }
        }
        Frame::new(width, frame.height(), pixels).expect("a frame")
    }

    /// D105: a patch that changes while the page scrolls under it. It disagrees at the
    /// true offset and nowhere in particular, which cost the true peak its tie with a
    /// spurious one and dragged its correlation to 0.79, under the accept line -- while the
    /// pixels differed by a patch's worth and no more. The scroll is placed, and a frame
    /// in which only the patch changed is a page that did not move.
    #[test]
    fn a_patch_that_changes_over_a_scrolling_page_does_not_hide_the_scroll() {
        let step = 3 * crate::testing::RULE;
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
        let first = looking(Look::Calendar, 400, 300, 0, 9);
        assert_eq!(stitcher.push(first, 180).expect("f0"), Next::Scroll);
        let scrolled = patched(looking(Look::Calendar, 400, 300, step, 9), 90);
        assert_eq!(stitcher.push(scrolled, 180).expect("f1"), Next::Scroll);
        assert_eq!((stitcher.accepted(), stitcher.growth()), (1, step), "the scroll is placed");
        let flickered = patched(looking(Look::Calendar, 400, 300, step, 9), 200);
        assert_eq!(stitcher.push(flickered, 180).expect("f2"), Next::Scroll);
        assert_eq!(stitcher.accepted(), 1, "the patch alone is not a scroll");
        assert_eq!(stitcher.size(), (400, 300 + step));
    }

    /// D105: a notification standing over the page while it scrolls, as one stood over
    /// the user's frame of 2026-09-23. It agrees with itself at offset zero and with
    /// nothing at the offset the page moved, and over flush-left type -- where there is
    /// little else in the band to outvote it -- it turned eleven placements in twelve into
    /// misses. Left out of every comparison but zero's, as everything that stood still is
    /// (D107), it costs nothing, and it is drawn once, where it stood in the first frame.
    #[test]
    fn a_banner_standing_over_a_scrolling_page_does_not_hide_the_scroll() {
        for (seed, tall) in [(11u32, 60u32), (11, 100), (4, 60), (4, 100)] {
            for shift in [24u32, 90, 200] {
                let framed = |top: u32| {
                    let page = looking(Look::Flush, 480, 700, top, seed);
                    crate::testing::with_banner(&page, 150, 30, 180, tall)
                };
                let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
                stitcher.push(framed(0), shift).expect("f0");
                let next = stitcher.push(framed(shift), shift).expect("f1");
                let case = format!("seed {seed}, {tall} rows, shift {shift}");
                assert_eq!(next, Next::Scroll, "{case}");
                assert_eq!(stitcher.growth(), shift, "{case}");
                let page = looking(Look::Flush, 480, 700 + shift, 0, seed);
                let wanted = crate::testing::with_banner(&page, 150, 30, 180, tall);
                assert_eq!(stitcher.finish().expect("a result"), wanted, "{case}");
            }
        }
    }

    #[test]
    fn a_frame_of_another_size_is_refused_rather_than_stitched() {
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
        stitcher.push(window(64, 200, 0, 0), 60).expect("f0");
        assert!(matches!(
            stitcher.push(window(64, 180, 60, 0), 60),
            Err(StitchError::Mismatch { width: 64, height: 180 })
        ));
    }

    #[test]
    fn one_frame_finishes_as_that_frame() {
        let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
        let frame = window(64, 200, 0, 0);
        stitcher.push(frame.clone(), 60).expect("f0");
        assert_eq!(stitcher.size(), (64, 200));
        assert_eq!(stitcher.finish().expect("a result"), frame);
    }


    /// `docs/progress/M6-REAL-SESSION-CHECKLIST.md` §1 item 3's spreadsheet: a frozen
    /// first column, captured sideways. It is a sticky header turned on its side, which
    /// is the whole claim `spec/07` §1.2's "horizontal mode is the transpose" makes about
    /// it -- so it is drawn once, on the left, and the columns behind it are not.
    #[test]
    fn a_frozen_first_column_is_a_sticky_header_turned_on_its_side() {
        let mut stitcher = Stitcher::new(Direction::Right, Limits::default());
        for i in 0..4u32 {
            stitcher.push(sticky(64, 200, i * 50, 0, 24, 0).transpose(), 50).expect("a frame");
        }
        let fixed = stitcher.fixed().expect("a settled mask");
        assert_eq!((fixed.header, fixed.footer), (24, 0), "the frozen column, and nothing else");
        assert_eq!(stitcher.size(), (350, 64));
        let mut wanted = window(64, 24, 0, 0);
        wanted.stack(&window(64, 326, 24, 0)).expect("same width");
        assert_eq!(stitcher.finish().expect("a result"), wanted.transpose());
    }

    /// Paints `count` square patches at fixed *page* positions, as the avatars and
    /// preview images of a virtualised list arriving after the text they belong to: they
    /// scroll with the content, and they are only in the frames grabbed after they
    /// loaded.
    fn loaded(frame: &Frame, top: u32, side: u32, count: u32) -> Frame {
        let (width, height) = (frame.width(), frame.height());
        let mut pixels = frame.pixels().to_vec();
        for n in 0..count {
            let Some(y0) = (40 + n * 37).checked_sub(top) else { continue };
            let x0 = width / 4 + (n % 5) * side;
            for y in y0..(y0 + side).min(height) {
                for x in x0..(x0 + side).min(width) {
                    let at = ((y * width + x) * 4) as usize;
                    pixels[at..at + 3].copy_from_slice(&[250, 250, 250]);
                }
            }
        }
        Frame::new(width, height, pixels).expect("the size it was built at")
    }

    /// §1 item 2's Slack, in the part of it the matcher can be asked about: a list whose
    /// images arrive a frame or two after the text. They change the overlap the match is
    /// made over -- which is exactly what `FIT` is there to notice -- so enough of them
    /// at once is a miss rather than a confident placement.
    ///
    /// The point of the test is what happens next. A miss keeps the *previous* frame as
    /// the anchor (D75), so the following frame is matched against a frame nobody has
    /// doubted, at twice the distance, and the capture comes out the full height with
    /// every row in it. A few images cost nothing; a screenful of them costs a retry.
    #[test]
    fn images_arriving_late_cost_a_retry_and_not_a_row() {
        for count in [1u32, 8, 64] {
            let mut stitcher = Stitcher::new(Direction::Down, Limits::default());
            for i in 0..4u32 {
                let top = i * 90;
                let plain = looking(Look::Prose, 240, 300, top, 5);
                let frame = if i == 0 { plain } else { loaded(&plain, top, 24, count) };
                let next = stitcher.push(frame, 90).expect("a frame");
                assert_ne!(next, Next::Stop(End::Unmatched), "{count} images, frame {i}");
            }
            assert_eq!(stitcher.size(), (240, 570), "{count} images: every row of the page");
        }
    }


}
