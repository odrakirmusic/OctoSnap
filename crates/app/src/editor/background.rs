// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §4.13's background tool: the left sidebar, and the edit behind every control.
//!
//! > It is a **left** sidebar occupying the full window height, not a right-hand panel or
//! > a popover, and its controls map one-to-one onto the preset JSON.
//!
//! The arithmetic is `octosnap_scene::background`, where it is tested without a display:
//! what canvas a parameter set implies, where the picture sits inside it, what the twenty
//! gradients are made of, and what auto-balance trims. What is left here is a column of
//! widgets, one commit function, and the rule that every control goes through it.
//!
//! That rule is the reason this file is arranged the way it is. A panel whose sliders each
//! edited the scene their own way would be nine chances to forget the `CanvasChange` that
//! has to travel with every parameter change -- padding resizes the canvas, so an edit
//! that changed only the object would leave the export the wrong size until the next
//! crop. [`Editor::apply_background`] is the only thing that writes, and every control
//! calls it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use gtk::subclass::prelude::ObjectSubclassIsExt;
use octosnap_scene::background::{
    self, Blur, GRADIENT_SET, MAX_CORNER_RADIUS, MAX_INSET, MAX_PADDING, MAX_SHADOW,
};
use octosnap_scene::{
    Background, BackgroundParams, Bounds, Command, Geometry, Object, Point, Ratio, Style,
};

use super::picker::Picker;
use super::window::Editor;

/// The sidebar's width, and the only number that says how wide it is.
///
/// `editor-background-panel.png` is about a fifth of a 1322 pt window, and the narrowest
/// thing in it is the two sliders side by side: a caption, a four-character spin button
/// and a gap, twice, which measures 334 px plus the column's 24 px of margin. So this is
/// the panel's *real* minimum rather than a wish -- it used to be 300 while the panel
/// measured 414, and a width request nobody can honour is a width request that lies to
/// whoever is dividing the window up (D100).
pub(super) const WIDTH: i32 = 360;

/// A source swatch. Wide enough that a gradient reads as a gradient rather than as a
/// colour, and four of them plus the gaps fit the sidebar's inner width.
const SWATCH: (i32, i32) = (60, 42);

/// The narrowest a swatch may be squeezed to. The rows are homogeneous and fill the
/// panel, so [`SWATCH`]'s width is what they *get*, never what they need: asking for all
/// sixty px as a minimum made the plain-colour row -- five swatches, then five and the
/// custom picker -- demand 390 px of a 300 px panel, which is where the overrun came
/// from. A third of a swatch is still a swatch, and no panel is ever that narrow.
const SWATCH_FLOOR: i32 = 20;

/// How many of the twenty are shown when the grid is collapsed (`spec/05` §4.13's
/// "**Show less** disclosure"). Two rows, so the disclosure is worth pressing.
const COLLAPSED: usize = 8;

/// `spec/05` §4.13's "Two rows of solid swatches".
///
/// Ten, matching §2's palette exactly: the colours the editor already draws with are the
/// colours it puts behind a screenshot, so the user learns one set. White and black are
/// the two anyone actually reaches for here, so they lead.
const PLAIN: [(&str, u32); 10] = [
    ("White", 0xFF_FF_FF),
    ("Paper", 0xF4_F1_EA),
    ("Silver", 0xD5_D8_DC),
    ("Slate", 0x8B_94_9E),
    ("Charcoal", 0x36_3B_42),
    ("Black", 0x11_13_17),
    ("Ink", 0x1E_2B_4A),
    ("Forest", 0x1E_3F_33),
    ("Wine", 0x4A_1E_2B),
    ("Cream", 0xFF_E9_C7),
];

/// Everything the panel has to be able to find again.
///
/// Held by the editor rather than by the widget tree, because the sidebar is rebuilt
/// never and consulted often: a `Change` from anywhere -- an undo, a preset, the Shift
/// that skipped auto-apply -- has to be able to put every control back where the document
/// says it is.
#[derive(Default)]
pub struct BackgroundPanel {
    pub split: RefCell<Option<adw::OverlaySplitView>>,
    pub toggle: RefCell<Option<gtk::ToggleButton>>,
    /// Every source button, with the background it stands for.
    choices: RefCell<Vec<(Background, gtk::ToggleButton)>>,
    padding: RefCell<Option<gtk::Adjustment>>,
    inset: RefCell<Option<gtk::Adjustment>>,
    shadow: RefCell<Option<gtk::Adjustment>>,
    corners: RefCell<Option<gtk::Adjustment>>,
    balance: RefCell<Option<gtk::CheckButton>>,
    alignment: RefCell<Vec<gtk::ToggleButton>>,
    ratio: RefCell<Option<gtk::DropDown>>,
    wallpapers: RefCell<Option<gtk::Box>>,
    gradients: RefCell<Option<gtk::Box>>,
    disclosure: RefCell<Option<gtk::Button>>,
    expanded: Cell<bool>,
    /// True while [`Editor::sync_background_panel`] is writing to the controls, so their
    /// own handlers do not write back and start a loop.
    syncing: Cell<bool>,
    /// The custom colour picker, which a `GtkMenuButton` does not own.
    picker: RefCell<Option<Rc<Picker>>>,
    /// The Presets menu, rebuilt whenever the saved list changes.
    presets: RefCell<Option<gtk::MenuButton>>,
    /// True while the None button's place is waiting to be logged (`log_none_button`).
    none_logging: Rc<Cell<bool>>,
}

impl std::fmt::Debug for BackgroundPanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundPanel")
            .field(
                "open",
                &self.split.borrow().as_ref().is_some_and(adw::OverlaySplitView::shows_sidebar),
            )
            .field("expanded", &self.expanded.get())
            .finish()
    }
}

impl Editor {
    /// The parameters the panel is editing: the document's, or the defaults it opens on.
    #[must_use]
    pub(super) fn background_params(&self) -> BackgroundParams {
        self.canvas
            .scene()
            .and_then(|scene| scene.background().map(|(_, params)| params.clone()))
            .unwrap_or_default()
    }

