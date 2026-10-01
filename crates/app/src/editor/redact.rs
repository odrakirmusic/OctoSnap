// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §6's second pass for redactions: the worker, the texture cache and the
//! export path.
//!
//! > on gesture end, rasterize the exact region on a worker thread
//! > (`gio::spawn_blocking`) into a small `MemoryTexture` (this is the export-accurate
//! > version, including randomization) and swap it in. Only the redaction rectangle is
//! > touched, never the whole image.
//!
//! The pixel work is `octosnap_scene::redact`, which has no GTK in it and is tested
//! there. This file is the plumbing around it, and the plumbing has three jobs:
//!
//! 1. **Know when a raster is stale.** `objects.json` carries no revision, and a
//!    redaction's picture depends on more than the redaction: `spec/05` §5.3 has it
//!    "rasterize everything below it at its z", so an image object dragged underneath
//!    changes what it should show. So every redaction has a *key* -- a hash of the object
//!    and of everything beneath it -- recomputed whenever the scene changes, and a raster
//!    is current exactly when it was built for the key the scene now has.
//! 2. **Never block a frame.** The canvas's `snapshot` only *notes* what it wanted and
//!    could not have; an idle after the frame renders the region beneath the redaction
//!    (a GPU pass, clipped to the region) and hands the pixels to a worker. Until the
//!    worker answers, the stale raster stays on screen if there is one -- a redaction
//!    being dragged keeps looking like a redaction -- and the GPU blur preview stands in
//!    when there is none.
//! 3. **Make the export exact.** An export cannot wait for a worker, so before it walks
//!    the tree every redaction whose raster is stale is rasterized inline, bottom-most
//!    first, because a higher redaction samples the lower one's *raster*.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use adw::prelude::*;
use gtk::glib;
use gtk::graphene;
use gtk::subclass::prelude::*;
use octosnap_scene::redact::{self as ops, Inset, Pixels};
use octosnap_scene::{Bounds, Geometry, Object, ObjectId, RedactStyle, Scene};
use tracing::{debug, info, warn};

use super::Canvas;

/// `spec/05` §6: "redaction rasterization <= 50 ms for a 1000x600 region".
pub const BUDGET_MS: f64 = 50.0;

/// The most pixels one raster is allowed to be.
///
/// A redaction over a whole 5K canvas is 14.7 megapixels, and a Gaussian over that in
/// floating point is a quarter of a gigabyte of scratch. Above this the region is
/// rasterized at a lower scale and the GPU stretches it back -- a blur does not mind, and
/// a pixelation's blocks are dozens of pixels wide at that size anyway.
const MAX_PIXELS: f64 = 4_000_000.0;

/// One finished raster.
#[derive(Debug, Clone)]
pub struct Raster {
    /// The key of the inputs it was built from ([`key_for`]).
    pub key: u64,
    pub texture: gtk::gdk::Texture,
    /// Where it goes, in document units: the object's bounds snapped outward to the pixel
    /// grid it was rendered on, so the texture's pixels land on the canvas's.
    pub bounds: Bounds,
}

/// The rasters of every redaction in the document, and the bookkeeping around them.
#[derive(Default)]
pub struct Rasters {
    ready: RefCell<HashMap<ObjectId, Raster>>,
    /// The key each worker in flight is building for. One per object: a second request
    /// while one is running is answered by re-checking the key when it lands.
    pending: RefCell<HashMap<ObjectId, u64>>,
    /// The current key of every redaction, recomputed when the scene changes rather than
    /// on every frame -- hashing what lies beneath thirty objects per frame would eat the
    /// budget the cache exists to protect.
    keys: RefCell<HashMap<ObjectId, u64>>,
    /// Redactions a frame wanted a current raster for and did not have.
    wanted: RefCell<Vec<ObjectId>>,
    idle_scheduled: Cell<bool>,
    /// How many rasters have been built, for the log.
    built: Cell<u64>,
}

impl std::fmt::Debug for Rasters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rasters")
            .field("ready", &self.ready.borrow().len())
            .field("pending", &self.pending.borrow().len())
            .field("built", &self.built.get())
            .finish_non_exhaustive()
    }
}

