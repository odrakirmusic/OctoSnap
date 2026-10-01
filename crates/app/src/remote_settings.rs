// SPDX-License-Identifier: GPL-3.0-or-later

//! D123: the extension's settings and GNOME's Print keys, mirrored for an app in a
//! sandbox.
//!
//! Inside the Flatpak GSettings is a key file of the app's own, which GNOME Shell never
//! reads, and neither the extension's schema nor GNOME Shell's keybinding schema is where
//! the sandbox can see it. So the Settings dialog's shell rows -- the shortcuts, the
//! capture switches, the Print-key takeover -- would change nothing. The extension answers
//! for those keys over its interface (`extension/src/settingsBridge.ts`), and this module
//! keeps a mirror of each: a `gio::Settings` on a memory backend, over a copy of the schema
//! the Flatpak ships in `share/octosnap/remote-schemas`, filled from `GetSettings`, written
//! through with `SetSetting` or `ResetSetting`, and updated from `SettingChanged`. Every
//! caller keeps the `gio::Settings` it already binds to; only where it comes from changes.
//!
//! Outside a sandbox nothing here runs: the app reads the same dconf GNOME Shell does.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use octosnap_core::protocol::{SHELL_BUS_NAME, SHELL_INTERFACE, SHELL_OBJECT_PATH};
use tracing::{debug, info, warn};

/// Where the Flatpak installs the schemas the mirrors are built on, under a data dir.
const SCHEMA_DIR: &str = "octosnap/remote-schemas";
/// A mirror is opened when Settings is, on the main loop, so the wait is short.
const TIMEOUT_MS: i32 = 1500;

struct Mirror {
    settings: gio::Settings,
    /// Set while this module writes a value it was told, so the write is not sent back.
    applying: Rc<Cell<bool>>,
    _changes: gio::SignalSubscription,
}

thread_local! {
    static MIRRORS: RefCell<HashMap<String, Mirror>> = RefCell::new(HashMap::new());
    static WATCHING: Cell<bool> = const { Cell::new(false) };
}

/// The mirror of `schema_id`, opened on first use. `None` when the schema is not bundled or
/// the extension does not answer, which the Settings dialog shows as unavailable rows.
pub fn open(schema_id: &str) -> Option<gio::Settings> {
    if let Some(settings) = MIRRORS.with(|m| m.borrow().get(schema_id).map(|m| m.settings.clone())) {
        return Some(settings);
    }
    let connection = gio::Application::default()?.dbus_connection()?;
    let schema = bundled_schema(schema_id)?;
    let values = match fetch(&connection, schema_id) {
        Ok(values) => values,
        Err(e) => {
            warn!(schema = schema_id, "the extension did not share its settings: {e}");
            return None;
        }
    };
    let backend = gio::memory_settings_backend_new();
    let settings = gio::Settings::new_full(&schema, Some(&backend), None::<&str>);
    let applying = Rc::new(Cell::new(false));
    apply_all(&settings, &schema, &values, &applying);

    // Out: a change made here, by a row or a reset, goes to the extension.
    {
        let (connection, applying, id) = (connection.clone(), Rc::clone(&applying), schema_id.to_owned());
        settings.connect_changed(None, move |settings, key| {
            if applying.get() {
                return;
            }
            let value = settings.user_value(key);
            let (connection, id, key) = (connection.clone(), id.clone(), key.to_owned());
            glib::spawn_future_local(async move { send(&connection, &id, &key, value).await });
        });
    }
    // In: a change made anywhere else -- GNOME Settings, the extension itself -- comes here.
    let changes = {
        let (settings, schema, applying) = (settings.clone(), schema.clone(), Rc::clone(&applying));
        connection.subscribe_to_signal(
            Some(SHELL_BUS_NAME),
            Some(SHELL_INTERFACE),
            Some("SettingChanged"),
            Some(SHELL_OBJECT_PATH),
            Some(schema_id),
            gio::DBusSignalFlags::NONE,
            move |signal| {
                let params = signal.parameters;
                let (Some(key), Some(value)) =
                    (params.child_value(1).str().map(str::to_owned), params.child_value(2).as_variant())
                else {
                    return;
                };
                apply_one(&settings, &schema, &key, &value, &applying);
            },
        )
    };
    info!(schema = schema_id, keys = values.len(), "mirroring the extension's settings");
    MIRRORS.with(|m| {
        m.borrow_mut().insert(
            schema_id.to_owned(),
            Mirror { settings: settings.clone(), applying, _changes: changes },
        );
    });
    watch_extension(&connection);
    Some(settings)
}

