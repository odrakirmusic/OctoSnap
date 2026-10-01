// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §4.11's crop mode: the toolbar that replaces the toolbar, and the rect.
//!
//! > Enter crop mode and **the whole toolbar swaps** to crop controls: an aspect menu, a
//! > flip/swap-orientation button, a value field, a lock, a reset, and an **Image size**
//! > readout that doubles as the resize menu, reading e.g. `1709 x 806 px`.
//!
//! Crop is the one feature that makes `docs/decisions.md` D52 visible, and it is worth
//! saying what it does *not* do: no pixels are resampled and nothing is discarded. A crop
//! is one [`Command::CanvasChange`] -- `Scene::canvas` moves, `Scene::base` does not --
//! which is how §4.11's "non-destructive in the project (stored as canvas rect)" and
//! §11 item 4's byte-identical undo both come out right without a special case. An
//! annotation left outside the new canvas is clipped rather than deleted, and one Ctrl+Z
//! brings the whole picture back.
//!
//! The arithmetic is all in `octosnap_scene::crop` and `::handle`, where it is tested
//! without a display: the aspect presets, the snapping, the rect a handle drag produces,
//! and §4.11's "median of the border pixels". What is left here is the toolbar, three
//! gesture handlers, and the one edit that applies it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use octosnap_scene::crop::{Aspect, Crop, Drag, FillKind, Targets};
use octosnap_scene::{Bounds, Command, Geometry, Grip, Object, Point, Style, handle};

use octosnap_scene::tool::Control;

use super::picker::Picker;
use super::preview::{Kind, Preview};
use super::window::Editor;

/// How close the pointer has to come to a handle, in **widget** pixels.
///
/// Widget pixels and divided by the zoom before use, because that is what the user is
/// aiming at: the bracket is fifteen pixels on screen whatever the zoom, so its target
/// has to be too.
const GRAB: f64 = 12.0;

/// `spec/05` §4.11's crop mode, as the editor holds it.
#[derive(Debug, Default)]
pub struct CropState {
    /// The crop in progress, or `None` outside crop mode.
    pub active: Cell<Option<Crop>>,
    /// What the controls were set to last time, so re-entering crop mode does not throw
    /// away the aspect the user chose (`Crop::again`).
    pub memory: Cell<Option<Crop>>,
    /// The gesture in flight and the document point the press landed on.
    ///
    /// The press point is kept because `GtkGestureDrag` reports *offsets*, and a crop
    /// drag is absolute -- the same reason `Gesture::Moving` keeps the objects as they
    /// were rather than accumulating deltas.
    pub drag: Cell<Option<(Drag, Point)>>,
    /// The stack whose pages are the two toolbars, and the crop page's own widgets.
    pub stack: RefCell<Option<gtk::Stack>>,
    pub bar: RefCell<Option<adw::WrapBox>>,
    /// §4.11's `Image size` readout, updated on every motion.
    pub readout: RefCell<Option<gtk::Label>>,
}

impl Editor {
    /// `spec/05` §1's tools at the top, and §4.11's toolbar that replaces them.
    ///
    /// A `GtkStack` rather than two boxes shown and hidden, because "the whole toolbar
    /// swaps" is a statement about the *window's* layout: the canvas must not move when
    /// the swap happens, and a stack with `vhomogeneous` set is the one arrangement that
    /// guarantees it. `hhomogeneous` is off for the reason the colour picker's stack
    /// needed it off -- a stack sizes to its widest child, and the crop bar's minimum
    /// width would otherwise become the window's.
    pub(super) fn build_toolbars(self: &Rc<Self>) -> gtk::Widget {
        // One row, `spec/05` §1 [P→V]: the tools, then the active tool's options --
        // "the options for the active tool inline on the right" -- and a **wrap box**
        // rather than a box, so a window too narrow for both puts the options on a
        // second line instead of losing them off the end (D57). The strip is one child
        // and wraps as a unit: thirteen buttons split across two lines would be two
        // toolbars.
        let edit = wrapping_row();
        // `spec/05` §1, reading the reference toolbar left to right: "the window controls,
        // then **canvas actions** (crop, add image, background) in their own group, then
        // the **tools**". Crop is a mode and lives at the end of the strip already, so
        // what is left of that group here is the background -- which §1 also settles: "No
        // side panel is present by default. The background tool is a toolbar button that
        // opens its panel on demand."
        // One linked group for both, as §1 has it, rather than two groups of one.
        let canvas_actions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        canvas_actions.add_css_class("linked");
        canvas_actions.set_valign(gtk::Align::Center);
        canvas_actions.append(&self.build_background_button());
        canvas_actions.append(&self.build_add_image_button());
        edit.append(&canvas_actions);
        edit.append(&self.build_tool_strip());
        edit.append(&self.build_options_row());

        let crop = self.build_crop_bar();

        let stack = gtk::Stack::new();
        stack.set_hhomogeneous(false);
        stack.set_vhomogeneous(true);
        // §9's motion table: 100-200 ms for a state change, and a crossfade because
        // nothing is arriving from a direction.
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_transition_duration(120);
        stack.add_named(&edit, Some("edit"));
        stack.add_named(&crop, Some("crop"));
        stack.set_visible_child_name("edit");
        *self.crop.stack.borrow_mut() = Some(stack.clone());
        stack.upcast()
    }

