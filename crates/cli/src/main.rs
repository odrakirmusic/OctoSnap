// SPDX-License-Identifier: GPL-3.0-or-later

//! `octosnap`: the command-line front end and `octosnap://` URL handler
//! (`spec/10` §3.2).
//!
//! Everything that touches the extension goes through `octosnap-shell`'s bridge rather
//! than decoding variants here. An earlier version of this file had its own copy of the
//! decode and silently missed the connector enrichment the bridge does, which is exactly
//! the class of bug a second implementation of a wire format invites.
//!
//! `capture` does **not** call the extension's `BeginCapture` directly, even though it
//! could and that would be one hop shorter. It activates the app's `capture` action
//! instead, for three reasons that all point the same way: the app is the only half that
//! knows whether the extension is present at all (`spec/00` §4.3's degraded mode), it
//! owns `api-enabled` (`spec/08` §9), and it is where the `octosnap://` scheme lands --
//! so routing through it means the CLI and the URL scheme are one implementation rather
//! than two that drift. There is a bonus: the app has to be running to receive the
//! capture anyway, and starting it *before* the capture takes the cold-activation race
//! (`docs/spikes/13`) off the capture path entirely.

mod url;

use clap::{Parser, Subcommand, ValueEnum};
use glib::prelude::ToVariant;
use octosnap_core::{Monitor, Rect};
use octosnap_core::capture::{CaptureMode, RequestedAction};
use octosnap_core::protocol::{APP_BUS_NAME, APP_INTERFACE, APP_OBJECT_PATH, PROTOCOL_VERSION};
use octosnap_core::request::{CaptureRequest, parse_rect};
use gio::prelude::SettingsExt;
use octosnap_shell::{GnomeExtensionBridge, ShellBridge, capture};

const TIMEOUT_MS: i32 = 2_000;

/// `org.freedesktop.Application`, which `GApplication` exports during registration --
/// before it takes the bus name, which is what makes an action activation race-free
/// where a call to `App1` is not (`docs/spikes/13-cold-activation-race.md`).
const FDO_APPLICATION_INTERFACE: &str = "org.freedesktop.Application";
const APP_GAPPLICATION_PATH: &str = "/io/github/odrakirmusic/OctoSnap";

/// How many times to retry a call that lost the cold-activation race, and how long to
/// wait between attempts. See `docs/spikes/13-cold-activation-race.md`: when a call to
/// `App1` is the call that D-Bus-activates the app, GDBus rejects it from its worker
/// thread before the app has exported the interface. The app is running by the time the
/// rejection arrives, so retrying works -- the first attempt is what pays for the start.
const ACTIVATION_RETRIES: u32 = 5;
const ACTIVATION_RETRY_DELAY_MS: u64 = 250;

#[derive(Parser)]
#[command(
    name = "octosnap",
    about = "OctoSnap command line",
    long_about = "Screenshot, annotation and screen-recording tool for GNOME on Wayland.",
    version
)]
struct Cli {
    /// Nothing: open OctoSnap, the way the app grid does.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Take a screenshot.
    Capture(CaptureArgs),
    /// Record a GIF of a rectangle for a fixed time, in this process: the recorder's
    /// engine driven from the command line (`spec/06`, M5), which is how it is measured.
    Record(RecordArgs),
    /// Stop the recording in progress in the running app (`spec/06` §3). A no-op when
    /// nothing is recording; the app decides, so this only relays.
    StopRecording,
    /// Open a `.octosnap` project for editing (`spec/05` §8).
    Open {
        /// The project file.
        path: std::path::PathBuf,
    },
    /// Open the settings window.
    Settings {
        /// Which page to open.
        #[arg(value_enum, default_value_t = SettingsTab::General)]
        tab: SettingsTab,
    },
    /// Put the most recent capture back on the clipboard.
    CopyLast,
    /// Write the most recent capture to the export location.
    SaveLast,
    /// Pin an image to the screen (`spec/07` §3).
    Pin {
        /// The image file.
        path: std::path::PathBuf,
    },
    /// Open an image, or a `.octosnap` project, in the annotation editor.
    Annotate {
        /// The file.
        path: std::path::PathBuf,
    },
    /// Open the clipboard's image in the annotation editor.
    OpenFromClipboard,
    /// Hide, show or toggle the desktop icons.
    DesktopIcons {
        /// What to do with them.
        #[arg(value_enum, default_value_t = IconsAction::Toggle)]
        action: IconsAction,
    },
    /// Add an image file to the Quick Access Overlay as a card.
    AddOverlay {
        /// The image file.
        path: std::path::PathBuf,
    },
    /// Recognise the text in an image and copy it (`spec/07` §2.1).
    Text {
        /// The image file. Without one, select an area on screen and read that.
        path: Option<std::path::PathBuf>,
        /// Keep the line breaks the page had, overriding the setting.
        #[arg(long)]
        linebreaks: bool,
        /// Join each paragraph into one line, overriding the setting.
        #[arg(long, conflicts_with = "linebreaks")]
        single_line: bool,
    },
    /// Open the capture history.
    History,
    /// Bring back the most recently closed card.
    Restore,
    /// Call the application's Ping method, starting it via D-Bus activation if needed.
    Ping,
    /// Report the shell extension's version and protocol.
    ShellVersion,
    /// Print the monitor layout as the extension sees it, in logical coordinates.
    Monitors,
    /// Report on both halves at once.
    Status,
}

