// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/11` M7's telemetry-free crash log.
//!
//! Nothing here sends anything anywhere. Three pieces:
//!
//! - **A log file of the app's own.** The lines the journal gets are also written to
//!   `$XDG_STATE_HOME/octosnap/app.log`. The journal is where a developer looks; it is
//!   not somewhere a sandboxed app can read, and not somewhere a person filing an issue
//!   should have to learn to look.
//! - **A marker that says a run is in progress.** Written at startup, removed at a clean
//!   exit. Found at the next startup, it means the last run ended without one -- a panic,
//!   a crash in a library, a kill -- so that run's log is kept as `crash-<when>.log`
//!   instead of being rotated away, and the app offers the report.
//! - **The report.** One plain-text file, written where the person chooses, with what an
//!   issue needs: the versions, the session, the monitors, the settings, this run's log,
//!   the last crash's log and the extension's recent lines. Plain text, so it can be read
//!   before it is shared, and it says at the top that it names files on this computer.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use tracing::{info, warn};

use crate::settings;

/// This run's log.
const LOG: &str = "app.log";
/// The run before, or this run's first half once it outgrew [`ROTATE_AT`].
const PREVIOUS: &str = "app.previous.log";
/// Present while a run is in progress; see the module comment.
const MARKER: &str = "running";
const CRASH_PREFIX: &str = "crash-";
const CRASH_SUFFIX: &str = ".log";
/// How many crashed runs' logs to keep. The newest is the one a report includes; the two
/// before it are for a person who had several crashes before they got round to reporting.
const KEEP_CRASHES: usize = 3;
/// When this run's log is moved aside and started again. A debug-logging session writes
/// about a megabyte an hour; an info one, a few kilobytes a day.
const ROTATE_AT: u64 = 4 * 1024 * 1024;
/// How much of each log a report carries: the end, which is where the trouble is.
const REPORT_TAIL: u64 = 256 * 1024;

/// Where the logs live: `$XDG_STATE_HOME/octosnap`, which inside the Flatpak is the
/// app's own `~/.var/app/<id>/.local/state/octosnap`.
#[must_use]
pub fn directory() -> PathBuf {
    glib::user_state_dir().join("octosnap")
}

/// What [`begin`] found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Begun {
    /// The last run's log, kept because that run never reached [`end`].
    pub crashed: Option<PathBuf>,
}

/// Starts a run's bookkeeping in `dir`: keeps or rotates the last run's log, and writes
/// the marker. `now` names a crash log, so it has to sort in time order.
///
/// # Errors
/// When `dir` cannot be created or written. The app runs without a log file then; the
/// journal still has every line.
pub fn begin(dir: &Path, now: &str) -> io::Result<Begun> {
    fs::create_dir_all(dir)?;
    let log = dir.join(LOG);
    let marker = dir.join(MARKER);
    let mut begun = Begun::default();
    if marker.exists() {
        // The marker holds the time the crashed run started, which names its log better
        // than the time this one noticed. An unreadable marker still means a crash.
        let started = fs::read_to_string(&marker).unwrap_or_default();
        let stamp = started.lines().next().map_or(now, str::trim);
        let stamp = if stamp.is_empty() { now } else { stamp };
        if log.exists() {
            let kept = dir.join(format!("{CRASH_PREFIX}{stamp}{CRASH_SUFFIX}"));
            fs::rename(&log, &kept)?;
            begun.crashed = Some(kept);
        }
        prune_crashes(dir)?;
    } else if log.exists() {
        fs::rename(&log, dir.join(PREVIOUS))?;
    }
    fs::write(&marker, format!("{now}\n"))?;
    Ok(begun)
}

/// Ends a run cleanly: the next [`begin`] rotates this run's log instead of keeping it.
pub fn end(dir: &Path) {
    match fs::remove_file(dir.join(MARKER)) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => warn!(dir = %dir.display(), "could not clear the run marker: {e}"),
    }
}

/// The crashed runs' logs in `dir`, oldest first.
#[must_use]
pub fn crashes(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(CRASH_PREFIX) && name.ends_with(CRASH_SUFFIX))
        })
        .collect();
    // The names carry a sortable time, so name order is time order.
    found.sort();
    found
}