    /// §4.11's crop toolbar, left to right as `editor-crop-mode.png` shows it.
    fn build_crop_bar(self: &Rc<Self>) -> gtk::Widget {
        // A wrap box for the reason the edit page is one, and more so: this is the
        // wider of the two rows, and its last two children are the only way out of crop
        // mode that does not need a key.
        let bar = wrapping_row();
        *self.crop.bar.borrow_mut() = Some(bar.clone());
        self.rebuild_crop_bar();
        bar.upcast()
    }

    /// Fills the crop bar. Rebuilt on every control change, for the reason the options
    /// row is: the aspect menu's label, the value fields' sensitivity and the lock's state
    /// all follow the crop, and one rebuild is less state than four bindings.
    pub(super) fn rebuild_crop_bar(self: &Rc<Self>) {
        let Some(bar) = self.crop.bar.borrow().clone() else { return };
        // The window's default widget is the Crop button about to be removed with the
        // rest of the bar, and it has to be let go of **first**, while that button is
        // still alive. GTK 4.22 keeps the default widget as a raw pointer and unsets a
        // removed one *lazily*: `_gtk_window_unset_focus_and_default` only raises a flag
        // and waits for the next frame's after-paint. The loop below drops the last
        // reference to the old button, so the `set_default_widget` call at the end of
        // this function would have read a freed widget to take the `default` class off
        // it -- `gtk_widget_remove_css_class` on memory the heap had already reused,
        // which is the crash on the crop button in a long-lived editor (D62).
        self.window().set_default_widget(None::<&gtk::Widget>);
        while let Some(child) = bar.first_child() {
            bar.remove(&child);
        }
        // The bar's own pickers go with it. `refresh_options` does the same for the
        // options row and for the same reason: a `GtkMenuButton` owns its popover but a
        // `Picker` is a Rust struct beside one, so a rebuild that kept them would leave a
        // session that dragged a hundred crops holding a hundred pickers.
        self.pickers.borrow_mut().clear();
        let crop = self.crop.active.get().unwrap_or_else(|| Crop::new(Bounds::new(0.0, 0.0, 0.0, 0.0), true));
        let original = self.base_bounds();

        // §2's three crop controls, built from the same `Control` table every other row is
        // built from -- so the row that is wrong is wrong in the crate with the test.
        //
        // In two passes, and that is about the *evidence*: `editor-crop-mode.png` puts the
        // value fields immediately after the aspect menu, so the aspect goes first and
        // §2's other two follow §4.11's rect controls. Reading the same table twice keeps
        // the layout honest to the screenshot without a second list of controls to drift.
        let controls = octosnap_scene::Tool::Crop.controls();
        for control in controls.iter().filter(|c| **c == Control::CropAspect) {
            bar.append(&self.crop_control(*control));
        }

        // §4.11's "a value field" -- two of them, with the swap between, disabled unless
        // the aspect is Custom. Disabled and not hidden: `editor-crop-mode.png` shows them
        // greyed out beside a Freeform menu, which tells the user the fields belong to
        // Custom without them having to find out.
        let custom = crop.aspect == Aspect::Custom;
        // Blank unless the aspect has numbers of its own -- a preset or Custom. Freeform
        // constrains nothing and Original takes its ratio from the image, so there is no
        // ratio to type there; `editor-crop-mode.png` shows the fields empty beside a
        // Freeform menu, and filling them with the rect's *size* would say something the
        // Image size readout already says, in a field that does not mean it.
        let numbers = (crop.aspect == Aspect::Custom || crop.aspect.numbers().is_some())
            .then(|| crop.numbers(original));
        let w_field = ratio_field(numbers.map(|n| n.0), custom);
        let h_field = ratio_field(numbers.map(|n| n.1), custom);
        let swap = gtk::Button::from_icon_name("object-flip-horizontal-symbolic");
        swap.set_has_frame(false);
        swap.set_tooltip_text(Some("Swap orientation"));
        {
            let editor = Rc::downgrade(self);
            swap.connect_clicked(move |_| {
                let Some(editor) = editor.upgrade() else { return };
                let original = editor.base_bounds();
                editor.edit_crop(|crop| crop.swap_orientation(original));
            });
        }
        // Each field reads the other through a weak reference. Strong, the two handlers held
        // the two fields in a ring that neither a rebuild's `remove` nor the window's
        // destroy breaks -- GTK unparents a child and never disposes it -- and every
        // editor, and every crop change, left a pair behind.
        {
            let editor = Rc::downgrade(self);
            let other = h_field.downgrade();
            w_field.connect_value_changed(move |field| {
                let (Some(editor), Some(other)) = (editor.upgrade(), other.upgrade()) else { return };
                let original = editor.base_bounds();
                let numbers = (field.value(), other.value());
                editor.edit_crop(|crop| crop.set_custom(numbers, original));
            });
        }
        {
            let editor = Rc::downgrade(self);
            let other = w_field.downgrade();
            h_field.connect_value_changed(move |field| {
                let (Some(editor), Some(other)) = (editor.upgrade(), other.upgrade()) else { return };
                let original = editor.base_bounds();
                let numbers = (other.value(), field.value());
                editor.edit_crop(|crop| crop.set_custom(numbers, original));
            });
        }
        bar.append(&w_field);
        bar.append(&swap);
        bar.append(&h_field);

        // §4.11's lock. It only has a job while the aspect is Freeform -- with a preset
        // chosen the preset *is* the constraint -- so it says so rather than pretending.
        let lock = gtk::ToggleButton::builder()
            .icon_name(if crop.locked {
                "changes-prevent-symbolic"
            } else {
                "changes-allow-symbolic"
            })
            .tooltip_text("Keep the current proportions")
            .active(crop.locked)
            .build();
        lock.set_sensitive(crop.aspect == Aspect::Freeform);
        {
            let editor = Rc::downgrade(self);
            lock.connect_toggled(move |button| {
                let Some(editor) = editor.upgrade() else { return };
                let on = button.is_active();
                editor.edit_crop(|crop| crop.locked = on);
            });
        }
        bar.append(&lock);

        bar.append(&separator());

        let reset = gtk::Button::from_icon_name("edit-undo-symbolic");
        reset.set_has_frame(false);
        reset.set_tooltip_text(Some("Reset to the whole image"));
        {
            let editor = Rc::downgrade(self);
            reset.connect_clicked(move |_| {
                let Some(editor) = editor.upgrade() else { return };
                let original = editor.base_bounds();
                editor.edit_crop(|crop| crop.reset(original));
            });
        }
        bar.append(&reset);

        bar.append(&separator());

        // The rest of §2's row: the snapping toggle and the expand-canvas colour. Neither
        // is in `editor-crop-mode.png`, which was taken with the menus closed, so their
        // place in the row is a choice -- beside the reset, where the settings that are
        // about *behaviour* rather than about the rect belong together.
        for control in controls.iter().filter(|c| **c != Control::CropAspect) {
            bar.append(&self.crop_control(*control));
        }

        bar.append(&separator());

        // §4.11's "**Image size** readout that doubles as the resize menu, reading e.g.
        // `1709 x 806 px`" -- both halves of it now: the label is live while the menu is
        // closed, and the menu is §4.12's Resize, Rotate and Flip (`transform.rs`).
        // No "Image size:" caption: a frame with a size beside it needs no label, and
        // the caption was the width that pushed the Crop button onto a second line at
        // the window's own default size.
        bar.append(&self.build_size_menu());

        bar.append(&separator());

        // Enter and Esc are what §4.11 specifies, and they are not enough on their own:
        // a mode whose only way out is a key the user has to know about reads as a mode
        // that has trapped them. These are the same two actions with a visible target.
        let cancel = gtk::Button::with_label("Cancel");
        {
            let editor = Rc::downgrade(self);
            cancel.connect_clicked(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.cancel_crop();
                }
            });
        }
        bar.append(&cancel);
        let apply = gtk::Button::with_label("Crop");
        apply.add_css_class("suggested-action");
        {
            let editor = Rc::downgrade(self);
            apply.connect_clicked(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.apply_crop();
                }
            });
        }
        bar.append(&apply);
        // §4.11's "Enter applies", by the route GTK already has for it.
        //
        // The window shortcut alone was not enough, and the way it failed is worth
        // keeping: swapping the stack to the crop page unmaps whatever had focus, GTK
        // moves focus to the first focusable widget it finds -- a menu button on this
        // very bar -- and a focused `GtkMenuButton` consumes Return to open its popover.
        // So every Enter in the first run opened the aspect menu and no crop was ever
        // applied, with nothing in the log to say why. A default widget is the answer
        // that does not depend on where focus went: Return reaches it whenever the
        // focused widget does not want the key, which is also true inside the value
        // field.
        //
        // Only in crop mode. The bar is also built once with the window, before there is
        // a crop, and a default widget on a page nobody can see would make Return in
        // edit mode activate a hidden button (D62).
        if self.cropping() {
            apply.set_receives_default(true);
            self.window().set_default_widget(Some(&apply));
        }

        self.refresh_crop();
    }

    /// One of `spec/05` §2's three crop controls.
    fn crop_control(self: &Rc<Self>, control: Control) -> gtk::Widget {
        let crop = self.crop.active.get().unwrap_or_else(|| Crop::new(Bounds::new(0.0, 0.0, 0.0, 0.0), true));
        match control {
            Control::CropAspect => {
                let button = gtk::MenuButton::builder()
                    .tooltip_text("Aspect ratio")
                    .label(crop.aspect.label())
                    .build();
                let list = gtk::ListBox::new();
                list.set_selection_mode(gtk::SelectionMode::Single);
                for aspect in Aspect::ALL {
                    let row = gtk::ListBoxRow::new();
                    let label = gtk::Label::new(Some(aspect.label()));
                    label.set_xalign(0.0);
                    label.set_margin_start(8);
                    label.set_margin_end(8);
                    label.set_margin_top(4);
                    label.set_margin_bottom(4);
                    row.set_child(Some(&label));
                    list.append(&row);
                    if aspect == crop.aspect {
                        list.select_row(Some(&row));
                    }
                }
                let editor = Rc::downgrade(self);
                list.connect_row_activated(move |list, row| {
                    let Some(editor) = editor.upgrade() else { return };
                    let index = usize::try_from(row.index()).unwrap_or(0);
                    let Some(aspect) = Aspect::ALL.get(index).copied() else { return };
                    let original = editor.base_bounds();
                    editor.edit_crop(|crop| crop.set_aspect(aspect, original));
                    if let Some(popover) = list.ancestor(gtk::Popover::static_type()) {
                        popover.downcast_ref::<gtk::Popover>().map(gtk::Popover::popdown);
                    }
                });
                button.set_popover(Some(&gtk::Popover::builder().child(&list).build()));
                button.upcast()
            }
            Control::CropSnapping => {
                // Labelled rather than iconic. Adwaita has no magnet, and every icon
                // that could stand in for "snap" -- a grid, an alignment glyph -- says
                // something else; the six broken tool icons in step 3 were the cost of
                // treating an icon name as a free choice.
                let toggle = gtk::ToggleButton::builder()
                    .label("Snap")
                    .tooltip_text("Snap to the image's edges and to other objects")
                    .active(crop.snap)
                    .build();
                let editor = Rc::downgrade(self);
                toggle.connect_toggled(move |button| {
                    let Some(editor) = editor.upgrade() else { return };
                    let on = button.is_active();
                    // Written straight in rather than through `edit_crop`: rebuilding the
                    // bar from a toggle's own handler destroys the toggle mid-click.
                    if let Some(mut crop) = editor.crop.active.get() {
                        crop.snap = on;
                        editor.crop.active.set(Some(crop));
                    }
                    crate::settings::set_crop_snapping(on);
                });
                toggle.upcast()
            }
            Control::CropExpandColor => {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
                let button = gtk::MenuButton::builder()
                    .tooltip_text("What a canvas expansion is filled with")
                    .label(crop.expand.kind.label())
                    .build();
                let list = gtk::ListBox::new();
                list.set_selection_mode(gtk::SelectionMode::Single);
                for kind in FillKind::ALL {
                    let entry = gtk::ListBoxRow::new();
                    let label = gtk::Label::new(Some(kind.label()));
                    label.set_xalign(0.0);
                    label.set_margin_start(8);
                    label.set_margin_end(8);
                    label.set_margin_top(4);
                    label.set_margin_bottom(4);
                    entry.set_child(Some(&label));
                    list.append(&entry);
                    if kind == crop.expand.kind {
                        list.select_row(Some(&entry));
                    }
                }
                let editor = Rc::downgrade(self);
                list.connect_row_activated(move |list, row| {
                    let Some(editor) = editor.upgrade() else { return };
                    let index = usize::try_from(row.index()).unwrap_or(0);
                    let Some(kind) = FillKind::ALL.get(index).copied() else { return };
                    editor.edit_crop(|crop| crop.expand.kind = kind);
                    if let Some(popover) = list.ancestor(gtk::Popover::static_type()) {
                        popover.downcast_ref::<gtk::Popover>().map(gtk::Popover::popdown);
                    }
                });
                button.set_popover(Some(&gtk::Popover::builder().child(&list).build()));
                row.append(&button);

                // The swatch shows what the fill will actually be -- the *detected*
                // colour when that is the choice, so the user can see what was measured
                // without having to crop first -- and it opens §2's picker when the
                // choice is Custom. Insensitive otherwise, the same way §4.11's value
                // fields are insensitive unless the aspect is Custom: the control is
                // visible, so what it belongs to is visible with it.
                let shown = crop.expand.resolve(self.canvas.border_color());
                let swatch = Preview::new(match shown {
                    Some(color) => Kind::Color(color),
                    None => Kind::Empty,
                });
                let chooser = gtk::MenuButton::builder()
                    .child(&swatch)
                    .tooltip_text(match crop.expand.kind {
                        FillKind::Detected => "The colour detected at the image's border",
                        FillKind::Transparent => "Transparent",
                        FillKind::Custom => "The fill colour",
                    })
                    .sensitive(crop.expand.kind == FillKind::Custom)
                    .build();
                let picker = Picker::new(crop.expand.color, false, crate::settings::my_colors());
                {
                    let editor = Rc::downgrade(self);
                    let swatch = swatch.clone();
                    picker.connect_changed(move |color| {
                        swatch.set_kind(Kind::Color(color));
                        let Some(editor) = editor.upgrade() else { return };
                        // Written straight in rather than through `edit_crop`, for the
                        // reason `options.rs::change_live` gives: rebuilding the bar from
                        // a picker's own callback destroys the popover being dragged in.
                        if let Some(mut crop) = editor.crop.active.get() {
                            crop.expand.color = color;
                            editor.crop.active.set(Some(crop));
                        }
                        editor.refresh_crop();
                    });
                }
                {
                    // Weak, because the callback is kept in the picker itself: a strong
                    // `Rc` here was a cycle that outlived the bar and the window, with the
                    // picker's whole popover in it (`options.rs` had the same one).
                    let editor = Rc::downgrade(self);
                    let weak = Rc::downgrade(&picker);
                    picker.connect_pick(move || {
                        let (Some(editor), Some(picker)) = (editor.upgrade(), weak.upgrade()) else {
                            return;
                        };
                        editor.begin_picking(&picker);
                    });
                }
                chooser.set_popover(Some(picker.popover()));
                self.pickers.borrow_mut().push(picker);
                row.append(&chooser);
                row.upcast()
            }
            // Every other control belongs to a tool whose row is the options row.
            other => {
                tracing::warn!(?other, "not a crop control");
                gtk::Label::new(None).upcast()
            }
        }
    }

    /// The base image's rect, which is what [`Aspect::Original`] and the expansion test
    /// are both relative to.
    /// The picture's rect, where §4.11's crop measures it: in canvas coordinates.
    ///
    /// Not `scene.base.bounds()` any more, because `spec/05` §4.13's background moves the
    /// picture inside a larger canvas -- so the Original aspect preset and "does this
    /// crop expand past the picture" both have to ask where the picture actually is.
    fn base_bounds(&self) -> Bounds {
        self.canvas.scene().map_or(Bounds::new(0.0, 0.0, 0.0, 0.0), |scene| {
            scene.placement().apply_bounds(scene.picture_rect())
        })
    }

    /// Whether crop mode is open.
    #[must_use]
    pub(super) fn cropping(&self) -> bool {
        self.crop.active.get().is_some()
    }

    /// `spec/05` §4.11: "Enter crop mode".
    pub(super) fn enter_crop(self: &Rc<Self>) {
        let Some(scene) = self.canvas.scene() else { return };
        // On the *canvas*, not the base image: entering crop twice must not silently
        // undo a previous crop's expansion (D52).
        let crop = match self.crop.memory.get() {
            Some(remembered) => remembered.again(scene.canvas),
            None => Crop::new(scene.canvas, crate::settings::crop_snapping()),
        };
        self.crop.active.set(Some(crop));
        self.canvas.set_selection(Vec::new());
        if let Some(stack) = self.crop.stack.borrow().clone() {
            stack.set_visible_child_name("crop");
        }
        self.rebuild_crop_bar();
        // The canvas, not the toolbar. Whatever had focus is on the page that just went
        // away, and GTK's own answer -- the first focusable widget in the window -- puts
        // it on a menu button that then eats Enter and the arrow keys. The crop frame is
        // what the keyboard is for in this mode.
        self.canvas.grab_focus();
        tracing::debug!(
            aspect = crop.aspect.label(),
            snap = crop.snap,
            "crop mode entered"
        );
    }

    /// `spec/05` §4.11: "Esc cancels". Leaves the document exactly as it was.
    pub(super) fn cancel_crop(self: &Rc<Self>) {
        if !self.cropping() {
            return;
        }
        self.leave_crop();
        tracing::debug!("crop cancelled");
    }

    /// Drops crop mode without touching the document, and goes back to Select.
    ///
    /// `set_tool` is what restores the toolbar, and it is called *after* the state is
    /// cleared so its own "leaving crop cancels it" branch finds nothing to do.
    fn leave_crop(self: &Rc<Self>) {
        self.crop.memory.set(self.crop.active.get());
        self.crop.active.set(None);
        self.crop.drag.set(None);
        self.canvas.set_crop(None);
        self.canvas.set_crop_fill(None);
        self.canvas.set_cursor(None);
        // The Crop button is going away with the page; a default widget that outlived it
        // would make Return activate a button nobody can see.
        self.window().set_default_widget(None::<&gtk::Widget>);
        if let Some(stack) = self.crop.stack.borrow().clone() {
            stack.set_visible_child_name("edit");
        }
        self.set_tool(octosnap_scene::Tool::Select);
    }

    /// `spec/05` §4.11: "Enter applies".
    ///
    /// One undo step whether or not the canvas grew, which is what [`Command::Batch`] is
    /// for: a crop that expands is a canvas change *and* a background object, and undoing
    /// half of that would leave a fill around an uncropped image.
    pub(super) fn apply_crop(self: &Rc<Self>) {
        let Some(crop) = self.crop.active.get() else { return };
        let Some(scene) = self.canvas.scene() else { return };
        let before = scene.canvas;
        // Whole pixels. A crop is a rectangle of the picture's pixels, and a rect of
        // 715.4 units logged as 715, exported as a 716-wide texture (a render node's
        // bounds are rounded *up*) and saved as neither: the harness at 1.25 -- where a
        // drag lands on fractional units -- caught the three disagreeing.
        let fractional = crop.rect.normalised();
        let after = Bounds::new(
            fractional.x.round(),
            fractional.y.round(),
            fractional.width.round().max(1.0),
            fractional.height.round().max(1.0),
        );

        let mut commands = Vec::new();
        if after != before {
            commands.push(Command::CanvasChange { before, after });
        }

        // `spec/05` §11 item 7: "Crop beyond the image expands the canvas with the
        // detected colour."
        if octosnap_scene::crop::expands(after, scene.base.bounds())
            && let Some(fill) = crop.expand.resolve(self.canvas.border_color())
        {
            let existing =
                scene.objects().iter().find(|o| matches!(o.geometry, Geometry::Background { .. }));
            let plain = octosnap_scene::BackgroundParams::plain(fill);
            match existing {
                // One background, recoloured. A second would draw over the first and
                // §4.13 describes the background as a property of the document.
                //
                // Only one this crop could have made, though. §4.13's tool puts a
                // gradient or a wallpaper in the same slot, and one of those already
                // covers whatever the crop expanded into -- repainting it with the median
                // of the border pixels would throw away the background the user chose to
                // fill an area that is not empty.
                Some(was) if is_plain(was) && was.style.color != fill => {
                    let mut now = was.clone();
                    now.style.color = fill;
                    now.geometry = Geometry::Background { params: plain, source: None };
                    commands.push(Command::Change {
                        id: was.id,
                        before: Box::new(was.clone()),
                        after: Box::new(now),
                    });
                }
                Some(_) => {}
                None => {
                    let lowest = scene.objects().iter().map(|o| o.z).min().unwrap_or(0);
                    commands.push(Command::Add(Object::new(
                        lowest.saturating_sub(1),
                        now_seconds(),
                        Style::new(fill, 1, false),
                        Geometry::Background { params: plain, source: None },
                    )));
                }
            }
        }

        let grew = commands.len();
        self.commit_batch(commands);
        // The view, not the document: the canvas is a different size and the origin was
        // computed against the old one, so without this a crop of the top-left corner
        // leaves the user looking at empty desk.
        self.canvas.center();
        // §4.11's last line, and the end of §11 item 7: "A pinned window showing this
        // image updates after crop." A pin holds a file, so this renders the cropped
        // document and hands the pin the result -- which also means the pin ends up
        // showing the *annotations*, which is what someone who has just cropped is
        // looking at it to check.
        //
        // Only when something actually changed. A crop applied without a change would
        // otherwise re-render and re-place every pin for nothing.
        if grew > 0 {
            self.refresh_pins();
        }
        self.leave_crop();
        tracing::info!(
            x = after.x,
            y = after.y,
            width = after.width,
            height = after.height,
            expanded = octosnap_scene::crop::expands(after, scene.base.bounds()),
            edits = grew,
            "crop applied"
        );
    }

    /// Pushes the rendered document to any pin showing this capture.
    ///
    /// Both paths are offered as the pin's identity: the spool file this editor was
    /// opened on, and the render this editor exports to -- which is what a pin created by
    /// this editor's own Pin button is showing, so a second crop finds it.
    fn refresh_pins(self: &Rc<Self>) {
        let Some(capture) = self.rendered_capture(self.export_scale()) else {
            return;
        };
        let sources = vec![self.capture.path.clone(), capture.path.clone()];
        (self.actions.refresh_pins)(sources, capture);
    }

    /// Changes the crop and rebuilds the bar around it.
    fn edit_crop(self: &Rc<Self>, change: impl FnOnce(&mut Crop)) {
        let Some(mut crop) = self.crop.active.get() else { return };
        change(&mut crop);
        self.crop.active.set(Some(crop));
        self.rebuild_crop_bar();
    }

    /// Pushes the crop to the canvas and to the readout.
    ///
    /// One place, called from every change, because §4.11's readout, the veil and the
    /// expansion preview are three views of one rect and they cannot be allowed to drift.
    fn refresh_crop(self: &Rc<Self>) {
        let Some(crop) = self.crop.active.get() else { return };
        let rect = crop.rect.normalised();
        self.canvas.set_crop(Some(rect));
        let fill = if octosnap_scene::crop::expands(rect, self.base_bounds()) {
            crop.expand.resolve(self.canvas.border_color())
        } else {
            None
        };
        self.canvas.set_crop_fill(fill);
        #[allow(clippy::cast_possible_truncation)]
        let (w, h) = (rect.width.round() as i64, rect.height.round() as i64);
        if let Some(readout) = self.crop.readout.borrow().clone() {
            // Rounded, because §4.11's readout is in pixels and a crop of 1708.6 px is
            // exported as 1709. The *rect* stays fractional: `spec/05` §11 item 2 only
            // holds if nothing is rounded to the screen it was drawn on.
            readout.set_text(&format!("{w} \u{d7} {h} px"));
        }
        // One line per change, and it exists to be grepped. The crop rect is a veil and
        // four brackets on screen and a label in a toolbar -- nothing an assertion can
        // reach -- so `editor-test.sh` reads this instead of a screenshot. The same
        // reason `canvas allocated` and `options row` are logged.
        #[allow(clippy::cast_possible_truncation)]
        let (x, y) = (rect.x.round() as i64, rect.y.round() as i64);
        // The same rect in *widget* coordinates, which is the only way a harness can
        // aim at a bracket: the document's position inside the canvas depends on the zoom
        // and the pan, and D50's lesson -- twice over now -- is that a coordinate worked
        // out by hand outside the process is a coordinate that will be wrong later.
        let (wx, wy) = self.canvas.canvas_to_widget(octosnap_scene::Point::new(rect.x, rect.y));
        let zoom = self.canvas.zoom();
        #[allow(clippy::cast_possible_truncation)]
        let (wx, wy) = (wx.round() as i64, wy.round() as i64);
        #[allow(clippy::cast_possible_truncation)]
        let (ww, wh) = ((rect.width * zoom).round() as i64, (rect.height * zoom).round() as i64);
        tracing::debug!(
            x,
            y,
            width = w,
            height = h,
            expands = octosnap_scene::crop::expands(rect, self.base_bounds()),
            fill = fill.map_or_else(|| "none".to_owned(), |c| c.to_hex()),
            wx,
            wy,
            ww,
            wh,
            "crop rect"
        );
    }

    // --- gestures --------------------------------------------------------------------

    /// A press in crop mode. Answers whether it was consumed.
    pub(super) fn crop_press(self: &Rc<Self>, at: Point) -> bool {
        let Some(crop) = self.crop.active.get() else { return false };
        let tolerance = GRAB / self.canvas.zoom();
        let drag = match handle::grip_at(crop.rect, at, tolerance) {
            Grip::Handle(handle) => Drag::Resize { handle, start: crop.rect },
            Grip::Inside => Drag::Move { start: crop.rect, from: at },
            // A press outside the rect starts a new one, which is how a crop that has
            // been dragged into a corner is escaped without a Reset.
            Grip::Outside => Drag::New { from: at },
        };
        self.crop.drag.set(Some((drag, at)));
        true
    }

    /// A motion in crop mode, in **widget**-space offsets from the press.
    pub(super) fn crop_motion(self: &Rc<Self>, dx: f64, dy: f64) {
        let Some((drag, press)) = self.crop.drag.get() else { return };
        let Some(mut crop) = self.crop.active.get() else { return };
        let Some(scene) = self.canvas.scene() else { return };
        let zoom = self.canvas.zoom();
        let to = press.offset(dx / zoom, dy / zoom);
        let targets = if crop.snap { Targets::of(&scene) } else { Targets::default() };
        crop.rect = crop.drag_to(&drag, to, &targets, scene.base.bounds());
        self.crop.active.set(Some(crop));
        self.refresh_crop();
    }

    /// The button coming up. Nothing is committed -- Enter does that.
    pub(super) fn crop_release(self: &Rc<Self>) {
        if self.crop.drag.get().is_none() {
            return;
        }
        self.crop.drag.set(None);
        // A new rect drawn from scratch can have left the aspect menu saying one thing
        // and the rect being another, and the lock's sensitivity follows the aspect, so
        // the bar is rebuilt once per gesture rather than once per motion event.
        self.rebuild_crop_bar();
    }

    /// The resize cursor over a handle, so the frame says what it will do.
    pub(super) fn crop_hover(self: &Rc<Self>, x: f64, y: f64) {
        let Some(crop) = self.crop.active.get() else { return };
        let at = self.canvas.to_canvas(x, y);
        let tolerance = GRAB / self.canvas.zoom();
        let name = match handle::grip_at(crop.rect, at, tolerance) {
            Grip::Handle(handle) => handle.cursor(),
            Grip::Inside => "move",
            Grip::Outside => "crosshair",
        };
        self.canvas.set_cursor_from_name(Some(name));
    }
}

