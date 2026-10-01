// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/07` §2.1's QR and barcode reading.
//!
//! > QR codes: if the area contains a QR/barcode, its payload is copied instead (URLs
//! > offered to open).
//!
//! "Instead" is the whole of the rule: a code is not text that happens to be square, and a
//! capture with one in it is a capture of the code. The caller asks here first and only
//! falls through to the engine when there is nothing to find -- which costs a few
//! milliseconds and needs no model, no pack and no download, because a QR code is
//! arithmetic rather than recognition.

use crate::{Bounds, Gray, LinkKind, links};

/// One code, and where it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code {
    pub text: String,
    pub bounds: Bounds,
}

impl Code {
    /// What opening it should do, if anything: `spec/07` §2.1's "URLs offered to open".
    ///
    /// A payload is offered only when the *whole* of it is one link. A code carrying a
    /// vCard has a URL somewhere inside it and opening that is not what the code says.
    #[must_use]
    pub fn target(&self) -> Option<String> {
        let text = self.text.trim();
        match links(text).as_slice() {
            [link] if link.range == (0..text.len()) && link.kind == LinkKind::Url => {
                Some(link.target.clone())
            }
            _ => None,
        }
    }
}

/// Every QR code in the image, in the order they were found.
#[must_use]
pub fn codes(image: &Gray) -> Vec<Code> {
    let (width, height) = (image.width() as usize, image.height() as usize);
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(width, height, |x, y| {
        image.at(x as u32, y as u32)
    });
    prepared
        .detect_grids()
        .into_iter()
        .filter_map(|grid| {
            let bounds = around(&grid.bounds);
            grid.decode().ok().map(|(_, text)| Code { text, bounds })
        })
        .filter(|code| !code.text.is_empty())
        .collect()
}

/// The box around a grid's four corners, which `rqrr` gives in the order it found them.
fn around(corners: &[rqrr::Point; 4]) -> Bounds {
    let xs = corners.iter().map(|point| point.x);
    let ys = corners.iter().map(|point| point.y);
    let (left, right) = (xs.clone().min().unwrap_or(0), xs.max().unwrap_or(0));
    let (top, bottom) = (ys.clone().min().unwrap_or(0), ys.max().unwrap_or(0));
    Bounds::new(left, top, right - left, bottom - top)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
pub(crate) mod tests {
    use super::*;

    /// A real QR code, drawn at `scale` pixels a module with the four-module quiet zone
    /// the standard asks for, somewhere inside a `width` x `height` page.
    pub(crate) fn drawn(payload: &str, scale: u32, at: (u32, u32), size: (u32, u32)) -> Gray {
        let code = qrcode::QrCode::new(payload.as_bytes()).expect("a code");
        let modules = code.width() as u32;
        let colours = code.to_colors();
        let mut pixels = vec![255u8; (size.0 as usize) * (size.1 as usize)];
        let quiet = 4 * scale;
        for row in 0..modules {
            for column in 0..modules {
                let dark = colours[(row * modules + column) as usize] == qrcode::Color::Dark;
                if !dark {
                    continue;
                }
                for y in 0..scale {
                    for x in 0..scale {
                        let py = at.1 + quiet + row * scale + y;
                        let px = at.0 + quiet + column * scale + x;
                        if px < size.0 && py < size.1 {
                            pixels[(py as usize) * (size.0 as usize) + px as usize] = 0;
                        }
                    }
                }
            }
        }
        Gray::new(size.0, size.1, pixels).expect("a page")
    }

    #[test]
    fn a_qr_code_on_a_page_is_read_and_located() {
        let page = drawn("https://octosnap.example/help", 4, (60, 40), (400, 400));
        let found = codes(&page);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].text, "https://octosnap.example/help");
        // Roughly where it was drawn, give or take the quiet zone and the corner centres.
        let bounds = found[0].bounds;
        assert!(bounds.left() > 60 && bounds.left() < 120, "{bounds:?}");
        assert!(bounds.width > 60 && bounds.width < 260, "{bounds:?}");
    }

    #[test]
    fn a_payload_that_is_a_url_is_offered_to_open_and_one_that_is_not_is_not() {
        let url = drawn("https://example.com/a", 4, (20, 20), (300, 300));
        assert_eq!(
            codes(&url)[0].target().as_deref(),
            Some("https://example.com/a"),
            "the payload is the link"
        );

        let vcard = "BEGIN:VCARD\nURL:https://example.com\nEND:VCARD";
        let card = drawn(vcard, 4, (20, 20), (400, 400));
        let found = codes(&card);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].text.starts_with("BEGIN:VCARD"));
        assert_eq!(found[0].target(), None, "a card is not a link, even with one inside it");
    }

    #[test]
    fn a_page_with_no_code_on_it_has_no_codes() {
        let blank = Gray::new(200, 200, vec![240; 40_000]).expect("a page");
        assert!(codes(&blank).is_empty());
        let nothing = Gray::new(1, 1, vec![0]).expect("a pixel");
        assert!(codes(&nothing).is_empty());
    }

    #[test]
    fn two_codes_on_one_page_are_two_codes() {
        let mut page = drawn("first", 4, (10, 10), (500, 260));
        let second = drawn("second", 4, (10, 10), (500, 260));
        // Paste the second one's left third into the right of the first.
        for y in 0..260 {
            for x in 0..240 {
                let at = (y as usize) * 500 + (x as usize) + 250;
                if second.at(x, y) == 0 {
                    page = paste(page, at);
                }
            }
        }
        let found = codes(&page);
        assert_eq!(found.len(), 2, "{found:?}");
        let mut payloads: Vec<&str> = found.iter().map(|code| code.text.as_str()).collect();
        payloads.sort_unstable();
        assert_eq!(payloads, vec!["first", "second"]);
    }

    fn paste(page: Gray, at: usize) -> Gray {
        let (width, height) = (page.width(), page.height());
        let mut pixels = page.pixels().to_vec();
        if let Some(pixel) = pixels.get_mut(at) {
            *pixel = 0;
        }
        Gray::new(width, height, pixels).expect("a page")
    }
}
