// SPDX-License-Identifier: GPL-3.0-or-later

//! Synthetic scroll pages: `spec/10` §5's "fixtures (synthetic scroll pages, images)".
//!
//! A page here is infinite and deterministic -- row `r` looks the way it looks because of
//! `r`, not because of where it happens to be in a frame -- so a frame is a *window* onto
//! it and scrolling is arithmetic. That is what makes the stitcher's tests say what they
//! mean: "the capture of a page scrolled in sixties is the page" is a comparison against
//! the page itself, not against a second stitch of the same frames.
//!
//! Each row is a bar pattern with its own light and dark tone, which gives the matcher
//! both things it looks for: a mean that varies row to row, and edges that vary with it.

use crate::frame::Frame;

/// What the page is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// A bar pattern, different in every row: signal everywhere, which is the easy case
    /// and the one that makes an assertion about an offset unambiguous.
    Bars,
    /// Lines of type on a light ground with blank leading between them: nearly half the
    /// rows carry nothing at all, and the ones that do repeat on a 20-row pitch. This is
    /// what a README looks like to the matcher, and it is where a matcher that leans on
    /// row-to-row novelty slides a paragraph by one line.
    Prose,
    /// The same, inverted: `spec/07` §7 item 4's dark-mode terminal.
    Terminal,
    /// Type set to the *same ink in every line*: a table, a code listing, a column of
    /// `ls` output, the file the matcher first met on a real editor. Each line is a
    /// rotation of the one before, so its mean luminance and its edge energy are
    /// identical row for row and the row signatures cannot tell one line from another --
    /// only the pixels can.
    Listing,
    /// A calendar's hour grid: a plain ground with a hairline rule every [`RULE`] rows and
    /// nothing else at all. Not merely *even* like [`Look::Listing`], whose lines differ
    /// pixel for pixel -- **empty**, so the pixels cannot separate one rule from the next
    /// either and every offset a whole number of rules apart is the very same picture.
    /// This is what defeated the matcher on a real Google Calendar (D94).
    Ruled,
    /// The same grid with a day's worth of appointments on it: a block of a third colour
    /// in about one hour in three, inside the central column band. The page D94 came
    /// from, in the part of the day that has something in it -- so the rules still tie at
    /// every hour and the events are the only thing that says which hour is which.
    Calendar,
    /// Type set flush left on a dark ground, as a dialog's list of fields is: every line
    /// starts at the same column and most end inside the first third of the width, with
    /// the odd long value running on past the middle. The middle half of a frame of this
    /// is nearly all ground, which is what retired the centred band (D105).
    Flush,
}

/// One frame of the page: rows `[top, top + height)`.
pub fn window(width: u32, height: u32, top: u32, seed: u32) -> Frame {
    looking(Look::Bars, width, height, top, seed)
}

/// One frame of a page of the given look.
pub fn looking(look: Look, width: u32, height: u32, top: u32, seed: u32) -> Frame {
    rows(look, width, height, |y| top + y, seed)
}

/// One frame of a page the compositor resampled: the rows from `eighths` eighths of a row
/// down, each row of the frame blended from the two rows of the page it falls between.
///
/// A window drawn at twice the scale and shown at 1.25 is drawn in rows of its own that
/// are five eighths of a row of the screen, so a scroll by a whole number of *its* rows
/// lands between the screen's -- and the compositor fills each row of the screen from the
/// rows it falls between. This is that, done the simple way: linear, and along the scroll
/// only, because across it the two frames were blurred alike.
pub fn resampled(look: Look, width: u32, height: u32, eighths: u32, seed: u32) -> Frame {
    let page = looking(look, width, height + 1, eighths / 8, seed);
    let (far, near) = (eighths % 8, 8 - eighths % 8);
    let mut pixels = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for y in 0..height {
        for (a, b) in page.row(y).iter().zip(page.row(y + 1)) {
            let tone = (u32::from(*a) * near + u32::from(*b) * far + 4) / 8;
            pixels.push(u8::try_from(tone).unwrap_or(u8::MAX));
        }
    }
    Frame::new(width, height, pixels).expect("a frame of the size it was built at")
}

