// SPDX-License-Identifier: GPL-3.0-or-later

//! Capture history: the store and its janitor (`HIS-01`–`HIS-04`, `spec/07` §4.1,
//! `spec/04` §7).
//!
//! The record is `core::history`; this is the directory it describes and the two jobs
//! that keep it honest. **A directory per capture** under
//! `~/.local/share/octosnap/history/<id>/`, holding the spool's PNG and JSON twin as they
//! were, a thumbnail, and `entry.json` (`docs/decisions.md` D64: a database would be a
//! second serialisation of the same struct that can disagree with the files it indexes).
//!
//! **Filing is a hard link, not a move.** A card that closed because Annotate opened the
//! editor, or because a drag succeeded, closed while something else was still reading its
//! file -- the editor holds the path for the life of its window, and a drop target reads
//! the URI at its own pace. Moving the file out from under either would be a bug the user
//! meets as "the picture vanished". So the history takes a link (a copy where the file
//! system has no links) and the spool copy is removed at once only when nothing can be
//! reading it; the **janitor** removes the rest later, once no window holds the path.
//!
//! The janitor's second job is the recovery `spec/10` §8 asks for: a spool file with a
//! twin and no card -- the app crashed between the capture and the card, or was killed
//! with cards open -- is filed at launch rather than left in the cache for ever. That is
//! also what turns the spool of a build from before this milestone into history.

pub mod strip;
pub mod thumbnail;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use gtk::glib;
use octosnap_core::CaptureResult;
use octosnap_media::reel;
use octosnap_core::history::{self as core, ENTRY_FILE, Entry, Retention, THUMB_FILE};
use tracing::{debug, info, warn};

/// How long a spool file with no card has to be before the janitor treats it as
/// abandoned rather than as a capture on its way to becoming a card.
const ABANDONED_AFTER_US: u64 = 60 * 1_000_000;
/// How long a spool PNG *without a twin* -- an editor's export, which is derived from a
/// capture rather than being one -- is kept once nothing holds it. A day, because a
/// drop target or a pin can still be reading it for as long as a session lasts.
const DERIVED_KEPT_US: u64 = 24 * 3600 * 1_000_000;
/// How often the janitor runs after the one at launch (`spec/07` §4.1: "at launch and
/// hourly").
const JANITOR_INTERVAL_S: u32 = 3600;

/// Where every closed capture goes, and what the strip and "Restore recently closed" read.
pub struct History {
    root: PathBuf,
    spool: PathBuf,
    /// Newest first, as the strip shows them.
    entries: RefCell<Vec<Entry>>,
    retention: Cell<Retention>,
    keep_saved: Cell<bool>,
    listeners: RefCell<Vec<Box<dyn Fn() -> bool>>>,
    /// What [`Self::start_janitor`] was told answers the paths windows still hold, for a
    /// sweep between its hourly runs ([`Self::sweep_now`]).
    open: RefCell<Option<Open>>,
}

/// Answers every path some window still holds.
type Open = Rc<dyn Fn() -> HashSet<PathBuf>>;

impl std::fmt::Debug for History {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("History")
            .field("root", &self.root)
            .field("entries", &self.entries.borrow().len())
            .field("retention", &self.retention.get())
            .finish_non_exhaustive()
    }
}

impl History {
    /// `spec/07` §4.1's `~/.local/share/octosnap/history/`.
    #[must_use]
    pub fn default_root() -> PathBuf {
        glib::user_data_dir().join("octosnap").join("history")
    }

    /// `spec/10` §4's spool, where captures live while they have a card: the extension's
    /// `spool.ts` writes here and a restore puts files back here.
    #[must_use]
    pub fn default_spool() -> PathBuf {
        glib::user_cache_dir().join("octosnap").join("spool")
    }

    /// Opens the store, reading every entry it holds. An entry that cannot be read is
    /// skipped and reported rather than deleted: the files are the user's captures and
    /// the record is the recovery path, so a bad record must not take them with it.
    pub fn open(root: PathBuf, spool: PathBuf, retention: Retention, keep_saved: bool) -> Rc<Self> {
        if let Err(e) = std::fs::create_dir_all(&root) {
            warn!(root = %root.display(), "could not create the history directory: {e}");
        }
        let mut entries = Vec::new();
        let mut unreadable = 0usize;
        if let Ok(dirs) = std::fs::read_dir(&root) {
            for dir in dirs.flatten() {
                let record = dir.path().join(ENTRY_FILE);
                if !record.is_file() {
                    continue;
                }
                match std::fs::read(&record).map_err(|e| e.to_string()).and_then(|bytes| {
                    serde_json::from_slice::<Entry>(&bytes).map_err(|e| e.to_string())
                }) {
                    Ok(entry) => entries.push(entry),
                    Err(e) => {
                        unreadable += 1;
                        warn!(record = %record.display(), "skipping an unreadable history record: {e}");
                    }
                }
            }
        }
        core::sort_newest_first(&mut entries);
        info!(root = %root.display(), entries = entries.len(), unreadable, retention = retention.label(), "history opened");
        Rc::new(Self {
            root,
            spool,
            entries: RefCell::new(entries),
            retention: Cell::new(retention),
            keep_saved: Cell::new(keep_saved),
            listeners: RefCell::new(Vec::new()),
            open: RefCell::new(None),
        })
    }

