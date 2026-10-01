// SPDX-License-Identifier: GPL-3.0-or-later

//! Connector names from `org.gnome.Mutter.DisplayConfig`.
//!
//! `spec/10` §3.1 has the extension's `GetMonitors` returning a connector, but it cannot:
//! GNOME Shell's `Monitor` is a plain JS object carrying only
//! `index, x, y, width, height, geometry_scale`, and `MetaMonitorManager` exposes nothing
//! useful to JS. Verified by introspection on GNOME 50.1 --
//! `docs/spikes/15-monitor-metadata.md`.
//!
//! `DisplayConfig` is callable by any session process without a prompt (proven in spike
//! 5), and its `logical_monitors` are expressed in the same logical coordinate space the
//! extension uses. So the app resolves connectors itself and the extension stays thin,
//! which is the tie-break `spec/12` asks for: "when in doubt, put logic in the app".
//!
//! The same call carries each output's modes, and the current mode's **refresh rate** is
//! what a GIF recording asked to "match the screen" needs (D101), so it is read here from
//! the same reply. The extension can see it too, on the stage view Mutter paints each
//! monitor through, but reads it there only to say on the toolbar what the screen's rate
//! will be (D117): the rate a recording is made at is this one.

use std::collections::HashMap;

use crate::error::BridgeError;

const DEST: &str = "org.gnome.Mutter.DisplayConfig";
const PATH: &str = "/org/gnome/Mutter/DisplayConfig";

/// `logical_monitors` from `GetCurrentState`: `a(iiduba(ssss)a{sv})`.
#[derive(Debug, Clone, PartialEq)]
pub struct LogicalMonitor {
    /// Logical position, the same space as the extension's monitor geometry.
    pub x: i32,
    pub y: i32,
    /// Fractional scale, e.g. 1.25.
    pub scale: f64,
    pub primary: bool,
    /// Connectors driven by this logical monitor. More than one means mirroring.
    pub connectors: Vec<String>,
    /// The first connector's current refresh rate in hertz, when the reply carried one.
    pub refresh: Option<f64>,
}

pub async fn logical_monitors(
    connection: &gio::DBusConnection,
) -> Result<Vec<LogicalMonitor>, BridgeError> {
    let reply_type = glib::VariantTy::new("(ua((ssss)a(siiddada{sv})a{sv})a(iiduba(ssss)a{sv})a{sv})")
        .map_err(|e| BridgeError::MalformedReply {
            method: "GetCurrentState",
            detail: format!("invalid reply signature: {e}"),
        })?;

    let reply = connection
        .call_future(
            Some(DEST),
            PATH,
            DEST,
            "GetCurrentState",
            None,
            Some(reply_type),
            gio::DBusCallFlags::NONE,
            2_000,
            )
        .await
        .map_err(BridgeError::Dbus)?;

    // (serial, monitors, logical_monitors, properties) -- the physical monitors at index
    // 1 carry the modes, the logical ones at index 2 carry the layout.
    let rates = parse_refresh_rates(&reply.child_value(1));
    let logical = reply.child_value(2);
    let mut out = Vec::with_capacity(logical.n_children());

    for i in 0..logical.n_children() {
        let entry = logical.child_value(i);
        let Some(parsed) = parse_logical_monitor(&entry, &rates) else {
            return Err(BridgeError::MalformedReply {
                method: "GetCurrentState",
                detail: format!("logical_monitor {i} has an unexpected shape"),
            });
        };
        out.push(parsed);
    }
    Ok(out)
}

/// Each connector's current refresh rate, from `monitors: a((ssss)a(siiddada{sv})a{sv})`.
///
/// A mode is `(id s, width i, height i, refresh d, preferred_scale d, scales ad,
/// properties a{sv})`, and the one in use says so with `is-current` in its properties.
/// Lenient on purpose: a monitor with no current mode -- one that is off -- is simply
/// left out, and a shape this code does not expect costs a rate, not the layout.
fn parse_refresh_rates(monitors: &glib::Variant) -> HashMap<String, f64> {
    let mut rates = HashMap::new();
    for i in 0..monitors.n_children() {
        let monitor = monitors.child_value(i);
        if monitor.n_children() < 2 {
            continue;
        }
        let Some(connector) = monitor.child_value(0).child_value(0).get::<String>() else {
            continue;
        };
        let modes = monitor.child_value(1);
        let described = (0..modes.n_children()).filter_map(|j| {
            let mode = modes.child_value(j);
            if mode.n_children() < 7 {
                return None;
            }
            let refresh = mode.child_value(3).get::<f64>()?;
            let properties = glib::VariantDict::new(Some(&mode.child_value(6)));
            let current = properties
                .lookup_value("is-current", Some(glib::VariantTy::BOOLEAN))
                .and_then(|v| v.get::<bool>())
                .unwrap_or(false);
            Some((refresh, current))
        });
        if let Some(refresh) = current_refresh(described) {
            rates.insert(connector, refresh);
        }
    }
    rates
}