    /// `spec/05` §4.13's one edit: the object, and the canvas it implies, as one step.
    ///
    /// `open` keeps the undo step open, which is what a slider still under the pointer
    /// needs -- `History::apply` folds the next batch of the same shape into this one, so
    /// a drag from 100 to 240 is one press of Ctrl+Z (`Command::absorb`).
    ///
    /// An empty parameter set removes the object rather than storing one that draws
    /// nothing, and puts the canvas back to the rect the background was laid out over.
    pub(super) fn apply_background(self: &Rc<Self>, params: &BackgroundParams, open: bool) {
        let Some(scene) = self.canvas.scene() else { return };
        let source = self.background_source(&params.clone());
        let before = scene.canvas;
        let existing = scene.background().map(|(id, _)| id);
        let mut commands = Vec::new();

        if params.is_empty() {
            if let Some(id) = existing
                && let Some(object) = scene.get(id)
            {
                commands.push(Command::Remove(object.clone()));
                // Back to what the background was laid out over, which is the crop if
                // there was one and the capture if there was not.
                if before != source {
                    commands.push(Command::CanvasChange { before, after: source });
                }
            }
        } else {
            let after = background::layout(source, params).canvas;
            let geometry =
                Geometry::Background { params: params.clone(), source: Some(source) };
            match existing.and_then(|id| scene.get(id)) {
                Some(was) => {
                    let mut now = was.clone();
                    now.geometry = geometry;
                    // Kept in step so a project written by this build still opens as the
                    // right flat colour in one written before it (`nodes.rs`).
                    if let Background::Color { color } = params.background {
                        now.style.color = color;
                    }
                    commands.push(Command::Change {
                        id: was.id,
                        before: Box::new(was.clone()),
                        after: Box::new(now),
                    });
                }
                None => {
                    let lowest = scene.objects().iter().map(|o| o.z).min().unwrap_or(0);
                    let color = match params.background {
                        Background::Color { color } => color,
                        _ => Style::default().color,
                    };
                    commands.push(Command::Add(Object::new(
                        lowest.saturating_sub(1),
                        now_seconds(),
                        Style::new(color, 1, false),
                        geometry,
                    )));
                }
            }
            if before != after {
                commands.push(Command::CanvasChange { before, after });
            }
        }

        if commands.is_empty() {
            return;
        }
        if open {
            self.commit_batch_open(commands);
        } else {
            self.commit_batch(commands);
        }
        self.canvas.center();
        self.sync_background_panel();
    }

    /// The rect a background lays out, with `spec/05` §4.13's Auto Balance applied.
    ///
    /// Auto Balance is stored as a *shift of the source rect* rather than as an offset of
    /// its own, and that is what keeps the renderer free of pixels: `Scene::placement`
    /// maps the source onto where the layout put it, so a source nudged by the content's
    /// own bias draws the picture with its content centred. The pixels are read once,
    /// here, on the main loop -- `content` is four edge scans over an already-downloaded
    /// buffer, and it happens when a checkbox is ticked rather than per frame.
    ///
    /// **Only on the change.** `apply_background` runs on every slider, every checkbox
    /// and every sync, and the source it starts from is the one the last call stored --
    /// which already carries the shift if the box was already ticked. Adding it again is
    /// not a smaller error than adding it once too few: the panel asked fifty-five times
    /// in a row and put a 1200 × 700 screenshot seven thousand units off its own canvas.
    /// So the shift goes on when the box goes on, comes off when it comes off, and the
    /// rest of the time the source is left exactly as it was found.
    fn background_source(&self, params: &BackgroundParams) -> Bounds {
        let Some(scene) = self.canvas.scene() else { return Bounds::new(0.0, 0.0, 0.0, 0.0) };
        let source = scene.picture_rect();
        let was = scene.background().is_some_and(|(_, stored)| stored.auto_balance);
        if was == params.auto_balance {
            return source;
        }
        let Some(pixels) = self.canvas.base_pixels() else { return source };
        let Some(content) = background::content(&pixels) else { return source };
        let (dx, dy) = background::balance_shift(source, content);
        let sign = if params.auto_balance { 1.0 } else { -1.0 };
        Bounds::new(source.x + dx * sign, source.y + dy * sign, source.width, source.height)
    }

    /// `spec/05` §2's Background entry: a toolbar toggle, not a tool that draws, so it
    /// does not wear the active tool's accent when its panel is open (`spec/13` #7).
    pub(super) fn build_background_button(self: &Rc<Self>) -> gtk::Widget {
        let button = gtk::ToggleButton::builder()
            .icon_name("image-x-generic-symbolic")
            .tooltip_text("Background (G)")
            .build();
        {
            let editor = Rc::downgrade(self);
            button.connect_toggled(move |button| {
                let Some(editor) = editor.upgrade() else { return };
                editor.show_background_panel(button.is_active());
            });
        }
        *self.background.toggle.borrow_mut() = Some(button.clone());
        button.upcast()
    }

    /// The window actions the presets menu names.
    pub(super) fn install_background_actions(self: &Rc<Self>) {
        let toggle = gio::SimpleAction::new("background", None);
        {
            let editor = Rc::downgrade(self);
            toggle.connect_activate(move |_, _| {
                let Some(editor) = editor.upgrade() else { return };
                let open = editor.background_panel_open();
                editor.show_background_panel(!open);
            });
        }
        self.window.add_action(&toggle);

        let previous = gio::SimpleAction::new("background-previous", None);
        {
            let editor = Rc::downgrade(self);
            previous.connect_activate(move |_, _| {
                let Some(editor) = editor.upgrade() else { return };
                match crate::settings::background_last() {
                    Some(params) => editor.apply_background_preset(&params),
                    None => editor.toast("No background has been used yet", None),
                }
            });
        }
        self.window.add_action(&previous);

        let apply = gio::SimpleAction::new("background-preset", Some(glib::VariantTy::STRING));
        {
            let editor = Rc::downgrade(self);
            apply.connect_activate(move |_, parameter| {
                let Some(editor) = editor.upgrade() else { return };
                let Some(id) = parameter.and_then(glib::Variant::str) else { return };
                let Some(preset) =
                    crate::settings::background_presets().into_iter().find(|p| p.id == id)
                else {
                    return;
                };
                editor.apply_background_preset(&preset.params);
            });
        }
        self.window.add_action(&apply);

        let default = gio::SimpleAction::new("background-default", Some(glib::VariantTy::STRING));
        {
            let editor = Rc::downgrade(self);
            default.connect_activate(move |_, parameter| {
                let Some(editor) = editor.upgrade() else { return };
                let id = parameter.and_then(glib::Variant::str).unwrap_or_default();
                crate::settings::set_background_default(id);
                if id.is_empty() {
                    editor.toast("New screenshots will have no background", None);
                } else {
                    // A default is only ever used by `ann-background-auto`, so choosing
                    // one turns that on: before D167 it was off with nothing to turn it
                    // on, and the toast promised a background no screenshot ever got.
                    crate::settings::set_background_auto(true);
                    editor.toast("New screenshots will get this background", None);
                }
            });
        }
        self.window.add_action(&default);

        // The submenu's check item, which Settings → Annotate's switch also writes
        // (D167). Named for its key, as `gio::Settings::create_action` names it.
        if let Some(auto) = crate::settings::background_auto_action() {
            self.window.add_action(&auto);
        }

        let save = gio::SimpleAction::new("background-save", None);
        {
            let editor = Rc::downgrade(self);
            save.connect_activate(move |_, _| {
                if let Some(editor) = editor.upgrade() {
                    editor.ask_for_preset_name();
                }
            });
        }
        self.window.add_action(&save);

        let delete = gio::SimpleAction::new("background-delete", None);
        {
            let editor = Rc::downgrade(self);
            delete.connect_activate(move |_, _| {
                if let Some(editor) = editor.upgrade() {
                    editor.ask_which_preset_to_delete();
                }
            });
        }
        self.window.add_action(&delete);
    }

