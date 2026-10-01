// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/07` §2.1's clickable links, found in the text rather than in the pixels.
//!
//! > Links detected (URLs, e-mails) become clickable in the result window; toggle in
//! > settings.
//!
//! Over the text and not over the boxes, because the two shortcuts produce two different
//! strings and a link has to be a range in whichever one the window is showing. By hand
//! and not by a regular expression, because the fiddly part of this is not matching a URL
//! -- it is deciding where one stops, and a pattern that ends at whitespace eats the full
//! stop at the end of the sentence.

use std::ops::Range;

/// What a found link is, which is what opening it should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Url,
    Email,
}

/// A run of the text worth making clickable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// Byte range in the text it was found in.
    pub range: Range<usize>,
    pub kind: LinkKind,
    /// What to hand to the opener: the URL itself, `https://` in front of a bare `www.`,
    /// or `mailto:` in front of an address.
    pub target: String,
}

/// Punctuation a link can be wrapped in, which is never part of it.
const OPENERS: &[char] = &['(', '[', '{', '<', '"', '\'', '\u{201c}', '\u{2018}', '\u{ab}'];

/// Punctuation a link can be followed by. Brackets are checked against the openers inside
/// the link first, so `.../Rust_(programming_language)` keeps the bracket it opened.
const CLOSERS: &[char] = &[
    ')', ']', '}', '>', '"', '\'', '\u{201d}', '\u{2019}', '\u{bb}', '.', ',', ';', ':', '!', '?',
];

/// The schemes worth detecting. Not a general list: these are the ones a screenshot of a
/// browser, a terminal or a chat window actually contains.
const SCHEMES: &[&str] = &["https://", "http://", "ftp://"];

/// Every URL and e-mail address in `text`, in the order they appear.
#[must_use]
pub fn links(text: &str) -> Vec<Link> {
    let mut found = Vec::new();
    for (at, token) in tokens(text) {
        let span = trim(token);
        if span.is_empty() {
            continue;
        }
        if let Some((kind, target)) = classify(&token[span.clone()]) {
            found.push(Link { range: at + span.start..at + span.end, kind, target });
        }
    }
    found
}

/// Is this a link, and if so what should opening it do?
///
/// A scheme or a leading `www.` is required for a URL, and no list of top-level domains is
/// consulted. Guessing at bare dotted words is how `crates/ocr/src/lib.rs` in a paste from
/// a terminal becomes a link to Slovenia, and the text this runs over is more often a
/// developer's screen than a page of prose.
fn classify(candidate: &str) -> Option<(LinkKind, String)> {
    let lower = candidate.to_ascii_lowercase();
    if let Some(address) = lower.strip_prefix("mailto:") {
        return is_email(address).then(|| (LinkKind::Email, candidate.to_string()));
    }
    for scheme in SCHEMES {
        if let Some(rest) = lower.strip_prefix(scheme) {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
            return (!authority.is_empty()).then(|| (LinkKind::Url, candidate.to_string()));
        }
    }
    if let Some(rest) = lower.strip_prefix("www.")
        && rest.contains('.')
        && rest.starts_with(|c: char| c.is_ascii_alphanumeric())
    {
        return Some((LinkKind::Url, format!("https://{candidate}")));
    }
    is_email(&lower).then(|| (LinkKind::Email, format!("mailto:{candidate}")))
}

/// `local@host.tld`, strictly enough that a Rust turbofish or a shell redirection is not
/// an address.
fn is_email(candidate: &str) -> bool {
    let Some((local, host)) = candidate.split_once('@') else { return false };
    !local.is_empty()
        && local.chars().all(|c| c.is_ascii_alphanumeric() || "._%+-".contains(c))
        && is_host(host)
}

/// Dotted labels ending in at least two letters.
fn is_host(candidate: &str) -> bool {
    let labels: Vec<&str> = candidate.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
        && labels.last().is_some_and(|tld| {
            tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())
        })
}

/// The runs of non-whitespace in `text`, each with the byte it starts at.
fn tokens(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (at, c) in text.char_indices() {
        match (c.is_whitespace(), start) {
            (true, Some(from)) => {
                out.push((from, &text[from..at]));
                start = None;
            }
            (false, None) => start = Some(at),
            _ => {}
        }
    }
    if let Some(from) = start {
        out.push((from, &text[from..]));
    }
    out
}

