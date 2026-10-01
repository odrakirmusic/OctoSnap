// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §2's tool strip and the gestures that drive it.
//!
//! The division of labour with `octosnap_scene::tool` is the point of this file: every
//! *rule* is over there and testable -- Shift snapping to 45°, Alt drawing from the
//! centre, the eight-unit minimum, the resampling -- and everything here is plumbing
//! between a `GtkGesture` and a [`Draft`]. If a behaviour in §4 turns out to be wrong, the
//! fix belongs in the other crate, where it can be proved without a display.
//!
//! Two rules do live here. The first is about the history rather than the geometry:
//! **nothing reaches the undo stack until the button comes up.** A drag previews by
//! handing the canvas an object that is not in the scene, or by mutating a copy of the
//! scene; the `Command` is built once, on release. `History::absorb` would have folded
//! per-frame pushes for a single object, but a marquee-dragged group emits one change per
//! object and those never coalesce -- which is why [`Command::Batch`] exists and why the
//! commit happens once rather than sixty times a second.
//!
//! The second is about what a press means (D56): **an object under the pointer is
//! selectable with any tool that is not freehand.** Pressing on an existing arrow with the
//! rectangle tool selects and moves the arrow; pressing on empty picture draws the
//! rectangle. The first version let only the Select tool touch existing objects, and the
//! report from hardware was "they are not editable after clicking" -- a user who has just
//! drawn a box and clicks it expects it to answer, whichever button is lit. The pencil and
//! the highlighter are the exception because a freehand stroke crosses other objects all
//! the time and must start wherever the pen comes down.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};
use octosnap_scene::tool::{Draft, Modifiers, Tool, ToolSettings};
use octosnap_scene::{Bounds, Command, Grips, Handle, Object, ObjectId, Point, Scene, handle, hit};
use tracing::debug;

use super::cursor::Hover;
use super::window::Editor;

/// How far a press may travel and still be a click, in **widget** pixels -- GTK's own
/// `gtk-dnd-drag-threshold`, the distance every drag on this desktop already has to cross
/// before it is one.
const CLICK_SLOP: f64 = 8.0;

/// `spec/05` §4.1: "Ctrl+D duplicates offset by 12 units".
const DUPLICATE_OFFSET: f64 = 12.0;

thread_local! {
    /// `spec/05` §4.1's internal object clipboard. One per process, not one per editor, so
    /// that objects copied in one capture paste into another. Every editor lives on the main
    /// thread, which is the only thread this is ever touched from.
    static OBJECTS: RefCell<octosnap_scene::clipboard::Clipboard> =
        const { RefCell::new(octosnap_scene::clipboard::Clipboard::new()) };
    /// The desktop clipboard's formats when it last changed. The compositor offers the
    /// same selection again each time one of the app's windows gets the keyboard back from
    /// another application, and that is not a copy.
    static DESKTOP_FORMATS: RefCell<Option<String>> = const { RefCell::new(None) };
    static WATCHING_DESKTOP: Cell<bool> = const { Cell::new(false) };
}

/// Empties `spec/05` §4.1's object clipboard, because something newer is on the desktop's
/// and Ctrl+V means that now, as it would in any other application (`spec/13` #3). The app
/// calls this whenever it copies an image or text itself; [`watch_desktop_clipboard`] calls
/// it when another application copies.
pub fn forget_copied_objects(why: &str) {
    let had = OBJECTS.with_borrow_mut(|clipboard| {
        let had = !clipboard.is_empty();
        *clipboard = octosnap_scene::clipboard::Clipboard::new();
        had
    });
    if had {
        tracing::info!(why, "the copied objects gave way to the desktop clipboard");
    }
}

/// Watches the desktop clipboard for a copy made in another application, once per process.
///
/// Formats the clipboard already had are the same selection offered again, and the objects
/// stay: a user who looks at another window and comes back still has them. A copy made
/// elsewhere with exactly the formats of the one before cannot be told apart, and keeps
/// them too; the app's own copies say so for themselves.
pub(super) fn watch_desktop_clipboard(clipboard: &gdk::Clipboard) {
    if WATCHING_DESKTOP.replace(true) {
        return;
    }
    let seen = clipboard.formats().to_str().to_string();
    DESKTOP_FORMATS.with_borrow_mut(|formats| *formats = Some(seen));
    clipboard.connect_changed(|clipboard| {
        let now = clipboard.formats().to_str().to_string();
        let same = DESKTOP_FORMATS
            .with_borrow_mut(|formats| formats.replace(now.clone()).is_some_and(|was| was == now));
        if !same && !clipboard.is_local() {
            forget_copied_objects("another application copied");
        }
    });
}

/// How close the pointer has to come to one of `spec/05` §4.1's handles, in **widget**
/// pixels.
///
/// A shade larger than `spec/09`'s eight-pixel handle, because the target should be at
/// least as big as the thing you can see. Divided by the zoom before use.
const HANDLE_GRAB: f64 = 10.0;

/// How far from a stroke a press may land and still be on it, in **widget** pixels.
///
/// `scene::hit::TOLERANCE` is the same idea in image pixels, and it is the wrong unit for
/// a pointer: at a 40 % fit four image pixels are not even two on screen, and a hairline
/// became unclickable exactly when the document was zoomed out enough to see it whole.
/// Divided by the zoom before use, and never allowed below the scene's own default.
const STROKE_GRAB: f64 = 6.0;

/// One gesture in progress on the canvas.
///
/// A three-way enum rather than three sets of `Option` fields, because the states are
/// genuinely exclusive and the compiler should say so: a drag is drawing a new object, or
/// moving existing ones, or sweeping a rubber band, and never two of those.
#[derive(Debug)]
pub enum Gesture {
    /// A creating tool, drawing something that is not in the scene yet.
    Creating(Draft),
    /// `spec/05` §4.1's "Drag moves", with the objects as they were at the press so the
    /// move is absolute rather than accumulated.
    ///
    /// Absolute matters twice. Shift locks to the dominant axis mid-drag, and a locked
    /// move computed from per-frame deltas keeps whatever sideways travel happened before
    /// the key went down; and floating-point deltas accumulated over a few hundred frames
    /// drift off the pointer.
    Moving {
        before: Vec<Object>,
        from: Point,
        /// The label a *click* -- a release inside [`CLICK_SLOP`] -- opens for editing,
        /// when the press came from the text tool (D63). `None` for every other move.
        click_edits: Option<ObjectId>,
    },
    /// `spec/05` §4.1's "Drag on empty space rubber-band selects".
    Marquee { from: Point, additive: bool },
    /// `spec/05` §4.1's eight handles, on one object.
    ///
    /// One object and not the selection: §4.1 says "8 handles … for resizable objects"
    /// and shows them on *an* object, and resizing a group by one handle is a different
    /// feature with its own questions (does a group of arrows scale their heads?). The
    /// object as it was at the press, and the box the handles were on, so the drag is
    /// absolute for the same two reasons `Moving` keeps its own -- Shift can change what
    /// the drag means mid-gesture, and per-frame deltas drift off the pointer.
    Resizing { before: Object, from: Bounds, handle: Handle },
    /// §4.1's "2 end handles (+1 curve handle for curved arrows)".
    Ending { before: Object, index: usize },
    /// `spec/05` §9's "Space pan": a primary drag with Space held moves the view, not the
    /// document. `last` is the previous update's offset, in widget pixels, because a pan
    /// is the one gesture that *is* accumulated -- there is no object to be absolute
    /// about.
    Panning { last: (f64, f64) },
}

