// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading `spec/08`'s settings into the types the flow uses.
//!
//! Lives in the app rather than in `crates/core` where `spec/10` §5 puts the "settings
//! wrapper". `core` is deliberately free of GTK and glib so the geometry, the capture
//! model and the action rules can be tested without a display or a bus, and a
//! `gio::Settings` wrapper would end that. `core` keeps defining the shapes; this reads
//! them.
//!
//! The guard in [`Settings::load`] is not defensive programming for its own sake:
//! `gio::Settings::new` on a schema that is not installed **aborts the process**. A
//! development build whose schema has not been compiled into a search path would take
//! the whole service down on startup rather than run with defaults.

use std::path::PathBuf;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use octosnap_core::actions::{AfterAction, CopyUploadBehavior, Policy};

// Recordings share `copy-upload-behavior` with screenshots rather than having their own
// key (`recording_policy`). `ACT-04`'s shutter is not here at all: it lives in the
// extension's schema, because it has to sound on the compositor at the instant of the
// grab (`docs/decisions.md` D11).
use octosnap_core::qao::{self, AutoClose, Edge};
use octosnap_core::savepath::ImageFormat;
use tracing::{debug, info, warn};

use crate::flow::SaveConfig;
use crate::qao::QaoConfig;

pub const SCHEMA_ID: &str = "io.github.odrakirmusic.OctoSnap";

/// Every app key this build reads. Checked once at load, because `gio::Settings::boolean`
/// and friends call `g_error` -- **aborting the process** -- on a key the schema does not
/// have. A stale compiled schema is the normal state of a development machine between a
/// `cargo build` and the next `install-dev.sh`, and it used to take the service down.
const REQUIRED_KEYS: &[&str] = &[
    "after-screenshot",
    "area-shortcuts-respect-actions",
    "copy-upload-behavior",
    "screenshot-folder",
    "shot-format",
    "shot-jpg-quality",
    "filename-template",
    "filename-counter-start",
    "filename-counter-width",
    "filename-utc",
    "retina-suffix",
    "api-enabled",
    "my-colors",
    "annotate-crop-snap",
    "ann-background-presets",
    "ann-background-last",
    "ann-background-default",
    "ann-background-auto",
    "ann-background-remember",
    "ann-background-was-open",
    "window-background",
    "window-background-image",
    "window-background-color",
    "window-padding",
    "window-shadow",
    "window-rounded-corners",
    "notifications",
    "qao-edge",
    "qao-follow-pointer",
    "qao-size",
    "qao-auto-close-seconds",
    "qao-close-after-drag",
    "qao-ask-destination",
    "qao-shortcuts",
    "history-retention-days",
    "history-keep-saved",
    "ann-export-scale",
    "debug",
    "pin-rounded-corners",
    "pin-border",
    // REC (M5)
    "after-recording",
    "recording-folder",
    "gif-fps",
    "gif-max-width",
    "gif-quality",
    "rec-cursor",
    // OCR (M6)
    "ocr-language",
    "ocr-line-breaks",
    "ocr-detect-links",
    // First run (M7)
    "welcomed",
    // D126
    "launch-at-login",
    // D133
    "stale-extension",
    // D160
    "extension-installed",
];

/// GNOME Shell's keybindings (`org.gnome.shell.keybindings`), which `spec/08` §3's
/// Print-key takeover writes. In a sandbox the schema is not there and dconf is not the
/// shell's, so it is the extension's copy of its three Print keys, mirrored (D123).
#[must_use]
pub fn shell_keybindings() -> Option<gio::Settings> {
    if sandboxed() {
        return crate::remote_settings::open(octosnap_core::shortcuts::GNOME_SHELL_KEYBINDINGS);
    }
    open_schema(octosnap_core::shortcuts::GNOME_SHELL_KEYBINDINGS)
}

/// Whether this process runs inside a Flatpak sandbox, where the extension's files, the
/// host's dconf and the host's autostart folder are all out of reach (`spec/10` §10).
#[must_use]
pub fn sandboxed() -> bool {
    std::path::Path::new("/.flatpak-info").exists()
}

/// The extension's schema. The app reads and writes it from one place only -- the
/// Preferences dialog -- so that the user sees a single settings surface even though two
/// processes own the keys (`docs/decisions.md` D11).
pub const EXTENSION_SCHEMA_ID: &str = "org.gnome.shell.extensions.octosnap";

/// The extension's UUID, which is also the name of its directory.
pub const EXTENSION_UUID: &str = "octosnap@odrakirmusic.github.io";

