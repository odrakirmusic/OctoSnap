// SPDX-License-Identifier: GPL-3.0-or-later

//! Boxes into lines, lines into paragraphs, paragraphs into the two strings `spec/07`
//! §2.1 offers.
//!
//! > Two shortcuts/variants: keep line breaks vs. join into one line. Long text ->
//! > paragraphs: lines are grouped by vertical gap (> 1.4 x line height starts a new
//! > paragraph); within a paragraph lines are joined with spaces.

use crate::{Bounds, Word};

/// `spec/07` §2.1's number: a gap wider than this many line heights starts a paragraph.
pub const PARAGRAPH: f64 = 1.4;

/// How much of the shorter box has to share rows with the line for it to join it. [P]
///
/// More than half, so that a 40 px heading and the 14 px line under it -- which touch by a
/// few pixels when the detector is generous -- stay two lines, while a superscript, a
/// mis-sized box or a second column sitting a few pixels high stays on the line it belongs
/// to. The measure is *of the shorter box* rather than of either one in particular: a tall
/// box has no business claiming a short one just because it covers all of it.
const SHARE: f64 = 0.5;

/// One row of text: every box the detector put on it, read left to right.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    pub bounds: Bounds,
}

/// Lines with no gap between them wide enough to be a paragraph break.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Paragraph {
    pub lines: Vec<Line>,
}

impl Paragraph {
    #[must_use]
    pub fn bounds(&self) -> Bounds {
        self.lines.iter().fold(Bounds::default(), |all, line| all.union(line.bounds))
    }
}

/// Which of `spec/07` §2.1's two shortcuts is asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Breaks {
    /// Keep the line breaks: what is on the screen, line for line.
    #[default]
    Keep,
    /// Join into one line: `spec/07` §2.1's paragraph formatting, which reflows each
    /// paragraph into a single line so a wrapped paragraph pastes as a paragraph.
    Join,
}

/// What the engine read, in the order a person reads it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Shaped {
    pub paragraphs: Vec<Paragraph>,
}

impl Shaped {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.paragraphs.iter().all(|p| p.lines.is_empty())
    }

    /// The first line, which is what `spec/07` §2.1's notification shows as its preview.
    #[must_use]
    pub fn first_line(&self) -> Option<&str> {
        self.paragraphs.iter().flat_map(|p| p.lines.first()).map(|l| l.text.as_str()).next()
    }

    /// The text for one of the two shortcuts.
    ///
    /// A paragraph break is a blank line in both variants. That is not a line break being
    /// kept or dropped -- it is the gap that was on the screen, and dropping it would run
    /// two paragraphs together in the variant whose whole point is not to.
    #[must_use]
    pub fn text(&self, breaks: Breaks) -> String {
        let within = match breaks {
            Breaks::Keep => "\n",
            Breaks::Join => " ",
        };
        let mut out = String::new();
        for paragraph in self.paragraphs.iter().filter(|p| !p.lines.is_empty()) {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            for (i, line) in paragraph.lines.iter().enumerate() {
                if i > 0 {
                    out.push_str(within);
                }
                out.push_str(&line.text);
            }
        }
        out
    }
}

/// Groups a detector's boxes into lines and paragraphs.
#[must_use]
pub fn shape(words: Vec<Word>) -> Shaped {
    let mut words: Vec<Word> = words
        .into_iter()
        .map(|mut word| {
            word.text = word.text.trim().to_string();
            word
        })
        .filter(|word| !word.text.is_empty() && !word.bounds.is_empty())
        .collect();
    // Reading order, before anything is asked about neighbours: a detector answers in
    // whatever order its boxes came off the model, which is not top to bottom.
    words.sort_by_key(|word| (word.bounds.top(), word.bounds.left()));

    let mut lines = rows(words);
    lines.sort_by_key(|line| (line.bounds.top(), line.bounds.left()));
    Shaped { paragraphs: paragraphs(lines) }
}