impl Rasters {
    /// The raster to draw for `object` right now, and whether it is the current one.
    ///
    /// A stale raster is still handed back -- continuity beats a flicker to the preview
    /// and back -- but a want is noted so the idle replaces it. Nothing is noted while a
    /// worker is already building the current key.
    pub fn texture_for(&self, object: &Object) -> Option<(Raster, bool)> {
        let key = self.keys.borrow().get(&object.id).copied();
        let ready = self.ready.borrow().get(&object.id).cloned();
        let current = matches!((&ready, key), (Some(raster), Some(key)) if raster.key == key);
        if !current {
            let building = key.is_some_and(|k| self.pending.borrow().get(&object.id) == Some(&k));
            if !building {
                self.wanted.borrow_mut().push(object.id);
            }
        }
        ready.map(|raster| (raster, current))
    }

    /// Recomputes every redaction's key against the scene as it now is, and forgets the
    /// rasters of redactions that are no longer in it.
    pub fn refresh_keys(&self, scene: &Scene) {
        let mut keys = self.keys.borrow_mut();
        keys.clear();
        for object in scene.objects() {
            if matches!(object.geometry, Geometry::Redact { .. }) {
                keys.insert(object.id, key_for(object, scene));
            }
        }
        self.ready.borrow_mut().retain(|id, _| keys.contains_key(id));
    }

    /// A new document: nothing built for the old one applies.
    pub fn clear(&self) {
        self.ready.borrow_mut().clear();
        self.pending.borrow_mut().clear();
        self.keys.borrow_mut().clear();
        self.wanted.borrow_mut().clear();
    }

    /// Whether every redaction's raster is the one its inputs ask for.
    #[must_use]
    pub fn all_current(&self) -> bool {
        let ready = self.ready.borrow();
        self.keys.borrow().iter().all(|(id, key)| ready.get(id).is_some_and(|r| r.key == *key))
    }

    fn take_wanted(&self) -> Vec<ObjectId> {
        let mut wanted = std::mem::take(&mut *self.wanted.borrow_mut());
        wanted.sort_unstable();
        wanted.dedup();
        wanted
    }
}

