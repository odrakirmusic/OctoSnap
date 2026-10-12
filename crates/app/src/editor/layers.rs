// SPDX-License-Identifier: GPL-3.0-or-later

//! `ANN-30` (D186): the layers panel -- every mark in a list, to select, reorder, hide and
//! lock.
//!
//! A sidebar on the right, under the title bar, behind F9 and the bottom bar's Layers
//! button. The list and what its edits are come from `octosnap_scene::layers`, tested
//! without a display; this file is the rows and the wiring. A row is a mark: its tool's
//! icon, what it says or what it is, and two toggles, the eye and the lock. Selecting rows
//! selects the marks, and the canvas's selection selects rows; dragging a row onto another,
//! or Alt+Up and Alt+Down, moves its mark among its group. Every edit is one undo step, and
//! the list is built again from the document after every edit, an undo's too, so it can
//! never say something the canvas does not.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::{gdk, glib};
use octosnap_scene::tool::Tool;
use octosnap_scene::{Command, Geometry, Object, ObjectId, layers};
use tracing::info;

use super::window::Editor;

/// The panel's width: a row's icon, a label of a few words and the two toggles.
pub(super) const WIDTH: i32 = 280;

/// Everything the panel has to find again.
#[derive(Default)]
pub struct LayersPanel {
    pub split: RefCell<Option<adw::OverlaySplitView>>,
    pub toggle: RefCell<Option<gtk::ToggleButton>>,
    list: RefCell<Option<gtk::ListBox>>,
    /// The marks the rows stand for, in the rows' order.
    rows: RefCell<Vec<ObjectId>>,
    /// True while the rows are written from the document or the canvas, so their own
    /// handlers do not write back.
    syncing: Cell<bool>,
    /// The editor, for the rows' handlers: the list is built from `after_edit`, which has
    /// no `Rc` to hand out.
    editor: RefCell<Weak<Editor>>,
    /// True while the rows' places are waiting to be logged (`log_rows`).
    logging: Rc<Cell<bool>>,
}

impl std::fmt::Debug for LayersPanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayersPanel").field("rows", &self.rows.borrow().len()).finish()
    }
}

