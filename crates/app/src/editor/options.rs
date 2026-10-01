// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §2's options row: the controls that change per tool -- and per selection.
//!
//! The row is built from data. `scene::tool::Tool::controls()` is §2's Options column as a
//! list of [`Control`]s, and this file has one arm per control -- so a row that is wrong is
//! wrong in the crate where a test can compare it against the spec, rather than in a
//! `match` on the tool buried in a widget. §2's rows are [V], verified against screenshots,
//! and getting a verified fact wrong silently is the failure worth designing against.
//!
//! Three rules shape every control here.
//!
//! **The row follows the selection (D56).** With something selected, the row shows the
//! controls of the tool that draws that kind of object, filled with *that object's* values,
//! and every change lands on the selection. With nothing selected it shows the active
//! tool's controls and its remembered values, for the next object. The first version showed
//! the tool's row only, which left a selected box with no way to change its colour -- the
//! Select tool's row is an em dash in §2 -- and that is exactly what was reported from
//! hardware: "after already created a box, changing the color of the box outline is not
//! possible".
//!
//! **One control writes one value.** A change goes through [`Control::apply`], which
//! touches the one field the control owns, so recolouring a selection of two arrows of
//! different weights leaves both weights alone.
//!
//! **Sliders are for continuous perceptual values and popovers for discrete choices.**
//! §4.8 states it outright -- the spotlight's opacity is "the one place a real slider
//! appears; colour and size both use popovers instead" -- and it is why size is six drawn
//! previews in a popover rather than the 1-6 scale it obviously could have been.
//!
//! And one rule about widgets: **the row is rebuilt only when its control set changes.** A
//! value change updates the controls in place through the refreshers each one registers.
//! Rebuilding on every change was the first version, and it is what made "changing
//! properties" feel clunky: the popover the user was in vanished under them, and a menu
//! whose value they had just picked was a new widget with no memory of being open.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use octosnap_scene::tool::{Control, Tool, ToolSettings};
use octosnap_scene::{
    ArrowHead, ArrowStyle, COUNTER_SIZES, Command, CounterStyle, FONT_SIZES, RedactStyle, Rgba,
    SIZE_LEVELS, SpotlightShape, TextStyle,
};

use super::picker::Picker;
use super::preview::{Kind, Preview};
use super::window::Editor;

/// `[` and `]`'s step on the row's intensity slider: a tenth of its range (D137).
const INTENSITY_STEP: f64 = 0.1;

/// One control's way of showing fresh settings without being rebuilt.
type Refresher = Box<dyn Fn(&ToolSettings)>;

/// A choice's label, and its picture when it has one (`choice_menu`).
type Names<T> = (fn(T) -> &'static str, Option<fn(T) -> &'static str>);

/// One line per rebuild **and per value change**, and it exists to be grepped. `spec/05`
/// §2's per-tool row and its per-tool memory are both invisible from outside -- a colour
/// is a pixel in a popover and the memory is a map -- so `editor-test.sh` reads this
/// instead. It moved out of `refresh_options` when the row stopped rebuilding on every
/// change: a harness that read the last line was then reading the value *before* the
/// digit it had just pressed.
fn log_row(tool: Tool, settings: &ToolSettings, selection: bool) {
    tracing::debug!(
        tool = tool.label(),
        controls = tool.controls().len(),
        size = settings.style.size,
        color = settings.style.color.to_hex(),
        shadow = settings.style.shadow,
        selection,
        "options row"
    );
}

/// Where the row's state lives, beside the tool strip's.
#[derive(Default)]
pub struct OptionsRow {
    /// The row itself, once built.
    pub row: RefCell<Option<gtk::Box>>,
    /// Whose controls the row is showing: the selection's tool (D56) or the active one.
    pub tool: Cell<Tool>,
    /// Whether the row was built over a selection. Kept so a selection change from one
    /// object to another of the same kind still rebuilds -- the *values* are new even
    /// though the control set is not, and the pickers hold values of their own.
    pub over_selection: Cell<Option<octosnap_scene::ObjectId>>,
    /// One per control on the row.
    pub refreshers: RefCell<Vec<Refresher>>,
}

impl std::fmt::Debug for OptionsRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OptionsRow").field("tool", &self.tool.get()).finish_non_exhaustive()
    }
}

impl Editor {
    /// The row, empty until a tool fills it.
    pub(super) fn build_options_row(self: &Rc<Self>) -> gtk::Widget {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_valign(gtk::Align::Center);
        *self.options.row.borrow_mut() = Some(row.clone());
        self.refresh_options();
        row.upcast()
    }