#[derive(clap::Args)]
struct CaptureArgs {
    /// What to capture.
    #[arg(short, long, value_enum, default_value_t = Mode::Area)]
    mode: Mode,

    /// What to do with it, overriding the configured after-capture actions.
    #[arg(short, long, value_enum)]
    action: Option<Action>,

    /// Skip the overlay and capture exactly this region, as logical x,y,width,height.
    #[arg(long, value_name = "X,Y,W,H")]
    rect: Option<String>,

    /// Which monitor, by connector name. See `octosnap monitors`.
    #[arg(long, value_name = "CONNECTOR")]
    display: Option<String>,

    /// Freeze the screen while selecting, overriding the setting.
    #[arg(long)]
    freeze: bool,
    /// Do not freeze the screen, overriding the setting.
    #[arg(long, conflicts_with = "freeze")]
    no_freeze: bool,

    /// Include the pointer, overriding the setting.
    #[arg(long)]
    cursor: bool,
    /// Exclude the pointer, overriding the setting.
    #[arg(long, conflicts_with = "cursor")]
    no_cursor: bool,

    /// Countdown in seconds. Implies --mode self-timer.
    #[arg(long, value_name = "SECONDS")]
    timer: Option<i32>,
}

#[derive(clap::Args)]
struct RecordArgs {
    /// The output is a GIF. The only format M5 records; accepted so scripts read right.
    #[arg(long)]
    gif: bool,

    /// Record exactly this region, as logical x,y,width,height. Default: the whole
    /// monitor under the pointer.
    #[arg(long, value_name = "X,Y,W,H")]
    rect: Option<String>,

    /// Which monitor, by connector name. See `octosnap monitors`.
    #[arg(long, value_name = "CONNECTOR")]
    display: Option<String>,

    /// How long to record.
    #[arg(long, value_name = "SECONDS", default_value_t = 5.0)]
    seconds: f64,

    /// Frames per second (`gif-fps`); 0 is the recorded screen's own rate, and nothing
    /// above 50 is honoured, because a GIF cannot play faster (D101).
    #[arg(long, default_value_t = 15)]
    fps: u32,

    /// The widest the GIF may be, in pixels; 0 for no cap (`gif-max-width`).
    #[arg(long, value_name = "PIXELS", default_value_t = 800)]
    max_width: u32,

    /// gifski's quality, 1-100 (`gif-quality`).
    #[arg(long, default_value_t = 80)]
    quality: u8,

    /// Leave the pointer out of the recording (`rec-cursor`).
    #[arg(long)]
    no_cursor: bool,

    /// Where to write the GIF. Default: `octosnap-<date>.gif` in the current directory.
    #[arg(short, long, value_name = "FILE")]
    output: Option<std::path::PathBuf>,

    /// How pipewiresrc is pointed at the stream (`docs/spikes/11`).
    #[arg(long, value_enum, default_value_t = TargetArg::NodeId)]
    target: TargetArg,

    /// Tell the compositor this is a recording, so it shows its own indicator too.
    #[arg(long)]
    mark_recording: bool,
}

/// `pipewiresrc`'s two ways of naming a node.
#[derive(Clone, Copy, ValueEnum)]
enum TargetArg {
    /// `path=<node id>`, straight from `PipeWireStreamAdded`.
    NodeId,
    /// `target-object=<object.serial>`, resolved through `pw-cli`.
    Serial,
}

/// `DSK-01`'s three verbs.
#[derive(Clone, Copy, ValueEnum)]
enum IconsAction {
    Toggle,
    Hide,
    Show,
}

impl IconsAction {
    fn as_wire(self) -> &'static str {
        match self {
            Self::Toggle => url::DesktopIcons::Toggle.as_wire(),
            Self::Hide => url::DesktopIcons::Hide.as_wire(),
            Self::Show => url::DesktopIcons::Show.as_wire(),
        }
    }
}

/// `spec/10` §3.1's mode vocabulary, limited to what a CLI can usefully ask for.
///
/// `record` is absent because `spec/11` puts recording in M5 and `octosnap record` will
/// want its own subcommand with its own flags rather than a mode of `capture`.
#[derive(Clone, Copy, ValueEnum)]
enum Mode {
    /// The overlay with its toolbar and the last selection restored.
    AllInOne,
    /// Drag out a region.
    Area,
    /// Pick a window.
    Window,
    /// The whole monitor under the pointer.
    Fullscreen,
    /// The last selection again, with no overlay.
    PreviousArea,
    /// Select a region, then capture after a countdown.
    SelfTimer,
}