fn prune_crashes(dir: &Path) -> io::Result<()> {
    let found = crashes(dir);
    let excess = found.len().saturating_sub(KEEP_CRASHES);
    for old in &found[..excess] {
        fs::remove_file(old)?;
    }
    Ok(())
}

/// The last `max` bytes of the file at `path`, from the first whole line in them.
///
/// # Errors
/// When the file cannot be opened or read.
pub fn tail(path: &Path, max: u64) -> io::Result<String> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(max);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    if start == 0 {
        return Ok(text.into_owned());
    }
    // A cut in the middle of a line would start the excerpt with half a sentence.
    Ok(text.split_once('\n').map_or(text.as_ref(), |(_, rest)| rest).to_owned())
}

/// This run's log file, as a `tracing` writer.
///
/// One `write` per event: `tracing-subscriber` formats a whole line before writing it,
/// so a line is never interleaved with another thread's. Unbuffered, because the line a
/// crash needs most is the last one before it, and a buffer would still be holding it.
#[derive(Debug, Clone)]
pub struct LogFile {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug)]
struct Inner {
    dir: PathBuf,
    file: File,
    written: u64,
}

impl LogFile {
    /// Opens `dir`'s log for appending.
    ///
    /// # Errors
    /// When the file cannot be opened.
    pub fn open(dir: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(dir.join(LOG))?;
        let written = file.metadata()?.len();
        Ok(Self { inner: Arc::new(Mutex::new(Inner { dir: dir.to_owned(), file, written })) })
    }
}

impl Write for LogFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if inner.written >= ROTATE_AT {
            let log = inner.dir.join(LOG);
            fs::rename(&log, inner.dir.join(PREVIOUS))?;
            inner.file = OpenOptions::new().create(true).append(true).open(&log)?;
            inner.written = 0;
        }
        let n = inner.file.write(buf)?;
        inner.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.file.flush()
    }
}

/// Writes a panic's message and where it happened to the log before the process goes.
///
/// A panic in a GTK callback aborts, because it cannot unwind through C. Rust's default
/// hook prints to stderr, which reaches the journal and nothing else; this puts the same
/// words, with a backtrace, in the file the report reads. The default hook still runs.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!("panicked: {panic}\n{backtrace}");
        default(panic);
    }));
}

/// A name for the report that sorts by time and says what it is.
#[must_use]
pub fn report_name(now: &chrono::DateTime<chrono::Local>) -> String {
    format!("octosnap-report-{}.txt", now.format("%Y-%m-%d-%H%M%S"))
}

/// The time format [`begin`] names crash logs with.
#[must_use]
pub fn stamp(now: &chrono::DateTime<chrono::Local>) -> String {
    now.format("%Y-%m-%d-%H%M%S").to_string()
}