/// Opens a schema without aborting when it is not installed.
///
/// `gio::Settings::new` calls `g_error` on an unknown schema, which kills the process, so
/// every path into GSettings in this app goes through a lookup first.
#[must_use]
pub fn open_schema(id: &str) -> Option<gio::Settings> {
    let source = gio::SettingsSchemaSource::default()?;
    source.lookup(id, true)?;
    Some(gio::Settings::new(id))
}

/// The extension's settings, when its schema can be found.
///
/// A GNOME extension's schema is compiled into its own directory, which is **not** a
/// GSettings search path -- `~/.local/share/gnome-shell/extensions/<uuid>/schemas` is not
/// under `~/.local/share/glib-2.0/schemas`. So the default source does not know about it
/// and `Settings::new` would abort. GNOME's own extension preferences solve this by
/// building a `SettingsSchemaSource` over the extension directory, and that is what this
/// does; the alternative, installing a second copy of the schema into the app's own
/// search path, would leave two schemas that can disagree about their defaults.
#[must_use]
pub fn extension_settings() -> Option<gio::Settings> {
    // In a sandbox the extension's keys live in GNOME Shell's dconf, which is not this
    // process's, so the settings are the extension's own, mirrored (D123).
    if sandboxed() {
        return crate::remote_settings::open(EXTENSION_SCHEMA_ID);
    }
    // Installed in a search path already (a packaged build, or a distro that puts
    // extension schemas in the system directory): nothing more to do.
    if let Some(settings) = open_schema(EXTENSION_SCHEMA_ID) {
        return Some(settings);
    }

    for dir in extension_schema_dirs() {
        // `trusted = true` is right for a directory the user installed an extension
        // into: the alternative is to reject a schema whose translations are not
        // byte-perfect, which has nothing to do with whether the keys are usable.
        let source = match gio::SettingsSchemaSource::from_directory(
            &dir,
            gio::SettingsSchemaSource::default().as_ref(),
            true,
        ) {
            Ok(source) => source,
            // A directory that does not exist is the normal case for most of the
            // candidates, so this is not worth a warning.
            Err(_) => continue,
        };
        let Some(schema) = source.lookup(EXTENSION_SCHEMA_ID, true) else {
            continue;
        };
        info!(dir = %dir.display(), "found the extension's schema");
        // `Settings::new_full` rather than `new`, because the schema came from a source
        // the default one has never heard of.
        return Some(gio::Settings::new_full(&schema, None::<&gio::SettingsBackend>, None));
    }

    warn!(
        schema = EXTENSION_SCHEMA_ID,
        "the extension's schema was not found; its settings cannot be edited"
    );
    None
}

/// Where a GNOME extension's compiled schema can be, most-specific first.
fn extension_schema_dirs() -> Vec<PathBuf> {
    let mut roots = vec![glib::user_data_dir()];
    roots.extend(glib::system_data_dirs());
    roots
        .into_iter()
        .map(|root| root.join("gnome-shell/extensions").join(EXTENSION_UUID).join("schemas"))
        .collect()
}

#[derive(Debug, Clone)]
pub struct Settings {
    inner: Option<gio::Settings>,
}

impl Settings {
    /// Loads the schema, or falls back to `spec/08`'s documented defaults.
    #[must_use]
    pub fn load() -> Self {
        let Some(source) = gio::SettingsSchemaSource::default() else {
            warn!("no GSettings schema source; using built-in defaults");
            return Self { inner: None };
        };

        // `lookup` rather than `Settings::new`, because the latter aborts on a missing
        // schema instead of returning an error.
        let Some(schema) = source.lookup(SCHEMA_ID, true) else {
            warn!(
                schema = SCHEMA_ID,
                "schema is not installed; using built-in defaults. \
                 Install it with scripts/install-dev.sh"
            );
            return Self { inner: None };
        };

        // An installed-but-stale schema is worse than a missing one, because every
        // accessor below would abort on the first key it added. Fall back wholesale
        // rather than per key: a schema this build does not recognise cannot be trusted
        // to hold the user's real preferences either.
        let missing: Vec<&str> =
            REQUIRED_KEYS.iter().copied().filter(|key| !schema.has_key(key)).collect();
        if !missing.is_empty() {
            warn!(
                schema = SCHEMA_ID,
                ?missing,
                "the installed schema is older than this build; using built-in defaults. \
                 Reinstall it with scripts/install-dev.sh"
            );
            return Self { inner: None };
        }

        info!(schema = SCHEMA_ID, "settings loaded");
        Self { inner: Some(gio::Settings::new(SCHEMA_ID)) }
    }