/// The tool strip's state.
#[derive(Debug, Default)]
pub struct ToolState {
    pub active: Cell<Tool>,
    /// `spec/05` §2: "The last colour and size **per tool** persist." So a map and not one
    /// shared value -- choosing red for the arrow must not make the pencil red. Each tool
    /// remembers everything it can set, which also stops the pixelate intensity and the
    /// spotlight opacity from being the same number.
    ///
    /// In-session only, for now. §2 names `markupLastColor` and `markupLastWidth`, which
    /// is cross-session persistence and belongs with the settings schema.
    pub memory: RefCell<std::collections::BTreeMap<Tool, ToolSettings>>,
    pub gesture: RefCell<Option<Gesture>>,
    /// Space is held (`spec/05` §9's "Space pan"). Set by the window's key controller.
    pub space_down: Cell<bool>,
    buttons: RefCell<Vec<(Tool, gtk::ToggleButton)>>,
    /// Guards the strip against its own signal.
    ///
    /// `set_active` on a `GtkToggleButton` emits `toggled`, whose handler calls
    /// `set_tool`, which calls `set_active` on every button. Without this the first key
    /// press recurses until the stack gives out.
    syncing: Cell<bool>,
}

impl Editor {
    /// `spec/05` §1: "tools at top".
    pub(super) fn build_tool_strip(self: &Rc<Self>) -> gtk::Widget {
        // No margins of its own: it sits in the header bar now (`spec/05` §1's single
        // toolbar row), which pads it.
        // One linked run per chunk (`Tool::CHUNKS`), with a gap between chunks: thirteen
        // identical toggles in one run was the one flat list left in the editor
        // (`spec/13` #5). The radio group still spans all of them -- GTK4's toggle group
        // is the first button, wherever the others are parented.
        let strip = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        strip.set_valign(gtk::Align::Center);

        let mut first: Option<gtk::ToggleButton> = None;
        for chunk in Tool::CHUNKS {
            // Linked, which is what makes a run of toggles read as one control.
            let group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            group.add_css_class("linked");
            group.add_css_class("octosnap-tools");
            for &tool in chunk {
                let button = gtk::ToggleButton::builder()
                    .icon_name(tool.icon())
                    .tooltip_text(format!("{} ({})", tool.label(), accelerator_label(tool)))
                    .build();
                match &first {
                    Some(anchor) => button.set_group(Some(anchor)),
                    None => first = Some(button.clone()),
                }
                button.set_active(tool == self.tools.active.get());
                {
                    let editor = Rc::downgrade(self);
                    button.connect_toggled(move |button| {
                        if !button.is_active() {
                            return;
                        }
                        let Some(editor) = editor.upgrade() else { return };
                        if editor.tools.syncing.get() {
                            return;
                        }
                        editor.set_tool(tool);
                    });
                }
                self.tools.buttons.borrow_mut().push((tool, button.clone()));
                group.append(&button);
            }
            strip.append(&group);
        }
        strip.upcast()
    }

    /// Selects a tool, from the strip or from a key.
    ///
    /// `spec/05` §4.11's crop is a *mode* rather than a tool that draws, so choosing it
    /// opens crop mode and choosing anything else closes it -- and closing it cancels,
    /// because §4.11 gives Enter as the only thing that applies. Switching tools with a
    /// crop half-dragged and having it apply itself would be an edit nobody asked for.
    pub(super) fn set_tool(self: &Rc<Self>, tool: Tool) {
        let was_cropping = self.cropping();
        self.tools.active.set(tool);
        // Leaving a tool abandons anything half-drawn: a draft belongs to the tool that
        // started it, and keeping it would commit a rectangle with the ellipse tool.
        *self.tools.gesture.borrow_mut() = None;
        self.canvas.set_draft(None);
        self.canvas.set_marquee(None);
        if !tool.creates() {
            // Only the Select tool has a selection to show. Switching to a drawing tool
            // and leaving the chrome behind invites a drag on a handle that does nothing.
        } else {
            self.canvas.set_selection(Vec::new());
        }

        self.tools.syncing.set(true);
        for (which, button) in self.tools.buttons.borrow().iter() {
            button.set_active(*which == tool);
        }
        self.tools.syncing.set(false);
        // `spec/05` §2's row belongs to the tool, so it is rebuilt here rather than
        // anywhere the tool could have changed without this being noticed.
        self.refresh_options();
        if tool == Tool::Crop {
            if !was_cropping {
                self.enter_crop();
            }
        } else if was_cropping {
            // `cancel_crop` calls back into `set_tool(Select)`, which is why the state is
            // read once at the top: the second pass finds `was_cropping` false and stops.
            self.cancel_crop();
        }
        debug!(tool = tool.label(), "tool selected");
    }

