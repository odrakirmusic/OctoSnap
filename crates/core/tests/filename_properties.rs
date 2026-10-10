// SPDX-License-Identifier: GPL-3.0-or-later

//! Properties of file names made from a template (ENG-03, D173).
//!
//! A template is the user's, and `{window}` and `{app}` are whatever the captured window
//! calls itself, so both are drawn from any text at all. What has to hold is that the name
//! is one a file can have, here and on whatever the file is copied to: never empty, no
//! separator, no control character, not hidden, and short enough for Linux's 255 bytes
//! once `unique_path` has added what it adds (D174).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use chrono::{TimeZone, Utc};
use octosnap_core::filename::{CaptureType, Context, NAME_BYTES, render, sanitize};
use octosnap_core::savepath::unique_path;
use proptest::prelude::*;

const ILLEGAL: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

fn legal(name: &str) -> Result<(), String> {
    if let Some(c) = name.chars().find(|c| ILLEGAL.contains(c) || c.is_control()) {
        return Err(format!("{name:?} has {c:?}"));
    }
    if name.starts_with('.') {
        return Err(format!("{name:?} is hidden"));
    }
    if name != name.trim() {
        return Err(format!("{name:?} has space at an end"));
    }
    if name.contains("  ") {
        return Err(format!("{name:?} has a double space"));
    }
    Ok(())
}

/// Templates as people write them, tokens and all, and any text besides.
fn template() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        prop::sample::select(vec![
            "{yyyy}", "{yy}", "{MM}", "{MMM}", "{MMMM}", "{dd}", "{ddd}", "{HH}", "{hh}", "{mm}",
            "{ss}", "{a}", "{n}", "{app}", "{window}", "{type}", "{w}", "{h}", "{", "}", "{nope}",
            " ", ".", "/", "..",
        ])
        .prop_map(str::to_owned),
        ".{0,8}",
    ];
    prop::collection::vec(piece, 0..12).prop_map(|pieces| pieces.concat())
}

fn context() -> impl Strategy<Value = Context> {
    (
        0..4_102_444_800i64,
        any::<bool>(),
        any::<u32>(),
        0..8u8,
        ".{0,300}",
        ".{0,600}",
        prop::sample::select(vec![
            CaptureType::Screenshot,
            CaptureType::Recording,
            CaptureType::Scrolling,
        ]),
        any::<u32>(),
        any::<u32>(),
        any::<bool>(),
    )
        .prop_map(
            |(
                seconds,
                use_utc,
                counter,
                counter_width,
                app,
                window,
                capture_type,
                w,
                h,
                retina,
            )| {
                Context {
                    timestamp: Utc.timestamp_opt(seconds, 0).unwrap(),
                    use_utc,
                    counter,
                    counter_width,
                    app,
                    window,
                    capture_type,
                    width: w,
                    height: h,
                    retina_suffix: retina,
                }
            },
        )
}

/// Found while writing the properties: dots and spaces in turn at the start outlasted the
/// one pass that took the dots off, and left a hidden file (D174).
#[test]
fn dots_and_spaces_at_the_start_never_leave_a_hidden_name() {
    for text in [". . a", " . . . a", ".\t.a", ". .", "..  .. a"] {
        let name = sanitize(text);
        assert!(!name.starts_with('.'), "{text:?} gave {name:?}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_000))]

    #[test]
    fn sanitizing_leaves_a_legal_name_and_is_done_once(text in ".*") {
        let once = sanitize(&text);
        if let Err(why) = legal(&once) {
            prop_assert!(false, "{}", why);
        }
        prop_assert_eq!(sanitize(&once), once);
    }

    #[test]
    fn any_template_gives_a_name_a_file_can_have(t in template(), c in context()) {
        let name = render(&t, &c);
        prop_assert!(!name.is_empty());
        prop_assert!(name.len() <= NAME_BYTES, "{} bytes: {:?}", name.len(), name);
        if let Err(why) = legal(&name) {
            prop_assert!(false, "{}", why);
        }
        prop_assert_eq!(render(&t, &c), name, "the same capture gets the same name");
    }

    /// The name fits once `unique_path` has added the most it adds: a space, an id and the
    /// longest extension.
    #[test]
    fn the_saved_file_name_fits_in_255_bytes(t in template(), c in context()) {
        let stem = render(&t, &c);
        let path = unique_path(Path::new("/shots"), &stem, "webp", |_| true);
        let file = path.file_name().unwrap().to_str().unwrap();
        prop_assert!(file.len() <= 255, "{} bytes: {:?}", file.len(), file);
    }
}