    /// True when the real schema is in use rather than the fallback defaults.
    #[must_use]
    pub fn is_installed(&self) -> bool {
        self.inner.is_some()
    }

    /// `ACT-01`'s policy for screenshots.
    #[must_use]
    pub fn screenshot_policy(&self) -> Policy {
        let Some(s) = &self.inner else {
            return Policy::default();
        };
        Policy {
            configured: actions_from(&s.strv("after-screenshot")),
            shortcuts_respect_configured: s.boolean("area-shortcuts-respect-actions"),
            copy_upload: CopyUploadBehavior::from_wire(&s.string("copy-upload-behavior"))
                .unwrap_or_default(),
        }
    }

    /// `spec/08` §5's after-recording plan. Recordings share `copy-upload-behavior` with
    /// screenshots (there is one clipboard), but have their own action list and never the
    /// area-shortcut override, which is a screenshot idea.
    #[must_use]
    pub fn recording_policy(&self) -> Policy {
        let Some(s) = &self.inner else {
            return Policy::default();
        };
        Policy {
            configured: actions_from(&s.strv("after-recording")),
            shortcuts_respect_configured: false,
            copy_upload: CopyUploadBehavior::from_wire(&s.string("copy-upload-behavior"))
                .unwrap_or_default(),
        }
    }

    /// `spec/08` §5's GIF encoder settings. The valid frame rates and quality are the
    /// schema's range; the page offers `spec/08`'s rates up to the format's ceiling, and
    /// zero, which is the recorded screen's own rate (D101).
    #[must_use]
    pub fn gif_settings(&self) -> octosnap_media::gif::GifSettings {
        use octosnap_media::gif::{GIF_FPS_MAX, GifSettings};
        let Some(s) = &self.inner else {
            return GifSettings::default();
        };
        GifSettings {
            fps: u32::try_from(s.int("gif-fps")).unwrap_or(15).min(GIF_FPS_MAX),
            max_width: u32::try_from(s.int("gif-max-width").clamp(0, 7680)).unwrap_or(800),
            quality: u8::try_from(s.int("gif-quality").clamp(1, 100)).unwrap_or(80),
            cursor: s.boolean("rec-cursor"),
        }
    }

    /// `spec/08` §4's Quick Access tab.
    #[must_use]
    pub fn qao_config(&self) -> QaoConfig {
        let Some(s) = &self.inner else {
            return QaoConfig::default();
        };
        QaoConfig {
            edge: Edge::from_wire(&s.string("qao-edge")).unwrap_or_default(),
            follow_pointer: s.boolean("qao-follow-pointer"),
            // Clamped rather than trusted: the schema's range keeps the settings UI
            // honest, but `gsettings set` bypasses nothing and `longest_side` would have
            // to clamp anyway.
            size_step: u8::try_from(s.int("qao-size").clamp(1, i32::from(qao::SIZE_STEPS)))
                .unwrap_or(qao::DEFAULT_SIZE_STEP),
            // `exact`, not `snapped`: this is a round trip through the settings, and
            // snapping here would rewrite a value the user set deliberately. Snapping
            // belongs in the UI that offers the menu.
            auto_close: AutoClose::exact(
                u32::try_from(s.int("qao-auto-close-seconds")).unwrap_or(0),
            ),
            close_after_drag: s.boolean("qao-close-after-drag"),
            shortcuts: s.boolean("qao-shortcuts"),
            ask_destination: s.boolean("qao-ask-destination"),
        }
    }

    /// `ACT-06` and `ACT-07`.
    #[must_use]
    pub fn save_config(&self) -> SaveConfig {
        let Some(s) = &self.inner else {
            return SaveConfig::default();
        };
        let folder = s.string("screenshot-folder");
        let recording_folder = s.string("recording-folder");
        SaveConfig {
            folder: if folder.is_empty() { None } else { Some(PathBuf::from(folder.as_str())) },
            recording_folder: if recording_folder.is_empty() {
                None
            } else {
                Some(PathBuf::from(recording_folder.as_str()))
            },
            format: ImageFormat::from_wire(&s.string("shot-format")).unwrap_or_default(),
            jpeg_quality: s.int("shot-jpg-quality").clamp(1, 100) as u8,
            template: s.string("filename-template").to_string(),
            // The schema clamps these, but a hand-edited dconf value does not go through
            // the schema's range, so they are clamped again here.
            counter_start: s.int("filename-counter-start").clamp(0, 1_000_000) as u32,
            counter_width: s.int("filename-counter-width").clamp(1, 4) as u8,
            utc: s.boolean("filename-utc"),
            retina_suffix: s.boolean("retina-suffix"),

            clipboard_folder: None,
        }
    }