    /// Pointer gestures on the canvas: draw, move, and rubber-band.
    ///
    /// **The order the controllers are added in is load-bearing.** GTK runs a widget's
    /// controllers most-recently-added first, and the double-click controller has to run
    /// *after* the drag's: a second press reaches `begin_gesture` first, which finds no
    /// edit open and starts a `Moving`, and then the click controller opens the label for
    /// editing and abandons that move. The other way round -- which is what adding the
    /// click controller second produced -- the edit opened and the drag's `begin_gesture`
    /// saw an edit open and committed it in the same millisecond, so every double-click
    /// flashed an editor that was gone before the button came up.
    pub(super) fn wire_tools(self: &Rc<Self>) {
        // `spec/05` §4.1: "Double-click a text object edits it." On its own controller
        // rather than inside the drag, because a `GtkGestureDrag` has no press count and
        // a double-click is two drags of zero length as far as it is concerned.
        let clicks = gtk::GestureClick::new();
        clicks.set_button(gtk::gdk::BUTTON_PRIMARY);
        {
            let editor = Rc::downgrade(self);
            clicks.connect_pressed(move |_, presses, x, y| {
                if presses != 2 {
                    return;
                }
                let Some(editor) = editor.upgrade() else { return };
                let at = editor.canvas.to_document(x, y);
                if editor.edit_text_under(at) {
                    debug!("double-click opened a text object");
                } else if editor.edit_counter_under(at) {
                    debug!("double-click opened a counter's number");
                }
            });
        }
        self.canvas.add_controller(clicks);

        let drag = gtk::GestureDrag::new();
        drag.set_button(gtk::gdk::BUTTON_PRIMARY);

        {
            let editor = Rc::downgrade(self);
            drag.connect_drag_begin(move |gesture, x, y| {
                let Some(editor) = editor.upgrade() else { return };
                editor.begin_gesture(x, y, modifiers_of(gesture));
            });
        }
        {
            let editor = Rc::downgrade(self);
            drag.connect_drag_update(move |gesture, ox, oy| {
                let Some(editor) = editor.upgrade() else { return };
                editor.update_gesture(ox, oy, modifiers_of(gesture));
            });
        }
        {
            let editor = Rc::downgrade(self);
            drag.connect_drag_end(move |gesture, ox, oy| {
                let Some(editor) = editor.upgrade() else { return };
                editor.update_gesture(ox, oy, modifiers_of(gesture));
                editor.end_gesture();
            });
        }
        self.canvas.add_controller(drag);

        // The resize cursor over a crop handle. On its own controller because it is about
        // the pointer being *somewhere* rather than about a button being down, and the
        // crop frame is unusable without it: eight grips that all look draggable and give
        // no feedback about which edge they will move.
        let motion = gtk::EventControllerMotion::new();
        {
            let editor = Rc::downgrade(self);
            motion.connect_motion(move |_, x, y| {
                let Some(editor) = editor.upgrade() else { return };
                if editor.cropping() {
                    editor.crop_hover(x, y);
                } else {
                    editor.handle_hover(x, y);
                }
            });
        }
        self.canvas.add_controller(motion);
    }

    fn begin_gesture(self: &Rc<Self>, x: f64, y: f64, modifiers: Modifiers) {
        let at = self.canvas.to_document(x, y);
        // `spec/05` §2's pipette, if it is armed, takes the click before any tool sees it.
        if self.take_sample(at) {
            return;
        }
        // A click anywhere while a text box is open ends the edit, and does nothing else.
        // Standard behaviour, and the alternative -- committing *and* starting a shape --
        // makes it impossible to simply finish typing.
        if self.text_editing() {
            self.commit_text_edit();
            return;
        }
        // Space held: the drag pans, whatever tool is lit and whether or not a crop frame
        // is up -- panning is how you reach the part of a large picture you want to crop.
        if self.tools.space_down.get() {
            self.set_hover(Hover::Panning);
            *self.tools.gesture.borrow_mut() = Some(Gesture::Panning { last: (0.0, 0.0) });
            return;
        }
        // `spec/05` §4.11's crop mode owns every pointer gesture on the canvas: there is
        // nothing else to click on while a crop frame is up.
        if self.crop_press(at) {
            return;
        }
        let tool = self.tools.active.get();
        let Some(scene) = self.canvas.scene() else { return };

        // A handle on the current selection wins over whatever is under it, with any
        // tool. It has to: the corner handles sit *on* the object's own edge, so a hit
        // test that ran first would answer the object every time and the handles would
        // be undraggable -- and a user who has just drawn a box still has the box tool
        // lit when they reach for its corner.
        if let Some(gesture) = self.grab_handle(&scene, at) {
            *self.tools.gesture.borrow_mut() = Some(gesture);
            return;
        }

        // What is under the pointer, unless the tool is freehand -- see the note at the
        // top of this file.
        let under = if tool.is_freehand() { None } else { self.object_at(&scene, at) };

        // The text tool on a label: a drag moves it and a click edits it. Which of the two
        // a press is going to be is only known when the button comes up, so the press
        // starts the move like any other tool's and remembers what a click would mean;
        // `end_gesture` decides by how far the pointer travelled. The first version opened
        // the edit on the press -- "one click, not two" -- and a label could then never be
        // moved with the tool that made it, because every press on it was an edit (D63).
        // `spec/05` §4.1's double-click under the Select tool works as it did.
        let click_edits = (tool == Tool::Text && !modifiers.alt && !modifiers.shift)
            .then(|| under.and_then(|id| scene.get(id)))
            .flatten()
            .filter(|object| matches!(object.geometry, octosnap_scene::Geometry::Text { .. }))
            .map(|object| object.id);

        if under.is_none() && tool.creates() {
            let draft = Draft::begin(tool, self.settings(), at, modifiers);
            self.preview_draft(draft.as_ref());
            *self.tools.gesture.borrow_mut() = draft.map(Gesture::Creating);
            return;
        }

        match under {
            Some(id) => {
                let mut selection = self.canvas.selection();
                if modifiers.shift {
                    // "Shift+click adds to selection", and removes again -- a toggle is
                    // what makes a mis-added object recoverable without starting over.
                    if let Some(index) = selection.iter().position(|s| *s == id) {
                        selection.remove(index);
                    } else {
                        selection.push(id);
                    }
                } else if !selection.contains(&id) {
                    selection = vec![id];
                }
                self.canvas.set_selection(selection.clone());

                // "Alt-drag duplicates": the copies are made now and it is the copies
                // that move, so the originals stay where they were.
                let selection = if modifiers.alt && !selection.is_empty() {
                    self.duplicate(&selection, 0.0)
                } else {
                    selection
                };
                let before = objects_of(&scene, &selection);
                *self.tools.gesture.borrow_mut() =
                    Some(Gesture::Moving { before, from: at, click_edits });
            }
            None => {
                if !modifiers.shift {
                    self.canvas.set_selection(Vec::new());
                }
                *self.tools.gesture.borrow_mut() =
                    Some(Gesture::Marquee { from: at, additive: modifiers.shift });
            }
        }
    }

