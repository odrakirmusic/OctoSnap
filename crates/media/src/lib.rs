// SPDX-License-Identifier: GPL-3.0-or-later
//! OctoSnap's recorder (`spec/06`), built in two halves: M5's GIF half is here, and M9
//! puts the video pipeline on the same spine (`spec/11`, D68).
//!
//! The pure half -- what a recording is asked to be, and what state it is in -- has no
//! GStreamer, no bus and no file in it, so it is tested to the pixel at every scale
//! without a compositor. The engine half owns the ScreenCast session, the graph, the reel
//! a recording's frames are kept in (D113) and the encoder that writes a GIF from them.

pub mod encoder;
pub mod gif;
pub mod gif_edit;
pub mod live;
pub mod pipeline;
pub mod recorder;
pub mod reel;
pub mod screencast;
pub mod state;
pub mod text;