    /// Whose controls the row shows, and what it shows in them.
    ///
    /// The selection's, if there is one whose kind a tool draws (D56): the first selected
    /// object's own values, over the memory of the tool that draws it. Otherwise the active
    /// tool's remembered values. The `ObjectId` is the object the values came from.
    pub(super) fn row_context(&self) -> (Tool, ToolSettings, Option<octosnap_scene::ObjectId>) {
        if let Some(scene) = self.canvas.scene() {
            let selection = self.canvas.selection();
            if let Some(object) = selection.first().and_then(|id| scene.get(*id))
                && let Some(tool) = Tool::for_object(object)
            {
                let base = self.memory_of(tool);
                return (tool, ToolSettings::from_object(object, base), Some(object.id));
            }
        }
        let active = self.tools.active.get();
        (active, self.memory_of(active), None)
    }

    /// Rebuilds the row for whatever it should now be showing.
    ///
    /// Called when the tool or the selection changes -- the two things that can change
    /// the *set* of controls. A value change goes through [`Self::sync_options`] instead.
    pub(super) fn refresh_options(self: &Rc<Self>) {
        let Some(row) = self.options.row.borrow().clone() else { return };
        while let Some(child) = row.first_child() {
            row.remove(&child);
        }
        // The old row's pickers and refreshers go with it. Dropping them here rather than
        // letting them accumulate means a session that switches tools a hundred times
        // holds one row's worth.
        self.pickers.borrow_mut().clear();
        self.options.refreshers.borrow_mut().clear();

        let (tool, settings, subject) = self.row_context();
        self.options.tool.set(tool);
        self.options.over_selection.set(subject);
        let controls = tool.controls();
        log_row(tool, &settings, subject.is_some());
        // `spec/05` §4.11 swaps the whole toolbar for crop mode: the crop tool's three
        // controls are built by `crop.rs` on the page that replaces this one, and this row
        // is not on screen while that tool is lit. So there is nothing to build -- and
        // building it anyway logged three "asked for outside crop mode" warnings on every
        // crop entry, which were the last lines in the journal before a crash they had
        // nothing to do with (D62).
        if tool == octosnap_scene::Tool::Crop {
            return;
        }
        if controls.is_empty() {
            // `spec/05` §2 gives Select an em dash. Saying so beats an empty strip that
            // looks like a widget that failed to build.
            let hint = gtk::Label::new(Some("Click an object to edit it, drag to move"));
            hint.add_css_class("dim-label");
            hint.set_margin_start(4);
            row.append(&hint);
            return;
        }
        for control in controls {
            let widget = match control {
                Control::Color => self.color_button(&settings, false),
                Control::ColorWithAlpha => self.color_button(&settings, true),
                Control::Size => self.size_button(&settings),
                Control::Shadow => self.flag_toggle(
                    Control::Shadow,
                    &settings,
                    "Shadow",
                    |s| s.style.shadow,
                    |s, on| s.style.shadow = on,
                ),
                Control::ArrowStyle => self.choice_menu(
                    Control::ArrowStyle,
                    &settings,
                    "Arrow style",
                    &ArrowStyle::ALL,
                    (ArrowStyle::label, Some(ArrowStyle::icon)),
                    |s| s.arrow,
                    |s, v| s.arrow = v,
                ),
                Control::ArrowHead => self.choice_menu(
                    Control::ArrowHead,
                    &settings,
                    "Arrowhead",
                    &ArrowHead::ALL,
                    (ArrowHead::label, None),
                    |s| s.head,
                    |s, v| s.head = v,
                ),
                Control::TextStyle => self.choice_menu(
                    Control::TextStyle,
                    &settings,
                    "Text style",
                    &TextStyle::ALL,
                    (TextStyle::label, None),
                    |s| s.text,
                    |s, v| s.text = v,
                ),
                Control::FontSize => self.number_menu(
                    Control::FontSize,
                    &settings,
                    "Text size",
                    &FONT_SIZES,
                    |value| format!("{value:.0} pt"),
                    |s| s.font_size,
                    |s, v| s.font_size = v,
                ),
                Control::CornerRadius => self.flag_toggle(
                    Control::CornerRadius,
                    &settings,
                    "Rounded",
                    |s| s.rounded_corners,
                    |s, on| s.rounded_corners = on,
                ),
                Control::Smoothing => self.flag_toggle(
                    Control::Smoothing,
                    &settings,
                    "Smooth",
                    |s| s.smoothing,
                    |s, on| s.smoothing = on,
                ),
                Control::SmartMode => self.smart_mode_toggle(&settings),
                Control::SpotlightShape => self.choice_menu(
                    Control::SpotlightShape,
                    &settings,
                    "Shape",
                    &SpotlightShape::ALL,
                    (SpotlightShape::label, None),
                    |s| s.spotlight,
                    |s, v| s.spotlight = v,
                ),
                Control::SpotlightOpacity => self.slider(
                    Control::SpotlightOpacity,
                    &settings,
                    "Opacity",
                    |s| s.spotlight_opacity,
                    |s, v| s.spotlight_opacity = v,
                ),
                Control::CounterSettings => self.counter_menu(&settings),
                Control::RedactStyle => self.choice_menu(
                    Control::RedactStyle,
                    &settings,
                    "Redaction style",
                    &RedactStyle::ALL,
                    (RedactStyle::label, Some(RedactStyle::icon)),
                    |s| s.redact,
                    |s, v| s.redact = v,
                ),
                Control::RedactIntensity => self.slider(
                    Control::RedactIntensity,
                    &settings,
                    "Intensity",
                    |s| s.redact_intensity,
                    |s, v| s.redact_intensity = v,
                ),
                // `spec/05` §4.11 swaps the *entire* toolbar for crop mode, so these three
                // are built by `crop.rs` from this same table and never by this row --
                // the row is not on screen while the crop tool is active. Reaching here
                // means the stack did not swap, which is worth a line rather than a
                // silently empty control.
                Control::CropAspect | Control::CropSnapping | Control::CropExpandColor => {
                    tracing::warn!("a crop control asked for outside crop mode");
                    let label = gtk::Label::new(Some("Crop"));
                    label.add_css_class("dim-label");
                    label.upcast()
                }
            };
            row.append(&widget);
        }
    }

