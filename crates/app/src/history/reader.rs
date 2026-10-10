// SPDX-License-Identifier: GPL-3.0-or-later

//! `HIS-05` (D181): every capture in the history is read on the device, so the history and
//! the Activities search find it by what it says.
//!
//! One capture at a time, on the text capture's own worker, and at most one every few
//! seconds. A capture filed while the app runs is read a few seconds after it is filed:
//! one read is about half a second of one core, and a search for the screenshot just taken
//! should find it. The ones from before -- the history a first run finds, or what was filed
//! while reading was off -- wait until GNOME says the session is idle
//! (`org.gnome.SessionManager.Presence`, whose status the screen's dimming follows), and
//! not in power-saver mode. With no session manager to ask, they wait.
//!
//! Nothing is read without a language pack or with Settings' switch off, and a capture
//! that could not be read is not tried again until the next start.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use octosnap_core::history::Entry;
use tracing::{debug, info};

use super::History;

/// How often the reader looks for something to read.
const EVERY_S: u32 = 5;
/// How long after it is filed a capture is read: its card has just closed, and its
/// thumbnail is still being made.
const FRESH_AFTER_US: u64 = 4 * 1_000_000;
/// The largest picture read, in pixels. A scrolling capture of a long page can be tens of
/// megapixels, which is hundreds of megabytes to hold for a read.
const LARGEST: u64 = 40_000_000;
/// `org.gnome.SessionManager.Presence`'s status while the session is idle.
const PRESENCE_IDLE: u32 = 3;

/// What the reader does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Next {
    /// Read this capture now: it was filed while the app runs.
    Now(String),
    /// Read this capture once the session is idle.
    WhenIdle(String),
    Nothing,
}

/// The next capture to read, of those not read yet, newest first. One filed since
/// `started_us`, and long enough ago, is read now; the newest of the rest waits for the
/// session to be idle. `failed` are left out.
pub(crate) fn next(
    unread: &[Entry],
    failed: &HashSet<String>,
    started_us: u64,
    now_us: u64,
) -> Next {
    let mut later = None;
    for entry in unread.iter().filter(|entry| !failed.contains(&entry.id)) {
        if entry.filed_at >= started_us {
            if now_us.saturating_sub(entry.filed_at) >= FRESH_AFTER_US {
                return Next::Now(entry.id.clone());
            }
            continue;
        }
        if later.is_none() {
            later = Some(entry.id.clone());
        }
    }
    later.map_or(Next::Nothing, Next::WhenIdle)
}

/// The reader, for as long as the app runs.
pub struct Reader {
    history: Weak<History>,
    settings: crate::settings::Settings,
    started_us: u64,
    reading: Cell<bool>,
    failed: RefCell<HashSet<String>>,
    presence: RefCell<Option<gio::DBusProxy>>,
    /// Why the last look read nothing, so the log says it once rather than every tick.
    waiting: Cell<&'static str>,
}

impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reader")
            .field("reading", &self.reading.get())
            .field("failed", &self.failed.borrow().len())
            .finish_non_exhaustive()
    }
}

/// Starts reading the history's captures, and keeps on for the life of the app: the timer
/// holds the reader.
pub fn start(history: &Rc<History>, settings: crate::settings::Settings) {
    let reader = Rc::new(Reader {
        history: Rc::downgrade(history),
        settings,
        started_us: glib::real_time().unsigned_abs(),
        reading: Cell::new(false),
        failed: RefCell::new(HashSet::new()),
        presence: RefCell::new(None),
        waiting: Cell::new(""),
    });
    {
        let weak = Rc::downgrade(&reader);
        glib::spawn_future_local(async move {
            let proxy = gio::DBusProxy::for_bus_future(
                gio::BusType::Session,
                gio::DBusProxyFlags::DO_NOT_AUTO_START,
                None,
                "org.gnome.SessionManager",
                "/org/gnome/SessionManager/Presence",
                "org.gnome.SessionManager.Presence",
            )
            .await;
            let Some(reader) = weak.upgrade() else { return };
            match proxy {
                Ok(proxy) if proxy.name_owner().is_some() => {
                    *reader.presence.borrow_mut() = Some(proxy);
                }
                Ok(_) => debug!("no session manager: captures from before wait"),
                Err(e) => debug!("no session manager ({e}): captures from before wait"),
            }
        });
    }
    glib::timeout_add_seconds_local(EVERY_S, move || {
        reader.tick();
        glib::ControlFlow::Continue
    });
}

impl Reader {
    fn tick(self: &Rc<Self>) {
        if self.reading.get() {
            return;
        }
        let Some(history) = self.history.upgrade() else { return };
        if !self.settings.history_read_text() {
            self.wait("reading is off in Settings");
            return;
        }
        if !crate::ocr::ready() {
            self.wait("no language pack");
            return;
        }
        let now = glib::real_time().unsigned_abs();
        let id = match next(&history.unread(), &self.failed.borrow(), self.started_us, now) {
            Next::Nothing => {
                self.wait("everything is read");
                return;
            }
            Next::Now(id) => id,
            Next::WhenIdle(id) => {
                if !self.idle() {
                    self.wait("the session is not idle");
                    return;
                }
                if gio::PowerProfileMonitor::get_default().is_power_saver_enabled() {
                    self.wait("power saver is on");
                    return;
                }
                id
            }
        };
        let Some(entry) = history.get(&id) else { return };
        self.waiting.set("");
        self.read(entry);
    }

