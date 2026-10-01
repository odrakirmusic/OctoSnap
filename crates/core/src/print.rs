// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §7's Print, laid out: one page for a screenshot, and as many as it takes for a
//! scrolling capture (`spec/07` §1 item 6).
//!
//! A screenshot is fitted inside the printable area with its proportions kept and centred,
//! and a small one is enlarged to fill it: a 400-pixel dialog printed at one point per pixel
//! would be a stamp in the corner of the sheet. That rule, applied to a page-long capture,
//! prints a column a few centimetres wide. So a capture much longer than the page, in the
//! page's own proportions, is set to the page's width instead and cut across as many pages
//! as its length needs, each row printed once. Across a wide capture the same happens
//! sideways, but only for a scrolling one: a two-monitor screenshot is wide too, and it
//! prints as one strip, which is what anyone printing a screenshot expects.

/// How much longer than the page, in the page's own proportions, a capture has to be before
/// it is cut. At twice, a tall window still prints whole on one page, and a scrolled page of
/// any length is cut.
const LONGER_THAN_THE_PAGE: f64 = 2.0;

/// Which way the pages follow each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    /// One page.
    None,
    /// Top to bottom: the capture is set to the page's width.
    Down,
    /// Left to right: the capture is set to the page's height.
    Across,
}

/// Where a document of `image` pixels goes on pages of `page` points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// Points per pixel.
    pub scale: f64,
    /// How many pages. At least one.
    pub pages: u32,
    pub split: Split,
    image: (f64, f64),
    page: (f64, f64),
}

impl Layout {
    /// The layout for an `image` of pixels on a printable area of `page` points.
    /// `scrolling` says whether the image is a scrolling capture, which is the only kind cut
    /// across its width.
    #[must_use]
    pub fn new(image: (f64, f64), page: (f64, f64), scrolling: bool) -> Self {
        let (w, h) = image;
        let (pw, ph) = page;
        if w <= 0.0 || h <= 0.0 || pw <= 0.0 || ph <= 0.0 {
            return Self { scale: 1.0, pages: 1, split: Split::None, image, page };
        }
        let one_page = Self { scale: (pw / w).min(ph / h), pages: 1, split: Split::None, image, page };

        // How much longer than the page the image is, along each axis, in the page's own
        // proportions: 1 is the page's shape exactly.
        let taller = (h / w) / (ph / pw);
        let wider = (w / h) / (pw / ph);
        let (scale, length, page_length, split) = if taller >= LONGER_THAN_THE_PAGE {
            let scale = pw / w;
            (scale, h * scale, ph, Split::Down)
        } else if scrolling && wider >= LONGER_THAN_THE_PAGE {
            let scale = ph / h;
            (scale, w * scale, pw, Split::Across)
        } else {
            return one_page;
        };
        // A hair under a whole number of pages is that number, not one more page with a
        // row of rounding error on it.
        let count = (length / page_length - 1e-9).ceil().max(1.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let pages = count.min(f64::from(u32::MAX)) as u32;
        Self { scale, pages, split, image, page }
    }

    /// Where page `index` puts the image's top-left corner, in the page's points. The page
    /// clips to itself, so this is all a page needs to draw its part.
    #[must_use]
    pub fn origin(&self, index: u32) -> (f64, f64) {
        let (w, h) = (self.image.0 * self.scale, self.image.1 * self.scale);
        let (pw, ph) = self.page;
        let step = f64::from(index.min(self.pages.saturating_sub(1)));
        match self.split {
            Split::None => ((pw - w) / 2.0, (ph - h) / 2.0),
            Split::Down => (0.0, -step * ph),
            Split::Across => (-step * pw, 0.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A4's printable area at GTK's default margins, near enough.
    const A4: (f64, f64) = (540.0, 780.0);

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn a_screenshot_is_one_page_fitted_and_centred() {
        let layout = Layout::new((1920.0, 1080.0), A4, false);
        assert_eq!(layout.pages, 1);
        assert_eq!(layout.split, Split::None);
        assert!(close(layout.scale, 540.0 / 1920.0));
        let (x, y) = layout.origin(0);
        assert!(close(x, 0.0));
        assert!(close(y, (780.0 - 1080.0 * 540.0 / 1920.0) / 2.0));
    }

    #[test]
    fn a_small_screenshot_is_enlarged_to_the_page() {
        let layout = Layout::new((400.0, 300.0), A4, false);
        assert_eq!(layout.pages, 1);
        assert!(close(layout.scale, 540.0 / 400.0));
    }

    #[test]
    fn a_tall_window_still_prints_whole() {
        // 1.75 times as tall as wide, against the page's 1.44: well under twice the page.
        let layout = Layout::new((800.0, 1400.0), A4, false);
        assert_eq!(layout.pages, 1);
        assert!(close(layout.scale, 780.0 / 1400.0));
    }

    #[test]
    fn a_scrolled_page_is_set_to_the_width_and_cut_down_the_pages() {
        let layout = Layout::new((1280.0, 8000.0), A4, true);
        assert_eq!(layout.split, Split::Down);
        assert!(close(layout.scale, 540.0 / 1280.0));
        // 8000 px at 0.421875 pt/px is 3375 pt: four full pages and a part.
        assert_eq!(layout.pages, 5);
        assert_eq!(layout.origin(0), (0.0, 0.0));
        assert!(close(layout.origin(3).1, -3.0 * 780.0));
        // Past the last page is the last page, not a blank one.
        assert_eq!(layout.origin(9), layout.origin(4));
    }

    #[test]
    fn a_tall_image_is_cut_whether_or_not_it_was_scrolled() {
        let layout = Layout::new((1280.0, 8000.0), A4, false);
        assert_eq!(layout.split, Split::Down);
        assert_eq!(layout.pages, 5);
    }

    #[test]
    fn an_exact_number_of_pages_has_no_blank_page_after_it() {
        // An image the page's width prints at a point per pixel, so 2340 px is exactly
        // three pages of 780 pt.
        let layout = Layout::new((540.0, 2340.0), A4, true);
        assert_eq!(layout.split, Split::Down);
        assert_eq!(layout.pages, 3);
    }

    #[test]
    fn a_wide_screenshot_prints_as_one_strip_and_a_wide_scroll_goes_across() {
        let screens = Layout::new((3840.0, 1080.0), A4, false);
        assert_eq!(screens.pages, 1);
        assert_eq!(screens.split, Split::None);

        let scrolled = Layout::new((9000.0, 900.0), A4, true);
        assert_eq!(scrolled.split, Split::Across);
        assert!(close(scrolled.scale, 780.0 / 900.0));
        // 9000 px at 0.8667 pt/px is 7800 pt, over 540 pt pages: fourteen and a part.
        assert_eq!(scrolled.pages, 15);
        assert!(close(scrolled.origin(2).0, -2.0 * 540.0));
        assert!(close(scrolled.origin(2).1, 0.0));
    }

    #[test]
    fn nothing_to_print_is_one_page_that_draws_nothing_wrong() {
        let layout = Layout::new((0.0, 100.0), A4, true);
        assert_eq!(layout.pages, 1);
        assert_eq!(layout.split, Split::None);
        let layout = Layout::new((100.0, 100.0), (0.0, 0.0), true);
        assert_eq!(layout.pages, 1);
    }
}
