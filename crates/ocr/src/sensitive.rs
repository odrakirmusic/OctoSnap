// SPDX-License-Identifier: GPL-3.0-or-later

//! `ANN-26` (D178): what in a picture's text is not for sharing, and where it is.
//!
//! One action in the editor reads the picture and covers each of these with an ordinary
//! redaction, which the user can then move, restyle or delete like one they drew:
//!
//! - e-mail addresses;
//! - phone numbers written as phone numbers are: international (`+41 22 555 12 34`,
//!   `0041 …`), national with a leading `0` or a bracketed area code, and North American
//!   (`555-123-4567`);
//! - IBANs, which carry their own check digits, and card numbers, which pass Luhn's check
//!   and start the way a card network's do;
//! - IPv4 and IPv6 addresses;
//! - keys and tokens: the prefixes the big providers give theirs, JSON web tokens, and any
//!   long random-looking run of letters and digits;
//! - credentials in links (`https://user:password@host`, `?token=…`), and whatever follows
//!   a label such as `Password:`, `api_key=` or `Bearer`.
//!
//! By hand rather than with regular expressions, for `links`'s reason: the hard part is
//! where a thing stops. And over the text first, then back to the boxes, so every rule is
//! a test on a string.
//!
//! A false alarm costs a redaction the user deletes; a miss leaks the thing. So where the
//! two pull apart the rules lean to covering, within reason: a version number shaped like
//! an IPv4 address is covered, a date or a price is not.

use std::net::{Ipv4Addr, Ipv6Addr};
use std::ops::Range;

use crate::links::{is_email, tokens, trim};
use crate::{Bounds, Word};

/// What a covered run is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Email,
    Phone,
    Iban,
    Card,
    Ip,
    Secret,
    Credential,
}

impl Kind {
    /// For the editor's toast and log: what was covered.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Email => "e-mail address",
            Self::Phone => "phone number",
            Self::Iban => "IBAN",
            Self::Card => "card number",
            Self::Ip => "IP address",
            Self::Secret => "key or token",
            Self::Credential => "credential",
        }
    }

    /// [`Self::label`] for more than one.
    #[must_use]
    pub const fn plural(self) -> &'static str {
        match self {
            Self::Email => "e-mail addresses",
            Self::Phone => "phone numbers",
            Self::Iban => "IBANs",
            Self::Card => "card numbers",
            Self::Ip => "IP addresses",
            Self::Secret => "keys or tokens",
            Self::Credential => "credentials",
        }
    }
}

/// A run of text to cover: a byte range in the text it was found in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub kind: Kind,
}

/// A place in the picture to cover, in the picture's own pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Found {
    pub kind: Kind,
    pub bounds: Bounds,
}

/// The prefixes providers put on their keys, with how many characters at least follow.
const PREFIXES: &[(&str, usize)] = &[
    ("AKIA", 16),
    ("ASIA", 16),
    ("ghp_", 30),
    ("gho_", 30),
    ("ghu_", 30),
    ("ghs_", 30),
    ("ghr_", 30),
    ("github_pat_", 30),
    ("glpat-", 20),
    ("xoxb-", 20),
    ("xoxp-", 20),
    ("xoxa-", 20),
    ("xoxr-", 20),
    ("xoxs-", 20),
    ("sk-", 20),
    ("sk_live_", 16),
    ("sk_test_", 16),
    ("rk_live_", 16),
    ("pk_live_", 16),
    ("AIza", 30),
    ("ya29.", 20),
    ("npm_", 30),
    ("pypi-", 30),
    ("hf_", 30),
    ("dop_v1_", 30),
    ("SG.", 30),
];

/// Labels whose value is a secret wherever the label ends a longer name: `DB_PASSWORD=`,
/// `client_secret:`, `X-Api-Key:`.
const LABEL_ENDINGS: &[&str] = &[
    "password",
    "passwd",
    "passphrase",
    "secret",
    "token",
    "api_key",
    "apikey",
    "access_key",
    "private_key",
    "credentials",
];

/// Labels that are only labels on their own: `pin` ends `spin`, and `pass` ends `compass`.
const LABELS: &[&str] = &["pass", "pwd", "pin", "auth", "authorization", "key"];