    fn update_gesture(self: &Rc<Self>, ox: f64, oy: f64, modifiers: Modifiers) {
        if let Some(Gesture::Panning { last }) = self.tools.gesture.borrow_mut().as_mut() {
            let (px, py) = *last;
            *last = (ox, oy);
            self.canvas.pan_by(ox - px, oy - py);
            self.reposition_text_edit();
            return;
        }
        if self.cropping() {
            self.crop_motion(ox, oy);
            return;
        }
        let zoom = self.canvas.zoom();
        let (dx, dy) = (ox / zoom, oy / zoom);
        let mut gesture = self.tools.gesture.borrow_mut();
        match gesture.as_mut() {
            Some(Gesture::Creating(draft)) => {
                let from = draft_origin(draft);
                draft.extend(from.offset(dx, dy), modifiers);
                let draft = draft.clone();
                drop(gesture);
                self.preview_draft(Some(&draft));
            }
            Some(Gesture::Moving { before, from, .. }) => {
                // "Shift locks to the dominant axis" -- decided from the whole travel, so
                // a drag that starts sideways and turns downwards locks to whichever the
                // user has committed to rather than to the first pixel.
                let (dx, dy) = if modifiers.shift {
                    if dx.abs() >= dy.abs() { (dx, 0.0) } else { (0.0, dy) }
                } else {
                    (dx, dy)
                };
                let before = before.clone();
                let _ = from;
                drop(gesture);
                self.preview_move(&before, dx, dy);
            }
            Some(Gesture::Marquee { from, .. }) => {
                let bounds = Bounds::from_corners(*from, from.offset(dx, dy));
                drop(gesture);
                self.canvas.set_marquee(Some(bounds));
            }
            Some(Gesture::Resizing { before, from, handle }) => {
                let (before, from, handle) = (before.clone(), *from, *handle);
                drop(gesture);
                let grabbed = handle.position(from);
                let to = grabbed.offset(dx, dy);
                // Shift keeps the proportions. `spec/05` §4.1 gives Shift as an axis lock
                // for a *move* and says nothing about a resize; keeping the ratio is what
                // Shift means on a handle everywhere else, and the arithmetic is the same
                // function §4.11's aspect lock uses.
                let bounds = if modifiers.shift && !from.is_empty() {
                    handle::resize_to_ratio(from, handle, to, from.width / from.height)
                } else {
                    handle::resize(from, handle, to)
                };
                self.preview_resize(&before, from, bounds, handle);
            }
            Some(Gesture::Ending { before, index }) => {
                let (before, index) = (before.clone(), *index);
                drop(gesture);
                let Some(was) = before.points().get(index).copied() else { return };
                let to = was.offset(dx, dy);
                self.preview_end(&before, index, to);
            }
            // Handled above, before the crop check.
            Some(Gesture::Panning { .. }) | None => {}
        }
    }

    fn end_gesture(self: &Rc<Self>) {
        if matches!(*self.tools.gesture.borrow(), Some(Gesture::Panning { .. })) {
            *self.tools.gesture.borrow_mut() = None;
            self.set_hover(if self.tools.space_down.get() { Hover::Pan } else { Hover::Idle });
            return;
        }
        if self.cropping() {
            self.crop_release();
            return;
        }
        let Some(gesture) = self.tools.gesture.borrow_mut().take() else { return };
        match gesture {
            Gesture::Creating(draft) => {
                self.canvas.set_draft(None);
                // `spec/05` §4.5: text is not committed by the gesture at all. The click
                // "places a text box … and starts inline editing", and what gets stored is
                // whatever the user then types -- so an empty click leaves nothing behind
                // rather than an invisible object with no text in it.
                if draft.tool() == Tool::Text {
                    // A click is an auto-width box at the pointer; a drag is a box of
                    // the dragged width, which wraps (`Draft::text_box`).
                    let (pos, width) = draft.text_box();
                    self.begin_text_edit(pos, width, None);
                    return;
                }
                let Some(mut scene) = self.canvas.scene() else { return };
                let Some(mut object) = draft.object(
                    scene.top_z().saturating_add(1),
                    now_seconds(),
                    scene.next_counter_number(),
                ) else {
                    // Below the minimum drag: `spec/05` §4.2's "otherwise cancelled".
                    return;
                };
                // The same call the preview made, with the same modifiers, so the stroke
                // that lands is the stroke the user was looking at (`spec/05` §6's
                // argument, one gesture down).
                self.snap_highlighter(&mut object, draft.modifiers());
                let id = object.id;
                if self.history.borrow_mut().apply(&mut scene, Command::Add(object)) {
                    self.canvas.update_scene(scene);
                    self.history.borrow_mut().end_gesture();
                    // Newly drawn objects are selected, which is what makes the options
                    // row able to restyle the thing that was just made.
                    self.canvas.set_selection(vec![id]);
                    self.after_edit();
                }
            }
            Gesture::Moving { before, click_edits, .. } => {
                let Some(mut scene) = self.canvas.scene() else { return };
                // A press on a label with the text tool that came up within a click's
                // travel was a click, and a click edits (D63). The objects go back where
                // the press found them -- a click's own jitter has moved them by a pixel
                // -- and the label opens.
                if let Some(id) = click_edits
                    && !self.travelled(&before, &scene)
                {
                    for was in &before {
                        scene.replace(was.clone());
                    }
                    self.canvas.update_scene(scene);
                    let label = self.canvas.scene().and_then(|scene| scene.get(id).cloned());
                    if let Some(object) = label
                        && let octosnap_scene::Geometry::Text { pos, .. } = object.geometry
                    {
                        self.begin_text_edit(pos, None, Some(object));
                        debug!("the text tool's click opened a label");
                    }
                    return;
                }
                // The preview has already moved them; the command records the pair so
                // undo has somewhere to go back to.
                let changes: Vec<Command> = before
                    .iter()
                    .filter_map(|was| {
                        let now = scene.get(was.id)?;
                        (now != was).then(|| Command::Change {
                            id: was.id,
                            before: Box::new(was.clone()),
                            after: Box::new(now.clone()),
                        })
                    })
                    .collect();
                self.commit_batch(changes);
            }
            Gesture::Resizing { before, .. } | Gesture::Ending { before, .. } => {
                // The preview has already changed the object; the command records the
                // pair. One `Change`, whatever the drag did, because §5.2 makes a gesture
                // one step.
                let Some(scene) = self.canvas.scene() else { return };
                let Some(now) = scene.get(before.id) else { return };
                if *now == before {
                    return;
                }
                let after = now.clone();
                self.commit(Command::Change {
                    id: before.id,
                    before: Box::new(before.clone()),
                    after: Box::new(after.clone()),
                });
                // One line per reshape, and it exists to be grepped. An object's geometry
                // is invisible from outside the process -- `after_edit` can say a step
                // happened and not what it did -- so `editor-test.sh` reads this, the same
                // way it reads `crop rect` and `canvas allocated`.
                let box_after = after.bounds();
                // Two decimals, not rounded: `spec/05` §4.5's whole point about the
                // corner handle is that it "sets any value in between, which is why the
                // stored preference is a float such as 37.744", and an integer in the log
                // cannot tell 38.4 from one of the thirteen presets.
                let font = match after.geometry {
                    octosnap_scene::Geometry::Text { font_size, .. } => format!("{font_size:.2}"),
                    _ => "0".to_owned(),
                };
                #[allow(clippy::cast_possible_truncation)]
                let (width, height) =
                    (box_after.width.round() as i64, box_after.height.round() as i64);
                tracing::debug!(kind = ?after.kind(), width, height, font, "object reshaped");
                // The handles moved with the shape; say where they are now, or a harness
                // aiming at the next one aims at where the old box was.
                self.canvas.report_selection();
            }
            // Ended above; the arm exists so the match stays exhaustive.
            Gesture::Panning { .. } => {}
            Gesture::Marquee { from, additive } => {
                self.canvas.set_marquee(None);
                let Some(scene) = self.canvas.scene() else { return };
                // The band's own bounds, re-derived: the pointer has stopped, so the last
                // update's rectangle is the final one.
                let Some(bounds) = self.last_marquee(from) else { return };
                let mut picked = hit::within(&scene, bounds);
                if additive {
                    let mut selection = self.canvas.selection();
                    selection.retain(|id| !picked.contains(id));
                    selection.append(&mut picked);
                    self.canvas.set_selection(selection);
                } else {
                    self.canvas.set_selection(picked);
                }
            }
        }
    }

