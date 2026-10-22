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
//! the time and must start wherever the pen comes down -- except on their own strokes: a
//! pencil takes a pencil line and a highlighter a highlight, as every other tool takes its
//! own marks (D198). Without that, a stroke under the pointer showed the pencil's cursor,
//! inviting a new line where there already was one.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};
use octosnap_scene::tool::{Draft, Modifiers, Tool, ToolSettings};
use octosnap_scene::{
    Bounds, Command, Grips, Handle, Object, ObjectId, Point, Scene, callout, counters, handle, hit,
};
use tracing::debug;

use super::canvas::GripLook;
use super::cursor::Hover;
use super::window::Editor;

/// How far a press may travel and still be a click, in **widget** pixels -- GTK's own
/// `gtk-dnd-drag-threshold`, the distance every drag on this desktop already has to cross
/// before it is one.
pub(super) const CLICK_SLOP: f64 = 8.0;

/// `spec/05` §4.1: "Ctrl+D duplicates offset by 12 units".
const DUPLICATE_OFFSET: f64 = 12.0;

/// `ANN-28` (D177): how long the pencil has to stay still, with the button down, before the
/// stroke is asked what shape it was meant to be. Long enough that the pause at a corner
/// is not one; short enough to be a deliberate wait rather than a test of patience.
const HOLD: std::time::Duration = std::time::Duration::from_millis(500);

/// How far the pointer may drift, in widget pixels, and still be held still: a hand on a
/// mouse or a pen is never quite motionless.
const HOLD_SLOP: f64 = 4.0;

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
    /// `ANN-27`'s live text: a drag selects the picture's words, from the one the press
    /// was on or nearest to (D180). `press` and `last` are document points: the release
    /// is a click, which opens a link, if it came up within [`CLICK_SLOP`] of the press.
    Reading { from: octosnap_ocr::live::Pos, press: Point, last: Point },
    /// `ANN-31`'s Cut Out: a band of the picture, from the press to the pointer (D183).
    Slicing { from: Point, to: Point },
}

