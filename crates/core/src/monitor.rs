// SPDX-License-Identifier: GPL-3.0-or-later

//! Monitor description as reported by the extension's `GetMonitors` (`spec/10` §3.1).

use serde::{Deserialize, Serialize};

use crate::geometry::Rect;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Monitor {
    pub index: i32,
    /// Connector name, e.g. `eDP-1`. Empty when Mutter does not expose one.
    pub connector: String,
    /// Position and size in logical stage coordinates.
    pub geometry: Rect,
    /// Work area, i.e. geometry minus panels and docks. Logical.
    pub work_area: Rect,
    /// The fractional monitor scale: 1.0, 1.25, 1.5, 2.0 -- every one of them is a
    /// machine somebody runs this on, and none of them is the default case.
    pub scale: f64,
    /// Mutter's integer scale. Differs from `scale` on any fractionally scaled output,
    /// which is why both cross the boundary (`spec/01` §1).
    pub geometry_scale: i32,
    pub primary: bool,
    /// The monitor the pointer is on, i.e. where a capture should default to.
    pub current: bool,
    /// The current mode's refresh rate in hertz, when Mutter's `DisplayConfig` told the
    /// bridge (D101). The extension's `GetMonitors` does not carry one, for the reason
    /// it carries no connector (`docs/spikes/15`), so the bridge fills it in from the
    /// same call that names the connector. `None` is "not known", never "zero".
    #[serde(default)]
    pub refresh: Option<f64>,
}

impl Monitor {
    /// True when this output is fractionally scaled, so callers can refuse or warn about
    /// operations that only behave correctly at integer scale.
    #[must_use]
    pub fn is_fractionally_scaled(&self) -> bool {
        (self.scale - self.scale.round()).abs() > f64::EPSILON
    }
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn monitor_at(scale: f64) -> Monitor {
        Monitor {
            index: 0,
            connector: "eDP-1".to_owned(),
            geometry: Rect::new(0, 0, 1536, 960),
            work_area: Rect::new(0, 0, 1536, 923),
            scale,
            geometry_scale: 2,
            primary: true,
            current: true,
            refresh: Some(60.0),
        }
    }

    #[test]
    fn detects_fractional_scaling() {
        assert!(monitor_at(1.25).is_fractionally_scaled());
        assert!(monitor_at(1.5).is_fractionally_scaled());
        assert!(!monitor_at(1.0).is_fractionally_scaled());
        assert!(!monitor_at(2.0).is_fractionally_scaled());
    }
}