    /// The rubber band as the canvas last drew it.
    ///
    /// Asked of the canvas rather than recomputed from the gesture, because the canvas is
    /// what the user was looking at -- and a selection that disagrees with the rectangle
    /// on screen is the kind of bug nobody reports precisely.
    fn last_marquee(&self, from: Point) -> Option<Bounds> {
        self.canvas.marquee().or(Some(Bounds::from_corners(from, from)))
    }

    fn preview_draft(&self, draft: Option<&Draft>) {
        let object = draft.and_then(|draft| {
            let scene = self.canvas.scene()?;
            let mut object = draft.object(
                scene.top_z().saturating_add(1),
                now_seconds(),
                scene.next_counter_number(),
            )?;
            // The preview snaps too, and that is the whole feel of §4.7: the band jumps to
            // the line while the pen is still down, so the user can see what releasing
            // will leave -- and what Ctrl would leave instead.
            self.snap_highlighter(&mut object, draft.modifiers());
            Some(object)
        });
        self.canvas.set_draft(object);
    }

    /// Draws a highlighter stroke between two points, as a drag would.
    ///
    /// The `highlight` window action's other half, and the reason it goes through `Draft`
    /// rather than building an `Object`: the point of the check is that the *production*
    /// path snaps, so anything it skips is a thing that could be broken and not noticed.
    pub(super) fn highlight_between(self: &Rc<Self>, from: Point, to: Point, held: Modifiers) {
        let settings = self.memory_of(Tool::Highlighter);
        let Some(mut draft) = Draft::begin(Tool::Highlighter, settings, from, held) else {
            return;
        };
        draft.extend(to, held);
        let Some(mut scene) = self.canvas.scene() else { return };
        let Some(mut object) = draft.object(
            scene.top_z().saturating_add(1),
            now_seconds(),
            scene.next_counter_number(),
        ) else {
            debug!("the highlight action's two points are below the minimum drag");
            return;
        };
        self.snap_highlighter(&mut object, held);
        if self.history.borrow_mut().apply(&mut scene, Command::Add(object)) {
            self.canvas.update_scene(scene);
            self.history.borrow_mut().end_gesture();
            self.after_edit();
        }
    }

    /// `spec/05` §4.7's Smart Highlighter, applied to a stroke the highlighter drafted.
    ///
    /// > while dragging, detect the text line under the stroke … and snap the stroke to a
    /// > straight horizontal band covering those rows with 2 units of margin. Ctrl held
    /// > disables snapping. Falls back to the freehand stroke if no line is found.
    ///
    /// Every one of those clauses is a `return` here, and the arithmetic is
    /// `scene::highlight`. What is left is the two coordinate changes the detector cannot
    /// make for itself: the stroke goes into the stored image's own pixel space -- through
    /// `Base::orientation`, because §4.12 may have turned the document since (D91) -- and
    /// the band it answers with comes back out.
    pub(super) fn snap_highlighter(&self, object: &mut Object, modifiers: Modifiers) {
        // §4.7's "Ctrl held disables snapping", and §2's Smart toggle on the row.
        if modifiers.ctrl || !self.memory_of(Tool::Highlighter).smart_highlighter {
            return;
        }
        let octosnap_scene::Geometry::Path { points, highlighter: true, .. } = &object.geometry
        else {
            return;
        };
        let Some(scene) = self.canvas.scene() else { return };
        let Some(pixels) = self.canvas.base_pixels() else { return };
        let Some(per_unit) = self.canvas.base_pixel_scale() else { return };
        let orientation = scene.base.orientation;
        let (width, height) = (scene.base.width, scene.base.height);
        let stored: Vec<Point> =
            points.iter().map(|p| orientation.unplace(*p, width, height)).collect();
        // §4.7's "±(3×size)", which is the highlighter's own drawn width: the size control
        // is how far the snap looks, and a size too small for the line finds nothing.
        let reach = object.style.stroke_width() * octosnap_scene::tool::HIGHLIGHTER_WIDTH_FACTOR;
        let Some(band) = octosnap_scene::highlight::snap(&pixels, per_unit, &stored, reach)
        else {
            return;
        };
        let snapped: Vec<Point> =
            band.points().iter().map(|p| orientation.place(*p, width, height)).collect();
        object.geometry = octosnap_scene::Geometry::Path {
            points: snapped,
            // A two-point band has nothing to smooth, and saying so keeps the stored
            // object honest about what it is.
            smoothing: false,
            highlighter: true,
            band: Some(band.height),
        };
    }

    /// The topmost object under `at`, with a click target that is the same size on
    /// screen at every zoom -- and, for text, the box the glyphs actually occupy rather
    /// than the scene's 0.6-em estimate of it (D57: the hit box is what is drawn).
    pub(super) fn object_at(&self, scene: &Scene, at: Point) -> Option<ObjectId> {
        let tolerance = (STROKE_GRAB / self.canvas.zoom()).max(hit::TOLERANCE);
        let canvas = self.canvas.clone();
        hit::topmost_measured(scene, at, tolerance, &|object| {
            matches!(object.geometry, octosnap_scene::Geometry::Text { .. })
                .then(|| canvas.measured_bounds(object))
        })
    }