impl Editor {
    /// The sidebar, built once with the window.
    pub(super) fn build_layers_sidebar(self: &Rc<Self>) -> gtk::Widget {
        *self.layers.editor.borrow_mut() = Rc::downgrade(self);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Multiple);
        // A click selects its row alone, and Ctrl and Shift add, as in Files. A list that
        // activates on one click adds every clicked row to a multiple selection instead.
        list.set_activate_on_single_click(false);
        list.add_css_class("navigation-sidebar");
        let empty = gtk::Label::new(Some("No marks yet"));
        empty.add_css_class("dim-label");
        empty.set_margin_top(24);
        list.set_placeholder(Some(&empty));
        {
            let editor = Rc::downgrade(self);
            list.connect_selected_rows_changed(move |list| {
                let Some(editor) = editor.upgrade() else { return };
                if editor.layers.syncing.get() {
                    return;
                }
                let ids = editor.layers_selected(list);
                info!(count = ids.len(), "layers selected");
                editor.canvas.set_selection(ids);
            });
        }
        // Alt+Up and Alt+Down move the selected mark a place, as a drag does.
        let keys = gtk::EventControllerKey::new();
        {
            let editor = Rc::downgrade(self);
            keys.connect_key_pressed(move |_, key, _, state| {
                let Some(editor) = editor.upgrade() else { return glib::Propagation::Proceed };
                if !state.contains(gdk::ModifierType::ALT_MASK) {
                    return glib::Propagation::Proceed;
                }
                let up = match key {
                    gdk::Key::Up => true,
                    gdk::Key::Down => false,
                    _ => return glib::Propagation::Proceed,
                };
                editor.step_layer(up);
                glib::Propagation::Stop
            });
        }
        list.add_controller(keys);

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&list));
        let title = gtk::Label::new(Some("Layers"));
        title.add_css_class("heading");
        title.set_xalign(0.0);
        title.set_margin_start(12);
        title.set_margin_top(10);
        title.set_margin_bottom(4);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&title);
        column.append(&scroller);
        *self.layers.list.borrow_mut() = Some(list);
        column.upcast()
    }

    /// The Layers button for the bottom bar, which F9 presses too.
    pub(super) fn build_layers_toggle(self: &Rc<Self>) -> gtk::ToggleButton {
        let toggle = gtk::ToggleButton::new();
        toggle.set_icon_name("layers-symbolic");
        toggle.set_tooltip_text(Some("Layers (F9)"));
        toggle.update_property(&[gtk::accessible::Property::Label("Layers")]);
        {
            let editor = Rc::downgrade(self);
            toggle.connect_toggled(move |toggle| {
                // F9 sets the button to match the panel, and that is no second showing.
                if let Some(editor) = editor.upgrade()
                    && editor.layers_panel_open() != toggle.is_active()
                {
                    editor.show_layers_panel(toggle.is_active());
                }
            });
        }
        *self.layers.toggle.borrow_mut() = Some(toggle.clone());
        toggle
    }

    pub(super) fn layers_panel_open(&self) -> bool {
        self.layers.split.borrow().as_ref().is_some_and(adw::OverlaySplitView::shows_sidebar)
    }

    pub(super) fn show_layers_panel(&self, show: bool) {
        if let Some(split) = self.layers.split.borrow().as_ref() {
            // Focus in the sidebar as it goes back to the canvas, as the background
            // panel's does (D166).
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
        self.fit_window();
        if let Some(toggle) = self.layers.toggle.borrow().as_ref()
            && toggle.is_active() != show
        {
            toggle.set_active(show);
        }
        info!(shown = show, "layers panel");
        if show {
            self.refresh_layers();
        }
    }

    /// The rows, built again from the document. Only while the panel shows: it is built
    /// when it is shown.
    pub(super) fn refresh_layers(&self) {
        if !self.layers_panel_open() {
            return;
        }
        let Some(list) = self.layers.list.borrow().clone() else { return };
        let Some(scene) = self.canvas.scene() else { return };
        let editor = self.layers.editor.borrow().clone();
        self.layers.syncing.set(true);
        list.remove_all();
        let rows = layers::rows(&scene);
        let selection = self.canvas.selection();
        for (index, id) in rows.iter().enumerate() {
            let Some(object) = scene.get(*id) else { continue };
            let row = layer_row(&editor, object, index);
            list.append(&row);
            if selection.contains(id) && row.is_selectable() {
                list.select_row(Some(&row));
            }
        }
        let hidden = rows.iter().filter(|id| scene.get(**id).is_some_and(|o| o.hidden)).count();
        let locked = rows.iter().filter(|id| scene.get(**id).is_some_and(|o| o.locked)).count();
        *self.layers.rows.borrow_mut() = rows;
        self.layers.syncing.set(false);
        info!(rows = self.layers.rows.borrow().len(), hidden, locked, "layers listed");
        self.log_rows(&list);
    }

    /// The canvas's selection, shown in the rows.
    pub(super) fn layers_follow_selection(&self) {
        if !self.layers_panel_open() || self.layers.syncing.get() {
            return;
        }
        let Some(list) = self.layers.list.borrow().clone() else { return };
        let selection = self.canvas.selection();
        let rows = self.layers.rows.borrow().clone();
        self.layers.syncing.set(true);
        for (index, id) in rows.iter().enumerate() {
            let Some(row) = i32::try_from(index).ok().and_then(|i| list.row_at_index(i)) else {
                continue;
            };
            if selection.contains(id) && row.is_selectable() {
                list.select_row(Some(&row));
            } else {
                list.unselect_row(&row);
            }
        }
        self.layers.syncing.set(false);
    }

    /// The marks of the selected rows.
    fn layers_selected(&self, list: &gtk::ListBox) -> Vec<ObjectId> {
        let rows = self.layers.rows.borrow();
        list.selected_rows()
            .iter()
            .filter_map(|row| usize::try_from(row.index()).ok().and_then(|i| rows.get(i)))
            .copied()
            .collect()
    }

    /// The mark of row `from` put where row `to` is, among its group (a drop).
    fn move_layer(&self, from: usize, to: usize) {
        let Some(id) = self.layers.rows.borrow().get(from).copied() else { return };
        let Some(scene) = self.canvas.scene() else { return };
        let commands = layers::move_to(&scene, id, to);
        if commands.is_empty() {
            return;
        }
        info!(from, to, moves = commands.len(), "layer moved");
        self.commit_batch(commands);
    }

    /// The selected mark a place up or down the list (Alt+Up, Alt+Down).
    fn step_layer(&self, up: bool) {
        let Some(list) = self.layers.list.borrow().clone() else { return };
        let ids = self.layers_selected(&list);
        let [id] = ids.as_slice() else { return };
        let Some(scene) = self.canvas.scene() else { return };
        let commands = layers::step(&scene, *id, up);
        if commands.is_empty() {
            return;
        }
        info!(up, moves = commands.len(), "layer moved");
        self.commit_batch(commands);
        // The row follows its mark, so the next Alt+Down moves it again.
        self.canvas.set_selection(vec![*id]);
    }

    /// The eye: `ids` hidden or shown, as one step. A hidden mark leaves the selection,
    /// which would otherwise be handles around nothing.
    fn hide_layers(&self, ids: &[ObjectId], hidden: bool) {
        let Some(scene) = self.canvas.scene() else { return };
        let changes: Vec<Command> = ids
            .iter()
            .filter_map(|id| {
                let was = scene.get(*id)?;
                if was.hidden == hidden || !layers::hideable(was) {
                    return None;
                }
                let mut now = was.clone();
                now.hidden = hidden;
                Some(Command::Change {
                    id: *id,
                    before: Box::new(was.clone()),
                    after: Box::new(now),
                })
            })
            .collect();
        if changes.is_empty() {
            return;
        }
        info!(count = changes.len(), hidden, "layers hidden");
        if hidden {
            let selection: Vec<ObjectId> =
                self.canvas.selection().into_iter().filter(|id| !ids.contains(id)).collect();
            self.canvas.set_selection(selection);
        }
        self.commit_batch(changes);
    }

    /// The lock, which the canvas menu's Lock and Unlock set too (D162).
    fn lock_layers(&self, ids: &[ObjectId], locked: bool) {
        let count = self.set_locked(ids, locked);
        if count > 0 && locked {
            let selection: Vec<ObjectId> =
                self.canvas.selection().into_iter().filter(|id| !ids.contains(id)).collect();
            self.canvas.set_selection(selection);
        }
        info!(count, locked, "layers locked");
    }

    /// Where each row, its eye and its lock are in the window once the panel has settled,
    /// for `editor-test.sh`, which clicks and drags them.
    fn log_rows(&self, list: &gtk::ListBox) {
        if self.layers.logging.replace(true) {
            return;
        }
        let last = Cell::new(None::<(i32, i32)>);
        let still = Cell::new(0_u32);
        let logging = self.layers.logging.clone();
        let ids = self.layers.rows.borrow().clone();
        list.add_tick_callback(move |list, _| {
            let root = list.root();
            let bounds = |widget: &gtk::Widget| {
                root.as_ref().and_then(|root| widget.compute_bounds(root)).map(|b| {
                    #[allow(clippy::cast_possible_truncation)]
                    let rect = (b.x() as i32, b.y() as i32, b.width() as i32, b.height() as i32);
                    rect
                })
            };
            let first = list.row_at_index(0).and_then(|row| bounds(row.upcast_ref()));
            let at = first.map(|r| (r.1, r.2));
            if (first.is_some() && at.is_none_or(|a| a.1 <= 0)) || last.replace(at) != at {
                still.set(0);
                return glib::ControlFlow::Continue;
            }
            still.set(still.get() + 1);
            if still.get() < 10 {
                return glib::ControlFlow::Continue;
            }
            let mut index = 0;
            while let Some(row) = list.row_at_index(index) {
                let Some((x, y, width, height)) = bounds(row.upcast_ref()) else { break };
                let toggles = toggles_of(&row);
                let eye = toggles.0.and_then(|w| bounds(&w));
                let lock = toggles.1.and_then(|w| bounds(&w));
                let centre = |r: Option<(i32, i32, i32, i32)>| {
                    r.map_or_else(
                        || "none".to_owned(),
                        |r| format!("{},{}", r.0 + r.2 / 2, r.1 + r.3 / 2),
                    )
                };
                let id = usize::try_from(index)
                    .ok()
                    .and_then(|i| ids.get(i))
                    .map_or_else(String::new, ToString::to_string);
                tracing::debug!(
                    index,
                    id,
                    x,
                    y,
                    width,
                    height,
                    eye = centre(eye),
                    lock = centre(lock),
                    "layers row"
                );
                index += 1;
            }
            logging.set(false);
            glib::ControlFlow::Break
        });
    }
}

