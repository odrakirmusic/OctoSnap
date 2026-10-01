// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §4.14's "Add image / combine": a picture into the document, by drop or by
//! chooser.
//!
//! > Drop or open an image: inserted as an **image object** with move/resize handles; drop
//! > zones at the four canvas edges extend the canvas and place the image beside the
//! > existing content ("stitch"). Rounded corners and alignment respect the background
//! > tool.
//!
//! Where a picture lands is `octosnap_scene::combine`, tested without a display. What is
//! here is the three things that need one: the file chooser, the `GtkDropTarget` that
//! turns a drag into a zone and a path, and the edit that commits both halves of a stitch
//! -- the object and the canvas it needed -- as one undo step.
//!
//! "Rounded corners and alignment respect the background tool" needs no code, and that is
//! worth saying rather than leaving as an absence: an image object is drawn inside
//! `append_scene`'s picture transform like every other object, so §4.13's rounded clip and
//! its placement already cover it (D89).

use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use octosnap_scene::combine::{self, Zone};
use octosnap_scene::{Bounds, Command, Geometry, Object, Point, Style};
use tracing::{info, warn};

use super::window::Editor;

/// The picture formats the chooser offers and a drop accepts.
const MIME_TYPES: [&str; 5] =
    ["image/png", "image/jpeg", "image/webp", "image/avif", "image/gif"];