    /// `spec/08` §9: whether the CLI and the `octosnap://` scheme may start captures.
    ///
    /// Defaults to **true** when the schema is missing, matching the schema's own
    /// default. Failing closed would be the safer-looking choice and the wrong one: a
    /// development build with no compiled schema would silently refuse every CLI
    /// capture, and the user would have no setting to find and no message to read.
    #[must_use]
    pub fn api_enabled(&self) -> bool {
        self.inner.as_ref().is_none_or(|s| s.boolean("api-enabled"))
    }

    /// `ACT-05`.
    #[must_use]
    pub fn notifications_enabled(&self) -> bool {
        self.inner.as_ref().is_none_or(|s| s.boolean("notifications"))
    }

    /// `spec/07` §4.1's retention, from `spec/08` §9's `history-retention-days`.
    #[must_use]
    pub fn history_retention(&self) -> octosnap_core::history::Retention {
        use octosnap_core::history::Retention;
        self.inner
            .as_ref()
            .map_or(Retention::Week, |s| Retention::from_days(s.int("history-retention-days")))
    }

    /// `spec/04` §7's "unless saved and 'keep history' is off".
    #[must_use]
    pub fn history_keep_saved(&self) -> bool {
        self.inner.as_ref().is_none_or(|s| s.boolean("history-keep-saved"))
    }

    /// `spec/08` §9's debug logging.
    #[must_use]
    pub fn debug(&self) -> bool {
        self.inner.as_ref().is_some_and(|s| s.boolean("debug"))
    }

    /// Whether the welcome window has been seen through to Done (`spec/11` M7). A build
    /// on built-in defaults answers yes, so a stale schema does not greet the user at every
    /// launch with a window whose Done cannot be remembered.
    #[must_use]
    pub fn welcomed(&self) -> bool {
        self.inner.as_ref().is_none_or(|s| s.boolean("welcomed"))
    }

    /// Records that the welcome window was seen through to Done.
    pub fn set_welcomed(&self) {
        if let Some(s) = &self.inner
            && let Err(e) = s.set_boolean("welcomed", true)
        {
            warn!("could not remember the welcome window: {e}");
        }
    }

    /// D126: what the Background portal last agreed to, for the switch in a sandbox.
    #[must_use]
    pub fn launch_at_login(&self) -> bool {
        self.inner.as_ref().is_some_and(|s| s.boolean("launch-at-login"))
    }

    pub fn set_launch_at_login(&self, on: bool) {
        if let Some(s) = &self.inner
            && let Err(e) = s.set_boolean("launch-at-login", on)
        {
            warn!("could not remember launching at login: {e}");
        }
    }

    /// D133: the out-of-date extension last found and the login it was found in, as
    /// `version|protocol|login`, or empty.
    #[must_use]
    pub fn stale_extension(&self) -> String {
        self.inner.as_ref().map(|s| s.string("stale-extension").to_string()).unwrap_or_default()
    }

    pub fn set_stale_extension(&self, value: &str) {
        if let Some(s) = &self.inner
            && s.string("stale-extension") != value
            && let Err(e) = s.set_string("stale-extension", value)
        {
            warn!("could not remember the out-of-date extension: {e}");
        }
    }

    /// D160: the carried extension this app copied into place, as `version|login`, until
    /// the first start after the logout has turned it on; empty otherwise. The login is
    /// `setup`'s, and empty where the bus would not say.
    #[must_use]
    pub fn extension_installed(&self) -> String {
        self.inner.as_ref().map(|s| s.string("extension-installed").to_string()).unwrap_or_default()
    }

    pub fn set_extension_installed(&self, record: &str) {
        if let Some(s) = &self.inner
            && s.string("extension-installed") != record
            && let Err(e) = s.set_string("extension-installed", record)
        {
            warn!("could not remember the installed extension: {e}");
        }
    }