    /// `spec/05` §4.13's sidebar, built once with the window.
    pub(super) fn build_background_sidebar(self: &Rc<Self>) -> gtk::Widget {
        install_background_css();
        let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
        column.set_margin_top(12);
        column.set_margin_bottom(12);
        column.set_margin_start(12);
        column.set_margin_end(12);

        column.append(&self.build_presets_row());
        column.append(&self.build_none_button());
        column.append(&section("Gradients", &self.build_gradients()));
        column.append(&section("Wallpapers", &self.build_wallpapers()));
        column.append(&section("Blurred", &self.build_blurred()));
        column.append(&section("Plain color", &self.build_plain()));
        column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        column.append(&self.build_spacing());
        column.append(&self.build_weight());
        column.append(&self.build_placement());

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_child(Some(&column));
        scroller.set_vexpand(true);
        scroller.set_width_request(WIDTH);

        // A header of its own, because the sidebar is the full window height and the
        // window's own controls are on the other side of the split: without one, the
        // panel's first row would sit level with the title bar and read as part of it.
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&adw::WindowTitle::new("Background", "")));
        header.set_show_end_title_buttons(false);
        let close = gtk::Button::from_icon_name("sidebar-show-symbolic");
        close.set_tooltip_text(Some("Hide the background panel (G)"));
        {
            let editor = Rc::downgrade(self);
            close.connect_clicked(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.show_background_panel(false);
                }
            });
        }
        header.pack_end(&close);

        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&scroller));
        view.upcast()
    }

    /// §4.13's first row: "A `Presets…` menu plus a `+` button."
    fn build_presets_row(self: &Rc<Self>) -> gtk::Widget {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let menu = gtk::MenuButton::new();
        menu.set_label("Presets…");
        menu.set_hexpand(true);
        menu.set_menu_model(Some(&presets_menu()));
        *self.background.presets.borrow_mut() = Some(menu.clone());
        row.append(&menu);

        let add = gtk::Button::from_icon_name("list-add-symbolic");
        add.set_tooltip_text(Some("Save these settings as a preset"));
        {
            let editor = Rc::downgrade(self);
            add.connect_clicked(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.ask_for_preset_name();
                }
            });
        }
        row.append(&add);
        row.upcast()
    }

    /// §4.13's "A wide **None** button showing the active background, selected when there
    /// is none".
    fn build_none_button(self: &Rc<Self>) -> gtk::Widget {
        let button = self.source_button(Background::None, "No background");
        button.set_hexpand(true);
        // The swatch with its name over it, because a wide chequerboard on its own is a
        // pattern rather than a choice -- and this is the row a user lands on to undo
        // everything the panel did.
        if let Some(swatch) = button.child().and_downcast::<Swatch>() {
            swatch.set_height_request(46);
            // Unparented first: the swatch is the button's child, and handing a parented
            // widget to `Overlay::set_child` is a GTK warning and an empty overlay.
            button.set_child(None::<&gtk::Widget>);
            let label = gtk::Label::new(Some("None"));
            label.add_css_class("heading");
            label.add_css_class("octosnap-on-swatch");
            let stack = gtk::Overlay::new();
            stack.set_child(Some(&swatch));
            stack.add_overlay(&label);
            button.set_child(Some(&stack));
        }
        button.upcast()
    }

    /// §4.13's "4 × 5 grid of twenty gradient swatches with a **Show less** disclosure".
    fn build_gradients(self: &Rc<Self>) -> gtk::Widget {
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let grid = gtk::Box::new(gtk::Orientation::Vertical, 6);
        *self.background.gradients.borrow_mut() = Some(grid.clone());
        holder.append(&grid);

        let disclosure = gtk::Button::with_label("Show less");
        disclosure.add_css_class("flat");
        disclosure.set_halign(gtk::Align::Center);
        *self.background.disclosure.borrow_mut() = Some(disclosure.clone());
        {
            let editor = Rc::downgrade(self);
            disclosure.connect_clicked(move |_| {
                let Some(editor) = editor.upgrade() else { return };
                let was = editor.background.expanded.get();
                editor.background.expanded.set(!was);
                editor.fill_gradients();
            });
        }
        holder.append(&disclosure);
        self.background.expanded.set(true);
        self.fill_gradients();
        holder.upcast()
    }

    /// Fills the gradient grid, at whatever the disclosure last said.
    fn fill_gradients(self: &Rc<Self>) {
        let Some(grid) = self.background.gradients.borrow().clone() else { return };
        while let Some(child) = grid.first_child() {
            grid.remove(&child);
        }
        // The buttons that are about to be dropped must go from the list too, or the
        // sync would check a widget with no parent and leave the visible grid unmarked.
        self.background
            .choices
            .borrow_mut()
            .retain(|(background, _)| !matches!(background, Background::Gradient { .. }));

        let expanded = self.background.expanded.get();
        let showing = if expanded { GRADIENT_SET.len() } else { COLLAPSED };
        let mut row = new_row();
        for (index, gradient) in GRADIENT_SET.iter().take(showing).enumerate() {
            if index > 0 && index % 4 == 0 {
                grid.append(&row);
                row = new_row();
            }
            let button =
                self.source_button(Background::Gradient { id: gradient.id }, gradient.name);
            row.append(&button);
        }
        grid.append(&row);
        if let Some(disclosure) = self.background.disclosure.borrow().clone() {
            disclosure.set_label(if expanded { "Show less" } else { "Show more" });
        }
        self.sync_background_panel();
    }

    /// §4.13's "The user's own images as thumbnails, plus a dashed `+` tile to add one".
    fn build_wallpapers(self: &Rc<Self>) -> gtk::Widget {
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 6);
        *self.background.wallpapers.borrow_mut() = Some(holder.clone());
        self.fill_wallpapers();
        holder.upcast()
    }

    fn fill_wallpapers(self: &Rc<Self>) {
        let Some(holder) = self.background.wallpapers.borrow().clone() else { return };
        while let Some(child) = holder.first_child() {
            holder.remove(&child);
        }
        self.background
            .choices
            .borrow_mut()
            .retain(|(background, _)| !matches!(background, Background::Image { .. }));

        let mut row = new_row();
        let mut count = 0;
        for file in wallpaper_files() {
            if count > 0 && count % 4 == 0 {
                holder.append(&row);
                row = new_row();
            }
            let name = std::path::Path::new(&file)
                .file_stem()
                .map_or_else(|| file.clone(), |stem| stem.to_string_lossy().into_owned());
            let button = self.source_button(Background::Image { file: file.clone() }, &name);
            if let Some(swatch) = button.child().and_downcast::<Swatch>() {
                swatch.set_wallpaper(gtk::gdk::Texture::from_filename(&file).ok());
            }
            row.append(&button);
            count += 1;
        }
        if count > 0 && count % 4 == 0 {
            holder.append(&row);
            row = new_row();
        }

        // The dashed `+` tile, last, because it is where the row of thumbnails ends.
        let add = gtk::Button::from_icon_name("list-add-symbolic");
        add.add_css_class("octosnap-add-tile");
        add.set_size_request(SWATCH_FLOOR, SWATCH.1);
        add.set_tooltip_text(Some("Add an image to use as a background"));
        {
            let editor = Rc::downgrade(self);
            add.connect_clicked(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.choose_wallpaper();
                }
            });
        }
        row.append(&add);
        holder.append(&row);
    }

    /// §4.13's "Three swatches that derive a background from the screenshot itself".
    fn build_blurred(self: &Rc<Self>) -> gtk::Widget {
        let row = new_row();
        for strength in Blur::ALL {
            row.append(&self.source_button(Background::Blurred { strength }, strength.name()));
        }
        row.upcast()
    }

    /// §4.13's "Two rows of solid swatches, the last opening a custom picker".
    fn build_plain(self: &Rc<Self>) -> gtk::Widget {
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let mut row = new_row();
        for (index, (name, packed)) in PLAIN.into_iter().enumerate() {
            if index > 0 && index % 5 == 0 {
                holder.append(&row);
                row = new_row();
            }
            row.append(&self.source_button(
                Background::Color { color: background::rgb(packed) },
                name,
            ));
        }

        // The custom picker, at the end of the second row: §2's own colour popover, so
        // the ten saved colours a user built up while annotating are here too.
        let custom = gtk::MenuButton::new();
        custom.set_icon_name("color-select-symbolic");
        custom.set_tooltip_text(Some("Custom colour"));
        custom.set_size_request(SWATCH_FLOOR, SWATCH.1);
        let current = match self.background_params().background {
            Background::Color { color } => color,
            _ => background::rgb(0xFF_FF_FF),
        };
        let picker = Picker::new(current, false, crate::settings::my_colors());
        {
            let editor = Rc::downgrade(self);
            picker.connect_changed(move |color| {
                let Some(editor) = editor.upgrade() else { return };
                let params = editor.background_params();
                editor.apply_background(&params.with(Background::Color { color }), false);
            });
        }
        custom.set_popover(Some(picker.popover()));
        *self.background.picker.borrow_mut() = Some(picker);
        row.append(&custom);
        holder.append(&row);
        holder.upcast()
    }

    /// §4.13's Padding row and its Inset row, with Auto-balance beside the second.
    fn build_spacing(self: &Rc<Self>) -> gtk::Widget {
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 10);

        let (padding_row, padding) = slider("Padding", MAX_PADDING, None);
        *self.background.padding.borrow_mut() = Some(padding.clone());
        self.on_change(&padding, |params, value| params.padding = value);
        holder.append(&padding_row);

        // §4.13: "**Auto-balance sits next to Inset**, not off in a menu, because it
        // modifies that spacing."
        let balance = gtk::CheckButton::with_label("Auto");
        balance.set_tooltip_text(Some(
            "Trim the capture's uniform border and centre what is left, so the picture \
             looks balanced rather than merely centred.",
        ));
        let (inset_row, inset) = slider("Inset", MAX_INSET, Some(&balance));
        *self.background.inset.borrow_mut() = Some(inset.clone());
        self.on_change(&inset, |params, value| params.inset = value);
        {
            let editor = Rc::downgrade(self);
            balance.connect_toggled(move |button| {
                let Some(editor) = editor.upgrade() else { return };
                if editor.background.syncing.get() {
                    return;
                }
                let mut params = editor.background_params();
                params.auto_balance = button.is_active();
                editor.apply_background(&params, false);
            });
        }
        *self.background.balance.borrow_mut() = Some(balance);
        holder.append(&inset_row);
        holder.upcast()
    }

    /// §4.13's "Two sliders side by side".
    fn build_weight(self: &Rc<Self>) -> gtk::Widget {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.set_homogeneous(true);

        let (shadow_row, shadow) = slider("Shadow", MAX_SHADOW, None);
        *self.background.shadow.borrow_mut() = Some(shadow.clone());
        self.on_change(&shadow, |params, value| params.shadow_intensity = value);
        row.append(&shadow_row);

        // Stored as a fraction of the shorter side (appendix A.7) and shown as a
        // percentage, because 0.08 is not a number anyone has an opinion about.
        let (corners_row, corners) = slider("Corners", MAX_CORNER_RADIUS * 100.0, None);
        *self.background.corners.borrow_mut() = Some(corners.clone());
        self.on_change(&corners, |params, value| params.corner_radius = value / 100.0);
        row.append(&corners_row);
        row.upcast()
    }

    /// §4.13's "A 3 × 3 alignment grid, centre selected, beside a **Ratio** dropdown".
    fn build_placement(self: &Rc<Self>) -> gtk::Widget {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.set_valign(gtk::Align::End);

        let grid = gtk::Grid::new();
        grid.set_row_spacing(2);
        grid.set_column_spacing(2);
        grid.add_css_class("linked");
        grid.set_valign(gtk::Align::Center);
        let mut first: Option<gtk::ToggleButton> = None;
        let mut buttons = Vec::new();
        for index in 0_u8..9 {
            let button = gtk::ToggleButton::new();
            button.add_css_class("octosnap-align");
            button.set_tooltip_text(Some(alignment_name(index)));
            match &first {
                Some(anchor) => button.set_group(Some(anchor)),
                None => first = Some(button.clone()),
            }
            {
                let editor = Rc::downgrade(self);
                button.connect_toggled(move |button| {
                    if !button.is_active() {
                        return;
                    }
                    let Some(editor) = editor.upgrade() else { return };
                    if editor.background.syncing.get() {
                        return;
                    }
                    let mut params = editor.background_params();
                    params.alignment = index;
                    editor.apply_background(&params, false);
                });
            }
            grid.attach(&button, i32::from(index % 3), i32::from(index / 3), 1, 1);
            buttons.push(button);
        }
        *self.background.alignment.borrow_mut() = buttons;
        row.append(&grid);

        let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
        column.set_hexpand(true);
        column.append(&caption("Ratio"));
        let labels: Vec<String> = std::iter::once("Auto".to_owned())
            .chain(Ratio::ALL.iter().map(|ratio| ratio.label()))
            .collect();
        let strings: Vec<&str> = labels.iter().map(String::as_str).collect();
        let ratio = gtk::DropDown::from_strings(&strings);
        {
            let editor = Rc::downgrade(self);
            ratio.connect_selected_notify(move |drop| {
                let Some(editor) = editor.upgrade() else { return };
                if editor.background.syncing.get() {
                    return;
                }
                let mut params = editor.background_params();
                params.ratio = usize::try_from(drop.selected())
                    .ok()
                    .and_then(|index| index.checked_sub(1))
                    .and_then(|index| Ratio::ALL.get(index).copied());
                editor.apply_background(&params, false);
            });
        }
        *self.background.ratio.borrow_mut() = Some(ratio.clone());
        column.append(&ratio);
        row.append(&column);
        row.upcast()
    }

    /// Wires one adjustment to one field of the parameters.
    ///
    /// The step stays open while the pointer is down -- `value-changed` fires per pixel
    /// of drag -- and closes on the next committed edit, which is what `commit_batch_open`
    /// is for. Without it a slider dragged across its range would be four hundred steps
    /// in the undo history.
    fn on_change(
        self: &Rc<Self>,
        adjustment: &gtk::Adjustment,
        set: fn(&mut BackgroundParams, f64),
    ) {
        let editor = Rc::downgrade(self);
        adjustment.connect_value_changed(move |adjustment| {
            let Some(editor) = editor.upgrade() else { return };
            if editor.background.syncing.get() {
                return;
            }
            let mut params = editor.background_params();
            set(&mut params, adjustment.value());
            editor.apply_background(&params, true);
        });
    }

    /// One swatch, as a radio button in the group every source shares.
    fn source_button(self: &Rc<Self>, background: Background, name: &str) -> gtk::ToggleButton {
        let swatch = Swatch::new();
        swatch.set_background(background.clone());
        swatch.set_size_request(SWATCH_FLOOR, SWATCH.1);
        if matches!(background, Background::Blurred { .. }) {
            swatch.set_wallpaper(self.canvas.base_texture());
        }
        let button = gtk::ToggleButton::new();
        button.add_css_class("octosnap-swatch");
        button.set_child(Some(&swatch));
        button.set_tooltip_text(Some(name));
        if let Some((_, anchor)) = self.background.choices.borrow().first() {
            button.set_group(Some(anchor));
        }
        {
            let editor = Rc::downgrade(self);
            let chosen = background.clone();
            button.connect_toggled(move |button| {
                if !button.is_active() {
                    return;
                }
                let Some(editor) = editor.upgrade() else { return };
                if editor.background.syncing.get() {
                    return;
                }
                let params = editor.background_params().with(chosen.clone());
                // None takes the background away, margin, corners, shadow and all, and
                // gives the canvas back (D166). Keeping the rest, it left a margin of
                // nothing with the picture's shadow in it, and only sliding the padding
                // and the inset to zero as well took it off. A margin of nothing is
                // still there to make: None, then the padding.
                let params = if chosen.is_none() {
                    BackgroundParams { padding: 0.0, inset: 0.0, ..params }
                } else {
                    params
                };
                editor.apply_background(&params, false);
            });
        }
        self.background.choices.borrow_mut().push((background, button.clone()));
        button
    }

    /// Puts every control where the document says it is.
    ///
    /// Called after every edit this panel makes *and* after every undo, which is the case
    /// that makes it necessary: Ctrl+Z on a padding drag has to move the slider back, and
    /// nothing else would.
    pub(super) fn sync_background_panel(&self) {
        if self.background.syncing.replace(true) {
            return;
        }
        let params = self.background_params();
        let present = self.canvas.scene().is_some_and(|scene| scene.background().is_some());
        let active = if present { params.background.clone() } else { Background::None };
        let capture = self.canvas.base_texture();
        for (background, button) in self.background.choices.borrow().iter() {
            button.set_active(*background == active);
            // The three blurred swatches are the capture, and the capture is uploaded
            // after the window is built -- so they are filled in here rather than when
            // they were made, and every sync is a chance to catch the first frame that
            // has one.
            if matches!(background, Background::Blurred { .. })
                && let Some(swatch) = swatch_of(button)
                && swatch.wallpaper().is_none()
            {
                swatch.set_wallpaper(capture.clone());
            }
        }
        if let Some(adjustment) = self.background.padding.borrow().as_ref() {
            adjustment.set_value(params.padding);
        }
        if let Some(adjustment) = self.background.inset.borrow().as_ref() {
            adjustment.set_value(params.inset);
        }
        if let Some(adjustment) = self.background.shadow.borrow().as_ref() {
            adjustment.set_value(params.shadow_intensity);
        }
        if let Some(adjustment) = self.background.corners.borrow().as_ref() {
            adjustment.set_value(params.corner_radius * 100.0);
        }
        if let Some(balance) = self.background.balance.borrow().as_ref() {
            balance.set_active(params.auto_balance);
        }
        if let Some(button) =
            self.background.alignment.borrow().get(usize::from(params.alignment.min(8)))
        {
            button.set_active(true);
        }
        if let Some(drop) = self.background.ratio.borrow().as_ref() {
            let index = params.ratio.and_then(|wanted| {
                Ratio::ALL.iter().position(|ratio| *ratio == wanted).map(|at| at + 1)
            });
            drop.set_selected(u32::try_from(index.unwrap_or(0)).unwrap_or(0));
        }
        self.background.syncing.set(false);
    }

    /// Shows or hides the sidebar, keeping the toolbar's toggle in step.
    pub(super) fn show_background_panel(&self, show: bool) {
        if let Some(split) = self.background.split.borrow().as_ref() {
            // Focus in the sidebar as it goes is GTK's to put somewhere, and it chose the
            // Background toggle, which the next Enter pressed: back to the canvas, as
            // crop mode does (D166).
            let inside = !show
                && split.sidebar().is_some_and(|sidebar| {
                    gtk::prelude::GtkWindowExt::focus(&self.window)
                        .is_some_and(|focus| focus.is_ancestor(&sidebar))
                });
            split.set_show_sidebar(show);
            if inside {
                self.canvas.grab_focus();
            }
        }
        if let Some(toggle) = self.background.toggle.borrow().as_ref()
            && toggle.is_active() != show
        {
            toggle.set_active(show);
        }
        if show {
            self.sync_background_panel();
            self.log_none_button();
        }
        // `spec/08` §1's "Remember if background tool was opened", written on every
        // change rather than on close: an editor that is killed, or a session that ends
        // with one open, still remembered the last thing the user actually did.
        crate::settings::set_background_panel_was_open(show);
    }

    /// Where the None button is in the window once the sidebar has slid in, for
    /// `editor-test.sh` to click it (D166), as `canvas allocated` is logged for its drags.
    /// Logged when it has stood still for ten frames, which is after the slide.
    fn log_none_button(&self) {
        // Once a showing: the toggle's own handler shows the panel a second time.
        if self.background.none_logging.replace(true) {
            return;
        }
        let choices = self.background.choices.borrow();
        let Some((_, none)) = choices.iter().find(|(background, _)| background.is_none()) else {
            self.background.none_logging.set(false);
            return;
        };
        let last = Cell::new(None::<(i32, i32, i32, i32)>);
        let still = Cell::new(0_u32);
        let logging = self.background.none_logging.clone();
        none.add_tick_callback(move |button, _| {
            let bounds = button.root().and_then(|root| button.compute_bounds(&root));
            #[allow(clippy::cast_possible_truncation)]
            let rect = bounds.map(|b| {
                (b.x() as i32, b.y() as i32, b.width() as i32, b.height() as i32)
            });
            if rect.is_none_or(|r| r.2 <= 0) || last.replace(rect) != rect {
                still.set(0);
                return glib::ControlFlow::Continue;
            }
            still.set(still.get() + 1);
            if still.get() < 10 {
                return glib::ControlFlow::Continue;
            }
            if let Some((x, y, width, height)) = rect {
                tracing::debug!(x, y, width, height, "background none button");
            }
            logging.set(false);
            glib::ControlFlow::Break
        });
    }

    /// The same row, read: whether a newly opened editor starts with the panel showing.
    pub(super) fn restore_background_panel(self: &Rc<Self>) {
        if crate::settings::background_panel_opens() {
            self.show_background_panel(true);
        }
    }

    /// `spec/08` §2's "Capture window shadow", turned off.
    ///
    /// The compositor drew the shadow into the capture's alpha before anyone here saw it
    /// (`extension/src/capture.ts`), so the setting cannot add one and this is the whole
    /// of what turning it off can mean: crop the canvas to the window's own body. It runs
    /// before §4.15's background is applied, because the background is laid out over
    /// whatever the canvas is -- and it is not a `Command`, because it is how the capture
    /// was *read* rather than something the user did to it. Ctrl+Z after opening should
    /// not put the compositor's shadow back any more than it should un-decode the PNG.
    pub(super) fn trim_window_shadow(self: &Rc<Self>) {
        if crate::settings::window_shadow() {
            return;
        }
        let Some(mut scene) = self.canvas.scene() else { return };
        let Some(pixels) = self.canvas.base_pixels() else { return };
        let Some(body) = background::window_body(&pixels) else { return };
        let Some(per_unit) = self.canvas.base_pixel_scale() else { return };
        if per_unit <= 0.0 {
            return;
        }
        // `window_body` answers in the stored image's pixels, and the canvas is in
        // document units; §4.12 could have turned the document between the two, which it
        // has not at open, but the arithmetic should not be the thing that depends on it.
        let orientation = scene.base.orientation;
        let (width, height) = (scene.base.width, scene.base.height);
        let corner = |x: f64, y: f64| {
            orientation.place(Point::new(x / per_unit, y / per_unit), width, height)
        };
        let near = corner(body.x, body.y);
        let far = corner(body.x + body.width, body.y + body.height);
        scene.canvas = Bounds::new(
            near.x.min(far.x),
            near.y.min(far.y),
            (far.x - near.x).abs(),
            (far.y - near.y).abs(),
        );
        self.canvas.update_scene(scene);
        self.canvas.center();
        self.refresh_size();
    }

    /// The Presets menu again, after the saved list changed.
    fn refresh_presets_menu(&self) {
        if let Some(menu) = self.background.presets.borrow().as_ref() {
            menu.set_menu_model(Some(&presets_menu()));
        }
    }

    /// §4.13's "Add New Preset…": a name, then the current parameters under it.
    fn ask_for_preset_name(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::new(Some("Save background preset"), None);
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some("Preset name"));
        entry.set_activates_default(true);
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("save", "Save");
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("save"));
        dialog.set_close_response("cancel");
        let editor = Rc::downgrade(self);
        dialog.connect_response(None, move |dialog, response| {
            if response != "save" {
                return;
            }
            let Some(editor) = editor.upgrade() else { return };
            let name = entry.text().trim().to_owned();
            let name = if name.is_empty() { "Preset".to_owned() } else { name };
            let mut presets = crate::settings::background_presets();
            presets.push(octosnap_scene::Preset::new(name.clone(), editor.background_params()));
            crate::settings::set_background_presets(&presets);
            editor.refresh_presets_menu();
            editor.toast(&format!("Saved “{name}”"), None);
            dialog.close();
        });
        dialog.present(Some(&self.window));
    }

    /// §4.13's "delete": one preset, chosen from a list.
    fn ask_which_preset_to_delete(self: &Rc<Self>) {
        let presets = crate::settings::background_presets();
        if presets.is_empty() {
            self.toast("There are no presets to delete", None);
            return;
        }
        let dialog = adw::AlertDialog::new(Some("Delete background preset"), None);
        let names: Vec<String> = presets.iter().map(|preset| preset.name.clone()).collect();
        let strings: Vec<&str> = names.iter().map(String::as_str).collect();
        let list = gtk::DropDown::from_strings(&strings);
        dialog.set_extra_child(Some(&list));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_close_response("cancel");
        let editor = Rc::downgrade(self);
        dialog.connect_response(None, move |dialog, response| {
            if response != "delete" {
                return;
            }
            let Some(editor) = editor.upgrade() else { return };
            let mut presets = crate::settings::background_presets();
            let at = usize::try_from(list.selected()).unwrap_or(0);
            if at < presets.len() {
                let gone = presets.remove(at);
                // The default cannot point at something that is not there any more.
                if crate::settings::background_default()
                    .is_some_and(|preset| preset.id == gone.id)
                {
                    crate::settings::set_background_default("");
                }
                crate::settings::set_background_presets(&presets);
                editor.refresh_presets_menu();
                editor.toast(&format!("Deleted “{}”", gone.name), None);
            }
            dialog.close();
        });
        dialog.present(Some(&self.window));
    }

    /// §4.13's dashed `+` tile: an image copied into the user's backgrounds folder.
    ///
    /// Copied rather than referenced, because §4.13 says the custom images are "stored in
    /// `~/.local/share/octosnap/backgrounds/`" -- a background that stopped working when
    /// the user tidied their Downloads folder would be a poor kind of preset.
    fn choose_wallpaper(self: &Rc<Self>) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Images"));
        for mime in ["image/png", "image/jpeg", "image/webp", "image/avif"] {
            filter.add_mime_type(mime);
        }
        let dialog = gtk::FileDialog::builder()
            .title("Choose a background image")
            .default_filter(&filter)
            .modal(true)
            .build();
        let editor = Rc::downgrade(self);
        dialog.open(Some(&self.window), gio::Cancellable::NONE, move |answer| {
            let Some(editor) = editor.upgrade() else { return };
            let Ok(file) = answer else { return };
            let Some(path) = file.path() else { return };
            match import_wallpaper(&path) {
                Ok(stored) => {
                    editor.fill_wallpapers();
                    let params = editor.background_params();
                    editor.apply_background(
                        &params.with(Background::Image { file: stored }),
                        false,
                    );
                }
                Err(why) => editor.toast(&format!("Could not add the image: {why}"), None),
            }
        });
    }

    /// Applies a saved preset, or the last settings used.
    pub(super) fn apply_background_preset(self: &Rc<Self>, params: &BackgroundParams) {
        self.apply_background(params, false);
        crate::settings::set_background_last(params);
        self.show_background_panel(true);
    }

    /// `spec/05` §4.15's window capture: what is behind the window, as an object.
    ///
    /// > A window capture keeps `window.png` (alpha) and the background parameters
    /// > (wallpaper crop/custom/colour + padding) as an editable background object; the
    /// > panel can change or remove it.
    ///
    /// The panel can change or remove it because it *is* a §4.13 background object --
    /// there is no second kind. Shift at capture means transparent, which `spec/08` §2
    /// spells out in the same words as §4.13's preset skip ("Hold Shift while taking a
    /// screenshot to get a transparent background"), so one flag answers both.
    pub(super) fn apply_window_background(self: &Rc<Self>, shift_held: bool) {
        if shift_held {
            return;
        }
        let Some(params) = crate::settings::window_background() else { return };
        self.apply_background(&params, false);
        // Not an edit the user made, so the dot stays off (`spec/13` #11).
        if let Some(scene) = self.canvas.scene() {
            self.mark_saved(scene);
        }
    }

    /// `spec/05` §4.13's "automatically apply preset to all screenshots (skippable by
    /// holding Shift at capture)".
    ///
    /// Silent: no toast, no panel, and the document is marked as saved afterwards. The
    /// preference says every screenshot gets this background, so getting it is not news
    /// and the dot beside the document size is for marks the *user* made. One Ctrl+Z
    /// still takes it off, which is what someone who wanted this one plain will reach for.
    pub(super) fn auto_apply_background(self: &Rc<Self>, shift_held: bool) {
        if shift_held || !crate::settings::background_auto() {
            return;
        }
        if self.canvas.scene().is_some_and(|scene| scene.background().is_some()) {
            return;
        }
        let Some(preset) = crate::settings::background_default() else { return };
        self.apply_background(&preset.params, false);
        if let Some(scene) = self.canvas.scene() {
            self.mark_saved(scene);
        }
    }

    #[must_use]
    pub(super) fn background_panel_open(&self) -> bool {
        self.background
            .split
            .borrow()
            .as_ref()
            .is_some_and(adw::OverlaySplitView::shows_sidebar)
    }
}

