// SPDX-License-Identifier: GPL-3.0-or-later

//! The canvas (`spec/05` §3, §6): a custom widget whose `snapshot()` emits render nodes.
//!
//! > The canvas is a custom `gtk::Widget` whose `snapshot()` emits **render nodes**;
//! > GTK's GPU renderer composites them. No Cairo, no Pillow, no per-frame pixel work.
//!
//! `spec/05` §6 is unusually prescriptive about this and says why at the end: the previous
//! attempt at this application was unusable because it did per-frame pixel work. So there
//! is one rule here and everything else follows from it -- **nothing in this file touches
//! a pixel.** It builds a tree, applies a transform, and hands it to GSK.
//!
//! The zoom and pan live on the widget rather than in the scene, and that is not an
//! accident of where they were convenient to put. A `Scene` is what gets saved
//! (`spec/05` §8) and what gets exported; how far the user happens to have zoomed in is
//! neither. Keeping them apart is what makes `spec/05` §11 item 2 -- an export at 2x
//! matching the canvas at 100 % -- a property rather than a coincidence: the export walks
//! the same tree with a different transform.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use gtk::graphene;
use gtk::pango;
use gtk::subclass::prelude::*;
use octosnap_scene::{Bounds, Geometry, Handle, Object, ObjectId, Point, Rgba, Scene};

use super::nodes;

/// `spec/05` §3's checkerboard, which shows where the capture is transparent -- a window
/// capture with its shadow cut out, or a canvas a crop has grown beyond the image.
const CHECKER: f32 = 8.0;

/// `spec/05` §9's zoom range. Beyond 800 % the render nodes are exact but a stroke is
/// wider than the viewport, and below 10 % a 5K capture is a postage stamp.
pub const ZOOM_MIN: f64 = 0.1;
pub const ZOOM_MAX: f64 = 8.0;

/// `spec/09` §3's "Editor | zoom | 160 ms | ease-out-cubic", anchored at the pointer.
const ZOOM_MS: u32 = 160;

/// `spec/09` §3's "Editor | tool switch, selection, handles | 80–120 ms | ease-out": a
/// new selection's outline and handles fade in over this.
const SELECTION_MS: u32 = 100;

/// The largest natural size the canvas will ask its window for.
///
/// Not a limit on the document -- a 5K capture opens at 5K and is *fitted*. This is only
/// what the window is sized from on first show, and a window larger than the display is
/// worse than a document shown smaller than 1:1.
const MAX_NATURAL: i32 = 1600;

/// `spec/05` §4.11's crop chrome, in widget pixels: the arm length and weight of an
/// L-shaped corner bracket, and the size of a bar handle at an edge midpoint.
///
/// Measured off `editor-crop-mode.png` rather than invented -- the screenshot is a 2x
/// capture, so its 30-pixel arms and 40x13 bars are these numbers.
const BRACKET: f64 = 15.0;
const BRACKET_WEIGHT: f64 = 3.0;
const EDGE_BAR: (f64, f64) = (20.0, 5.0);

/// `spec/09`: "handles 8 px white with accent border". Widget pixels, at every zoom.
const HANDLE: f64 = 8.0;

/// Whether an object's node may be kept between frames.
///
/// `spec/05` §6's cache says "each object builds a cached `RenderNode` when it changes",
/// and three kinds break the premise: their nodes do not depend only on themselves, so
/// "when it changes" is not a question about the object at all.
///
/// - A **spotlight** draws the union of every spotlight, once, because §5.3 makes them one
///   mask layer -- two overlapping ones must brighten one region rather than darken the
///   overlap twice. Its node changes when a *different* spotlight moves.
/// - A **redaction** "rasterizes everything below it at its z" (§5.3), so its node changes
///   when anything underneath it does.
/// - A **background** is the canvas's own fill, so its node changes when a crop changes the
///   canvas (§4.11, D52) -- which is not a change to the object.
///
/// Each could be cached with a wider key. None is worth it: there are at most a handful of
/// them in a document, and a key that has to summarise "everything below this" is a
/// correctness risk in exchange for a saving the measurement does not ask for.
const fn cacheable(geometry: &Geometry) -> bool {
    !matches!(
        geometry,
        Geometry::Spotlight { .. } | Geometry::Redact { .. } | Geometry::Background { .. }
    )
}

/// A scene colour as a GDK one.
fn to_gdk(color: Rgba) -> gtk::gdk::RGBA {
    #[allow(clippy::cast_possible_truncation)]
    gtk::gdk::RGBA::new(color.r as f32, color.g as f32, color.b as f32, color.a as f32)
}

