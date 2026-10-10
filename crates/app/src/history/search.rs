// SPDX-License-Identifier: GPL-3.0-or-later

//! The history's text, and finding captures by it (`HIS-05`, `SYS-13`, D181).
//!
//! What the device read in a capture is kept beside it, as `text.txt` in its directory, so
//! it goes wherever the entry goes: a delete, a restore or the retention janitor removes
//! the directory and the text with it. The strip's search and the Activities search ask
//! [`History::search`], which reads each file once and keeps it.

use std::io::Write as _;
use std::rc::Rc;

use octosnap_core::history::Entry;
use octosnap_core::search::{self, TEXT_FILE};
use tracing::{debug, info, warn};

use super::History;

impl History {
    /// What was read in a capture: `None` while it has not been read, and an empty text
    /// for one that was read and holds none.
    #[must_use]
    pub fn text_of(&self, entry: &Entry) -> Option<Rc<str>> {
        if let Some(text) = self.texts.borrow().get(&entry.id) {
            return Some(Rc::clone(text));
        }
        let file = entry.dir()?.join(TEXT_FILE);
        let text: Rc<str> = std::fs::read_to_string(&file).ok()?.into();
        self.texts.borrow_mut().insert(entry.id.clone(), Rc::clone(&text));
        Some(text)
    }

    /// Keeps what was read in a capture, if it is still in the history. Written beside the
    /// capture through a temporary file, so a search never reads half a text, and never
    /// into a directory that is gone: the capture may have been restored or deleted while
    /// it was being read.
    pub fn set_text(&self, id: &str, text: &str) -> bool {
        let Some(entry) = self.get(id) else { return false };
        let Some(dir) = entry.dir() else { return false };
        if !dir.is_dir() {
            return false;
        }
        let file = dir.join(TEXT_FILE);
        let partial = dir.join(format!("{TEXT_FILE}.partial"));
        let written = std::fs::File::create(&partial)
            .and_then(|mut out| out.write_all(text.as_bytes()))
            .and_then(|()| std::fs::rename(&partial, &file));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&partial);
            debug!(id, "the text read was not kept: {e}");
            return false;
        }
        self.texts.borrow_mut().insert(id.to_owned(), text.into());
        true
    }

    /// Settings turned reading off: every text read so far is deleted.
    pub fn forget_texts(&self) -> usize {
        let mut removed = 0;
        for entry in self.entries.borrow().iter() {
            let Some(dir) = entry.dir() else { continue };
            match std::fs::remove_file(dir.join(TEXT_FILE)) {
                Ok(()) => removed += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => warn!(id = entry.id, "could not delete the text read: {e}"),
            }
        }
        self.texts.borrow_mut().clear();
        info!(removed, "the history's texts deleted");
        removed
    }

    /// The captures every term finds, newest first: in their text, their app or their
    /// window's title (`octosnap_core::search`).
    #[must_use]
    pub fn search(&self, terms: &[String]) -> Vec<Entry> {
        let entries = self.entries();
        entries
            .into_iter()
            .filter(|entry| {
                let text = self.text_of(entry);
                search::matches(terms, entry, text.as_deref().unwrap_or_default())
            })
            .collect()
    }

    /// The entries not read yet, newest first, of the kinds that are a still picture.
    #[must_use]
    pub fn unread(&self) -> Vec<Entry> {
        self.entries()
            .into_iter()
            .filter(|entry| readable(entry) && self.text_of(entry).is_none())
            .collect()
    }
}

/// Whether an entry is a picture the device can read: a recording is not, and a GIF's
/// frames are what its first frame shows, which its title says better.
fn readable(entry: &Entry) -> bool {
    use octosnap_core::history::Kind;
    matches!(
        entry.kind,
        Kind::Screenshot | Kind::Scrolling | Kind::Ocr | Kind::External | Kind::Project
    ) && !octosnap_core::history::is_gif(&entry.path)
}