/// Writes the report to `path`.
///
/// Every section is best effort: a report is most wanted exactly when something is
/// broken, so a piece that cannot be read says so in its place and the rest is written.
///
/// # Errors
/// When the file itself cannot be written.
pub async fn write_report(path: &Path) -> io::Result<()> {
    let mut out = String::new();
    let now = chrono::Local::now();
    out.push_str("OctoSnap report\n================\n\n");
    out.push_str(&format!("Written {}.\n\n", now.to_rfc2822()));
    out.push_str(
        "Nothing in this file has been sent anywhere. Read it before you share it: it names\n\
         files and folders on this computer, and the logs show what the app was doing.\n\n",
    );

    section(&mut out, "Versions");
    out.push_str(&format!("OctoSnap {}\n", env!("CARGO_PKG_VERSION")));
    out.push_str(&format!(
        "Extension protocol this build speaks: {}\n",
        octosnap_core::protocol::PROTOCOL_VERSION
    ));
    out.push_str(&format!(
        "GTK {}.{}.{}, libadwaita {}.{}.{}\n",
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version(),
        adw::major_version(),
        adw::minor_version(),
        adw::micro_version(),
    ));
    out.push_str(&format!("Build: {}\n", if cfg!(debug_assertions) { "debug" } else { "release" }));
    out.push_str(&format!("Sandbox: {}\n", if settings::sandboxed() { "Flatpak" } else { "none" }));
    out.push_str(&format!("System: {}\n", operating_system()));
    for key in ["XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP", "LANG"] {
        let value = std::env::var(key).unwrap_or_else(|_| "(not set)".to_owned());
        out.push_str(&format!("{key}={value}\n"));
    }

    section(&mut out, "GNOME Shell and the extension");
    let connection = gio::Application::default().and_then(|app| app.dbus_connection());
    match &connection {
        Some(connection) => {
            out.push_str(&format!("GNOME Shell {}\n", shell_version(connection).await));
            out.push_str(&format!("Extension: {}\n", crate::setup::status(connection).await.describe()));
        }
        None => out.push_str("No session bus connection.\n"),
    }

    section(&mut out, "Monitors");
    match crate::capture_flow() {
        Some(flow) => {
            use octosnap_shell::ShellBridge;
            match flow.bridge().monitors().await {
                Ok(monitors) => {
                    for m in monitors {
                        let (pw, ph) = m.geometry.to_physical(m.scale);
                        out.push_str(&format!(
                            "{} {}x{}+{}+{} logical, {pw}x{ph} physical, scale {}{}{}\n",
                            if m.connector.is_empty() { "?" } else { &m.connector },
                            m.geometry.width,
                            m.geometry.height,
                            m.geometry.x,
                            m.geometry.y,
                            m.scale,
                            if m.primary { ", primary" } else { "" },
                            m.refresh.map(|hz| format!(", {hz:.2} Hz")).unwrap_or_default(),
                        ));
                    }
                }
                Err(e) => out.push_str(&format!("Could not ask the extension: {e}\n")),
            }
        }
        None => out.push_str("The capture flow was never built.\n"),
    }

    section(&mut out, "Settings");
    out.push_str(&settings_listing(settings::open_schema(settings::SCHEMA_ID).as_ref()));
    section(&mut out, "Extension settings");
    out.push_str(&settings_listing(settings::extension_settings().as_ref()));

    let dir = directory();
    section(&mut out, "This run's log (the end of it)");
    out.push_str(&tail(&dir.join(LOG), REPORT_TAIL).unwrap_or_else(|e| format!("Not readable: {e}\n")));

    section(&mut out, "The last run that did not end cleanly");
    match crashes(&dir).last() {
        Some(crash) => {
            out.push_str(&format!("{}\n\n", crash.display()));
            out.push_str(&tail(crash, REPORT_TAIL).unwrap_or_else(|e| format!("Not readable: {e}\n")));
        }
        None => out.push_str("None kept.\n"),
    }

    section(&mut out, "The extension's recent lines");
    match crate::capture_flow() {
        Some(flow) => {
            use octosnap_shell::ShellBridge;
            match flow.bridge().recent_log().await {
                Ok(lines) if lines.is_empty() => out.push_str("None.\n"),
                Ok(lines) => {
                    for line in lines {
                        out.push_str(&line);
                        out.push('\n');
                    }
                }
                Err(e) => out.push_str(&format!("Could not ask the extension: {e}\n")),
            }
        }
        None => out.push_str("The capture flow was never built.\n"),
    }

    fs::write(path, out)?;
    info!(path = %path.display(), "wrote a report");
    Ok(())
}

/// `app.export-report`: asks where, then writes [`write_report`] there.
pub fn register(app: &adw::Application) {
    let export = gio::SimpleAction::new("export-report", None);
    {
        let app = app.clone();
        export.connect_activate(move |_, _| {
            let app = app.clone();
            glib::spawn_future_local(async move { export_report(&app).await });
        });
    }
    app.add_action(&export);

    // `write-report (path)`: the same report with no dialog, for the harness
    // (`firstrun-test.sh`), which cannot answer a file chooser.
    let write = gio::SimpleAction::new("write-report", Some(glib::VariantTy::STRING));
    write.connect_activate(|_, parameter| {
        let Some(path) = parameter.and_then(glib::Variant::str).map(PathBuf::from) else {
            warn!("write-report needs a path");
            return;
        };
        glib::spawn_future_local(async move {
            if let Err(e) = write_report(&path).await {
                warn!(path = %path.display(), "could not write the report: {e}");
            }
        });
    });
    app.add_action(&write);
}