/// Query parameters whose value is a credential.
const SECRET_PARAMS: &[&str] = &[
    "token",
    "access_token",
    "refresh_token",
    "id_token",
    "api_key",
    "apikey",
    "key",
    "secret",
    "client_secret",
    "password",
    "pass",
    "pwd",
    "sig",
    "signature",
    "auth",
    "code",
];

/// The shortest run the entropy rule looks at, and how random it has to look.
const RANDOM_LENGTH: usize = 32;
const RANDOM_BITS: f64 = 4.0;

/// Every run of `text` to cover, in order, overlapping runs merged.
#[must_use]
pub fn spans(text: &str) -> Vec<Span> {
    let mut found = Vec::new();
    ibans(text, &mut found);
    numbers(text, &mut found);
    let tokens = tokens(text);
    for (index, &(at, token)) in tokens.iter().enumerate() {
        // A bracketed IPv6 address keeps its brackets, which `trim` takes for punctuation.
        let bracketed = token.trim_end_matches([',', ';', '.', ')']);
        if bracketed.starts_with('[') && is_ip(bracketed) {
            found.push(Span { range: at..at + bracketed.len(), kind: Kind::Ip });
            continue;
        }
        let span = trim(token);
        if span.is_empty() {
            continue;
        }
        let start = at + span.start;
        one_token(&token[span.clone()], start, &mut found);
        // The label rule wants the run to its end: `trim` takes the colon off `Password:`.
        labelled(&token[span.start..], start, tokens.get(index + 1), &mut found);
    }
    merge(found)
}

/// What one whitespace-free run is, if anything.
fn one_token(token: &str, at: usize, found: &mut Vec<Span>) {
    let lower = token.to_ascii_lowercase();
    let address = lower.strip_prefix("mailto:").unwrap_or(&lower);
    if is_email(address) {
        let skip = lower.len() - address.len();
        found.push(Span { range: at + skip..at + token.len(), kind: Kind::Email });
        return;
    }
    if let Some(scheme_end) = token.find("://") {
        link(token, at, scheme_end + 3, found);
        return;
    }
    if is_ip(token) {
        found.push(Span { range: at..at + token.len(), kind: Kind::Ip });
        return;
    }
    if is_secret(token) {
        found.push(Span { range: at..at + token.len(), kind: Kind::Secret });
    }
}

/// A link's credentials: the user and password before its host, and any query parameter
/// that carries a key or a token.
fn link(token: &str, at: usize, authority_start: usize, found: &mut Vec<Span>) {
    let rest = &token[authority_start..];
    let authority_end = rest.find(['/', '?', '#']).map_or(token.len(), |i| authority_start + i);
    let authority = &token[authority_start..authority_end];
    if let Some(user) = authority.rfind('@').filter(|&i| i > 0) {
        found.push(Span {
            range: at + authority_start..at + authority_start + user,
            kind: Kind::Credential,
        });
    }
    let Some(query) = token.find('?') else { return };
    let end = token[query..].find('#').map_or(token.len(), |i| query + i);
    let mut from = query + 1;
    for pair in token[query + 1..end].split('&') {
        if let Some((name, value)) = pair.split_once('=')
            && !value.is_empty()
            && SECRET_PARAMS.contains(&name.to_ascii_lowercase().as_str())
        {
            let start = from + name.len() + 1;
            found
                .push(Span { range: at + start..at + start + value.len(), kind: Kind::Credential });
        }
        from += pair.len() + 1;
    }
}

/// An IPv4 address, possibly with a port or a prefix length, or an IPv6 one, possibly in
/// brackets.
fn is_ip(token: &str) -> bool {
    let bare = token.split('/').next().unwrap_or(token);
    let v4 = bare.rsplit_once(':').map_or(bare, |(host, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) && host.contains('.') { host } else { bare }
    });
    if v4.parse::<Ipv4Addr>().is_ok() {
        return true;
    }
    let v6 = bare.strip_prefix('[').and_then(|b| b.split_once(']')).map_or(bare, |(host, _)| host);
    v6.matches(':').count() >= 2
        && v6.chars().filter(char::is_ascii_hexdigit).count() >= 3
        && v6.parse::<Ipv6Addr>().is_ok()
}

