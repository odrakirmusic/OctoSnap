// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/07` §2's text recognition, from the boxes up.
//!
//! An engine reads pixels and hands back boxes of text. Everything that decides what those
//! boxes *mean* -- which of them are one line, which lines are one paragraph, what the two
//! shortcuts put on the clipboard, and which words in the result are worth clicking -- is
//! here, and nothing here knows what an engine is. The split is the same one `spec/10` §7
//! asks for elsewhere and it buys the same thing: the part that is easy to get wrong is
//! wrong in a test, over a page built to be awkward, rather than in a paste that someone
//! has to re-screenshot to reproduce.
//!
//! - [`Word`] is one box as a detector hands it over.
//! - [`Gray`] is the greyscale an engine is given, and the measurements taken off it.
//! - [`Engine`] is the inference, and [`Reader`] is pixels in, paragraphs out.
//! - [`shape`] groups boxes into [`Line`]s and lines into [`Paragraph`]s.
//! - [`Shaped::text`] is the two shortcuts of `spec/07` §2.1, keep or join.
//! - [`links`] finds the URLs and e-mail addresses in the text that comes out.
//! - [`qr`] reads the QR codes that `spec/07` §2.1 says are copied *instead* of text.

pub mod engine;
pub mod links;
pub mod packs;
pub mod qr;
pub mod image;
pub mod rapid;
pub mod shape;

pub use engine::{Engine, Pack, Read, Reader, Script};
pub use rapid::Rapid;
pub use links::{Link, LinkKind, links};
pub use qr::{Code, codes};
pub use image::Gray;
pub use shape::{Breaks, Line, Paragraph, Shaped, shape};

/// What can go wrong between a capture and its text.
#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    #[error("an image of {width}x{height} cannot be {pixels} bytes")]
    Size { width: u32, height: u32, pixels: usize },
    #[error("no model for {0} -- the language pack is not installed")]
    Missing(&'static str),
    #[error("{0}: {1}")]
    Engine(String, String),
    /// No ONNX Runtime library anywhere it is looked for (D172). Apart from
    /// [`OcrError::Engine`] with [`rapid::RUNTIME`], which is a library that was found and
    /// would not load: this one is asked again by the next read, so an install counts.
    #[error("ONNX Runtime is not installed")]
    NoRuntime,
}

/// An axis-aligned box in the **captured image's own pixels**.
///
/// Deliberately not `octosnap_core::Rect`, which says of itself that it is logical stage
/// coordinates and carries no scale so that a physical rect cannot be passed for a logical
/// one by accident. An engine is given an image and answers in that image's pixels; it has
/// never heard of the stage. Passing one of these where a `Rect` belongs should not
/// compile, and does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Bounds {
    #[must_use]
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self { x, y, width, height }
    }

    #[must_use]
    pub const fn left(&self) -> i32 {
        self.x
    }

    #[must_use]
    pub const fn top(&self) -> i32 {
        self.y
    }

    #[must_use]
    pub const fn right(&self) -> i32 {
        self.x + self.width
    }

    #[must_use]
    pub const fn bottom(&self) -> i32 {
        self.y + self.height
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    /// How much of this box's height shares rows with `other`'s, in pixels.
    #[must_use]
    pub fn shared_rows(&self, other: Self) -> i32 {
        (self.bottom().min(other.bottom()) - self.top().max(other.top())).max(0)
    }

    /// The smallest box holding both.
    #[must_use]
    pub fn union(&self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return *self;
        }
        let (x, y) = (self.left().min(other.left()), self.top().min(other.top()));
        Self::new(x, y, self.right().max(other.right()) - x, self.bottom().max(other.bottom()) - y)
    }
}

/// One box of text as a detector hands it over.
///
/// A detector's box is not a line: PaddleOCR's -- and so RapidOCR's, `spec/07` §2.2's
/// default -- splits where the ink stops, so a menu bar is six boxes and a table row is
/// one box a column. [`shape`] is what puts them back together.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    pub bounds: Bounds,
    /// The engine's own confidence, 0.0 to 1.0. Carried through so a caller can threshold
    /// it; nothing in this crate reads it, because a box the engine kept is a box we show.
    pub confidence: f32,
}

impl Word {
    #[must_use]
    pub fn new(text: impl Into<String>, bounds: Bounds, confidence: f32) -> Self {
        Self { text: text.into(), bounds, confidence }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// The two halves against each other: a link that was wrapped over two lines of a
    /// paragraph is one link once the paragraph is joined, and two once it is not.
    #[test]
    fn a_link_is_found_in_whichever_of_the_two_texts_the_window_is_showing() {
        let words = vec![
            Word::new("Docs at", Bounds::new(10, 100, 60, 14), 0.99),
            Word::new("https://example.com/docs", Bounds::new(80, 100, 190, 14), 0.99),
            Word::new("and www.gnome.org too.", Bounds::new(10, 120, 170, 14), 0.99),
        ];
        let shaped = shape(words);
        assert_eq!(shaped.paragraphs.len(), 1);

        let joined = shaped.text(Breaks::Join);
        let found = links(&joined);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(&joined[found[0].range.clone()], "https://example.com/docs");
        assert_eq!(found[1].target, "https://www.gnome.org");

        // The same two links, at different offsets, in the variant that keeps the break.
        let kept = shaped.text(Breaks::Keep);
        assert_ne!(kept, joined);
        assert_eq!(links(&kept).len(), 2);
    }
}