/// `spec/09`'s `accent`: "GNOME accent colour … fallback #3584E4".
///
/// Read from libadwaita rather than hard-coded, because the fallback is only a fallback:
/// a user who has set their accent to purple expects the selection outline to be purple,
/// and `AdwStyleManager` is already tracking the setting `spec/09` names.
fn accent_colour() -> gtk::gdk::RGBA {
    adw::StyleManager::default().accent_color_rgba()
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct Canvas {
        pub scene: RefCell<Option<Scene>>,
        /// The capture, uploaded **once** (`spec/05` §6) and reused every frame.
        pub base: RefCell<Option<gtk::gdk::Texture>>,
        /// `spec/05` §4.13's Wallpapers, uploaded once each and kept by path.
        ///
        /// The `Option` inside is the point: a file that will not decode is remembered as
        /// a failure, so a background pointing at a deleted image costs one failed load
        /// rather than one per frame. Small -- the panel offers the user's own folder and
        /// a session touches a handful -- and cleared with the editor.
        pub wallpapers: RefCell<std::collections::HashMap<String, Option<gtk::gdk::Texture>>>,
        pub zoom: Cell<f64>,
        /// Canvas-space coordinate drawn at the widget's top-left.
        pub origin: Cell<(f64, f64)>,
        /// A document has been opened and not yet fitted to the viewport.
        ///
        /// The fit has to happen on the first allocation with a real size, and nothing
        /// earlier will do: `spec/05` §11 item 1 opens a 5120x2880 capture, and a fit
        /// computed against a zero-size widget answers 1:1 for everything.
        pub fit_pending: Cell<bool>,
        /// The view is still the fit -- nobody has zoomed or panned by hand.
        ///
        /// While this holds, a resize of the window re-fits the document to it, which is
        /// what a user who maximises the editor expects to see: the picture growing to
        /// use the room, centred. Once they have zoomed, their zoom is kept and only the
        /// centring is redone. Reported from hardware as "when it's open fullscreen,
        /// nothing is centred": the first version kept the origin fixed through a
        /// resize, so the document stayed parked in the top-left corner of a window
        /// three times its size.
        pub fitted: Cell<bool>,
        /// Told whenever the zoom changes, however it changes -- a wheel, a key, a refit
        /// on resize. The bottom bar's zoom readout is the listener, and it cannot poll.
        pub zoom_changed: Listener<dyn Fn(f64)>,
        /// `spec/09` §3's zoom while it plays, and the zoom it is heading for: a second
        /// wheel click steps on from where the first was going, not from wherever the
        /// animation had got to, so three quick clicks are three steps.
        pub zoom_animation: RefCell<Option<adw::TimedAnimation>>,
        pub zoom_goal: Cell<Option<f64>>,
        /// The selection chrome's opacity while it fades in, `None` once it is all there
        /// (`spec/09` §3), and the animation doing it.
        pub selection_fading: Cell<Option<f64>>,
        pub selection_animation: RefCell<Option<adw::TimedAnimation>>,
        /// Told whenever the selection changes. The options row is the listener: `spec/05`
        /// §2's row follows the selection (D56), so it has to know when there is one.
        pub selection_changed: Listener<dyn Fn()>,
        /// The last allocation reported, so the log line is emitted on change rather than
        /// on every frame clock tick.
        pub reported: Cell<(i32, i32, i32, i32)>,
        /// The base texture's pixels, downloaded on the first pipette click and kept.
        ///
        /// Downloading is the expensive part and it covers the whole texture -- 56 MB for
        /// a 5K capture -- so paying it once per document beats paying it per click.
        /// Cleared with the texture in `set_scene`, which is the only thing that can make
        /// it stale: annotations do not change the base.
        pub base_pixels: RefCell<Option<(glib::Bytes, usize)>>,
        /// The same bytes repacked to a tight stride, kept for the same reason again.
        ///
        /// `spec/05` §4.7's smart highlighter asks for them on **every motion event** of a
        /// stroke, and repacking 56 MB sixty times a second is not a thing to do twice.
        /// `Rc` so a caller can hold them without a copy; cleared beside the download.
        pub base_rgba: RefCell<Option<Rc<octosnap_scene::redact::Pixels>>>,
        /// The gesture in progress, previewed by being put *into* the scene.
        ///
        /// Not drawn on top as a special case, which was the obvious alternative and is
        /// wrong: `spec/05` §5.3 renders a spotlight above the base image and below the
        /// annotations, and a redaction samples what is beneath it. A draft drawn last
        /// would preview a spotlight dimming the wrong things and a pixelation sampling
        /// nothing -- so the preview would differ from the object the release commits,
        /// which is the one difference §6 exists to prevent. Cloning the scene for a
        /// frame is a `Vec` of objects and only happens while a button is down.
        pub draft: RefCell<Option<Object>>,
        pub selection: RefCell<Vec<ObjectId>>,
        /// `spec/05` §4.1's rubber band, in document coordinates.
        pub marquee: Cell<Option<Bounds>>,
        /// `spec/05` §4.14's drop zone under a file being dragged over the canvas, or
        /// `None` when nothing is. Chrome, not document: it never reaches the scene.
        pub drop_zone: Cell<Option<octosnap_scene::combine::Zone>>,
        /// `spec/05` §4.11's crop rect while crop mode is open, in document coordinates.
        ///
        /// Not in the scene, and that is what makes "Esc cancels" free: until Enter the
        /// document has not changed, so cancelling is clearing this.
        pub crop: Cell<Option<Bounds>>,
        /// The colour a crop expansion would be filled with, previewed under the
        /// document so what is on screen is what Enter will produce (`spec/05` §6).
        pub crop_fill: Cell<Option<Rgba>>,
        /// `spec/05` §6's per-object node cache: "each object builds a cached
        /// `RenderNode` when it changes; unchanged objects reuse their node".
        ///
        /// Keyed by id, holding the object it was built from beside the node, because
        /// "when it changes" has to be *answerable* and nothing in `objects.json` carries
        /// a revision. So the key is the object's own content: a hit is an id whose stored
        /// object still equals the one in the scene. Comparing a 320-point path costs a
        /// scan of a `Vec<Point>`, which is an order of magnitude less than building a
        /// `gsk::Path` from it -- and adding a revision field would have put a counter in
        /// the saved document, where §11 item 4 compares bytes.
        ///
        /// `None` for the node is cached too: an object that draws nothing is a question
        /// worth not asking twice.
        ///
        /// Deliberately **not** keyed on the zoom. The nodes are built in document
        /// coordinates and the transform is applied outside them, which is what makes the
        /// cache survive a pan and a zoom -- and is the same property that makes the
        /// export walk the same tree as the preview.
        pub cache: RefCell<std::collections::HashMap<ObjectId, (Object, Option<gtk::gsk::RenderNode>)>>,
        /// Cache hits and misses over the frames since the last reset.
        ///
        /// Reported beside the timing, because the timing alone cannot say *why*. A p95
        /// over budget with 29 hits and 1 miss per frame is a slow object; the same p95
        /// with 27 hits and 3 misses is the three kinds `cacheable` excludes, and they
        /// are different problems.
        pub hits: Cell<u64>,
        pub misses: Cell<u64>,
        /// `spec/05` §6's budget, as measured: "build <= 2 ms of nodes per frame during a
        /// drag on a 5120x2880 base".
        ///
        /// Timed here and nowhere else, because this is the thing the budget is about --
        /// the *node building*, not the frame. A frame also waits for the compositor, and
        /// `docs/spikes/16` records that a nested shell's timings are an order of
        /// magnitude off; this number is the app's own CPU work and is the same in either
        /// session.
        ///
        /// Bounded, so a session that draws for an hour holds one vector's worth. Reset
        /// through the `frame-stats` action, which is how a measurement gets bracketed
        /// around exactly one drag.
        pub build_ms: RefCell<Vec<f64>>,
        /// `spec/05` §4.11's detected background colour, measured once per document.
        ///
        /// `Some(None)` is a measurement that found nothing -- no texture -- and is
        /// cached like any other answer so a crop drag does not retry it every frame.
        pub border: Cell<Option<Option<Rgba>>>,
        /// `spec/05` §6's rasterized redactions and the worker behind them (`redact.rs`).
        pub rasters: crate::editor::redact::Rasters,
    }

    /// One optional callback, with a `Debug` that says whether it is installed --
    /// `derive(Debug)` on the widget cannot look inside a boxed closure.
    pub struct Listener<F: ?Sized>(pub RefCell<Option<Box<F>>>);

    impl<F: ?Sized> Default for Listener<F> {
        fn default() -> Self {
            Self(RefCell::new(None))
        }
    }

    impl<F: ?Sized> std::fmt::Debug for Listener<F> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Listener(installed: {})", self.0.borrow().is_some())
        }
    }

    impl Drop for Canvas {
        fn drop(&mut self) {
            // The imp struct is dropped at *finalize*, which is the moment the texture, the
            // node cache and the rasters are actually released. `dispose` above runs on
            // window destroy whether or not anybody still holds the widget; this line
            // does not.
            tracing::debug!("canvas finalized");
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Canvas {
        const NAME: &'static str = "OctosnapCanvas";
        type Type = super::Canvas;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Canvas {
        fn dispose(&self) {
            // The other half of `Editor::drop`'s line: an editor can be dropped while its
            // window -- and this canvas, with the capture's texture -- lives on in a
            // reference somebody else holds. `spec/05` §11 item 10 needs both.
            tracing::debug!(
                cached = self.cache.borrow().len(),
                rasters = ?self.rasters,
                "canvas disposed"
            );
        }

        fn constructed(&self) {
            self.parent_constructed();
            self.zoom.set(1.0);
            self.obj().set_overflow(gtk::Overflow::Hidden);
            self.obj().set_focusable(true);
        }
    }

    impl WidgetImpl for Canvas {
        /// The widget asks for the document's size at the current zoom.
        ///
        /// A natural size and not a minimum: the canvas has to be allowed to be smaller
        /// than its document, or opening a 5120x2880 capture would demand a window bigger
        /// than the screen. `spec/05` §11 item 1 opens exactly that capture.
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let zoom = self.zoom.get();
            let canvas = self.scene.borrow().as_ref().map_or(
                Bounds::new(0.0, 0.0, 0.0, 0.0),
                |s| s.canvas,
            );
            // Plus the crop rect, which `spec/05` §4.11 allows to reach outside the
            // canvas -- the same union `snapshot` clips to.
            let canvas = self.crop.get().map_or(canvas, |rect| canvas.union(rect));
            #[allow(clippy::cast_possible_truncation)]
            let document = match orientation {
                gtk::Orientation::Horizontal => (canvas.width * zoom).round() as i32,
                _ => (canvas.height * zoom).round() as i32,
            };
            // Capped, because this is what the window sizes itself from. `spec/05` §11
            // item 1 opens a 5120x2880 capture, and a natural size of 5120 asks for a
            // window wider than the screen -- which the compositor grants and the user
            // then has to resize before they can see the bottom of their own screenshot.
            // The document is fitted into whatever the window ends up being instead.
            (0, document.clamp(1, MAX_NATURAL), -1, -1)
        }

        /// The first allocation with a size is where an opened document gets fitted --
        /// and every later one keeps it in view.
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            if width > 0 && height > 0 {
                if self.fit_pending.get() || self.fitted.get() {
                    self.fit_pending.set(false);
                    self.obj().zoom_to_fit();
                } else {
                    // A hand-set zoom survives a resize; the document is re-centred or
                    // re-clamped inside the new viewport, and that is all.
                    self.constrain_origin();
                }
            }
            self.report_allocation(width, height);
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            // `spec/05` §6's budget is "build <= 2 ms of nodes per frame", so the whole
            // body is timed. `Instant` and not the frame clock: the clock measures the
            // frame, which includes waiting for the compositor, and the budget is about
            // this function.
            let started = std::time::Instant::now();
            self.build_nodes(snapshot);
            self.record_build(started.elapsed());
            // Outside the timing: it schedules, it does not render.
            self.obj().schedule_rasters();
        }

    }

    impl Canvas {
        /// Everything `snapshot` does, so the timing wraps exactly one thing.
        fn build_nodes(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (width, height) = (f64::from(widget.width()), f64::from(widget.height()));
            let _ = (width, height);

            let Some(scene) = self.scene.borrow().clone() else { return };
            // The checkerboard only inside the canvas -- see `draw_checkerboard`. Outside
            // it there is nothing to edit, so the surround is the widget's own CSS
            // background and not a pattern.
            //
            // While `spec/05` §4.11's crop is open the editable area is the canvas *plus*
            // the crop rect, because a crop dragged beyond the image expands the canvas
            // (§11 item 7, D52) and the area it will expand into has to be visible before
            // Enter rather than after it. One union decides the checkerboard, the clip and
            // the edge, so the three cannot disagree.
            let canvas = scene.canvas;
            let crop = self.crop.get();
            let visible = crop.map_or(canvas, |rect| canvas.union(rect));
            self.draw_checkerboard(snapshot, visible);
            let zoom = self.zoom.get();
            let (ox, oy) = self.origin.get();

            // Clipped to the canvas, because the canvas is what gets exported.
            //
            // Without this a stroke that strays off the picture is drawn on the desk at
            // full strength and then is not in the saved file -- the exact
            // preview-disagrees-with-export failure `spec/05` §6 is built to prevent, and
            // visible in the first screenshot taken after the checkerboard came out. The
            // user put the rule plainly: "the editor's only area of editing is the picture
            // itself". When §4.11's crop expands the canvas (D52) this clip widens with
            // it, and the area that was desk becomes editable -- one rect decides both.
            //
            // In widget space, before the transform, so the clip is the canvas rather than
            // something that moves with the zoom. The selection chrome is drawn *after*
            // the pop, so a handle on an object at the canvas edge stays whole.
            let clip = self.widget_rect(visible);
            snapshot.push_clip(&clip);

            // The expansion's fill, under everything the document draws.
            //
            // A preview of the background object that Enter will add, in the same place
            // and the same colour, so `spec/05` §6's "preview == export" holds across the
            // one edit that changes the canvas. Painted over the checkerboard rather than
            // instead of it: a transparent fill leaves the chequer showing, which is what
            // transparent looks like everywhere else in this editor.
            if let (Some(rect), Some(fill)) = (crop, self.crop_fill.get()) {
                snapshot.append_color(&to_gdk(fill), &self.widget_rect(rect));
            }

            // Everything the document draws happens inside this one transform, which is
            // what makes the export path identical: hand the same tree a different scale
            // and the result is the same picture at a different size.
            snapshot.save();
            #[allow(clippy::cast_possible_truncation)]
            snapshot.translate(&graphene::Point::new(
                (-ox * zoom) as f32,
                (-oy * zoom) as f32,
            ));
            #[allow(clippy::cast_possible_truncation)]
            snapshot.scale(zoom as f32, zoom as f32);

            // The draft joins the scene rather than being drawn after it -- see the
            // field's own note. `render_order` then places it exactly where the
            // committed object will land.
            let previewed = match self.draft.borrow().clone() {
                Some(draft) => {
                    let mut with_draft = scene.clone();
                    with_draft.add(draft);
                    with_draft
                }
                None => scene,
            };

            self.append_document(snapshot, &previewed);
            snapshot.restore();
            snapshot.pop();

            // Chrome is drawn *outside* the transform, in widget space, so a handle is
            // eight pixels at 10 % zoom and eight pixels at 800 %. Inside it, a handle
            // would be a speck on a zoomed-out document and cover the object on a zoomed-in
            // one -- and `spec/09` specifies it in pixels: "handles 8 px white with accent
            // border".
            // A hairline round the canvas, after the document and before the chrome.
            //
            // `spec/05` §1 says nothing about it, and the editor needs it for a reason the
            // user put plainly: "the editor's only area of editing is the picture itself".
            // Once the surround is a flat colour rather than a pattern, an image whose
            // edge happens to be the same tone has no visible boundary at all -- and after
            // a crop expands the canvas (D52) the editable area is *larger* than the
            // picture, which nothing else on screen would show.
            self.draw_canvas_edge(snapshot, visible);
            self.draw_selection(snapshot, &previewed);
            self.draw_marquee(snapshot);
            self.draw_drop_zone(snapshot);
            if let Some(rect) = crop {
                self.draw_crop(snapshot, rect);
            }
        }

        /// Puts the origin where the document is either centred or fills the viewport.
        ///
        /// Per axis: a document smaller than the viewport sits in the middle of it, and a
        /// larger one may be panned but never past its own edge -- so no drag can leave
        /// the picture stranded in a corner with desk on three sides, and a zoom anchored
        /// near an edge cannot walk it off-screen. Each axis is decided on its own, which
        /// is how a wide capture ends up centred vertically and scrollable horizontally.
        pub fn constrain_origin(&self) {
            let Some(canvas) = self.scene.borrow().as_ref().map(|s| s.canvas) else { return };
            let visible = self.crop.get().map_or(canvas, |rect| canvas.union(rect));
            let widget = self.obj();
            let zoom = self.zoom.get();
            let (vw, vh) = (f64::from(widget.width()) / zoom, f64::from(widget.height()) / zoom);
            if vw <= 0.0 || vh <= 0.0 {
                return;
            }
            let axis = |origin: f64, doc_start: f64, doc_extent: f64, view_extent: f64| {
                if doc_extent <= view_extent {
                    doc_start + (doc_extent - view_extent) / 2.0
                } else {
                    origin.clamp(doc_start, doc_start + doc_extent - view_extent)
                }
            };
            let (ox, oy) = self.origin.get();
            let constrained = (
                axis(ox, visible.x, visible.width, vw),
                axis(oy, visible.y, visible.height, vh),
            );
            if constrained != (ox, oy) {
                self.origin.set(constrained);
                widget.queue_draw();
            }
        }

        /// Says the zoom changed, to whoever asked to know.
        pub fn announce_zoom(&self) {
            if let Some(listener) = self.zoom_changed.0.borrow().as_ref() {
                listener(self.zoom.get());
            }
        }

        /// Keeps one frame's node-building time.
        fn record_build(&self, elapsed: std::time::Duration) {
            /// Ten seconds at 60 fps, which is longer than any drag a person makes in one
            /// go -- and bounded, because the alternative is a vector that grows for as
            /// long as the editor is open.
            const KEEP: usize = 600;
            let mut samples = self.build_ms.borrow_mut();
            if samples.len() >= KEEP {
                samples.remove(0);
            }
            samples.push(elapsed.as_secs_f64() * 1000.0);
        }

        /// Everything the document draws, in one place.
        ///
        /// Called by `snapshot` inside the zoom transform and by
        /// [`super::Canvas::render_texture`] inside the export transform, and that is the
        /// whole mechanism behind `spec/05` §6's "**preview == export by construction**".
        /// It is a construction rather than a claim only while there is exactly one
        /// function that draws a document; two functions that agree today are two that
        /// will disagree eventually, and §11 item 2 -- an export at 2x matching the canvas
        /// at 100 % -- is the test that would notice, months later.
        ///
        /// Text and the counter glyphs are here rather than in `nodes.rs` because a Pango
        /// layout needs a font map, which only a widget has. Text first, then the badges:
        /// §5.3 puts counters above everything. The honest consequence is that a text
        /// object draws above an annotation whose `z` is higher than its own -- the fix is
        /// for `append_scene` to take a glyph callback so one walk covers both, and it
        /// belongs with the Text *tool*, where there will be text worth ordering.
        pub fn append_document(&self, snapshot: &gtk::Snapshot, scene: &Scene) {
            nodes::append_scene(
                snapshot,
                scene,
                self.base.borrow().as_ref(),
                &|snapshot, object| self.draw_object(snapshot, object, scene),
            );
        }

        /// One object, from the cache when it can be.
        ///
        /// `spec/05` §6's cache, and the whole of it: a hit is an `append_node` of a tree
        /// built on some earlier frame, which is what the measurement in D54 says is
        /// needed -- dragging one of thirty objects rebuilt all thirty, every frame.
        ///
        /// Both paths draw the *same* thing, and that is not an accident of writing them
        /// alike: the miss path builds into a throwaway `Snapshot` with exactly the calls
        /// the uncached path used to make, keeps the node, and appends it. There is one
        /// sequence of drawing calls, so a cache hit cannot differ from a miss.
        fn draw_object(&self, snapshot: &gtk::Snapshot, object: &Object, scene: &Scene) {
            if !cacheable(&object.geometry) {
                // Counted as a miss, because that is what it costs: the three kinds
                // `cacheable` excludes are rebuilt on every frame by design.
                self.misses.set(self.misses.get().saturating_add(1));
                self.build_object(snapshot, object, scene);
                return;
            }
            if let Some((was, node)) = self.cache.borrow().get(&object.id)
                && was == object
            {
                self.hits.set(self.hits.get().saturating_add(1));
                if let Some(node) = node {
                    snapshot.append_node(node);
                }
                return;
            }
            self.misses.set(self.misses.get().saturating_add(1));
            let built = gtk::Snapshot::new();
            self.build_object(&built, object, scene);
            let node = built.to_node();
            if let Some(node) = &node {
                snapshot.append_node(node);
            }
            self.cache.borrow_mut().insert(object.id, (object.clone(), node));
        }

        /// The drawing itself: the shape, then its glyphs.
        ///
        /// Text and the counter glyphs are here rather than in `nodes.rs` because a Pango
        /// layout needs a font map, which only a widget has. Being in the same function as
        /// the shape is what makes a cached node whole -- a label's glyphs and the plate
        /// behind them are one node, not two things that can drift apart in `z`.
        fn build_object(&self, snapshot: &gtk::Snapshot, object: &Object, scene: &Scene) {
            // A redaction draws its raster where it has a current one, its stale one --
            // stretched to wherever the object now is -- while the worker catches up, and
            // the GPU preview when it has none (`redact.rs`).
            let raster = if matches!(object.geometry, Geometry::Redact { .. }) {
                self.rasters.texture_for(object).map(|(raster, current)| {
                    if current { raster } else { crate::editor::redact::Raster { bounds: object.bounds(), ..raster } }
                })
            } else {
                None
            };
            let base = self.base.borrow().clone();
            // §4.13's wallpaper, from the same cache whatever the frame: a background
            // whose file is missing draws nothing rather than being retried sixty times
            // a second, which is what `wallpaper` remembering its failures buys.
            let picture = match &object.geometry {
                Geometry::Background { params, .. } => match &params.background {
                    octosnap_scene::Background::Image { file } => self.wallpaper(file),
                    _ => None,
                },
                // §4.14's inserted image, through the same cache and for the same reason.
                Geometry::Image { file, .. } => self.wallpaper(file),
                _ => None,
            };
            nodes::append_object(
                snapshot,
                object,
                scene,
                base.as_ref(),
                raster.as_ref(),
                picture.as_ref(),
            );
            self.draw_glyphs(snapshot, object);
        }

        /// §4.13's wallpaper for a path, decoded once.
        pub fn wallpaper(&self, file: &str) -> Option<gtk::gdk::Texture> {
            if let Some(known) = self.wallpapers.borrow().get(file) {
                return known.clone();
            }
            let loaded = gtk::gdk::Texture::from_filename(file)
                .map_err(|why| {
                    tracing::warn!(file, "the background image would not load: {why}");
                })
                .ok();
            self.wallpapers.borrow_mut().insert(file.to_owned(), loaded.clone());
            loaded
        }

        /// Forgets the nodes of objects that are no longer in the scene.
        ///
        /// Called from `update_scene`, which is every edit. Without it the map is a leak
        /// with a slow fuse: `spec/05` §11 item 10 asks for "memory stays flat after
        /// opening/closing 20 editors", and a 320-point path's node is not small.
        pub fn evict(&self, scene: &Scene) {
            let mut cache = self.cache.borrow_mut();
            if cache.len() <= scene.len() {
                return;
            }
            cache.retain(|id, _| scene.get(*id).is_some());
        }

        /// The glyphs for one object, called from inside `append_scene`'s walk.
        ///
        /// Both kinds of glyph live here rather than in `nodes.rs` for one reason: a Pango
        /// layout needs a font map, and only a widget has one. Being *called from* the
        /// walk rather than after it is what keeps `spec/05` §5.3's order honest -- a
        /// rectangle whose `z` is above a label now draws above it.
        fn draw_glyphs(&self, snapshot: &gtk::Snapshot, object: &Object) {
            match &object.geometry {
                Geometry::Text { .. } => self.draw_text(snapshot, object),
                Geometry::Counter { .. } => self.draw_counter_glyph(snapshot, object),
                _ => {}
            }
        }

        /// `spec/05` §4.9's "white glyph" on each badge.
        ///
        /// Sized to fit rather than to a fixed fraction of the radius, because the four
        /// numbering systems produce wildly different widths: at badge 38 the Arabic
        /// system says `38` and the Roman one says `XXXVIII`, and a size that suits the
        /// first spills out of the circle on the second.
        fn draw_counter_glyph(&self, snapshot: &gtk::Snapshot, object: &Object) {
            let Geometry::Counter { center, number, style, radius } = &object.geometry else {
                return;
            };
            let text = style.format(*number);
            if text.is_empty() {
                return;
            }
            let layout = self.obj().create_pango_layout(Some(&text));
            let mut description = pango::FontDescription::new();
            description.set_family("sans");
            description.set_weight(pango::Weight::Bold);
            // A first pass at the size a single digit wants, then shrunk to the widest the
            // disc can hold: at badge 38 the Arabic system says `38` and the Roman one
            // says `XXXVIII`, and one size does not suit both.
            let nominal = radius * 1.25;
            #[allow(clippy::cast_possible_truncation)]
            description.set_absolute_size(nominal * f64::from(pango::SCALE));
            layout.set_font_description(Some(&description));

            let usable = radius * 1.5;
            let overflow = f64::from(layout.pixel_size().0) / usable;
            if overflow > 1.0 {
                #[allow(clippy::cast_possible_truncation)]
                description.set_absolute_size(nominal / overflow * f64::from(pango::SCALE));
                layout.set_font_description(Some(&description));
            }
            let (width, height) = {
                let (w, h) = layout.pixel_size();
                (f64::from(w), f64::from(h))
            };

            snapshot.save();
            #[allow(clippy::cast_possible_truncation)]
            snapshot.translate(&graphene::Point::new(
                (center.x - width / 2.0) as f32,
                (center.y - height / 2.0) as f32,
            ));
            // White, whatever the badge's colour: `spec/05` §4.9 says "drawn in the
            // current colour with a **white** glyph", so this one is not auto-contrasted.
            snapshot.append_layout(&layout, &gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 1.0));
            snapshot.restore();
        }

        /// Says where the canvas sits inside its window, whenever that changes.
        ///
        /// A client on Wayland can be told where its *window* is by the compositor, but
        /// nothing outside the process knows where the canvas is inside it -- and that
        /// depends on the header bar, the tool strip and `spec/05` §2's options row, all of
        /// which change height. `editor-test.sh` guessed "the window plus 100 px" and the
        /// options row moved the canvas past it, so three tools were driven at the toolbar
        /// and reported as broken. The same mistake as D50, in a different coordinate
        /// space: a hard-coded point that happened to be right once.
        fn report_allocation(&self, width: i32, height: i32) {
            let widget = self.obj();
            let Some(root) = widget.root() else { return };
            let Some(origin) =
                widget.compute_point(&root, &graphene::Point::new(0.0, 0.0))
            else {
                return;
            };
            #[allow(clippy::cast_possible_truncation)]
            let rect = (origin.x() as i32, origin.y() as i32, width, height);
            if self.reported.replace(rect) == rect {
                return;
            }
            tracing::debug!(
                x = rect.0,
                y = rect.1,
                width = rect.2,
                height = rect.3,
                "canvas allocated"
            );
        }

        /// A document point in widget space -- the inverse of [`super::Canvas::to_document`].
        fn to_widget(&self, point: Point) -> (f64, f64) {
            let zoom = self.zoom.get();
            let (ox, oy) = self.origin.get();
            ((point.x - ox) * zoom, (point.y - oy) * zoom)
        }

        fn widget_rect(&self, bounds: Bounds) -> graphene::Rect {
            let zoom = self.zoom.get();
            let (x, y) = self.to_widget(Point::new(bounds.x, bounds.y));
            #[allow(clippy::cast_possible_truncation)]
            graphene::Rect::new(
                x as f32,
                y as f32,
                (bounds.width * zoom) as f32,
                (bounds.height * zoom) as f32,
            )
        }

        /// What `spec/05` §4.13's background is doing to the picture, or nothing.
        ///
        /// The canvas has two coordinate spaces as soon as a background is applied: the
        /// *canvas* -- what gets exported, what a crop rect is in, what the checkerboard
        /// covers -- and the *picture*, which is where every object lives. This is the
        /// map between them, and the chrome has to go through it or a selection outline
        /// would be drawn where the object used to be.
        pub fn place(&self) -> octosnap_scene::Placement {
            self.scene
                .borrow()
                .as_ref()
                .map_or(octosnap_scene::Placement::NONE, Scene::placement)
        }

        /// [`Self::widget_rect`] for a rect in the *picture's* space: an object's bounds.
        fn picture_rect(&self, bounds: Bounds) -> graphene::Rect {
            self.widget_rect(self.place().apply_bounds(bounds))
        }

        /// [`Self::to_widget`] for a point in the *picture's* space.
        fn picture_point(&self, point: Point) -> (f64, f64) {
            self.to_widget(self.place().apply(point))
        }

        /// `spec/09`: "Selection chrome | accent 1 px + handles 8 px white with accent
        /// border".
        fn draw_selection(&self, snapshot: &gtk::Snapshot, scene: &Scene) {
            let selection = self.selection.borrow();
            if selection.is_empty() {
                return;
            }
            let fading = self.selection_fading.get();
            if let Some(opacity) = fading {
                snapshot.push_opacity(opacity);
            }
            self.draw_selection_chrome(snapshot, scene, &selection);
            if fading.is_some() {
                snapshot.pop();
            }
        }

        fn draw_selection_chrome(&self, snapshot: &gtk::Snapshot, scene: &Scene, selection: &[ObjectId]) {
            let accent = accent_colour();
            let white = gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 1.0);
            for id in selection.iter() {
                let Some(object) = scene.get(*id) else { continue };
                // `spec/05` §4.1 draws the outline around *resizable* objects and gives a
                // line or an arrow "2 end handles" instead. A box around a diagonal arrow
                // is mostly empty space and hides where the arrow actually is, which the
                // first screenshot with one selected made plain.
                let ends = matches!(object.grips(), octosnap_scene::Grips::Ends(_));
                if !ends {
                    let outline = self.picture_rect(self.obj().measured_bounds(object));
                    snapshot.append_border(
                        &gtk::gsk::RoundedRect::from_rect(outline, 0.0),
                        &[1.0; 4],
                        &[accent; 4],
                    );
                }
                for (index, point) in self.handles_for(object).into_iter().enumerate() {
                    #[allow(clippy::cast_possible_truncation)]
                    let rect = graphene::Rect::new(
                        (point.0 - HANDLE / 2.0) as f32,
                        (point.1 - HANDLE / 2.0) as f32,
                        HANDLE as f32,
                        HANDLE as f32,
                    );
                    let rounded = gtk::gsk::RoundedRect::from_rect(rect, (HANDLE / 2.0) as f32);
                    // The third handle of a curved arrow is its curve, not an end: filled
                    // in the accent so it reads as a different kind of grip (§4.1's "+1
                    // curve handle"), with a hairline back to the chord so it is visibly
                    // attached to the arrow it bends.
                    let curve = ends && index == 2;
                    if curve {
                        let points = object.points();
                        if let (Some(a), Some(b)) = (points.first(), points.get(1)) {
                            let (ax, ay) = self.picture_point(*a);
                            let (bx, by) = self.picture_point(*b);
                            let builder = gtk::gsk::PathBuilder::new();
                            #[allow(clippy::cast_possible_truncation)]
                            {
                                builder.move_to(((ax + bx) / 2.0) as f32, ((ay + by) / 2.0) as f32);
                                builder.line_to(point.0 as f32, point.1 as f32);
                            }
                            let hair = gtk::gsk::Stroke::new(1.0);
                            hair.set_dash(&[3.0, 3.0]);
                            snapshot.append_stroke(&builder.to_path(), &hair, &accent);
                        }
                    }
                    let (fill, edge) = if curve { (accent, white) } else { (white, accent) };
                    snapshot.push_rounded_clip(&rounded);
                    snapshot.append_color(&fill, &rect);
                    snapshot.pop();
                    snapshot.append_border(&rounded, &[1.0; 4], &[edge; 4]);
                }
            }
        }

        /// `spec/05` §4.1's handles, in widget space and in the order
        /// [`super::Canvas::grips_of`] answers -- so what is drawn is what is clickable.
        fn handles_for(&self, object: &Object) -> Vec<(f64, f64)> {
            self.obj()
                .grips_of(object)
                .into_iter()
                .map(|point| self.picture_point(point))
                .collect()
        }

        /// `spec/05` §4.1's rubber band: "Drag on empty space rubber-band selects".
        fn draw_marquee(&self, snapshot: &gtk::Snapshot) {
            let Some(bounds) = self.marquee.get() else { return };
            let rect = self.picture_rect(bounds);
            let accent = accent_colour();
            let mut fill = accent;
            fill.set_alpha(0.14);
            snapshot.append_color(&fill, &rect);
            snapshot.append_border(
                &gtk::gsk::RoundedRect::from_rect(rect, 0.0),
                &[1.0; 4],
                &[accent; 4],
            );
        }

        /// `spec/05` §4.14's drop zones, while a file is over the canvas.
        ///
        /// The whole picture tinted for a drop *onto* it, and one edge band for a stitch.
        /// Two different pictures on purpose: the difference between "this lands on top"
        /// and "this extends the canvas that way" is the only thing the user needs to know
        /// before letting go, and it has to be readable at a glance with a file icon under
        /// the cursor.
        fn draw_drop_zone(&self, snapshot: &gtk::Snapshot) {
            let Some(zone) = self.drop_zone.get() else { return };
            let Some(canvas) = self.scene.borrow().as_ref().map(|scene| scene.canvas) else {
                return;
            };
            let accent = accent_colour();
            let mut fill = accent;
            fill.set_alpha(0.16);
            let whole = self.widget_rect(canvas);
            if zone.is_edge() {
                snapshot.append_color(&fill, &self.widget_rect(zone.band(canvas)));
            } else {
                snapshot.append_color(&fill, &whole);
            }
            snapshot.append_border(
                &gtk::gsk::RoundedRect::from_rect(whole, 0.0),
                &[2.0; 4],
                &[accent; 4],
            );
        }

        /// `spec/05` §4.11's crop chrome: veil, thirds, brackets and bars.
        ///
        /// > The crop rect uses **L-shaped corner brackets and short bar handles at the
        /// > edge midpoints**, with rule-of-thirds guides inside and a flat grey veil
        /// > outside, lighter than the capture overlay's dim.
        ///
        /// Every part is deliberately *not* the selection chrome. §4.1's eight squares
        /// say "this object is selected and can be resized"; these say "this is the frame
        /// the picture will be cut to", and a crop UI that borrowed the selection handles
        /// would leave the user unable to tell which of the two they were looking at.
        ///
        /// The veil covers the whole widget outside the rect -- the desk as well as the
        /// part of the image being cropped away -- which is what `editor-crop-mode.png`
        /// shows: the excluded area and the surround are one uniform tone there, with no
        /// seam at the canvas edge. Grey rather than black and at a lower alpha than
        /// `spec/09`'s `dim` (0.38), so "lighter" holds in both senses.
        ///
        /// Drawn in widget space with the rest of the chrome, so a bracket is fifteen
        /// pixels at 10 % zoom and fifteen at 800 %.
        fn draw_crop(&self, snapshot: &gtk::Snapshot, crop: Bounds) {
            let widget = self.obj();
            let (w, h) = (f64::from(widget.width()), f64::from(widget.height()));
            let rect = self.widget_rect(crop);
            let veil = gtk::gdk::RGBA::new(0.5, 0.5, 0.5, 0.30);

            // Four rects around the crop instead of a mask: `push_mask` needs a whole
            // subtree and its own pop pair, and D33's lesson about `push_blend` is that a
            // pushed node whose contract is misread is worse than four rectangles.
            #[allow(clippy::cast_possible_truncation)]
            let (left, top) = (rect.x() as f64, rect.y() as f64);
            #[allow(clippy::cast_possible_truncation)]
            let (right, bottom) = (left + rect.width() as f64, top + rect.height() as f64);
            for around in [
                (0.0, 0.0, w, top),
                (0.0, bottom, w, h - bottom),
                (0.0, top, left, bottom - top),
                (right, top, w - right, bottom - top),
            ] {
                if around.2 <= 0.0 || around.3 <= 0.0 {
                    continue;
                }
                #[allow(clippy::cast_possible_truncation)]
                snapshot.append_color(
                    &veil,
                    &graphene::Rect::new(
                        around.0 as f32,
                        around.1 as f32,
                        around.2 as f32,
                        around.3 as f32,
                    ),
                );
            }

            // Rule-of-thirds guides, inside and hairline: they are a composition aid, so
            // they have to be visible without competing with the picture.
            let guide = gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 0.45);
            for third in [1.0 / 3.0, 2.0 / 3.0] {
                #[allow(clippy::cast_possible_truncation)]
                snapshot.append_color(
                    &guide,
                    &graphene::Rect::new(
                        (left + (right - left) * third) as f32,
                        top as f32,
                        1.0,
                        (bottom - top) as f32,
                    ),
                );
                #[allow(clippy::cast_possible_truncation)]
                snapshot.append_color(
                    &guide,
                    &graphene::Rect::new(
                        left as f32,
                        (top + (bottom - top) * third) as f32,
                        (right - left) as f32,
                        1.0,
                    ),
                );
            }

            // A hairline round the crop itself, so the frame reads as a frame even where
            // a bracket does not reach.
            snapshot.append_border(
                &gtk::gsk::RoundedRect::from_rect(rect, 0.0),
                &[1.0; 4],
                &[gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 0.55); 4],
            );

            let white = gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 1.0);
            // The brackets straddle the edge, half in and half out, which is what makes
            // them look like they are gripping the corner rather than drawn inside it.
            let half = BRACKET_WEIGHT / 2.0;
            for handle in Handle::CORNERS {
                let (dx, dy) = handle.direction();
                let at = self.to_widget(handle.position(crop));
                let (x, y) = (at.0, at.1);
                // Each arm runs from the corner *inwards*, so the sign of the direction
                // decides where the rect starts.
                let arm_x = if dx < 0 { x - half } else { x - BRACKET + half };
                let arm_y = if dy < 0 { y - half } else { y - BRACKET + half };
                #[allow(clippy::cast_possible_truncation)]
                snapshot.append_color(
                    &white,
                    &graphene::Rect::new(
                        arm_x as f32,
                        (y - half) as f32,
                        BRACKET as f32,
                        BRACKET_WEIGHT as f32,
                    ),
                );
                #[allow(clippy::cast_possible_truncation)]
                snapshot.append_color(
                    &white,
                    &graphene::Rect::new(
                        (x - half) as f32,
                        arm_y as f32,
                        BRACKET_WEIGHT as f32,
                        BRACKET as f32,
                    ),
                );
            }

            // The bar handles: long along the edge they sit on, so a horizontal edge gets
            // a horizontal bar. Rounded, which is what the screenshot shows and what
            // distinguishes them from a fragment of a bracket.
            for handle in Handle::EDGES {
                let (dx, _) = handle.direction();
                let (bw, bh) = if dx == 0 { EDGE_BAR } else { (EDGE_BAR.1, EDGE_BAR.0) };
                let at = self.to_widget(handle.position(crop));
                #[allow(clippy::cast_possible_truncation)]
                let bar = graphene::Rect::new(
                    (at.0 - bw / 2.0) as f32,
                    (at.1 - bh / 2.0) as f32,
                    bw as f32,
                    bh as f32,
                );
                #[allow(clippy::cast_possible_truncation)]
                let rounded =
                    gtk::gsk::RoundedRect::from_rect(bar, (EDGE_BAR.1 / 2.0) as f32);
                snapshot.push_rounded_clip(&rounded);
                snapshot.append_color(&white, &bar);
                snapshot.pop();
            }
        }

        /// `spec/05` §3's checkerboard, drawn in **widget** space so its squares stay the
        /// same size on screen at every zoom.
        ///
        /// A zooming checkerboard is the tell that it has been drawn in document space,
        /// and it reads as the transparency itself moving.
        /// `spec/05` §3's checkerboard -- **inside the canvas only**.
        ///
        /// It used to fill the whole viewport, and that was wrong in a way a screenshot
        /// showed and no test would: the user read it as a missing texture, which is
        /// exactly what an unexplained grey chequer looks like. The pattern means "this
        /// part of the canvas has nothing behind it", so painting it where there is no
        /// canvas at all says nothing true.
        ///
        /// It is not deleted, because it has two jobs still to come (D52): a window
        /// capture carries alpha where its shadow was cut out, and `spec/05` §4.11's crop
        /// dragged beyond the image *expands the canvas*, leaving real empty area. Today
        /// the canvas equals an opaque capture, so this draws nothing visible -- which is
        /// the requested outcome, arrived at by clipping rather than by removal.
        ///
        /// Drawn in **widget** space inside a clip, so the squares stay the same size on
        /// screen at every zoom; a pattern that scaled with the document would read as
        /// content.
        fn draw_checkerboard(&self, snapshot: &gtk::Snapshot, canvas: Bounds) {
            let rect = self.widget_rect(canvas);
            if rect.width() <= 0.0 || rect.height() <= 0.0 {
                return;
            }
            snapshot.push_clip(&rect);

            let light = gtk::gdk::RGBA::new(0.60, 0.60, 0.62, 1.0);
            let dark = gtk::gdk::RGBA::new(0.52, 0.52, 0.54, 1.0);
            snapshot.append_color(&light, &rect);

            // Anchored to the widget's origin rather than the canvas's, so the squares do
            // not crawl while the document is panned.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let first_col = (rect.x() / CHECKER).floor() as i32;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let first_row = (rect.y() / CHECKER).floor() as i32;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let cols = (rect.width() / CHECKER).ceil() as i32 + 2;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let rows = (rect.height() / CHECKER).ceil() as i32 + 2;
            for row in first_row..first_row + rows {
                for col in first_col..first_col + cols {
                    if (row + col) % 2 == 0 {
                        continue;
                    }
                    snapshot.append_color(
                        &dark,
                        &graphene::Rect::new(
                            col as f32 * CHECKER,
                            row as f32 * CHECKER,
                            CHECKER,
                            CHECKER,
                        ),
                    );
                }
            }
            snapshot.pop();
        }

        /// A hairline showing exactly where the editable area ends.
        fn draw_canvas_edge(&self, snapshot: &gtk::Snapshot, canvas: Bounds) {
            let rect = self.widget_rect(canvas);
            if rect.width() <= 0.0 || rect.height() <= 0.0 {
                return;
            }
            // Light-on-dark or dark-on-light, so the line survives either theme and either
            // image. Read from the style manager rather than fixed, for the same reason
            // the accent is.
            let edge = if adw::StyleManager::default().is_dark() {
                gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 0.18)
            } else {
                gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.22)
            };
            snapshot.append_border(
                &gtk::gsk::RoundedRect::from_rect(rect, 0.0),
                &[1.0; 4],
                &[edge; 4],
            );
        }

        /// Text, which needs a font map and therefore a widget.
        ///
        /// `nodes.rs` leaves its text arm empty rather than drawing an approximation,
        /// because an approximation would differ between the preview and the export --
        /// and `spec/05` §6's whole claim is that they cannot. So the layout is built
        /// here, from the widget's own Pango context, and appended in the scene's order.
        ///
        /// `spec/05` §6 also asks for the layout to be **cached** per object. It is not
        /// yet, and that is a deliberate order of work: the budget in §6 is "≤ 2 ms of
        /// nodes per frame during a drag on a 5120x2880 base", which is a thing to measure
        /// with the performance fixture rather than to guess at now.
        /// One text object, in `spec/05` §4.5's seven styles.
        ///
        /// Everything about the plate is measured from the layout rather than estimated,
        /// which is the whole reason text is drawn where a font map exists: §4.5 gives the
        /// padding and radius in **ems**, so they scale with the glyphs and a 10 pt label
        /// and a 288 pt one are recognisably the same style.
        fn draw_text(&self, snapshot: &gtk::Snapshot, object: &Object) {
            let Geometry::Text { pos, text, style, font_size, align, width } = &object.geometry
            else {
                return;
            };
            if text.is_empty() {
                return;
            }
            let layout = self.text_layout(text, *style, *font_size, *align, *width);
            let (w, h) = layout.pixel_size();

            // §4.5: box styles "pick the box colour from the text colour and auto-contrast
            // the text (white on dark, black on light)". So the user's one colour choice
            // becomes a plate, and the glyphs become whichever of black or white reads on
            // it. The plain styles put the colour on the glyphs instead.
            let (plate, ink) = if style.has_plate() {
                (Some(object.style.color), object.style.color.contrasting())
            } else {
                (None, object.style.color)
            };

            snapshot.save();
            #[allow(clippy::cast_possible_truncation)]
            snapshot.translate(&graphene::Point::new(pos.x as f32, pos.y as f32));

            if let Some(plate) = plate {
                #[allow(clippy::cast_possible_truncation)]
                let pad = (font_size * style.plate_padding_em()) as f32;
                #[allow(clippy::cast_possible_truncation)]
                let radius = (font_size * style.plate_radius_em()) as f32;
                #[allow(clippy::cast_precision_loss)]
                let rect = graphene::Rect::new(
                    -pad,
                    -pad,
                    w as f32 + 2.0 * pad,
                    h as f32 + 2.0 * pad,
                );
                let rounded = gtk::gsk::RoundedRect::from_rect(rect, radius);
                snapshot.push_rounded_clip(&rounded);
                snapshot.append_color(&to_gdk(plate), &rect);
                snapshot.pop();
            }

            if style.is_outlined() {
                // §4.5's "Outlined", as a real stroke of the glyph path. `add_layout`
                // turns a Pango layout into a `gsk::Path`, so the outline is the letters'
                // own outline -- not the usual trick of drawing the text eight times at
                // one-pixel offsets, which thickens badly at 288 pt and thins to nothing
                // at 10.
                let builder = gtk::gsk::PathBuilder::new();
                builder.add_layout(&layout);
                #[allow(clippy::cast_possible_truncation)]
                let spec = gtk::gsk::Stroke::new((font_size / 12.0).max(1.0) as f32);
                spec.set_line_join(gtk::gsk::LineJoin::Round);
                snapshot.append_stroke(&builder.to_path(), &spec, &to_gdk(ink));
            } else {
                snapshot.append_layout(&layout, &to_gdk(ink));
            }
            snapshot.restore();
        }

        /// The layout for a text object, built the same way wherever it is needed.
        ///
        /// Shared with the editing overlay, which has to be the same size and font as the
        /// text it is standing in for -- an overlay a few pixels off makes the glyphs jump
        /// the moment editing ends.
        pub fn text_layout(
            &self,
            text: &str,
            style: octosnap_scene::TextStyle,
            font_size: f64,
            align: octosnap_scene::TextAlign,
            width: Option<f64>,
        ) -> pango::Layout {
            let layout = self.obj().create_pango_layout(Some(text));
            layout.set_font_description(Some(&crate::editor::text::font_for(style, font_size)));
            layout.set_alignment(match align {
                octosnap_scene::TextAlign::Center => pango::Alignment::Center,
                octosnap_scene::TextAlign::End => pango::Alignment::Right,
                octosnap_scene::TextAlign::Start => pango::Alignment::Left,
            });
            if let Some(w) = width {
                #[allow(clippy::cast_possible_truncation)]
                layout.set_width((w * f64::from(pango::SCALE)) as i32);
                layout.set_wrap(pango::WrapMode::WordChar);
            }
            layout
        }
    }
}