/// A key or a token: a provider's prefix, a JSON web token, or a long random run.
fn is_secret(token: &str) -> bool {
    let key_char = |c: char| c.is_ascii_alphanumeric() || "-_./+=".contains(c);
    if !token.chars().all(key_char) {
        return false;
    }
    for (prefix, after) in PREFIXES {
        if let Some(rest) = token.strip_prefix(prefix)
            && rest.len() >= *after
        {
            return true;
        }
    }
    if token.starts_with("eyJ") {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() == 3 && parts.iter().all(|part| part.len() >= 10) {
            return true;
        }
    }
    token.len() >= RANDOM_LENGTH
        && token.chars().any(|c| c.is_ascii_uppercase())
        && token.chars().any(|c| c.is_ascii_lowercase())
        && token.chars().any(|c| c.is_ascii_digit())
        && entropy(token) >= RANDOM_BITS
}

/// Shannon entropy in bits per character.
fn entropy(text: &str) -> f64 {
    let mut counts = [0_u32; 128];
    let mut total = 0_u32;
    for byte in text.bytes().filter(u8::is_ascii) {
        counts[usize::from(byte)] += 1;
        total += 1;
    }
    if total == 0 {
        return 0.0;
    }
    let total = f64::from(total);
    counts
        .iter()
        .filter(|&&n| n > 0)
        .map(|&n| {
            let p = f64::from(n) / total;
            -p * p.log2()
        })
        .sum()
}

/// A label's value: `password=hunter2` in one run, `Password: hunter2` across two, and
/// the token after `Bearer`.
fn labelled(token: &str, at: usize, next: Option<&(usize, &str)>, found: &mut Vec<Span>) {
    if let Some((name, value)) = token.split_once(['=', ':'])
        && is_label(name)
    {
        let kept = trim(value);
        if is_value(&value[kept.clone()]) {
            let start = at + name.len() + 1;
            found
                .push(Span { range: start + kept.start..start + kept.end, kind: Kind::Credential });
            return;
        }
        if !value.is_empty() {
            return;
        }
    } else if !is_scheme(token) {
        return;
    }
    // The value is the next run: after `Password:` or `Bearer`. After `Authorization:` the
    // next run is `Bearer`, which is not the secret: its own turn covers what follows it.
    let Some(&(next_at, next)) = next else { return };
    let span = trim(next);
    let value = &next[span.clone()];
    if is_value(value) && !value.ends_with(':') && !is_scheme(value) {
        found
            .push(Span { range: next_at + span.start..next_at + span.end, kind: Kind::Credential });
    }
}

/// An HTTP authentication scheme, which a credential follows.
fn is_scheme(token: &str) -> bool {
    token.eq_ignore_ascii_case("bearer") || token.eq_ignore_ascii_case("basic")
}

fn is_label(name: &str) -> bool {
    let name: String = name
        .trim_matches(|c: char| !c.is_ascii_alphanumeric())
        .chars()
        .map(|c| if c == '-' || c == ' ' { '_' } else { c.to_ascii_lowercase() })
        .collect();
    !name.is_empty()
        && (LABELS.contains(&name.as_str())
            || LABEL_ENDINGS.iter().any(|ending| name.ends_with(ending)))
}

/// Worth covering as a label's value: three characters or more, and not already masked.
fn is_value(value: &str) -> bool {
    value.chars().count() >= 3 && !value.chars().all(|c| matches!(c, '*' | '•' | '●' | 'x' | 'X'))
}

/// IBANs: a country, two check digits, then letters and digits in groups of four, which
/// pass ISO 13616's mod-97 check. The longest run of groups that does.
fn ibans(text: &str, found: &mut Vec<Span>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        let starts = (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
            && bytes[i].is_ascii_uppercase()
            && bytes[i + 1].is_ascii_uppercase()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit();
        if !starts {
            i += 1;
            continue;
        }
        // Every place the run could end: after each letter or digit, through single spaces.
        let mut ends = Vec::new();
        let mut j = i;
        let mut count = 0;
        while j < bytes.len() && count < 34 {
            if bytes[j].is_ascii_uppercase() || bytes[j].is_ascii_digit() {
                count += 1;
                j += 1;
                ends.push((j, count));
            } else if bytes[j] == b' '
                && j + 1 < bytes.len()
                && bytes[j + 1].is_ascii_alphanumeric()
            {
                j += 1;
            } else {
                break;
            }
        }
        let valid = ends.iter().rev().find(|&&(end, count)| {
            (15..=34).contains(&count)
                && (end == bytes.len() || !bytes[end].is_ascii_alphanumeric())
                && mod97(&text[i..end])
        });
        match valid {
            Some(&(end, _)) => {
                found.push(Span { range: i..end, kind: Kind::Iban });
                i = end;
            }
            None => i += 1,
        }
    }
}

