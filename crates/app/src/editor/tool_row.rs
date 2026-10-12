// SPDX-License-Identifier: GPL-3.0-or-later

//! The title bar's tool page: the canvas actions and the tools on the first line, level
//! with the window's buttons, and the active tool's options after them (D199).
//!
//! It was an `adw::WrapBox`, which puts a child on the next line the moment it does not fit,
//! so where the options went depended on which tool was lit: at 980 px a counter's two
//! controls sat beside the tools and a rectangle's four went under them, the bar changing
//! height under the picture as the tool changed, and Select's hint dropping to a line of
//! its own when it was a few pixels too long. This lays the tools out the same way, and the
//! options by one rule for every tool: beside the tools when the line has room for the
//! widest options row any tool has, and under them otherwise. A row narrower than its
//! natural width, Select's hint, shrinks to fit rather than move.

use std::cell::Cell;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

/// Between two children on a line, and between two lines: the wrap box's spacing.
const SPACING: i32 = 6;

/// Where one child goes: its line, and its place and width along it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Place {
    line: usize,
    x: i32,
    width: i32,
}

/// Lays out children of `(minimum, natural)` widths along lines `width` wide.
///
/// As a wrap box does, each on the line if it fits at its natural width, and on the next
/// one if not -- except `trailing`, which stays on the line only if there is room for
/// `reserve` and its own minimum. On a line of its own, or beside the others, a child gets
/// its natural width or what the line has left, whichever is less, and never less than its
/// minimum.
fn place(widths: &[(i32, i32)], trailing: Option<usize>, reserve: i32, width: i32) -> Vec<Place> {
    let mut places = Vec::with_capacity(widths.len());
    let (mut line, mut x, mut empty) = (0, 0, true);
    for (i, &(min, nat)) in widths.iter().enumerate() {
        let need = if trailing == Some(i) { reserve.max(min) } else { nat };
        if !empty && x + SPACING + need > width {
            (line, x, empty) = (line + 1, 0, true);
        }
        if !empty {
            x += SPACING;
        }
        let given = nat.min(width - x).max(min);
        places.push(Place { line, x, width: given });
        x += given;
        empty = false;
    }
    places
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct ToolRowLayout {
        /// The options row, which keeps to the rule above.
        pub(super) trailing: glib::WeakRef<gtk::Widget>,
        /// The widest options row of any tool.
        pub(super) reserve: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ToolRowLayout {
        const NAME: &'static str = "OctoSnapToolRowLayout";
        type Type = super::ToolRowLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for ToolRowLayout {}

    impl ToolRowLayout {
        /// The children that take part, the trailing one's index among them, and where
        /// each goes at `width` -- or, at `None`, on one line at their natural widths.
        fn arrange(
            &self,
            widget: &gtk::Widget,
            width: Option<i32>,
        ) -> (Vec<gtk::Widget>, Vec<Place>) {
            let mut children = Vec::new();
            let mut child = widget.first_child();
            while let Some(c) = child {
                child = c.next_sibling();
                if c.should_layout() {
                    children.push(c);
                }
            }
            let trailing = self.trailing.upgrade();
            let index = children.iter().position(|c| Some(c) == trailing.as_ref());
            let widths: Vec<(i32, i32)> = children
                .iter()
                .map(|c| {
                    let (min, nat, _, _) = c.measure(gtk::Orientation::Horizontal, -1);
                    (min, nat)
                })
                .collect();
            let width = width.unwrap_or(i32::MAX);
            let places = place(&widths, index, self.reserve.get(), width);
            (children, places)
        }

        /// Each line's height at those places, and each child's.
        fn heights(children: &[gtk::Widget], places: &[Place]) -> (Vec<i32>, Vec<i32>) {
            let mut lines = Vec::new();
            let mut each = Vec::with_capacity(children.len());
            for (child, place) in children.iter().zip(places) {
                let (_, nat, _, _) = child.measure(gtk::Orientation::Vertical, place.width);
                if lines.len() <= place.line {
                    lines.resize(place.line + 1, 0);
                }
                lines[place.line] = lines[place.line].max(nat);
                each.push(nat);
            }
            (lines, each)
        }
    }

    impl LayoutManagerImpl for ToolRowLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            if orientation == gtk::Orientation::Horizontal {
                // As narrow as the widest child, which then has a line to itself; as
                // wide as all of them on one line, the options at the widest they get.
                let (children, places) = self.arrange(widget, None);
                let min = children
                    .iter()
                    .map(|c| c.measure(gtk::Orientation::Horizontal, -1).0)
                    .max()
                    .unwrap_or(0);
                let trailing = self.trailing.upgrade();
                let nat = children
                    .iter()
                    .zip(&places)
                    .map(|(c, p)| {
                        let reserve =
                            if Some(c) == trailing.as_ref() { self.reserve.get() } else { 0 };
                        p.x + p.width.max(reserve)
                    })
                    .max()
                    .unwrap_or(0);
                return (min, nat.max(min), -1, -1);
            }
            let (children, places) = self.arrange(widget, (for_size >= 0).then_some(for_size));
            let (lines, _) = Self::heights(&children, &places);
            let gaps = i32::try_from(lines.len().saturating_sub(1)).unwrap_or(0) * SPACING;
            let height = lines.iter().sum::<i32>() + gaps;
            (height, height, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, _height: i32, _baseline: i32) {
            let (children, places) = self.arrange(widget, Some(width));
            let (lines, each) = Self::heights(&children, &places);
            // Each line's top, and each child centred on its line's middle.
            let tops: Vec<i32> = lines
                .iter()
                .scan(0, |y, height| {
                    let top = *y;
                    *y += height + SPACING;
                    Some(top)
                })
                .collect();
            for ((child, place), height) in children.iter().zip(&places).zip(each) {
                let y = tops[place.line] + (lines[place.line] - height) / 2;
                child.size_allocate(&gtk::Allocation::new(place.x, y, place.width, height), -1);
            }
        }
    }
}

glib::wrapper! {
    /// The tool page's layout (D199).
    pub struct ToolRowLayout(ObjectSubclass<imp::ToolRowLayout>) @extends gtk::LayoutManager;
}

impl Default for ToolRowLayout {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl ToolRowLayout {
    /// The child that goes beside the others only with room for [`Self::set_reserve`].
    pub fn set_trailing(&self, child: &impl IsA<gtk::Widget>) {
        self.imp().trailing.set(Some(child.upcast_ref()));
        self.layout_changed();
    }

    /// The room the trailing child needs beside the others: the widest it gets.
    pub fn set_reserve(&self, width: i32) {
        self.imp().reserve.set(width);
        self.layout_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOOLS: [(i32, i32); 2] = [(100, 100), (531, 531)];

    fn lines(places: &[Place]) -> Vec<usize> {
        places.iter().map(|p| p.line).collect()
    }

    #[test]
    fn everything_on_one_line_with_room_for_the_widest_options() {
        let widths = [TOOLS[0], TOOLS[1], (80, 80)];
        let places = place(&widths, Some(2), 506, 100 + 6 + 531 + 6 + 506);
        assert_eq!(lines(&places), [0, 0, 0]);
        assert_eq!(places[2], Place { line: 0, x: 643, width: 80 });
    }

    #[test]
    fn the_options_go_under_the_tools_whenever_the_widest_would_not_fit() {
        // A counter's 80 px would fit at 848; the widest row would not, so no row does.
        let widths = [TOOLS[0], TOOLS[1], (80, 80)];
        let places = place(&widths, Some(2), 506, 848);
        assert_eq!(lines(&places), [0, 0, 1]);
        assert_eq!(places[2], Place { line: 1, x: 0, width: 80 });
    }

    #[test]
    fn a_hint_shrinks_on_its_line_rather_than_move() {
        // Select's hint: 40 px at its least, 254 at its natural width.
        let widths = [TOOLS[0], TOOLS[1], (40, 254)];
        let places = place(&widths, Some(2), 0, 848);
        assert_eq!(places[2], Place { line: 0, x: 643, width: 205 });
    }

    #[test]
    fn the_tools_wrap_as_a_wrap_box_does_when_the_line_is_too_short() {
        let places = place(&TOOLS, None, 0, 600);
        assert_eq!(lines(&places), [0, 1]);
        // And a child wider than the line keeps its minimum.
        assert_eq!(places[1].width, 531);
    }

    #[test]
    fn without_its_trailing_child_nothing_is_reserved() {
        let places = place(&TOOLS, None, 506, 637);
        assert_eq!(lines(&places), [0, 0]);
    }
}