/// Boxes that share enough rows to be the same line of type, joined left to right.
///
/// Always with a single space, even where the boxes nearly touch. A detector splits a line
/// where the ink stops, so the gap it split at may be a word space, a tab stop or a table
/// column, and it does not say which. A space that should not be there is visible in the
/// paste and a person deletes it; a join that should not be there reads as a word and
/// nobody notices.
fn rows(words: Vec<Word>) -> Vec<Line> {
    let mut lines: Vec<Vec<Word>> = Vec::new();
    let mut spans: Vec<Bounds> = Vec::new();
    for word in words {
        let mine = spans.iter().enumerate().rev().find(|(_, span)| {
            let shorter = span.height.min(word.bounds.height);
            f64::from(span.shared_rows(word.bounds)) > f64::from(shorter) * SHARE
        });
        match mine {
            Some((i, _)) => {
                spans[i] = spans[i].union(word.bounds);
                lines[i].push(word);
            }
            None => {
                spans.push(word.bounds);
                lines.push(vec![word]);
            }
        }
    }
    lines
        .into_iter()
        .zip(spans)
        .map(|(mut row, bounds)| {
            row.sort_by_key(|word| word.bounds.left());
            let text = row.into_iter().map(|word| word.text).collect::<Vec<_>>().join(" ");
            Line { text, bounds }
        })
        .collect()
}