glib::wrapper! {
    /// `spec/05` §3's canvas.
    pub struct Canvas(ObjectSubclass<imp::Canvas>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new()
    }
}

impl Canvas {
    #[must_use]
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    /// Opens a document, uploading its capture once.
    ///
    /// A failed texture load is not fatal: the objects still draw and the canvas still
    /// works, which is a far better answer for a spool file that has been deleted under
    /// the editor than refusing to open at all.
    pub fn set_scene(&self, scene: Scene) {
        let texture = gtk::gdk::Texture::from_filename(&scene.base.file)
            .inspect_err(|e| tracing::warn!(file = %scene.base.file, "no base texture: {e}"))
            .ok();
        let imp = self.imp();
        *imp.base.borrow_mut() = texture;
        *imp.base_pixels.borrow_mut() = None;
        *imp.base_rgba.borrow_mut() = None;
        imp.cache.borrow_mut().clear();
        imp.rasters.clear();
        imp.rasters.refresh_keys(&scene);
        imp.border.set(None);
        imp.crop.set(None);
        imp.crop_fill.set(None);
        *imp.scene.borrow_mut() = Some(scene);
        imp.fit_pending.set(true);
        self.queue_resize();
    }

    #[must_use]
    pub fn scene(&self) -> Option<Scene> {
        self.imp().scene.borrow().clone()
    }

