// SPDX-License-Identifier: GPL-3.0-or-later

//! `ANN-31` (D183): the Cut Out tool. A drag down the picture shows a band across it, and a
//! drag across shows one down it; letting go cuts the band out and closes the gap, as one
//! step.
//!
//! The pixels go, so the base image is written again without them, beside the capture in
//! the spool, and the scene's base becomes that file (`octosnap_scene::slice`). The canvas
//! keeps the texture it had, so an undo swaps back without reading the old file again.

use std::rc::Rc;
use std::time::Instant;

use gtk::prelude::*;
use gtk::{gdk, glib};
use octosnap_scene::slice::{self, Slice};
use octosnap_scene::{Command, Point};
use tracing::{debug, info, warn};

use super::tools::Gesture;
use super::window::Editor;

impl Editor {
    /// Whether a Cut Out drag is under way.
    pub(super) fn slicing(&self) -> bool {
        matches!(*self.tools.gesture.borrow(), Some(Gesture::Slicing { .. }))
    }

    /// The band a drag from `from` to `to` would cut, shown over the picture.
    pub(super) fn preview_slice(&self, from: Point, to: Point) {
        let band = self
            .canvas
            .scene()
            .and_then(|scene| Slice::dragged(from, to, scene.base.width, scene.base.height));
        self.canvas.set_slice(band);
    }

    /// Escape during the drag: the band goes and nothing is cut. Answers whether there was a
    /// drag to cancel.
    pub(super) fn cancel_slice(&self) -> bool {
        if !self.slicing() {
            return false;
        }
        *self.tools.gesture.borrow_mut() = None;
        self.canvas.set_slice(None);
        debug!("cut-out cancelled");
        true
    }

    /// The drag let go: the band it describes is cut out.
    pub(super) fn end_slice(self: &Rc<Self>, from: Point, to: Point) {
        self.canvas.set_slice(None);
        let Some(scene) = self.canvas.scene() else { return };
        match Slice::dragged(from, to, scene.base.width, scene.base.height) {
            Some(band) => {
                self.cut_out(band);
            }
            None => debug!("a cut-out drag too short to be a band"),
        }
    }

    /// Cuts `band` out of the picture and closes the gap. Answers whether it did.
    pub(super) fn cut_out(self: &Rc<Self>, band: Slice) -> bool {
        let started = Instant::now();
        let Some(scene) = self.canvas.scene() else { return false };
        let Some(pixels) = self.canvas.base_pixels() else {
            warn!("the picture's pixels are not loaded yet; nothing was cut");
            return false;
        };
        let Some((band, stored)) = band.on_pixels(&scene.base, (pixels.width, pixels.height))
        else {
            debug!("the band is less than a pixel of the picture, or all of it");
            return false;
        };
        let cut = slice::cut_pixels(&pixels, stored);
        let Some(texture) = texture_of(cut) else { return false };
        let Some(path) = self.picture_file(&texture, "cut") else {
            self.toast("The cut picture could not be written", None);
            return false;
        };
        let file = path.to_string_lossy().into_owned();
        self.canvas.adopt_base(&file, &texture);
        let Some(command) = slice::plan(&scene, band, file) else { return false };
        let (removed, moved) = match &command {
            Command::Batch(commands) => (
                commands.iter().filter(|c| matches!(c, Command::Remove(_))).count(),
                commands.iter().filter(|c| matches!(c, Command::Change { .. })).count(),
            ),
            _ => (0, 0),
        };
        self.canvas.set_selection(Vec::new());
        self.commit(command);
        let base = self.canvas.scene().map(|scene| scene.base);
        info!(
            axis = band.axis.name(),
            pixels = stored.count,
            band = format!("{:.2}", band.width()),
            removed,
            moved,
            width = base.as_ref().map_or(0.0, |b| b.width),
            height = base.as_ref().map_or(0.0, |b| b.height),
            ms = format!("{:.1}", started.elapsed().as_secs_f64() * 1000.0),
            "slice cut"
        );
        true
    }
}

/// A straight-alpha RGBA buffer as a texture.
fn texture_of(pixels: octosnap_scene::redact::Pixels) -> Option<gdk::Texture> {
    let width = i32::try_from(pixels.width).ok()?;
    let height = i32::try_from(pixels.height).ok()?;
    let stride = pixels.width.checked_mul(4)?;
    let bytes = glib::Bytes::from_owned(pixels.rgba);
    Some(
        gdk::MemoryTexture::new(width, height, gdk::MemoryFormat::R8g8b8a8, &bytes, stride)
            .upcast(),
    )
}