    /// The handle under `at`, if the selection is one object and it has any.
    ///
    /// One object, because that is what §4.1's chrome draws handles on. `GRAB` is in
    /// widget pixels and divided by the zoom, because the handle is eight pixels on
    /// screen at every zoom and its target has to be too.
    fn grab_handle(&self, scene: &Scene, at: Point) -> Option<Gesture> {
        let selection = self.canvas.selection();
        let [id] = selection[..] else { return None };
        let object = scene.get(id)?;
        if object.locked {
            return None;
        }
        let tolerance = HANDLE_GRAB / self.canvas.zoom();
        let grips = self.canvas.grips_of(object);
        let index = grips
            .iter()
            .enumerate()
            .filter(|(_, point)| point.distance_to(at) <= tolerance)
            .min_by(|(_, a), (_, b)| {
                a.distance_to(at).total_cmp(&b.distance_to(at))
            })
            .map(|(index, _)| index)?;
        match object.grips() {
            Grips::Box => Some(Gesture::Resizing {
                before: object.clone(),
                from: self.canvas.measured_bounds(object),
                handle: *Handle::ALL.get(index)?,
            }),
            Grips::Ends(_) => Some(Gesture::Ending { before: object.clone(), index }),
            Grips::None => None,
        }
    }

    /// Resizes for the preview only -- the scene changes, the history does not.
    fn preview_resize(&self, before: &Object, from: Bounds, to: Bounds, handle: Handle) {
        let Some(mut scene) = self.canvas.scene() else { return };
        let mut resized = before.clone();
        resized.resize(from, to, handle);
        scene.replace(resized);
        self.canvas.update_scene(scene);
    }

    /// Moves one end point for the preview only.
    fn preview_end(&self, before: &Object, index: usize, to: Point) {
        let Some(mut scene) = self.canvas.scene() else { return };
        let mut moved = before.clone();
        if moved.move_point(index, to) {
            scene.replace(moved);
            self.canvas.update_scene(scene);
        }
    }

    /// Whether a move went further than a click's own jitter, measured on screen.
    fn travelled(&self, before: &[Object], scene: &Scene) -> bool {
        let zoom = self.canvas.zoom();
        before.iter().any(|was| {
            scene.get(was.id).is_some_and(|now| {
                let (a, b) = (was.bounds(), now.bounds());
                (b.x - a.x).hypot(b.y - a.y) * zoom > CLICK_SLOP
            })
        })
    }

    /// Moves the selection for the preview only -- the scene changes, the history does not.
    fn preview_move(&self, before: &[Object], dx: f64, dy: f64) {
        let Some(mut scene) = self.canvas.scene() else { return };
        for was in before {
            let mut moved = was.clone();
            moved.translate(dx, dy);
            scene.replace(moved);
        }
        self.canvas.update_scene(scene);
    }

    /// Applies a group of commands as one undo step, or nothing if the group is empty.
    pub(super) fn commit_batch(&self, mut commands: Vec<Command>) {
        let command = match commands.len() {
            0 => return,
            1 => commands.remove(0),
            _ => Command::Batch(commands),
        };
        self.commit(command);
    }

    /// [`Self::commit_batch`] without closing the step, for a control still being dragged.
    ///
    /// `History::apply` folds a change into the previous one while the gesture is open, so
    /// a slider's forty ticks become one undo step. The caller closes it when the pointer
    /// lifts; until then the *next* closed commit closes it too, which is the right
    /// answer for a drag that was interrupted by something else.
    pub(super) fn commit_batch_open(&self, mut commands: Vec<Command>) {
        let command = match commands.len() {
            0 => return,
            1 => commands.remove(0),
            _ => Command::Batch(commands),
        };
        let Some(mut scene) = self.canvas.scene() else { return };
        if self.history.borrow_mut().apply(&mut scene, command) {
            self.canvas.update_scene(scene);
            self.after_edit();
        }
    }

    /// Applies one command and records it as a finished step.
    pub(super) fn commit(&self, command: Command) {
        let Some(mut scene) = self.canvas.scene() else { return };
        if self.history.borrow_mut().apply(&mut scene, command) {
            self.canvas.update_scene(scene);
            self.history.borrow_mut().end_gesture();
            self.after_edit();
        }
    }

    /// `spec/05` §4.1: "Delete/Backspace removes".
    pub(super) fn delete_selection(self: &Rc<Self>) {
        let Some(scene) = self.canvas.scene() else { return };
        let removals: Vec<Command> = self
            .canvas
            .selection()
            .iter()
            .filter_map(|id| scene.get(*id).cloned().map(Command::Remove))
            .collect();
        if removals.is_empty() {
            return;
        }
        self.canvas.set_selection(Vec::new());
        self.commit_batch(removals);
    }

    /// `spec/05` §4.1: "Ctrl+D duplicates offset by 12 units".
    pub(super) fn duplicate_selection(self: &Rc<Self>) {
        let selection = self.canvas.selection();
        if selection.is_empty() {
            return;
        }
        let copies = self.duplicate(&selection, DUPLICATE_OFFSET);
        self.canvas.set_selection(copies);
    }

    /// `spec/05` §4.1's "Ctrl+C/Ctrl+V copies objects": the selection onto the internal
    /// clipboard. Answers how many objects were copied, which is none when the selection
    /// holds only the kinds that are not copied (`clipboard::copyable`).
    pub(super) fn copy_objects(&self, ids: &[ObjectId]) -> usize {
        let Some(scene) = self.canvas.scene() else { return 0 };
        let count = OBJECTS.with_borrow_mut(|clipboard| clipboard.copy(&scene, ids));
        tracing::info!(count, "copied objects");
        count
    }

    /// Ctrl+X's half of the clipboard: [`Editor::copy_objects`], with the first paste put
    /// back in place (`clipboard::Clipboard::cut`). The caller deletes the objects.
    pub(super) fn cut_objects(&self, ids: &[ObjectId]) -> usize {
        let Some(scene) = self.canvas.scene() else { return 0 };
        let count = OBJECTS.with_borrow_mut(|clipboard| clipboard.cut(&scene, ids));
        tracing::info!(count, "cut objects");
        count
    }

    /// Ctrl+V: the objects on the internal clipboard, pasted as one step and selected.
    /// Answers whether anything was pasted, so the caller can try the desktop's clipboard
    /// instead.
    ///
    /// An image object that was copied out of a document whose files have since gone
    /// (another editor, closed, and its extracted project with it) is left out rather than
    /// pasted as a picture of nothing.
    pub(super) fn paste_objects(self: &Rc<Self>) -> bool {
        let Some(scene) = self.canvas.scene() else { return false };
        let pasted: Vec<Object> = OBJECTS
            .with_borrow_mut(|clipboard| clipboard.paste(&scene))
            .into_iter()
            .filter(|object| match &object.geometry {
                octosnap_scene::Geometry::Image { file, .. } => std::path::Path::new(file).is_file(),
                _ => true,
            })
            .collect();
        if pasted.is_empty() {
            return false;
        }
        let ids: Vec<ObjectId> = pasted.iter().map(|object| object.id).collect();
        tracing::info!(count = ids.len(), "pasted objects");
        self.commit_batch(pasted.into_iter().map(Command::Add).collect());
        self.canvas.set_selection(ids);
        true
    }

