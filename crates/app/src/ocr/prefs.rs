// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/07` §2.1's "downloadable language packs (with size shown)", as a settings group.
//!
//! One row a script, each saying what it reads, what it costs and whether it is here.
//! The size is next to the button because that is what it is for: 13 MB is not a number
//! to find out about afterwards, and `spec/00`'s "fast and private by design" means the
//! one download the app ever makes is one the user chose with the figure in front of them.
//!
//! **The first pack costs more than the ones after it.** The detection model is shared by
//! every script, so it is counted into every row's figure and downloaded once; a row
//! whose pack is the second one installed says the smaller number, because by then that
//! is what it will take.
//!
//! Here rather than in `prefs.rs` because this is the one settings group with a worker
//! thread, a progress bar and a cancel button behind it, and that is a page of state
//! rather than four rows of bindings.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use octosnap_ocr::Script;
use tracing::{info, warn};

use super::download::{self, Progress};

/// How often the bar reads the counters while a download runs. Fast enough to look live,
/// slow enough that it is not redrawing between packets.
const TICK: Duration = Duration::from_millis(120);

/// The group `prefs.rs` puts on the Advanced page under the Text Recognition rows.
pub fn packs_group() -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Language packs")
        .description(
            "Downloaded once and kept on this machine. The first pack also brings the \
             shared detection model, which is why it is the largest.",
        )
        .build();

    let rows: Vec<Rc<PackRow>> = super::packs().into_iter().map(PackRow::new).collect();
    for row in &rows {
        group.add(&row.row);
    }
    group
}

/// One script's row: what it costs, and the button that fetches or removes it.
struct PackRow {
    row: adw::ActionRow,
    script: Script,
    /// The pack's own size, for the subtitle when it is not installed.
    bytes: u64,
    button: gtk::Button,
    cancel: gtk::Button,
    bar: gtk::ProgressBar,
    /// The download in flight, which is also what Cancel stops.
    running: RefCell<Option<Progress>>,
    /// Bumped whenever a download ends, so a stale ticker stops rather than following
    /// the next one. The GIF editor's `render_run` for the same reason.
    run: Cell<u64>,
}

impl PackRow {
    fn new(pack: octosnap_ocr::Pack) -> Rc<Self> {
        let row = adw::ActionRow::builder().title(pack.script.name()).build();

        let bar = gtk::ProgressBar::builder()
            .valign(gtk::Align::Center)
            .width_request(120)
            .visible(false)
            .build();
        let cancel = gtk::Button::builder()
            .icon_name("process-stop-symbolic")
            .tooltip_text("Stop the download")
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        cancel.add_css_class("flat");
        let button = gtk::Button::builder().valign(gtk::Align::Center).build();

        let suffix = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        suffix.append(&bar);
        suffix.append(&cancel);
        suffix.append(&button);
        row.add_suffix(&suffix);

        let pack_row = Rc::new(Self {
            row,
            script: pack.script,
            bytes: pack.bytes,
            button: button.clone(),
            cancel: cancel.clone(),
            bar,
            running: RefCell::new(None),
            run: Cell::new(0),
        });
        pack_row.rest(pack.installed);

        let weak = Rc::downgrade(&pack_row);
        button.connect_clicked(move |_| {
            if let Some(row) = weak.upgrade() {
                row.pressed();
            }
        });
        let weak = Rc::downgrade(&pack_row);
        cancel.connect_clicked(move |_| {
            if let Some(row) = weak.upgrade()
                && let Some(progress) = row.running.borrow().as_ref()
            {
                info!(script = row.script.tag(), "cancelling a pack download");
                progress.cancel();
            }
        });
        pack_row
    }

    /// Install or Remove, depending on what the row is showing.
    fn pressed(self: &Rc<Self>) {
        if self.running.borrow().is_some() {
            return;
        }
        if installed(self.script) {
            self.take_away();
        } else {
            self.fetch();
        }
    }