    /// Replaces the document, keeping the view where it is.
    ///
    /// Every edit goes through here, and the zoom and origin are deliberately untouched:
    /// a canvas that jumped back to 100 % because a rectangle was drawn would be unusable.
    pub fn update_scene(&self, scene: Scene) {
        let resized = self
            .imp()
            .scene
            .borrow()
            .as_ref()
            .is_none_or(|old| old.canvas != scene.canvas);
        // Every redaction's key against the document as it now is, so the next frame
        // knows which rasters still apply (`redact.rs`).
        self.imp().rasters.refresh_keys(&scene);
        *self.imp().scene.borrow_mut() = Some(scene);
        // The cached nodes of objects that have gone. A `Remove`, an undo of an `Add` and
        // `spec/05` §4.1's Delete all arrive here.
        if let Some(scene) = self.imp().scene.borrow().as_ref() {
            self.imp().evict(scene);
        }
        if resized {
            self.queue_resize();
        } else {
            self.queue_draw();
        }
    }

    #[must_use]
    pub fn zoom(&self) -> f64 {
        self.imp().zoom.get()
    }

    /// Zooms, keeping `anchor` -- a **widget**-space point -- over the same document
    /// pixel.
    ///
    /// `spec/05` §10: "Zoom (wheel/keys) … anchored at pointer". Anchoring is the whole
    /// behaviour: zooming about the widget's origin instead walks the document off the
    /// top-left corner, and the user chases it with the scrollbars.
    pub fn zoom_to(&self, zoom: f64, anchor: (f64, f64)) {
        let imp = self.imp();
        let old = imp.zoom.get();
        let new = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        if (new - old).abs() < f64::EPSILON {
            return;
        }
        // The document point under the anchor before the change has to be under it after.
        let (ox, oy) = imp.origin.get();
        let doc_x = ox + anchor.0 / old;
        let doc_y = oy + anchor.1 / old;
        imp.zoom.set(new);
        imp.origin.set((doc_x - anchor.0 / new, doc_y - anchor.1 / new));
        // A hand-set zoom: from here on a resize keeps it (see `fitted`).
        imp.fitted.set(false);
        imp.constrain_origin();
        imp.announce_zoom();
        self.queue_draw();
    }