impl From<Mode> for CaptureMode {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::AllInOne => Self::AllInOne,
            Mode::Area => Self::Area,
            Mode::Window => Self::Window,
            Mode::Fullscreen => Self::Fullscreen,
            Mode::PreviousArea => Self::PreviousArea,
            Mode::SelfTimer => Self::SelfTimer,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Action {
    Copy,
    Save,
    Annotate,
    Upload,
    Pin,
}

impl From<Action> for RequestedAction {
    fn from(action: Action) -> Self {
        match action {
            Action::Copy => Self::Copy,
            Action::Save => Self::Save,
            Action::Annotate => Self::Annotate,
            Action::Upload => Self::Upload,
            Action::Pin => Self::Pin,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum SettingsTab {
    General,
    Shortcuts,
    QuickAccess,
    Screenshots,
    Annotate,
    Advanced,
    About,
}

impl SettingsTab {
    const fn as_wire(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Shortcuts => "shortcuts",
            Self::QuickAccess => "quick-access",
            Self::Screenshots => "screenshots",
            Self::Annotate => "annotate",
            Self::Advanced => "advanced",
            Self::About => "about",
        }
    }
}

impl CaptureArgs {
    /// Turns the flags into a request, or explains what is wrong with them.
    ///
    /// The paired `--freeze` / `--no-freeze` flags exist because a single `--freeze` bool
    /// cannot express "leave the setting alone", and that is the default. clap's
    /// `conflicts_with` makes passing both an error rather than a silent precedence rule.
    fn to_request(&self) -> Result<CaptureRequest, String> {
        let rect = match &self.rect {
            Some(text) => Some(parse_rect(text).map_err(|e| format!("--rect: {e}"))?),
            None => None,
        };

        // A countdown only means something in the mode that counts down, and silently
        // ignoring it would look like the timer had run and been zero.
        let mode = if self.timer.is_some() && matches!(self.mode, Mode::Area) {
            Mode::SelfTimer
        } else {
            self.mode
        };
        if self.timer.is_some() && !matches!(mode, Mode::SelfTimer) {
            return Err("--timer only applies to --mode self-timer".to_owned());
        }
        if let Some(seconds) = self.timer
            && seconds < 0
        {
            return Err("--timer cannot be negative".to_owned());
        }

        Ok(CaptureRequest::new(mode.into())
            .with_rect(rect)
            .with_display(self.display.clone().unwrap_or_default())
            .with_action(self.action.map(Into::into))
            .with_freeze(tristate(self.freeze, self.no_freeze))
            .with_cursor(tristate(self.cursor, self.no_cursor))
            .with_timer(self.timer))
    }
}

/// `None` when neither flag was given, which is what "leave the user's setting alone"
/// looks like on the wire (`spec/08` §6).
const fn tristate(yes: bool, no: bool) -> Option<bool> {
    match (yes, no) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    }
}

fn main() -> std::process::ExitCode {
    // The `.desktop` file's `Exec=octosnap %u` means a `octosnap://` link arrives as
    // argv, not as a subcommand, so it is handled before clap ever sees it. clap would
    // otherwise reject it as an unknown subcommand, which is a confusing thing for a
    // link to do.
    let mut argv = std::env::args();
    let program = argv.next();
    if let Some(first) = argv.next() {
        if first.starts_with(url::SCHEME) {
            let extra: Vec<String> = argv.collect();
            if !extra.is_empty() {
                eprintln!("octosnap: an octosnap:// link takes no other arguments");
                return std::process::ExitCode::FAILURE;
            }
            drop(program);
            return handle_url(&first);
        }
        // A `.octosnap` project, which a file manager hands over the same way: the
        // `.desktop` file's `Exec=octosnap %u` passes a `file://` URI in argv, and clap
        // would reject it as an unknown subcommand. `spec/05` §8's whole point is that
        // double-clicking one reopens an editable document.
        if let Some(path) = project_argument(&first) {
            drop(program);
            return if open_project(&path) {
                std::process::ExitCode::SUCCESS
            } else {
                std::process::ExitCode::FAILURE
            };
        }
    }

    let cli = Cli::parse();

    let connection = match gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cannot reach the session bus: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let bridge = GnomeExtensionBridge::from_connection(connection.clone());

    // The bare launch: the `.desktop` file's `Exec=octosnap %u` with nothing to hand
    // over, which is the app grid. It used to print this program's usage to a terminal
    // nobody could see, so clicking OctoSnap's icon did nothing at all.
    let Some(command) = cli.command else {
        return if launch(&connection) {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::FAILURE
        };
    };

    let ok = glib::MainContext::default().block_on(async {
        match command {
            Command::Capture(args) => match args.to_request() {
                Ok(request) if api_enabled() => activate(
                    &connection,
                    "capture",
                    Some(capture::request_to_variant(&request)),
                ),
                Ok(_) => {
                    eprintln!(
                        "octosnap: the command line is disabled. \
                         Turn it back on in Settings > Advanced, or with:\n  \
                         gsettings set io.github.odrakirmusic.OctoSnap api-enabled true"
                    );
                    false
                }
                Err(message) => {
                    eprintln!("octosnap: {message}");
                    false
                }
            },
            Command::Record(args) => {
                if api_enabled() {
                    record(&connection, &bridge, args).await
                } else {
                    gated(|| false)
                }
            }
            Command::StopRecording => gated(|| activate(&connection, "stop-recording", None)),
            Command::Open { path } => open_project(&path),
            Command::Settings { tab } => activate(
                &connection,
                "open-settings",
                Some(glib::Variant::from(tab.as_wire())),
            ),
            Command::CopyLast => activate(&connection, "copy-last", None),
            Command::SaveLast => activate(&connection, "save-last", None),
            Command::Pin { path } => with_file(&connection, "pin-file", &path),
            Command::Annotate { path } => with_file(&connection, "annotate-file", &path),
            Command::OpenFromClipboard => gated(|| activate(&connection, "open-from-clipboard", None)),
            Command::DesktopIcons { action } => gated(|| {
                activate(&connection, "desktop-icons", Some(glib::Variant::from(action.as_wire())))
            }),
            Command::AddOverlay { path } => with_file(&connection, "add-overlay", &path),
            // With a file it reads that file; without one it is an area capture in
            // `spec/07` §2.1's text mode, which goes through the overlay like any other
            // selection and comes back to the app as a capture.
            Command::Text { path, linebreaks, single_line } => {
                let keep = match (linebreaks, single_line) {
                    (true, _) => Some(true),
                    (_, true) => Some(false),
                    _ => None,
                };
                match path {
                    Some(path) => read_text(&connection, &path, keep),
                    None => gated(|| {
                        let request = CaptureRequest {
                            linebreaks: keep,
                            ..CaptureRequest::new(CaptureMode::Ocr)
                        };
                        activate(
                            &connection,
                            "capture",
                            Some(capture::request_to_variant(&request)),
                        )
                    }),
                }
            }
            Command::History => activate(&connection, "open-history", None),
            Command::Restore => activate(&connection, "restore-recent", None),
            Command::Ping => print_ping(&connection, true),
            Command::ShellVersion => print_shell_version(&bridge).await,
            Command::Monitors => print_monitors(&bridge).await,
            Command::Status => {
                // Report everything rather than stopping at the first failure: "the app
                // is up but the extension is not" is the single most useful thing to
                // know, and short-circuiting would hide half of it.
                //
                // Without starting the app. A status probe that D-Bus-activates the
                // service reports "running" about a process it just created, and that
                // process does everything a launch does -- the history janitor files
                // whatever the spool holds, for one. On 2026-09-12 a probe run against a
                // nested session did exactly that, into the developer's own history.
                let app = print_ping(&connection, false);
                let shell = print_shell_version(&bridge).await;
                let monitors = if shell { print_monitors(&bridge).await } else { true };
                app && shell && monitors
            }
        }
    });

    if ok { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE }
}

/// Routes a `octosnap://` link to the same actions the subcommands use.
/// A `.octosnap` path, from a bare argument or a `file://` URI.
///
/// `None` for anything else, so an ordinary subcommand still reaches clap.
fn project_argument(argument: &str) -> Option<std::path::PathBuf> {
    let path = match argument.strip_prefix("file://") {
        // Percent-decoding, because a file manager encodes spaces and anything else
        // awkward. `glib::filename_from_uri` is the same decoder GIO uses, so a name this
        // accepts is a name the app will find.
        Some(_) => glib::filename_from_uri(argument).ok().map(|(path, _)| path)?,
        None => std::path::PathBuf::from(argument),
    };
    octosnap_core::project::is_project(&path).then_some(path)
}

/// `spec/05` §8: hands a project to the running app.
///
/// Absolute, because the app's working directory is not this one -- it is whatever
/// directory the D-Bus activation happened to inherit, and a relative path would resolve
/// against that.
fn open_project(path: &std::path::Path) -> bool {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    };
    if !absolute.exists() {
        eprintln!("octosnap: {}: no such project", absolute.display());
        return false;
    }
    let connection = match gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cannot reach the session bus: {e}");
            return false;
        }
    };
    let parameter = absolute.to_string_lossy().to_string().to_variant();
    activate(&connection, "open-project", Some(parameter))
}