    /// Calls `on_change` whenever any key changes, so a running service picks up the
    /// Preferences dialog without a restart.
    pub fn connect_changed<F: Fn() + 'static>(&self, on_change: F) {
        if let Some(s) = &self.inner {
            s.connect_changed(None, move |_, key| {
                info!(key, "setting changed");
                on_change();
            });
        }
    }

    /// Like [`Settings::connect_changed`], for a change to one of `keys` only.
    pub fn connect_keys_changed<F: Fn() + 'static>(&self, keys: &'static [&'static str], on_change: F) {
        if let Some(s) = &self.inner {
            s.connect_changed(None, move |_, key| {
                if keys.contains(&key) {
                    on_change();
                }
            });
        }
    }

    /// D125: `spec/08` §5's four recording-row values as the `a{sv}` the extension's
    /// `app-gif-defaults` keeps: the schema's own values rather than
    /// [`Settings::gif_settings`]'s clamped ones, in a fixed order so that an unchanged
    /// copy compares equal.
    fn gif_defaults_copy(&self) -> Option<glib::Variant> {
        let s = self.inner.as_ref()?;
        let entries = [
            ("fps", s.int("gif-fps").to_variant()),
            ("max-width", s.int("gif-max-width").to_variant()),
            ("quality", s.int("gif-quality").to_variant()),
            ("cursor", s.boolean("rec-cursor").to_variant()),
        ]
        .map(|(key, value)| glib::variant::DictEntry::new(key.to_owned(), value));
        Some(entries.to_variant())
    }
}

/// The app keys [`sync_gif_defaults`] copies.
pub const GIF_DEFAULT_KEYS: &[&str] = &["gif-fps", "gif-max-width", "gif-quality", "rec-cursor"];

/// D125: copies the recording row's four values into the extension's `app-gif-defaults`,
/// so the All-In-One toolbar shows what a recording will be made with even where the
/// extension cannot read this schema -- which, from a Flatpak, is always. Written only
/// when the copy differs; a missing extension, or one whose schema has no such key, is
/// left alone.
pub fn sync_gif_defaults(config: &Settings) {
    const COPY: &str = "app-gif-defaults";
    let Some(value) = config.gif_defaults_copy() else { return };
    let Some(ext) = extension_settings() else { return };
    if !ext.settings_schema().is_some_and(|schema| schema.has_key(COPY)) {
        debug!("the extension's schema has no {COPY}; the recording row keeps its own defaults");
        return;
    }
    if ext.value(COPY) == value {
        return;
    }
    match ext.set_value(COPY, &value) {
        Ok(()) => info!(copy = %value.print(false), "copied the GIF defaults to the extension"),
        Err(e) => warn!("could not copy the GIF defaults to the extension: {e}"),
    }
}

/// Parses the action list, skipping anything this build does not recognise.
///
/// A value written by a newer build must not empty the list -- that would silently turn
/// every capture into a no-op.
fn actions_from(values: &gtk::glib::StrV) -> Vec<AfterAction> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        let text = value.as_str();
        match AfterAction::from_wire(text) {
            Some(action) if !out.contains(&action) => out.push(action),
            // A duplicate in dconf is harmless; the set is order-preserving and unique.
            Some(_) => {}
            None => warn!(value = text, "ignoring an unknown after-capture action"),
        }
    }
    out
}

/// `spec/07` §3.1's look for a pinned screenshot, "rounded corners (8 px) + shadow (both
/// switchable), optional 1 px border": whether the corners are rounded, and whether the
/// border is drawn. Read when a pin is made; a pin already on screen keeps its look.
#[must_use]
pub fn pin_look() -> PinLook {
    let Some(settings) = open_settings() else { return PinLook::default() };
    let has = |key: &str| settings.settings_schema().is_some_and(|s| s.has_key(key));
    let default = PinLook::default();
    let read = |key: &str, otherwise: bool| if has(key) { settings.boolean(key) } else { otherwise };
    PinLook {
        rounded: read("pin-rounded-corners", default.rounded),
        bordered: read("pin-border", default.bordered),
        shadow: read("pin-shadow", default.shadow),
    }
}

/// [`pin_look`]'s answer: the schema's defaults when there is no schema to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinLook {
    pub rounded: bool,
    pub bordered: bool,
    /// Drawn by the compositor, outside the window (D132).
    pub shadow: bool,
}

impl Default for PinLook {
    fn default() -> Self {
        Self { rounded: true, bordered: false, shadow: true }
    }
}

/// `spec/08` §1's Advanced > Text Recognition, read at the moment of a text capture.
///
/// A free function like [`pin_look`], and for the same reason: the flow does not hold
/// these, they are read once per capture, and a capture is rare enough that a GSettings
/// lookup on the way into it costs nothing worth holding state for.
#[must_use]
pub fn ocr_config() -> crate::ocr::Config {
    let default = crate::ocr::Config::default();
    let Some(settings) = open_settings() else { return default };
    let Some(schema) = settings.settings_schema() else { return default };
    if !["ocr-language", "ocr-line-breaks", "ocr-detect-links"].iter().all(|k| schema.has_key(k)) {
        return default;
    }
    crate::ocr::Config {
        // An unrecognised tag is auto rather than an error: `spec/07` §2.1's auto-detect
        // is what a build that has never heard of the script would fall back to anyway.
        script: octosnap_ocr::Script::from_tag(&settings.string("ocr-language")),
        breaks: crate::ocr::breaks(settings.boolean("ocr-line-breaks")),
        links: settings.boolean("ocr-detect-links"),
        sounds: settings.boolean("ui-sounds"),
    }
}