    /// The row with nothing happening in it.
    fn rest(&self, installed: bool) {
        self.bar.set_visible(false);
        self.cancel.set_visible(false);
        self.button.set_sensitive(true);
        self.button.remove_css_class("destructive-action");
        self.button.remove_css_class("suggested-action");
        // Only Install is the whole row's. A click anywhere on an installed row would
        // otherwise delete a download there and then, with nothing to take it back; Remove
        // is its own button, and a plain one, so a list of installed packs is not a column
        // of red.
        if installed {
            self.row.set_subtitle("Installed");
            self.button.set_label("Remove");
            self.row.set_activatable_widget(None::<&gtk::Widget>);
        } else {
            self.row.set_subtitle(&megabytes(self.bytes));
            self.button.set_label("Install");
            self.row.set_activatable_widget(Some(&self.button));
        }
    }

    fn fetch(self: &Rc<Self>) {
        let progress = Progress::new();
        *self.running.borrow_mut() = Some(progress.clone());
        self.row.set_subtitle("Downloading\u{2026}");
        self.button.set_sensitive(false);
        self.bar.set_fraction(0.0);
        self.bar.set_visible(true);
        self.cancel.set_visible(true);
        self.follow();

        let home = super::home();
        let script = self.script;
        let watched = progress.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let outcome =
                gio::spawn_blocking(move || download::install(&home, script, &watched)).await;
            let Some(row) = weak.upgrade() else { return };
            *row.running.borrow_mut() = None;
            row.run.set(row.run.get() + 1);
            match outcome {
                Ok(Ok(())) => {
                    // The first pack is also the first detection model, so the engine
                    // this process opened is the one that found nothing. D85.
                    super::reopen();
                    info!(script = script.tag(), "installed a language pack");
                    row.rest(true);
                }
                Ok(Err(download::Failure::Cancelled)) => {
                    info!(script = script.tag(), "the download was cancelled");
                    row.rest(false);
                }
                Ok(Err(download::Failure::Why(why))) => row.failed(&why),
                Err(_) => row.failed("the download stopped unexpectedly"),
            }
        });
    }

    /// Follows the counters until this download is over.
    fn follow(self: &Rc<Self>) {
        let run = self.run.get();
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(TICK, move || {
            let Some(row) = weak.upgrade() else { return glib::ControlFlow::Break };
            if row.run.get() != run {
                return glib::ControlFlow::Break;
            }
            let Some(progress) = row.running.borrow().clone() else {
                return glib::ControlFlow::Break;
            };
            match progress.fraction() {
                Some(fraction) => {
                    row.bar.set_fraction(fraction);
                    let (done, total) = progress.bytes();
                    row.row.set_subtitle(&format!("{} of {}", megabytes(done), megabytes(total)));
                }
                // Nothing is known yet, which lasts about as long as the first request.
                None => row.bar.pulse(),
            }
            glib::ControlFlow::Continue
        });
    }

    fn take_away(self: &Rc<Self>) {
        match download::remove(&super::home(), self.script) {
            Ok(()) => {
                super::reopen();
                self.rest(false);
            }
            Err(why) => self.failed(&why),
        }
    }

    /// Says what went wrong on the row itself rather than in a notification.
    ///
    /// The user is looking at this row: they pressed its button a moment ago, and a
    /// message anywhere else would be a message they have to go and find.
    fn failed(&self, why: &str) {
        warn!(script = self.script.tag(), "the language pack could not be installed: {why}");
        self.bar.set_visible(false);
        self.cancel.set_visible(false);
        self.button.set_sensitive(true);
        self.button.set_label("Try again");
        self.row.set_subtitle(why);
    }
}

/// Whether a script's pack is on disk *now*, rather than when the page was built.
fn installed(script: Script) -> bool {
    super::packs().into_iter().any(|pack| pack.script == script && pack.installed)
}

/// A size a person can read. Megabytes throughout: every pack is between five and
/// seventeen of them, so switching units between rows would only make them harder to
/// compare with each other.
fn megabytes(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let mb = bytes as f64 / 1_000_000.0;
    format!("{mb:.1} MB")
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_size_reads_like_a_download() {
        assert_eq!(megabytes(12_875_358), "12.9 MB");
        assert_eq!(megabytes(0), "0.0 MB");
        assert_eq!(megabytes(21_509_645), "21.5 MB");
    }
}
