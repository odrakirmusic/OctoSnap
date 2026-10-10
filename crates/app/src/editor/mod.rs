// SPDX-License-Identifier: GPL-3.0-or-later

//! The annotation editor (`spec/05`), `ANN-01`.
//!
//! The document itself is `octosnap_scene`, which has no GTK in it and is tested without
//! a display. This module is the half that needs a GPU: the canvas widget, the render
//! nodes, and the window around them.

pub mod actions;
pub mod background;
pub mod canvas;
pub mod combine;
pub mod context;
pub mod crop;
pub mod cursor;
pub mod layers;
pub mod live_text;
pub mod nodes;
pub mod options;
pub mod picker;
pub mod preview;
pub mod redact;
pub mod saved;
pub mod sensitive;
pub mod slice;
pub mod text;
pub mod tools;
pub mod transform;
pub mod vector;
pub mod window;

pub use actions::EditorActions;
pub use canvas::Canvas;
pub use window::Editor;