    /// Shows fresh values in the controls already on the row.
    ///
    /// After every edit that reached the history, because a gesture can change what the
    /// row shows -- a corner drag on a text box sets a font size no preset has.
    pub(super) fn sync_options(&self) {
        let (_, settings, _) = self.row_context();
        for refresh in self.options.refreshers.borrow().iter() {
            refresh(&settings);
        }
    }

    /// Whether the row is showing this selection already, or needs rebuilding for it.
    ///
    /// The selection listener asks before rebuilding: a marquee that lands on the same
    /// object, or a move of the selected one, changes nothing the row shows.
    pub(super) fn options_follow_selection(self: &Rc<Self>) {
        let (tool, _, subject) = self.row_context();
        if tool != self.options.tool.get() || subject != self.options.over_selection.get() {
            self.refresh_options();
        } else {
            self.sync_options();
        }
    }

    fn remember(&self, control: Control, settings: &ToolSettings) {
        let _ = control;
        let tool = self.options.tool.get();
        self.tools.memory.borrow_mut().insert(tool, *settings);
    }

    // --- the settings behind the row ------------------------------------------------

    /// The active tool's settings, defaulted on first use. What the next object is drawn
    /// with.
    pub(super) fn settings(&self) -> ToolSettings {
        self.memory_of(self.tools.active.get())
    }

    pub(super) fn memory_of(&self, tool: Tool) -> ToolSettings {
        *self.tools.memory.borrow_mut().entry(tool).or_default()
    }

    /// One control changed on the row.
    ///
    /// Three things happen, in this order: the row's tool remembers the value for the
    /// next object it draws (`spec/05` §2: "the last colour and size per tool persist");
    /// the selection takes it, as one undo step however many objects it touches; and the
    /// controls are refreshed in place. The tool remembering it *even when the change was
    /// to a selection* is deliberate: a user who recolours an arrow red is telling the
    /// arrow tool what colour they are working in.
    pub(super) fn change(self: &Rc<Self>, control: Control, edit: impl FnOnce(&mut ToolSettings)) {
        let (tool, mut settings, subject) = self.row_context();
        edit(&mut settings);
        self.remember(control, &settings);
        self.apply_to_selection(control, &settings, true);
        self.sync_options();
        log_row(tool, &settings, subject.is_some());
    }

    /// `spec/05` §9's `[` and `]`: the row's intensity a tenth down or up -- a
    /// redaction's strength, or how far a spotlight dims -- by the slider's own path, so
    /// the key and the slider cannot disagree (D137). A row with no slider has nothing
    /// for them to do.
    pub(super) fn step_intensity(self: &Rc<Self>, up: bool) {
        let (tool, settings, _) = self.row_context();
        let delta = if up { INTENSITY_STEP } else { -INTENSITY_STEP };
        let step = |value: f64| ((value + delta) * 100.0).round().clamp(0.0, 100.0) / 100.0;
        let has = |wanted: Control| tool.controls().contains(&wanted);
        if has(Control::RedactIntensity) {
            let value = step(settings.redact_intensity);
            self.change(Control::RedactIntensity, |s| s.redact_intensity = value);
            tracing::debug!(value, "redaction intensity stepped");
        } else if has(Control::SpotlightOpacity) {
            let value = step(settings.spotlight_opacity);
            self.change(Control::SpotlightOpacity, |s| s.spotlight_opacity = value);
            tracing::debug!(value, "spotlight opacity stepped");
        }
    }

