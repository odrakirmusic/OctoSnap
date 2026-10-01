// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/07` §1's scrolling capture, as far as the model goes: which way it travels.
//!
//! Here rather than in the stitcher or the bridge because four different places need the
//! same answer and none of them should own it: the settings store a default, the CLI and
//! the URL parse one, the extension is told one over the bus, and the stitcher turns one
//! into a direction to grow in. A string in each of those is four chances to disagree.
//!
//! Who scrolls was here too until 2026-09-27, when auto-scroll was taken out (D153): the
//! user scrolls, and the capture follows.

/// Which way a scrolling capture travels.
///
/// `spec/07` §1.1 item 2 gives the two the controls first offered -- "direction ↓ or →" --
/// and `spec/07` §1.2 ends "horizontal mode is the transpose". The two backwards
/// directions are here because the transpose has a twin: a capture that grows upwards is
/// the same algorithm on a frame turned over, and leaving them out would mean a user who
/// starts at the bottom of a page has no way to capture it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollDirection {
    #[default]
    Down,
    Up,
    Right,
    Left,
}

impl ScrollDirection {
    /// In the order a direction control offers them.
    pub const ALL: [Self; 4] = [Self::Down, Self::Up, Self::Right, Self::Left];

    /// The wire string `StartScrollAssist` expects, and the one the settings store.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Down => "down",
            Self::Up => "up",
            Self::Right => "right",
            Self::Left => "left",
        }
    }

    /// A direction from the bus, a setting, a URL or a command line.
    ///
    /// `None` rather than a default, so a caller can tell a typo from a choice: the CLI
    /// reports it, and the bus refuses the call.
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.as_wire() == value)
    }

    /// Whether the capture grows sideways rather than downwards.
    #[must_use]
    pub const fn horizontal(self) -> bool {
        matches!(self, Self::Right | Self::Left)
    }

    /// Whether the new content arrives *before* what is already captured.
    #[must_use]
    pub const fn backwards(self) -> bool {
        matches!(self, Self::Up | Self::Left)
    }

    /// What the direction control says.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Down => "Down",
            Self::Up => "Up",
            Self::Right => "Right",
            Self::Left => "Left",
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn every_direction_round_trips_through_the_wire() {
        for direction in ScrollDirection::ALL {
            assert_eq!(ScrollDirection::from_wire(direction.as_wire()), Some(direction));
        }
        assert_eq!(ScrollDirection::from_wire("sideways"), None);
        assert_eq!(ScrollDirection::from_wire("Down"), None);
    }

    #[test]
    fn the_two_axes_of_a_direction_are_independent() {
        assert!(!ScrollDirection::Down.horizontal() && !ScrollDirection::Down.backwards());
        assert!(!ScrollDirection::Up.horizontal() && ScrollDirection::Up.backwards());
        assert!(ScrollDirection::Right.horizontal() && !ScrollDirection::Right.backwards());
        assert!(ScrollDirection::Left.horizontal() && ScrollDirection::Left.backwards());
    }
}