impl Editor {
    /// §2's "Add image | — | file chooser or drop", beside the background button.
    ///
    /// In the canvas-actions group at the left of the toolbar, which `spec/05` §1 names
    /// outright: "canvas actions (crop, add image, background) in their own group".
    pub(super) fn build_add_image_button(self: &Rc<Self>) -> gtk::Widget {
        let button = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add an image — or drop one on the canvas, edges included")
            .build();
        {
            let editor = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.choose_image();
                }
            });
        }
        button.upcast()
    }

    /// §4.14's "open an image": the chooser, then an insert in the middle of the picture.
    fn choose_image(self: &Rc<Self>) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Images"));
        for mime in MIME_TYPES {
            filter.add_mime_type(mime);
        }
        let dialog = gtk::FileDialog::builder()
            .title("Add an image")
            .default_filter(&filter)
            .modal(true)
            .build();
        let editor = Rc::downgrade(self);
        dialog.open(Some(&self.window), gio::Cancellable::NONE, move |answer| {
            let Some(editor) = editor.upgrade() else { return };
            let Ok(file) = answer else { return };
            let Some(path) = file.path() else { return };
            // The middle of the picture, because a chooser has no pointer to have let go
            // at -- and the object arrives selected with handles on it, so moving it is
            // the next thing the user can do without looking for a menu.
            let centre = editor
                .canvas
                .scene()
                .map_or(Point::new(0.0, 0.0), |scene| scene.canvas.center());
            editor.insert_image(&path, Zone::Inside, centre);
        });
    }

    /// §4.14's drop, with §4.14's four edge zones.
    ///
    /// Installed on the canvas rather than on the window: the zones are positions *in the
    /// document*, and a target on the window would have to undo the toolbar's height to
    /// find out where the pointer was.
    pub(super) fn install_image_drop(self: &Rc<Self>) {
        let target = gtk::DropTarget::new(glib::Type::INVALID, gdk::DragAction::COPY);
        // Three types, in the order they are preferred: a list because a file manager
        // sends one even for a single file, a file because most other apps send that, and
        // a texture because a browser sends pixels with no file behind them at all.
        target.set_types(&[
            gdk::FileList::static_type(),
            gio::File::static_type(),
            gdk::Texture::static_type(),
        ]);

        {
            let editor = Rc::downgrade(self);
            target.connect_motion(move |_, x, y| {
                let Some(editor) = editor.upgrade() else { return gdk::DragAction::empty() };
                editor.canvas.set_drop_zone(Some(editor.zone_at_widget(x, y)));
                gdk::DragAction::COPY
            });
        }
        {
            let editor = Rc::downgrade(self);
            target.connect_leave(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.canvas.set_drop_zone(None);
                }
            });
        }
        {
            let editor = Rc::downgrade(self);
            target.connect_drop(move |_, value, x, y| {
                let Some(editor) = editor.upgrade() else { return false };
                editor.canvas.set_drop_zone(None);
                let zone = editor.zone_at_widget(x, y);
                let at = editor.canvas.to_canvas(x, y);
                let Some(path) = editor.dropped_file(value) else {
                    warn!("a drop arrived with nothing this editor could read");
                    return false;
                };
                editor.insert_image(&path, zone, at)
            });
        }
        self.canvas.add_controller(target);
    }

    /// §4.14's insert by name, for the `add-image` action: `in`, `left`, `right`, `top`
    /// or `bottom`.
    pub(super) fn add_image_at(self: &Rc<Self>, path: &Path, zone: &str) {
        let zone = match zone {
            "left" => Zone::Left,
            "right" => Zone::Right,
            "top" => Zone::Top,
            "bottom" => Zone::Bottom,
            _ => Zone::Inside,
        };
        let at = self
            .canvas
            .scene()
            .map_or(Point::new(0.0, 0.0), |scene| scene.canvas.center());
        self.insert_image(path, zone, at);
    }

    /// The zone a widget-space pointer is over.
    fn zone_at_widget(&self, x: f64, y: f64) -> Zone {
        let Some(scene) = self.canvas.scene() else { return Zone::Inside };
        combine::zone_at(scene.canvas, self.canvas.to_canvas(x, y))
    }

    /// A dropped value as a file on disk, writing one out when the drop was only pixels.
    fn dropped_file(&self, value: &glib::Value) -> Option<PathBuf> {
        if let Ok(list) = value.get::<gdk::FileList>()
            && let Some(file) = list.files().into_iter().next()
        {
            return file.path();
        }
        if let Ok(file) = value.get::<gio::File>() {
            return file.path();
        }
        let texture = value.get::<gdk::Texture>().ok()?;
        self.picture_file(&texture, "dropped")
    }

    /// Pixels with no file, written out so an image object can hold them: a drag out of a
    /// browser or another editor, or a picture pasted from the desktop's clipboard.
    ///
    /// Into the spool beside the capture, which is the app's own cache and is already what
    /// the janitor sweeps -- `spec/10` §3's "images cross the boundary as files" applies
    /// inside the app too, and an image object holds a path and not a texture.
    fn picture_file(&self, texture: &gdk::Texture, how: &str) -> Option<PathBuf> {
        let stem = self.capture.path.file_stem()?.to_string_lossy().into_owned();
        let stamp = glib::real_time();
        let path = self.capture.path.with_file_name(format!("{stem}-{how}-{stamp}.png"));
        Self::encode_png(texture, &path).then_some(path)
    }

    /// Ctrl+V with nothing on the internal object clipboard: a picture on the desktop's
    /// clipboard goes in as an image object, as a drop in the middle of the canvas would.
    ///
    /// Jakob's law (`spec/13` #3) in the other direction from §4.1's copy: Ctrl+V in an
    /// image editor pastes the image that was copied elsewhere. With neither, nothing
    /// happens, which is what Ctrl+V with an empty clipboard does everywhere else.
    pub(super) fn paste_picture(self: &Rc<Self>) {
        let clipboard = self.window.clipboard();
        // Another application offers MIME types, not GTypes, so the question is whether
        // any of them can be read as a texture -- which is what the union answers.
        if !clipboard.formats().union_deserialize_types().contains_type(gdk::Texture::static_type()) {
            info!("nothing to paste");
            return;
        }
        let editor = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let texture = match clipboard.read_texture_future().await {
                Ok(Some(texture)) => texture,
                Ok(None) => return,
                Err(why) => {
                    warn!("the clipboard's picture could not be read: {why}");
                    return;
                }
            };
            let Some(editor) = editor.upgrade() else { return };
            let Some(path) = editor.picture_file(&texture, "pasted") else {
                editor.toast("The pasted picture could not be written out", None);
                return;
            };
            let at = editor
                .canvas
                .scene()
                .map_or(Point::new(0.0, 0.0), |scene| scene.canvas.center());
            editor.insert_image(&path, Zone::Inside, at);
        });
    }

    /// §4.14's insert: one image object, and the canvas a stitch needed.
    ///
    /// Answers whether anything was inserted, which is what a `GtkDropTarget` wants back.
    fn insert_image(self: &Rc<Self>, path: &Path, zone: Zone, at: Point) -> bool {
        let Some(mut scene) = self.canvas.scene() else { return false };
        let texture = match gdk::Texture::from_filename(path) {
            Ok(texture) => texture,
            Err(why) => {
                warn!(path = %path.display(), "the image would not load: {why}");
                self.toast("That file is not an image this editor can read", None);
                return false;
            }
        };
        let (width, height) = (f64::from(texture.width()), f64::from(texture.height()));
        let Some(placed) = combine::place(scene.canvas, zone, at, width, height) else {
            return false;
        };

        let object = Object::new(
            scene.top_z().saturating_add(1),
            super::tools::now_seconds(),
            // No stroke and no shadow: an inserted picture is not a drawn shape, and a
            // shadow under one would be §4.13's job and not this object's.
            Style::new(octosnap_scene::Rgba::new(0.0, 0.0, 0.0, 0.0), 1, false),
            Geometry::Image {
                bounds: placed.image,
                file: path.to_string_lossy().into_owned(),
            },
        );
        let id = object.id;
        // One step, both halves. A stitch that undid to "the canvas is still wide but the
        // picture is gone" would be worse than no undo at all.
        let mut commands = vec![Command::Add(object)];
        if placed.canvas != scene.canvas {
            commands.push(Command::CanvasChange { before: scene.canvas, after: placed.canvas });
        }
        if !self.history.borrow_mut().apply(&mut scene, Command::Batch(commands)) {
            return false;
        }
        self.history.borrow_mut().end_gesture();
        self.canvas.update_scene(scene);
        self.canvas.center();
        self.canvas.set_selection(vec![id]);
        self.after_edit();
        info!(
            path = %path.display(),
            zone = ?zone,
            bounds = ?Bounds::normalised(placed.image),
            "added an image"
        );
        true
    }
}