    /// Pans by a delta in **widget** space, which is what a drag produces.
    pub fn pan_by(&self, dx: f64, dy: f64) {
        let imp = self.imp();
        let zoom = imp.zoom.get();
        let (ox, oy) = imp.origin.get();
        imp.origin.set((ox - dx / zoom, oy - dy / zoom));
        imp.fitted.set(false);
        imp.constrain_origin();
        self.queue_draw();
    }

    /// [`Canvas::zoom_to`] over `spec/09` §3's 160 ms instead of at once, for the wheel's
    /// clicks, the keys and the zoom menu. A touchpad's continuous zoom stays direct: it
    /// follows the fingers, and an animation would trail them. `each_frame` runs after
    /// every step, for what has to follow the view, such as text being typed.
    pub fn zoom_smoothly(&self, zoom: f64, anchor: (f64, f64), each_frame: impl Fn() + 'static) {
        let to = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        self.glide(to, each_frame, move |canvas, zoom| canvas.zoom_to(zoom, anchor));
    }

    /// Where the zoom is heading: a running animation's goal, or the zoom itself.
    #[must_use]
    pub fn zoom_goal(&self) -> f64 {
        self.imp().zoom_goal.get().unwrap_or_else(|| self.zoom())
    }

    /// `Ctrl+0` and `Ctrl+1` as the user asks for them, over the same 160 ms: the zoom
    /// runs to the fit or to 1:1 and the view slides to the centre as it goes, landing
    /// exactly where [`Canvas::zoom_to_fit`] and [`Canvas::zoom_to_actual`] would. Those
    /// two stay instant, because a resize refits through them on every allocation.
    pub fn glide_to_fit(&self, each_frame: impl Fn() + 'static) {
        let Some(to) = self.fit_zoom() else { return };
        self.glide_centred(to, true, each_frame);
    }