fn handle_url(link: &str) -> std::process::ExitCode {
    let command = match url::parse(link) {
        Ok(command) => command,
        Err(e) => {
            eprintln!("octosnap: {link}: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let connection = match gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cannot reach the session bus: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let ok = match command {
        url::Command::Capture(request) if !api_enabled() => {
            eprintln!(
                "octosnap: octosnap:// links are disabled. \
                 Turn them back on in Settings > Advanced."
            );
            let _ = request;
            false
        }
        url::Command::Capture(request) => activate(
            &connection,
            "capture",
            Some(capture::request_to_variant(&request)),
        ),
        url::Command::OpenSettings(tab) => {
            activate(&connection, "open-settings", Some(glib::Variant::from(tab)))
        }
        url::Command::CopyLast => activate(&connection, "copy-last", None),
        url::Command::SaveLast => activate(&connection, "save-last", None),
        url::Command::Pin(path) => with_file(&connection, "pin-file", &path),
        url::Command::OpenAnnotate(path) => with_file(&connection, "annotate-file", &path),
        url::Command::OpenFromClipboard => gated(|| activate(&connection, "open-from-clipboard", None)),
        url::Command::DesktopIcons(what) => gated(|| {
            activate(&connection, "desktop-icons", Some(glib::Variant::from(what.as_wire())))
        }),
        url::Command::AddQuickAccessOverlay(path) => with_file(&connection, "add-overlay", &path),
        url::Command::OpenHistory => activate(&connection, "open-history", None),
        url::Command::RestoreRecentlyClosed => activate(&connection, "restore-recent", None),
        url::Command::RecordGif(rect) => gated(|| {
            // A bare `record-gif` records the whole current monitor, which the extension
            // knows and this process does not, so it is resolved over the bus. An explicit
            // rect skips that round trip.
            let bridge = GnomeExtensionBridge::from_connection(connection.clone());
            glib::MainContext::default().block_on(record_gif(&connection, &bridge, rect))
        }),
        url::Command::StopRecording => gated(|| activate(&connection, "stop-recording", None)),
        url::Command::CaptureText { file, linebreaks } => {
            read_text(&connection, &file, linebreaks)
        }
        url::Command::ScrollingCapture(request) => gated(|| {
            let bridge = GnomeExtensionBridge::from_connection(connection.clone());
            scrolling_capture(&connection, &bridge, request)
        }),
    };

    if ok { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE }
}

/// `spec/08` §9's `api-enabled`, checked here as well as in the app.
///
/// Not redundant: an action activation has **no reply**, so a refusal inside the app can
/// only reach the journal. Reading the setting here is what turns "the command exits 0
/// and nothing happens" into a message that names the setting and how to change it. The
/// app keeps its own check, because the CLI is not the only caller of that action.
///
/// Defaults to true on a missing schema, matching the app: a development build with no
/// compiled schema must not refuse every capture with no way to find out why.
fn api_enabled() -> bool {
    const SCHEMA_ID: &str = "io.github.odrakirmusic.OctoSnap";
    // Looked up rather than opened directly: `Settings::new` aborts the process on an
    // unknown schema, and this binary is the one most likely to run before install.
    let Some(source) = gio::SettingsSchemaSource::default() else { return true };
    if source.lookup(SCHEMA_ID, true).is_none() {
        return true;
    }
    gio::Settings::new(SCHEMA_ID).boolean("api-enabled")
}

/// Runs a command only while `spec/08` §9's `api-enabled` allows it, with the message
/// that names the setting otherwise. The commands that open a window the user can see
/// -- settings, history -- are not gated: they are how the setting is turned back on.
fn gated(run: impl FnOnce() -> bool) -> bool {
    if api_enabled() {
        return run();
    }
    eprintln!(
        "octosnap: the command line and octosnap:// links are disabled. \
         Turn them back on in Settings > Advanced, or with:\n  \
         gsettings set io.github.odrakirmusic.OctoSnap api-enabled true"
    );
    false
}

/// Activates an action that takes a file, with the path made absolute here: the app is
/// another process with another working directory, so `shot.png` means nothing to it.
fn with_file(connection: &gio::DBusConnection, action: &str, path: &std::path::Path) -> bool {
    gated(|| {
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if !absolute.is_file() {
            eprintln!("octosnap: {}: no such file", absolute.display());
            return false;
        }
        activate(connection, action, Some(glib::Variant::from(absolute.to_string_lossy().as_ref())))
    })
}

/// `spec/07` §2.1's file read, as the `read-text` action's `a{sv}`.
///
/// Absolute here for the same reason [`with_file`] makes its path absolute: the app is a
/// different process with a different working directory, and a relative path means the
/// directory of whoever typed it.
fn read_text(
    connection: &gio::DBusConnection,
    path: &std::path::Path,
    linebreaks: Option<bool>,
) -> bool {
    gated(|| {
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if !absolute.is_file() {
            eprintln!("octosnap: {}: no such file", absolute.display());
            return false;
        }
        let options = glib::VariantDict::new(None);
        options.insert("path", absolute.to_string_lossy().as_ref());
        if let Some(keep) = linebreaks {
            options.insert("linebreaks", keep);
        }
        activate(connection, "read-text", Some(options.end()))
    })
}

/// Activates a GApplication action, starting the app if it is not running.
///
/// This is `org.freedesktop.Application.ActivateAction`, not the app's own interface, and
/// the difference is the whole point: `GApplication` exports the freedesktop interface
/// during registration, before it acquires the bus name, so a call that *causes* the
/// activation is queued and delivered. A call to `App1` in the same situation is answered
/// with "no such object" from GDBus's worker thread before the app's main loop has run
/// (`docs/spikes/13-cold-activation-race.md`), which is why `Ping` below needs a retry
/// loop and this does not.
///
/// Fire-and-forget by design: an action activation has no reply, so a failure the app
/// hits *after* accepting the call cannot be reported here. `spec/10` §8 puts that
/// reporting in the journal and, for anything the user needs to know, in a notification.
fn activate(
    connection: &gio::DBusConnection,
    action: &str,
    parameter: Option<glib::Variant>,
) -> bool {
    // ActivateAction takes the parameter as an *array* of variants, zero or one long,
    // which is how GApplication spells "this action may take no parameter".
    let parameters: Vec<glib::Variant> = parameter.into_iter().collect();
    let platform_data: std::collections::HashMap<String, glib::Variant> =
        std::collections::HashMap::new();

    let args = glib::Variant::tuple_from_iter([
        action.to_variant(),
        glib::Variant::array_from_iter_with_type(
            glib::VariantTy::VARIANT,
            parameters.iter().map(|p| p.to_variant()),
        ),
        platform_data.to_variant(),
    ]);

    match connection.call_sync(
        Some(APP_BUS_NAME),
        APP_GAPPLICATION_PATH,
        FDO_APPLICATION_INTERFACE,
        "ActivateAction",
        Some(&args),
        None,
        gio::DBusCallFlags::NONE,
        TIMEOUT_MS,
        gio::Cancellable::NONE,
    ) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("octosnap: could not reach OctoSnap: {}", e.message());
            false
        }
    }
}