/// A vertical rule, which is what separates the crop toolbar's groups in
/// `editor-crop-mode.png`.
fn separator() -> gtk::Widget {
    let rule = gtk::Separator::new(gtk::Orientation::Vertical);
    rule.set_margin_top(6);
    rule.set_margin_bottom(6);
    rule.upcast()
}

/// One line of toolbar that becomes two when the window is too narrow for one (D57).
fn wrapping_row() -> adw::WrapBox {
    let row = adw::WrapBox::new();
    row.set_child_spacing(6);
    row.set_line_spacing(6);
    row.set_valign(gtk::Align::Center);
    row
}

/// One half of §4.11's value field. `None` shows an empty field.
///
/// A `GtkSpinButton` for the steppers `editor-crop-mode.png` shows on it, which is also
/// why blanking it takes the `output` signal rather than `set_text`: a spin button
/// re-renders its text from its value whenever the value is set or the adjustment
/// changes, so a `set_text("")` came straight back as the adjustment's lower bound and
/// the field read `1`. `output` is the hook that decides what the number *looks* like.
/// Whether a background object is one the crop's own expansion could have made.
fn is_plain(object: &Object) -> bool {
    match &object.geometry {
        Geometry::Background { params, .. } => params.is_plain(),
        _ => false,
    }
}

fn ratio_field(value: Option<f64>, sensitive: bool) -> gtk::SpinButton {
    let field = gtk::SpinButton::with_range(1.0, 9999.0, 1.0);
    field.set_digits(0);
    field.set_width_chars(4);
    field.set_sensitive(sensitive);
    match value {
        Some(value) => field.set_value(value.max(1.0).round()),
        None => {
            field.connect_output(|field| {
                field.set_text("");
                gtk::glib::Propagation::Stop
            });
            // Once, to run the handler that has just been connected.
            field.set_value(1.0);
        }
    }
    field
}

/// `spec/05` §5.1's `created`, in seconds since the epoch. The same clock `tools.rs` uses.
fn now_seconds() -> u64 {
    gtk::glib::real_time().unsigned_abs() / 1_000_000
}