    /// [`Self::change`] from a control still being dragged.
    ///
    /// The history is left open, so the drag folds into one step (`History::absorb`) and
    /// the row is **not** refreshed -- refreshing the slider that is being dragged would
    /// fight the drag. [`Self::end_live_change`] closes the step when the pointer lifts.
    fn change_live(self: &Rc<Self>, control: Control, edit: impl FnOnce(&mut ToolSettings)) {
        let (_, mut settings, _) = self.row_context();
        edit(&mut settings);
        self.remember(control, &settings);
        self.apply_to_selection(control, &settings, false);
    }

    fn end_live_change(&self) {
        self.history.borrow_mut().end_gesture();
        self.sync_options();
        let (tool, settings, subject) = self.row_context();
        log_row(tool, &settings, subject.is_some());
    }

    /// Pushes one control's value onto every selected object.
    fn apply_to_selection(&self, control: Control, settings: &ToolSettings, close: bool) {
        let Some(scene) = self.canvas.scene() else { return };
        let changes: Vec<Command> = self
            .canvas
            .selection()
            .iter()
            .filter_map(|id| {
                let was = scene.get(*id)?;
                let mut now = was.clone();
                control.apply(settings, &mut now).then(|| Command::Change {
                    id: *id,
                    before: Box::new(was.clone()),
                    after: Box::new(now),
                })
            })
            .collect();
        if close {
            self.commit_batch(changes);
        } else {
            self.commit_batch_open(changes);
        }
    }

    /// Arms `spec/05` §2's pipette: the next click on the canvas is a sample.
    ///
    /// Reachable from `crop.rs` as well as from this row: §4.11's expand-canvas colour is
    /// a colour like any other, and sampling it off the image's own border is the obvious
    /// thing to want. `begin_gesture` takes the sample before the crop frame sees the
    /// click, so the frame does not move under the pipette.
    pub(super) fn begin_picking(self: &Rc<Self>, picker: &Rc<Picker>) {
        picker.popover().popdown();
        // §2: "A pipette samples from the canvas and, via the extension, from anywhere on
        // screen." The screen first, because the canvas is on it: the shell's picker
        // covers the editor too, so a click on the picture picks the picture. The canvas
        // pipette is the fallback for an extension without `PickColor` -- one that
        // predates it and has not been reloaded by a logout -- and for a shell that
        // refused, which is a capture being in progress.
        let picker = Rc::downgrade(picker);
        let editor = Rc::downgrade(self);
        tracing::debug!("pipette: asking the shell");
        (self.actions.pick_screen_color)(Box::new(move |answer| {
            let Some(editor) = editor.upgrade() else { return };
            let Some(picker) = picker.upgrade() else { return };
            match answer {
                Ok(Some(sampled)) => {
                    editor.take_picked(&picker, sampled);
                    tracing::info!(color = sampled.to_hex(), "pipette sampled screen");
                }
                Ok(None) => tracing::debug!("pipette cancelled"),
                Err(super::actions::PickFailure::Unsupported) => {
                    tracing::info!(
                        "the shell has no PickColor yet; sampling the picture instead \
                         (log out once to load the new extension)"
                    );
                    editor.arm_canvas_pipette(&picker);
                }
                Err(super::actions::PickFailure::Other(reason)) => {
                    tracing::warn!(reason, "the shell could not pick; sampling the picture instead");
                    editor.arm_canvas_pipette(&picker);
                }
            }
        }));
    }

    /// The canvas half of §2's pipette: the next click on the picture is the sample.
    pub(super) fn arm_canvas_pipette(self: &Rc<Self>, picker: &Rc<Picker>) {
        *self.picking.borrow_mut() = Some(Rc::downgrade(picker));
        self.canvas.set_cursor_from_name(Some("crosshair"));
        tracing::debug!("pipette armed");
    }

    /// Hands a sampled colour to the picker, as one closed undo step.
    ///
    /// The alpha of the pixel is not the alpha of the pen: sampling a colour off a
    /// transparent corner of a window capture would otherwise give an invisible pen.
    /// §2's alpha slider is how opacity gets set. The step is closed here because the
    /// popover that normally closes it -- `connect_closed` -> `end_live_change` -- went
    /// down *before* the sample arrived.
    fn take_picked(self: &Rc<Self>, picker: &Rc<Picker>, sampled: Rgba) {
        let keep = picker.color().a;
        picker.set_color(Rgba { a: keep, ..sampled });
        self.end_live_change();
    }