/// A frame of a page with sticky rows: `header` rows pinned at the top and `footer` rows
/// pinned at the bottom, both showing the same content in every frame.
pub fn sticky(width: u32, height: u32, top: u32, seed: u32, header: u32, footer: u32) -> Frame {
    rows(
        Look::Bars,
        width,
        height,
        |y| {
            if y < header || y >= height - footer {
                // The rows a sticky region shows are the rows it showed at the top of the
                // page, which is what makes the first frame of a sticky page identical to
                // a plain window onto it.
                y
            } else {
                top + y
            }
        },
        seed,
    )
}

/// `spec/07` §2.1's "> 1.4 x line height starts a new paragraph" has a sibling here: the
/// pitch a page of type falls into. Eleven rows of glyphs and nine of leading is ordinary
/// 16 px body text, and it is the period the matcher must not mistake for the scroll.
const PITCH: u32 = 20;
const INK: u32 = 11;

/// Cells in [`Look::Listing`]'s pattern. Sixteen three-pixel cells is 48 px, so a width
/// that is a multiple of 48 holds a whole number of them at every rotation.
const CELLS: u32 = 16;

fn rows(look: Look, width: u32, height: u32, at: impl Fn(u32) -> u32, seed: u32) -> Frame {
    let mut pixels = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for y in 0..height {
        match look {
            Look::Bars => bars(&mut pixels, width, at(y), seed),
            Look::Prose => type_set(&mut pixels, width, at(y), seed, false, false),
            Look::Terminal => type_set(&mut pixels, width, at(y), seed, true, false),
            Look::Listing => type_set(&mut pixels, width, at(y), seed, false, true),
            Look::Ruled => ruled(&mut pixels, width, at(y)),
            Look::Calendar => calendar(&mut pixels, width, at(y), seed),
            Look::Flush => flush(&mut pixels, width, at(y), seed),
        }
    }
    Frame::new(width, height, pixels).expect("a frame of the size it was built at")
}

/// [`Look::Ruled`]'s pitch: one hour of Google Calendar's day view at 100 %, near enough.
pub const RULE: u32 = 58;

/// One row of an hour grid: the rule, or the ground it is ruled on.
fn ruled(out: &mut Vec<u8>, width: u32, row: u32) {
    let tone = if row.is_multiple_of(RULE) { 58u8 } else { 26u8 };
    for _ in 0..width {
        out.extend_from_slice(&[tone, tone, tone, 255]);
    }
}

/// One row of a day with appointments in it.
///
/// The block spans the fifth to the seventeenth twentieth of the width, which puts it
/// inside the centred half the signatures are taken over -- an event drawn in the margin
/// would be a page the matcher cannot see and a fixture that proves nothing.
fn calendar(out: &mut Vec<u8>, width: u32, row: u32, seed: u32) {
    let (hour, within) = (row / RULE, row % RULE);
    let bits = hash(hour, seed);
    let top = 6 + bits % 20;
    let event = bits.is_multiple_of(3) && within >= top && within < top + 10;
    let (from, to) = (width / 5, width * 17 / 20);
    for x in 0..width {
        let tone = if event && x >= from && x < to {
            110u8
        } else if within == 0 {
            58
        } else {
            26
        };
        out.extend_from_slice(&[tone, tone, tone, 255]);
    }
}

/// [`Look::Flush`]'s line pitch: a list row of a GTK dialog at 1x, near enough.
pub const FLUSH_PITCH: u32 = 24;

/// One row of [`Look::Flush`]: eleven rows of glyphs in a pitch of twenty-four, from a
/// sixteenth of the way in to wherever the line's own hash says it ends -- inside the first
/// quarter of the width, but for one line in twenty-four that runs on to three quarters.
/// The user's frame of 2026-09-23 had three such lines in 1073 rows, and nothing else in
/// the middle half at all.
fn flush(out: &mut Vec<u8>, width: u32, row: u32, seed: u32) {
    let (ground, ink) = (30u8, 220u8);
    let (line, within) = (row / FLUSH_PITCH, row % FLUSH_PITCH);
    let bits = hash(line, seed);
    let start = width / 16;
    let end = if bits.is_multiple_of(24) {
        width * 3 / 4
    } else {
        start + width / 16 + (bits >> 5) % (width / 10).max(1)
    };
    let inked = (6..17).contains(&within);
    for x in 0..width {
        let glyph = hash(x / 3, line ^ seed.wrapping_add(within / 4)) & 1 == 1;
        let on = inked && x >= start && x < end && glyph && (x / 3) % 7 != 6;
        let tone = if on { ink } else { ground };
        out.extend_from_slice(&[tone, tone, tone, 255]);
    }
}