/// Every keybinding a new shortcut could collide with, as the extension reads them from
/// GNOME Shell's dconf (`GetKeybindings`), for `prefs.rs`'s conflict check.
pub fn keybindings() -> Option<Vec<octosnap_core::shortcuts::Binding>> {
    let connection = gio::Application::default()?.dbus_connection()?;
    let reply = connection
        .call_sync(
            Some(SHELL_BUS_NAME),
            SHELL_OBJECT_PATH,
            SHELL_INTERFACE,
            "GetKeybindings",
            None,
            glib::VariantTy::new("(a(ssas))").ok(),
            gio::DBusCallFlags::NONE,
            TIMEOUT_MS,
            gio::Cancellable::NONE,
        )
        .map_err(|e| warn!("the extension did not share its keybindings: {e}"))
        .ok()?;
    let (bindings,) = reply.get::<(Vec<(String, String, Vec<String>)>,)>()?;
    Some(
        bindings
            .into_iter()
            .map(|(schema, key, accelerators)| octosnap_core::shortcuts::Binding { schema, key, accelerators })
            .collect(),
    )
}

/// D124: the desktop wallpaper for a window capture's background, as a copy the
/// extension made where this app can read it (`GetWallpaper`). `None` when there is none.
pub fn wallpaper(dark: bool) -> Option<String> {
    let connection = gio::Application::default()?.dbus_connection()?;
    let reply = connection
        .call_sync(
            Some(SHELL_BUS_NAME),
            SHELL_OBJECT_PATH,
            SHELL_INTERFACE,
            "GetWallpaper",
            Some(&(dark,).to_variant()),
            glib::VariantTy::new("(s)").ok(),
            gio::DBusCallFlags::NONE,
            // A first call copies the file, which can be a few megabytes.
            5000,
            gio::Cancellable::NONE,
        )
        .map_err(|e| warn!("the extension did not hand over the wallpaper: {e}"))
        .ok()?;
    let (path,) = reply.get::<(String,)>()?;
    if path.is_empty() {
        info!(dark, "the extension has no wallpaper to hand over");
        return None;
    }
    info!(dark, %path, "the extension handed over the wallpaper");
    Some(path)
}

/// The bundled copy of `schema_id`, from the first data directory that has one.
fn bundled_schema(schema_id: &str) -> Option<gio::SettingsSchema> {
    let default = gio::SettingsSchemaSource::default();
    for root in glib::system_data_dirs() {
        let dir = root.join(SCHEMA_DIR);
        if !dir.join("gschemas.compiled").is_file() {
            continue;
        }
        match gio::SettingsSchemaSource::from_directory(&dir, default.as_ref(), false) {
            // Not recursive: the copy here, never a runtime's schema of the same name.
            Ok(source) => {
                if let Some(schema) = source.lookup(schema_id, false) {
                    return Some(schema);
                }
            }
            Err(e) => warn!(dir = %dir.display(), "could not read the bundled schemas: {e}"),
        }
    }
    warn!(schema = schema_id, "no bundled copy of the schema to mirror");
    None
}

