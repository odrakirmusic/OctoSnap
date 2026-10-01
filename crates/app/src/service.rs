// SPDX-License-Identifier: GPL-3.0-or-later

//! The app's own D-Bus surface: `io.github.odrakirmusic.OctoSnap.App1` (`spec/10` §3.2).
//!
//! Registered on `GApplication`'s connection, not a new one. That is a hard requirement,
//! not a convenience: `GApplication` owns the well-known name, so an object exported on
//! any other connection would be unreachable for anyone addressing that name. It is also
//! the reason this crate uses gio rather than zbus for D-Bus (see `docs/decisions.md` D7).

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use octosnap_core::protocol::{APP_INTERFACE, APP_OBJECT_PATH, PROTOCOL_VERSION};
use octosnap_shell::{APP1_INTERFACE_XML, capture};
use tracing::{info, warn};

use crate::capture_window;

/// Error names from `spec/10` §8.
const ERROR_INVALID_ARGS: &str = "io.github.odrakirmusic.OctoSnap.Error.InvalidArgs";
const ERROR_UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";

pub fn register(app: &adw::Application) -> Result<gio::RegistrationId, glib::Error> {
    let connection = app.dbus_connection().ok_or_else(|| {
        glib::Error::new(
            gio::IOErrorEnum::NotConnected,
            "the application has no session bus connection",
        )
    })?;

    let node = gio::DBusNodeInfo::for_xml(APP1_INTERFACE_XML)?;
    let interface = node.lookup_interface(APP_INTERFACE).ok_or_else(|| {
        glib::Error::new(
            gio::IOErrorEnum::Failed,
            &format!("{APP_INTERFACE} is missing from the interface XML"),
        )
    })?;

    let app = app.clone();
    let id = connection
        .register_object(APP_OBJECT_PATH, &interface)
        .method_call(move |_connection, _sender, _path, _interface, method, params, invocation| {
            match method {
                // The extension calls this to learn whether the app is alive and which
                // protocol it speaks. Also what the extension's watchdog polls during
                // recording and scroll assist (`spec/10` §2).
                "Ping" => invocation.return_value(Some(&(PROTOCOL_VERSION,).to_variant())),

                "HandleCapture" => {
                    let payload = params.child_value(0);
                    match capture::from_variant(&payload) {
                        Ok(result) => {
                            info!(capture = %capture_window::describe(&result), "HandleCapture");
                            capture_window::show(&app, &result);
                            invocation.return_value(None);
                        }
                        Err(e) => {
                            // A typed error, never a panic (`spec/10` §8). The extension
                            // has already written the PNG and its JSON twin, so nothing
                            // is lost -- the capture can be recovered from the spool.
                            warn!("rejected a malformed HandleCapture payload: {e}");
                            invocation.return_dbus_error(ERROR_INVALID_ARGS, &e.to_string());
                        }
                    }
                }

                other => invocation.return_dbus_error(
                    ERROR_UNKNOWN_METHOD,
                    &format!("{APP_INTERFACE} has no method '{other}'"),
                ),
            }
        })
        .build()?;

    info!(path = APP_OBJECT_PATH, interface = APP_INTERFACE, "exported app interface");
    Ok(id)
}