/// Everything a redaction's picture depends on, folded into one number.
///
/// The object itself (bounds, style, intensity, seed), everything `spec/05` §5.3 puts
/// beneath it in the order it is drawn, the canvas rect (a crop that expands the canvas
/// changes what a background fills), and the base image. The objects go in as their
/// `objects.json`, which is the one serialisation the document already has and the one
/// §11 item 4 compares -- a field that mattered and was left out of it would be a bug in
/// the project format before it was a bug here.
fn key_for(object: &Object, scene: &Scene) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    hash_object(object, &mut hasher);
    for id in scene.beneath(object.id) {
        if let Some(below) = scene.get(id) {
            hash_object(below, &mut hasher);
        }
    }
    for value in [scene.canvas.x, scene.canvas.y, scene.canvas.width, scene.canvas.height] {
        value.to_bits().hash(&mut hasher);
    }
    scene.base.file.hash(&mut hasher);
    for value in [scene.base.width, scene.base.height, scene.base.scale] {
        value.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

fn hash_object(object: &Object, hasher: &mut impl Hasher) {
    match serde_json::to_string(object) {
        Ok(json) => json.hash(hasher),
        Err(_) => format!("{object:?}").hash(hasher),
    }
}

/// One raster's inputs, gathered on the main thread, run anywhere.
struct Job {
    key: u64,
    style: RedactStyle,
    intensity: f64,
    seed: u64,
    scale: f64,
    source: Pixels,
    inset: Inset,
    bounds: Bounds,
    /// The GPU render and read-back that produced `source`, for the log.
    render_ms: f64,
}

/// What a job produces.
struct Done {
    key: u64,
    style: RedactStyle,
    pixels: Pixels,
    bounds: Bounds,
    render_ms: f64,
    raster_ms: f64,
}

impl Job {
    fn run(self) -> Done {
        let started = std::time::Instant::now();
        let pixels = ops::rasterize(
            self.style,
            self.intensity,
            self.seed,
            self.scale,
            &self.source,
            self.inset,
        );
        Done {
            key: self.key,
            style: self.style,
            pixels,
            bounds: self.bounds,
            render_ms: self.render_ms,
            raster_ms: started.elapsed().as_secs_f64() * 1000.0,
        }
    }
}

/// A rectangle in whole physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PixelRect {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

impl PixelRect {
    /// `bounds` at `scale`, snapped *outward* so no pixel the region touches is missed.
    fn snap(bounds: Bounds, scale: f64) -> Self {
        let b = bounds.normalised();
        #[allow(clippy::cast_possible_truncation)]
        Self {
            x0: (b.x * scale).floor() as i64,
            y0: (b.y * scale).floor() as i64,
            x1: ((b.x + b.width) * scale).ceil() as i64,
            y1: ((b.y + b.height) * scale).ceil() as i64,
        }
    }

    fn inflated(self, by: i64) -> Self {
        Self { x0: self.x0 - by, y0: self.y0 - by, x1: self.x1 + by, y1: self.y1 + by }
    }

    fn intersect(self, other: Self) -> Self {
        Self {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        }
    }

    fn is_empty(self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    fn width(self) -> i64 {
        self.x1 - self.x0
    }

    fn height(self) -> i64 {
        self.y1 - self.y0
    }

    /// Back to document units.
    #[allow(clippy::cast_precision_loss)]
    fn to_bounds(self, scale: f64) -> Bounds {
        Bounds::new(
            self.x0 as f64 / scale,
            self.y0 as f64 / scale,
            self.width() as f64 / scale,
            self.height() as f64 / scale,
        )
    }
}

impl Canvas {
    /// Called at the end of every `snapshot`: if the frame wanted a raster it did not
    /// have, an idle after the frame goes and gets it. Nothing is rendered here -- this
    /// runs inside GTK's own paint.
    pub(super) fn schedule_rasters(&self) {
        let rasters = &self.imp().rasters;
        if rasters.wanted.borrow().is_empty() || rasters.idle_scheduled.replace(true) {
            return;
        }
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            let Some(canvas) = weak.upgrade() else { return };
            canvas.imp().rasters.idle_scheduled.set(false);
            for id in canvas.imp().rasters.take_wanted() {
                canvas.start_raster(id, false);
            }
        });
    }

    /// Before an export: every redaction's raster is the one its inputs ask for, built
    /// inline where it is not. Bottom-most first, in `spec/05` §5.3's own order, because a
    /// redaction above another samples the lower one's raster and not its preview.
    pub(super) fn ensure_rasters(&self, scene: &Scene) {
        for object in scene.render_list() {
            if matches!(object.geometry, Geometry::Redact { .. }) {
                self.start_raster(object.id, true);
            }
        }
    }

    /// Whether every redaction is showing its exact raster, for the harness.
    #[must_use]
    pub fn rasters_current(&self) -> bool {
        self.imp().rasters.all_current()
    }

    /// Builds one redaction's raster: on a worker, or inline when `inline`.
    ///
    /// Answers whether the raster is, or is on its way to being, current.
    fn start_raster(&self, id: ObjectId, inline: bool) -> bool {
        let Some(scene) = self.scene() else { return false };
        let Some(object) = scene.get(id).cloned() else { return false };
        let rasters = &self.imp().rasters;
        let Some(key) = rasters.keys.borrow().get(&id).copied() else { return false };
        if rasters.ready.borrow().get(&id).is_some_and(|raster| raster.key == key) {
            return true;
        }
        if !inline && rasters.pending.borrow().get(&id) == Some(&key) {
            return true;
        }
        let Some(job) = self.prepare(&object, &scene, key) else { return false };
        if inline {
            let done = job.run();
            self.install(id, done, true);
            return true;
        }
        rasters.pending.borrow_mut().insert(id, key);
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let outcome = gio::spawn_blocking(move || job.run()).await;
            let Some(canvas) = weak.upgrade() else { return };
            canvas.imp().rasters.pending.borrow_mut().remove(&id);
            if let Ok(done) = outcome {
                canvas.install(id, done, false);
            } else {
                warn!("a redaction's worker panicked; the preview stays");
            }
        });
        true
    }

    /// Puts a finished raster where the next frame will find it.
    fn install(&self, id: ObjectId, done: Done, inline: bool) {
        let rasters = &self.imp().rasters;
        // The scene may have moved on while the worker ran: the object gone, or its
        // inputs changed. A raster for an object that is gone is dropped; one for inputs
        // that have changed is installed anyway -- it is fresher than whatever is there --
        // and the next frame notes the mismatch and asks again.
        let Some(current) = rasters.keys.borrow().get(&id).copied() else {
            debug!("a redaction's raster landed after the redaction left");
            return;
        };
        let (width, height) = (done.pixels.width, done.pixels.height);
        let Ok((w, h)) = i32::try_from(width).and_then(|w| i32::try_from(height).map(|h| (w, h)))
        else {
            return;
        };
        if w == 0 || h == 0 {
            return;
        }
        let texture = gtk::gdk::MemoryTexture::new(
            w,
            h,
            gtk::gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(done.pixels.rgba),
            width * 4,
        );
        rasters.built.set(rasters.built.get() + 1);
        info!(
            style = done.style.label(),
            width,
            height,
            render_ms = format!("{:.1}", done.render_ms),
            raster_ms = format!("{:.1}", done.raster_ms),
            total_ms = format!("{:.1}", done.render_ms + done.raster_ms),
            budget_ms = format!("{BUDGET_MS:.1}"),
            inline,
            stale = done.key != current,
            "redaction rasterized"
        );
        rasters
            .ready
            .borrow_mut()
            .insert(id, Raster { key: done.key, texture: texture.upcast(), bounds: done.bounds });
        self.queue_draw();
    }

    /// Renders what lies beneath the redaction, clipped to its region plus the margin
    /// the style needs, and reads the pixels back.
    ///
    /// The GPU pass, on the main thread, because a renderer belongs to a surface. It is
    /// the same `append_document` the frame and the export use, on a scene holding only
    /// what `spec/05` §5.3 puts beneath this object -- so what a redaction hides is
    /// exactly what was drawn there, an earlier redaction's raster included.
    fn prepare(&self, object: &Object, scene: &Scene, key: u64) -> Option<Job> {
        let Geometry::Redact { bounds, style, intensity, seed } = object.geometry else {
            return None;
        };
        let started = std::time::Instant::now();
        let renderer = self.native().and_then(|native| native.renderer())?;

        // The base image's resolution, capped -- see `MAX_PIXELS`.
        let base_scale = scene.base.scale.max(f64::EPSILON);
        let area = bounds.normalised().width * bounds.normalised().height * base_scale * base_scale;
        let scale = if area > MAX_PIXELS { base_scale * (MAX_PIXELS / area).sqrt() } else { base_scale };

        let region = PixelRect::snap(bounds, scale);
        let canvas = PixelRect::snap(scene.canvas, scale);
        let margin = i64::try_from(ops::margin(style, intensity, scale)).unwrap_or(0);
        let source = region.inflated(margin).intersect(canvas);
        if source.is_empty() || region.is_empty() {
            return None;
        }
        let clamp = |v: i64| usize::try_from(v.max(0)).unwrap_or(0);
        let inset = Inset {
            left: clamp(region.x0 - source.x0),
            top: clamp(region.y0 - source.y0),
            right: clamp(source.x1 - region.x1),
            bottom: clamp(source.y1 - region.y1),
        };

        // Only what is beneath. `Scene::add` keeps the z order, so the walk below draws
        // them as the document does.
        let mut beneath = Scene::new(scene.base.clone());
        beneath.canvas = scene.canvas;
        for id in scene.beneath(object.id) {
            if let Some(below) = scene.get(id) {
                beneath.add(below.clone());
            }
        }

        let snapshot = gtk::Snapshot::new();
        #[allow(clippy::cast_possible_truncation)]
        snapshot.scale(scale as f32, scale as f32);
        let origin = source.to_bounds(scale);
        #[allow(clippy::cast_possible_truncation)]
        snapshot.translate(&graphene::Point::new(-origin.x as f32, -origin.y as f32));
        self.imp().append_document(&snapshot, &beneath);

        #[allow(clippy::cast_precision_loss)]
        let size = graphene::Rect::new(0.0, 0.0, source.width() as f32, source.height() as f32);
        let (width, height) = (clamp(source.width()), clamp(source.height()));
        let pixels = match snapshot.to_node() {
            Some(node) => {
                let texture = renderer.render_texture(&node, Some(&size));
                download(&texture, width, height)?
            }
            // Nothing drawn there at all: transparent, which is what the export shows.
            None => Pixels::new(width, height),
        };

        Some(Job {
            key,
            style,
            intensity,
            seed,
            scale,
            source: pixels,
            inset,
            bounds: region.to_bounds(scale),
            render_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    }
}

/// A texture's pixels as straight RGBA8 with no row padding.
fn download(texture: &gtk::gdk::Texture, width: usize, height: usize) -> Option<Pixels> {
    let (bytes, stride) = {
        let mut downloader = gtk::gdk::TextureDownloader::new(texture);
        downloader.set_format(gtk::gdk::MemoryFormat::R8g8b8a8);
        downloader.download_bytes()
    };
    let row = width.checked_mul(4)?;
    let mut rgba = Vec::with_capacity(row.checked_mul(height)?);
    for y in 0..height {
        let start = y.checked_mul(stride)?;
        rgba.extend_from_slice(bytes.get(start..start.checked_add(row)?)?);
    }
    Pixels::from_rgba(width, height, rgba)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use octosnap_scene::{Base, Rgba, Style};

    fn scene() -> Scene {
        Scene::new(Base::new("base.png", 800.0, 600.0, 1.0))
    }

    fn redaction(z: i32, seed: u64) -> Object {
        Object::new(
            z,
            0,
            Style::new(Rgba::new(0.0, 0.0, 0.0, 1.0), 2, false),
            Geometry::Redact {
                bounds: Bounds::new(10.0, 10.0, 100.0, 50.0),
                style: RedactStyle::Pixelate,
                intensity: 0.5,
                seed,
            },
        )
    }

    #[test]
    fn the_key_changes_with_the_object_and_with_what_is_beneath_it() {
        let mut s = scene();
        let low = redaction(1, 1);
        let high = redaction(2, 2);
        let (low_id, high_id) = (low.id, high.id);
        s.add(low);
        s.add(high);
        let before = key_for(s.get(high_id).unwrap(), &s);
        // Moving the *lower* redaction changes the upper one's key: §5.3 says it
        // rasterizes what is beneath it.
        s.get_mut(low_id).unwrap().translate(5.0, 0.0);
        let after = key_for(s.get(high_id).unwrap(), &s);
        assert_ne!(before, after);
        // And the lower one's own key did not depend on the upper.
        let low_before = key_for(s.get(low_id).unwrap(), &s);
        s.get_mut(high_id).unwrap().translate(5.0, 0.0);
        assert_eq!(low_before, key_for(s.get(low_id).unwrap(), &s));
    }

    #[test]
    fn the_key_changes_with_the_canvas() {
        let mut s = scene();
        let r = redaction(1, 1);
        let id = r.id;
        s.add(r);
        let before = key_for(s.get(id).unwrap(), &s);
        s.canvas = Bounds::new(-20.0, 0.0, 820.0, 600.0);
        assert_ne!(before, key_for(s.get(id).unwrap(), &s));
    }

    #[test]
    fn snapping_is_outward_and_intersections_clamp() {
        let px = PixelRect::snap(Bounds::new(10.4, 10.6, 99.2, 49.9), 2.0);
        assert_eq!((px.x0, px.y0, px.x1, px.y1), (20, 21, 220, 121));
        let canvas = PixelRect::snap(Bounds::new(0.0, 0.0, 100.0, 100.0), 2.0);
        let clipped = px.inflated(30).intersect(canvas);
        assert_eq!((clipped.x0, clipped.y0, clipped.x1, clipped.y1), (0, 0, 200, 151));
        assert!(PixelRect::snap(Bounds::new(0.0, 0.0, 0.0, 5.0), 1.0).is_empty());
        let back = px.to_bounds(2.0);
        assert!((back.x - 10.0).abs() < 1e-9 && (back.width - 100.0).abs() < 1e-9);
    }

    #[test]
    fn a_want_is_noted_once_per_frame_and_not_while_building() {
        let rasters = Rasters::default();
        let mut s = scene();
        let r = redaction(1, 1);
        let id = r.id;
        s.add(r);
        rasters.refresh_keys(&s);
        let key = *rasters.keys.borrow().get(&id).unwrap();
        assert!(rasters.texture_for(s.get(id).unwrap()).is_none());
        rasters.texture_for(s.get(id).unwrap());
        assert_eq!(rasters.take_wanted(), vec![id]);
        rasters.pending.borrow_mut().insert(id, key);
        assert!(rasters.texture_for(s.get(id).unwrap()).is_none());
        assert!(rasters.take_wanted().is_empty(), "wanted while a worker builds it");
        assert!(!rasters.all_current());
    }

    #[test]
    fn refreshing_the_keys_forgets_departed_redactions() {
        let rasters = Rasters::default();
        let mut s = scene();
        let r = redaction(1, 1);
        let id = r.id;
        s.add(r);
        rasters.refresh_keys(&s);
        assert_eq!(rasters.keys.borrow().len(), 1);
        s.remove(id);
        rasters.refresh_keys(&s);
        assert!(rasters.keys.borrow().is_empty());
        assert!(rasters.all_current(), "nothing to be current about");
    }
}