/// Opens OctoSnap: `org.freedesktop.Application.Activate`, which the app answers with
/// its welcome window or Settings (`crates/app/src/setup.rs`).
///
/// The launcher's activation token rides along, so the window that opens is allowed to
/// take the focus: a Wayland compositor only gives focus to a window whose activation
/// came from something the user did, and the token is the proof. The wait is the bus's
/// own rather than [`TIMEOUT_MS`]: this call is usually the one that starts the app, and a
/// cold start -- a Flatpak's especially -- takes longer than two seconds.
fn launch(connection: &gio::DBusConnection) -> bool {
    let mut platform_data: std::collections::HashMap<String, glib::Variant> =
        std::collections::HashMap::new();
    if let Ok(token) = std::env::var("XDG_ACTIVATION_TOKEN") {
        platform_data.insert("activation-token".to_owned(), token.to_variant());
    }
    if let Ok(id) = std::env::var("DESKTOP_STARTUP_ID") {
        platform_data.insert("desktop-startup-id".to_owned(), id.to_variant());
    }
    let args = glib::Variant::tuple_from_iter([platform_data.to_variant()]);
    match connection.call_sync(
        Some(APP_BUS_NAME),
        APP_GAPPLICATION_PATH,
        FDO_APPLICATION_INTERFACE,
        "Activate",
        Some(&args),
        None,
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
    ) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("octosnap: could not open OctoSnap: {}", e.message());
            false
        }
    }
}