fn fetch(connection: &gio::DBusConnection, schema_id: &str) -> Result<Vec<(String, glib::Variant)>, String> {
    let reply = connection
        .call_sync(
            Some(SHELL_BUS_NAME),
            SHELL_OBJECT_PATH,
            SHELL_INTERFACE,
            "GetSettings",
            Some(&(schema_id,).to_variant()),
            glib::VariantTy::new("(a{sv})").ok(),
            gio::DBusCallFlags::NONE,
            TIMEOUT_MS,
            gio::Cancellable::NONE,
        )
        .map_err(|e| e.message().to_owned())?;
    let dict = reply.child_value(0);
    let mut values = Vec::with_capacity(dict.n_children());
    for index in 0..dict.n_children() {
        let entry = dict.child_value(index);
        let (Some(key), Some(value)) = (entry.child_value(0).str().map(str::to_owned), entry.child_value(1).as_variant())
        else {
            continue;
        };
        values.push((key, value));
    }
    Ok(values)
}

fn apply_all(
    settings: &gio::Settings,
    schema: &gio::SettingsSchema,
    values: &[(String, glib::Variant)],
    applying: &Cell<bool>,
) {
    for (key, value) in values {
        apply_one(settings, schema, key, value, applying);
    }
}

/// Writes one value the extension reported, if this copy of the schema has the key with
/// that type: an extension newer or older than this app may not.
fn apply_one(
    settings: &gio::Settings,
    schema: &gio::SettingsSchema,
    key: &str,
    value: &glib::Variant,
    applying: &Cell<bool>,
) {
    if !schema.has_key(key) || schema.key(key).value_type() != *value.type_() {
        debug!(key, "a mirrored key this schema does not have, or of another type");
        return;
    }
    if settings.value(key) == *value {
        return;
    }
    applying.set(true);
    match settings.set_value(key, value) {
        Ok(()) => debug!(key, value = %value.print(false), "mirrored a change from the extension"),
        Err(e) => warn!(key, "could not mirror a setting: {e}"),
    }
    applying.set(false);
}

/// Sends one change: the value, or `None` for a key put back to its default.
async fn send(connection: &gio::DBusConnection, schema_id: &str, key: &str, value: Option<glib::Variant>) {
    let (method, params) = match &value {
        Some(value) => ("SetSetting", glib::Variant::tuple_from_iter([
            schema_id.to_variant(),
            key.to_variant(),
            glib::Variant::from_variant(value),
        ])),
        None => ("ResetSetting", (schema_id, key).to_variant()),
    };
    let result = connection
        .call_future(
            Some(SHELL_BUS_NAME),
            SHELL_OBJECT_PATH,
            SHELL_INTERFACE,
            method,
            Some(&params),
            None,
            gio::DBusCallFlags::NONE,
            TIMEOUT_MS,
        )
        .await;
    match result {
        Ok(_) => debug!(schema = schema_id, key, method, "sent a setting to the extension"),
        Err(e) => warn!(schema = schema_id, key, "the extension refused a setting: {e}"),
    }
}

/// When the extension comes back -- GNOME Shell restarted, the extension turned off and
/// on -- every mirror is filled again, since whatever changed meanwhile was not signalled.
fn watch_extension(connection: &gio::DBusConnection) {
    if WATCHING.with(|w| w.replace(true)) {
        return;
    }
    let _watch = gio::bus_watch_name_on_connection(
        connection,
        SHELL_BUS_NAME,
        gio::BusNameWatcherFlags::NONE,
        |connection, _, _| {
            let ids: Vec<String> = MIRRORS.with(|m| m.borrow().keys().cloned().collect());
            for id in ids {
                let Ok(values) = fetch(&connection, &id) else { continue };
                MIRRORS.with(|m| {
                    if let Some(mirror) = m.borrow().get(&id)
                        && let Some(schema) = mirror.settings.settings_schema()
                    {
                        apply_all(&mirror.settings, &schema, &values, &mirror.applying);
                    }
                });
            }
        },
        |_, _| {},
    );
}