/// Lines split wherever they sit `PARAGRAPH` times further apart than the page's own
/// lines do, or wherever the space between their boxes is wider than the ink in them.
///
/// `spec/07` §2.1 says "> 1.4 x line height starts a new paragraph" and leaves both
/// numbers to be read. Two readings, because each covers what the other misses.
///
/// **Pitch against the median pitch** is the main one: the distance from one line's top
/// to the next, against the middle such distance on the page. It is the reading a typesetter
/// would give -- a paragraph break is one and a half to two line pitches where a line
/// break is one -- and it does not care how tall the ink is, which matters because the
/// detector's boxes are grown back out by `unclip_ratio` and are very nearly the pitch
/// already. That is what this could not do before: a 17 px page set on a 31 px pitch with
/// a blank line between paragraphs left a 34 px gap against a 39 px threshold, so two of
/// its three paragraph breaks were found and one was not, on the same page, for no reason
/// the page could see (2026-09-16, D95).
///
/// **The gap against the median ink** is kept beside it, because it is the only thing that
/// can answer a page with two lines on it: one pitch is its own median and never 1.4 times
/// itself. It fires rarely -- it needs a pitch of nearly two and a half times the ink --
/// and it has been right every time it has.
///
/// Medians rather than each line's own numbers, because a line of dashes, a line of digits
/// and a line with no descender all have honest heights that are not the page's. The lower
/// median of the pitches, because the pitch of a paragraph break is always the longer one
/// and a page of two paragraphs would otherwise take the break as its own ordinary pitch.
fn paragraphs(lines: Vec<Line>) -> Vec<Paragraph> {
    if lines.is_empty() {
        return Vec::new();
    }
    let middle = |mut of: Vec<i32>| {
        of.sort_unstable();
        of.get(of.len().saturating_sub(1) / 2).copied().unwrap_or(0)
    };
    let ink = f64::from(middle(lines.iter().map(|line| line.bounds.height).collect()));
    let pitches: Vec<i32> =
        lines.windows(2).map(|pair| (pair[1].bounds.top() - pair[0].bounds.top()).max(0)).collect();
    let pitch = f64::from(middle(pitches));
    let (widest_gap, widest_pitch) = (ink * PARAGRAPH, pitch * PARAGRAPH);

    let mut out: Vec<Paragraph> = Vec::new();
    let mut current = Paragraph::default();
    let mut previous: Option<Bounds> = None;
    for line in lines {
        if let Some(above) = previous {
            let gap = f64::from((line.bounds.top() - above.bottom()).max(0));
            let step = f64::from((line.bounds.top() - above.top()).max(0));
            let apart = gap > widest_gap || (pitch > 0.0 && step > widest_pitch);
            if apart && !current.lines.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        }
        previous = Some(line.bounds);
        current.lines.push(line);
    }
    if !current.lines.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A line of ordinary type: 14 px of ink on a 20 px pitch, which is what a browser at
    /// 16 px with its default leading gives.
    const INK: i32 = 14;

    fn word(text: &str, x: i32, y: i32, width: i32, height: i32) -> Word {
        Word::new(text, Bounds::new(x, y, width, height), 0.99)
    }

    /// A page laid out the way a browser lays one out: 14 px of ink on a 20 px pitch, a
    /// blank line's worth of margin between paragraphs, and every word in its own box,
    /// which is the most a detector ever splits a line into.
    fn laid_out(paragraphs: &[&[&str]]) -> Vec<Word> {
        let mut words = Vec::new();
        let mut top = 40;
        for paragraph in paragraphs {
            for line in *paragraph {
                let mut left = 30;
                for text in line.split(' ') {
                    let width = 8 * text.len() as i32;
                    words.push(word(text, left, top, width, INK));
                    left += width + 8;
                }
                top += 20;
            }
            top += 20;
        }
        words
    }

    /// One line of type per entry, laid out down the page at the tops given.
    fn page(lines: &[(i32, &str)]) -> Vec<Word> {
        lines
            .iter()
            .map(|(top, text)| word(text, 10, *top, 8 * text.len() as i32, INK))
            .collect()
    }

    #[test]
    fn boxes_that_share_a_row_are_one_line_read_left_to_right() {
        let shaped = shape(vec![
            word("three", 200, 100, 40, INK),
            word("one", 10, 101, 30, INK),
            word("two", 100, 99, 30, INK),
        ]);
        assert_eq!(shaped.text(Breaks::Keep), "one two three");
    }

    #[test]
    fn a_heading_whose_box_touches_the_line_under_it_is_still_two_lines() {
        // A 42 px heading whose box reaches two pixels into the 14 px line below it: they
        // share rows, but only two of the shorter one's fourteen.
        let shaped = shape(vec![
            word("Heading", 10, 100, 200, 42),
            word("body text", 10, 140, 100, INK),
        ]);
        assert_eq!(shaped.text(Breaks::Keep), "Heading\nbody text");
    }

    #[test]
    fn a_heading_is_its_own_paragraph_because_of_the_margin_under_it() {
        // The heading's ink is 38 px; the body is three lines of 14 on a 20 px pitch, so
        // the median line is the body's and the heading's margin clears it.
        let shaped = shape(vec![
            word("A Heading", 10, 100, 200, 38),
            word("body one", 10, 160, 100, INK),
            word("body two", 10, 180, 100, INK),
            word("body three", 10, 200, 100, INK),
        ]);
        assert_eq!(shaped.paragraphs.len(), 2, "{shaped:?}");
        assert_eq!(shaped.text(Breaks::Join), "A Heading\n\nbody one body two body three");
    }

    #[test]
    fn a_wrapped_paragraph_is_one_paragraph_however_many_lines_it_took() {
        let shaped = shape(page(&[
            (100, "the quick brown fox"),
            (120, "jumps over the lazy"),
            (140, "dog and keeps going"),
        ]));
        assert_eq!(shaped.paragraphs.len(), 1);
        assert_eq!(
            shaped.text(Breaks::Join),
            "the quick brown fox jumps over the lazy dog and keeps going"
        );
    }

    #[test]
    fn a_blank_line_between_paragraphs_starts_a_new_one() {
        // A plain file: the same 20 px pitch, doubled where the blank line is.
        let shaped = shape(page(&[
            (100, "first line"),
            (120, "still the first"),
            (160, "second para"),
            (180, "still the second"),
        ]));
        assert_eq!(shaped.paragraphs.len(), 2);
        assert_eq!(
            shaped.text(Breaks::Join),
            "first line still the first\n\nsecond para still the second"
        );
    }

    #[test]
    fn a_web_pages_paragraph_margin_is_a_break_and_its_leading_is_not() {
        // 16 px type, 24 px line boxes, a collapsed 1em margin between paragraphs.
        let shaped = shape(
            [100, 124, 148, 188, 212]
                .iter()
                .enumerate()
                .map(|(i, top)| word(&format!("line {i}"), 10, *top, 90, 16))
                .collect(),
        );
        assert_eq!(shaped.paragraphs.len(), 2, "{shaped:?}");
        assert_eq!(shaped.paragraphs[0].lines.len(), 3);
        assert_eq!(shaped.paragraphs[1].lines.len(), 2);
    }

    /// D95, as the page that found it. The detector hands back boxes grown out by
    /// `unclip_ratio`, so a 17 px page set on a 31 px pitch has boxes 28 px tall and the
    /// space between them is 3 px -- while the blank line between two paragraphs is 34,
    /// against a threshold of 39. Measured by the ink, two of that page's three breaks
    /// were found and one was not. Measured by the pitch, all three are.
    #[test]
    fn a_paragraph_break_is_found_however_little_white_the_boxes_leave() {
        let tall = |top: i32| word("line", 10, top, 900, 28);
        let shaped = shape(vec![tall(100), tall(131), tall(162), tall(224), tall(255)]);
        assert_eq!(shaped.paragraphs.len(), 2, "{shaped:?}");
        assert_eq!(shaped.paragraphs[0].lines.len(), 3);
        assert_eq!(shaped.paragraphs[1].lines.len(), 2);
    }

    /// And the other half of it: boxes that nearly touch on a page with nothing but
    /// ordinary leading in it are one paragraph, not five.
    #[test]
    fn evenly_set_lines_are_one_paragraph_however_little_white_they_leave() {
        let tall = |top: i32| word("line", 10, top, 900, 28);
        let shaped = shape(vec![tall(100), tall(131), tall(162), tall(193), tall(224)]);
        assert_eq!(shaped.paragraphs.len(), 1, "{shaped:?}");
    }

    #[test]
    fn double_spaced_type_is_still_one_paragraph() {
        // 14 px of ink on a 28 px pitch: a 14 px gap, against a 19.6 px threshold.
        let shaped = shape(page(&[(100, "one"), (128, "two"), (156, "three")]));
        assert_eq!(shaped.paragraphs.len(), 1, "{shaped:?}");
        // And the break in the same page is still a break.
        let broken = shape(page(&[(100, "one"), (128, "two"), (184, "three")]));
        assert_eq!(broken.paragraphs.len(), 2, "{broken:?}");
    }

    #[test]
    fn the_two_shortcuts_differ_inside_a_paragraph_and_nowhere_else() {
        let words = page(&[(100, "alpha"), (120, "beta"), (160, "gamma")]);
        let shaped = shape(words);
        assert_eq!(shaped.text(Breaks::Keep), "alpha\nbeta\n\ngamma");
        assert_eq!(shaped.text(Breaks::Join), "alpha beta\n\ngamma");
    }

    #[test]
    fn the_boxes_are_read_down_the_page_whatever_order_they_arrived_in() {
        let mut words = page(&[(100, "first"), (120, "second"), (140, "third")]);
        words.reverse();
        assert_eq!(shape(words).text(Breaks::Keep), "first\nsecond\nthird");
    }

    #[test]
    fn a_box_the_engine_read_as_blank_is_not_a_line() {
        let shaped = shape(vec![
            word("real", 10, 100, 40, INK),
            word("   ", 60, 100, 40, INK),
            word("text", 10, 120, 40, INK),
        ]);
        assert_eq!(shaped.text(Breaks::Keep), "real\ntext");
    }

    #[test]
    fn nothing_recognised_is_nothing_shaped() {
        let shaped = shape(Vec::new());
        assert!(shaped.is_empty());
        assert_eq!(shaped.text(Breaks::Keep), "");
        assert_eq!(shaped.first_line(), None);
    }

    #[test]
    fn the_first_line_is_what_the_notification_previews() {
        let shaped = shape(page(&[(100, "Text copied"), (120, "and more")]));
        assert_eq!(shaped.first_line(), Some("Text copied"));
    }

    #[test]
    fn a_table_row_reads_across_before_it_reads_down() {
        let shaped = shape(vec![
            word("Name", 10, 100, 60, INK),
            word("Size", 300, 100, 60, INK),
            word("frame.rs", 10, 120, 80, INK),
            word("12 kB", 300, 120, 60, INK),
        ]);
        assert_eq!(shaped.text(Breaks::Keep), "Name Size\nframe.rs 12 kB");
    }

    #[test]
    fn a_page_laid_out_from_paragraphs_shapes_back_into_those_paragraphs() {
        let page: &[&[&str]] = &[
            &["Select an area and the text in it is", "recognised and copied."],
            &[
                "Two shortcuts: one keeps the line",
                "breaks, the other joins each",
                "paragraph into a line.",
            ],
            &["Links become clickable."],
        ];
        // Reversed, because a detector answers in the order its boxes came off the model.
        let mut words = laid_out(page);
        words.reverse();
        let shaped = shape(words);

        let joined = |within: &str| {
            page.iter().map(|p| p.join(within)).collect::<Vec<_>>().join("\n\n")
        };
        assert_eq!(shaped.paragraphs.len(), page.len(), "{shaped:?}");
        assert_eq!(shaped.text(Breaks::Keep), joined("\n"));
        assert_eq!(shaped.text(Breaks::Join), joined(" "));
    }
}