/// `octosnap://record-gif`: asks the running app to record a rectangle as a GIF.
///
/// The app owns the recording (`crate::recording`), so this activates its `record` action
/// with the same `a{sv}` the extension sends -- routed through the app rather than the
/// extension for the reasons `capture` is (degraded mode, `api-enabled`, one path for the
/// CLI and the URL scheme). A missing rect means "the whole current monitor", resolved over
/// the bus because only the extension knows the layout.
async fn record_gif(
    connection: &gio::DBusConnection,
    bridge: &GnomeExtensionBridge,
    rect: Option<Rect>,
) -> bool {
    let rect = match rect {
        Some(rect) => rect,
        None => match current_monitor_rect(bridge).await {
            Ok(rect) => rect,
            Err(message) => {
                eprintln!("octosnap: {message}");
                return false;
            }
        },
    };
    activate(connection, "record", Some(record_request_variant(rect)))
}

/// The rectangle of the monitor the pointer is on, for a `record-gif` with no rect given.
/// Falls back to the primary, then the first, so it always has an answer when there is any
/// monitor at all.
async fn current_monitor_rect(bridge: &GnomeExtensionBridge) -> Result<Rect, String> {
    let monitors =
        bridge.monitors().await.map_err(|e| format!("cannot read the monitors: {e}"))?;
    monitors
        .iter()
        .find(|m| m.current)
        .or_else(|| monitors.iter().find(|m| m.primary))
        .or_else(|| monitors.first())
        .map(|m| m.geometry)
        .ok_or_else(|| "no monitors reported; is the extension enabled?".to_owned())
}

/// `octosnap://scrolling-capture`: asks the running app for `spec/07` §1's capture.
///
/// With a rect this is the app's `scroll-capture` action and nothing else happens here.
/// *Without* one there is nobody to ask but the user, and the thing that asks is the
/// extension's overlay -- so this goes through the extension's `BeginCapture`, which is
/// the one path in the API that can put an overlay up.
fn scrolling_capture(
    connection: &gio::DBusConnection,
    bridge: &GnomeExtensionBridge,
    request: url::ScrollRequest,
) -> bool {
    let Some(rect) = request.rect else {
        let mut capture = CaptureRequest::new(CaptureMode::Scrolling);
        capture.start = Some(request.start).filter(|start| *start);
        return match glib::MainContext::default().block_on(bridge.begin_capture(&capture)) {
            Ok(handle) => {
                eprintln!("octosnap: scrolling capture {handle}");
                true
            }
            Err(e) => {
                eprintln!("octosnap: could not start a scrolling capture: {e}");
                false
            }
        };
    };
    activate(connection, "scroll-capture", Some(scroll_request_variant(rect, request)))
}

/// The `scroll-capture` action's payload, the same `a{sv}` the extension's
/// `startScrollCapture` sends. Absent options are left out rather than defaulted, so a
/// URL that says nothing about the direction gets the user's setting.
fn scroll_request_variant(rect: Rect, request: url::ScrollRequest) -> glib::Variant {
    let mut dict: std::collections::HashMap<String, glib::Variant> =
        std::collections::HashMap::new();
    dict.insert("rect".to_owned(), (rect.x, rect.y, rect.width, rect.height).to_variant());
    if let Some(direction) = request.direction {
        dict.insert("direction".to_owned(), direction.as_wire().to_variant());
    }
    if request.start {
        dict.insert("start".to_owned(), true.to_variant());
    }
    dict.to_variant()
}

/// The `record` action's payload: the same `a{sv}` shape the extension's `notifyRecord`
/// sends, so the app's one `RecordRequest::from_variant` reads both. `display` and `cursor`
/// are left out, so the app resolves the monitor from the rect and uses its `rec-cursor`.
fn record_request_variant(rect: Rect) -> glib::Variant {
    let mut dict: std::collections::HashMap<String, glib::Variant> =
        std::collections::HashMap::new();
    dict.insert("rect".to_owned(), (rect.x, rect.y, rect.width, rect.height).to_variant());
    dict.insert("record-format".to_owned(), "gif".to_variant());
    dict.to_variant()
}