    /// Every entry, newest first.
    #[must_use]
    pub fn entries(&self) -> Vec<Entry> {
        self.entries.borrow().clone()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.borrow().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.borrow().is_empty()
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<Entry> {
        self.entries.borrow().iter().find(|e| e.id == id).cloned()
    }

    /// The capture an entry stands for, read from its twin, without taking it out --
    /// for Copy and Save as…, which leave the entry where it is.
    #[must_use]
    pub fn capture_of(&self, id: &str) -> Option<CaptureResult> {
        let entry = self.get(id)?;
        let mut capture = read_capture(&entry)?;
        // The twin says where the file was when it was filed -- the spool, which no longer
        // has it. A recording's GIF not yet written is written here, from the frames filed
        // beside it (D113).
        capture.path.clone_from(&entry.path);
        if let Some(meta) = &entry.meta_path {
            capture.meta_path.clone_from(meta);
        }
        Some(capture)
    }

    /// The most recently *filed* entry -- what "Restore recently closed" brings back.
    /// Filing order, not capture order: the card closed last is the one the user just
    /// lost, whatever its capture's age.
    #[must_use]
    pub fn last_filed(&self) -> Option<Entry> {
        self.entries.borrow().iter().max_by_key(|e| (e.filed_at, e.id.clone())).cloned()
    }

    #[must_use]
    pub fn retention(&self) -> Retention {
        self.retention.get()
    }

    /// `spec/04` §7: whether a closing card's capture is filed at all.
    #[must_use]
    pub fn files(&self, saved: bool) -> bool {
        core::files(saved, self.keep_saved.get())
    }

    /// The Preferences dialog changed a history setting. A shorter retention purges now.
    pub fn set_policy(&self, retention: Retention, keep_saved: bool) {
        let changed = self.retention.replace(retention) != retention;
        self.keep_saved.set(keep_saved);
        if changed {
            info!(retention = retention.label(), "history retention changed");
            self.purge(now_us());
        }
    }

    /// Runs `listener` after every change to the entries, for the strip, for as long as it
    /// answers `true`. A strip that has closed answers `false` and is let go; every strip
    /// ever opened used to stay on the list for the life of the service (D139).
    pub fn connect_changed(&self, listener: impl Fn() -> bool + 'static) {
        self.listeners.borrow_mut().push(Box::new(listener));
    }

    fn changed(&self) {
        // Out of the cell while they run, so a listener that connects another does not find
        // the list borrowed. One connected meanwhile goes after the ones kept.
        let listeners = std::mem::take(&mut *self.listeners.borrow_mut());
        let mut kept: Vec<_> = listeners.into_iter().filter(|listener| listener()).collect();
        let mut listeners = self.listeners.borrow_mut();
        kept.append(&mut listeners);
        *listeners = kept;
    }

    /// Files a capture: its files linked into a directory of their own, a record beside
    /// them, a thumbnail on the way.
    ///
    /// `keep_source` leaves the spool copy where it is, for a card that closed because
    /// something else took the file (see the module note); the janitor removes it once
    /// nothing holds it. An entry with the same id -- a capture restored and closed again
    /// -- is replaced.
    pub fn file(
        &self,
        capture: &CaptureResult,
        saved_to: Option<&Path>,
        project: Option<&Path>,
        keep_source: bool,
    ) -> Option<Entry> {
        let id = core::id_of(capture)?;
        let dir = core::entry_dir(&self.root, &id);
        if dir.exists() {
            debug!(id, "replacing an earlier history entry for the same capture");
            let _ = std::fs::remove_dir_all(&dir);
        }
        if let Err(e) = std::fs::create_dir_all(&dir) {
            warn!(dir = %dir.display(), "could not create the history entry: {e}");
            return None;
        }
        let entry = Entry::filed(capture, &dir, now_us(), saved_to, project);
        // A recording is filed as its GIF once one has been written, and as its frames until
        // then (D113): a second's frames at full size are megabytes, and the history keeps
        // a week of them.
        let linked = if capture.path.exists() {
            link_or_copy(&capture.path, &entry.path)
        } else {
            link_reel(&capture.path, &entry.path).and_then(|reel| {
                if reel { Ok(()) } else { link_or_copy(&capture.path, &entry.path) }
            })
        };
        if let Err(e) = linked {
            warn!(from = %capture.path.display(), "could not file the capture: {e}");
            let _ = std::fs::remove_dir_all(&dir);
            return None;
        }
        if let Some(meta) = &entry.meta_path {
            if capture.meta_path.is_file() {
                if let Err(e) = link_or_copy(&capture.meta_path, meta) {
                    warn!(from = %capture.meta_path.display(), "could not file the capture's twin: {e}");
                }
            } else {
                // A capture without its twin -- an old spool, or a file the user opened.
                // Written from what the app holds, so the restore has something to read.
                write_json(meta, capture);
            }
        }
        write_json(&dir.join(ENTRY_FILE), &entry);
        if !keep_source {
            remove_spool_copy(capture);
        }
        thumbnail::spawn(entry.path.clone(), dir.join(THUMB_FILE));

        {
            let mut entries = self.entries.borrow_mut();
            entries.retain(|e| e.id != id);
            entries.push(entry.clone());
            core::sort_newest_first(&mut entries);
        }
        info!(
            id,
            kind = entry.kind.label(),
            saved = saved_to.is_some(),
            kept_source = keep_source,
            entries = self.len(),
            "filed in history"
        );
        self.changed();
        Some(entry)
    }

    /// Takes an entry out of the history and puts its files back in the spool, as the
    /// capture they were: `spec/04` §7's "history/<id>/ back to a card". The saved copy's
    /// path comes with it, so the card offers Trash rather than a second Save (D47).
    pub fn take(&self, id: &str) -> Option<(CaptureResult, Option<PathBuf>)> {
        let entry = self.get(id)?;
        let mut capture = read_capture(&entry)?;
        if let Err(e) = std::fs::create_dir_all(&self.spool) {
            warn!(spool = %self.spool.display(), "could not create the spool: {e}");
            return None;
        }
        let back = |file: &Path| -> PathBuf {
            file.file_name().map_or_else(|| self.spool.join(format!("{id}.png")), |name| self.spool.join(name))
        };
        let png = back(&entry.path);
        let restored = link_reel(&entry.path, &png).and_then(|reel| {
            if reel && !entry.path.exists() { Ok(()) } else { link_or_copy(&entry.path, &png) }
        });
        if let Err(e) = restored {
            warn!(from = %entry.path.display(), "could not restore the capture: {e}");
            return None;
        }
        capture.path = png;
        capture.meta_path = match &entry.meta_path {
            Some(meta) if meta.is_file() => {
                let twin = back(meta);
                if let Err(e) = link_or_copy(meta, &twin) {
                    warn!(from = %meta.display(), "could not restore the twin: {e}");
                }
                twin
            }
            _ => capture.path.with_extension("json"),
        };
        // The twin in the spool says where the file is *now*.
        write_json(&capture.meta_path, &capture);
        // The entry's directory goes with the entry: the files are in the spool again,
        // and a capture is in exactly one place (D47).
        if let Some(dir) = entry.dir()
            && let Err(e) = remove_entry_dir(dir)
        {
            warn!(dir = %dir.display(), "could not remove the restored entry's directory: {e}");
        }
        self.forget(id);
        info!(id, "restored from history");
        Some((capture, entry.saved_path))
    }

    /// Deletes an entry and its files (`HIS-01`'s Delete).
    pub fn remove(&self, id: &str) -> bool {
        let Some(entry) = self.get(id) else { return false };
        if let Some(dir) = entry.dir()
            && let Err(e) = remove_entry_dir(dir)
        {
            warn!(dir = %dir.display(), "could not delete the history entry: {e}");
        }
        self.forget(id);
        info!(id, "deleted from history");
        true
    }

    /// `HIS-01`'s Clear history.
    pub fn clear(&self) -> usize {
        let ids: Vec<String> = self.entries.borrow().iter().map(|e| e.id.clone()).collect();
        let mut removed = 0;
        for id in ids {
            if self.remove(&id) {
                removed += 1;
            }
        }
        info!(removed, "history cleared");
        removed
    }

    /// The retention janitor: removes what has been here longer than the setting allows.
    pub fn purge(&self, now_us: u64) -> usize {
        let expired: Vec<String> = core::expired(&self.entries.borrow(), now_us, self.retention.get())
            .into_iter()
            .map(|e| e.id.clone())
            .collect();
        let count = expired.len();
        for id in expired {
            self.remove(&id);
        }
        if count > 0 {
            info!(count, retention = self.retention.get().label(), "history purged");
        }
        count
    }

    /// The spool janitor. `open` is every path some window still holds -- a card, a pin,
    /// an editor. A spool file already in the history loses its spool copy; one with a
    /// twin and no window, older than a minute, is a capture nothing will ever show and
    /// is filed; one with no twin -- an editor's `-annotated.png` export, derived from a
    /// capture -- is removed once it is a day old and nothing holds it, because it is not
    /// a capture and nothing else will ever clear it. Answers `(filed, removed)`.
    ///
    /// A recording's reel is looked after as the GIF it will be written to, `<id>.gif`,
    /// is: through that GIF's file once there is one, and through its own directory until
    /// then (D113).
    pub fn sweep_spool(&self, open: &HashSet<PathBuf>, now_us: u64) -> (usize, usize) {
        let Ok(files) = std::fs::read_dir(&self.spool) else { return (0, 0) };
        let (mut filed, mut removed) = (0, 0);
        for file in files.flatten() {
            let found = file.path();
            let is_reel = found.extension().is_some_and(|ext| ext == reel::EXTENSION)
                && reel::is_reel(&found);
            // A still or a GIF (M5), or a GIF that is still frames; a twin beside either
            // makes it a capture.
            let path = if is_reel { found.with_extension("gif") } else { found.clone() };
            let is_capture =
                is_reel || path.extension().is_some_and(|ext| ext == "png" || ext == "gif");
            if !is_capture || open.contains(&path) || (is_reel && path.exists()) {
                continue;
            }
            let Some(id) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
            let twin = path.with_extension("json");
            if let Some(entry) = self.get(&id) {
                // Filed while a window still held it; the window is gone. A GIF written
                // since from the frames it was filed as takes their place (D113).
                if path.is_file() && !entry.path.exists() {
                    match link_or_copy(&path, &entry.path) {
                        Ok(()) => {
                            reel::remove(&reel::beside(&entry.path));
                            debug!(id, "the history keeps the GIF written since, not its frames");
                        }
                        Err(e) => warn!(id, "could not file the GIF written since: {e}"),
                    }
                }
                let _ = std::fs::remove_file(&path);
                let _ = std::fs::remove_file(&twin);
                reel::remove(&reel::beside(&path));
                removed += 1;
                debug!(id, "spool copy removed; the history has it");
                continue;
            }
            let age = file
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(u64::MAX, |d| now_us.saturating_sub(u64::try_from(d.as_micros()).unwrap_or(u64::MAX)));
            if !twin.is_file() {
                if age >= DERIVED_KEPT_US {
                    let _ = std::fs::remove_file(&path);
                    reel::remove(&reel::beside(&path));
                    removed += 1;
                    debug!(id, "a derived spool file nobody holds was removed");
                }
                continue;
            }
            if age < ABANDONED_AFTER_US {
                continue;
            }
            let Some(capture) = read_twin(&twin) else { continue };
            if self.file(&capture, None, None, false).is_some() {
                filed += 1;
                info!(id, "a spool file with no card was filed in history");
            }
        }
        (filed, removed)
    }

    /// Runs the janitor now and every hour. `open` answers which paths windows still hold.
    pub fn start_janitor(self: &Rc<Self>, open: impl Fn() -> HashSet<PathBuf> + 'static) {
        let open: Open = Rc::new(open);
        *self.open.borrow_mut() = Some(Rc::clone(&open));
        let run = {
            let history: Weak<Self> = Rc::downgrade(self);
            let open = Rc::clone(&open);
            move || {
                let Some(history) = history.upgrade() else { return glib::ControlFlow::Break };
                let purged = history.purge(now_us());
                let (filed, removed) = history.sweep_spool(&open(), now_us());
                debug!(purged, filed, removed, "janitor ran");
                glib::ControlFlow::Continue
            }
        };
        // Once soon, off the startup path, then hourly.
        {
            let run = run.clone();
            glib::idle_add_local_once(move || {
                let _ = run();
            });
        }
        glib::timeout_add_seconds_local(JANITOR_INTERVAL_S, run);
    }

    /// The spool janitor now, not at its next hourly run: a recording's GIF has just been
    /// written from the frames the history filed it as, and the GIF takes their place
    /// (D113). Nothing before [`Self::start_janitor`] has said what windows hold.
    pub fn sweep_now(&self) {
        let Some(open) = self.open.borrow().clone() else { return };
        let (filed, removed) = self.sweep_spool(&open(), now_us());
        debug!(filed, removed, "spool swept after a recording's GIF was written");
    }

    fn forget(&self, id: &str) {
        self.entries.borrow_mut().retain(|e| e.id != id);
        self.changed();
    }
}

/// Microseconds since the epoch, the clock `CaptureResult::timestamp` uses.
fn now_us() -> u64 {
    glib::real_time().unsigned_abs()
}

/// An entry's directory, gone. Twice if need be: the thumbnail filing started is written
/// into it on a worker, and a restore or a delete that comes quickly enough can find that
/// file arriving between emptying the directory and removing it.
fn remove_entry_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::remove_dir_all(dir).or_else(|e| match e.kind() {
        std::io::ErrorKind::DirectoryNotEmpty => std::fs::remove_dir_all(dir),
        _ => Err(e),
    })
}