async fn export_report(app: &adw::Application) {
    let dialog = gtk::FileDialog::builder()
        .title("Save a Report")
        .initial_name(report_name(&chrono::Local::now()))
        .modal(true)
        .build();
    let parent = app.active_window();
    let file = match dialog.save_future(parent.as_ref()).await {
        Ok(file) => file,
        // Dismissed: nothing to say.
        Err(_) => return,
    };
    let Some(path) = file.path() else {
        warn!(uri = %file.uri(), "a report can only be written to a local file");
        return;
    };
    // The report asks the shell half a handful of questions, each with a timeout of its
    // own, so it can take seconds. The result replaces this under the same id.
    let saving = gio::Notification::new("Saving the report\u{2026}");
    saving.set_body(Some(&path.display().to_string()));
    app.send_notification(Some("report"), &saving);
    let notification = match write_report(&path).await {
        Ok(()) => {
            let saved = gio::Notification::new("Saved the report");
            saved.set_body(Some(&format!(
                "{}. Read it before you attach it to an issue.",
                path.display()
            )));
            saved.add_button_with_target_value(
                "Show in Files",
                "app.reveal-file",
                Some(&path.display().to_string().to_variant()),
            );
            saved
        }
        Err(e) => {
            warn!(path = %path.display(), "could not write the report: {e}");
            let failed = gio::Notification::new("Could not save the report");
            failed.set_body(Some(&e.to_string()));
            failed
        }
    };
    app.send_notification(Some("report"), &notification);
}

/// Tells the user the last run ended without shutting down, and offers the report.
///
/// A notification rather than a window: the app starts in the background, often at login,
/// and a dialog there would be in the way of whatever the person sat down to do.
pub fn offer_report(app: &adw::Application, crash: &Path) {
    let notification = gio::Notification::new("OctoSnap stopped unexpectedly");
    notification.set_body(Some(
        "A report of what it was doing is ready if you want to send one. \
         Nothing is sent unless you do.",
    ));
    notification.add_button("Save Report\u{2026}", "app.export-report");
    // Normal: the button is the whole offer, and a low-priority notification can go to
    // the tray without showing it.
    notification.set_priority(gio::NotificationPriority::Normal);
    app.send_notification(Some("crashed"), &notification);
    info!(log = %crash.display(), "offered a report of the last run");
}

fn section(out: &mut String, title: &str) {
    out.push_str(&format!("\n## {title}\n\n"));
}

/// Every key of `settings` and its value, sorted by key.
fn settings_listing(settings: Option<&gio::Settings>) -> String {
    let Some(settings) = settings else { return "Not available.\n".to_owned() };
    let Some(schema) = settings.settings_schema() else { return "No schema.\n".to_owned() };
    let mut keys: Vec<String> = schema.list_keys().iter().map(ToString::to_string).collect();
    keys.sort();
    let mut out = String::new();
    for key in keys {
        out.push_str(&format!("{key} = {}\n", settings.value(&key).print(false)));
    }
    out
}

/// The host's name for itself. Inside the Flatpak `/etc/os-release` is the runtime's,
/// and the host's is at `/run/host/os-release`.
fn operating_system() -> String {
    let pretty = |path: &str| {
        let text = fs::read_to_string(path).ok()?;
        text.lines()
            .find_map(|line| line.strip_prefix("PRETTY_NAME="))
            .map(|name| name.trim_matches('"').to_owned())
    };
    match (pretty("/run/host/os-release"), pretty("/etc/os-release")) {
        (Some(host), Some(runtime)) => format!("{host} (runtime: {runtime})"),
        (Some(name), None) | (None, Some(name)) => name,
        (None, None) => "unknown".to_owned(),
    }
}