/// `start` is whether the probe may D-Bus-activate the app. `ping` does -- it means "wake
/// it and check the protocol" -- while `status` reports what is there and starts nothing.
fn print_ping(connection: &gio::DBusConnection, start: bool) -> bool {
    let reply_type = match glib::VariantTy::new("(u)") {
        Ok(t) => t,
        Err(e) => {
            println!("app:        internal error ({e})");
            return false;
        }
    };

    let mut attempt = 0;
    loop {
        let result = connection.call_sync(
            Some(APP_BUS_NAME),
            APP_OBJECT_PATH,
            APP_INTERFACE,
            "Ping",
            None,
            Some(reply_type),
            if start { gio::DBusCallFlags::NONE } else { gio::DBusCallFlags::NO_AUTO_START },
            TIMEOUT_MS,
            gio::Cancellable::NONE,
        );

        match result {
            Ok(reply) => {
                let protocol = reply.child_value(0).get::<u32>().unwrap_or_default();
                let note = if protocol == PROTOCOL_VERSION {
                    String::new()
                } else {
                    format!(" (this build speaks {PROTOCOL_VERSION} -- MISMATCH)")
                };
                println!("app:        running, protocol {protocol}{note}");
                return true;
            }
            Err(e) if attempt < ACTIVATION_RETRIES && lost_activation_race(&e) => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(ACTIVATION_RETRY_DELAY_MS));
            }
            Err(e) if !start && not_running(&e) => {
                // A true answer to "status", not a failure: activation starts the app
                // the moment anything asks for it.
                println!("app:        not running (D-Bus activation starts it on demand)");
                return true;
            }
            Err(e) => {
                println!("app:        unavailable ({})", e.message());
                return false;
            }
        }
    }
}

/// What the bus answers when the name has no owner and the call said not to start one.
fn not_running(e: &glib::Error) -> bool {
    matches!(
        e.kind::<gio::DBusError>(),
        Some(gio::DBusError::NameHasNoOwner | gio::DBusError::ServiceUnknown)
    )
}

/// Distinguishes "the object is not exported yet" from "there is no such service".
///
/// Only the former is worth retrying. A missing extension reports `ServiceUnknown` and
/// will never appear on its own, so retrying that would just add a second of latency to
/// the common degraded-mode case.
fn lost_activation_race(e: &glib::Error) -> bool {
    let message = e.message();
    message.contains("Object does not exist at path")
        || message.contains("UnknownObject")
        || message.contains("UnknownInterface")
}

async fn print_shell_version(bridge: &GnomeExtensionBridge) -> bool {
    match bridge.version().await {
        Ok(shell) => {
            let note = if shell.protocol == PROTOCOL_VERSION {
                String::new()
            } else {
                format!(" (this build speaks {PROTOCOL_VERSION} -- MISMATCH)")
            };
            println!("extension:  {}, protocol {}{note}", shell.version, shell.protocol);
            true
        }
        Err(e) if e.is_extension_missing() => {
            println!("extension:  not enabled");
            false
        }
        Err(e) => {
            println!("extension:  error ({e})");
            false
        }
    }
}

