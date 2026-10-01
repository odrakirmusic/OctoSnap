// SPDX-License-Identifier: GPL-3.0-or-later

//! Pure models and logic shared by every OctoSnap binary.
//!
//! This crate deliberately depends on neither GTK nor glib. Everything here is plain
//! Rust so that the geometry, the filename template and the capture model can be unit
//! tested without a display, a session bus or a main loop (`spec/10` §11). The
//! conversion between these types and D-Bus variants lives in `octosnap-shell`, which
//! is where the boundary actually is.

pub mod actions;
pub mod autostart;
pub mod capture;
pub mod filename;
pub mod geometry;
pub mod history;
pub mod monitor;
pub mod pin;
pub mod print;
pub mod project;
pub mod selection;
pub mod shortcuts;
pub mod protocol;
pub mod qao;
pub mod request;
pub mod savepath;
pub mod scroll;

pub use actions::{AfterAction, Plan, Policy};
pub use capture::{CaptureMode, CaptureResult, RequestedAction, SourceWindow};
pub use request::CaptureRequest;
pub use savepath::ImageFormat;
pub use filename::CaptureType;
pub use geometry::Rect;
pub use monitor::Monitor;
pub use qao::{AutoClose, CardId, CardKind, Edge, Stack};
pub use scroll::ScrollDirection;
pub use selection::{Handle, Hit, Modifiers};