/// `spec/08` §7's export scale: whether the annotate editor exports at 1x rather than at
/// the capture's own density. A free function like [`my_colors`], read at the moment of
/// an export.
#[must_use]
pub fn export_at_1x() -> bool {
    let Some(settings) = open_settings() else { return false };
    settings.settings_schema().is_some_and(|s| s.has_key("ann-export-scale"))
        && settings.string("ann-export-scale") == "1x"
}

/// `spec/08` §1's "Play sounds for recording/OCR/upload", which is also the switch for
/// copying and pinning (D134). Read at the moment of the sound, like [`export_at_1x`]; on
/// when there is no schema to say otherwise, as the schema's default is.
#[must_use]
pub fn ui_sounds() -> bool {
    open_settings().is_none_or(|settings| settings.boolean("ui-sounds"))
}

/// The log filter the process started with, kept so `spec/08` §9's debug switch can
/// change it while the service runs.
struct LogControl {
    handle: tracing_subscriber::reload::Handle<tracing_subscriber::EnvFilter, tracing_subscriber::Registry>,
    /// `RUST_LOG` was set: the environment asked for a level and the setting yields to it.
    from_environment: bool,
}

static LOG_CONTROL: std::sync::OnceLock<LogControl> = std::sync::OnceLock::new();

/// Installed once by `main`, before anything reads the setting.
pub fn install_log_control(
    handle: tracing_subscriber::reload::Handle<tracing_subscriber::EnvFilter, tracing_subscriber::Registry>,
    from_environment: bool,
) {
    let _ = LOG_CONTROL.set(LogControl { handle, from_environment });
}

/// The filter debug logging turns on: this project's crates at `debug`, the rest at `info`.
pub const DEBUG_FILTER: &str = "octosnap_app=debug,octosnap_shell=debug,octosnap_scene=debug,octosnap_core=debug,info";

/// Applies `spec/08` §9's debug switch to the running process.
pub fn apply_debug_logging(on: bool) {
    let Some(control) = LOG_CONTROL.get() else { return };
    if control.from_environment {
        info!(on, "debug setting ignored: RUST_LOG is set for this process");
        return;
    }
    let filter = tracing_subscriber::EnvFilter::new(if on { DEBUG_FILTER } else { "info" });
    match control.handle.reload(filter) {
        Ok(()) => info!(on, "debug logging"),
        Err(e) => warn!("could not change the log filter: {e}"),
    }
}

/// `spec/05` §2's saved colours, as hex strings without a leading `#`.
///
/// Free functions rather than methods on [`Settings`]: the picker is opened from a popover
/// deep in the editor and reads them at that moment, where threading a loaded `Settings`
/// through would mean holding one for the life of a window to answer one question.
/// [`schema_installed`] is checked for the same reason [`Settings::load`] checks it --
/// `gio::Settings::new` on a missing schema **aborts the process**.
#[must_use]
pub fn my_colors() -> Vec<String> {
    let Some(settings) = open_settings() else { return Vec::new() };
    settings.strv("my-colors").iter().map(ToString::to_string).collect()
}

/// `spec/05` §4.13's saved background presets.
///
/// Stored as a list of JSON objects rather than as one blob, so that a preset a newer
/// build wrote -- with a field this one does not know -- costs the user that preset
/// instead of every preset. Unreadable entries are dropped on the way past.
#[must_use]
pub fn background_presets() -> Vec<octosnap_scene::Preset> {
    let Some(settings) = open_settings() else { return Vec::new() };
    settings
        .strv("ann-background-presets")
        .iter()
        .filter_map(|text| octosnap_scene::Preset::parse(text))
        .collect()
}

pub fn set_background_presets(presets: &[octosnap_scene::Preset]) {
    let Some(settings) = open_settings() else { return };
    let written: Vec<String> =
        presets.iter().filter_map(octosnap_scene::Preset::to_json).collect();
    let borrowed: Vec<&str> = written.iter().map(String::as_str).collect();
    if let Err(e) = settings.set_strv("ann-background-presets", borrowed) {
        warn!("could not save ann-background-presets: {e}");
    }
}

