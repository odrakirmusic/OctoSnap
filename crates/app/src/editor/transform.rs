// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §4.12's Resize, Rotate and Flip, behind §4.11's `Image size` readout.
//!
//! > an **Image size** readout that doubles as the resize menu, reading e.g.
//! > `1709 x 806 px`.
//!
//! So the menu lives on the crop toolbar, where §4.11 puts it, and the readout stays the
//! label of the button that opens it. Everything the menu does is
//! `octosnap_scene::transform::plan`: one `Command::Batch` carrying the base, every
//! object that moved and the canvas, applied through the same history as every other edit
//! so one Ctrl+Z puts a rotated document back (D91).
//!
//! The one thing this file has to do that the crate cannot is move the **crop rect in
//! flight**. A user who has dragged out a crop and then turns the picture expects the
//! selection to turn with it; the rect is editor state, not document state, so it is
//! mapped here by the same `Mapping` the document was.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use octosnap_scene::transform::{Mapping, Transform};

use super::window::Editor;

/// §4.12's resize dialog: the largest a document may be asked to become, per axis.
///
/// Not a limit the spec states. A guard rather than a policy: the field accepts typed
/// digits, and a document a million units wide is a canvas nothing can render and an
/// export nothing can open. Twice the widest display anyone is shipping is room enough.
const MAX_SIZE: f64 = 32_768.0;

impl Editor {
    /// §4.11's readout, as the button §4.11 says it is.
    ///
    /// The label inside it is the one `refresh_crop` writes to, so the readout is still
    /// live while the menu is closed -- a menu button whose label is a widget rather than
    /// a string is what makes that possible.
    pub(super) fn build_size_menu(self: &Rc<Self>) -> gtk::Widget {
        let readout = gtk::Label::new(None);
        readout.add_css_class("numeric");
        // A chevron, because a label alone does not say it opens anything (`spec/13` #13).
        let button = gtk::MenuButton::builder()
            .tooltip_text("Image size: resize, rotate or flip")
            .child(&readout)
            .always_show_arrow(true)
            .build();
        *self.crop.readout.borrow_mut() = Some(readout);

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        // `None` is the row that opens the dialog; the rest apply at once, because a
        // rotate has nothing to ask.
        let entries: [(&str, Option<Transform>); 5] = [
            ("Resize…", None),
            ("Rotate left", Some(Transform::RotateLeft)),
            ("Rotate right", Some(Transform::RotateRight)),
            ("Flip horizontal", Some(Transform::FlipHorizontal)),
            ("Flip vertical", Some(Transform::FlipVertical)),
        ];
        for (text, _) in entries {
            let row = gtk::ListBoxRow::new();
            let label = gtk::Label::new(Some(text));
            label.set_xalign(0.0);
            label.set_margin_start(8);
            label.set_margin_end(8);
            label.set_margin_top(4);
            label.set_margin_bottom(4);
            row.set_child(Some(&label));
            list.append(&row);
        }
        {
            let editor = Rc::downgrade(self);
            list.connect_row_activated(move |list, row| {
                if let Some(popover) = list.ancestor(gtk::Popover::static_type()) {
                    popover.downcast_ref::<gtk::Popover>().map(gtk::Popover::popdown);
                }
                let Some(editor) = editor.upgrade() else { return };
                let index = usize::try_from(row.index()).unwrap_or(0);
                match entries.get(index).map(|(_, transform)| *transform) {
                    Some(Some(transform)) => editor.apply_transform(transform),
                    Some(None) => editor.ask_for_resize(),
                    None => {}
                }
            });
        }
        button.set_popover(Some(&gtk::Popover::builder().child(&list).build()));
        button.upcast()
    }

    /// One of §4.12's operations, as one undo step.
    pub(super) fn apply_transform(self: &Rc<Self>, transform: Transform) {
        let Some(mut scene) = self.canvas.scene() else { return };
        let Some(mapping) = Mapping::of(&scene, transform) else {
            self.toast("That size is not a picture", None);
            return;
        };
        let Some(command) = octosnap_scene::transform::plan(&scene, transform) else {
            // A resize to the size it already is. Not a failure and not worth a toast.
            return;
        };
        if !self.history.borrow_mut().apply(&mut scene, command) {
            tracing::warn!(transform = transform.label(), "the scene refused a transform");
            return;
        }
        self.history.borrow_mut().end_gesture();
        // The crop rect is editor state and turns with the document it is over.
        if let Some(mut crop) = self.crop.active.get() {
            crop.rect = mapping.bounds(crop.rect);
            self.crop.active.set(Some(crop));
            self.crop.memory.set(Some(crop));
            // The drag in flight is over the rect that just moved, and its press point is
            // in the old space. Dropped rather than mapped: a pointer that is still down
            // is where it is, and the next motion picks the gesture up again.
            self.crop.drag.set(None);
        }
        self.canvas.update_scene(scene);
        self.canvas.center();
        self.after_edit();
        // The crop bar's fields read the rect that just moved, and it is rebuilt rather
        // than nudged for the same reason every other control change rebuilds it.
        if self.cropping() {
            self.rebuild_crop_bar();
        }
        tracing::info!(transform = transform.label(), "transformed the document");
    }