/// Copies an image into `~/.local/share/octosnap/backgrounds/`, answering with its path.
///
/// A name collision keeps both: the second `desk.png` becomes `desk-2.png`, because a
/// silent overwrite of a background another project is already using would change that
/// project the next time it was opened.
fn import_wallpaper(from: &std::path::Path) -> Result<String, String> {
    let home = wallpaper_home();
    std::fs::create_dir_all(&home).map_err(|why| why.to_string())?;
    let stem = from.file_stem().map_or_else(|| "background".into(), std::ffi::OsStr::to_os_string);
    let extension = from.extension().map_or_else(|| "png".into(), std::ffi::OsStr::to_os_string);
    let mut to = home.join(&stem).with_extension(&extension);
    let mut n = 2;
    while to.exists() {
        to = home
            .join(format!("{}-{n}", stem.to_string_lossy()))
            .with_extension(&extension);
        n += 1;
    }
    std::fs::copy(from, &to).map_err(|why| why.to_string())?;
    Ok(to.to_string_lossy().into_owned())
}

/// The seconds a new object is stamped with, as `spec/05` §5.1's `created`.
fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// The swatch inside a source button, through the overlay the None button wraps it in.
fn swatch_of(button: &gtk::ToggleButton) -> Option<Swatch> {
    let child = button.child()?;
    match child.downcast::<Swatch>() {
        Ok(swatch) => Some(swatch),
        Err(other) => other.downcast::<gtk::Overlay>().ok()?.child()?.downcast().ok(),
    }
}

