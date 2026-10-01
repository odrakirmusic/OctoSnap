// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/13` #8's canvas context menu [P]: a right-click opens, at the pointer, what can be
//! done to what is under it. The frequent actions on an object otherwise sit across the
//! window, in the title bar and the bottom bar -- up to the height of a maximised window
//! away from the object they act on.
//!
//! Each entry shows its shortcut, so the menu also teaches the keys (`spec/13` #3). The
//! object's colour and size are not repeated here: the options row already follows the
//! selection (D56).

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use octosnap_scene::{ObjectKind, Point};
use tracing::info;

use super::window::Editor;

/// One entry: its label, its action in the `canvas` group, and the shortcut shown beside it.
type Entry = (&'static str, &'static str, Option<&'static str>);

impl Editor {
    /// The secondary button, on its own controller as the double-click is: the drag takes
    /// only the primary one. Shift+F10 and the Menu key open the same menu at the
    /// selection, for a keyboard.
    pub(super) fn wire_context_menu(self: &Rc<Self>) {
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let editor = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(editor) = editor.upgrade() else { return };
            gesture.set_state(gtk::EventSequenceState::Claimed);
            editor.open_context_menu(x, y, true);
        });
        self.canvas.add_controller(click);
    }

    /// The menu from the keyboard: at the middle of the first selected object, or of the
    /// canvas when nothing is selected.
    pub(super) fn open_context_menu_at_selection(self: &Rc<Self>) {
        let first = self.canvas.selection().first().copied();
        let centre = first.and_then(|id| self.canvas.scene()?.get(id).map(|o| o.bounds()));
        let (x, y) = match centre {
            Some(b) => self.canvas.to_widget(Point::new(b.x + b.width / 2.0, b.y + b.height / 2.0)),
            None => (f64::from(self.canvas.width()) / 2.0, f64::from(self.canvas.height()) / 2.0),
        };
        self.open_context_menu(x, y, false);
    }

    /// `x` and `y` in the canvas's widget pixels. With `pick`, what is under the pointer
    /// becomes the selection first, unless it is already part of it: a right-click on one
    /// of three selected objects acts on all three, as it does in a file manager.
    fn open_context_menu(self: &Rc<Self>, x: f64, y: f64, pick: bool) {
        if self.cropping() || self.typing() {
            return;
        }
        let Some(scene) = self.canvas.scene() else { return };
        let at = self.canvas.to_document(x, y);
        if pick {
            match self.object_at(&scene, at) {
                Some(id) if !self.canvas.selection().contains(&id) => self.canvas.set_selection(vec![id]),
                Some(_) => {}
                None => self.canvas.set_selection(Vec::new()),
            }
        }
        let selection = self.canvas.selection();

        let sections: Vec<Vec<Entry>> = if selection.is_empty() {
            vec![
                vec![("Paste", "paste", Some("<Control>v")), ("Select All", "select-all", Some("<Control>a"))],
                vec![("Copy Image", "copy", Some("<Control>c"))],
            ]
        } else {
            // One text object or one counter: its words or its number, in place.
            let one = match selection.as_slice() {
                [id] => scene.get(*id).map(octosnap_scene::Object::kind),
                _ => None,
            };
            let mut sections = Vec::new();
            match one {
                Some(ObjectKind::Text) => sections.push(vec![("Edit Text", "edit", None)]),
                Some(ObjectKind::Counter) => sections.push(vec![("Edit Number", "edit", None)]),
                _ => {}
            }
            sections.push(vec![
                ("Cut", "cut", Some("<Control>x")),
                ("Copy", "copy", Some("<Control>c")),
                ("Duplicate", "duplicate", Some("<Control>d")),
            ]);
            sections.push(vec![
                // To the very front and back, which is what the keys do: `reorder_selection`
                // takes the selection past every other object, not one step (D136).
                ("Bring to Front", "forward", Some("<Control>bracketright")),
                ("Send to Back", "backward", Some("<Control>bracketleft")),
            ]);
            sections.push(vec![("Delete", "delete", Some("Delete"))]);
            sections
        };

        let menu = gio::Menu::new();
        for section in &sections {
            let part = gio::Menu::new();
            for (label, action, accel) in section {
                let item = gio::MenuItem::new(Some(label), Some(&format!("canvas.{action}")));
                if let Some(accel) = accel {
                    item.set_attribute_value("accel", Some(&accel.to_variant()));
                }
                part.append_item(&item);
            }
            menu.append_section(None, &part);
        }

        type Run = Box<dyn Fn(&Rc<Editor>)>;
        let runs: [(&str, Run); 9] = [
            ("paste", Box::new(Editor::paste_shortcut)),
            ("select-all", Box::new(Editor::select_all)),
            ("copy", Box::new(Editor::copy_shortcut)),
            ("cut", Box::new(Editor::cut_shortcut)),
            ("duplicate", Box::new(Editor::duplicate_selection)),
            ("forward", Box::new(|e: &Rc<Editor>| e.reorder_selection(true))),
            ("backward", Box::new(|e: &Rc<Editor>| e.reorder_selection(false))),
            ("delete", Box::new(Editor::delete_selection)),
            (
                "edit",
                Box::new(move |e: &Rc<Editor>| {
                    let _ = e.edit_text_under(at) || e.edit_counter_under(at);
                }),
            ),
        ];
        let group = gio::SimpleActionGroup::new();
        for (name, run) in runs {
            let action = gio::SimpleAction::new(name, None);
            let editor = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(editor) = editor.upgrade() {
                    info!(action = name, "canvas context menu");
                    run(&editor);
                }
            });
            group.add_action(&action);
        }

        // Parented to the canvas's parent, an `Overlay`, as the counter's popover is: the
        // canvas sizes its own children and would have to present each popover by hand.
        let host: gtk::Widget = self.canvas.parent().unwrap_or_else(|| self.canvas.clone().upcast());
        host.insert_action_group("canvas", Some(&group));
        let (hx, hy) = self
            .canvas
            .compute_point(&host, &gtk::graphene::Point::new(x as f32, y as f32))
            .map_or((x, y), |p| (f64::from(p.x()), f64::from(p.y())));
        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(&host);
        popover.set_has_arrow(false);
        #[allow(clippy::cast_possible_truncation)]
        popover.set_pointing_to(Some(&gdk::Rectangle::new(hx as i32, hy as i32, 1, 1)));
        popover.connect_closed(|popover| {
            // On the next turn, so the entry it closed for has run whatever order GTK does
            // the two in: the entry finds its action through this parent.
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
        popover.popup();
        info!(selected = selection.len(), "canvas context menu opened");
    }
}