/// The current mode's rate out of `(refresh, is_current)` pairs; `None` when no mode is
/// current or the rate is not a rate. Split out so the rule is testable without a bus.
fn current_refresh(modes: impl Iterator<Item = (f64, bool)>) -> Option<f64> {
    modes
        .filter(|(_, current)| *current)
        .map(|(refresh, _)| refresh)
        .find(|refresh| refresh.is_finite() && *refresh > 0.0)
}

fn parse_logical_monitor(
    entry: &glib::Variant,
    rates: &HashMap<String, f64>,
) -> Option<LogicalMonitor> {
    // (x i, y i, scale d, transform u, primary b, monitors a(ssss), properties a{sv})
    let x = entry.child_value(0).get::<i32>()?;
    let y = entry.child_value(1).get::<i32>()?;
    let scale = entry.child_value(2).get::<f64>()?;
    let primary = entry.child_value(4).get::<bool>()?;

    let monitors = entry.child_value(5);
    let mut connectors = Vec::with_capacity(monitors.n_children());
    for j in 0..monitors.n_children() {
        // (connector, vendor, product, serial)
        if let Some(connector) = monitors.child_value(j).child_value(0).get::<String>() {
            connectors.push(connector);
        }
    }

    let refresh = connectors.first().and_then(|connector| rates.get(connector).copied());
    Some(LogicalMonitor { x, y, scale, primary, connectors, refresh })
}

/// Finds the connector for a monitor at logical position `(x, y)`.
///
/// Position is the key rather than scale or size, because two outputs can share a scale
/// and a resolution but never a logical origin. Split out as a pure function so the
/// matching rule is testable without a session bus.
#[must_use]
pub fn connector_at(logical: &[LogicalMonitor], x: i32, y: i32) -> Option<&str> {
    logical
        .iter()
        .find(|m| m.x == x && m.y == y)
        .and_then(|m| m.connectors.first())
        .map(String::as_str)
}

/// The refresh rate of the monitor at logical position `(x, y)`, by the same key.
#[must_use]
pub fn refresh_at(logical: &[LogicalMonitor], x: i32, y: i32) -> Option<f64> {
    logical.iter().find(|m| m.x == x && m.y == y).and_then(|m| m.refresh)
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn layout() -> Vec<LogicalMonitor> {
        vec![
            LogicalMonitor {
                x: 0,
                y: 0,
                scale: 1.25,
                primary: true,
                connectors: vec!["eDP-1".to_owned()],
                refresh: Some(60.0),
            },
            LogicalMonitor {
                x: 1536,
                y: 0,
                scale: 2.0,
                primary: false,
                connectors: vec!["DP-2".to_owned()],
                refresh: Some(143.98),
            },
        ]
    }

    #[test]
    fn matches_by_logical_origin() {
        let l = layout();
        assert_eq!(connector_at(&l, 0, 0), Some("eDP-1"));
        assert_eq!(connector_at(&l, 1536, 0), Some("DP-2"));
    }

    #[test]
    fn an_unmatched_origin_is_none_rather_than_a_wrong_guess() {
        assert_eq!(connector_at(&layout(), 99, 99), None);
        assert_eq!(refresh_at(&layout(), 99, 99), None);
    }

    /// The rate follows the same key as the connector, and a 144 Hz panel reports what
    /// Mutter reports -- 143.98 -- rather than a rounded number this side invented.
    #[test]
    fn the_refresh_rate_is_found_by_the_same_origin() {
        assert_eq!(refresh_at(&layout(), 0, 0), Some(60.0));
        assert_eq!(refresh_at(&layout(), 1536, 0), Some(143.98));
    }

    /// Only the mode that is current counts, and a rate that is not a rate does not.
    #[test]
    fn the_current_mode_is_the_one_asked_about() {
        let modes = [(60.0, false), (143.98, true), (120.0, false)];
        assert_eq!(current_refresh(modes.into_iter()), Some(143.98));
        assert_eq!(current_refresh([(60.0, false)].into_iter()), None, "nothing current");
        assert_eq!(current_refresh([(0.0, true)].into_iter()), None, "zero is not a rate");
        assert_eq!(current_refresh(std::iter::empty()), None, "an output that is off");
    }

    /// Mirrored outputs share an origin; the first connector is a defensible pick and
    /// must not panic or pick arbitrarily from an empty list.
    #[test]
    fn handles_mirroring_and_empty_connector_lists() {
        let mirrored = vec![LogicalMonitor {
            x: 0,
            y: 0,
            scale: 1.0,
            primary: true,
            connectors: vec!["eDP-1".to_owned(), "HDMI-1".to_owned()],
            refresh: None,
        }];
        assert_eq!(connector_at(&mirrored, 0, 0), Some("eDP-1"));

        let empty = vec![LogicalMonitor {
            x: 0,
            y: 0,
            scale: 1.0,
            primary: true,
            connectors: vec![],
            refresh: None,
        }];
        assert_eq!(connector_at(&empty, 0, 0), None);
    }
}