fn new_row() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.set_homogeneous(true);
    row
}

/// A section: its name, then its content. `spec/05` §4.13's own argument for them is that
/// "the background **sources are categorised** rather than dumped into one grid".
fn section(title: &str, content: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let holder = gtk::Box::new(gtk::Orientation::Vertical, 6);
    holder.append(&caption(title));
    holder.append(content);
    holder.upcast()
}

fn caption(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("dim-label");
    label.add_css_class("caption-heading");
    label
}

/// A labelled slider with the number beside it, because §4.13 ends "values can be typed
/// manually".
fn slider(
    title: &str,
    max: f64,
    beside: Option<&gtk::CheckButton>,
) -> (gtk::Widget, gtk::Adjustment) {
    let holder = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let label = caption(title);
    label.set_hexpand(true);
    head.append(&label);
    if let Some(check) = beside {
        head.append(check);
    }
    let adjustment = gtk::Adjustment::new(0.0, 0.0, max, 1.0, 10.0, 0.0);
    // Whole numbers: every one of §4.13's four is in pixels or in per cent, and a
    // padding of 100.0 reads as a measurement someone tuned rather than as a default.
    let entry = gtk::SpinButton::new(Some(&adjustment), 1.0, 0);
    entry.set_width_chars(4);
    entry.set_max_width_chars(4);
    head.append(&entry);
    holder.append(&head);

    let scale = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&adjustment));
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    holder.append(&scale);
    (holder.upcast(), adjustment)
}