impl Gesture {
    /// What the debug log calls it.
    const fn name(&self) -> &'static str {
        match self {
            Self::Creating(_) => "creating",
            Self::Moving { .. } => "moving",
            Self::Marquee { .. } => "marquee",
            Self::Resizing { .. } => "resizing",
            Self::Ending { .. } => "ending",
            Self::Panning { .. } => "panning",
            Self::Reading { .. } => "reading",
            Self::Slicing { .. } => "slicing",
        }
    }
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
    /// The press on empty space began with something selected, and has not yet moved
    /// past a click's travel: if it comes up a click, it only deselects (D197).
    click_deselects: Cell<bool>,
    /// `ANN-28`: the wait for the pencil to be held still, and where it started.
    hold: RefCell<Option<glib::SourceId>>,
    hold_from: Cell<Option<Point>>,
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
        // A tool chosen is the end of live text, which is a tool of its own (D180).
        self.leave_live_text();
        // And the end of a label being typed: the label is drawn by the canvas as the
        // draft (D197), which the next lines clear.
        if self.text_editing() {
            self.commit_text_edit();
        }
        let was_cropping = self.cropping();
        self.stop_holding();
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
                // Crop mode owns the canvas's presses, a double-click's too: one inside the
                // frame opened the label under it for typing, with the frame still up (D205).
                if editor.cropping() {
                    return;
                }
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
                editor.begin_gesture(x, y, modifiers_of(gesture), pressure_of(gesture));
            });
        }
        {
            let editor = Rc::downgrade(self);
            drag.connect_drag_update(move |gesture, ox, oy| {
                let Some(editor) = editor.upgrade() else { return };
                editor.update_gesture(ox, oy, modifiers_of(gesture), pressure_of(gesture));
            });
        }
        {
            let editor = Rc::downgrade(self);
            drag.connect_drag_end(move |gesture, ox, oy| {
                let Some(editor) = editor.upgrade() else { return };
                // `ANN-34`: a drag ends when its finger lifts, and also when a second
                // finger comes down, which makes it a pinch's (`Editor::wire_zoom`). The
                // second is no end the user asked for: ended, the stroke the first finger
                // had begun would be committed under the pinch, so it is given up.
                if ended_by_a_second_finger(gesture) {
                    editor.cancel_gesture();
                    return;
                }
                // The release is the pen's tip leaving the surface, a pressure of nothing,
                // whatever the event carries (`Draft::feel`).
                editor.update_gesture(ox, oy, modifiers_of(gesture), Some(0.0));
                editor.end_gesture();
            });
        }
        // And the pinch's claim, or the compositor, can cancel the drag outright.
        {
            let editor = Rc::downgrade(self);
            drag.connect_cancel(move |_, _| {
                let Some(editor) = editor.upgrade() else { return };
                editor.cancel_gesture();
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
        {
            // The pointer gone, the outline of what it was over goes with it (D197).
            let editor = Rc::downgrade(self);
            motion.connect_leave(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.canvas.set_hovered(None);
                }
            });
        }
        self.canvas.add_controller(motion);
    }

    fn begin_gesture(self: &Rc<Self>, x: f64, y: f64, modifiers: Modifiers, pressure: Option<f64>) {
        // A press takes the object it was over, or does something else: either way the
        // hover's outline has said what it had to.
        self.canvas.set_hovered(None);
        let at = self.canvas.to_document(x, y);
        // `spec/05` §2's pipette, if it is armed, takes the click before any tool sees it.
        if self.take_sample(at) {
            return;
        }
        // A click anywhere while a text box is open ends the edit, and does nothing else.
        // Standard behaviour, and the alternative -- committing *and* starting a shape --
        // makes it impossible to simply finish typing. Nothing is left selected, so the
        // next click places the next label (D197).
        if self.text_editing() {
            self.finish_text_edit(false);
            return;
        }
        // Space held: the drag pans, whatever tool is lit and whether or not a crop frame
        // is up -- panning is how you reach the part of a large picture you want to crop.
        if self.tools.space_down.get() {
            self.set_hover(Hover::Panning);
            *self.tools.gesture.borrow_mut() = Some(Gesture::Panning { last: (0.0, 0.0) });
            return;
        }
        // `ANN-27`'s live text owns the picture while it is on: a press selects words,
        // and nothing is drawn, moved or cropped (D180).
        if self.live_text_on() {
            if let Some(from) = self.live_press(at) {
                *self.tools.gesture.borrow_mut() =
                    Some(Gesture::Reading { from, press: at, last: at });
            }
            return;
        }
        // `spec/05` §4.11's crop mode owns every pointer gesture on the canvas: there is
        // nothing else to click on while a crop frame is up. It takes the widget's point,
        // since its rect is the canvas's and `at` is the picture's (D205).
        if self.crop_press(x, y) {
            return;
        }
        let tool = self.tools.active.get();
        // `ANN-31`'s Cut Out: the drag is a band of the picture, whatever is on it.
        if tool == Tool::CutOut {
            *self.tools.gesture.borrow_mut() = Some(Gesture::Slicing { from: at, to: at });
            return;
        }
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

        // What is under the pointer, or for a freehand tool its own stroke -- see the
        // note at the top of this file.
        let under = self.object_for(tool, &scene, at);

        // The text tool on a label: a drag moves it and a click edits it. Which of the two
        // a press is going to be is only known when the button comes up, so the press
        // starts the move like any other tool's and remembers what a click would mean;
        // `end_gesture` decides by how far the pointer travelled. The first version opened
        // the edit on the press -- "one click, not two" -- and a label could then never be
        // moved with the tool that made it, because every press on it was an edit (D63).
        // `spec/05` §4.1's double-click under the Select tool works as it did.
        //
        // And only a label already selected: the first click on one selects it, with its
        // handles, and the second edits it, as a file's name is renamed in a file manager.
        // D56's "edits in one click" left no way to take a label with the tool that made
        // it and resize or restyle it without typing into it (D197).
        let selected = self.canvas.selection();
        let click_edits = (tool == Tool::Text && !modifiers.alt && !modifiers.shift)
            .then(|| under.and_then(|id| scene.get(id)))
            .flatten()
            .filter(|object| matches!(object.geometry, octosnap_scene::Geometry::Text { .. }))
            .filter(|object| selected == [object.id])
            .map(|object| object.id);

        if under.is_none() && tool.creates() {
            // A click off the selection leaves it, whatever tool is lit, and does nothing
            // else; a drag off it still draws. The owner's report was that a click away
            // from a box kept the box selected: the click began a draft too small to keep,
            // and nothing deselected. A badge and a lens are placed *by* a click, so a
            // row of numbered steps stays one click each (D197).
            self.tools.click_deselects.set(
                !self.canvas.selection().is_empty()
                    && !matches!(tool, Tool::Counter | Tool::Magnifier),
            );
            self.stop_holding();
            let draft = Draft::begin(tool, self.settings(), at, modifiers).map(|mut draft| {
                draft.feel(pressure);
                draft
            });
            self.preview_draft(draft.as_ref());
            *self.tools.gesture.borrow_mut() = draft.map(Gesture::Creating);
            return;
        }

        match under {
            // `ANN-32`: a press on a magnifier's spot, off its lens, takes the spot -- what
            // the lens enlarges, which a move of the lens leaves where it is.
            Some(id) if !modifiers.alt && !modifiers.shift && self.on_spot(&scene, id, at) => {
                if let Some(object) = scene.get(id) {
                    self.canvas.set_selection(vec![id]);
                    *self.tools.gesture.borrow_mut() =
                        Some(Gesture::Ending { before: object.clone(), index: 0 });
                }
            }
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
                self.set_hover(Hover::Moving);
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

    fn update_gesture(
        self: &Rc<Self>,
        ox: f64,
        oy: f64,
        modifiers: Modifiers,
        pressure: Option<f64>,
    ) {
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
                if ox.hypot(oy) > CLICK_SLOP {
                    self.tools.click_deselects.set(false);
                }
                let from = draft_origin(draft);
                let at = from.offset(dx, dy);
                draft.extend(at, modifiers);
                draft.feel(pressure);
                let draft = draft.clone();
                drop(gesture);
                self.preview_draft(Some(&draft));
                if draft.tool() == Tool::Pencil && draft.held().is_none() {
                    self.wait_for_hold(at, zoom);
                }
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
            Some(Gesture::Reading { from, press, last }) => {
                let at = press.offset(dx, dy);
                *last = at;
                let (from, press) = (*from, *press);
                drop(gesture);
                self.live_drag(from, press, at);
            }
            Some(Gesture::Slicing { from, to }) => {
                *to = from.offset(dx, dy);
                let (from, to) = (*from, *to);
                drop(gesture);
                self.preview_slice(from, to);
            }
            // Handled above, before the crop check.
            Some(Gesture::Panning { .. }) | None => {}
        }
    }

    /// The gesture in progress given up, as if it had not begun: a pinch that takes the
    /// fingers cancels the drag the first one started (`ANN-34`). What a move, a resize or
    /// an end's drag changed is put back; a stroke, a band or a rubber band goes; a crop
    /// frame returns to where the press found it.
    pub(super) fn cancel_gesture(self: &Rc<Self>) {
        self.stop_holding();
        if self.cropping() {
            self.crop_cancel_drag();
        }
        let Some(gesture) = self.tools.gesture.borrow_mut().take() else { return };
        let restore = |before: &[Object]| {
            let Some(mut scene) = self.canvas.scene() else { return };
            for was in before {
                scene.replace(was.clone());
            }
            self.canvas.update_scene(scene);
        };
        match &gesture {
            Gesture::Creating(_) => self.canvas.set_draft(None),
            Gesture::Moving { before, .. } => restore(before),
            Gesture::Resizing { before, .. } | Gesture::Ending { before, .. } => {
                restore(std::slice::from_ref(before));
            }
            Gesture::Marquee { .. } => self.canvas.set_marquee(None),
            Gesture::Slicing { .. } => self.canvas.set_slice(None),
            Gesture::Panning { .. } | Gesture::Reading { .. } => {}
        }
        self.set_hover(if self.tools.space_down.get() { Hover::Pan } else { Hover::Idle });
        debug!(gesture = gesture.name(), "gesture cancelled");
    }

    fn end_gesture(self: &Rc<Self>) {
        self.stop_holding();
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
                if self.tools.click_deselects.replace(false) {
                    self.canvas.set_selection(Vec::new());
                    return;
                }
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
                // Put down, and still under the pointer: the open hand again.
                self.set_hover(Hover::Selected);
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
            Gesture::Reading { press, last, .. } => self.live_release(press, last),
            Gesture::Slicing { from, to } => self.end_slice(from, to),
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

    /// `ANN-28` (D177): starts the wait for the pencil to be held still at `at`, unless it
    /// is already waiting there. Every move past [`HOLD_SLOP`] starts it again, so it ends
    /// only when the pointer has stopped.
    fn wait_for_hold(self: &Rc<Self>, at: Point, zoom: f64) {
        let still =
            self.tools.hold_from.get().is_some_and(|from| from.distance_to(at) * zoom <= HOLD_SLOP);
        if still && self.tools.hold.borrow().is_some() {
            return;
        }
        self.stop_holding();
        self.tools.hold_from.set(Some(at));
        let editor = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(HOLD, move || {
            let Some(editor) = editor.upgrade() else { return };
            // Spent, so it is forgotten rather than removed.
            editor.tools.hold.borrow_mut().take();
            editor.held();
        });
        *self.tools.hold.borrow_mut() = Some(id);
    }

    fn stop_holding(&self) {
        if let Some(id) = self.tools.hold.borrow_mut().take() {
            id.remove();
        }
        self.tools.hold_from.set(None);
    }

    /// The pencil has been held still: the stroke becomes the shape it was meant to be, if
    /// it was meant to be one, and the preview shows it while the button is still down.
    fn held(self: &Rc<Self>) {
        let mut gesture = self.tools.gesture.borrow_mut();
        let Some(Gesture::Creating(draft)) = gesture.as_mut() else { return };
        let Some(shape) = draft.hold() else { return };
        let draft = draft.clone();
        drop(gesture);
        // Read by `editor-test.sh`, which cannot see a stroke turn into a shape.
        tracing::info!(shape = shape.name(), "a held pencil stroke became a shape");
        self.preview_draft(Some(&draft));
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
        let Some(band) = octosnap_scene::highlight::snap(&pixels, per_unit, &stored, reach) else {
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
            pressure: None,
        };
    }

    /// The topmost object under `at`, with a click target that is the same size on
    /// screen at every zoom -- and, for text, the box the glyphs actually occupy rather
    /// than the scene's 0.6-em estimate of it (D57: the hit box is what is drawn).
    /// What a press with `tool` takes at `at`: the object there, or for the pencil and
    /// the highlighter only a stroke of their own, so that they draw over everything else
    /// (D198).
    fn object_for(&self, tool: Tool, scene: &Scene, at: Point) -> Option<ObjectId> {
        let id = self.object_at(scene, at)?;
        if !tool.is_freehand() {
            return Some(id);
        }
        let own = matches!(
            scene.get(id)?.geometry,
            octosnap_scene::Geometry::Path { highlighter, .. }
                if highlighter == (tool == Tool::Highlighter)
        );
        own.then_some(id)
    }

    pub(super) fn object_at(&self, scene: &Scene, at: Point) -> Option<ObjectId> {
        let tolerance = (STROKE_GRAB / self.canvas.zoom()).max(hit::TOLERANCE);
        let canvas = self.canvas.clone();
        hit::topmost_measured(scene, at, tolerance, &|object| {
            matches!(object.geometry, octosnap_scene::Geometry::Text { .. })
                .then(|| canvas.measured_bounds(object))
        })
    }

    /// [`Self::object_at`], locked objects included: what a right-click is on (D162).
    pub(super) fn object_at_any(&self, scene: &Scene, at: Point) -> Option<ObjectId> {
        let tolerance = (STROKE_GRAB / self.canvas.zoom()).max(hit::TOLERANCE);
        let canvas = self.canvas.clone();
        hit::topmost_measured_any(scene, at, tolerance, &|object| {
            matches!(object.geometry, octosnap_scene::Geometry::Text { .. })
                .then(|| canvas.measured_bounds(object))
        })
    }

    /// Whether `at` is on the spot of the magnifier `id` rather than on its lens (`ANN-32`).
    fn on_spot(&self, scene: &Scene, id: ObjectId, at: Point) -> bool {
        let tolerance = (STROKE_GRAB / self.canvas.zoom()).max(hit::TOLERANCE);
        scene.get(id).is_some_and(|object| match object.geometry {
            octosnap_scene::Geometry::Magnifier { center, radius, source, zoom } => {
                let reach = object.style.stroke_width() / 2.0 + tolerance;
                octosnap_scene::magnifier::on_spot(center, radius, source, zoom, at, reach)
            }
            _ => false,
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
        let looks = self.canvas.grip_looks(object);
        let index = grips
            .iter()
            .enumerate()
            // What is not drawn is not there to grab (D197).
            .filter(|(index, _)| looks.get(*index) != Some(&GripLook::Hidden))
            .filter(|(_, point)| point.distance_to(at) <= tolerance)
            .min_by(|(_, a), (_, b)| a.distance_to(at).total_cmp(&b.distance_to(at)))
            .map(|(index, _)| index)?;
        match object.grips() {
            // `ANN-29`'s ninth handle is the tail's tip, dragged as a line's end is.
            Grips::BoxAndTip if index == Handle::ALL.len() => {
                Some(Gesture::Ending { before: object.clone(), index: 0 })
            }
            Grips::Box | Grips::BoxAndTip => Some(Gesture::Resizing {
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

    /// `spec/05` §4.1: "Delete/Backspace removes". The badges left behind close the gap,
    /// in the same step (`ANN-10`, D182).
    pub(super) fn delete_selection(self: &Rc<Self>) {
        let Some(scene) = self.canvas.scene() else { return };
        let leaving = self.canvas.selection();
        let mut commands: Vec<Command> =
            leaving.iter().filter_map(|id| scene.get(*id).cloned().map(Command::Remove)).collect();
        if commands.is_empty() {
            return;
        }
        commands.extend(renumbering(&scene, counters::closed_up(&scene, &leaving)));
        self.canvas.set_selection(Vec::new());
        self.commit_batch(commands);
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
        let mut pasted: Vec<Object> = OBJECTS
            .with_borrow_mut(|clipboard| clipboard.paste(&scene))
            .into_iter()
            .filter(|object| match &object.geometry {
                octosnap_scene::Geometry::Image { file, .. } => {
                    std::path::Path::new(file).is_file()
                }
                _ => true,
            })
            .collect();
        if pasted.is_empty() {
            return false;
        }
        // A pasted badge is another badge: it takes the next number (`ANN-10`).
        counters::appended(&scene, &mut pasted);
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
        let mut copies = Vec::new();
        for id in ids {
            let Some(original) = scene.get(*id) else { continue };
            let mut copy = original.clone();
            // A new identity, not a copy of one: `spec/05` §5.1's id is a ULID and two
            // objects sharing one would make `Scene::get` answer either of them.
            copy.id = ObjectId::new();
            copy.translate(offset, offset);
            z = z.saturating_add(1);
            copy.z = z;
            copies.push(copy);
        }
        // A copy of a badge is another badge: it takes the next number (`ANN-10`).
        counters::appended(&scene, &mut copies);
        let new_ids = copies.iter().map(|copy| copy.id).collect();
        self.commit_batch(copies.into_iter().map(Command::Add).collect());
        new_ids
    }

    /// `spec/05` §4.1: "double-click a counter edits its number".
    ///
    /// A popover on the badge with one spin button. Enter commits and closes; closing by
    /// any other route commits whatever the field holds, because a value typed and then
    /// clicked away from is a value the user chose. The badge moves to that number's place
    /// in the sequence and the others make room (`ANN-10`, D182).
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
        let host: gtk::Widget =
            self.canvas.parent().unwrap_or_else(|| self.canvas.clone().upcast());
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

    /// One badge's number, as one undo step: it moves to that place in the sequence, and
    /// the badges between make room (`counters::moved`).
    fn renumber(self: &Rc<Self>, before: &Object, number: u32) {
        let octosnap_scene::Geometry::Counter { number: was, .. } = before.geometry else { return };
        if was == number {
            return;
        }
        let Some(scene) = self.canvas.scene() else { return };
        let changes = renumbering(&scene, counters::moved(&scene, before.id, number));
        let badges = changes.len();
        self.commit_batch(changes);
        tracing::info!(from = was, to = number, badges, "counter renumbered");
    }

    /// `ANN-29` (D182): Add Pointer gives the one selected counter or text a tail, down and
    /// away from it, and Remove Pointer takes it away. One step either way; the tip is then
    /// the selection's ninth handle.
    pub(super) fn toggle_pointer(self: &Rc<Self>) {
        let Some(scene) = self.canvas.scene() else { return };
        let [id] = self.canvas.selection()[..] else { return };
        let Some(before) = scene.get(id).cloned() else { return };
        let tip = match before.tail() {
            Some(_) => None,
            None => Some(callout::default_tip(self.canvas.measured_bounds(&before), scene.canvas)),
        };
        let mut after = before.clone();
        if !after.set_tail(tip) {
            return;
        }
        self.commit(Command::Change { id, before: Box::new(before), after: Box::new(after) });
        tracing::info!(pointer = tip.is_some(), "callout pointer");
        self.canvas.report_selection();
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

    /// D162's lock, from the canvas menu: the selection stays where it is, and a left
    /// click passes through it to whatever is behind -- it is neither selected, moved,
    /// resized nor swept up by a band or by Ctrl+A -- until a right-click unlocks it.
    ///
    /// One undo step, like any other edit, and saved with the project.
    pub(super) fn lock_selection(self: &Rc<Self>) {
        let ids = self.canvas.selection();
        let count = self.set_locked(&ids, true);
        if count == 0 {
            return;
        }
        self.canvas.set_selection(Vec::new());
        tracing::info!(count, "locked");
        let how = if count == 1 {
            "Locked · Right-click it to unlock"
        } else {
            "Locked · Right-click one to unlock it"
        };
        self.toast(how, None);
    }

    /// Unlocks `ids` and selects them, so what comes back is plainly editable again.
    pub(super) fn unlock(self: &Rc<Self>, ids: &[ObjectId]) {
        let count = self.set_locked(ids, false);
        if count == 0 {
            return;
        }
        let Some(scene) = self.canvas.scene() else { return };
        let live: Vec<ObjectId> =
            ids.iter().copied().filter(|id| scene.get(*id).is_some()).collect();
        self.canvas.set_selection(live);
        tracing::info!(count, "unlocked");
    }

    /// Every locked object, unlocked: the keyboard's way back, since a locked object can
    /// never be the selection Shift+F10 opens its menu on.
    pub(super) fn unlock_all(self: &Rc<Self>) {
        let ids = self.locked_objects();
        self.unlock(&ids);
    }

    /// The locked objects in the document, in its order.
    pub(super) fn locked_objects(&self) -> Vec<ObjectId> {
        self.canvas.scene().map_or_else(Vec::new, |scene| {
            scene.objects().iter().filter(|o| o.locked).map(|o| o.id).collect()
        })
    }

    /// Sets `locked` on every one of `ids` that has handles and is not already so, as one
    /// step. Answers how many changed.
    pub(super) fn set_locked(&self, ids: &[ObjectId], locked: bool) -> usize {
        let Some(scene) = self.canvas.scene() else { return 0 };
        let changes: Vec<Command> = ids
            .iter()
            .filter_map(|id| {
                let was = scene.get(*id)?;
                if was.locked == locked || was.grips() == Grips::None {
                    return None;
                }
                let mut now = was.clone();
                now.locked = locked;
                Some(Command::Change {
                    id: *id,
                    before: Box::new(was.clone()),
                    after: Box::new(now),
                })
            })
            .collect();
        let count = changes.len();
        self.commit_batch(changes);
        count
    }

    /// The selection without what an undo or redo removed, locked or hid: a locked object
    /// is never the selection, or a key could move what a click cannot (D162), and a hidden
    /// one would be handles around nothing (`ANN-30`).
    fn pruned_selection(&self, scene: &Scene) -> Vec<ObjectId> {
        self.canvas
            .selection()
            .into_iter()
            .filter(|id| scene.get(*id).is_some_and(|o| !o.locked && o.shown()))
            .collect()
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
            // A selection can name an object the undo has just removed, or locked again.
            // Dropping only those rather than clearing everything keeps a multi-select
            // intact through an undo of one member.
            let live = self.pruned_selection(&scene);
            self.canvas.update_scene(scene);
            self.canvas.set_selection(live);
            self.after_edit();
        }
    }

    pub(super) fn redo(self: &Rc<Self>) {
        let Some(mut scene) = self.canvas.scene() else { return };
        if self.history.borrow_mut().redo(&mut scene) {
            let live = self.pruned_selection(&scene);
            self.canvas.update_scene(scene);
            self.canvas.set_selection(live);
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
        // The stored number is for the badges placed when there are none, a preference
        // and not an edit. The badges there are move with it as one step (`ANN-10`), so
        // Ctrl+Z puts their numbers back.
        scene.set_next_number(number);
        let changes = renumbering(&scene, counters::started_at(&scene, number));
        self.canvas.update_scene(scene);
        self.commit_batch(changes);
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
        if let Some(hover) = self.live_hover(at) {
            self.canvas.set_hovered(None);
            self.set_hover(hover);
            return;
        }
        let tool = self.tools.active.get();
        // Cut Out cuts whatever is under it, so nothing here is grabbed or selected.
        if tool == Tool::CutOut {
            self.canvas.set_hovered(None);
            self.set_hover(Hover::Draw(tool));
            return;
        }
        let mut hovered = None;
        let hover = match self.grab_handle(&scene, at) {
            Some(Gesture::Resizing { handle, .. }) => Hover::Resize(handle),
            // A line's end or an arrow's curve handle: it goes anywhere, so no direction
            // to point at.
            Some(_) => Hover::Grip,
            None => {
                let under = self.object_for(tool, &scene, at);
                match under {
                    Some(id) if self.canvas.selection().contains(&id) => Hover::Selected,
                    Some(id) => {
                        hovered = Some(id);
                        Hover::Selectable
                    }
                    None => match tool {
                        Tool::Select | Tool::Crop => Hover::Idle,
                        Tool::Text => Hover::Text,
                        drawing => Hover::Draw(drawing),
                    },
                }
            }
        };
        self.canvas.set_hovered(hovered);
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
    /// Not a hidden mark (`ANN-30`): a Delete after Ctrl+A takes what can be seen.
    pub(super) fn select_all(self: &Rc<Self>) {
        if self.live_select_all() {
            return;
        }
        let Some(scene) = self.canvas.scene() else { return };
        let all: Vec<ObjectId> = scene
            .objects()
            .iter()
            .filter(|o| o.grips() != Grips::None && !o.locked && o.shown())
            .map(|o| o.id)
            .collect();
        tracing::info!(selected = all.len(), "select all");
        self.canvas.set_selection(all);
    }
}

/// `ANN-10`'s renumbered badges as changes, from what each badge is now to what it
/// becomes. Logged by how many, which `editor-test.sh` reads.
fn renumbering(scene: &Scene, badges: Vec<Object>) -> Vec<Command> {
    let changes: Vec<Command> = badges
        .into_iter()
        .filter_map(|after| {
            let before = scene.get(after.id)?.clone();
            Some(Command::Change { id: after.id, before: Box::new(before), after: Box::new(after) })
        })
        .collect();
    if !changes.is_empty() {
        tracing::info!(badges = changes.len(), "badges renumbered");
    }
    changes
}

fn objects_of(scene: &Scene, ids: &[ObjectId]) -> Vec<Object> {
    ids.iter().filter_map(|id| scene.get(*id).cloned()).collect()
}

/// A draft's own starting point, so an update can be absolute.
fn draft_origin(draft: &Draft) -> Point {
    draft.start()
}

/// `ANN-34` (D185): the pen's pressure on the gesture's event, 0 to 1. A mouse, a finger
/// and a pen's eraser end have none, and a pen without the axis has none to give.
fn pressure_of(gesture: &gtk::GestureDrag) -> Option<f64> {
    let event = gesture.current_event()?;
    let tool = event.device_tool()?;
    matches!(
        tool.tool_type(),
        gdk::DeviceToolType::Pen
            | gdk::DeviceToolType::Pencil
            | gdk::DeviceToolType::Brush
            | gdk::DeviceToolType::Airbrush
    )
    .then(|| event.axis(gdk::AxisUse::Pressure))
    .flatten()
}

/// Whether the event a drag ended on is another finger touching down, or the touch being
/// cancelled, rather than the drag's own finger or button coming up (`ANN-34`).
fn ended_by_a_second_finger(gesture: &gtk::GestureDrag) -> bool {
    gesture.current_event().is_some_and(|event| {
        matches!(event.event_type(), gdk::EventType::TouchBegin | gdk::EventType::TouchCancel)
    })
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