    /// See [`Canvas::glide_to_fit`].
    pub fn glide_to_actual(&self, each_frame: impl Fn() + 'static) {
        self.glide_centred(1.0, false, each_frame);
    }

    /// The zoom that fits the document to the widget, when there is a document and a size.
    fn fit_zoom(&self) -> Option<f64> {
        let scene = self.imp().scene.borrow().clone()?;
        let (w, h) = (f64::from(self.width()), f64::from(self.height()));
        if w <= 0.0 || h <= 0.0 || scene.canvas.width <= 0.0 || scene.canvas.height <= 0.0 {
            return None;
        }
        // Never *up* to fit: `spec/05` §9 pairs `Ctrl+0` with `Ctrl+1` for 1:1, so "fit"
        // means "make it visible", and blowing a 200x100 capture up to fill a 4K window
        // is not that.
        let scale = (w / scene.canvas.width).min(h / scene.canvas.height).min(1.0);
        Some(scale.clamp(ZOOM_MIN, ZOOM_MAX))
    }

    fn glide_centred(&self, to: f64, fitted: bool, each_frame: impl Fn() + 'static) {
        let from_origin = self.imp().origin.get();
        let from_zoom = self.imp().zoom.get();
        let Some(end) = self.centred_origin(to) else { return };
        self.glide(to, each_frame, move |canvas, zoom| {
            // How far along the zoom is, which is how far along the slide goes: the two
            // arrive together whatever the curve.
            let t = if (to - from_zoom).abs() < f64::EPSILON {
                1.0
            } else {
                (zoom - from_zoom) / (to - from_zoom)
            };
            let imp = canvas.imp();
            imp.zoom.set(zoom);
            imp.origin.set((
                from_origin.0 + (end.0 - from_origin.0) * t,
                from_origin.1 + (end.1 - from_origin.1) * t,
            ));
            imp.fitted.set(false);
            imp.constrain_origin();
            imp.announce_zoom();
            canvas.queue_draw();
        });
        // The flag the instant versions set, once the glide is under way: set before it,
        // the first frame's `fitted = false` would take it straight back.
        if let Some(animation) = self.imp().zoom_animation.borrow().as_ref() {
            let weak = self.downgrade();
            animation.connect_done(move |_| {
                if let Some(canvas) = weak.upgrade() {
                    canvas.imp().fitted.set(fitted);
                }
            });
        } else {
            self.imp().fitted.set(fitted);
        }
    }

    /// The origin that centres the document at `zoom`, as [`Canvas::center`] sets it.
    fn centred_origin(&self, zoom: f64) -> Option<(f64, f64)> {
        let scene = self.imp().scene.borrow().clone()?;
        let visible_w = f64::from(self.width()) / zoom;
        let visible_h = f64::from(self.height()) / zoom;
        Some((
            scene.canvas.x + (scene.canvas.width - visible_w) / 2.0,
            scene.canvas.y + (scene.canvas.height - visible_h) / 2.0,
        ))
    }

    /// Runs the zoom from where it is to `to`, calling `step` with each value on the way.
    ///
    /// A glide already running stops where it is and this one starts from there. With
    /// animations off `step` is called once, with `to`. The animation holds the canvas
    /// weakly, as every libadwaita animation holds its widget, so an editor closed
    /// mid-zoom goes with its canvas.
    fn glide(
        &self,
        to: f64,
        each_frame: impl Fn() + 'static,
        step: impl Fn(&Self, f64) + 'static,
    ) {
        self.stop_glide();
        let imp = self.imp();
        let from = imp.zoom.get();
        if !adw::is_animations_enabled(self) || !self.is_mapped() {
            step(self, to);
            each_frame();
            return;
        }
        let weak = self.downgrade();
        let target = adw::CallbackAnimationTarget::new(move |value| {
            if let Some(canvas) = weak.upgrade() {
                step(&canvas, value);
                each_frame();
            }
        });
        let animation = adw::TimedAnimation::builder()
            .widget(self)
            .value_from(from)
            .value_to(to)
            .duration(ZOOM_MS)
            .easing(adw::Easing::EaseOutCubic)
            .target(&target)
            .build();
        {
            let weak = self.downgrade();
            animation.connect_done(move |_| {
                if let Some(canvas) = weak.upgrade() {
                    canvas.imp().zoom_goal.set(None);
                }
            });
        }
        imp.zoom_goal.set(Some(to));
        *imp.zoom_animation.borrow_mut() = Some(animation.clone());
        animation.play();
    }

    /// Stops a glide where it is, for anything that sets the view itself.
    fn stop_glide(&self) {
        let imp = self.imp();
        let running = imp.zoom_animation.borrow_mut().take();
        if let Some(animation) = running {
            animation.pause();
        }
        imp.zoom_goal.set(None);
    }

    /// `spec/05` §9's `Ctrl+0`: fit the document to the widget.
    pub fn zoom_to_fit(&self) {
        let Some(scale) = self.fit_zoom() else { return };
        self.stop_glide();
        self.imp().zoom.set(scale);
        self.imp().fitted.set(true);
        self.center();
        self.imp().announce_zoom();
    }

    /// `spec/05` §9's `Ctrl+1`: 1:1.
    pub fn zoom_to_actual(&self) {
        self.stop_glide();
        self.imp().zoom.set(1.0);
        self.imp().fitted.set(false);
        self.center();
        self.imp().announce_zoom();
    }

    /// Whether the view is still the fit -- see the `fitted` field.
    #[must_use]
    pub fn is_fitted(&self) -> bool {
        self.imp().fitted.get()
    }

    /// Installs the zoom readout's listener. One, because there is one readout.
    pub fn connect_zoom_changed(&self, listener: impl Fn(f64) + 'static) {
        *self.imp().zoom_changed.0.borrow_mut() = Some(Box::new(listener));
    }

    /// Installs the options row's listener. One, because there is one row.
    pub fn connect_selection_changed(&self, listener: impl Fn() + 'static) {
        *self.imp().selection_changed.0.borrow_mut() = Some(Box::new(listener));
    }

    /// Puts the document's middle in the widget's middle.
    pub fn center(&self) {
        let Some(scene) = self.imp().scene.borrow().clone() else { return };
        let imp = self.imp();
        let zoom = imp.zoom.get();
        let visible_w = f64::from(self.width()) / zoom;
        let visible_h = f64::from(self.height()) / zoom;
        imp.origin.set((
            scene.canvas.x + (scene.canvas.width - visible_w) / 2.0,
            scene.canvas.y + (scene.canvas.height - visible_h) / 2.0,
        ));
        // `queue_draw`, not `queue_resize`: this runs from `size_allocate` on every
        // resize while the view is fitted, and a resize queued from inside an allocation
        // is a layout loop. The natural size the zoom feeds into `measure` only matters
        // before the window has a size, which is the one allocation this is not.
        self.queue_draw();
    }

    /// Shows or clears the gesture in progress.
    ///
    /// Takes the whole object rather than a tool and two points, because the object is
    /// what `scene::Draft` already answers with -- and handing the canvas anything less
    /// would mean a second place that knows how a rectangle is built from a drag.
    pub fn set_draft(&self, draft: Option<Object>) {
        let changed = *self.imp().draft.borrow() != draft;
        if changed {
            *self.imp().draft.borrow_mut() = draft;
            self.queue_draw();
        }
    }