/// The nine tooltips of the 3 × 3 grid, reading like the grid does.
const fn alignment_name(index: u8) -> &'static str {
    match index {
        0 => "Top left",
        1 => "Top",
        2 => "Top right",
        3 => "Left",
        4 => "Centre",
        5 => "Right",
        6 => "Bottom left",
        7 => "Bottom",
        _ => "Bottom right",
    }
}

/// `spec/05` §4.13's "custom images stored in `~/.local/share/octosnap/backgrounds/`".
#[must_use]
pub fn wallpaper_home() -> std::path::PathBuf {
    glib::user_data_dir().join("octosnap").join("backgrounds")
}

/// Every image in that folder, in a stable order.
fn wallpaper_files() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(wallpaper_home()) else { return Vec::new() };
    let mut files: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    matches!(
                        extension.to_ascii_lowercase().as_str(),
                        "png" | "jpg" | "jpeg" | "webp" | "avif"
                    )
                })
        })
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    files.sort();
    files
}

/// §4.13's presets menu: "**Apply Previous Settings**, **Default Preset** with a submenu,
/// and **Add New Preset…**".
fn presets_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let top = gio::Menu::new();
    top.append(Some("Apply Previous Settings"), Some("win.background-previous"));
    menu.append_section(None, &top);

    let saved = gio::Menu::new();
    let presets = crate::settings::background_presets();
    if presets.is_empty() {
        let empty = gio::MenuItem::new(Some("No presets yet"), None);
        empty.set_action_and_target_value(None, None);
        saved.append_item(&empty);
    }
    let defaults = gio::Menu::new();
    defaults.append(Some("None"), Some("win.background-default::"));
    for preset in &presets {
        saved.append(
            Some(&preset.name),
            Some(&format!("win.background-preset::{}", preset.id)),
        );
        defaults.append(
            Some(&preset.name),
            Some(&format!("win.background-default::{}", preset.id)),
        );
    }
    menu.append_section(Some("Presets"), &saved);
    // Shift at capture still skips it (§4.13), as Settings' row says.
    let auto = gio::Menu::new();
    auto.append(Some("Add to New Screenshots"), Some("win.ann-background-auto"));
    defaults.append_section(None, &auto);

    let bottom = gio::Menu::new();
    bottom.append_submenu(Some("Default Preset"), &defaults);
    bottom.append(Some("Add New Preset…"), Some("win.background-save"));
    bottom.append(Some("Delete a Preset…"), Some("win.background-delete"));
    menu.append_section(None, &bottom);
    menu
}