/// The reel beside the capture at `from`, linked beside `to` frame by frame, as the capture's
/// own file is (D113). `Ok(false)`, and nothing done, when `from` has no reel.
fn link_reel(from: &Path, to: &Path) -> std::io::Result<bool> {
    let (from, to) = (reel::beside(from), reel::beside(to));
    if !reel::is_reel(&from) {
        return Ok(false);
    }
    if to.exists() {
        std::fs::remove_dir_all(&to)?;
    }
    std::fs::create_dir_all(&to)?;
    for file in std::fs::read_dir(&from)? {
        let file = file?;
        link_or_copy(&file.path(), &to.join(file.file_name()))?;
    }
    Ok(true)
}

/// A hard link, or a copy where the file system refuses one (another mount, a FAT
/// drive). Either way the source is untouched.
fn link_or_copy(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        std::fs::remove_file(to)?;
    }
    match std::fs::hard_link(from, to) {
        Ok(()) => Ok(()),
        Err(_) => std::fs::copy(from, to).map(|_| ()),
    }
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) {
    match serde_json::to_vec_pretty(value) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(path, bytes) {
                warn!(path = %path.display(), "could not write: {e}");
            }
        }
        Err(e) => warn!(path = %path.display(), "could not serialise: {e}"),
    }
}