/// The part of a token that could be a link, with the punctuation around it removed.
fn trim(token: &str) -> Range<usize> {
    let mut start = 0;
    while let Some(c) = token[start..].chars().next() {
        if !OPENERS.contains(&c) {
            break;
        }
        start += c.len_utf8();
    }
    let mut end = token.len();
    while let Some(last) = token[start..end].chars().last() {
        let inside = &token[start..end];
        let spare = match last {
            ')' => !balanced(inside, '(', ')'),
            ']' => !balanced(inside, '[', ']'),
            '}' => !balanced(inside, '{', '}'),
            c => CLOSERS.contains(&c),
        };
        if !spare {
            break;
        }
        end -= last.len_utf8();
    }
    start..end
}

/// Does this run close no more brackets than it opens?
fn balanced(run: &str, open: char, close: char) -> bool {
    run.matches(open).count() >= run.matches(close).count()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn found(text: &str) -> Vec<(&str, LinkKind, String)> {
        links(text)
            .into_iter()
            .map(|link| (&text[link.range], link.kind, link.target))
            .collect()
    }

    #[test]
    fn a_url_with_a_scheme_is_a_link_and_keeps_its_path() {
        let text = "See https://example.com/a/b?c=d#e for more";
        assert_eq!(
            found(text),
            vec![(
                "https://example.com/a/b?c=d#e",
                LinkKind::Url,
                "https://example.com/a/b?c=d#e".to_string()
            )]
        );
    }

    #[test]
    fn the_full_stop_at_the_end_of_the_sentence_is_not_part_of_the_link() {
        assert_eq!(found("Read https://example.com/docs.")[0].0, "https://example.com/docs");
        assert_eq!(found("(see https://example.com)")[0].0, "https://example.com");
        assert_eq!(found("<https://example.com>")[0].0, "https://example.com");
        assert_eq!(found("\"https://example.com\",")[0].0, "https://example.com");
    }

    #[test]
    fn a_bracket_the_link_opened_is_part_of_the_link() {
        let text = "https://en.wikipedia.org/wiki/Rust_(programming_language)";
        assert_eq!(found(text)[0].0, text);
        assert_eq!(found(&format!("(see {text})"))[0].0, text);
    }

    #[test]
    fn a_bare_www_host_is_a_url_and_is_given_a_scheme_to_open_with() {
        assert_eq!(
            found("www.gnome.org for the docs"),
            vec![("www.gnome.org", LinkKind::Url, "https://www.gnome.org".to_string())]
        );
    }

    #[test]
    fn an_address_is_a_link_and_opens_with_mailto() {
        assert_eq!(
            found("write to bob.smith+tag@example.co.uk, please"),
            vec![(
                "bob.smith+tag@example.co.uk",
                LinkKind::Email,
                "mailto:bob.smith+tag@example.co.uk".to_string()
            )]
        );
        assert_eq!(found("mailto:bob@example.com")[0].1, LinkKind::Email);
    }

    #[test]
    fn a_dotted_word_without_a_scheme_is_not_a_link() {
        assert!(links("crates/ocr/src/lib.rs is the crate root").is_empty());
        assert!(links("version 1.2.3 of the thing").is_empty());
        assert!(links("Vec::<u8>::new() and 2>&1 and a@b").is_empty());
        assert!(links("ends in a dot. and nothing else").is_empty());
    }

    #[test]
    fn a_scheme_with_nothing_after_it_is_not_a_link() {
        assert!(links("https:// on its own").is_empty());
    }

    #[test]
    fn the_ranges_are_bytes_into_the_text_that_was_searched() {
        let text = "caf\u{e9} \u{2014} https://example.com \u{2014} caf\u{e9}";
        let link = &links(text)[0];
        assert_eq!(&text[link.range.clone()], "https://example.com");
        assert_eq!(text.get(link.range.clone()), Some("https://example.com"));
    }

    #[test]
    fn every_link_on_a_line_is_found_in_order() {
        let text = "a@b.com then https://x.test then www.y.test";
        let kinds: Vec<LinkKind> = links(text).into_iter().map(|l| l.kind).collect();
        assert_eq!(kinds, vec![LinkKind::Email, LinkKind::Url, LinkKind::Url]);
    }

    #[test]
    fn a_link_broken_over_two_lines_is_the_part_on_each_line() {
        // Nothing here glues them back together: an OCR that split a URL at a wrap has
        // lost the information that it was one, and a guess would open the wrong page.
        let text = "https://example.com/very/long\npath/continues";
        assert_eq!(found(text).len(), 1);
        assert_eq!(found(text)[0].0, "https://example.com/very/long");
    }
}