/// One swatch's paint: `spec/05` §4.13's sources, drawn by the canvas's own renderer.
mod swatch {
    use super::{Background, glib};
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    /// The radius of a swatch's corners, in widget pixels.
    const RADIUS: f32 = 6.0;

    #[derive(Debug, Default)]
    pub struct Swatch {
        pub background: std::cell::RefCell<Background>,
        /// The capture for a blurred swatch, or the file for an image one.
        pub texture: std::cell::RefCell<Option<gtk::gdk::Texture>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Swatch {
        const NAME: &'static str = "OctoSnapBackgroundSwatch";
        type Type = super::Swatch;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Swatch {}

    impl WidgetImpl for Swatch {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (width, height) = (f64::from(widget.width()), f64::from(widget.height()));
            if width <= 0.0 || height <= 0.0 {
                return;
            }
            let bounds = octosnap_scene::Bounds::new(0.0, 0.0, width, height);
            #[allow(clippy::cast_possible_truncation)]
            let rect = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
            let rounded = gtk::gsk::RoundedRect::from_rect(rect, RADIUS);
            snapshot.push_rounded_clip(&rounded);
            // The chequer under a transparent swatch, so §4.13's None reads as "nothing
            // behind the picture" rather than as "a grey background".
            let pale = gtk::gdk::RGBA::new(0.86, 0.86, 0.88, 1.0);
            let dark = gtk::gdk::RGBA::new(0.74, 0.74, 0.76, 1.0);
            snapshot.append_color(&pale, &rect);
            let cell = 6.0_f32;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let cols = (width as f32 / cell).ceil() as i32;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let rows = (height as f32 / cell).ceil() as i32;
            for row in 0..rows {
                for col in 0..cols {
                    if (row + col) % 2 == 0 {
                        continue;
                    }
                    #[allow(clippy::cast_precision_loss)]
                    snapshot.append_color(
                        &dark,
                        &gtk::graphene::Rect::new(
                            col as f32 * cell,
                            row as f32 * cell,
                            cell,
                            cell,
                        ),
                    );
                }
            }
            let held = self.texture.borrow();
            crate::editor::nodes::paint_background(
                snapshot,
                &self.background.borrow(),
                bounds,
                held.as_ref(),
                held.as_ref(),
            );
            snapshot.pop();
            // A hairline, so a white swatch on a white panel is still a swatch.
            let edge = gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.18);
            snapshot.append_border(&rounded, &[1.0; 4], &[edge; 4]);
        }
    }
}