    /// §4.12's "Resize dialog: width/height in px with lock, or percent".
    ///
    /// One dialog with three ways in, which is what the spec's list is: two spin buttons,
    /// a lock that keeps them in proportion, and a percentage that drives both. The lock
    /// starts **on**, because a screenshot resized on one axis only is a screenshot nobody
    /// asked for -- and it can be turned off, because §4.12 lists it as a toggle.
    pub(super) fn ask_for_resize(self: &Rc<Self>) {
        let Some(scene) = self.canvas.scene() else { return };
        let (was_w, was_h) = (scene.base.width, scene.base.height);
        if !(was_w > 0.0 && was_h > 0.0) {
            return;
        }

        let dialog = adw::AlertDialog::new(Some("Resize image"), None);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 12);

        let width = spin(was_w);
        let height = spin(was_h);
        let lock = gtk::ToggleButton::builder()
            .icon_name("changes-prevent-symbolic")
            .tooltip_text("Keep the proportions")
            .active(true)
            .build();
        // Closed when it holds the proportions and open when it does not, as the overlay
        // toolbar's lock is.
        lock.connect_toggled(|lock| {
            lock.set_icon_name(if lock.is_active() {
                "changes-prevent-symbolic"
            } else {
                "changes-allow-symbolic"
            });
        });
        let percent = gtk::SpinButton::with_range(1.0, 1000.0, 1.0);
        percent.set_value(100.0);

        let pixels = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        pixels.append(&gtk::Label::new(Some("Width")));
        pixels.append(&width);
        pixels.append(&lock);
        pixels.append(&gtk::Label::new(Some("Height")));
        pixels.append(&height);
        column.append(&pixels);

        let proportion = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        proportion.append(&gtk::Label::new(Some("Percent")));
        proportion.append(&percent);
        column.append(&proportion);

        // Three fields that are one number, so each has to write the other two without
        // writing itself back again -- which is what the flag is for. Without it the two
        // handlers chase each other and the rounding walks the value away from what was
        // typed, one event at a time.
        let syncing = Rc::new(Cell::new(false));
        // Each field holds the others weakly, as `crop.rs`'s do: strong, they held each other
        // in a ring that outlived the dialog (D139).
        {
            let (height, percent, lock, syncing) =
                (height.downgrade(), percent.downgrade(), lock.downgrade(), Rc::clone(&syncing));
            width.connect_value_changed(move |width| {
                let (Some(height), Some(percent), Some(lock)) = (height.upgrade(), percent.upgrade(), lock.upgrade())
                else {
                    return;
                };
                if syncing.get() {
                    return;
                }
                syncing.set(true);
                let ratio = width.value() / was_w;
                if lock.is_active() {
                    height.set_value((was_h * ratio).round().clamp(1.0, MAX_SIZE));
                }
                percent.set_value((ratio * 100.0).clamp(1.0, 1000.0));
                syncing.set(false);
            });
        }
        {
            let (width, percent, lock, syncing) =
                (width.downgrade(), percent.downgrade(), lock.downgrade(), Rc::clone(&syncing));
            height.connect_value_changed(move |height| {
                let (Some(width), Some(percent), Some(lock)) = (width.upgrade(), percent.upgrade(), lock.upgrade())
                else {
                    return;
                };
                if syncing.get() {
                    return;
                }
                syncing.set(true);
                let ratio = height.value() / was_h;
                if lock.is_active() {
                    width.set_value((was_w * ratio).round().clamp(1.0, MAX_SIZE));
                }
                percent.set_value((ratio * 100.0).clamp(1.0, 1000.0));
                syncing.set(false);
            });
        }
        {
            let (width, height, syncing) = (width.downgrade(), height.downgrade(), Rc::clone(&syncing));
            percent.connect_value_changed(move |percent| {
                let (Some(width), Some(height)) = (width.upgrade(), height.upgrade()) else { return };
                if syncing.get() {
                    return;
                }
                syncing.set(true);
                let ratio = percent.value() / 100.0;
                width.set_value((was_w * ratio).round().clamp(1.0, MAX_SIZE));
                height.set_value((was_h * ratio).round().clamp(1.0, MAX_SIZE));
                syncing.set(false);
            });
        }

        dialog.set_extra_child(Some(&column));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("resize", "Resize");
        dialog.set_response_appearance("resize", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("resize"));
        dialog.set_close_response("cancel");
        {
            let editor = Rc::downgrade(self);
            dialog.connect_response(None, move |dialog, response| {
                dialog.close();
                if response != "resize" {
                    return;
                }
                let Some(editor) = editor.upgrade() else { return };
                editor.apply_transform(Transform::Resize {
                    width: width.value(),
                    height: height.value(),
                });
            });
        }
        dialog.present(Some(&self.window));
    }
}

/// One of the dialog's two pixel fields.
fn spin(value: f64) -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(1.0, MAX_SIZE, 1.0);
    spin.set_value(value.round());
    spin.set_width_chars(6);
    // §4.11's own value field does the same: a size typed and then committed with Return
    // rather than with the mouse is the faster way through this dialog.
    spin.set_activates_default(true);
    spin
}