/// The recorder's engine, run here for `--seconds`: how M5 drives it before the app has
/// a recording flow, and how `gif-test.sh` measures it at every scale.
async fn record(
    connection: &gio::DBusConnection,
    bridge: &GnomeExtensionBridge,
    args: RecordArgs,
) -> bool {
    use octosnap_media::gif::{GifPlan, GifSettings, StreamSource, expected_frames};
    use octosnap_media::recorder::{Options, Recording, Targeting};
    use octosnap_media::text::{badge, size_label};

    if !args.seconds.is_finite() || args.seconds <= 0.0 {
        eprintln!("octosnap: --seconds must be a positive number");
        return false;
    }
    let rect = match args.rect.as_deref().map(parse_rect) {
        Some(Ok(rect)) => Some(rect),
        Some(Err(e)) => {
            eprintln!("octosnap: --rect: {e}");
            return false;
        }
        None => None,
    };
    let monitors = match bridge.monitors().await {
        Ok(monitors) => monitors,
        Err(e) => {
            eprintln!("octosnap: the shell extension did not report the monitors: {e}");
            return false;
        }
    };
    let Some(monitor) = pick_monitor(&monitors, rect, args.display.as_deref()) else {
        eprintln!("octosnap: no monitor holds that rectangle");
        return false;
    };
    if monitor.connector.is_empty() {
        eprintln!("octosnap: Mutter reports no connector for that monitor, so it cannot be recorded");
        return false;
    }
    let selection = rect.unwrap_or(monitor.geometry);
    let settings = GifSettings {
        fps: args.fps,
        max_width: args.max_width,
        quality: args.quality,
        cursor: !args.no_cursor,
    };
    let source = StreamSource {
        connector: monitor.connector.clone(),
        rect: monitor.geometry,
        scale: monitor.scale,
        refresh: monitor.refresh,
    };
    let plan = match GifPlan::new(selection, &source, settings) {
        Ok(plan) => plan,
        Err(e) => {
            eprintln!("octosnap: {e}");
            return false;
        }
    };
    let output = args.output.unwrap_or_else(|| {
        let stamp = glib::DateTime::now_local()
            .and_then(|now| now.format("%Y%m%d-%H%M%S"))
            .map_or_else(|_| "now".to_owned(), |s| s.to_string());
        std::path::PathBuf::from(format!("octosnap-{stamp}.gif"))
    });
    println!(
        "plan:       {connector} {sw}x{sh} physical at scale {scale}; crop {l}/{t}/{r}/{b} \
         -> {rw}x{rh} -> {ow}x{oh}; {fps} fps, keepalive {keep} ms, quality {q}, cursor {cursor}",
        connector = plan.connector,
        sw = plan.stream.width,
        sh = plan.stream.height,
        scale = monitor.scale,
        l = plan.crop.left,
        t = plan.crop.top,
        r = plan.crop.right,
        b = plan.crop.bottom,
        rw = plan.region.width,
        rh = plan.region.height,
        ow = plan.output.width,
        oh = plan.output.height,
        fps = plan.fps,
        keep = plan.keepalive_ms,
        q = plan.quality,
        cursor = if settings.cursor { "shown" } else { "hidden" },
    );

    let options = Options {
        targeting: match args.target {
            TargetArg::NodeId => Targeting::NodeId,
            TargetArg::Serial => Targeting::Serial,
        },
        mark_recording: args.mark_recording,
    };
    let duration = std::time::Duration::from_secs_f64(args.seconds);
    // The frames are kept in a reel beside the GIF, which is written from them once the
    // recording has stopped (D113).
    let reel = octosnap_media::reel::beside(&output);
    let recording = match Recording::start(connection, &plan, &reel, options).await {
        Ok(recording) => recording,
        Err(e) => {
            eprintln!("octosnap: could not start recording: {e}");
            return false;
        }
    };
    println!(
        "recording:  node {node} via {target:?}, {seconds} s, to {path}",
        node = recording.node,
        target = recording.target,
        seconds = args.seconds,
        path = output.display(),
    );
    // Every phase of the start, because "the recorder starts slowly" is otherwise one
    // number with five suspects behind it. `first frame` is the compositor's negotiation;
    // everything before it is ours.
    let (phases, _) = recording.timings();
    println!(
        "started:    stream {} ms, node {} ms, graph {} ms, reel {} ms = {} ms",
        phases.stream.as_millis(),
        phases.node.as_millis(),
        phases.graph.as_millis(),
        phases.reel.as_millis(),
        phases.total().as_millis(),
    );
    let wall = std::time::Instant::now();
    let kept = match recording.record_for(duration).await {
        Ok(kept) => kept,
        Err(e) => {
            eprintln!("octosnap: recording failed: {e}");
            return false;
        }
    };
    let asked_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    println!(
        "kept:       {taken} frames ({expected} expected), {kept} of them different, {size} of \
         frames in {wall:.1} s wall: {path}",
        taken = kept.taken,
        expected = expected_frames(asked_ms, plan.fps),
        kept = kept.kept,
        size = size_label(kept.bytes),
        wall = wall.elapsed().as_secs_f64(),
        path = kept.dir.display(),
    );
    let started = std::time::Instant::now();
    let options = octosnap_media::gif_edit::TrimOptions {
        quality: plan.quality,
        fps: None,
        scale_percent: None,
    };
    let frames = usize::try_from(kept.kept).unwrap_or(usize::MAX);
    match octosnap_media::gif_edit::trim(&kept.dir, 0..frames, options, &output) {
        Ok(written) => {
            let duration_ms = u64::try_from(written.duration.as_millis()).unwrap_or(u64::MAX);
            println!(
                "recorded:   {frames} frames, {badge}, {size}, written in {secs:.1} s: {path}",
                frames = written.frames,
                badge = badge(duration_ms, written.bytes),
                size = size_label(written.bytes),
                secs = started.elapsed().as_secs_f64(),
                path = written.path.display(),
            );
            octosnap_media::reel::remove(&kept.dir);
            true
        }
        Err(e) => {
            eprintln!("octosnap: the GIF could not be written from {}: {e}", kept.dir.display());
            false
        }
    }
}

/// The monitor a recording is of: the one named, else the one the rectangle mostly
/// lies on, else the one under the pointer.
fn pick_monitor<'a>(
    monitors: &'a [Monitor],
    rect: Option<octosnap_core::Rect>,
    connector: Option<&str>,
) -> Option<&'a Monitor> {
    if let Some(name) = connector {
        return monitors.iter().find(|m| m.connector == name);
    }
    if let Some(rect) = rect {
        return monitors
            .iter()
            .max_by_key(|m| m.geometry.overlap_area(rect))
            .filter(|m| m.geometry.overlap_area(rect) > 0);
    }
    monitors.iter().find(|m| m.current).or_else(|| monitors.iter().find(|m| m.primary)).or(monitors.first())
}

async fn print_monitors(bridge: &GnomeExtensionBridge) -> bool {
    let monitors = match bridge.monitors().await {
        Ok(m) => m,
        Err(e) => {
            println!("monitors:   unavailable ({e})");
            return false;
        }
    };

    if monitors.is_empty() {
        println!("monitors:   none reported");
        return false;
    }

    for m in &monitors {
        let (pw, ph) = m.geometry.to_physical(m.scale);
        println!(
            "monitor {i}:  {connector:<8} logical {lw}x{lh}+{lx}+{ly}  scale {scale} \
             (geometry {gs})  physical {pw}x{ph}{flags}",
            i = m.index,
            connector = if m.connector.is_empty() { "?" } else { &m.connector },
            lw = m.geometry.width,
            lh = m.geometry.height,
            lx = m.geometry.x,
            ly = m.geometry.y,
            scale = m.scale,
            gs = m.geometry_scale,
            flags = format_flags(m),
        );
    }
    true
}

fn format_flags(m: &Monitor) -> String {
    let mut flags = Vec::new();
    if m.primary {
        flags.push("primary");
    }
    if m.current {
        flags.push("current");
    }
    if m.is_fractionally_scaled() {
        flags.push("fractional");
    }
    if flags.is_empty() { String::new() } else { format!("  [{}]", flags.join(", ")) }
}
