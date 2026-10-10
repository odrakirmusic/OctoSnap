// SPDX-License-Identifier: GPL-3.0-or-later

//! `SYS-12` (D179): every key OctoSnap answers, in one dialog, opened with Ctrl+? from any
//! of its windows.
//!
//! Each window's keys come from the list it binds them from, where it has one: the editor's
//! bindings and its tools' letters, the cards' and the pins' shortcut lists, and the global
//! shortcuts as Settings has them now, so a key changed there shows changed here. The
//! history strip, the GIF editor and the text window take their keys in a `match`, and
//! each keeps the list for this dialog beside it. The selection is the extension's, and
//! its list is here.

use adw::prelude::*;
use tracing::info;

/// The key that opens the dialog, in every window of the app.
pub const ACCEL: &str = "<Control>question";

/// Rows of keys (in GTK's accelerator syntax, several separated by spaces) and what they do.
pub type Rows = Vec<(String, String)>;

/// A section of the dialog: its title, and its rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: &'static str,
    pub rows: Rows,
}

/// `spec/03` §5.1's keys over a selection, which the extension takes: its overlay's
/// `#onKeyPress` (`extension/src/overlay/engine.ts`) and the hints above it, and `CAP-15`'s
/// modifiers at the confirm (`octosnap_core::actions`).
const SELECTING: &[(&str, &str)] = &[
    ("Return", "Capture"),
    ("<Control>Return", "Capture and Copy"),
    ("<Shift>Return", "Capture Without the Background Preset"),
    ("Tab <Shift>Tab", "Change the Mode"),
    ("Left Right Up Down", "Move the Selection"),
    ("<Shift>Left <Shift>Right <Shift>Up <Shift>Down", "Move It by 10 Pixels"),
    ("<Control>Left <Control>Right <Control>Up <Control>Down", "Resize It"),
    ("space", "Move It While Drawing, Held"),
    ("Escape", "Clear the Selection, Then Close"),
];

/// Every section, in the order a capture meets them.
#[must_use]
pub fn sections() -> Vec<Section> {
    let mut sections = vec![Section {
        title: "General",
        rows: vec![(ACCEL.to_owned(), "Keyboard Shortcuts".to_owned())],
    }];
    let global = global_rows();
    if !global.is_empty() {
        sections.push(Section { title: "Anywhere", rows: global });
    }
    let (editor, tools) = crate::editor::window::shortcut_rows();
    let listed = |rows: &[(&str, &str)]| -> Vec<(String, String)> {
        rows.iter().map(|&(keys, title)| (keys.to_owned(), title.to_owned())).collect()
    };
    sections.extend([
        Section { title: "Selecting an Area", rows: listed(SELECTING) },
        Section { title: "Cards", rows: crate::qao::shortcut_rows() },
        Section { title: "Editor", rows: editor },
        Section { title: "Editor Tools", rows: tools },
        Section { title: "GIF Editor", rows: listed(crate::gif_editor::KEYS) },
        Section { title: "Pinned Screenshots", rows: crate::pin::shortcut_rows() },
        Section { title: "History", rows: listed(crate::history::strip::KEYS) },
        Section { title: "Text Window", rows: listed(crate::ocr::window::KEYS) },
    ]);
    for section in &mut sections {
        section.rows = merged(std::mem::take(&mut section.rows));
    }
    sections
}

/// One row for each thing a section's keys do: the keys that do the same thing (Delete and
/// Backspace, the four arrows) on one row, in the order they first came.
fn merged(rows: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (keys, title) in rows {
        match out.iter_mut().find(|(_, t)| *t == title) {
            Some((joined, _)) => {
                joined.push(' ');
                joined.push_str(&keys);
            }
            None => out.push((keys, title)),
        }
    }
    out
}