/// §4.13's "Apply Previous Settings": the parameters last used, not a preset id.
#[must_use]
pub fn background_last() -> Option<octosnap_scene::BackgroundParams> {
    let settings = open_settings()?;
    let text = settings.string("ann-background-last");
    serde_json::from_str::<octosnap_scene::BackgroundParams>(&text)
        .ok()
        .map(|params| params.clamped())
}

pub fn set_background_last(params: &octosnap_scene::BackgroundParams) {
    let Some(settings) = open_settings() else { return };
    let Ok(text) = serde_json::to_string(params) else { return };
    if let Err(e) = settings.set_string("ann-background-last", &text) {
        warn!("could not save ann-background-last: {e}");
    }
}

/// §4.13's "Default Preset", by id. Empty is none.
#[must_use]
pub fn background_default() -> Option<octosnap_scene::Preset> {
    let settings = open_settings()?;
    let id = settings.string("ann-background-default");
    if id.is_empty() {
        return None;
    }
    background_presets().into_iter().find(|preset| preset.id == id)
}

pub fn set_background_default(id: &str) {
    let Some(settings) = open_settings() else { return };
    if let Err(e) = settings.set_string("ann-background-default", id) {
        warn!("could not save ann-background-default: {e}");
    }
}

/// §4.13's "automatically apply preset to all screenshots".
#[must_use]
pub fn background_auto() -> bool {
    open_settings().is_some_and(|settings| settings.boolean("ann-background-auto"))
}

/// `spec/08` §1's Annotate row "Remember if background tool was opened".
///
/// Two keys, because a preference and the thing it remembers are different questions --
/// see the schema. Off means the panel opens closed, whatever the last editor did.
#[must_use]
pub fn background_panel_opens() -> bool {
    let Some(settings) = open_settings() else { return false };
    settings.boolean("ann-background-remember") && settings.boolean("ann-background-was-open")
}

/// What an editor writes as it closes, for the row above to read next time.
///
/// Written whether or not the preference is on: a user who turns it on expects it to
/// remember from then on, not from the next restart of the application.
pub fn set_background_panel_was_open(open: bool) {
    let Some(settings) = open_settings() else { return };
    if settings.boolean("ann-background-was-open") == open {
        return;
    }
    if let Err(e) = settings.set_boolean("ann-background-was-open", open) {
        warn!("could not save ann-background-was-open: {e}");
    }
}

/// `spec/08` §2's "Capture window shadow", which can only ever take one away.
///
/// The compositor has already drawn it into the capture's alpha, so `true` -- the default
/// -- is "leave the file as it is" and `false` is `background::window_body`'s trim.
#[must_use]
pub fn window_shadow() -> bool {
    let Some(settings) = open_settings() else { return true };
    !settings.settings_schema().is_some_and(|schema| schema.has_key("window-shadow"))
        || settings.boolean("window-shadow")
}

/// `spec/08` §2's Wallpaper tab, as `spec/05` §4.15's background object.
///
/// > A window capture keeps `window.png` (alpha) and the background parameters (wallpaper
/// > crop/custom/colour + padding) as an editable background object; the panel can change
/// > or remove it.
///
/// `None` for a transparent background, which is the one answer that means "no object at
/// all" -- the capture already has alpha where the compositor's shadow was, and a
/// background object with nothing in it would be a row in `objects.json` for nothing.
///
/// No corner radius and no shadow of our own, and that is not an omission: the PNG
/// `Shell.Screenshot.screenshot_window` writes already has the window's rounded corners
/// and the compositor's shadow in its alpha, so §4.13's two would be a second set over
/// the top. `window-shadow` and `window-rounded-corners` are about *removing* what is
/// already there, which is a capture-side change (`extension/src/capture.ts` says so).
#[must_use]
pub fn window_background() -> Option<octosnap_scene::BackgroundParams> {
    let settings = open_settings()?;
    if !settings.settings_schema().is_some_and(|schema| schema.has_key("window-background")) {
        return None;
    }
    let padding = f64::from(settings.int("window-padding").clamp(0, 400));
    let background = match settings.string("window-background").as_str() {
        "wallpaper" => octosnap_scene::Background::Image { file: desktop_wallpaper()? },
        "image" => {
            let file = settings.string("window-background-image").to_string();
            if file.is_empty() {
                return None;
            }
            octosnap_scene::Background::Image { file }
        }
        "color" => octosnap_scene::Background::Color {
            color: octosnap_scene::Rgba::from_hex(&settings.string("window-background-color"))?,
        },
        // "transparent", and anything a newer build wrote that this one does not know.
        _ => return None,
    };
    Some(octosnap_scene::BackgroundParams {
        background,
        padding,
        inset: 0.0,
        corner_radius: 0.0,
        ratio: None,
        alignment: octosnap_scene::background::CENTRE,
        auto_balance: false,
        shadow_intensity: 0.0,
    })
}