    /// Copies the given objects, offset, and selects nothing -- the caller decides.
    ///
    /// Answers the new ids so an Alt-drag can move the copies instead of the originals.
    fn duplicate(self: &Rc<Self>, ids: &[ObjectId], offset: f64) -> Vec<ObjectId> {
        let Some(scene) = self.canvas.scene() else { return Vec::new() };
        let mut z = scene.top_z();
        let mut adds = Vec::new();
        let mut new_ids = Vec::new();
        for id in ids {
            let Some(original) = scene.get(*id) else { continue };
            let mut copy = original.clone();
            // A new identity, not a copy of one: `spec/05` §5.1's id is a ULID and two
            // objects sharing one would make `Scene::get` answer either of them.
            copy.id = ObjectId::new();
            copy.translate(offset, offset);
            z = z.saturating_add(1);
            copy.z = z;
            new_ids.push(copy.id);
            adds.push(Command::Add(copy));
        }
        self.commit_batch(adds);
        new_ids
    }

    /// `spec/05` §4.1: "double-click a counter edits its number".
    ///
    /// A popover on the badge with one spin button. Enter commits and closes; closing by
    /// any other route commits whatever the field holds, because a value typed and then
    /// clicked away from is a value the user chose. The badge keeps its place in the
    /// sequence -- `Scene::next_counter_number` counts badges, not numbers -- so the next
    /// one placed continues from the *count*, which is what makes renumbering one badge
    /// a local edit rather than a change to the document's numbering.
    pub(super) fn edit_counter_under(self: &Rc<Self>, at: Point) -> bool {
        let Some(scene) = self.canvas.scene() else { return false };
        let Some(id) = self.object_at(&scene, at) else { return false };
        let Some(object) = scene.get(id).cloned() else { return false };
        let octosnap_scene::Geometry::Counter { center, number, radius, .. } = object.geometry
        else {
            return false;
        };
        self.canvas.set_selection(vec![id]);

        // Parented to the canvas's own parent -- an `Overlay`, which has a layout manager
        // and so presents popover children by itself. The canvas is a custom widget with
        // its own `size_allocate` and would have to call `present` for each one by hand.
        let host: gtk::Widget = self.canvas.parent().unwrap_or_else(|| self.canvas.clone().upcast());
        let popover = gtk::Popover::new();
        popover.set_parent(&host);
        popover.set_autohide(true);
        let zoom = self.canvas.zoom();
        let (wx, wy) = self.canvas.to_widget(Point::new(center.x - radius, center.y - radius));
        #[allow(clippy::cast_possible_truncation)]
        let side = ((radius * 2.0 * zoom).round() as i32).max(1);
        #[allow(clippy::cast_possible_truncation)]
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
            wx.round() as i32,
            wy.round() as i32,
            side,
            side,
        )));

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_margin_top(6);
        row.set_margin_bottom(6);
        row.set_margin_start(6);
        row.set_margin_end(6);
        row.append(&gtk::Label::new(Some("Number")));
        let spin = gtk::SpinButton::with_range(0.0, 9999.0, 1.0);
        spin.set_value(f64::from(number));
        row.append(&spin);
        popover.set_child(Some(&row));
        // The popover weakly in the spin button's handler, and from the controller in its
        // own: both are inside the popover, and a strong copy in either was a ring that
        // kept every popover a counter had opened (D139).
        {
            let popover = popover.downgrade();
            spin.connect_activate(move |_| {
                if let Some(popover) = popover.upgrade() {
                    popover.popdown();
                }
            });
        }
        {
            // Enter, caught on the popover itself. The spin button's own `activate` did
            // not fire for an injected Return in the nested shell while Escape closed the
            // popover fine, so the key is taken here, before the field sees it: commit
            // what is typed, then close, and the closed handler does the rest.
            let spin = spin.clone();
            let keys = gtk::EventControllerKey::new();
            keys.set_propagation_phase(gtk::PropagationPhase::Capture);
            keys.connect_key_pressed(move |keys, key, _, _| {
                if matches!(key, gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter) {
                    spin.update();
                    if let Some(popover) = keys.widget().and_downcast::<gtk::Popover>() {
                        popover.popdown();
                    }
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            popover.add_controller(keys);
        }
        {
            let editor = Rc::downgrade(self);
            let spin = spin.clone();
            popover.connect_closed(move |popover| {
                // A typed value reaches the adjustment on activate or focus-out; a click
                // outside the popover is neither, so ask for it.
                spin.update();
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let value = spin.value().round().max(0.0) as u32;
                if let Some(editor) = editor.upgrade() {
                    editor.renumber(&object, value);
                }
                // Parented by hand, so unparented by hand -- after the signal, because a
                // widget must not be unparented from inside its own closing.
                let popover = popover.clone();
                glib::idle_add_local_once(move || popover.unparent());
            });
        }
        popover.popup();
        spin.grab_focus();
        tracing::info!(number, "counter number edit opened");
        true
    }

    /// One badge's number, as one undo step.
    fn renumber(self: &Rc<Self>, before: &Object, number: u32) {
        let octosnap_scene::Geometry::Counter { number: was, .. } = before.geometry else { return };
        if was == number {
            return;
        }
        let mut after = before.clone();
        if let octosnap_scene::Geometry::Counter { number: n, .. } = &mut after.geometry {
            *n = number;
        }
        self.commit(Command::Change {
            id: before.id,
            before: Box::new(before.clone()),
            after: Box::new(after),
        });
        tracing::info!(from = was, to = number, "counter renumbered");
    }

    /// `spec/05` §4.1: "arrow keys nudge 1 unit, Shift+arrows 10".
    pub(super) fn nudge_selection(self: &Rc<Self>, dx: f64, dy: f64) {
        let Some(scene) = self.canvas.scene() else { return };
        let changes: Vec<Command> = self
            .canvas
            .selection()
            .iter()
            .filter_map(|id| {
                let was = scene.get(*id)?;
                let mut now = was.clone();
                now.translate(dx, dy);
                Some(Command::Change {
                    id: *id,
                    before: Box::new(was.clone()),
                    after: Box::new(now),
                })
            })
            .collect();
        self.commit_batch(changes);
    }

    /// `spec/05` §4.1: "Ctrl+] / Ctrl+[ bring forward/back".
    pub(super) fn reorder_selection(self: &Rc<Self>, forward: bool) {
        let Some(scene) = self.canvas.scene() else { return };
        let extreme = if forward {
            scene.top_z().saturating_add(1)
        } else {
            scene.objects().iter().map(|o| o.z).min().unwrap_or(0).saturating_sub(1)
        };
        let changes: Vec<Command> = self
            .canvas
            .selection()
            .iter()
            .filter_map(|id| {
                let object = scene.get(*id)?;
                (object.z != extreme).then_some(Command::Reorder {
                    id: *id,
                    before: object.z,
                    after: extreme,
                })
            })
            .collect();
        self.commit_batch(changes);
    }

    /// `spec/05` §2: "Digits 1–6 set the size level of the current tool/selection".
    ///
    /// Both, as the spec says: the tool keeps it for the next object and the selection
    /// takes it now.
    pub(super) fn set_size_level(self: &Rc<Self>, level: u8) {
        // The same path a click in the size popover takes, so the digit and the popover
        // cannot disagree about what a size change does to a rounded corner.
        self.change(octosnap_scene::tool::Control::Size, |s| s.style.size = level);
    }

    pub(super) fn undo(self: &Rc<Self>) {
        let Some(mut scene) = self.canvas.scene() else { return };
        if self.history.borrow_mut().undo(&mut scene) {
            // A selection can name an object the undo has just removed. Dropping the ones
            // that are gone rather than clearing everything keeps a multi-select intact
            // through an undo of one member.
            let live: Vec<ObjectId> =
                self.canvas.selection().into_iter().filter(|id| scene.get(*id).is_some()).collect();
            self.canvas.update_scene(scene);
            self.canvas.set_selection(live);
            self.after_edit();
        }
    }

    pub(super) fn redo(self: &Rc<Self>) {
        let Some(mut scene) = self.canvas.scene() else { return };
        if self.history.borrow_mut().redo(&mut scene) {
            self.canvas.update_scene(scene);
            self.after_edit();
        }
    }

    /// `spec/05` §4.9's starting number, which belongs to the *document*.
    ///
    /// "the badge numbering is a property of the document rather than of each badge", so
    /// it is read from and written to the scene, not to the tool's settings.
    pub(super) fn starting_number(&self) -> f64 {
        self.canvas.scene().map_or(1.0, |scene| f64::from(scene.starting_number()))
    }

    pub(super) fn set_starting_number(self: &Rc<Self>, number: u32) {
        let Some(mut scene) = self.canvas.scene() else { return };
        if scene.starting_number() == number {
            return;
        }
        scene.set_next_number(number);
        // Not a `Command`: this is a document *preference* rather than an edit to an
        // object, and putting it on the undo stack would make Ctrl+Z renumber badges the
        // user had not touched.
        self.canvas.update_scene(scene);
    }

    /// The cursor says what a press would do here.
    ///
    /// Resize arrows over §4.1's handles, a move cursor over the selected object, the
    /// plain pointer over an object a press would select, and the tool's own shape over
    /// empty picture: a crosshair for the drawing tools, an I-beam for text. Eight
    /// identical squares that all look draggable and give no feedback about which edge
    /// they will move was the gap the crop frame had; a drawing tool that shows a
    /// crosshair over the very box it is about to *select* is the same gap.
    pub(super) fn handle_hover(self: &Rc<Self>, x: f64, y: f64) {
        // Left alone when the pipette is armed: that cursor was set deliberately and a
        // pointer wandering over a handle must not take it off. And left alone during a
        // gesture, which set its own.
        if self.picking.borrow().is_some()
            || self.tools.gesture.borrow().is_some()
            || self.tools.space_down.get()
        {
            return;
        }
        let Some(scene) = self.canvas.scene() else { return };
        let at = self.canvas.to_document(x, y);
        let tool = self.tools.active.get();
        let hover = match self.grab_handle(&scene, at) {
            Some(Gesture::Resizing { handle, .. }) => Hover::Resize(handle),
            // A line's end or an arrow's curve handle: it goes anywhere, so no direction
            // to point at.
            Some(_) => Hover::Grip,
            None => {
                let under = if tool.is_freehand() { None } else { self.object_at(&scene, at) };
                match under {
                    Some(id) if self.canvas.selection().contains(&id) => Hover::Selected,
                    Some(_) => Hover::Selectable,
                    None => match tool {
                        Tool::Select | Tool::Crop => Hover::Idle,
                        Tool::Text => Hover::Text,
                        drawing => Hover::Draw(drawing),
                    },
                }
            }
        };
        self.set_hover(hover);
    }

    /// Puts the cursor for a [`Hover`] on the canvas.
    pub(super) fn set_hover(&self, hover: Hover) {
        self.canvas.set_cursor(self.cursors.for_hover(hover).as_ref());
    }

    /// `spec/05` §2: "Esc also deselects".
    pub(super) fn deselect(self: &Rc<Self>) {
        self.canvas.set_selection(Vec::new());
    }

    /// Ctrl+A: every object that has handles (`spec/13` #3). The background and the crop
    /// *are* the canvas -- `Grips::None` -- and selecting them would offer nothing to move.
    pub(super) fn select_all(self: &Rc<Self>) {
        let Some(scene) = self.canvas.scene() else { return };
        let all: Vec<ObjectId> = scene
            .objects()
            .iter()
            .filter(|o| o.grips() != Grips::None)
            .map(|o| o.id)
            .collect();
        tracing::info!(selected = all.len(), "select all");
        self.canvas.set_selection(all);
    }
}

fn objects_of(scene: &Scene, ids: &[ObjectId]) -> Vec<Object> {
    ids.iter().filter_map(|id| scene.get(*id).cloned()).collect()
}

/// A draft's own starting point, so an update can be absolute.
fn draft_origin(draft: &Draft) -> Point {
    draft.start()
}

fn modifiers_of(gesture: &gtk::GestureDrag) -> Modifiers {
    let state = gesture.current_event_state();
    Modifiers {
        shift: state.contains(gtk::gdk::ModifierType::SHIFT_MASK),
        alt: state.contains(gtk::gdk::ModifierType::ALT_MASK),
        ctrl: state.contains(gtk::gdk::ModifierType::CONTROL_MASK),
    }
}

/// `spec/05` §5.1's `created`, in seconds since the epoch.
pub(super) fn now_seconds() -> u64 {
    glib::real_time().unsigned_abs() / 1_000_000
}

/// The accelerator as a person would read it in a tooltip.
fn accelerator_label(tool: Tool) -> String {
    let accel = tool.accelerator();
    match accel.strip_prefix("<Shift>") {
        Some(letter) => format!("Shift+{}", letter.to_uppercase()),
        None => accel.to_uppercase(),
    }
}
