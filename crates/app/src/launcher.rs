// SPDX-License-Identifier: GPL-3.0-or-later

//! The app's icon's right-click menu (`SYS-09`, D175): the `.desktop` file's
//! `[Desktop Action]` groups, and what each one does.
//!
//! The panel menu's capture rows in the panel's order (`extension/src/indicator.ts`), then
//! the history, so a right-click on the icon in the dash or the app grid offers what a
//! click on the octopus in the top bar does, under the same names. Less Previous Area,
//! which the top bar greys out until there has been an area and a desktop file cannot,
//! and Self-Timer, to keep the menu short. Record GIF waits for the extension: nothing
//! outside the shell can open its selection yet, and the app asking for a `record`
//! capture is told the mode is not implemented. Each group runs `octosnap launcher <id>`,
//! which hands the id to the app's `launcher` action.

use octosnap_core::CaptureMode;

/// Every action the `.desktop` file names, in its order, which a test holds it to.
#[cfg(test)]
pub const IDS: [&str; 5] =
    ["all-in-one", "capture-area", "capture-fullscreen", "capture-window", "history"];

/// What one of the icon's actions does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The overlay, in this mode, as the panel menu's row of the same name opens it.
    Capture(CaptureMode),
    /// The history strip.
    History,
}

impl Action {
    /// The action a `[Desktop Action]` id names, or `None` for one this build does not
    /// have: a `.desktop` file from a newer install, say, read by an older app.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "all-in-one" => Self::Capture(CaptureMode::AllInOne),
            "capture-area" => Self::Capture(CaptureMode::Area),
            "capture-fullscreen" => Self::Capture(CaptureMode::Fullscreen),
            "capture-window" => Self::Capture(CaptureMode::Window),
            "history" => Self::History,
            _ => return None,
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    const DESKTOP: &str = include_str!("../../../data/io.github.odrakirmusic.OctoSnap.desktop.in");

    /// The `.desktop` file lists exactly these actions, and each has a group with a name
    /// and an `Exec` that hands its own id to the app.
    #[test]
    fn the_desktop_file_names_every_action_and_nothing_else() {
        let listed = DESKTOP.lines().find_map(|l| l.strip_prefix("Actions=")).expect("Actions=");
        let listed: Vec<&str> = listed.split(';').filter(|s| !s.is_empty()).collect();
        assert_eq!(listed, IDS);
        for id in IDS {
            assert!(Action::from_id(id).is_some(), "{id}");
            let group = format!("[Desktop Action {id}]");
            let at = DESKTOP.find(&group).unwrap_or_else(|| panic!("no {group}"));
            let body = DESKTOP[at + group.len()..].split("\n[").next().unwrap();
            assert!(body.contains(&format!("\nExec=@BINDIR@/octosnap launcher {id}\n")), "{group}");
            assert!(body.lines().any(|l| l.starts_with("Name=")), "{group} has no name");
        }
    }

    #[test]
    fn an_id_this_build_does_not_have_is_none() {
        assert_eq!(Action::from_id("capture-text"), None);
        assert_eq!(Action::from_id(""), None);
        assert_eq!(Action::from_id("Capture-Area"), None);
        // Not until the extension can open the recorder's selection for the app (D175).
        assert_eq!(Action::from_id("record-gif"), None);
    }
}
