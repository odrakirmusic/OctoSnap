// SPDX-License-Identifier: GPL-3.0-or-later

//! Wire constants shared with the GNOME Shell extension. Mirrors `spec/10` §3 and must
//! stay byte-identical to `extension/src/protocol.ts`.

/// Bus name the extension owns.
pub const SHELL_BUS_NAME: &str = "io.github.odrakirmusic.OctoSnap.Shell";
pub const SHELL_OBJECT_PATH: &str = "/org/octosnap/Shell";
pub const SHELL_INTERFACE: &str = "io.github.odrakirmusic.OctoSnap.Shell";

/// The application half. D-Bus-activated via its `.service` file (`spec/10` §2).
pub const APP_BUS_NAME: &str = "io.github.odrakirmusic.OctoSnap";
pub const APP_OBJECT_PATH: &str = "/org/octosnap/App";
pub const APP_INTERFACE: &str = "io.github.odrakirmusic.OctoSnap.App1";

/// `spec/10` §3: "D-Bus contract (protocol version 1)".
///
/// The handshake in `spec/10` §2 compares this against the extension's reported value
/// and refuses mismatched features when the major differs.
pub const PROTOCOL_VERSION: u32 = 1;