/// The user's desktop wallpaper, which `spec/08` §2 calls "the image used both to hide
/// desktop icons and as the background behind window screenshots".
///
/// Read from GNOME's own schema, and guarded the way every other read here is:
/// `gio::Settings::new` on a missing schema **aborts the process**, and a session without
/// `org.gnome.desktop.background` is a session this should answer `None` in.
#[must_use]
fn desktop_wallpaper() -> Option<String> {
    // In a sandbox the background setting is the sandbox's and the file is out of sight:
    // the extension reads GNOME's and copies the file where this app can (D124).
    if sandboxed() {
        return crate::remote_settings::wallpaper(adw::StyleManager::default().is_dark());
    }
    const SCHEMA: &str = "org.gnome.desktop.background";
    let source = gio::SettingsSchemaSource::default()?;
    let schema = source.lookup(SCHEMA, true)?;
    let dark = adw::StyleManager::default().is_dark();
    let key = if dark && schema.has_key("picture-uri-dark") {
        "picture-uri-dark"
    } else if schema.has_key("picture-uri") {
        "picture-uri"
    } else {
        return None;
    };
    let uri = gio::Settings::new(SCHEMA).string(key);
    if uri.is_empty() {
        return None;
    }
    // A path, because that is what the background object stores and what the canvas
    // hands to `gdk::Texture::from_filename`.
    gio::File::for_uri(&uri).path().map(|path| path.to_string_lossy().into_owned())
}

/// `spec/05` §4.11's snapping preference (`snapInAnnotateCrop` [V]).
///
/// Defaults to on without the schema, which is the same answer the schema gives -- a
/// development machine between a `cargo build` and the next `install-dev.sh` should get
/// the specified behaviour rather than the opposite of it.
#[must_use]
pub fn crop_snapping() -> bool {
    open_settings().is_none_or(|settings| settings.boolean("annotate-crop-snap"))
}

pub fn set_crop_snapping(on: bool) {
    let Some(settings) = open_settings() else { return };
    if let Err(e) = settings.set_boolean("annotate-crop-snap", on) {
        warn!("could not save annotate-crop-snap: {e}");
    }
}

/// `spec/05` §1: "Window is resizable and remembers its size." Width, height, maximised.
///
/// `None` until an editor has been closed once, or on a machine whose installed schema
/// predates the key -- checked, because reading a key a schema does not have aborts the
/// process, and a development machine between a `cargo build` and the next
/// `install-dev.sh` is exactly that machine.
#[must_use]
pub fn editor_window() -> Option<(i32, i32, bool)> {
    let settings = open_settings()?;
    if !settings.settings_schema()?.has_key("editor-window") {
        return None;
    }
    let stored: Vec<i32> = settings.value("editor-window").get()?;
    match stored[..] {
        [width, height, maximized] if width > 0 && height > 0 => {
            Some((width, height, maximized != 0))
        }
        _ => None,
    }
}

pub fn set_editor_window(width: i32, height: i32, maximized: bool) {
    let Some(settings) = open_settings() else { return };
    if !settings.settings_schema().is_some_and(|schema| schema.has_key("editor-window")) {
        return;
    }
    let value = [width, height, i32::from(maximized)].to_variant();
    if let Err(e) = settings.set_value("editor-window", &value) {
        warn!("could not save editor-window: {e}");
    }
}

pub fn set_my_colors(colors: &[String]) {
    let Some(settings) = open_settings() else { return };
    let refs: Vec<&str> = colors.iter().map(String::as_str).collect();
    if let Err(e) = settings.set_strv("my-colors", refs) {
        warn!("could not save my-colors: {e}");
    }
}

/// A `Settings` for the app's schema, or `None` when it is missing or stale.
///
/// `lookup` and not `Settings::new`, for the reason [`Settings::load`] gives: the latter
/// **aborts the process** on a schema it cannot find, and a development machine between a
/// `cargo build` and the next `install-dev.sh` is exactly that machine.
fn open_settings() -> Option<gio::Settings> {
    let source = gio::SettingsSchemaSource::default()?;
    let schema = source.lookup(SCHEMA_ID, true)?;
    if !schema.has_key("my-colors") {
        warn!("the installed schema has no my-colors; saved colours will not persist");
        return None;
    }
    Some(gio::Settings::new(SCHEMA_ID))
}
