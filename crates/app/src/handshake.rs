// SPDX-License-Identifier: GPL-3.0-or-later

//! Startup handshake with the extension (`spec/10` §2).
//!
//! Runs asynchronously on the GTK main loop rather than blocking startup: `spec/10` §6
//! caps UI-thread work at 4 ms, and a D-Bus round trip is nowhere near that. A missing
//! extension is an expected state on a fresh install -- it leads to degraded mode and the
//! setup page (M4), never to an error dialog.

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use octosnap_core::protocol::PROTOCOL_VERSION;
use octosnap_shell::{GnomeExtensionBridge, ShellBridge};
use tracing::{error, info, warn};

pub fn run(app: &adw::Application) {
    let Some(connection) = app.dbus_connection() else {
        warn!("no session bus connection; the shell handshake cannot run");
        return;
    };

    // D122: captures are written where this app reads them. Said whenever the extension
    // appears, not once: an extension that GNOME Shell restarts, or that is turned off and
    // on, starts with nothing announced. A watch reports a name that is already owned as
    // appearing, so this is also the first announcement.
    let _watch = gio::bus_watch_name_on_connection(
        &connection,
        octosnap_core::protocol::SHELL_BUS_NAME,
        gio::BusNameWatcherFlags::NONE,
        |connection, _, _| {
            // D125: and what the recording row should show, which it may not be able to read.
            crate::settings::sync_gif_defaults(&crate::settings::Settings::load());
            let bridge = GnomeExtensionBridge::from_connection(connection);
            glib::spawn_future_local(async move {
                let spool = crate::history::History::default_spool();
                match bridge.set_spool(&spool).await {
                    Ok(()) => info!(spool = %spool.display(), "told the extension where the spool is"),
                    Err(e) => warn!("could not tell the extension where the spool is: {e}"),
                }
            });
        },
        |_, _| {},
    );

    let bridge = GnomeExtensionBridge::from_connection(connection.clone());
    let app = app.clone();

    glib::MainContext::default().spawn_local(async move {
        // `spec/13` #15: a stale extension is said in the UI, not only here.
        let found = crate::setup::status(&connection).await;
        // D160: an install the last session asked for, finished now that GNOME Shell has
        // found the extension.
        let found = crate::setup::finish_install(&connection, found).await;
        // D160: and the extension it installed, kept in step with an app updated since.
        let found = crate::setup::update_carried(&connection, found).await;
        info!(?found, "extension status");
        crate::setup::after_handshake(&app, &found);

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