/// `frame` with its first `columns` columns replaced by a sidebar that does not scroll:
/// the same rows of its own at the same place in every frame, as a docs site's navigation
/// or a mail client's folder list is while the page beside it moves.
pub fn with_sidebar(frame: &Frame, columns: u32, seed: u32) -> Frame {
    let (width, height) = (frame.width(), frame.height());
    let mut pixels = frame.pixels().to_vec();
    for y in 0..height {
        let bits = hash(y / 18, seed ^ 0x5eed);
        let inked = y % 18 < 9 && !bits.is_multiple_of(5);
        for x in 0..columns.min(width) {
            let on = inked && (bits >> (x % 23)) & 1 == 1;
            let tone = if on { 60u8 } else { 238 };
            let at = ((y * width + x) * 4) as usize;
            pixels[at..at + 4].copy_from_slice(&[tone, tone, tone, 255]);
        }
    }
    Frame::new(width, height, pixels).expect("a frame of the size it came in at")
}

/// `frame` with a notification standing over it at `(x, y)`, `width` by `height`: a panel
/// with a hairline edge, an icon, two lines of text and a close button, drawn the same at
/// the same place in every frame while the page under it scrolls -- as GNOME's banners
/// are, and a chat button, and the pill of a capture whose selection fills the screen.
pub fn with_banner(frame: &Frame, x: u32, y: u32, width: u32, height: u32) -> Frame {
    let (columns, rows) = (frame.width(), frame.height());
    let mut pixels = frame.pixels().to_vec();
    let lines = [(height / 5, width * 3 / 4), (height / 5 + 16, width / 2)];
    for dy in 0..height.min(rows.saturating_sub(y)) {
        for dx in 0..width.min(columns.saturating_sub(x)) {
            let edge = dx == 0 || dy == 0 || dx + 1 == width || dy + 1 == height;
            let icon = (12..36).contains(&dx) && (12..36).contains(&dy);
            let close = dx + 26 >= width && dx + 14 < width && (12..24).contains(&dy);
            let text = lines.iter().any(|&(top, end)| {
                (top..top + 10).contains(&dy) && (48..end).contains(&dx)
            }) && hash(dx / 3, dy / 4) & 1 == 1;
            let tone = if edge {
                140u8
            } else if icon || close || text {
                235
            } else {
                64
            };
            let at = (((y + dy) * columns + x + dx) * 4) as usize;
            pixels[at..at + 4].copy_from_slice(&[tone, tone, tone, 255]);
        }
    }
    Frame::new(columns, rows, pixels).expect("a frame of the size it came in at")
}

/// `frame` with a flat rectangle of `tone` over it: a panel with no detail at all, like
/// the preview of a capture's pill before anything is in it, or a card showing a blank
/// capture.
pub fn with_panel(frame: &Frame, x: u32, y: u32, width: u32, height: u32, tone: u8) -> Frame {
    let (columns, rows) = (frame.width(), frame.height());
    let mut pixels = frame.pixels().to_vec();
    for dy in 0..height.min(rows.saturating_sub(y)) {
        for dx in 0..width.min(columns.saturating_sub(x)) {
            let at = (((y + dy) * columns + x + dx) * 4) as usize;
            pixels[at..at + 4].copy_from_slice(&[tone, tone, tone, 255]);
        }
    }
    Frame::new(columns, rows, pixels).expect("a frame of the size it came in at")
}

