// SPDX-License-Identifier: GPL-3.0-or-later

//! Properties of the links found in recognised text (ENG-03, D173).
//!
//! The text comes from a picture, and a picture can say anything: what is read is drawn
//! from any text at all. A link becomes something the text window opens, so the property
//! that matters most is the one about what it can open: the web, FTP and mail, and never
//! another scheme, whatever the picture says.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use octosnap_ocr::{LinkKind, links};
use proptest::prelude::*;

const OPENABLE: [&str; 4] = ["https://", "http://", "ftp://", "mailto:"];

/// Text that is mostly what a screen shows, with links and their usual company in it.
fn screen_text() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        prop::sample::select(vec![
            "https://",
            "http://",
            "ftp://",
            "mailto:",
            "www.",
            "javascript:",
            "file://",
            "@",
            ".",
            "(",
            ")",
            "[",
            "]",
            "\"",
            "'",
            ",",
            " ",
            "\n",
            "\u{201c}",
            "\u{201d}",
            "com",
            "example",
            "a@b.cd",
        ])
        .prop_map(str::to_owned),
        "[a-zA-Z0-9/?#=&%_.-]{0,10}",
        ".{0,6}",
    ];
    prop::collection::vec(piece, 0..24).prop_map(|pieces| pieces.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    #[test]
    fn every_link_is_a_well_formed_range_of_the_text(text in prop_oneof![screen_text(), ".*"]) {
        let found = links(&text);
        let mut last_end = 0;
        for link in &found {
            prop_assert!(link.range.start < link.range.end, "{:?}", link);
            prop_assert!(link.range.end <= text.len());
            prop_assert!(text.is_char_boundary(link.range.start));
            prop_assert!(text.is_char_boundary(link.range.end));
            prop_assert!(link.range.start >= last_end, "links in order, none overlapping");
            last_end = link.range.end;
            prop_assert!(!text[link.range.clone()].chars().any(char::is_whitespace));
        }
    }

    /// Whatever the picture says, a link opens the web, FTP or mail.
    #[test]
    fn a_link_opens_nothing_but_the_web_ftp_and_mail(text in prop_oneof![screen_text(), ".*"]) {
        for link in links(&text) {
            let target = link.target.to_ascii_lowercase();
            prop_assert!(OPENABLE.iter().any(|scheme| target.starts_with(scheme)), "{:?}", link);
            match link.kind {
                LinkKind::Email => prop_assert!(target.starts_with("mailto:")),
                LinkKind::Url => prop_assert!(!target.starts_with("mailto:")),
            }
        }
    }

    /// A plain link among words is found whole, without the full stop after it.
    #[test]
    fn a_plain_link_in_a_sentence_is_found_whole(
        before in "[a-zA-Z ]{0,20}",
        host in "[a-z]{1,10}\\.[a-z]{2,6}",
        path in "(/[a-zA-Z0-9_-]{1,8}){0,3}",
        after in "[a-zA-Z ]{0,20}",
    ) {
        let url = format!("https://{host}{path}");
        let text = format!("{before} {url}. {after}");
        let found = links(&text);
        prop_assert!(found.iter().any(|l| text[l.range.clone()] == url && l.target == url),
            "{:?} in {:?}", found, text);
    }
}
