// SPDX-License-Identifier: GPL-3.0-or-later

//! The app's client for the GNOME Shell extension: `spec/10` §5's `crates/shell`.
//!
//! Despite the name this is not the extension. It is the *other end* of the D-Bus
//! contract in `spec/10` §3.1, and the only place in the Rust tree that knows the
//! contract's wire shapes.

pub mod bridge;
pub mod capture;
pub mod display_config;
pub mod error;
pub mod gnome;
pub mod null;
pub mod variant;

pub use bridge::{
    Cue, PickedColor, Placement, PlacementMonitor, RecordingState, ShellBridge, ShellVersion,
};
pub use capture::APP1_INTERFACE_XML;
pub use error::BridgeError;
pub use gnome::GnomeExtensionBridge;
pub use null::NullBridge;