    pub fn set_selection(&self, selection: Vec<ObjectId>) {
        if *self.imp().selection.borrow() != selection {
            // Only what was not selected before fades in: something from nothing, or one
            // object for another. A rubber band gathering objects one by one keeps what it
            // has, so its chrome does not blink each time it grows.
            let appearing = {
                let before = self.imp().selection.borrow();
                !selection.is_empty() && !selection.iter().any(|id| before.contains(id))
            };
            *self.imp().selection.borrow_mut() = selection;
            if appearing {
                self.fade_in_selection();
            }
            self.queue_draw();
            self.report_grips();
            // Told last, after the canvas agrees with itself: the listener rebuilds the
            // options row from the selection and reads it back through `selection()`.
            if let Some(listener) = self.imp().selection_changed.0.borrow().as_ref() {
                listener();
            }
        }
    }

    /// `spec/09` §3's selection row: the outline and handles fade in over 100 ms. With
    /// animations off they are simply there.
    fn fade_in_selection(&self) {
        let imp = self.imp();
        let running = imp.selection_animation.borrow_mut().take();
        if let Some(animation) = running {
            animation.pause();
        }
        imp.selection_fading.set(None);
        if !adw::is_animations_enabled(self) || !self.is_mapped() {
            return;
        }
        imp.selection_fading.set(Some(0.0));
        let weak = self.downgrade();
        let target = adw::CallbackAnimationTarget::new(move |value| {
            if let Some(canvas) = weak.upgrade() {
                canvas.imp().selection_fading.set((value < 1.0).then_some(value));
                canvas.queue_draw();
            }
        });
        let animation = adw::TimedAnimation::builder()
            .widget(self)
            .value_from(0.0)
            .value_to(1.0)
            .duration(SELECTION_MS)
            .easing(adw::Easing::EaseOutCubic)
            .target(&target)
            .build();
        *imp.selection_animation.borrow_mut() = Some(animation.clone());
        animation.play();
    }

    /// Says where the selected object's handles are, whenever the selection changes.
    ///
    /// In **widget** coordinates, and for the same reason `canvas allocated` and
    /// `crop rect` are logged: a handle's position on screen depends on the zoom, the pan
    /// and -- for a text run -- on the font map, so it cannot be worked out from outside
    /// the process. `editor-test.sh` drove one from arithmetic once and that is D50's
    /// mistake; the app is the only thing that knows.
    ///
    /// Only for a single selection, which is the only case §4.1 draws handles for.
    fn report_grips(&self) {
        let selection = self.imp().selection.borrow().clone();
        let [id] = selection[..] else { return };
        let Some(scene) = self.imp().scene.borrow().clone() else { return };
        let Some(object) = scene.get(id) else { return };
        let count = self.grips_of(object).len();
        let box_of = self.measured_bounds(object);
        let (x, y) = self.to_widget(Point::new(box_of.x, box_of.y));
        let zoom = self.imp().zoom.get();
        #[allow(clippy::cast_possible_truncation)]
        let (wx, wy) = (x.round() as i64, y.round() as i64);
        #[allow(clippy::cast_possible_truncation)]
        let (ww, wh) =
            ((box_of.width * zoom).round() as i64, (box_of.height * zoom).round() as i64);
        tracing::debug!(grips = count, kind = ?object.kind(), wx, wy, ww, wh, "selection grips");
    }

    #[must_use]
    pub fn selection(&self) -> Vec<ObjectId> {
        self.imp().selection.borrow().clone()
    }

    /// `spec/05` §4.1's rubber band, in document coordinates, or `None` to clear it.
    /// `spec/05` §4.14's drop zone, or `None` to take the chrome away.
    pub fn set_drop_zone(&self, zone: Option<octosnap_scene::combine::Zone>) {
        if self.imp().drop_zone.get() == zone {
            return;
        }
        self.imp().drop_zone.set(zone);
        self.queue_draw();
    }

    pub fn set_marquee(&self, marquee: Option<Bounds>) {
        self.imp().marquee.set(marquee);
        self.queue_draw();
    }

    /// `spec/05` §6's export: the same node tree, offscreen, at `scale`.
    ///
    /// > **Export** = render the same node tree offscreen with
    /// > `gsk::Renderer::render_texture(node, bounds)` at `export_scale`, then encode.
    /// > Preview == export by construction.
    ///
    /// The construction is `append_document`, which this and `snapshot` both call. What
    /// differs is only the transform: the canvas applies zoom and pan, and this applies
    /// `scale` and moves the canvas's own origin to zero -- because `Scene::canvas` is
    /// what gets exported and a crop can have moved it away from the base image's corner
    /// (D52).
    ///
    /// The draft and the selection chrome are deliberately absent. A gesture in progress
    /// is not part of the document, and `spec/05` §5.3 ends "Selection chrome (**never
    /// exported**)".
    ///
    /// `None` when the window is not realised: `render_texture` needs the surface's
    /// renderer, and an editor that has not been shown has no surface. Every caller is a
    /// button in that window.
    #[must_use]
    pub fn render_texture(&self, scale: f64) -> Option<gtk::gdk::Texture> {
        let scene = self.imp().scene.borrow().clone()?;
        // An export cannot wait for a worker: every redaction gets its exact raster now.
        self.ensure_rasters(&scene);
        let canvas = scene.canvas;
        if canvas.width <= 0.0 || canvas.height <= 0.0 {
            return None;
        }
        let renderer = self.native().and_then(|native| native.renderer())?;

        let snapshot = gtk::Snapshot::new();
        // Scale, then move the canvas's origin to zero -- in that order, because a
        // translation applied after a scale is itself scaled. That makes this exactly
        // `(document - origin) * scale` rather than a rounding of it.
        #[allow(clippy::cast_possible_truncation)]
        snapshot.scale(scale as f32, scale as f32);
        #[allow(clippy::cast_possible_truncation)]
        snapshot.translate(&graphene::Point::new(-canvas.x as f32, -canvas.y as f32));
        self.imp().append_document(&snapshot, &scene);

        // `None` for a document with nothing in it at all -- a scene whose texture failed
        // to load and which has no objects yet. Not an error; nothing to encode.
        let node = snapshot.to_node()?;
        // The texture is exactly `export_size`: a renderer rounds fractional bounds *up*,
        // so a canvas of 562.5 would come out a row taller than the size every label and
        // filename says it is.
        let (width, height) = self.export_size(scale)?;
        #[allow(clippy::cast_precision_loss)]
        let bounds = graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
        Some(renderer.render_texture(&node, Some(&bounds)))
    }

    /// The document's size in pixels at `scale`, which is what an export will be.
    #[must_use]
    pub fn export_size(&self, scale: f64) -> Option<(i32, i32)> {
        let scene = self.imp().scene.borrow().clone()?;
        #[allow(clippy::cast_possible_truncation)]
        Some((
            (scene.canvas.width * scale).round() as i32,
            (scene.canvas.height * scale).round() as i32,
        ))
    }

    /// The base image's colour at a document point, for `spec/05` §2's pipette.
    ///
    /// The **base image**, not the composited canvas: picking a colour out of the
    /// screenshot is what the pipette is for, and compositing the annotations first would
    /// mean rendering the scene to a texture -- which is the export path, and is not built
    /// yet. So a colour picked from *on top of* a rectangle you drew answers the pixel
    /// underneath it. Worth knowing; not worth blocking the pipette on.
    ///
    /// Downloaded as `R8g8b8a8`, which is straight RGBA rather than Cairo's premultiplied
    /// ARGB32 -- so the channels need no dividing by alpha, and a window capture's
    /// transparent border does not read as black.
    #[must_use]
    pub fn sample(&self, at: Point) -> Option<Rgba> {
        let imp = self.imp();
        let texture = imp.base.borrow().clone()?;
        let scene = imp.scene.borrow().clone()?;
        let origin = scene.base.bounds();
        #[allow(clippy::cast_possible_truncation)]
        let x = (at.x - origin.x).floor() as i32;
        #[allow(clippy::cast_possible_truncation)]
        let y = (at.y - origin.y).floor() as i32;
        if x < 0 || y < 0 || x >= texture.width() || y >= texture.height() {
            return None;
        }

        if imp.base_pixels.borrow().is_none() {
            let mut downloader = gtk::gdk::TextureDownloader::new(&texture);
            downloader.set_format(gtk::gdk::MemoryFormat::R8g8b8a8);
            *imp.base_pixels.borrow_mut() = Some(downloader.download_bytes());
        }
        let borrowed = imp.base_pixels.borrow();
        let (bytes, stride) = borrowed.as_ref()?;
        let offset = usize::try_from(y).ok()?.checked_mul(*stride)?
            + usize::try_from(x).ok()?.checked_mul(4)?;
        let pixel = bytes.get(offset..offset + 4)?;
        Some(Rgba::new(
            f64::from(pixel[0]) / 255.0,
            f64::from(pixel[1]) / 255.0,
            f64::from(pixel[2]) / 255.0,
            f64::from(pixel[3]) / 255.0,
        ))
    }

    /// The capture's own texture, for `spec/05` §4.13's three blurred swatches.
    #[must_use]
    pub fn base_texture(&self) -> Option<gtk::gdk::Texture> {
        self.imp().base.borrow().clone()
    }

