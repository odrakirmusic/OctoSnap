// SPDX-License-Identifier: GPL-3.0-or-later

//! CTC recognition: what the text says.
//!
//! One line of type at a time, scaled to the 48 rows the model was trained on, and its
//! output decoded against the character list PaddleX ships inside the model's
//! `inference.yml`.

use crate::Gray;

/// The row count every recognition model here was trained at (`image_shape: [3, 48, 320]`).
pub const ROWS: u32 = 48;
/// The narrowest crop worth scaling up to `ROWS`. Below it there is no glyph left.
const NARROWEST: u32 = 16;
/// The widest. The model's own dynamic shapes stop at 3200, and a line wider than this at
/// 48 rows is a ruled line rather than type.
const WIDEST: u32 = 3200;

/// One line of type as the model wants it: NCHW, three channels of the same grey, scaled
/// to [-1, 1].
#[must_use]
pub fn tensor(line: &Gray) -> (Vec<f32>, u32) {
    let ratio = f64::from(line.width()) / f64::from(line.height().max(1));
    let width = ((f64::from(ROWS) * ratio).round() as u32).clamp(NARROWEST, WIDEST);
    let scaled = line.resized(width, ROWS);
    let plane: Vec<f32> =
        scaled.pixels().iter().map(|&p| f32::from(p) / 127.5 - 1.0).collect();
    let mut out = Vec::with_capacity(plane.len() * 3);
    for _ in 0..3 {
        out.extend_from_slice(&plane);
    }
    (out, width)
}

/// What the model said, and how sure it was.
///
/// Greedy CTC, which is what PaddleOCR's own `CTCLabelDecode` does: take the likeliest
/// class at every step, drop the blank, and collapse a class that repeats at adjacent
/// steps. The confidence is the mean of the probabilities of the steps that survived --
/// the ones that actually produced a character.
#[must_use]
pub fn decode(
    logits: &[f32],
    steps: usize,
    classes: usize,
    characters: &[String],
) -> (String, f32) {
    let mut text = String::new();
    let mut sure = 0.0f32;
    let mut kept = 0usize;
    let mut previous = usize::MAX;
    for step in 0..steps {
        let Some(row) = logits.get(step * classes..(step + 1) * classes) else { break };
        let mut best = 0usize;
        for (class, value) in row.iter().enumerate() {
            if *value > row[best] {
                best = class;
            }
            let _ = value;
        }
        if best != 0 && best != previous && let Some(character) = characters.get(best) {
            text.push_str(character);
            sure += row[best];
            kept += 1;
        }
        previous = best;
    }
    (text, if kept == 0 { 0.0 } else { sure / kept as f32 })
}

/// The class list for a model, read out of the `inference.yml` beside it.
///
/// PaddleX writes the recogniser's alphabet into the model's own metadata as a
/// `character_dict:` block of YAML scalars. Parsed by hand and not by a YAML crate,
/// because this is the only YAML this application will ever read and the shape of it is
/// fixed by the exporter: one `  - item` a line, single-quoted where the item would not
/// survive unquoted, with `''` for a quote.
///
/// Class 0 is CTC's blank. `classes` is the width of the model's own output, and the
/// difference between it and the dictionary is how the space is detected: PaddleOCR
/// appends one when the model was trained with `use_space_char`, and the metadata does not
/// say whether it was.
#[must_use]
pub fn characters(yml: &str, classes: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut inside = false;
    for line in yml.lines() {
        if line.trim_end() == "  character_dict:" {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        let Some(item) = line.strip_prefix("  - ") else { break };
        out.push(unquote(item));
    }
    while out.len() < classes {
        out.push(" ".to_string());
    }
    out.truncate(classes.max(1));
    out
}

fn unquote(item: &str) -> String {
    let item = item.trim_end_matches(['\r', '\n']);
    match item.strip_prefix('\'').and_then(|rest| rest.strip_suffix('\'')) {
        Some(inner) => inner.replace("''", "'"),
        None => item.to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    const YML: &str = concat!(
        "PostProcess:\n  name: CTCLabelDecode\n  character_dict:\n",
        "  - '0'\n  - a\n  - ''''\n  - \u{2660}\nGlobal:\n  x: 1\n"
    );

    fn probabilities(rows: &[&[f32]]) -> (Vec<f32>, usize, usize) {
        let classes = rows.first().map_or(0, |row| row.len());
        (rows.concat(), rows.len(), classes)
    }

    #[test]
    fn the_alphabet_comes_out_of_the_models_own_metadata() {
        // Blank, then the four entries: the quoted digit, the bare letter, the escaped
        // quote and a character no exporter would leave unquoted by accident.
        let characters = characters(YML, 5);
        assert_eq!(characters, vec!["", "0", "a", "'", "\u{2660}"]);
    }

    #[test]
    fn a_model_with_one_class_more_than_its_dictionary_was_trained_with_a_space() {
        assert_eq!(characters(YML, 6).last().map(String::as_str), Some(" "));
        // And one that was not gets no space bolted on.
        assert_eq!(characters(YML, 5).len(), 5);
    }

    #[test]
    fn ctc_drops_the_blank_and_collapses_what_repeats() {
        let characters = characters(YML, 5);
        // blank, 0, 0, blank, 0, a, a  ->  "00a"
        let (logits, steps, classes) = probabilities(&[
            &[0.9, 0.1, 0.0, 0.0, 0.0],
            &[0.1, 0.9, 0.0, 0.0, 0.0],
            &[0.1, 0.8, 0.0, 0.0, 0.0],
            &[0.7, 0.3, 0.0, 0.0, 0.0],
            &[0.2, 0.8, 0.0, 0.0, 0.0],
            &[0.0, 0.1, 0.9, 0.0, 0.0],
            &[0.0, 0.1, 0.7, 0.0, 0.0],
        ]);
        let (text, sure) = decode(&logits, steps, classes, &characters);
        assert_eq!(text, "00a");
        // The mean of the three steps that produced a character: 0.9, 0.8, 0.9.
        assert!((sure - 0.866_666_7).abs() < 1e-5, "{sure}");
    }

    #[test]
    fn a_line_of_nothing_but_blanks_is_an_empty_string_and_no_confidence() {
        let characters = characters(YML, 5);
        let (logits, steps, classes) =
            probabilities(&[&[1.0, 0.0, 0.0, 0.0, 0.0], &[1.0, 0.0, 0.0, 0.0, 0.0]]);
        assert_eq!(decode(&logits, steps, classes, &characters), (String::new(), 0.0));
    }

    #[test]
    fn a_line_is_scaled_to_the_rows_the_model_was_trained_on() {
        let line = Gray::new(200, 25, vec![128; 5000]).expect("a line");
        let (values, width) = tensor(&line);
        assert_eq!(width, 384, "200/25 = 8, and 8 x 48 rows");
        assert_eq!(values.len(), 3 * (width as usize) * (ROWS as usize));
        // 128 is the middle of the range, so it is the middle of [-1, 1].
        assert!(values.iter().all(|v| v.abs() < 0.01), "{:?}", &values[..4]);
    }

    #[test]
    fn a_crop_too_thin_to_hold_a_glyph_is_still_given_a_width() {
        let hair = Gray::new(1, 40, vec![0; 40]).expect("a hair");
        assert_eq!(tensor(&hair).1, NARROWEST);
    }
}