glib::wrapper! {
    /// One background source, drawn the way the canvas draws it.
    pub struct Swatch(ObjectSubclass<swatch::Swatch>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for Swatch {
    fn default() -> Self {
        Self::new()
    }
}

impl Swatch {
    #[must_use]
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn set_background(&self, background: Background) {
        *self.imp().background.borrow_mut() = background;
        self.queue_draw();
    }

    /// The capture, for a blurred swatch; the file, for a wallpaper one.
    pub fn set_wallpaper(&self, texture: Option<gtk::gdk::Texture>) {
        *self.imp().texture.borrow_mut() = texture;
        self.queue_draw();
    }

    #[must_use]
    pub fn wallpaper(&self) -> Option<gtk::gdk::Texture> {
        self.imp().texture.borrow().clone()
    }
}

/// The panel's own stylesheet, once per process (the reason is `install_editor_css`'s).
fn install_background_css() {
    thread_local! {
        static INSTALLED: Cell<bool> = const { Cell::new(false) };
    }
    if INSTALLED.replace(true) {
        return;
    }
    let Some(display) = gtk::gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        "
/* A swatch is its picture and a ring, not a button with a picture inside it. */
.octosnap-swatch {
    padding: 0;
    min-width: 0;
    min-height: 0;
    background: none;
    box-shadow: none;
    border: none;
    border-radius: 7px;
}
.octosnap-swatch:checked,
.octosnap-swatch:focus-visible {
    outline: 2px solid @accent_color;
    outline-offset: 2px;
    background: none;
}
/* `spec/05` §4.13's \"dashed `+` tile\". */
.octosnap-add-tile {
    border: 1px dashed alpha(currentColor, 0.45);
    border-radius: 7px;
    background: none;
}
.octosnap-align {
    min-width: 18px;
    min-height: 18px;
    padding: 0;
}
.octosnap-align:checked {
    background-color: @accent_bg_color;
    color: @accent_fg_color;
}
/* Legible over whatever the swatch under it turns out to be. */
.octosnap-on-swatch {
    color: #1b1b1b;
    text-shadow: 0 1px 2px alpha(#ffffff, 0.75);
}
",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