    /// The base image as `spec/05` §4.13's Auto Balance wants it: whole, RGBA, in rows.
    ///
    /// Downloaded once and shared with the pipette's cache, because both want the same
    /// bytes and a 5K capture is 56 MB of them. `None` before the texture has been
    /// uploaded, which is the first few frames of a freshly opened editor.
    #[must_use]
    pub fn base_pixels(&self) -> Option<Rc<octosnap_scene::redact::Pixels>> {
        let imp = self.imp();
        if let Some(kept) = imp.base_rgba.borrow().clone() {
            return Some(kept);
        }
        let texture = imp.base.borrow().clone()?;
        if imp.base_pixels.borrow().is_none() {
            let mut downloader = gtk::gdk::TextureDownloader::new(&texture);
            downloader.set_format(gtk::gdk::MemoryFormat::R8g8b8a8);
            *imp.base_pixels.borrow_mut() = Some(downloader.download_bytes());
        }
        let borrowed = imp.base_pixels.borrow();
        let (bytes, stride) = borrowed.as_ref()?;
        let width = usize::try_from(texture.width()).ok()?;
        let height = usize::try_from(texture.height()).ok()?;
        // Repacked to a tight `width * 4` stride: a downloader's rows are padded, and
        // `Pixels` indexes by `y * width + x`.
        let mut rgba = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            let from = y.checked_mul(*stride)?;
            rgba.extend_from_slice(bytes.get(from..from + width * 4)?);
        }
        let pixels = Rc::new(octosnap_scene::redact::Pixels { width, height, rgba });
        drop(borrowed);
        *imp.base_rgba.borrow_mut() = Some(Rc::clone(&pixels));
        Some(pixels)
    }

    /// How many pixels of the stored base image make one document unit.
    ///
    /// Measured from the texture rather than read from `Base::scale`, because §4.12's
    /// resize changes the rect the same pixels are drawn into (D91) and the texture is
    /// the thing the caller is about to index.
    #[must_use]
    pub fn base_pixel_scale(&self) -> Option<f64> {
        let imp = self.imp();
        let texture = imp.base.borrow().clone()?;
        let scene = imp.scene.borrow();
        let base = &scene.as_ref()?.base;
        let across = if base.orientation.swaps_axes() { base.height } else { base.width };
        (across > 0.0).then(|| f64::from(texture.width()) / across)
    }

    /// `spec/05` §6's node budget, over the frames since the last reset.
    ///
    /// Answers `(frames, mean_ms, p95_ms, max_ms)`, or `None` when nothing has been
    /// drawn. The p95 rather than the mean alone, because the budget is per *frame*: a
    /// mean of 0.4 ms with a 9 ms spike drops a frame, and only the tail says so.
    #[must_use]
    pub fn build_stats(&self) -> Option<(usize, f64, f64, f64)> {
        let samples = self.imp().build_ms.borrow();
        if samples.is_empty() {
            return None;
        }
        let mut sorted = samples.clone();
        sorted.sort_by(f64::total_cmp);
        let count = sorted.len();
        #[allow(clippy::cast_precision_loss)]
        let mean = sorted.iter().sum::<f64>() / count as f64;
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let p95_at = ((count as f64) * 0.95).ceil() as usize;
        let p95 = sorted.get(p95_at.saturating_sub(1)).copied().unwrap_or(mean);
        let max = sorted.last().copied().unwrap_or(mean);
        Some((count, mean, p95, max))
    }

    /// Forgets every sample, so the next measurement covers one gesture.
    pub fn reset_build_stats(&self) {
        self.imp().build_ms.borrow_mut().clear();
        self.imp().hits.set(0);
        self.imp().misses.set(0);
    }

    /// `spec/05` §6's cache, as hits and misses since the last reset.
    ///
    /// Divided by the frame count by the caller, because "misses per frame" is the number
    /// that means something: it is how many of the document's objects are being rebuilt
    /// on every frame of a drag, which is what the cache exists to make small.
    #[must_use]
    pub fn cache_counts(&self) -> (u64, u64) {
        (self.imp().hits.get(), self.imp().misses.get())
    }

    /// Replaces every object from an `objects.json` (`spec/05` §8).
    ///
    /// The canvas and not the history: this is a *fixture* being loaded, not an edit, so
    /// it deliberately leaves the undo stack alone. Answers how many objects arrived.
    ///
    /// The two failures are kept apart because they mean different things to whoever is
    /// reading the log: `None` is "no document is open yet", which is a race against the
    /// texture upload and is worth retrying, and `Some(Err)` is "that file is not
    /// `objects.json`", which is not.
    pub fn load_objects(&self, json: &str) -> Option<Result<usize, serde_json::Error>> {
        let mut scene = self.imp().scene.borrow().clone()?;
        Some(match scene.load_objects_json(json) {
            Ok(()) => {
                let count = scene.len();
                self.set_selection(Vec::new());
                self.update_scene(scene);
                Ok(count)
            }
            Err(e) => Err(e),
        })
    }

    /// The object's box as it is actually drawn.
    ///
    /// `Object::bounds` is exact for every kind but one. A text run's box needs a Pango
    /// layout, so `octosnap_scene` *estimates* it -- "0.6 em per character is the usual
    /// rule of thumb for a proportional face; the app measures for real" -- and this is
    /// the app measuring. It matters twice: `spec/05` §4.1's outline has to be around the
    /// glyphs rather than near them, and §4.5's "drag a corner handle" scales the font by
    /// the ratio of this box to the dragged one, which is wrong by however much the
    /// estimate is wrong.
    ///
    /// The plate is included, because a Box style's plate is what the user sees.
    /// The Pango layout for a text run, the same one the renderer draws with.
    ///
    /// Public for the editing overlay (`spec/05` §4.5), which sizes itself from it: an
    /// overlay measured any other way is a box the glyphs jump out of on commit.
    #[must_use]
    pub fn layout_for(
        &self,
        text: &str,
        style: octosnap_scene::TextStyle,
        font_size: f64,
        align: octosnap_scene::TextAlign,
        width: Option<f64>,
    ) -> pango::Layout {
        self.imp().text_layout(text, style, font_size, align, width)
    }

    /// Logs the selected object's handles again -- after a reshape moved them.
    pub fn report_selection(&self) {
        self.report_grips();
    }

    #[must_use]
    pub fn measured_bounds(&self, object: &Object) -> Bounds {
        let Geometry::Text { pos, text, style, font_size, align, width } = &object.geometry
        else {
            return object.bounds();
        };
        let layout = self.imp().text_layout(text, *style, *font_size, *align, *width);
        let (w, h) = layout.pixel_size();
        let pad = if style.is_outlined() { 0.0 } else { style.plate_padding_em() * font_size };
        Bounds::new(
            pos.x - pad,
            pos.y - pad,
            f64::from(w) + pad * 2.0,
            f64::from(h) + pad * 2.0,
        )
    }

    /// Where `spec/05` §4.1's handles are, in document coordinates.
    ///
    /// One list, read by the chrome that draws them and by the gesture that grabs them.
    /// Two lists would agree until one of them was edited, and the symptom -- a handle
    /// that is drawn and cannot be clicked -- is the kind nobody reports precisely.
    #[must_use]
    pub fn grips_of(&self, object: &Object) -> Vec<Point> {
        match object.grips() {
            octosnap_scene::Grips::Ends(_) => object.points(),
            octosnap_scene::Grips::Box => {
                let b = self.measured_bounds(object);
                octosnap_scene::Handle::ALL
                    .into_iter()
                    .map(|handle| handle.position(b))
                    .collect()
            }
            octosnap_scene::Grips::None => Vec::new(),
        }
    }

    /// `spec/05` §4.11's crop rect, or `None` to leave crop mode.
    ///
    /// `queue_resize` and not `queue_draw`, because the crop rect can be larger than the
    /// canvas and the widget's natural size is measured from what is visible.
    pub fn set_crop(&self, crop: Option<Bounds>) {
        if self.imp().crop.get() != crop {
            self.imp().crop.set(crop);
            self.queue_resize();
        }
    }

    #[must_use]
    pub fn crop(&self) -> Option<Bounds> {
        self.imp().crop.get()
    }

    /// The colour an expansion would be filled with, previewed under the document.
    pub fn set_crop_fill(&self, fill: Option<Rgba>) {
        if self.imp().crop_fill.get() != fill {
            self.imp().crop_fill.set(fill);
            self.queue_draw();
        }
    }

    /// `spec/05` §4.11's "detected background colour (median of the border pixels)".
    ///
    /// Measured off the base texture's own border and cached for the document's life --
    /// the pixels cannot change, since annotations do not touch the base image, and
    /// `set_scene` is the only thing that invalidates it.
    ///
    /// Sampled with a stride rather than exhaustively: a 5120x2880 capture has 16 000
    /// border pixels and the median of 256 of them, evenly spaced, is the same colour for
    /// any real screenshot. Every corner is included, because a border that is one colour
    /// on three sides and another on the fourth should still read as the majority.
    #[must_use]
    pub fn border_color(&self) -> Option<Rgba> {
        let imp = self.imp();
        if let Some(cached) = imp.border.get() {
            return cached;
        }
        let measured = self.measure_border();
        imp.border.set(Some(measured));
        measured
    }

    fn measure_border(&self) -> Option<Rgba> {
        let scene = self.imp().scene.borrow().clone()?;
        let base = scene.base.bounds();
        let (w, h) = (base.width, base.height);
        if w < 1.0 || h < 1.0 {
            return None;
        }
        /// Roughly how many samples to take per side.
        const PER_SIDE: f64 = 64.0;
        let step_x = (w / PER_SIDE).max(1.0);
        let step_y = (h / PER_SIDE).max(1.0);

        let mut samples = Vec::new();
        let mut push = |x: f64, y: f64| {
            if let Some(color) = self.sample(Point::new(x, y)) {
                samples.push(color);
            }
        };
        let (last_x, last_y) = (w - 1.0, h - 1.0);
        let mut x = 0.0;
        while x <= last_x {
            push(x, 0.0);
            push(x, last_y);
            x += step_x;
        }
        let mut y = 0.0;
        while y <= last_y {
            push(0.0, y);
            push(last_x, y);
            y += step_y;
        }
        // The four corners, whatever the stride landed on.
        for (cx, cy) in [(0.0, 0.0), (last_x, 0.0), (0.0, last_y), (last_x, last_y)] {
            push(cx, cy);
        }
        octosnap_scene::crop::median_color(&samples)
    }

    #[must_use]
    pub fn marquee(&self) -> Option<Bounds> {
        self.imp().marquee.get()
    }

    /// A **picture** point in widget space -- the inverse of [`Self::to_document`].
    ///
    /// Public because the text-editing overlay (`spec/05` §4.5) has to be *positioned* in
    /// widget coordinates, which means going the other way.
    #[must_use]
    pub fn to_widget(&self, point: Point) -> (f64, f64) {
        self.canvas_to_widget(self.imp().place().apply(point))
    }

    /// How much `spec/05` §4.13's background is shrinking the picture, 1.0 for none.
    ///
    /// For the one thing that is not drawn by this widget: §4.5's text overlay is a real
    /// `GtkTextView` positioned over the canvas, and its transform has to carry the
    /// picture's scale as well as the zoom.
    #[must_use]
    pub fn picture_scale(&self) -> f64 {
        self.imp().place().scale
    }

    /// A **canvas** point in widget space: `spec/05` §4.11's crop rect, and the canvas
    /// itself.
    #[must_use]
    pub fn canvas_to_widget(&self, point: Point) -> (f64, f64) {
        let imp = self.imp();
        let zoom = imp.zoom.get();
        let (ox, oy) = imp.origin.get();
        ((point.x - ox) * zoom, (point.y - oy) * zoom)
    }

    /// A widget-space point in the document's coordinates.
    ///
    /// Every pointer event has to come through here before it reaches the scene, because
    /// `octosnap_scene` knows nothing about zoom -- which is exactly why hit testing in
    /// that crate works at any zoom. It knows nothing about `spec/05` §4.13's background
    /// either, and for the same reason: an object is at the same place in the picture
    /// whatever padding is around it, so the placement is undone here too.
    #[must_use]
    pub fn to_document(&self, x: f64, y: f64) -> Point {
        self.imp().place().invert(self.to_canvas(x, y))
    }

    /// A widget-space point in the **canvas's** coordinates, which is where a crop rect
    /// and the canvas rect are measured.
    #[must_use]
    pub fn to_canvas(&self, x: f64, y: f64) -> Point {
        let imp = self.imp();
        let zoom = imp.zoom.get();
        let (ox, oy) = imp.origin.get();
        Point::new(ox + x / zoom, oy + y / zoom)
    }
}
