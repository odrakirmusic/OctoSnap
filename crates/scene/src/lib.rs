// SPDX-License-Identifier: GPL-3.0-or-later

//! The annotation editor's document, and everything about it that needs no display.
//!
//! `spec/05` §5 and §6 split the editor cleanly in two, and this crate is the half that
//! can be tested at full speed: what objects exist, what order they render in, which one
//! the pointer is over, and what an edit did so it can be undone. The other half -- the
//! `gsk` render nodes, the canvas widget, the export -- lives in the app, because it
//! needs a GPU and a frame clock.
//!
//! The boundary is drawn where it is for a reason `spec/05` §6 states outright: "preview
//! == export by construction", because both walk the same node tree. That only holds if
//! the *tree's shape* is decided here, by data, rather than by drawing code that the
//! export path might reproduce slightly differently. So [`Scene::render_order`] is in
//! this crate and is a list of object ids, not a sequence of draw calls.
//!
//! `spec/05` §11 item 4 -- "undo 50 steps and redo 50 steps restores byte-identical
//! objects.json" -- is an acceptance test about this crate alone, and it is a test rather
//! than an aspiration because [`History`] holds commands and not snapshots.

pub mod background;
pub mod callout;
pub mod clipboard;
pub mod combine;
pub mod command;
pub mod counters;
pub mod crop;
pub mod geometry;
pub mod handle;
pub mod highlight;
pub mod hit;
pub mod layers;
pub mod magnifier;
pub mod object;
pub mod pressure;
pub mod project;
pub mod redact;
pub mod scene;
pub mod shape;
pub mod slice;
pub mod style;
pub mod tool;
pub mod transform;

pub use background::{Background, BackgroundParams, Blur, Layout, Preset, Ratio, layout};
pub use combine::{Insert, Zone};
pub use command::{Command, History};
pub use crop::{Aspect, Crop, Expand, FillKind};
pub use geometry::{Bounds, Point, Segment};
pub use handle::{Grip, Handle};
pub use highlight::Band;
pub use object::TextAlign;
pub use object::{Cover, Geometry, Grips, Object, ObjectId, ObjectKind};
pub use project::{Manifest, Opened};
pub use redact::{Inset, Pixels};
pub use scene::Scene;
pub use scene::{Base, Placement, Visible};
pub use style::{
    ArrowHead, ArrowStyle, COUNTER_SIZES, CounterStyle, FONT_SIZES, PALETTE, RedactStyle, Rgba,
    SIZE_LEVELS, SpotlightShape, Style, TextStyle, remember_color, stroke_width,
};
pub use tool::{Draft, Modifiers, Tool, ToolSettings};
pub use transform::{Mapping, Orientation, Transform};