/// The global shortcuts as they are set now, in Settings' groups and words. None when the
/// extension, which holds them, is not there; a shortcut with no key is left out.
fn global_rows() -> Vec<(String, String)> {
    let Some(settings) = crate::settings::extension_settings() else { return Vec::new() };
    let Some(schema) = settings.settings_schema() else { return Vec::new() };
    let mut rows = Vec::new();
    for (group, entries) in crate::prefs::SHORTCUT_GROUPS {
        for &(key, label) in *entries {
            if !schema.has_key(key) {
                continue;
            }
            let keys: Vec<String> = settings
                .strv(key)
                .iter()
                .map(ToString::to_string)
                .filter(|k| !k.is_empty())
                .collect();
            if keys.is_empty() {
                continue;
            }
            rows.push((keys.join(" "), global_title(group, label)));
        }
    }
    rows
}

/// "Capture Area and …" with "… Save" is "Capture Area and Save": Settings shows the
/// group's title above its rows, and a row here stands alone.
fn global_title(group: &str, label: &str) -> String {
    match (group.strip_suffix('\u{2026}'), label.strip_prefix('\u{2026}')) {
        (Some(head), Some(tail)) => format!("{}{}", head, tail.trim_start()),
        _ => label.to_owned(),
    }
}

/// Opens the dialog over the window that has the focus, or on its own with none.
///
/// Inside an `AdwApplicationWindow` it is a sheet over that window. A card, a pin and the
/// history strip are plain windows too small to hold it, and over them libadwaita gives it
/// a window of its own.
pub fn present(app: &adw::Application) {
    let dialog = adw::ShortcutsDialog::new();
    let sections = sections();
    let rows: usize = sections.iter().map(|s| s.rows.len()).sum();
    for section in &sections {
        let part = adw::ShortcutsSection::new(Some(section.title));
        for (keys, title) in &section.rows {
            part.add(adw::ShortcutsItem::new(title, keys));
        }
        dialog.add(part);
    }
    let parent = app.active_window();
    // Read by `editor-test.sh`.
    info!(sections = sections.len(), rows, over = parent.is_some(), "shortcuts dialog");
    dialog.present(parent.as_ref());
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn keys_that_do_the_same_thing_share_a_row() {
        let rows = vec![
            ("Delete".to_owned(), "Delete".to_owned()),
            ("<Control>c".to_owned(), "Copy".to_owned()),
            ("BackSpace".to_owned(), "Delete".to_owned()),
        ];
        assert_eq!(
            merged(rows),
            vec![
                ("Delete BackSpace".to_owned(), "Delete".to_owned()),
                ("<Control>c".to_owned(), "Copy".to_owned()),
            ]
        );
    }

    #[test]
    fn a_global_row_carries_its_group() {
        assert_eq!(
            global_title("Capture Area and \u{2026}", "\u{2026} Save"),
            "Capture Area and Save"
        );
        assert_eq!(global_title("Screenshots", "All-In-One"), "All-In-One");
    }

    /// Every section the dialog shows, without the global one (which needs the extension's
    /// schema), has rows, and no row is without keys or words.
    #[test]
    fn every_section_has_keys_and_words() {
        let sections = sections();
        for title in [
            "General",
            "Selecting an Area",
            "Cards",
            "Editor",
            "Editor Tools",
            "GIF Editor",
            "Pinned Screenshots",
            "History",
            "Text Window",
        ] {
            let section = sections.iter().find(|s| s.title == title).expect(title);
            assert!(!section.rows.is_empty(), "{title}");
            for (keys, words) in &section.rows {
                assert!(
                    !keys.trim().is_empty() && !words.trim().is_empty(),
                    "{title}: {keys:?} {words:?}"
                );
            }
        }
    }

    /// No key means two things in one section. The global section is the user's to set,
    /// and Settings refuses a key that is taken.
    #[test]
    fn no_key_is_on_two_rows_of_a_section() {
        for section in sections().into_iter().filter(|s| s.title != "Anywhere") {
            let mut seen = std::collections::HashSet::new();
            for (keys, words) in &section.rows {
                for key in keys.split(' ') {
                    assert!(
                        seen.insert(key.to_owned()),
                        "{}: {key} again, for {words}",
                        section.title
                    );
                }
            }
        }
    }
}