/// ISO 13616: the first four characters moved to the end, letters as 10 to 35, and the
/// number modulo 97 is 1.
fn mod97(iban: &str) -> bool {
    let compact: Vec<u8> = iban.bytes().filter(|b| *b != b' ').collect();
    let (head, tail) = compact.split_at(4);
    let mut remainder = 0_u32;
    for &b in tail.iter().chain(head) {
        let value = match b {
            b'0'..=b'9' => u32::from(b - b'0'),
            b'A'..=b'Z' => u32::from(b - b'A') + 10,
            _ => return false,
        };
        remainder = if value >= 10 {
            (remainder * 100 + value) % 97
        } else {
            (remainder * 10 + value) % 97
        };
    }
    remainder == 1
}

/// Card and phone numbers: runs of digits with the separators they are written with.
fn numbers(text: &str, found: &mut Vec<Span>) {
    let bytes = text.as_bytes();
    let joins = |b: u8| matches!(b, b' ' | b'-' | b'.' | b'(' | b')' | b'/');
    let mut i = 0;
    while i < bytes.len() {
        let opens = bytes[i].is_ascii_digit() || bytes[i] == b'+' || bytes[i] == b'(';
        if !opens || (i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'.')) {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < bytes.len() {
            let b = bytes[j];
            // A separator goes on when a digit or a bracket follows it, and `(415) 555`'s
            // closing bracket when a space and a digit do.
            let next = |k: usize| bytes.get(j + k).copied().unwrap_or(0);
            let goes_on = next(1).is_ascii_digit()
                || next(1) == b'('
                || (b == b')' && next(1) == b' ' && next(2).is_ascii_digit());
            if b.is_ascii_digit() || (joins(b) && goes_on) {
                // Two separators in a row end it, but for a bracket's close and a space.
                if joins(b) && joins(bytes[j - 1]) && !(bytes[j - 1] == b')' && b == b' ') {
                    break;
                }
                j += 1;
            } else {
                break;
            }
        }
        // It ends on a digit, and not in the middle of a word or a longer number.
        let mut end = j;
        while end > i && !bytes[end - 1].is_ascii_digit() {
            end -= 1;
        }
        let run = &text[i..end];
        let alone = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if alone && let Some(kind) = number(run) {
            found.push(Span { range: i..end, kind });
            i = end;
        } else {
            i = j.max(i + 1);
        }
    }
}

/// What a run of digits and separators is, if it is a card or a phone number.
fn number(run: &str) -> Option<Kind> {
    let digits: Vec<u8> = run.bytes().filter(u8::is_ascii_digit).map(|b| b - b'0').collect();
    let groups: Vec<&str> =
        run.split([' ', '-', '.', '(', ')', '/']).filter(|g| !g.is_empty()).collect();
    if is_card(run, &digits, &groups) {
        return Some(Kind::Card);
    }
    is_phone(run, &digits, &groups).then_some(Kind::Phone)
}

fn is_card(run: &str, digits: &[u8], groups: &[&str]) -> bool {
    if !(13..=19).contains(&digits.len()) || run.contains(['(', ')', '.', '/', '+']) {
        return false;
    }
    // Written as one run, or in groups of four (the last shorter), or Amex's 4-6-5.
    let shaped = groups.len() == 1
        || groups[..groups.len() - 1].iter().all(|g| g.len() == 4)
        || groups.iter().map(|g| g.len()).collect::<Vec<_>>() == [4, 6, 5];
    shaped && luhn(digits) && network(digits)
}