/// `frame` with a floating toolbar standing over it at `(x, y)`: a tall panel with an
/// icon every forty rows, the same at the same place in every frame -- a table of contents
/// that follows the reader down a page that goes on either side of it.
pub fn with_toolbar(frame: &Frame, x: u32, y: u32, width: u32, height: u32) -> Frame {
    let (columns, rows) = (frame.width(), frame.height());
    let mut pixels = frame.pixels().to_vec();
    for dy in 0..height.min(rows.saturating_sub(y)) {
        for dx in 0..width.min(columns.saturating_sub(x)) {
            let edge = dx == 0 || dx + 1 == width;
            let icon = (8..width.saturating_sub(8)).contains(&dx) && (8..32).contains(&(dy % 40));
            let glyph = icon && hash(dx / 2, dy / 3 + dy / 40 * 7) & 1 == 1;
            let tone = if edge { 150u8 } else if glyph { 225 } else { 72 };
            let at = (((y + dy) * columns + x + dx) * 4) as usize;
            pixels[at..at + 4].copy_from_slice(&[tone, tone, tone, 255]);
        }
    }
    Frame::new(columns, rows, pixels).expect("a frame of the size it came in at")
}

fn bars(out: &mut Vec<u8>, width: u32, row: u32, seed: u32) {
    let bits = hash(row, seed);
    let light = 200 + u8::try_from(bits % 50).unwrap_or(0);
    let dark = 10 + u8::try_from((bits >> 8) % 50).unwrap_or(0);
    for x in 0..width {
        // 29 rather than 32, so the bar pattern does not repeat exactly every word and a
        // wide frame keeps telling its columns apart.
        let tone = if (bits >> (x % 29)) & 1 == 1 { dark } else { light };
        out.extend_from_slice(&[tone, tone, tone, 255]);
    }
}

/// One row of a page of type: blank leading, or glyph columns from the line's own hash.
///
/// `even` rotates one pattern instead of hashing a new one per line. A rotation keeps both
/// the popcount and the number of transitions, so every line weighs the same to the row
/// signatures -- which is the whole point of [`Look::Listing`].
fn type_set(out: &mut Vec<u8>, width: u32, row: u32, seed: u32, dark_mode: bool, even: bool) {
    let (ground, ink) = if dark_mode { (26u8, 222u8) } else { (246u8, 34u8) };
    let within = row % PITCH;
    if within >= INK {
        for _ in 0..width {
            out.extend_from_slice(&[ground, ground, ground, 255]);
        }
        return;
    }
    let line = row / PITCH;
    let bits = hash(line, seed);
    let word = hash(0, seed) % (1 << CELLS);
    // The top and bottom row of a line of type is a stem end or an overshoot, never a
    // full glyph, so it reads lighter -- which is what stops every row of a line looking
    // identical and gives the +-2 px refine something to land on.
    let faint = within == 0 || within + 1 == INK;
    for x in 0..width {
        let on = if even {
            // One pattern of `CELLS` cells, turned by one cell per line. A frame's width is
            // a whole number of patterns whichever way it is turned, so every line carries
            // exactly the same ink and exactly the same edges: the row signatures cannot
            // tell two lines apart, and the pixels can.
            (word >> ((x / 3 + line) % CELLS)) & 1 == 1
        } else {
            // Three-pixel glyph columns, and a gap wherever the word ends.
            (bits >> ((x / 3) % 29)) & 1 == 1 && (x / 3) % 11 != 10
        };
        let tone = if !on {
            ground
        } else if faint {
            u8::try_from((u32::from(ground) + u32::from(ink)) / 2).unwrap_or(ink)
        } else {
            ink
        };
        out.extend_from_slice(&[tone, tone, tone, 255]);
    }
}

/// A row's look, from its number. Any decent avalanche would do; this one is `splitmix`'s
/// finaliser, which spreads single-bit differences across the whole word -- so row 40 and
/// row 41 look nothing like each other, which is the property the matcher is tested on.
fn hash(row: u32, seed: u32) -> u32 {
    let mut x = row.wrapping_mul(0x9E37_79B1).wrapping_add(seed.wrapping_mul(0x85EB_CA6B)).wrapping_add(0x1234_5678);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    x
}
