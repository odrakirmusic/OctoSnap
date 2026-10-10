// SPDX-License-Identifier: GPL-3.0-or-later

//! `ANN-27` (D180): the text in a picture as something to select, copy and follow, the way
//! the text in a document is.
//!
//! The engine answers in boxes, and a box can hold several words, so each box is cut into
//! its words by their share of its characters, as [`crate::sensitive`] places what it
//! covers. The words go into lines and the lines into paragraphs as a text capture's do
//! ([`crate::shape`]). A selection is every word in reading order from one word to
//! another, whichever of the two comes first, which is how a drag selects in a document.

use crate::shape::{self, Breaks, Line};
use crate::{Bounds, Word, links};

/// Where a word is: its line, and its place on the line. Ordered as the text is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub line: usize,
    pub word: usize,
}

/// One word and its place in the picture, in the picture's own pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveWord {
    pub text: String,
    pub bounds: Bounds,
}

/// A line's words, left to right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveLine {
    pub words: Vec<LiveWord>,
    pub bounds: Bounds,
    /// A paragraph starts here, so the text has a blank line before it.
    pub starts_paragraph: bool,
}

/// A link and the words it is written across.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveLink {
    /// What to open: a URL, or `mailto:` and an address.
    pub target: String,
    pub from: Pos,
    pub to: Pos,
}

/// A picture's text, word by word.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Page {
    pub lines: Vec<LiveLine>,
    pub links: Vec<LiveLink>,
}