    /// Says once why nothing is being read.
    fn wait(&self, why: &'static str) {
        if self.waiting.replace(why) != why {
            debug!(why, "the history reader waits");
        }
    }

    /// Whether GNOME says the session is idle.
    fn idle(&self) -> bool {
        self.presence
            .borrow()
            .as_ref()
            .and_then(|proxy| proxy.cached_property("status"))
            .and_then(|status| status.get::<u32>())
            .is_some_and(|status| status == PRESENCE_IDLE)
    }

    fn read(self: &Rc<Self>, entry: Entry) {
        self.reading.set(true);
        let script = crate::settings::ocr_config().script;
        let started = std::time::Instant::now();
        let id = entry.id.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let path = entry.path.clone();
            let size = u64::from(entry.width) * u64::from(entry.height);
            let pixels = if size > LARGEST {
                Err(format!("{}x{} is more than the reader reads", entry.width, entry.height))
            } else {
                gio::spawn_blocking(move || rgba_of(&path))
                    .await
                    .unwrap_or_else(|_| Err("the decoder stopped".to_owned()))
            };
            let read = match pixels {
                Ok((width, height, rgba)) => crate::ocr::words(width, height, rgba, script).await,
                Err(why) => Err(why),
            };
            let Some(reader) = weak.upgrade() else { return };
            reader.reading.set(false);
            let ms = started.elapsed().as_millis();
            let Some(history) = reader.history.upgrade() else { return };
            match read {
                Ok(words) => {
                    let page = octosnap_ocr::live::Page::new(&words);
                    let text = page
                        .all()
                        .map(|(a, b)| page.text(a, b, octosnap_ocr::Breaks::Keep))
                        .unwrap_or_default();
                    let kept = history.set_text(&id, &text);
                    // Read by `history-test.sh`: how much, never what.
                    info!(id, chars = text.chars().count(), kept, ms, "history read");
                }
                Err(why) => {
                    info!(id, why, ms, "history read failed");
                    reader.failed.borrow_mut().insert(id);
                }
            }
        });
    }
}

/// A capture as straight RGBA rows: a PNG through the decoder the thumbnails use, anything
/// else -- a file opened with the app -- through GDK's loaders.
fn rgba_of(path: &std::path::Path) -> Result<(usize, usize, Vec<u8>), String> {
    if let Ok((rgba, width, height)) = super::thumbnail::decode_png(path) {
        return Ok((width, height, rgba));
    }
    let texture = gdk::Texture::from_filename(path).map_err(|e| e.to_string())?;
    let width = usize::try_from(texture.width()).map_err(|e| e.to_string())?;
    let height = usize::try_from(texture.height()).map_err(|e| e.to_string())?;
    let mut downloader = gdk::TextureDownloader::new(&texture);
    downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
    let (bytes, stride) = downloader.download_bytes();
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let from = y * stride;
        rgba.extend_from_slice(bytes.get(from..from + width * 4).ok_or("short rows")?);
    }
    Ok((width, height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;
    use octosnap_core::capture::SourceWindow;
    use octosnap_core::history::Kind;

    const S: u64 = 1_000_000;

    fn entry(id: &str, filed_at: u64) -> Entry {
        Entry {
            id: id.to_owned(),
            kind: Kind::Screenshot,
            created_at: filed_at,
            filed_at,
            path: format!("/tmp/{id}/{id}.png").into(),
            meta_path: None,
            saved_path: None,
            project_path: None,
            thumb_path: None,
            width: 10,
            height: 10,
            scale: 1.0,
            source: SourceWindow::default(),
            display: String::new(),
            duration_ms: None,
        }
    }

    #[test]
    fn a_capture_filed_while_the_app_runs_is_read_a_few_seconds_later() {
        let started = 10_000 * S;
        let unread = [entry("new", started + 10 * S), entry("old", started - 3600 * S)];
        let none = HashSet::new();
        // Two seconds after it was filed: too soon, and the old one waits for idle.
        assert_eq!(next(&unread, &none, started, started + 12 * S), Next::WhenIdle("old".into()));
        // Five seconds after: now.
        assert_eq!(next(&unread, &none, started, started + 15 * S), Next::Now("new".into()));
    }

    #[test]
    fn the_captures_from_before_wait_for_idle_newest_first() {
        let started = 10_000 * S;
        let unread = [entry("b", started - 10 * S), entry("a", started - 20 * S)];
        assert_eq!(next(&unread, &HashSet::new(), started, started), Next::WhenIdle("b".into()));
    }

    #[test]
    fn a_capture_that_failed_is_not_tried_again() {
        let started = 10_000 * S;
        let unread = [entry("bad", started + S), entry("old", started - S)];
        let failed: HashSet<String> = ["bad".to_owned()].into();
        assert_eq!(next(&unread, &failed, started, started + 60 * S), Next::WhenIdle("old".into()));
        let failed: HashSet<String> = ["bad".to_owned(), "old".to_owned()].into();
        assert_eq!(next(&unread, &failed, started, started + 60 * S), Next::Nothing);
    }
}