/// Luhn's check, which every card number passes and one random number in ten does.
fn luhn(digits: &[u8]) -> bool {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            let d = u32::from(d);
            if i % 2 == 1 { if d * 2 > 9 { d * 2 - 9 } else { d * 2 } } else { d }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// Whether the number starts the way a card network's do: Visa, Mastercard, American
/// Express, Discover, JCB, Diners Club, UnionPay and Maestro.
fn network(digits: &[u8]) -> bool {
    let lead = |n: usize| digits.iter().take(n).fold(0_u32, |acc, &d| acc * 10 + u32::from(d));
    let (one, two, four) = (lead(1), lead(2), lead(4));
    one == 4
        || (51..=55).contains(&two)
        || (2221..=2720).contains(&four)
        || two == 34
        || two == 37
        || four == 6011
        || two == 65
        || (644..=649).contains(&lead(3))
        || (3528..=3589).contains(&four)
        || two == 36
        || two == 38
        || (300..=305).contains(&lead(3))
        || two == 62
        || two == 50
        || (56..=69).contains(&two)
}

fn is_phone(run: &str, digits: &[u8], groups: &[&str]) -> bool {
    if run.starts_with('+') || run.starts_with("00") {
        return (8..=15).contains(&digits.len()) && (groups.len() >= 2 || digits.len() >= 10);
    }
    if run.starts_with('0') || run.starts_with('(') {
        return (9..=12).contains(&digits.len()) && groups.len() >= 2 && !is_date(groups);
    }
    // North American: 555-123-4567 or 555.123.4567, and (555) 123-4567 above.
    groups.len() == 3
        && digits.len() == 10
        && groups[0].len() == 3
        && groups[1].len() == 3
        && groups[2].len() == 4
        && !run.contains(' ')
}

/// `01.02.2026` or `09/10/2026`: day, month and year, which a phone number is not.
fn is_date(groups: &[&str]) -> bool {
    groups.len() == 3 && groups[0].len() <= 2 && groups[1].len() <= 2 && groups[2].len() == 4
}

/// Sorted, and runs that overlap or touch made one.
fn merge(mut found: Vec<Span>) -> Vec<Span> {
    found.sort_by_key(|span| (span.range.start, std::cmp::Reverse(span.range.end)));
    let mut merged: Vec<Span> = Vec::new();
    for span in found {
        match merged.last_mut() {
            Some(last) if span.range.start <= last.range.end => {
                last.range.end = last.range.end.max(span.range.end);
            }
            _ => merged.push(span),
        }
    }
    merged
}

/// What to cover in a picture, from the boxes its engine read.
///
/// The boxes go into lines first, since a phone number or a `Password:` label and its
/// value can be two boxes, and each line's text is searched. A run that is part of a box is
/// placed by its share of the box's characters, which is close for the fonts screens use,
/// and widened by half a character each side and a little above and below, so that what a
/// redaction leaves visible of a proportional font is its neighbours and not its ends.
#[must_use]
pub fn find(words: &[Word]) -> Vec<Found> {
    let mut found = Vec::new();
    for line in crate::shape::rows_of(words) {
        let mut text = String::new();
        let mut starts = Vec::with_capacity(line.len());
        for word in &line {
            if !text.is_empty() {
                text.push(' ');
            }
            starts.push(text.len());
            text.push_str(&word.text);
        }
        for span in spans(&text) {
            let mut bounds: Option<Bounds> = None;
            for (word, &start) in line.iter().zip(&starts) {
                let end = start + word.text.len();
                if span.range.end <= start || span.range.start >= end {
                    continue;
                }
                let from = span.range.start.max(start) - start;
                let to = span.range.end.min(end) - start;
                let part = part_of(word, from, to);
                bounds = Some(bounds.map_or(part, |b| b.union(part)));
            }
            if let Some(bounds) = bounds {
                found.push(Found { kind: span.kind, bounds });
            }
        }
    }
    found
}

/// The part of `word`'s box that holds its text's bytes `from..to`, widened.
fn part_of(word: &Word, from: usize, to: usize) -> Bounds {
    let b = word.bounds;
    let chars = word.text.chars().count().max(1);
    let before = word.text[..from].chars().count();
    let through = word.text[..to].chars().count();
    #[allow(clippy::cast_precision_loss)]
    let per = f64::from(b.width) / chars as f64;
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let (left, right) = (
        f64::from(b.x) + per * (before as f64 - 0.5),
        f64::from(b.x) + per * (through as f64 + 0.5),
    );
    let pad = (f64::from(b.height) * 0.15).ceil();
    #[allow(clippy::cast_possible_truncation)]
    let (left, right) =
        ((left.floor() as i32).max(b.x - 2), (right.ceil() as i32).min(b.right() + 2));
    #[allow(clippy::cast_possible_truncation)]
    let pad = pad as i32;
    Bounds::new(left, b.y - pad, right - left, b.height + 2 * pad)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn covered(text: &str) -> Vec<(&str, Kind)> {
        spans(text).into_iter().map(|span| (&text[span.range], span.kind)).collect()
    }

    #[test]
    fn an_email_address_is_covered_and_its_sentence_is_not() {
        assert_eq!(covered("Write to anna.k@example.org."), [("anna.k@example.org", Kind::Email)]);
        assert_eq!(covered("<mailto:me@x.ch>"), [("me@x.ch", Kind::Email)]);
        assert_eq!(covered("Vec::<u8>::new() and a@b"), []);
    }

    #[test]
    fn phone_numbers_as_people_write_them() {
        assert_eq!(covered("Call +41 22 555 12 34 today"), [("+41 22 555 12 34", Kind::Phone)]);
        assert_eq!(covered("Tel. 0041 22 555 12 34"), [("0041 22 555 12 34", Kind::Phone)]);
        assert_eq!(covered("Office: 022 555 12 34,"), [("022 555 12 34", Kind::Phone)]);
        assert_eq!(covered("(415) 555-0132"), [("(415) 555-0132", Kind::Phone)]);
        assert_eq!(covered("ring 415-555-0132"), [("415-555-0132", Kind::Phone)]);
        assert_eq!(covered("+44 20 7946 0958"), [("+44 20 7946 0958", Kind::Phone)]);
    }

    #[test]
    fn dates_times_prices_and_versions_are_not_phone_numbers() {
        assert_eq!(covered("2026-10-09 14:30"), []);
        assert_eq!(covered("on 09.10.2026"), []);
        assert_eq!(covered("CHF 1 250.00"), []);
        assert_eq!(covered("version 0.1.9"), []);
        assert_eq!(covered("order 0123456789"), []);
        assert_eq!(covered("12 345 678 people"), []);
    }

    #[test]
    fn an_iban_is_covered_when_its_check_digits_agree() {
        assert_eq!(
            covered("IBAN CH93 0076 2011 6238 5295 7"),
            [("CH93 0076 2011 6238 5295 7", Kind::Iban)]
        );
        assert_eq!(covered("GB82WEST12345698765432 BIC"), [("GB82WEST12345698765432", Kind::Iban)]);
        // One digit off, and it is not an IBAN.
        assert_eq!(covered("CH94 0076 2011 6238 5295 7"), []);
        // Followed by capitals that are not part of it: the longest run that checks out.
        assert_eq!(
            covered("DE89 3704 0044 0532 0130 00 BIC COBADEFFXXX").first().map(|c| c.0),
            Some("DE89 3704 0044 0532 0130 00")
        );
    }

    #[test]
    fn a_card_number_passes_luhn_and_starts_like_a_card() {
        assert_eq!(covered("Visa 4111 1111 1111 1111 exp"), [("4111 1111 1111 1111", Kind::Card)]);
        assert_eq!(covered("5500-0000-0000-0004"), [("5500-0000-0000-0004", Kind::Card)]);
        assert_eq!(covered("Amex 3782 822463 10005"), [("3782 822463 10005", Kind::Card)]);
        // Luhn fails.
        assert_eq!(covered("4111 1111 1111 1112"), []);
        // Luhn passes, but no network starts with 9.
        assert_eq!(covered("9111 1111 1111 1111"), []);
    }

    #[test]
    fn ip_addresses_of_both_kinds() {
        assert_eq!(covered("from 192.168.1.20 at"), [("192.168.1.20", Kind::Ip)]);
        assert_eq!(covered("10.0.0.1:8080"), [("10.0.0.1:8080", Kind::Ip)]);
        assert_eq!(covered("route 10.0.0.0/8"), [("10.0.0.0/8", Kind::Ip)]);
        assert_eq!(covered("2001:db8::8a2e:370:7334"), [("2001:db8::8a2e:370:7334", Kind::Ip)]);
        assert_eq!(covered("[fe80::1]:22"), [("[fe80::1]:22", Kind::Ip)]);
        assert_eq!(covered("at 12:30:45"), []);
        assert_eq!(covered("999.1.1.1"), []);
    }

    #[test]
    fn keys_and_tokens() {
        let github = format!("token ghp_{}", "a1B2c3D4e5".repeat(4));
        assert_eq!(covered(&github)[0].1, Kind::Secret);
        assert_eq!(covered("AKIAIOSFODNN7EXAMPLE")[0].1, Kind::Secret);
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        assert_eq!(covered(jwt), [(jwt, Kind::Secret)]);
        assert_eq!(
            covered("x 9fG2kLq8Zt4Wn7Vb1Rc6Yp3Hs5Jd0Mx2Ae8Uo"),
            [("9fG2kLq8Zt4Wn7Vb1Rc6Yp3Hs5Jd0Mx2Ae8Uo", Kind::Secret)]
        );
    }

    #[test]
    fn long_words_and_paths_are_not_keys() {
        assert_eq!(covered("Donaudampfschifffahrtsgesellschaftskapitän"), []);
        assert_eq!(covered("crates/app/src/editor/window_background.rs"), []);
        assert_eq!(covered("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1A"), []);
    }

    #[test]
    fn credentials_in_links() {
        assert_eq!(
            covered("git clone https://anna:s3cret@git.example.com/repo.git"),
            [("anna:s3cret", Kind::Credential)]
        );
        assert_eq!(
            covered("https://api.example.com/v1?user=7&access_token=abc123def&x=1"),
            [("abc123def", Kind::Credential)]
        );
        assert_eq!(covered("https://example.com/about"), []);
    }

    #[test]
    fn what_follows_a_label() {
        assert_eq!(covered("Password: hunter2"), [("hunter2", Kind::Credential)]);
        assert_eq!(covered("DB_PASSWORD=correct-horse"), [("correct-horse", Kind::Credential)]);
        assert_eq!(covered("X-Api-Key: 0f9e8d7c"), [("0f9e8d7c", Kind::Credential)]);
        assert_eq!(
            covered("Authorization: Bearer abc.def.ghi"),
            [("abc.def.ghi", Kind::Credential)]
        );
        assert_eq!(
            covered("Authorization: Basic YW5uYTpzM2NyZXQ="),
            [("YW5uYTpzM2NyZXQ=", Kind::Credential)]
        );
        assert_eq!(covered("password=hunter2, then"), [("hunter2", Kind::Credential)]);
        assert_eq!(covered("PIN: 4821"), [("4821", Kind::Credential)]);
        // Already masked, a compass, and a label with nothing after it.
        assert_eq!(covered("Password: ********"), []);
        assert_eq!(covered("compass: north"), []);
        assert_eq!(covered("Password:"), []);
    }

    #[test]
    fn runs_that_overlap_are_one() {
        let text = "password=4111111111111111";
        assert_eq!(covered(text).len(), 1);
    }

    #[test]
    fn a_run_is_placed_by_its_share_of_its_box() {
        // "Mail: anna@example.org" in one box 220 px wide, 22 characters: 10 px each.
        let words = [Word::new("Mail: anna@example.org", Bounds::new(100, 50, 220, 20), 0.99)];
        let found = find(&words);
        assert_eq!(found.len(), 1);
        let b = found[0].bounds;
        assert_eq!(found[0].kind, Kind::Email);
        // "anna@" starts at the seventh character: 160 px, less half a character.
        assert_eq!((b.x, b.right()), (155, 322));
        assert!(b.y < 50 && b.bottom() > 70, "{b:?}");
    }

    #[test]
    fn a_label_and_its_value_in_two_boxes_on_one_line() {
        let words = [
            Word::new("hunter2", Bounds::new(200, 101, 70, 18), 0.98),
            Word::new("Password:", Bounds::new(100, 100, 90, 20), 0.99),
            Word::new("Username: anna", Bounds::new(100, 140, 140, 20), 0.99),
        ];
        let found = find(&words);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].kind, Kind::Credential);
        assert!(
            found[0].bounds.x >= 194 && found[0].bounds.right() <= 272,
            "{:?}",
            found[0].bounds
        );
    }

    #[test]
    fn a_phone_number_split_over_boxes_is_one_redaction() {
        let words = [
            Word::new("+41 22", Bounds::new(10, 10, 60, 16), 0.99),
            Word::new("555 12 34", Bounds::new(76, 10, 90, 16), 0.99),
        ];
        let found = find(&words);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].bounds.x <= 10 && found[0].bounds.right() >= 166, "{:?}", found[0].bounds);
    }
}