impl Page {
    /// The text an engine read, in lines, paragraphs and words.
    #[must_use]
    pub fn new(boxes: &[Word]) -> Self {
        let mut lines: Vec<LiveLine> = shape::rows_of(boxes)
            .into_iter()
            .map(|row| {
                let words: Vec<LiveWord> = row.iter().flat_map(|b| split(b)).collect();
                let bounds = row.iter().fold(row[0].bounds, |all, b| all.union(b.bounds));
                LiveLine { words, bounds, starts_paragraph: false }
            })
            .filter(|line| !line.words.is_empty())
            .collect();
        lines.sort_by_key(|line| (line.bounds.top(), line.bounds.left()));

        // Paragraphs as a text capture finds them, from the lines' text and boxes.
        let shaped: Vec<Line> =
            lines.iter().map(|line| Line { text: line_text(line), bounds: line.bounds }).collect();
        let mut at = 0;
        for paragraph in shape::paragraphs(shaped) {
            if let Some(line) = lines.get_mut(at) {
                line.starts_paragraph = at > 0;
            }
            at += paragraph.lines.len();
        }

        let mut page = Self { lines, links: Vec::new() };
        page.links = page.find_links();
        page
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The page without the words `hidden` answers yes for, as if they had never been read
    /// (D188): they are not marked, selected, copied or followed. A line left with no words
    /// goes, a paragraph whose first lines went starts at the first one left, and the links
    /// are found again in what is left, so one written partly under a redaction is no link.
    #[must_use]
    pub fn without(&self, hidden: impl Fn(&LiveWord) -> bool) -> Self {
        let mut lines: Vec<LiveLine> = Vec::new();
        let mut starts = false;
        for line in &self.lines {
            starts |= line.starts_paragraph;
            let words: Vec<LiveWord> = line.words.iter().filter(|w| !hidden(w)).cloned().collect();
            let Some(first) = words.first() else { continue };
            let bounds = words.iter().fold(first.bounds, |all, w| all.union(w.bounds));
            lines.push(LiveLine { words, bounds, starts_paragraph: starts && !lines.is_empty() });
            starts = false;
        }
        let mut page = Self { lines, links: Vec::new() };
        page.links = page.find_links();
        page
    }

    #[must_use]
    pub fn word(&self, at: Pos) -> Option<&LiveWord> {
        self.lines.get(at.line)?.words.get(at.word)
    }

    /// The first word and the last, which is the whole page as a selection.
    #[must_use]
    pub fn all(&self) -> Option<(Pos, Pos)> {
        let last = self.lines.len().checked_sub(1)?;
        let words = self.lines[last].words.len().checked_sub(1)?;
        Some((Pos { line: 0, word: 0 }, Pos { line: last, word: words }))
    }

    /// The word under a point, in the picture's pixels.
    #[must_use]
    pub fn at(&self, x: f64, y: f64) -> Option<Pos> {
        self.lines.iter().enumerate().find_map(|(l, line)| {
            if !spans(line.bounds.top(), line.bounds.bottom(), y) {
                return None;
            }
            line.words
                .iter()
                .position(|w| spans(w.bounds.left(), w.bounds.right(), x))
                .map(|word| Pos { line: l, word })
        })
    }

    /// The word nearest a point: on the line nearest it up and down, the word nearest it
    /// along the line. Where a drag that ends between words, past the end of a line or
    /// below the last one has got to.
    #[must_use]
    pub fn nearest(&self, x: f64, y: f64) -> Option<Pos> {
        let (l, line) = self.lines.iter().enumerate().min_by(|(_, a), (_, b)| {
            let (da, db) = (
                off(a.bounds.top(), a.bounds.bottom(), y),
                off(b.bounds.top(), b.bounds.bottom(), y),
            );
            da.total_cmp(&db)
        })?;
        let (word, _) = line.words.iter().enumerate().min_by(|(_, a), (_, b)| {
            let (da, db) = (
                off(a.bounds.left(), a.bounds.right(), x),
                off(b.bounds.left(), b.bounds.right(), x),
            );
            da.total_cmp(&db)
        })?;
        Some(Pos { line: l, word })
    }

    /// Every word from `a` to `b` in reading order, whichever of the two comes first.
    #[must_use]
    pub fn between(&self, a: Pos, b: Pos) -> Vec<Pos> {
        let (first, last) = if a <= b { (a, b) } else { (b, a) };
        let mut out = Vec::new();
        for (l, line) in self.lines.iter().enumerate().take(last.line + 1).skip(first.line) {
            let from = if l == first.line { first.word } else { 0 };
            let to = if l == last.line { last.word } else { line.words.len().saturating_sub(1) };
            out.extend(
                (from..=to.min(line.words.len().saturating_sub(1)))
                    .map(|word| Pos { line: l, word }),
            );
        }
        out
    }

    /// The text of every word from `a` to `b`: a space between words, a line break or a
    /// space between lines as `breaks` says, and a blank line between paragraphs, as a text
    /// capture gives it (`spec/07` §2.1).
    #[must_use]
    pub fn text(&self, a: Pos, b: Pos, breaks: Breaks) -> String {
        let mut out = String::new();
        let mut previous: Option<usize> = None;
        for at in self.between(a, b) {
            let Some(word) = self.word(at) else { continue };
            match previous {
                None => {}
                Some(line) if line == at.line => out.push(' '),
                Some(_) if self.lines[at.line].starts_paragraph => out.push_str("\n\n"),
                Some(_) => out.push(match breaks {
                    Breaks::Keep => '\n',
                    Breaks::Join => ' ',
                }),
            }
            out.push_str(&word.text);
            previous = Some(at.line);
        }
        out
    }

    /// The link a word is part of.
    #[must_use]
    pub fn link_at(&self, at: Pos) -> Option<&LiveLink> {
        self.links.iter().find(|link| link.from <= at && at <= link.to)
    }

    /// The links in each line's text, by the words they are written across.
    fn find_links(&self) -> Vec<LiveLink> {
        let mut found = Vec::new();
        for (l, line) in self.lines.iter().enumerate() {
            let mut text = String::new();
            let mut starts = Vec::with_capacity(line.words.len());
            for word in &line.words {
                if !text.is_empty() {
                    text.push(' ');
                }
                starts.push(text.len());
                text.push_str(&word.text);
            }
            for link in links(&text) {
                let covers = |w: usize| {
                    let (start, end) = (starts[w], starts[w] + line.words[w].text.len());
                    link.range.start < end && start < link.range.end
                };
                let words: Vec<usize> = (0..line.words.len()).filter(|&w| covers(w)).collect();
                if let (Some(&from), Some(&to)) = (words.first(), words.last()) {
                    found.push(LiveLink {
                        target: link.target,
                        from: Pos { line: l, word: from },
                        to: Pos { line: l, word: to },
                    });
                }
            }
        }
        found
    }
}

/// A box's words, each with the part of the box its characters take.
fn split(word: &Word) -> Vec<LiveWord> {
    let b = word.bounds;
    let chars = word.text.chars().count().max(1);
    #[allow(clippy::cast_precision_loss)]
    let per = f64::from(b.width) / chars as f64;
    let mut out = Vec::new();
    for piece in word.text.split_whitespace() {
        // `split_whitespace` gives slices of the text, so its offset is where it starts.
        let start = piece.as_ptr() as usize - word.text.as_ptr() as usize;
        let before = word.text[..start].chars().count();
        let through = before + piece.chars().count();
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let (left, right) = (
            (f64::from(b.x) + per * before as f64).round() as i32,
            (f64::from(b.x) + per * through as f64).round() as i32,
        );
        out.push(LiveWord {
            text: piece.to_owned(),
            bounds: Bounds::new(left, b.y, (right - left).max(1), b.height),
        });
    }
    out
}

fn line_text(line: &LiveLine) -> String {
    line.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
}

/// Whether `v` is within `[from, to)`.
fn spans(from: i32, to: i32, v: f64) -> bool {
    f64::from(from) <= v && v < f64::from(to)
}

/// How far `v` is outside `[from, to]`, and 0 inside.
fn off(from: i32, to: i32, v: f64) -> f64 {
    if v < f64::from(from) {
        f64::from(from) - v
    } else if v > f64::from(to) {
        v - f64::from(to)
    } else {
        0.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn word(text: &str, x: i32, y: i32, width: i32) -> Word {
        Word::new(text, Bounds::new(x, y, width, 20), 0.99)
    }

    /// Two paragraphs: a line in two boxes, a line under it, and one further down.
    fn page() -> Page {
        Page::new(&[
            word("see the docs", 100, 10, 120),
            word("Hello world,", 0, 10, 90),
            word("at https://example.org/a now", 0, 40, 280),
            word("Thanks.", 0, 120, 70),
        ])
    }

    #[test]
    fn boxes_are_cut_into_words_in_reading_order() {
        let page = page();
        let texts: Vec<Vec<&str>> =
            page.lines.iter().map(|l| l.words.iter().map(|w| w.text.as_str()).collect()).collect();
        assert_eq!(
            texts,
            vec![
                vec!["Hello", "world,", "see", "the", "docs"],
                vec!["at", "https://example.org/a", "now"],
                vec!["Thanks."]
            ]
        );
        // "see the docs" is 12 characters over 120 px: "the" is characters 4 to 7.
        assert_eq!(page.lines[0].words[3].bounds, Bounds::new(140, 10, 30, 20));
    }

    /// D188: a word the export hides is not there for live text: not in the text, not in
    /// a link, and its line goes when it was the line's only word.
    #[test]
    fn hidden_words_are_not_copied_or_followed() {
        let page = page();
        let shown = page.without(|w| w.text.starts_with("https") || w.text == "now");
        let (first, last) = shown.all().unwrap();
        assert_eq!(
            shown.text(first, last, Breaks::Keep),
            "Hello world, see the docs\nat\n\nThanks."
        );
        assert!(shown.links.is_empty());
        let shown = page.without(|w| w.text == "Thanks.");
        assert_eq!(shown.lines.len(), 2);
        assert_eq!(shown.links.len(), 1);
    }

    /// D188: a paragraph whose first line is hidden starts at the first line left.
    #[test]
    fn a_paragraph_starts_at_its_first_line_left() {
        let page = Page::new(&[
            word("Hello world,", 0, 10, 90),
            word("at https://example.org/a now", 0, 40, 280),
            word("Thanks.", 0, 120, 70),
            word("Bye.", 0, 150, 40),
        ]);
        let starts: Vec<bool> = page.lines.iter().map(|l| l.starts_paragraph).collect();
        assert_eq!(starts, [false, false, true, false]);
        let shown = page.without(|w| w.text == "Thanks.");
        let (first, last) = shown.all().unwrap();
        assert_eq!(
            shown.text(first, last, Breaks::Keep),
            "Hello world,\nat https://example.org/a now\n\nBye."
        );
    }

    #[test]
    fn a_wide_gap_starts_a_paragraph() {
        let page = page();
        let starts: Vec<bool> = page.lines.iter().map(|l| l.starts_paragraph).collect();
        assert_eq!(starts, [false, false, true]);
    }

    #[test]
    fn a_selection_is_the_same_whichever_way_it_is_dragged() {
        let page = page();
        let (a, b) = (Pos { line: 0, word: 3 }, Pos { line: 1, word: 0 });
        assert_eq!(page.between(a, b), page.between(b, a));
        assert_eq!(page.text(b, a, Breaks::Keep), "the docs\nat");
        assert_eq!(page.text(a, b, Breaks::Join), "the docs at");
    }

    #[test]
    fn the_whole_page_keeps_its_paragraph_break() {
        let page = page();
        let (first, last) = page.all().unwrap();
        assert_eq!(
            page.text(first, last, Breaks::Keep),
            "Hello world, see the docs\nat https://example.org/a now\n\nThanks."
        );
    }

    #[test]
    fn a_point_finds_its_word_and_a_point_outside_the_nearest() {
        let page = page();
        assert_eq!(page.at(150.0, 15.0), Some(Pos { line: 0, word: 3 }));
        assert_eq!(page.at(500.0, 15.0), None);
        // Past the end of the first line, and below the last.
        assert_eq!(page.nearest(500.0, 15.0), Some(Pos { line: 0, word: 4 }));
        assert_eq!(page.nearest(10.0, 900.0), Some(Pos { line: 2, word: 0 }));
    }

    #[test]
    fn a_link_is_the_words_it_is_written_across() {
        let page = page();
        assert_eq!(page.links.len(), 1);
        let link = &page.links[0];
        assert_eq!(link.target, "https://example.org/a");
        assert_eq!((link.from, link.to), (Pos { line: 1, word: 1 }, Pos { line: 1, word: 1 }));
        assert_eq!(
            page.link_at(Pos { line: 1, word: 1 }).map(|l| l.target.as_str()),
            Some("https://example.org/a")
        );
        assert!(page.link_at(Pos { line: 1, word: 2 }).is_none());
    }

    #[test]
    fn nothing_read_is_an_empty_page() {
        let page = Page::new(&[]);
        assert!(page.is_empty());
        assert_eq!(page.all(), None);
        assert_eq!(page.nearest(1.0, 1.0), None);
    }
}
