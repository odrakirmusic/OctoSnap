// SPDX-License-Identifier: GPL-3.0-or-later

//! `HIS-05` and `SYS-13` (D181): finding a capture by what it says, the app it came from or
//! its window's title.
//!
//! Pure, so the history strip's search and the Activities search cannot disagree about
//! what matches. A query is its words, and a capture matches when every word is somewhere
//! in its text, its app's name or id, or its window's title, whatever the case. That is how
//! GNOME's own search reads a query: word by word, each one narrowing what the last found.
//!
//! The text is what the device read in the capture (`HIS-05`), kept beside it as
//! [`TEXT_FILE`]. Accents are not folded: "café" finds "café" and not "cafe".

use crate::history::Entry;

/// The text read in a capture, in its history directory. An empty file is a capture that
/// was read and holds no text, which is not read again.
pub const TEXT_FILE: &str = "text.txt";

/// A query's words, lower-cased. None for a blank query.
#[must_use]
pub fn terms(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// The Activities search's terms, which arrive as words already, as [`terms`] makes them.
#[must_use]
pub fn terms_of(words: &[String]) -> Vec<String> {
    words.iter().flat_map(|word| terms(word)).collect()
}

/// Whether every term is in the capture's text, its app's name or id, or its window's
/// title. No terms match nothing: a blank query is not a search.
#[must_use]
pub fn matches(terms: &[String], entry: &Entry, text: &str) -> bool {
    if terms.is_empty() {
        return false;
    }
    let fields = [
        text.to_lowercase(),
        entry.source.app_name.to_lowercase(),
        entry.source.app_id.to_lowercase(),
        entry.source.title.to_lowercase(),
    ];
    terms.iter().all(|term| fields.iter().any(|field| field.contains(term.as_str())))
}

/// The line of `text` where a term is first found, for a result's description: at most
/// `width` characters, cut at spaces around the term and marked with an ellipsis where it
/// was cut. `None` when no term is in the text -- the match was the app or the title.
#[must_use]
pub fn snippet(text: &str, terms: &[String], width: usize) -> Option<String> {
    let width = width.max(8);
    for line in text.lines() {
        let line = line.trim();
        let lower = line.to_lowercase();
        let Some(at) = terms.iter().filter_map(|term| lower.find(term.as_str())).min() else {
            continue;
        };
        let chars: Vec<char> = line.chars().collect();
        if chars.len() <= width {
            return Some(line.to_owned());
        }
        // Where the term starts, in characters, measured on the lower-cased line; a letter
        // whose lower case is longer can move it by a character, which a snippet can bear.
        let start_char = lower[..at].chars().count().min(chars.len());
        // The term a third of the way in, so the words before it say what it is about.
        let mut from = start_char.saturating_sub(width / 3);
        let mut to = (from + width).min(chars.len());
        from = to.saturating_sub(width);
        // Whole words: back to a space at either end, unless that loses half the room.
        if from > 0
            && let Some(space) = chars[from..to].iter().position(|c| c.is_whitespace())
            && space < width / 2
        {
            from += space + 1;
        }
        if to < chars.len()
            && let Some(space) = chars[from..to].iter().rposition(|c| c.is_whitespace())
            && to - space < width / 2
        {
            to = space;
        }
        let mut out = String::new();
        if from > 0 {
            out.push('\u{2026}');
        }
        out.extend(chars[from..to].iter());
        let out = out.trim_end().to_owned();
        return Some(if to < chars.len() { format!("{out}\u{2026}") } else { out });
    }
    None
}

#[cfg(test)]
// A failing assertion should panic, as in `history`'s tests.
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::capture::SourceWindow;
    use crate::history::Kind;

    fn entry(app_name: &str, app_id: &str, title: &str) -> Entry {
        Entry {
            id: "01J".to_owned(),
            kind: Kind::Screenshot,
            created_at: 0,
            filed_at: 0,
            path: "/tmp/01J.png".into(),
            meta_path: None,
            saved_path: None,
            project_path: None,
            thumb_path: None,
            width: 10,
            height: 10,
            scale: 1.0,
            source: SourceWindow {
                app_id: app_id.to_owned(),
                app_name: app_name.to_owned(),
                title: title.to_owned(),
            },
            display: String::new(),
            duration_ms: None,
        }
    }

    #[test]
    fn a_query_is_its_words_in_lower_case() {
        assert_eq!(terms("  Invoice  MARCH "), ["invoice", "march"]);
        assert!(terms(" \t ").is_empty());
        let words = ["Invoice March".to_owned(), "2026".to_owned()];
        assert_eq!(terms_of(&words), ["invoice", "march", "2026"]);
    }

    #[test]
    fn every_word_has_to_be_found_somewhere() {
        let firefox = entry("Firefox", "org.mozilla.firefox", "Pull request 56 - GitHub");
        let text = "Total due: 49.90 CHF\nInvoice 2026-03";
        assert!(matches(&terms("invoice"), &firefox, text));
        assert!(matches(&terms("INVOICE chf"), &firefox, text));
        // One word in the text and one in the title.
        assert!(matches(&terms("invoice github"), &firefox, text));
        // The app's name, and its id.
        assert!(matches(&terms("firefox"), &firefox, ""));
        assert!(matches(&terms("mozilla"), &firefox, ""));
        // A word nowhere is no match, whatever the others found.
        assert!(!matches(&terms("invoice receipt"), &firefox, text));
        // A blank query finds nothing.
        assert!(!matches(&[], &firefox, text));
    }

    #[test]
    fn a_word_matches_inside_a_longer_one() {
        let e = entry("Files", "org.gnome.Nautilus", "");
        assert!(matches(&terms("screen"), &e, "Screenshots from 2026"));
        assert!(matches(&terms("2026"), &e, "Screenshots from 2026"));
    }

    #[test]
    fn a_snippet_is_the_line_the_word_is_on() {
        let text = "Welcome back\nTotal due: 49.90 CHF\nThanks";
        assert_eq!(snippet(text, &terms("chf"), 60).as_deref(), Some("Total due: 49.90 CHF"));
        assert_eq!(snippet(text, &terms("nowhere"), 60), None);
        // The first line with any of the words.
        assert_eq!(
            snippet(text, &terms("thanks total"), 60).as_deref(),
            Some("Total due: 49.90 CHF")
        );
    }

    #[test]
    fn a_long_line_is_cut_around_the_word_at_spaces() {
        let line = "The quick brown fox jumps over the lazy dog while the invoice number \
                    INV-2026-0042 waits at the end of a very long line of text";
        let cut = snippet(line, &terms("inv-2026"), 40).expect("a snippet");
        assert!(cut.contains("INV-2026-0042"), "{cut}");
        assert!(cut.starts_with('\u{2026}') && cut.ends_with('\u{2026}'), "{cut}");
        assert!(cut.chars().count() <= 42, "{cut}");
        // Cut at spaces: no half words at the ends.
        let inner = cut.trim_matches('\u{2026}');
        for word in inner.split_whitespace() {
            assert!(line.split_whitespace().any(|w| w == word), "{word} in {cut}");
        }
    }

    #[test]
    fn a_word_at_the_start_of_a_long_line_has_no_ellipsis_before_it() {
        let line =
            "Invoice INV-2026-0042 for March, due on the last day of the month, in Swiss francs";
        let cut = snippet(line, &terms("invoice"), 30).expect("a snippet");
        assert!(cut.starts_with("Invoice"), "{cut}");
        assert!(cut.ends_with('\u{2026}'), "{cut}");
    }
}