    /// Takes the sample, if one is armed. Answers whether it consumed the click.
    pub(super) fn take_sample(self: &Rc<Self>, at: octosnap_scene::Point) -> bool {
        let armed = self.picking.borrow_mut().take();
        let Some(picker) = armed else { return false };
        self.canvas.set_cursor_from_name(None);
        let Some(picker) = picker.upgrade() else { return true };
        match self.canvas.sample(at) {
            Some(sampled) => {
                self.take_picked(&picker, sampled);
                tracing::debug!(color = sampled.to_hex(), "pipette sampled");
            }
            None => tracing::debug!("pipette found no pixel there"),
        }
        // Consumed either way: a miss should not also draw a rectangle.
        true
    }

    fn register(&self, refresher: Refresher) {
        self.options.refreshers.borrow_mut().push(refresher);
    }

    // --- the controls ---------------------------------------------------------------

    /// `spec/05` §2's colour control: "a menu button showing the current swatch plus a
    /// chevron".
    ///
    /// The popover is [`Picker`], which holds both of §2's stages -- the ten-swatch column
    /// and the expanded HSV picker behind the rainbow wheel.
    fn color_button(self: &Rc<Self>, settings: &ToolSettings, with_alpha: bool) -> gtk::Widget {
        let current = settings.style.color;
        let swatch = Preview::new(Kind::Color(current));
        let button = gtk::MenuButton::builder()
            .tooltip_text(if with_alpha { "Colour and opacity" } else { "Colour" })
            .child(&swatch)
            .build();

        let picker = Picker::new(current, with_alpha, crate::settings::my_colors());
        {
            let editor = Rc::downgrade(self);
            let swatch = swatch.clone();
            let control = if with_alpha { Control::ColorWithAlpha } else { Control::Color };
            picker.connect_changed(move |color| {
                // The button's own swatch is updated here rather than by rebuilding the
                // row: rebuilding it would destroy the popover the user is dragging in.
                swatch.set_kind(Kind::Color(color));
                let Some(editor) = editor.upgrade() else { return };
                // Live: a drag across the HSV square is dozens of changes and one intent.
                editor.change_live(control, |s| s.style.color = color);
            });
        }
        {
            // Weak, because the picker keeps this callback: a strong `Rc` made the picker
            // hold itself, and every rebuilt options row left one behind with its popover.
            let editor = Rc::downgrade(self);
            let weak = Rc::downgrade(&picker);
            picker.connect_pick(move || {
                let (Some(editor), Some(picker)) = (editor.upgrade(), weak.upgrade()) else {
                    return;
                };
                editor.begin_picking(&picker);
            });
        }
        {
            // Closing the popover is the end of the colour gesture: one undo step for
            // however much dragging happened inside it.
            let editor = Rc::downgrade(self);
            picker.popover().connect_closed(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.end_live_change();
                }
            });
        }
        button.set_popover(Some(picker.popover()));
        {
            let swatch = swatch.clone();
            self.register(Box::new(move |s| swatch.set_kind(Kind::Color(s.style.color))));
        }
        // Held by the editor for as long as the row lives; `refresh_options` clears them.
        self.pickers.borrow_mut().push(picker);
        button.upcast()
    }

    /// §2's size control: "a menu button that opens a vertical popover of six diagonal
    /// stroke previews, thinnest at the top, thickest at the bottom".
    fn size_button(self: &Rc<Self>, settings: &ToolSettings) -> gtk::Widget {
        let level = settings.style.size;
        let face = Preview::new(Kind::Stroke(level));
        let button = gtk::MenuButton::builder()
            .tooltip_text("Stroke width (1-6)")
            .child(&face)
            .build();

        let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
        column.set_margin_top(6);
        column.set_margin_bottom(6);
        column.set_margin_start(6);
        column.set_margin_end(6);
        let mut samples = Vec::new();
        for step in 1..=SIZE_LEVELS {
            let sample = Preview::new(Kind::Stroke(step));
            sample.set_active(step == level);
            let row = gtk::Button::builder()
                .child(&sample)
                .has_frame(false)
                .tooltip_text(format!("Size {step}"))
                .build();
            let editor = Rc::downgrade(self);
            row.connect_clicked(move |row| {
                let Some(editor) = editor.upgrade() else { return };
                editor.change(Control::Size, |s| s.style.size = step);
                if let Some(popover) = row.ancestor(gtk::Popover::static_type()) {
                    popover.downcast_ref::<gtk::Popover>().map(gtk::Popover::popdown);
                }
            });
            column.append(&row);
            samples.push(sample);
        }
        button.set_popover(Some(&gtk::Popover::builder().child(&column).build()));
        self.register(Box::new(move |s| {
            face.set_kind(Kind::Stroke(s.style.size));
            for (index, sample) in samples.iter().enumerate() {
                #[allow(clippy::cast_possible_truncation)]
                sample.set_active(index as u8 + 1 == s.style.size);
            }
        }));
        button.upcast()
    }

    /// One of §2's discrete choices, as a menu of labelled rows.
    ///
    /// Generic over the value so the five style menus -- arrow, text, spotlight shape,
    /// redaction, counter numbering -- are one function. §4.2 asks for "the active row
    /// filled in the accent colour", which is what a `GtkListBox` selection already is.
    ///
    /// `names` is each choice's label and, for a choice that is a shape, its picture
    /// (`spec/09` §4b, D134), shown beside the label on the button and in every row.
    #[allow(clippy::too_many_arguments)]
    fn choice_menu<T: Copy + PartialEq + 'static>(
        self: &Rc<Self>,
        control: Control,
        settings: &ToolSettings,
        tooltip: &str,
        items: &'static [T],
        names: Names<T>,
        current: fn(&ToolSettings) -> T,
        set: fn(&mut ToolSettings, T),
    ) -> gtk::Widget {
        let (label, icon) = names;
        let active = current(settings);
        let button = gtk::MenuButton::builder().tooltip_text(tooltip).build();
        let (face, face_icon, face_label) = named(label(active), icon.map(|icon| icon(active)));
        face.set_margin_start(0);
        face.set_margin_end(0);
        face.set_margin_top(0);
        face.set_margin_bottom(0);
        button.set_child(Some(&face));

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        for item in items {
            let row = gtk::ListBoxRow::new();
            let (content, _, _) = named(label(*item), icon.map(|icon| icon(*item)));
            row.set_child(Some(&content));
            list.append(&row);
            if *item == active {
                list.select_row(Some(&row));
            }
        }
        {
            let editor = Rc::downgrade(self);
            let items: Vec<T> = items.to_vec();
            list.connect_row_activated(move |list, row| {
                let Some(editor) = editor.upgrade() else { return };
                let index = usize::try_from(row.index()).unwrap_or(0);
                let Some(item) = items.get(index).copied() else { return };
                editor.change(control, |s| set(s, item));
                if let Some(popover) = list.ancestor(gtk::Popover::static_type()) {
                    popover.downcast_ref::<gtk::Popover>().map(gtk::Popover::popdown);
                }
            });
        }
        button.set_popover(Some(&gtk::Popover::builder().child(&list).build()));
        {
            let list = list.clone();
            self.register(Box::new(move |s| {
                let value = current(s);
                face_label.set_label(label(value));
                if let (Some(image), Some(icon)) = (&face_icon, icon) {
                    image.set_icon_name(Some(icon(value)));
                }
                let row = items
                    .iter()
                    .position(|item| *item == value)
                    .and_then(|index| list.row_at_index(i32::try_from(index).unwrap_or(0)));
                list.select_row(row.as_ref());
            }));
        }
        button.upcast()
    }

    /// §4.5's thirteen font-size presets: a ladder over a continuous value.
    ///
    /// A menu and not a spin button, because §4.5 asks for presets -- and the value stays
    /// a float, because the same paragraph insists the ladder is not the property:
    /// "Dragging a text object's corner handle sets any value in between, which is why the
    /// stored preference is a float such as 37.744." So the button's label shows the value
    /// the object actually has, preset or not, and the list highlights a row only when the
    /// value is on the ladder.
    #[allow(clippy::too_many_arguments)]
    fn number_menu(
        self: &Rc<Self>,
        control: Control,
        settings: &ToolSettings,
        tooltip: &str,
        values: &'static [f64],
        label: fn(f64) -> String,
        current: fn(&ToolSettings) -> f64,
        set: fn(&mut ToolSettings, f64),
    ) -> gtk::Widget {
        let active = current(settings);
        let button = gtk::MenuButton::builder()
            .tooltip_text(tooltip)
            .label(label(active))
            .build();
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        for value in values {
            let row = gtk::ListBoxRow::new();
            let text = gtk::Label::new(Some(&label(*value)));
            text.set_xalign(0.0);
            text.set_margin_start(8);
            text.set_margin_end(8);
            text.set_margin_top(4);
            text.set_margin_bottom(4);
            row.set_child(Some(&text));
            list.append(&row);
            if (*value - active).abs() < f64::EPSILON {
                list.select_row(Some(&row));
            }
        }
        {
            let editor = Rc::downgrade(self);
            let values: Vec<f64> = values.to_vec();
            list.connect_row_activated(move |list, row| {
                let Some(editor) = editor.upgrade() else { return };
                let index = usize::try_from(row.index()).unwrap_or(0);
                let Some(value) = values.get(index).copied() else { return };
                editor.change(control, |s| set(s, value));
                if let Some(popover) = list.ancestor(gtk::Popover::static_type()) {
                    popover.downcast_ref::<gtk::Popover>().map(gtk::Popover::popdown);
                }
            });
        }
        button.set_popover(Some(&gtk::Popover::builder().child(&list).build()));
        {
            let button = button.clone();
            let list = list.clone();
            self.register(Box::new(move |s| {
                let value = current(s);
                button.set_label(&label(value));
                let row = values
                    .iter()
                    .position(|preset| (*preset - value).abs() < f64::EPSILON)
                    .and_then(|index| list.row_at_index(i32::try_from(index).unwrap_or(0)));
                list.select_row(row.as_ref());
            }));
        }
        button.upcast()
    }

    /// §4.8's and §4.10's continuous values -- the only two sliders in the editor.
    ///
    /// A drag is one undo step: every tick goes through [`Self::change_live`], which leaves
    /// the history open so consecutive changes fold, and the button coming up closes it.
    /// The release is watched by a capture-phase controller because the scale's own
    /// gesture claims the sequence and a click gesture beside it would never see the end.
    fn slider(
        self: &Rc<Self>,
        control: Control,
        settings: &ToolSettings,
        tooltip: &str,
        current: fn(&ToolSettings) -> f64,
        set: fn(&mut ToolSettings, f64),
    ) -> gtk::Widget {
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
        scale.set_value(current(settings));
        scale.set_width_request(120);
        scale.set_draw_value(false);
        scale.set_tooltip_text(Some(tooltip));
        {
            let editor = Rc::downgrade(self);
            scale.connect_value_changed(move |scale| {
                let Some(editor) = editor.upgrade() else { return };
                let value = scale.value();
                editor.change_live(control, |s| set(s, value));
            });
        }
        {
            let editor = Rc::downgrade(self);
            let release = gtk::EventControllerLegacy::new();
            release.set_propagation_phase(gtk::PropagationPhase::Capture);
            release.connect_event(move |_, event| {
                if event.event_type() == gtk::gdk::EventType::ButtonRelease
                    && let Some(editor) = editor.upgrade()
                {
                    editor.end_live_change();
                }
                glib::Propagation::Proceed
            });
            scale.add_controller(release);
        }
        {
            let scale = scale.clone();
            self.register(Box::new(move |s| {
                let value = current(s);
                if (scale.value() - value).abs() > 1e-6 {
                    scale.set_value(value);
                }
            }));
        }
        let label = gtk::Label::new(Some(tooltip));
        label.add_css_class("dim-label");
        let wrap = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        wrap.append(&label);
        wrap.append(&scale);
        wrap.upcast()
    }

    /// A plain on/off from §2's rows: shadow, corner radius, smoothing.
    fn flag_toggle(
        self: &Rc<Self>,
        control: Control,
        settings: &ToolSettings,
        label: &str,
        current: fn(&ToolSettings) -> bool,
        set: fn(&mut ToolSettings, bool),
    ) -> gtk::Widget {
        let toggle = gtk::ToggleButton::builder()
            .label(label)
            .active(current(settings))
            .tooltip_text(label)
            .build();
        {
            let editor = Rc::downgrade(self);
            toggle.connect_toggled(move |toggle| {
                let Some(editor) = editor.upgrade() else { return };
                let on = toggle.is_active();
                if current(&editor.row_context().1) == on {
                    // The refresher sets this button's state, which emits `toggled` again.
                    // Without this the second emission recurses.
                    return;
                }
                editor.change(control, |s| set(s, on));
            });
        }
        {
            let toggle = toggle.clone();
            self.register(Box::new(move |s| toggle.set_active(current(s))));
        }
        toggle.upcast()
    }

    /// §4.7's "smart mode on/off", present and honest about not working yet.
    /// `spec/05` §2's "smart mode on/off" on the highlighter's row (§4.7).
    ///
    /// A `flag_toggle` like the others, with one difference that is not a widget: smart
    /// mode is a property of the *gesture* and not of the stroke, so `Control::apply`
    /// deliberately writes nothing to a selected object. Turning it on with a highlighter
    /// selected tells the tool what to do next, which is what the tooltip says.
    fn smart_mode_toggle(self: &Rc<Self>, settings: &ToolSettings) -> gtk::Widget {
        let toggle = self.flag_toggle(
            Control::SmartMode,
            settings,
            "Smart",
            |s| s.smart_highlighter,
            |s, on| s.smart_highlighter = on,
        );
        toggle.set_tooltip_text(Some(
            "Snap the stroke to the line of text under it (hold Ctrl while drawing to skip it)",
        ));
        toggle
    }

    /// §4.9's settings menu: "four numbering systems … Below a separator sit **Starting
    /// number** as a stepper, and **Size** as a submenu of six values".
    fn counter_menu(self: &Rc<Self>, settings: &ToolSettings) -> gtk::Widget {
        let button = gtk::MenuButton::builder()
            .tooltip_text("Counter settings")
            .icon_name("emblem-system-symbolic")
            .build();
        let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
        column.set_margin_top(6);
        column.set_margin_bottom(6);
        column.set_margin_start(6);
        column.set_margin_end(6);

        let active = settings.counter;
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        for style in CounterStyle::ALL {
            let row = gtk::ListBoxRow::new();
            let text = gtk::Label::new(Some(style.label()));
            text.set_xalign(0.0);
            text.set_margin_start(8);
            text.set_margin_end(8);
            text.set_margin_top(4);
            text.set_margin_bottom(4);
            row.set_child(Some(&text));
            list.append(&row);
            if style == active {
                list.select_row(Some(&row));
            }
        }
        {
            let editor = Rc::downgrade(self);
            list.connect_row_activated(move |_, row| {
                let Some(editor) = editor.upgrade() else { return };
                let index = usize::try_from(row.index()).unwrap_or(0);
                let Some(style) = CounterStyle::ALL.get(index).copied() else { return };
                editor.change(Control::CounterSettings, |s| s.counter = style);
            });
        }
        column.append(&list);
        column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // The starting number is the *document's*, not the tool's: §4.9 calls the
        // numbering "a property of the document rather than of each badge", and
        // `Scene::next_counter_number` counts from it.
        let start = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        start.append(&gtk::Label::new(Some("Starting number")));
        let spin = gtk::SpinButton::with_range(0.0, 9999.0, 1.0);
        spin.set_value(self.starting_number());
        {
            let editor = Rc::downgrade(self);
            spin.connect_value_changed(move |spin| {
                let Some(editor) = editor.upgrade() else { return };
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                editor.set_starting_number(spin.value() as u32);
            });
        }
        start.append(&spin);
        column.append(&start);

        let sizes = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        sizes.append(&gtk::Label::new(Some("Size")));
        let mut first: Option<gtk::ToggleButton> = None;
        let mut toggles = Vec::new();
        for radius in COUNTER_SIZES {
            let toggle = gtk::ToggleButton::builder().label(format!("{radius:.0}")).build();
            match &first {
                Some(anchor) => toggle.set_group(Some(anchor)),
                None => first = Some(toggle.clone()),
            }
            toggle.set_active((settings.counter_radius - radius).abs() < f64::EPSILON);
            let editor = Rc::downgrade(self);
            toggle.connect_toggled(move |toggle| {
                if !toggle.is_active() {
                    return;
                }
                let Some(editor) = editor.upgrade() else { return };
                if (editor.row_context().1.counter_radius - radius).abs() < f64::EPSILON {
                    return;
                }
                editor.change(Control::CounterSettings, |s| s.counter_radius = radius);
            });
            sizes.append(&toggle);
            toggles.push((radius, toggle));
        }
        column.append(&sizes);
        {
            let list = list.clone();
            self.register(Box::new(move |s| {
                let row = CounterStyle::ALL
                    .iter()
                    .position(|style| *style == s.counter)
                    .and_then(|index| list.row_at_index(i32::try_from(index).unwrap_or(0)));
                list.select_row(row.as_ref());
                for (radius, toggle) in &toggles {
                    if (s.counter_radius - radius).abs() < f64::EPSILON && !toggle.is_active() {
                        toggle.set_active(true);
                    }
                }
            }));
        }

        button.set_popover(Some(&gtk::Popover::builder().child(&column).build()));
        button.upcast()
    }
}

/// A choice as a menu shows it: its picture, when it has one, then its label. Returns the
/// box, and the image and label so the button's can follow the choice.
fn named(text: &str, icon: Option<&str>) -> (gtk::Box, Option<gtk::Image>, gtk::Label) {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    content.set_margin_start(8);
    content.set_margin_end(8);
    content.set_margin_top(4);
    content.set_margin_bottom(4);
    let image = icon.map(|name| {
        let image = gtk::Image::from_icon_name(name);
        content.append(&image);
        image
    });
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    content.append(&label);
    (content, image, label)
}