async fn shell_version(connection: &gio::DBusConnection) -> String {
    let reply = connection
        .call_future(
            Some("org.gnome.Shell"),
            "/org/gnome/Shell",
            "org.freedesktop.DBus.Properties",
            "Get",
            Some(&("org.gnome.Shell", "ShellVersion").to_variant()),
            glib::VariantTy::new("(v)").ok(),
            gio::DBusCallFlags::NONE,
            2000,
        )
        .await;
    match reply {
        Ok(reply) => reply
            .child_value(0)
            .as_variant()
            .and_then(|value| value.str().map(str::to_owned))
            .unwrap_or_else(|| "(unreadable)".to_owned()),
        Err(e) => format!("(could not ask: {e})"),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_default()
    }

    #[test]
    fn a_clean_run_rotates_the_last_log_away() {
        let dir = tempfile::tempdir().expect("a temp dir");
        fs::write(dir.path().join(LOG), "last run\n").expect("write");
        let begun = begin(dir.path(), "2026-09-25-120000").expect("begin");
        assert_eq!(begun, Begun::default());
        assert_eq!(read(&dir.path().join(PREVIOUS)), "last run\n");
        assert!(!dir.path().join(LOG).exists());
        assert!(dir.path().join(MARKER).exists());
        end(dir.path());
        assert!(!dir.path().join(MARKER).exists());
    }

    #[test]
    fn a_run_that_never_ended_keeps_its_log_under_its_own_start_time() {
        let dir = tempfile::tempdir().expect("a temp dir");
        begin(dir.path(), "2026-09-25-090000").expect("the run that crashes");
        fs::write(dir.path().join(LOG), "panicked: here\n").expect("write");
        // No `end`: the process died.
        let begun = begin(dir.path(), "2026-09-25-100000").expect("the next run");
        let kept = dir.path().join("crash-2026-09-25-090000.log");
        assert_eq!(begun.crashed.as_deref(), Some(kept.as_path()));
        assert_eq!(read(&kept), "panicked: here\n");
        assert!(!dir.path().join(PREVIOUS).exists(), "a crash log is not also rotated");
        assert_eq!(read(&dir.path().join(MARKER)), "2026-09-25-100000\n");
    }

    #[test]
    fn a_crash_with_nothing_logged_keeps_nothing() {
        let dir = tempfile::tempdir().expect("a temp dir");
        begin(dir.path(), "2026-09-25-090000").expect("begin");
        let begun = begin(dir.path(), "2026-09-25-100000").expect("begin again");
        assert_eq!(begun.crashed, None);
    }

    #[test]
    fn only_the_newest_crash_logs_are_kept() {
        let dir = tempfile::tempdir().expect("a temp dir");
        for hour in 1..=5 {
            begin(dir.path(), &format!("2026-09-25-0{hour}0000")).expect("begin");
            fs::write(dir.path().join(LOG), format!("run {hour}\n")).expect("write");
        }
        begin(dir.path(), "2026-09-25-060000").expect("begin");
        let kept: Vec<String> = crashes(dir.path())
            .iter()
            .map(|path| read(path))
            .collect();
        assert_eq!(kept, ["run 3\n", "run 4\n", "run 5\n"]);
    }

    #[test]
    fn the_log_file_rotates_when_it_outgrows_its_cap() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut log = LogFile::open(dir.path()).expect("open");
        let line = vec![b'x'; 1024 * 1024];
        for _ in 0..4 {
            log.write_all(&line).expect("write");
        }
        log.write_all(b"after\n").expect("write");
        assert_eq!(read(&dir.path().join(LOG)), "after\n");
        assert_eq!(fs::metadata(dir.path().join(PREVIOUS)).expect("previous").len(), ROTATE_AT);
    }

    #[test]
    fn a_tail_starts_at_a_whole_line() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("log");
        fs::write(&path, "first line\nsecond line\nthird\n").expect("write");
        assert_eq!(tail(&path, 16).expect("tail"), "third\n");
        assert_eq!(tail(&path, 1000).expect("tail"), "first line\nsecond line\nthird\n");
    }

    #[test]
    fn the_report_name_sorts_by_time() {
        let at = chrono::TimeZone::with_ymd_and_hms(&chrono::Local, 2026, 9, 25, 13, 5, 9)
            .single()
            .expect("a local time");
        assert_eq!(report_name(&at), "octosnap-report-2026-09-25-130509.txt");
        assert_eq!(stamp(&at), "2026-09-25-130509");
    }
}