fn read_twin(path: &Path) -> Option<CaptureResult> {
    match std::fs::read(path).map_err(|e| e.to_string()).and_then(|bytes| {
        serde_json::from_slice::<CaptureResult>(&bytes).map_err(|e| e.to_string())
    }) {
        Ok(capture) => Some(capture),
        Err(e) => {
            warn!(path = %path.display(), "an unreadable capture twin: {e}");
            None
        }
    }
}

/// The capture an entry stands for: its twin, or -- for an entry whose twin is gone --
/// enough of one to show a card, built from the record.
fn read_capture(entry: &Entry) -> Option<CaptureResult> {
    if let Some(capture) = entry.meta_path.as_deref().filter(|p| p.is_file()).and_then(read_twin) {
        return Some(capture);
    }
    if !entry.path.is_file() && !reel::is_reel(&reel::beside(&entry.path)) {
        warn!(id = entry.id, "the history entry's file is gone");
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let logical = |physical: u32| (f64::from(physical) / entry.scale.max(0.01)).round() as i32;
    Some(CaptureResult {
        path: entry.path.clone(),
        meta_path: entry.path.with_extension("json"),
        mode: octosnap_core::CaptureMode::Area,
        rect: octosnap_core::Rect::new(0, 0, logical(entry.width), logical(entry.height)),
        scale: entry.scale,
        display: entry.display.clone(),
        cursor_rect: None,
        source_window: entry.source.clone(),
        window_alpha: false,
        timestamp: entry.created_at,
        confirmed_at: None,
        animation_ms: 0,
        requested_action: None,
        modifiers: 0,
        external: entry.kind == core::Kind::External,
        linebreaks: None,
        project: entry.project_path.clone(),
        // A recording's length is not in the entry's summary; the twin the card reads
        // carries it. History rebuilds enough to reopen, and the card reads the file.
        duration_ms: None,
    })
}

/// Deletes the spool's copy of a filed capture: the PNG and its twin.
fn remove_spool_copy(capture: &CaptureResult) {
    for path in [&capture.path, &capture.meta_path] {
        match std::fs::remove_file(path) {
            Ok(()) => debug!(path = %path.display(), "spool copy removed"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!(path = %path.display(), "could not remove the spool copy: {e}"),
        }
    }
    reel::remove(&reel::beside(&capture.path));
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use octosnap_core::{CaptureMode, Rect};

    fn capture(spool: &Path, id: &str) -> CaptureResult {
        let png = spool.join(format!("{id}.png"));
        std::fs::write(&png, b"not really a png").expect("spool file");
        let capture = CaptureResult {
            path: png.clone(),
            meta_path: png.with_extension("json"),
            mode: CaptureMode::Area,
            rect: Rect::new(0, 0, 40, 30),
            scale: 2.0,
            display: "eDP-1".to_owned(),
            cursor_rect: None,
            source_window: octosnap_core::SourceWindow::default(),
            window_alpha: false,
            timestamp: 5,
            confirmed_at: None,
            animation_ms: 0,
            requested_action: None,
            modifiers: 0,
            external: false,
            linebreaks: None,
            project: None,
            duration_ms: None,
        };
        write_json(&capture.meta_path, &capture);
        capture
    }

    fn store(temp: &tempfile::TempDir) -> (Rc<History>, PathBuf) {
        let spool = temp.path().join("spool");
        std::fs::create_dir_all(&spool).expect("spool dir");
        let history = History::open(temp.path().join("history"), spool.clone(), Retention::Week, true);
        (history, spool)
    }

    #[test]
    fn filing_links_the_files_into_a_directory_and_removes_the_spool_copy() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (history, spool) = store(&temp);
        let capture = capture(&spool, "01A");
        let entry = history.file(&capture, None, None, false).expect("filed");
        assert_eq!(entry.id, "01A");
        assert!(entry.path.is_file(), "{}", entry.path.display());
        assert!(entry.meta_path.as_deref().is_some_and(Path::is_file));
        assert!(entry.dir().unwrap().join(ENTRY_FILE).is_file());
        assert!(!capture.path.exists(), "the spool copy is gone");
        assert!(!capture.meta_path.exists());
        assert_eq!(history.len(), 1);
        assert_eq!((entry.width, entry.height), (80, 60));
    }

    #[test]
    fn keep_source_leaves_the_spool_copy_for_the_janitor() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (history, spool) = store(&temp);
        let capture = capture(&spool, "01B");
        history.file(&capture, None, None, true).expect("filed");
        assert!(capture.path.is_file(), "still in the spool");
        // The janitor sees the file, sees the entry, and removes the copy -- unless a
        // window still holds the path.
        let held: HashSet<PathBuf> = [capture.path.clone()].into_iter().collect();
        assert_eq!(history.sweep_spool(&held, 0), (0, 0));
        assert!(capture.path.is_file());
        assert_eq!(history.sweep_spool(&HashSet::new(), 0), (0, 1));
        assert!(!capture.path.exists());
        assert!(!capture.meta_path.exists());
    }

    #[test]
    fn a_restore_puts_the_files_back_and_forgets_the_entry() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (history, spool) = store(&temp);
        let capture = capture(&spool, "01C");
        let saved = temp.path().join("saved.png");
        history.file(&capture, Some(&saved), None, false).expect("filed");
        let (back, saved_to) = history.take("01C").expect("restored");
        assert_eq!(back.path, capture.path);
        assert!(back.path.is_file());
        assert!(back.meta_path.is_file());
        assert_eq!(back.rect, capture.rect);
        assert_eq!(saved_to.as_deref(), Some(saved.as_path()));
        assert!(history.is_empty());
        assert!(!temp.path().join("history/01C").exists(), "the directory is gone");
        assert!(history.take("01C").is_none());
    }

    /// D167: a render of a project is filed under Projects, and the project comes back
    /// out with it, so the restored card's Annotate still opens the project and its next
    /// close files it under Projects again.
    #[test]
    fn a_render_of_a_project_files_as_one_and_keeps_it_through_a_restore() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (history, spool) = store(&temp);
        let project = temp.path().join("work.octosnap");
        let mut render = capture(&spool, "01P");
        render.project = Some(project.clone());
        write_json(&render.meta_path, &render);
        let entry = history.file(&render, None, None, false).expect("filed");
        assert_eq!(entry.kind, core::Kind::Project);
        assert_eq!(entry.project_path.as_deref(), Some(project.as_path()));
        assert!(core::Filter::Projects.matches(entry.kind));
        let (back, _) = history.take("01P").expect("restored");
        assert_eq!(back.project.as_deref(), Some(project.as_path()));
        let again = history.file(&back, None, None, false).expect("filed again");
        assert_eq!(again.kind, core::Kind::Project);
    }

    #[test]
    fn the_store_reopens_from_disk_newest_first_and_skips_a_bad_record() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (history, spool) = store(&temp);
        let mut older = capture(&spool, "01D");
        older.timestamp = 1;
        let mut newer = capture(&spool, "01E");
        newer.timestamp = 9;
        history.file(&older, None, None, false).expect("filed");
        history.file(&newer, None, None, false).expect("filed");
        std::fs::create_dir_all(temp.path().join("history/01Z")).expect("dir");
        std::fs::write(temp.path().join("history/01Z").join(ENTRY_FILE), b"{ not json").expect("bad record");
        drop(history);

        let reopened = History::open(temp.path().join("history"), spool, Retention::Week, true);
        let ids: Vec<String> = reopened.entries().into_iter().map(|e| e.id).collect();
        assert_eq!(ids, vec!["01E", "01D"]);
        assert!(temp.path().join("history/01Z").exists(), "a bad record is kept for a human");
    }

    #[test]
    fn purge_removes_what_retention_says_and_a_shorter_setting_purges_at_once() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (history, spool) = store(&temp);
        let capture = capture(&spool, "01F");
        let entry = history.file(&capture, None, None, false).expect("filed");
        assert_eq!(history.purge(entry.filed_at + Retention::Day.micros() - 1), 0);
        assert_eq!(history.len(), 1);
        assert_eq!(history.purge(entry.filed_at + Retention::Week.micros()), 1);
        assert!(history.is_empty());
        assert!(!temp.path().join("history/01F").exists());
    }

    #[test]
    fn the_spool_janitor_files_an_abandoned_capture_and_leaves_a_fresh_one() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (history, spool) = store(&temp);
        let abandoned = capture(&spool, "01G");
        let fresh = capture(&spool, "01H");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_micros()).unwrap_or(0))
            .unwrap_or(0);
        // Both files were written just now; only with the clock moved on is one abandoned.
        assert_eq!(history.sweep_spool(&HashSet::new(), now), (0, 0));
        let later = now + 2 * ABANDONED_AFTER_US;
        let held: HashSet<PathBuf> = [fresh.path.clone()].into_iter().collect();
        assert_eq!(history.sweep_spool(&held, later), (1, 0));
        assert!(history.get("01G").is_some());
        assert!(!abandoned.path.exists());
        assert!(fresh.path.is_file(), "a held file is never touched");
    }

    #[test]
    fn the_janitor_files_an_abandoned_gif_and_clears_a_twinless_trim_like_a_png() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (history, spool) = store(&temp);
        // A recording that fell through a crash: a GIF with its twin and no card (M5).
        let mut recording = capture(&spool, "01R");
        let gif = spool.join("01R.gif");
        std::fs::rename(&recording.path, &gif).expect("rename to gif");
        recording.path = gif.clone();
        recording.mode = CaptureMode::Record;
        recording.duration_ms = Some(3_000);
        write_json(&recording.meta_path, &recording);
        // And the GIF editor's export beside it, derived and twinless.
        let trim = spool.join("01R-trim.gif");
        std::fs::write(&trim, b"derived").expect("trim");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_micros()).unwrap_or(0))
            .unwrap_or(0);
        assert_eq!(history.sweep_spool(&HashSet::new(), now), (0, 0));
        assert_eq!(history.sweep_spool(&HashSet::new(), now + 2 * ABANDONED_AFTER_US), (1, 0));
        assert!(history.get("01R").is_some(), "the abandoned recording was filed");
        assert!(!gif.exists());
        assert!(trim.is_file(), "a trim is kept for a day like an annotated export");
        assert_eq!(history.sweep_spool(&HashSet::new(), now + DERIVED_KEPT_US + 1), (0, 1));
        assert!(!trim.exists());
        assert!(history.get("01R-trim").is_none(), "a trim is never filed as a capture");
    }

    /// A recording as it stops (D113): its twin says `<id>.gif`, and the GIF is still the
    /// frames in `<id>.reel` beside it.
    fn recording(spool: &Path, id: &str) -> CaptureResult {
        let mut recording = capture(spool, id);
        std::fs::remove_file(&recording.path).expect("no PNG");
        recording.path = spool.join(format!("{id}.gif"));
        recording.mode = CaptureMode::Record;
        recording.duration_ms = Some(300);
        write_json(&recording.meta_path, &recording);
        crate::recording::render::tests::write_reel(&reel::beside(&recording.path), 3);
        recording
    }

    fn frames_in(dir: &Path) -> usize {
        std::fs::read_dir(dir).map_or(0, |files| {
            files.flatten().filter(|f| f.path().extension().is_some_and(|ext| ext == "png")).count()
        })
    }

    #[test]
    fn a_recording_still_in_frames_is_filed_and_restored_as_its_frames() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (history, spool) = store(&temp);
        let recording = recording(&spool, "01G");
        let entry = history.file(&recording, None, None, false).expect("filed");
        assert!(!entry.path.exists(), "no GIF was written to file it");
        assert_eq!(frames_in(&reel::beside(&entry.path)), 3);
        assert!(!reel::beside(&recording.path).exists(), "the spool's frames went with it");
        assert!(!recording.meta_path.exists());
        // The strip's Copy and Save as… read it where it is now, not where it was.
        let filed = history.capture_of("01G").expect("readable");
        assert_eq!(filed.path, entry.path);
        assert_eq!(filed.mode, CaptureMode::Record);

        let (back, _) = history.take("01G").expect("restored");
        assert_eq!(back.path, recording.path);
        assert!(!back.path.exists());
        assert!(reel::is_reel(&reel::beside(&back.path)));
        assert_eq!(frames_in(&reel::beside(&back.path)), 3);
        assert!(!temp.path().join("history/01G").exists(), "the directory is gone");

        // Written since: the GIF is filed, and the frames it was written from are not.
        std::fs::write(&back.path, b"GIF89a").expect("a GIF");
        let entry = history.file(&back, None, None, false).expect("filed again");
        assert!(entry.path.is_file());
        assert!(!reel::beside(&entry.path).exists(), "a week of frames is not kept for a GIF");
        assert!(!back.path.exists() && !reel::beside(&back.path).exists());
    }

    #[test]
    fn the_janitor_looks_after_a_reel_as_the_gif_it_will_be_written_to() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (history, spool) = store(&temp);
        let now = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| u64::try_from(d.as_micros()).unwrap_or(0))
                .unwrap_or(0)
        };

        // Filed while a render still reads its frames (a card closed untouched, D113):
        // left while the GIF is held; once it is not, the GIF the render wrote takes the
        // frames' place in the history, and the spool copy goes.
        let rendered = recording(&spool, "01H");
        let entry = history.file(&rendered, None, None, true).expect("filed");
        let frames = reel::beside(&rendered.path);
        let held: HashSet<PathBuf> = [rendered.path.clone()].into_iter().collect();
        assert_eq!(history.sweep_spool(&held, now()), (0, 0));
        assert!(reel::is_reel(&frames), "a render still reads them");
        std::fs::write(&rendered.path, b"GIF89a").expect("the render's GIF");
        assert_eq!(history.sweep_spool(&HashSet::new(), now()), (0, 1));
        assert!(entry.path.is_file(), "the history has the GIF");
        assert!(!reel::beside(&entry.path).exists(), "and not the frames");
        assert!(!frames.exists() && !rendered.path.exists());
        assert!(!rendered.meta_path.exists());

        // A recording nothing ever showed -- a crash between Stop and its card: filed,
        // frames and all, once it has been abandoned.
        let abandoned = recording(&spool, "01I");
        assert_eq!(history.sweep_spool(&HashSet::new(), now()), (0, 0));
        assert_eq!(history.sweep_spool(&HashSet::new(), now() + 2 * ABANDONED_AFTER_US), (1, 0));
        let entry = history.get("01I").expect("filed");
        assert_eq!(frames_in(&reel::beside(&entry.path)), 3);
        assert!(!reel::beside(&abandoned.path).exists());

        // Frames with no twin to say what they are: a day, like any derived file.
        let stray = spool.join("01J.reel");
        crate::recording::render::tests::write_reel(&stray, 1);
        assert_eq!(history.sweep_spool(&HashSet::new(), now()), (0, 0));
        assert!(reel::is_reel(&stray));
        assert_eq!(history.sweep_spool(&HashSet::new(), now() + 2 * DERIVED_KEPT_US), (0, 1));
        assert!(!stray.exists());
        assert!(history.get("01J").is_none(), "stray frames are never filed as a capture");
    }

    #[test]
    fn a_twinless_export_is_removed_only_after_a_day_and_only_when_nothing_holds_it() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (history, spool) = store(&temp);
        let export = spool.join("01K-annotated.png");
        std::fs::write(&export, b"derived").expect("export");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_micros()).unwrap_or(0))
            .unwrap_or(0);
        // Fresh: kept. A day old but held by a pin: kept. A day old and free: gone.
        assert_eq!(history.sweep_spool(&HashSet::new(), now), (0, 0));
        assert!(export.is_file());
        let later = now + DERIVED_KEPT_US + 1;
        let held: HashSet<PathBuf> = [export.clone()].into_iter().collect();
        assert_eq!(history.sweep_spool(&held, later), (0, 0));
        assert!(export.is_file());
        assert_eq!(history.sweep_spool(&HashSet::new(), later), (0, 1));
        assert!(!export.exists());
        assert!(history.is_empty(), "a derived file is never filed as a capture");
    }

    #[test]
    fn keep_saved_off_means_a_saved_capture_is_not_filed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (history, _spool) = store(&temp);
        assert!(history.files(true));
        history.set_policy(Retention::Week, false);
        assert!(!history.files(true));
        assert!(history.files(false));
    }

    #[test]
    fn last_filed_is_the_card_that_closed_last_not_the_newest_capture() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (history, spool) = store(&temp);
        let mut newest_capture = capture(&spool, "01J");
        newest_capture.timestamp = 100;
        let mut oldest_capture = capture(&spool, "01I");
        oldest_capture.timestamp = 1;
        history.file(&newest_capture, None, None, false).expect("filed");
        std::thread::sleep(std::time::Duration::from_millis(2));
        history.file(&oldest_capture, None, None, false).expect("filed");
        assert_eq!(history.last_filed().map(|e| e.id), Some("01I".to_owned()));
        assert_eq!(history.entries()[0].id, "01J", "the strip still sorts by capture time");
    }
}
