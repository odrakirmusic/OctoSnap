// SPDX-License-Identifier: GPL-3.0-or-later

//! Startup handshake with the extension (`spec/10` §2).
//!
//! Runs asynchronously on the GTK main loop rather than blocking startup: `spec/10` §6
//! caps UI-thread work at 4 ms, and a D-Bus round trip is nowhere near that. A missing
//! extension is an expected state on a fresh install -- it leads to degraded mode and the
//! setup page (M4), never to an error dialog.

use std::cell::Cell;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use octosnap_core::protocol::PROTOCOL_VERSION;
use octosnap_shell::{GnomeExtensionBridge, ShellBridge};
use tracing::{error, info, warn};

thread_local! {
    /// D161: the start found the extension off for a lock, and has not looked again since.
    static FOUND_LOCKED: Cell<bool> = const { Cell::new(false) };
}

pub fn run(app: &adw::Application) {
    let Some(connection) = app.dbus_connection() else {
        warn!("no session bus connection; the shell handshake cannot run");
        return;
    };

    // D122: captures are written where this app reads them. Said whenever the extension
    // appears, not once: an extension that GNOME Shell restarts, or that is turned off and
    // on, starts with nothing announced. A watch reports a name that is already owned as
    // appearing, so this is also the first announcement.
    let _watch = {
        let app = app.clone();
        gio::bus_watch_name_on_connection(
            &connection,
            octosnap_core::protocol::SHELL_BUS_NAME,
            gio::BusNameWatcherFlags::NONE,
            move |connection, _, _| {
                // D125: and what the recording row should show, which it may not be able to read.
                crate::settings::sync_gif_defaults(&crate::settings::Settings::load());
                let bridge = GnomeExtensionBridge::from_connection(connection.clone());
                glib::spawn_future_local(async move {
                    let spool = crate::history::History::default_spool();
                    match bridge.set_spool(&spool).await {
                        Ok(()) => info!(spool = %spool.display(), "told the extension where the spool is"),
                        Err(e) => warn!("could not tell the extension where the spool is: {e}"),
                    }
                });
                // D161: back from the lock the start found, which said nothing then.
                if FOUND_LOCKED.replace(false) {
                    settle_again(&app, &connection);
                }
            },
            |_, _| {},
        )
    };

    let bridge = GnomeExtensionBridge::from_connection(connection.clone());
    let app = app.clone();

    glib::MainContext::default().spawn_local(async move {
        // `spec/13` #15: a stale extension is said in the UI, not only here.
        let found = crate::setup::settle(&connection).await;
        info!(?found, "extension status");
        crate::setup::after_handshake(&app, &found);
        if found == crate::setup::Status::Locked {
            look_again_at_unlock(&app, &connection).await;
        }

        match bridge.version().await {
            Ok(shell) => {
                info!(
                    version = %shell.version,
                    protocol = shell.protocol,
                    "shell extension is present"
                );

                if shell.protocol != PROTOCOL_VERSION {
                    warn!(
                        expected = PROTOCOL_VERSION,
                        found = shell.protocol,
                        "protocol mismatch; mismatched features must be refused (spec/10 §2)"
                    );
                }

                report_monitors(&bridge).await;
            }
            Err(e) if e.is_extension_missing() => {
                warn!("running in degraded mode: {e}");
            }
            Err(e) => {
                error!("shell handshake failed: {e}");
            }
        }
    });
}

/// D161: a start that found the extension off for a lock says nothing then, and looks again
/// when the extension's name comes back, which the unlock brings. The name can have come
/// back while the start was asking, so that is looked at here too.
async fn look_again_at_unlock(app: &adw::Application, connection: &gio::DBusConnection) {
    FOUND_LOCKED.set(true);
    let owned = connection
        .call_future(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            Some(&(octosnap_core::protocol::SHELL_BUS_NAME,).to_variant()),
            glib::VariantTy::new("(b)").ok(),
            gio::DBusCallFlags::NONE,
            2000,
        )
        .await
        .ok()
        .and_then(|reply| reply.child_value(0).get::<bool>())
        .unwrap_or(false);
    if owned && FOUND_LOCKED.replace(false) {
        settle_again(app, connection);
    }
}

fn settle_again(app: &adw::Application, connection: &gio::DBusConnection) {
    let (app, connection) = (app.clone(), connection.clone());
    glib::spawn_future_local(async move {
        let found = crate::setup::settle(&connection).await;
        info!(?found, "extension status after the unlock");
        crate::setup::after_handshake(&app, &found);
        if found == crate::setup::Status::Locked {
            look_again_at_unlock(&app, &connection).await;
        }
    });
}

/// Logs the monitor layout. This is the M0 verification of the coordinate policy: on the
/// current target machine it should report one 1536x960 logical output at scale 1.25,
/// i.e. a 1920x1200 panel.
async fn report_monitors(bridge: &GnomeExtensionBridge) {
    match bridge.monitors().await {
        Ok(monitors) => {
            info!(count = monitors.len(), "monitors reported by the extension");
            for m in &monitors {
                let (pw, ph) = m.geometry.to_physical(m.scale);
                info!(
                    connector = %m.connector,
                    logical = %format!("{}x{}+{}+{}", m.geometry.width, m.geometry.height, m.geometry.x, m.geometry.y),
                    physical = %format!("{pw}x{ph}"),
                    scale = m.scale,
                    geometry_scale = m.geometry_scale,
                    fractional = m.is_fractionally_scaled(),
                    primary = m.primary,
                    current = m.current,
                    "monitor"
                );
            }
        }
        Err(e) => warn!("GetMonitors failed: {e}"),
    }
}
