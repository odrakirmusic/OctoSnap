// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/07` §1's scrolling capture, from the frames down.
//!
//! The extension scrolls the content and grabs frames; everything that decides what those
//! frames *mean* is here, and nothing here knows what a compositor is. `spec/10` §7 is the
//! reason -- "move anything loop-shaped to the app", with the scroll-assist iteration
//! budgeted at under 120 ms of which the extension's share is 5 ms -- but the reason that
//! matters day to day is smaller: a stitcher that takes frames and returns an image can be
//! wrong in a unit test, at full speed, over a page built to be awkward, instead of wrong
//! in a screenshot of a README that someone has to scroll again to reproduce.
//!
//! - [`Frame`] is RGBA pixels and the two directions of PNG.
//! - [`signature`] turns a row into the two numbers the matcher compares.
//! - [`align`] finds how far the page moved, and which rows refused to move with it.
//! - [`Stitcher`] is the loop: push frames, get told what to do next, finish with an image.

pub mod align;
pub mod frame;
pub mod signature;
pub mod stitcher;

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod testing;

pub use align::{Fixed, Match};
pub use frame::Frame;
pub use signature::Signatures;
pub use stitcher::{Direction, End, Limits, Next, Stitcher};

/// `spec/07` §1.2's step: "scroll by S = 0.6 x rect.height".
///
/// Here rather than in the caller because it is the number the matcher's tie-break is
/// tuned against: a step much larger leaves too little overlap to score, and a step much
/// smaller spends frames on rows that are already on the canvas.
pub const STEP: f64 = 0.6;

/// What can go wrong between a frame and the canvas.
#[derive(Debug, thiserror::Error)]
pub enum StitchError {
    #[error("an empty frame")]
    Empty,
    #[error("a frame of {width}x{height} is not the size the capture started at")]
    Mismatch { width: u32, height: u32 },
    #[error("{0}: {1}")]
    Io(String, String),
}

/// The step, in rows, for a selection of this height.
#[must_use]
pub fn step_for(height: u32) -> u32 {
    ((f64::from(height) * STEP) as u32).max(1)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_step_is_three_fifths_of_the_selection_and_never_nothing() {
        assert_eq!(step_for(1000), 600);
        assert_eq!(step_for(100), 60);
        assert_eq!(step_for(1), 1);
        assert_eq!(step_for(0), 1);
    }
}