/// A mark's row: its icon, its label, the eye and the lock; a drag source, and a drop
/// target for the others.
fn layer_row(editor: &Weak<Editor>, object: &Object, index: usize) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    line.set_margin_start(6);
    line.set_margin_end(2);
    let icon = gtk::Image::from_icon_name(icon_of(object));
    let name = gtk::Label::new(Some(&layers::label(object)));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    if object.hidden && layers::hideable(object) {
        icon.add_css_class("dim-label");
        name.add_css_class("dim-label");
    }
    line.append(&icon);
    line.append(&name);
    let id = object.id;

    // Buttons, not toggles: a toggle shows its "on" as a pressed background, and every
    // row's eye would be pressed. The icon is the state, and a click asks for the other.
    let eye = gtk::Button::from_icon_name(if object.hidden {
        "view-conceal-symbolic"
    } else {
        "view-reveal-symbolic"
    });
    eye.add_css_class("flat");
    eye.set_valign(gtk::Align::Center);
    eye.set_widget_name("layer-eye");
    // The click rebuilds the list and destroys the button. Focused, it would hand the focus
    // on as it went, and the list selected a row nobody clicked.
    eye.set_focus_on_click(false);
    if layers::hideable(object) {
        let (hidden, action) = (object.hidden, if object.hidden { "Show" } else { "Hide" });
        eye.set_tooltip_text(Some(action));
        eye.update_property(&[gtk::accessible::Property::Label(action)]);
        let editor = editor.clone();
        eye.connect_clicked(move |_| {
            if let Some(editor) = editor.upgrade() {
                editor.hide_layers(&[id], !hidden);
            }
        });
    } else {
        // A redaction is always drawn (`layers::hideable`): the eye is there, open, and
        // says why it cannot close.
        eye.set_sensitive(false);
        eye.set_tooltip_text(Some("A redaction is always in the picture"));
        eye.update_property(&[gtk::accessible::Property::Label("Always shown")]);
    }
    line.append(&eye);

    let locked = object.locked;
    let lock = gtk::Button::from_icon_name(if locked {
        "changes-prevent-symbolic"
    } else {
        "changes-allow-symbolic"
    });
    lock.add_css_class("flat");
    lock.set_valign(gtk::Align::Center);
    lock.set_widget_name("layer-lock");
    lock.set_focus_on_click(false);
    // An open padlock on every row is noise; a closed one is what the eye looks for.
    if !locked {
        lock.add_css_class("dim-label");
    }
    let action = if locked { "Unlock" } else { "Lock" };
    lock.set_tooltip_text(Some(action));
    lock.update_property(&[gtk::accessible::Property::Label(action)]);
    lock.set_sensitive(object.grips() != octosnap_scene::Grips::None);
    {
        let editor = editor.clone();
        lock.connect_clicked(move |_| {
            if let Some(editor) = editor.upgrade() {
                editor.lock_layers(&[id], !locked);
            }
        });
    }
    line.append(&lock);
    row.set_child(Some(&line));
    // A locked mark is never the selection (D162): its row is not either.
    row.set_selectable(!object.locked);

    // A row is dragged by its index, and dropped on another's.
    let source = gtk::DragSource::new();
    source.set_actions(gdk::DragAction::MOVE);
    #[allow(clippy::cast_possible_truncation)]
    let from = index as u32;
    source.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&from.to_value())));
    {
        let row = row.clone();
        source.connect_drag_begin(move |source, _| {
            let paintable = gtk::WidgetPaintable::new(Some(&row));
            source.set_icon(Some(&paintable), 0, 0);
        });
    }
    row.add_controller(source);
    let target = gtk::DropTarget::new(u32::static_type(), gdk::DragAction::MOVE);
    {
        let editor = editor.clone();
        target.connect_drop(move |_, value, _, _| {
            let (Some(editor), Ok(from)) = (editor.upgrade(), value.get::<u32>()) else {
                return false;
            };
            editor.move_layer(from as usize, index);
            true
        });
    }
    row.add_controller(target);
    row
}

/// A row's eye and lock, found by name.
fn toggles_of(row: &gtk::ListBoxRow) -> (Option<gtk::Widget>, Option<gtk::Widget>) {
    let mut eye = None;
    let mut lock = None;
    let mut child = row.child().and_then(|line| line.first_child());
    while let Some(widget) = child {
        match widget.widget_name().as_str() {
            "layer-eye" => eye = Some(widget.clone()),
            "layer-lock" => lock = Some(widget.clone()),
            _ => {}
        }
        child = widget.next_sibling();
    }
    (eye, lock)
}

/// The icon of the tool that makes the mark; a picture's is the theme's image.
fn icon_of(object: &Object) -> &'static str {
    match Tool::for_object(object) {
        Some(tool) => tool.icon(),
        None if matches!(object.geometry, Geometry::Image { .. }) => "image-x-generic-symbolic",
        None => "image-missing-symbolic",
    }
}
