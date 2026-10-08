// SPDX-License-Identifier: GPL-3.0-or-later

//! `ANN-01`: the editor window (`spec/05` §1).
//!
//! > Resizable window; tools at top; actions at bottom; draggable from the bottom bar;
//! > optional always-on-top; opens in ~instantly; dark/light.
//!
//! "Opens in ~instantly" is the constraint that shapes this file. The window is built and
//! shown before anything is decoded: `Canvas::set_scene` uploads the texture, and a 5K
//! PNG takes long enough that doing it first would show the user nothing for a beat. So
//! the order is window, then document -- the same order the capture overlay uses, and the
//! same reason.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use octosnap_core::CaptureResult;
use octosnap_scene::tool::Tool;
use octosnap_scene::{Base, History, Scene};
use tracing::info;

use super::Canvas;
use super::actions::EditorActions;
use super::tools::ToolState;

/// `spec/05` §9's zoom step. A ratio rather than an increment, so each press moves the
/// same visual amount at every zoom -- an additive step is imperceptible at 800 % and a
/// jump at 10 %.
const ZOOM_STEP: f64 = 1.25;

/// How far one wheel click pans, in widget pixels. About a line of the toolbar.
const WHEEL_PAN: f64 = 48.0;

/// How many touchpad pixels of Ctrl+scroll make one `ZOOM_STEP`.
const SMOOTH_ZOOM_PIXELS: f64 = 60.0;

/// The narrowest the editor's own half can be: D57's tool strip beside the window
/// buttons, which is the one thing in the toolbar that cannot wrap.
///
/// Measured rather than chosen -- 671 px with Ubuntu Sans 11 -- and rounded up for the
/// fonts that are wider. D57 wrote 660 here, which was eleven px short of what the
/// toolbar had grown to need, and eleven px is enough: a window at its own minimum
/// over-allocated its content and GTK pushed the last thing in the title bar past the
/// right-hand edge. Still under D57's "tiled to half of a 1366 px screen is 683, and
/// that has to work" (D100).
const MIN_CANVAS: i32 = 680;

/// The shortcuts a focused text field has its own meaning for, which therefore yield to
/// it while the user is typing (`spec/13` #3).
const TEXT_FIELD_KEYS: [&str; 16] = [
    "<Control>a",
    "<Control>c",
    "<Control>x",
    "<Control>v",
    "<Control>z",
    "<Control><Shift>z",
    "<Control>y",
    "<Control>d",
    "Delete",
    "BackSpace",
    "Left",
    "Right",
    "Up",
    "Down",
    "<Shift>Left",
    "<Control>bracketright",
];

/// Shift held at the capture's confirm, in `CaptureResult::modifiers`: `CAP-15`, and
/// `spec/05` §4.13's "skippable by holding Shift at capture".
pub(super) const SHIFT_MASK: u32 = 1 << 0;

/// How the window is closing, which decides what it leaves behind (D108).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Closing {
    /// The title bar's ×, Ctrl+W: back to the stack as a card.
    ToPreview,
    /// Final Close: into the history, and no card.
    Final,
    /// Pin took the render: the pin holds the picture, so only the capture is left to
    /// file.
    Pinned,
}

/// One open editor.
pub struct Editor {
    pub(super) window: adw::ApplicationWindow,
    pub(super) canvas: Canvas,
    /// `spec/05` §5.2's undo stack. Held beside the canvas rather than inside it because
    /// a history without a scene is meaningless and a scene without a history is a
    /// perfectly good export -- the canvas owns the document, this owns the edits to it.
    pub(super) history: RefCell<History>,
    /// `spec/05` §2's tool strip: which tool is active, what the options row has it set
    /// to, and the gesture in flight. In its own struct because `tools.rs` owns every
    /// rule about it and this file only has to hold it.
    pub(super) tools: ToolState,
    /// `spec/05` §2's options row, held so a tool change can rebuild it.
    pub(super) options: super::options::OptionsRow,
    /// `spec/05` §4.11's crop mode: the rect, the controls, and the toolbar that replaces
    /// the toolbar. Outside crop mode every field is empty and the document is untouched,
    /// which is what makes "Esc cancels" cost nothing.
    pub(super) crop: super::crop::CropState,
    /// The application, for the notifications a failed action sends.
    pub(super) app: adw::Application,
    /// The capture this editor was opened on. Every export is described as a variant of
    /// it, so the flow's clipboard route and filename template apply unchanged.
    pub(super) capture: CaptureResult,
    /// What the bottom bar asks of the capture flow (`spec/05` §7).
    pub(super) actions: EditorActions,
    /// The document at the last Copy, Save or Pin, and at the last Save with the file it
    /// wrote (D168).
    ///
    /// The unsaved dot's question is not "has anything changed" but "has what changed
    /// left the editor" (`spec/13` #11), and a card left behind for exactly the saved
    /// document has its picture on disk already. Documents and not undo depths, which an
    /// undo and a new mark bring back to the same number with different marks.
    pub(super) saved: RefCell<super::saved::Saved>,
    /// The document as the last Save as Project wrote it, or as one was opened, and the
    /// `.octosnap` file: a render of exactly that document is the project's picture, and
    /// carries it to its card and the history (D167). The document and not the undo depth,
    /// which an undo and a new mark bring back to the same number with different marks.
    pub(super) project: RefCell<Option<(Scene, std::path::PathBuf)>>,
    /// How the window is closing, set by whichever way out was taken (D108).
    pub(super) closing: Cell<Closing>,
    /// Whether [`Editor::hand_back`] has run: a close-request can be emitted twice.
    pub(super) handed_back: Cell<bool>,
    /// The document as a project file gave it (D108). A project closed with nothing
    /// changed hands nothing back, because the file already holds it. `None` for a
    /// capture, and for a document lent back by a card, which returns to its card however
    /// little was done to it.
    pub(super) opened_on: RefCell<Option<Scene>>,
    /// The picker waiting for §2's pipette to land, if one is armed.
    ///
    /// Weak, because the picker belongs to a popover that the row can rebuild away while
    /// the pointer is on its way to the canvas.
    pub(super) picking: RefCell<Option<std::rc::Weak<super::picker::Picker>>>,
    /// `spec/05` §4.5's editing overlay, and what is open in it.
    ///
    /// A `GtkFixed` above the canvas, and §6 asks for exactly that: "overlays a real
    /// `Gtk.TextView` scaled with a `Gtk.Fixed` + transform, so IME/emoji input works".
    /// Hidden when nothing is being typed -- a visible overlay child covers the canvas and
    /// eats every click, and `set_can_target(false)` would stop the `TextView` inside it
    /// receiving any (the trap D39 is about).
    pub(super) text_layer: gtk::Fixed,
    pub(super) editing: super::text::TextLayer,
    /// The pickers the current row owns.
    ///
    /// A `GtkMenuButton` owns its popover but a `Picker` is a Rust struct beside one, so
    /// something has to hold it. `g_object_set_data` would, and the workspace forbids
    /// `unsafe` -- which is the right call here rather than an obstacle: this vector is
    /// cleared whenever the row is rebuilt, so the lifetime is stated in safe code
    /// instead of hidden in a GObject's data table.
    pub(super) pickers: RefCell<Vec<Rc<super::picker::Picker>>>,
    /// The canvas cursors (D57), drawn once each.
    pub(super) cursors: super::cursor::Cursors,
    /// The stylesheet of the text edit that is open, if one is (`text::install_text_css`).
    pub(super) text_css: RefCell<Option<gtk::CssProvider>>,
    /// Where "Copied" and "Saved as …" appear (`spec/05` §10, `spec/13` #6).
    pub(super) toasts: adw::ToastOverlay,
    /// The dot beside the document size that says marks have not left the editor
    /// (`spec/13` #11).
    pub(super) unsaved: gtk::Label,
    /// The document's size beside the zoom, which the canvas decides rather than the
    /// capture: `spec/05` §4.11's crop and §4.13's padding both change it, and a readout
    /// that still said what the capture was would be wrong for the whole of both.
    pub(super) size: gtk::Label,
    /// `spec/05` §4.13's left sidebar and every control on it.
    pub(super) background: super::background::BackgroundPanel,
}

/// One line when an editor is really gone, which is the only evidence `spec/05` §11
/// item 10 -- "memory stays flat after opening/closing 20 editors (no texture leaks)" --
/// leaves behind. A closed window whose editor is still referenced holds its canvas, and
/// the canvas holds the capture's texture; nothing else in the process would notice.
impl Drop for Editor {
    fn drop(&mut self) {
        info!(file = %self.capture.path.display(), "editor dropped");
    }
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Editor")
            .field("zoom", &self.canvas.zoom())
            .field("undo_depth", &self.history.borrow().depth())
            .finish_non_exhaustive()
    }
}

/// `spec/10` §7's "Editor open with a 5K capture < 400 ms", said at the end of the first
/// frame the window paints. The document is set before that frame can run -- decoding the
/// base holds the main loop, window or no window -- so this is the frame with the capture
/// in it, counted from the moment the editor was asked for.
fn report_first_frame(window: &gtk::Widget, requested: std::time::Instant, size: (f64, f64)) {
    let window = window.clone();
    // After the present, from the idle loop: the window has its frame clock by then, and
    // its first frame has not been painted yet, because this callback is holding the
    // loop that would paint it.
    glib::idle_add_local_once(move || {
        let Some(clock) = window.frame_clock() else { return };
        let handler: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::new(RefCell::new(None));
        let id = {
            let handler = Rc::clone(&handler);
            clock.connect_after_paint(move |clock| {
                let ms = requested.elapsed().as_secs_f64() * 1000.0;
                info!(
                    ms = format!("{ms:.1}"),
                    width = size.0,
                    height = size.1,
                    budget_ms = 400,
                    "editor visible"
                );
                if let Some(id) = handler.borrow_mut().take() {
                    clock.disconnect(id);
                }
            })
        };
        *handler.borrow_mut() = Some(id);
    });
}

impl Editor {
    /// Opens a capture for annotation.
    pub fn open(
        app: &adw::Application,
        capture: &CaptureResult,
        actions: EditorActions,
    ) -> Rc<Self> {
        Self::open_with(app, capture, actions, None)
    }

    /// [`Self::open`], on a document that already exists.
    ///
    /// `spec/05` §8: "opening a project from a card or history restores full
    /// editability". A project arrives as a `Scene` that `octosnap_scene::project`
    /// already assembled -- objects, canvas rect and the extracted base image -- so the
    /// editor's job is to show it rather than to build one from the capture.
    ///
    /// The capture is still required, because everything the bottom bar does is described
    /// as a variant of one (`spec/05` §7's filename template, the clipboard route, the
    /// pin's position). For a project it describes the *extracted* base.
    pub fn open_with(
        app: &adw::Application,
        capture: &CaptureResult,
        actions: EditorActions,
        document: Option<Scene>,
    ) -> Rc<Self> {
        let requested = std::time::Instant::now();
        let canvas = Canvas::new();

        // No `ScrolledWindow` around the canvas, and that is a correction rather than a
        // simplification. It was there first, and it hands its child the child's *natural*
        // size rather than the viewport's -- so `Canvas::width()` answered the document's
        // width, `zoom_to_fit` divided the viewport by itself, and the first frame opened
        // at 1:1 with the bottom of the capture cut off. Visible in the first screenshot
        // of the editor and in nothing before it.
        //
        // The canvas owns its own viewport anyway: it has the zoom, the origin, and the
        // pan, so a scroller would be a second thing deciding what is visible. Expanding
        // into the window's space is what makes `width()` mean the viewport.
        canvas.set_hexpand(true);
        canvas.set_vexpand(true);
        // The surround is a flat theme colour, not a pattern. `spec/05` §3's checkerboard
        // is clipped to the canvas now (D52), and what is left outside it is a plain
        // desk -- painted by GTK from this class's CSS background before `snapshot` runs,
        // so it follows the light/dark theme without this file knowing which is on.
        canvas.add_css_class("octosnap-canvas");
        install_editor_css();

        // The canvas, with the text-editing layer above it.
        let text_layer = gtk::Fixed::new();
        text_layer.set_visible(false);
        let stage = gtk::Overlay::new();
        stage.set_child(Some(&canvas));
        stage.add_overlay(&text_layer);
        stage.set_hexpand(true);
        stage.set_vexpand(true);

        // `spec/05` §1, [P→V]: "The real editor uses **one toolbar row**, with the options
        // for the active tool inline on the right." So the tools live *in* the title bar
        // and there is no title -- the window's name is on the taskbar, and a row that
        // said "Annotate" above a row of tools above a row of options was 140 px of
        // chrome over the picture. The bar is `Raised` because the desk around the canvas
        // is the window's own background colour; a flat bar would have no edge.
        //
        // The title bar is the app's own `WindowHandle` + `WindowControls`, not an
        // `adw::HeaderBar`. A header bar hands a packed child its natural width and no
        // more, and the first version learnt what that means at 833 px: the tool strip
        // and the options row did not fit beside the window buttons, GTK either grew the
        // window to make room (entering crop mode widened it by 400 px) or, when the
        // window was tiled and could not grow, clipped the row -- and in crop mode the
        // clipped end is where Cancel and Crop live. Reported from hardware as "cannot
        // choose any tool at all" (D57). Here the toolbar stack takes the whole width
        // between the two sets of window buttons, and each of its pages is an
        // `adw::WrapBox`, so a row that does not fit wraps onto a second line instead.
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.add_css_class("toolbar");
        let controls_start = gtk::WindowControls::new(gtk::PackType::Start);
        let controls_end = gtk::WindowControls::new(gtk::PackType::End);
        for controls in [&controls_start, &controls_end] {
            // Stays on the first line when the toolbar wraps, and hides when the user's
            // button layout puts nothing on its side -- a header bar does the same.
            controls.set_valign(gtk::Align::Start);
            controls.set_visible(!controls.is_empty());
            controls.connect_empty_notify(|controls| controls.set_visible(!controls.is_empty()));
        }
        bar.append(&controls_start);
        let title_bar = gtk::WindowHandle::new();
        title_bar.set_child(Some(&bar));
        // Toasts float over the canvas, not over the bars: a "Saved as …" that covered
        // the bottom bar would cover the button that made it.
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stage));
        let view = adw::ToolbarView::new();
        view.set_top_bar_style(adw::ToolbarStyle::Raised);
        view.set_bottom_bar_style(adw::ToolbarStyle::Raised);
        view.add_top_bar(&title_bar);
        view.set_content(Some(&toasts));

        // `spec/05` §4.13, [P→V]: "It is a **left** sidebar occupying the full window
        // height, not a right-hand panel or a popover." Full height means *outside* the
        // toolbar view -- a sidebar inside it would start below the title bar -- so the
        // split is the window's content and the whole editor is its content pane. Hidden
        // until the Background button is pressed, which is §1's "No side panel is present
        // by default".
        let split = adw::OverlaySplitView::new();
        split.set_content(Some(&view));
        split.set_show_sidebar(false);
        split.set_sidebar_width_fraction(0.26);
        // The panel is exactly as wide as it says it is. A cap *below* the panel's own
        // minimum is not a cap at all -- Adwaita asks for the fraction, clamps it to the
        // maximum, and then GTK hands the child its minimum anyway -- so the two numbers
        // have to be the same one or the sidebar quietly takes more of the window than
        // anything here believes it does (D100).
        split.set_max_sidebar_width(f64::from(super::background::WIDTH));

        let unsaved = gtk::Label::new(Some("•"));
        unsaved.add_css_class("accent");
        unsaved.set_tooltip_text(Some(
            "Not copied, saved or pinned yet. Closing keeps them: × leaves a preview card, \
             Final Close files them in the history.",
        ));
        unsaved.set_visible(false);

        // §1: "Window is resizable and remembers its size. It opened at 1322 x 934 pt for a
        // 1710 x 1107 pt screen, so roughly 77 % of the screen width [V]."
        let (width, height, maximized) = window_size_for(capture);
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Annotate")
            .default_width(width)
            .default_height(height)
            .content(&split)
            .build();
        window.add_css_class("octosnap-editor");
        // Wide enough for the tool strip beside the window buttons, which is the one
        // thing in the toolbar that cannot wrap; everything else does (D57). Tiled to
        // half of a 1366 px screen is 683, and that has to work.
        window.set_size_request(MIN_CANVAS, 440);
        if maximized {
            window.maximize();
        }
        window.connect_close_request(|window| {
            crate::settings::set_editor_window(
                window.default_width(),
                window.default_height(),
                window.is_maximized(),
            );
            glib::Propagation::Proceed
        });
        // The third of `editor-test.sh`'s counts, beside "editor dropped" and "canvas
        // finalized": the window holds the editor's actions and every handler connected to
        // them, and a window that outlives its editor keeps all of those.
        let _ = window.add_weak_ref_notify_local(|| tracing::debug!("editor window finalized"));

        let editor = Rc::new(Self {
            window: window.clone(),
            canvas: canvas.clone(),
            history: RefCell::new(History::new()),
            tools: ToolState::default(),
            options: super::options::OptionsRow::default(),
            crop: super::crop::CropState::default(),
            app: app.clone(),
            capture: capture.clone(),
            actions,
            saved: RefCell::new(super::saved::Saved::default()),
            project: RefCell::new(None),
            closing: Cell::new(Closing::ToPreview),
            handed_back: Cell::new(false),
            opened_on: RefCell::new(None),
            picking: RefCell::new(None),
            pickers: RefCell::new(Vec::new()),
            text_layer: text_layer.clone(),
            editing: super::text::TextLayer::default(),
            cursors: super::cursor::Cursors::default(),
            text_css: RefCell::new(None),
            toasts,
            unsaved,
            size: gtk::Label::new(None),
            background: super::background::BackgroundPanel::default(),
        });
        {
            // What the window leaves behind (D108), while it still has a renderer to draw
            // the document with. Before `hold_until_closed`'s handler, which is connected
            // once this returns and lets go of the editor.
            let weak = Rc::downgrade(&editor);
            window.connect_close_request(move |_| {
                if let Some(editor) = weak.upgrade() {
                    editor.hand_back();
                }
                glib::Propagation::Proceed
            });
        }

        // The strip is built after the `Rc` exists, because every button needs a weak
        // reference back to the editor it drives. `spec/05` §1 puts the tools at the top
        // with §2's options inline after them; both are one page of a stack whose other
        // page is §4.11's crop toolbar -- "the whole toolbar swaps" -- so the swap is a
        // page change rather than a rebuild of the window's layout. Left-anchored, so the
        // tools never move when the options row changes width: a strip that re-centred
        // itself on every tool change would put the button the user is about to press
        // somewhere else.
        // Built after the `Rc`, like everything else that needs a weak reference back.
        split.set_sidebar(Some(&editor.build_background_sidebar()));
        *editor.background.split.borrow_mut() = Some(split.clone());
        // Narrow windows get an overlay rather than a second column: `MIN_CANVAS` is
        // what the editor's own half needs, and a sidebar beside it in a window that
        // small would leave the canvas too narrow to hold the toolbar. `adw::Breakpoint`
        // rather than a size-allocate handler, because the condition is exactly what a
        // breakpoint is for.
        //
        // The width is the sum of the two things that have to fit side by side, and it
        // is spelt as that sum on purpose. When it was a lone `980px` it was wrong by a
        // hundred pixels, and the way that showed up is worth remembering: a split view
        // asked for more than the window had made `AdwApplicationWindow` over-allocate,
        // GTK gave the title bar's box its children's minimums, and the *last* of them
        // fell off the right-hand edge -- which on GNOME's own button layout is the
        // close button (D100).
        let narrow = format!("max-width: {}px", super::background::WIDTH + MIN_CANVAS);
        if let Ok(condition) = adw::BreakpointCondition::parse(&narrow) {
            let breakpoint = adw::Breakpoint::new(condition);
            breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
            window.add_breakpoint(breakpoint);
        }

        let toolbars = editor.build_toolbars();
        toolbars.set_hexpand(true);
        bar.append(&toolbars);
        bar.append(&controls_end);
        // Last, and after the `Rc` exists: every button on it needs a weak reference back.
        view.add_bottom_bar(&editor.build_bottom_bar());

        editor.install_actions();
        editor.install_background_actions();
        // `spec/05` §4.14's drop zones, on the canvas because they are positions in the
        // document rather than in the window.
        editor.install_image_drop();
        editor.sync_background_panel();
        editor.wire_zoom();
        editor.wire_shortcuts();
        editor.wire_tools();
        editor.wire_context_menu();
        super::tools::watch_desktop_clipboard(&editor.window.clipboard());
        {
            // `spec/05` §2's row follows the selection (D56).
            let weak = Rc::downgrade(&editor);
            canvas.connect_selection_changed(move || {
                if let Some(editor) = weak.upgrade() {
                    editor.options_follow_selection();
                }
            });
        }

        // Shown first, then filled -- see the note at the top of this file.
        window.present();

        // `CAP-15`'s Shift at confirm, which `spec/05` §4.13 gives a second meaning:
        // "skippable by holding Shift at capture".
        let shift_held = capture.modifiers & SHIFT_MASK != 0;
        let had_document = document.is_some();
        let scene = document.unwrap_or_else(|| Scene::new(base_of(capture)));
        // `set_scene` asks to be fitted on the first real allocation. An idle callback
        // was not enough: it runs before the widget has been measured, so the fit divided
        // by a zero-size viewport and answered 1:1 -- which is what the editor's first
        // screenshot showed, with the bottom of the capture cut off.
        canvas.set_scene(scene);
        editor.refresh_size();
        if had_document {
            *editor.opened_on.borrow_mut() = canvas.scene();
        }
        // `spec/05` §4.13's auto-apply, and only for a capture: a project already carries
        // whatever background it was saved with, and putting the default one over it
        // would edit a document the user opened to look at.
        if !had_document {
            // §4.15 first: a window capture's background is what was behind the window,
            // and §4.13's preset is a choice about screenshots in general. A user who set
            // both gets the preset, because the preset is the more specific instruction --
            // `auto_apply_background` leaves a document that already has one alone.
            if capture.mode == octosnap_core::CaptureMode::Window {
                // §2's shadow switch first: the background is laid out over the canvas,
                // and the trim is what the canvas is.
                editor.trim_window_shadow();
                editor.apply_window_background(shift_held);
            }
            editor.auto_apply_background(shift_held);
        }
        // `spec/08` §1's Annotate row, which is about the editor and not about the
        // document, so a project reopens with the panel too.
        editor.restore_background_panel();

        info!(
            file = %capture.path.display(),
            window = editor.object_path().unwrap_or_default(),
            "editor opened"
        );
        let size = canvas.scene().map_or((0.0, 0.0), |scene| (scene.base.width, scene.base.height));
        report_first_frame(editor.window.upcast_ref(), requested, size);
        editor
    }

    /// [`Self::open_with`], on the document a closed editor left on a card (D108): the
    /// same capture, the same objects and the same undo stack, so the card's Annotate
    /// carries on exactly where the × left off -- Ctrl+Z included.
    pub fn reopen(
        app: &adw::Application,
        document: super::actions::Document,
        actions: EditorActions,
    ) -> Rc<Self> {
        let super::actions::Document { capture, scene, history, saved, project, .. } = document;
        let editor = Self::open_with(app, &capture, actions, Some(scene));
        editor.opened_on.replace(None);
        *editor.history.borrow_mut() = history;
        *editor.saved.borrow_mut() = saved;
        *editor.project.borrow_mut() = project;
        editor.refresh_unsaved();
        info!(undo = editor.history.borrow().depth(), "editor reopened on its document");
        editor
    }

    /// Whether closing has nothing to render (D108): the document is still the capture it
    /// was opened on -- no objects, no crop or padding, no rotate, flip or resize -- or,
    /// opened on a project file, still the project as it was read.
    pub(super) fn untouched(&self) -> bool {
        self.canvas.scene().is_none_or(|scene| self.untouched_as(&scene))
    }

    /// [`Self::untouched`], asked of a document the caller already holds.
    pub(super) fn untouched_as(&self, scene: &Scene) -> bool {
        if let Some(opened) = self.opened_on.borrow().as_ref() {
            return scene == opened;
        }
        let base = base_of(&self.capture);
        scene.objects().is_empty() && scene.base == base && scene.canvas == base.bounds()
    }

    /// The size readout, from the canvas rather than from the capture.
    pub(super) fn refresh_size(&self) {
        let Some(scene) = self.canvas.scene() else { return };
        #[allow(clippy::cast_possible_truncation)]
        let (w, h) = (scene.canvas.width.round() as i64, scene.canvas.height.round() as i64);
        self.size.set_label(&format!("{w} × {h}"));
    }

    #[must_use]
    pub fn window(&self) -> &adw::ApplicationWindow {
        &self.window
    }

    /// The project this editor was opened from, which its document then carries (D167).
    pub fn set_project(&self, path: &std::path::Path) {
        if let Some(scene) = self.canvas.scene() {
            *self.project.borrow_mut() = Some((scene, path.to_path_buf()));
        }
    }

    /// The window's exported D-Bus path, the way the cards and the pins derive theirs.
    ///
    /// The reason it is here is testability, and it is the same reason `pin/window.rs`
    /// has one: on Wayland a client cannot know where its own window is, so every
    /// geometry assertion in `pin-test.sh` goes through the extension's
    /// `MoveWindowBy(path, 0, 0)` -- a read disguised as a no-op move. Without a path the
    /// editor can only be checked by looking at a screenshot, which is how the two bugs
    /// in the canvas were found and is not how they should have been.
    #[must_use]
    pub fn object_path(&self) -> Option<String> {
        let app = self.window.application()?;
        let base = app.dbus_object_path()?;
        Some(format!("{base}/window/{}", self.window.id()))
    }

    /// `spec/05` §9: "Ctrl+scroll … zoom", a bare wheel pans, and "Space pan".
    fn wire_zoom(self: &Rc<Self>) {
        // Both axes and no `DISCRETE`: a touchpad reports smooth deltas in surface pixels
        // and a wheel reports clicks, and `unit()` says which, so the two can be scaled
        // to the same feel instead of the touchpad being chopped into wheel steps.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        {
            let editor = Rc::downgrade(self);
            scroll.connect_scroll(move |controller, dx, dy| {
                let Some(editor) = editor.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let state = controller.current_event_state();
                let wheel = controller.unit() == gtk::gdk::ScrollUnit::Wheel;

                if !state.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
                    // A bare wheel pans -- the document is what a viewer scrolls -- and
                    // Shift turns a one-axis wheel sideways. `pan_by` clamps, so a
                    // document that fits does not move.
                    let step = if wheel { WHEEL_PAN } else { 1.0 };
                    let (px, py) = if state.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                        (dy + dx, 0.0)
                    } else {
                        (dx, dy)
                    };
                    editor.canvas.pan_by(-px * step, -py * step);
                    editor.reposition_text_edit();
                    return glib::Propagation::Stop;
                }

                // Anchored at the pointer (`spec/05` §10), in the canvas's coordinates
                // rather than the window's -- the event carries a position relative to
                // the surface, and the canvas sits below a header bar.
                //
                // Falling back to the widget's centre and not its origin: an event with
                // no position is possible, a keyboard-driven zoom has none by definition,
                // and the corner is the one anchor that always looks wrong.
                let anchor = controller
                    .current_event()
                    .and_then(|event| event.position())
                    .and_then(|(x, y)| {
                        editor
                            .window
                            .compute_point(
                                &editor.canvas,
                                &gtk::graphene::Point::new(x as f32, y as f32),
                            )
                            .map(|p| (f64::from(p.x()), f64::from(p.y())))
                    })
                    .unwrap_or_else(|| editor.viewport_centre());
                // One wheel click is one `ZOOM_STEP`; a touchpad's pixels are turned into
                // a fraction of one, so a pinch-sized swipe zooms about as far as a click.
                let clicks = if wheel { -dy } else { -dy / SMOOTH_ZOOM_PIXELS };
                let factor = ZOOM_STEP.powf(clicks);
                // A wheel's click glides (`spec/09` §3); a touchpad's pixels stay direct,
                // because they follow the fingers and a glide would trail behind them.
                if wheel {
                    let goal = editor.canvas.zoom_goal() * factor;
                    editor.canvas.zoom_smoothly(goal, anchor, editor.follow_view());
                } else {
                    editor.canvas.zoom_to(editor.canvas.zoom() * factor, anchor);
                }
                editor.reposition_text_edit();
                glib::Propagation::Stop
            });
        }
        self.canvas.add_controller(scroll);

        // A middle-drag pans, beside §9's Space-drag, because a one-handed pan is what
        // people with a mouse reach for. `last` is reset on every begin: a drag reports
        // offsets from its own start, and the first version kept the previous drag's last
        // offset, so the second pan began with a jump.
        let drag = gtk::GestureDrag::new();
        drag.set_button(gtk::gdk::BUTTON_MIDDLE);
        let last = Rc::new(std::cell::Cell::new((0.0, 0.0)));
        {
            let editor = Rc::downgrade(self);
            let last = Rc::clone(&last);
            drag.connect_drag_update(move |_, x, y| {
                let Some(editor) = editor.upgrade() else { return };
                let (px, py) = last.get();
                editor.canvas.pan_by(x - px, y - py);
                editor.reposition_text_edit();
                last.set((x, y));
            });
        }
        drag.connect_drag_begin(move |_, _, _| last.set((0.0, 0.0)));
        self.canvas.add_controller(drag);

        // `spec/05` §9's "Space pan": while Space is held, a primary drag pans instead of
        // drawing or selecting. A key controller in the capture phase, because Space
        // reaching a focused toggle button presses it -- and left alone whenever the
        // focus is somewhere text is typed, where a space is a space.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let editor = Rc::downgrade(self);
            keys.connect_key_pressed(move |_, key, _, _| {
                let Some(editor) = editor.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if key != gtk::gdk::Key::space || editor.typing() {
                    return glib::Propagation::Proceed;
                }
                if !editor.tools.space_down.replace(true) {
                    editor.set_hover(super::cursor::Hover::Pan);
                }
                glib::Propagation::Stop
            });
        }
        {
            let editor = Rc::downgrade(self);
            keys.connect_key_released(move |_, key, _, _| {
                let Some(editor) = editor.upgrade() else { return };
                if key == gtk::gdk::Key::space && editor.tools.space_down.replace(false) {
                    editor.set_hover(super::cursor::Hover::Idle);
                }
            });
        }
        self.window.add_controller(keys);
    }

    /// Whether the keyboard is currently typing into something -- §4.5's text overlay, a
    /// value field, the picker's hex entry -- so keys that are shortcuts elsewhere have
    /// to be left alone here.
    pub(super) fn typing(&self) -> bool {
        if self.text_editing() {
            return true;
        }
        gtk::prelude::RootExt::focus(&self.window).is_some_and(|focused| {
            focused.is::<gtk::Text>() || focused.is::<gtk::TextView>()
        })
    }

    /// `spec/05` §9's keyboard map, as far as this milestone implements it.
    fn wire_shortcuts(self: &Rc<Self>) {
        let controller = gtk::ShortcutController::new();
        controller.set_scope(gtk::ShortcutScope::Global);

        type Binding = (&'static str, fn(&Rc<Editor>));
        /// A generated binding's body. Boxed because the tool keys and the size digits
        /// each close over a value, which a plain `fn` pointer cannot.
        type Run = Box<dyn Fn(&Rc<Editor>)>;
        let bindings: &[Binding] = &[
            // Each of these moves the document under the editing overlay, so each one has
            // to move the overlay with it.
            ("<Control>0", |e| e.canvas.glide_to_fit(e.follow_view())),
            ("<Control>1", |e| e.canvas.glide_to_actual(e.follow_view())),
            ("<Control>plus", |e| e.zoom_by(ZOOM_STEP)),
            ("<Control>equal", |e| e.zoom_by(ZOOM_STEP)),
            ("<Control>minus", |e| e.zoom_by(1.0 / ZOOM_STEP)),
            // `spec/05` §7: "Close | Ctrl+W". Back to the stack as a card; with Shift,
            // Final Close, which leaves none (D108).
            ("<Control>w", Editor::close_to_preview),
            ("<Control><Shift>w", Editor::final_close),
            // §7's bottom-bar keys. Copy keeps the editor open (D108).
            ("<Control>c", Editor::copy_shortcut),
            ("<Control>x", Editor::cut_shortcut),
            ("<Control>v", Editor::paste_shortcut),
            ("<Control>s", Editor::save_rendered),
            ("<Control><Shift>s", Editor::save_rendered_as),
            ("<Control><Shift>p", |e| e.pin_rendered(false)),
            ("<Control><Shift><Alt>p", |e| e.pin_rendered(true)),
            // §4.1's selection, all of it (`spec/13` #3).
            ("<Control>a", Editor::select_all),
            ("<Control>p", Editor::print_rendered),
            // `spec/13` #8's context menu, from the keyboard.
            ("<Shift>F10", Editor::open_context_menu_at_selection),
            ("Menu", Editor::open_context_menu_at_selection),
            // `spec/05` §2's Background row: "G [P] | a full **left sidebar**".
            ("g", |e| {
                let open = e.background_panel_open();
                e.show_background_panel(!open);
            }),
            // §5.2's undo stack, §4.1's list of what it undoes.
            ("<Control>z", Editor::undo),
            ("<Control><Shift>z", Editor::redo),
            ("<Control>y", Editor::redo),
            ("<Control>d", Editor::duplicate_selection),
            ("Delete", Editor::delete_selection),
            ("BackSpace", Editor::delete_selection),
            // Escape cancels and deselects, and never closes (D61).
            ("Escape", Editor::escape),
            ("<Control>bracketright", |e| e.reorder_selection(true)),
            ("<Control>bracketleft", |e| e.reorder_selection(false)),
            // `spec/05` §9's `[` `]` intensity and F11 fullscreen, which were not bound
            // (D137). A field typing a bracket takes it first: these run after it.
            ("bracketleft", |e| e.step_intensity(false)),
            ("bracketright", |e| e.step_intensity(true)),
            ("F11", |e| {
                if e.window.is_fullscreen() {
                    e.window.unfullscreen();
                } else {
                    e.window.fullscreen();
                }
            }),
            // §4.1: "arrow keys nudge 1 unit, Shift+arrows 10".
            ("Left", |e| e.nudge_selection(-1.0, 0.0)),
            ("Right", |e| e.nudge_selection(1.0, 0.0)),
            ("Up", |e| e.nudge_selection(0.0, -1.0)),
            ("Down", |e| e.nudge_selection(0.0, 1.0)),
            ("<Shift>Left", |e| e.nudge_selection(-10.0, 0.0)),
        ];
        // The remaining nudges and the whole tool alphabet are generated rather than
        // listed: §2's Key column is already a table in `scene::tool`, and writing it out
        // again here is a second copy to disagree with the first.
        let generated: Vec<(String, Run)> = [
            ("<Shift>Right", (10.0, 0.0)),
            ("<Shift>Up", (0.0, -10.0)),
            ("<Shift>Down", (0.0, 10.0)),
        ]
        .into_iter()
        .map(|(accel, (dx, dy))| {
            let run: Run = Box::new(move |e: &Rc<Editor>| e.nudge_selection(dx, dy));
            (accel.to_owned(), run)
        })
        .chain(Tool::ALL.into_iter().map(|tool| {
            let run: Run = Box::new(move |e: &Rc<Editor>| e.set_tool(tool));
            (tool.accelerator().to_owned(), run)
        }))
        .chain((1..=octosnap_scene::SIZE_LEVELS).map(|level| {
            let run: Run = Box::new(move |e: &Rc<Editor>| e.set_size_level(level));
            (level.to_string(), run)
        }))
        .collect();
        for (accel, run) in generated {
            let Some(trigger) = gtk::ShortcutTrigger::parse_string(&accel) else {
                tracing::warn!(accel = %accel, "unparseable accelerator");
                continue;
            };
            let editor = Rc::downgrade(self);
            let action = gtk::CallbackAction::new(move |_, _| {
                if let Some(editor) = editor.upgrade() {
                    run(&editor);
                }
                glib::Propagation::Stop
            });
            controller.add_shortcut(gtk::Shortcut::new(Some(trigger), Some(action)));
        }
        // §4.11's "Enter applies", and the one binding that has to *decline* the key
        // when it is not for it: `ShortcutScope::Global` runs before the focused widget,
        // so an unconditional Return would take every newline out of §4.5's text overlay.
        for accel in ["Return", "KP_Enter"] {
            let Some(trigger) = gtk::ShortcutTrigger::parse_string(accel) else { continue };
            let editor = Rc::downgrade(self);
            let action = gtk::CallbackAction::new(move |_, _| {
                let Some(editor) = editor.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if !editor.cropping() {
                    return glib::Propagation::Proceed;
                }
                editor.apply_crop();
                glib::Propagation::Stop
            });
            controller.add_shortcut(gtk::Shortcut::new(Some(trigger), Some(action)));
        }

        for &(accel, run) in bindings {
            let Some(trigger) = gtk::ShortcutTrigger::parse_string(accel) else { continue };
            let editor = Rc::downgrade(self);
            // A key that means something inside a text field is the field's while one has
            // focus: Ctrl+A selects the hex digits, Ctrl+C copies them, Ctrl+Z takes back a
            // keystroke, Delete deletes a character. A global shortcut with a modifier
            // reaches the window before the field does, so these decline while typing
            // (`spec/13` #3, Jakob's law). The window's own keys -- Ctrl+W, Ctrl+S,
            // Ctrl+P, the zoom -- have no text meaning and stay.
            let yields_to_text = TEXT_FIELD_KEYS.contains(&accel);
            let action = gtk::CallbackAction::new(move |_, _| {
                let Some(editor) = editor.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if yields_to_text && editor.typing() {
                    return glib::Propagation::Proceed;
                }
                run(&editor);
                glib::Propagation::Stop
            });
            controller.add_shortcut(gtk::Shortcut::new(Some(trigger), Some(action)));
        }
        self.window.add_controller(controller);
    }

    /// Called after every edit that reached the history.
    ///
    /// One line, and it exists to be grepped. Both harnesses in `docs/spikes/tools` read
    /// the app's log rather than its pixels -- that is what let `pin-test.sh` state
    /// "a pin the compositor moved comes back where it was left" as an assertion instead
    /// of a screenshot -- and an editor with no such line can only be tested by looking
    /// at it. `objects` and `undo` together are enough to check that a drag produced one
    /// object and one step, which is `spec/05` §5.2's whole claim.
    pub(super) fn after_edit(&self) {
        let objects = self.canvas.scene().map_or(0, |scene| scene.len());
        let history = self.history.borrow();
        info!(
            objects,
            undo = history.depth(),
            can_redo = history.can_redo(),
            selected = self.canvas.selection().len(),
            "edit"
        );
        drop(history);
        // The row shows the selection's values, and an edit can change them -- a corner
        // drag on a text box sets a font size no preset has.
        self.sync_options();
        self.refresh_unsaved();
        self.refresh_size();
        // §4.13's panel follows the document too: an undo of a padding drag has to move
        // the slider back, and nothing else would.
        self.sync_background_panel();
    }

    /// `spec/05` §7's Ctrl+C, which means two different things.
    ///
    /// §7 qualifies it as "Ctrl+C (**nothing selected**)" and §4.1 says what the other
    /// case is: "Ctrl+C/Ctrl+V copies objects (internal clipboard); **when nothing is
    /// selected Ctrl+C copies the image**". A selection of nothing but the kinds that are
    /// not copied -- the background, the crop -- is nothing to copy as objects, so it copies
    /// the image too.
    pub(super) fn copy_shortcut(self: &Rc<Self>) {
        let selection = self.canvas.selection();
        if selection.is_empty() {
            self.copy_rendered();
            return;
        }
        match self.copy_objects(&selection) {
            0 => self.copy_rendered(),
            1 => self.toast("Copied 1 object", None),
            n => self.toast(&format!("Copied {n} objects"), None),
        }
    }

    /// Ctrl+X: §4.1's copy, then the delete, of what was copied. Only what the object
    /// clipboard takes is cut: the background and the crop would be deleted with nothing
    /// to paste them back from.
    pub(super) fn cut_shortcut(self: &Rc<Self>) {
        let Some(scene) = self.canvas.scene() else { return };
        let cut: Vec<octosnap_scene::ObjectId> = self
            .canvas
            .selection()
            .into_iter()
            .filter(|id| {
                scene.get(*id).is_some_and(|o| octosnap_scene::clipboard::copyable(o.kind()))
            })
            .collect();
        if cut.is_empty() || self.cut_objects(&cut) == 0 {
            return;
        }
        self.canvas.set_selection(cut.clone());
        self.delete_selection();
        match cut.len() {
            1 => self.toast("Cut 1 object", None),
            n => self.toast(&format!("Cut {n} objects"), None),
        }
    }

    /// §4.1's Ctrl+V: the copied objects, or else a picture from the desktop's clipboard.
    pub(super) fn paste_shortcut(self: &Rc<Self>) {
        if !self.paste_objects() {
            self.paste_picture();
        }
    }

    /// Escape, which means "back out one step" and never "close" (D61).
    ///
    /// Crop mode first (§4.11: "Esc cancels"), then §4.5's text edit -- though the
    /// `TextView`'s own key controller usually takes it before this ever runs -- then an
    /// armed pipette, then §2's deselect. Innermost first is the only order that lets a
    /// user back out one step at a time; with nothing to back out of, nothing happens.
    /// `spec/05` §7's "Esc when nothing selected" *closed* the window: a GNOME window does
    /// not close on Escape, and an Escape pressed out of habit after a drag met "Discard
    /// the annotations?". Ctrl+W and the title bar close.
    fn escape(self: &Rc<Self>) {
        if self.cropping() {
            self.cancel_crop();
        } else if self.text_editing() {
            self.commit_text_edit();
        } else if self.picking.borrow_mut().take().is_some() {
            self.canvas.set_cursor_from_name(None);
            tracing::debug!("pipette disarmed");
        } else if self.canvas.selection().is_empty() {
            tracing::debug!("Escape with nothing to cancel; the window stays (D61)");
        } else {
            self.deselect();
        }
    }

    fn zoom_by(self: &Rc<Self>, factor: f64) {
        let goal = self.canvas.zoom_goal() * factor;
        self.canvas.zoom_smoothly(goal, self.viewport_centre(), self.follow_view());
    }

    /// What has to move with the view on every frame of a glide: the text being typed,
    /// whose overlay sits over the document rather than in it.
    pub(super) fn follow_view(self: &Rc<Self>) -> impl Fn() + 'static {
        let editor = Rc::downgrade(self);
        move || {
            if let Some(editor) = editor.upgrade() {
                editor.reposition_text_edit();
            }
        }
    }

    fn viewport_centre(&self) -> (f64, f64) {
        (
            f64::from(self.canvas.width()) / 2.0,
            f64::from(self.canvas.height()) / 2.0,
        )
    }
}

/// The capture's pixels as a document's base image.
///
/// Its size is the rounded physical size the extension wrote -- `Rect::to_physical`, the
/// app's one rounding rule -- and not `logical x scale` unrounded. At 1.25 a 450-unit
/// capture is 562.5 by the arithmetic and 563 pixels in the file, and a canvas of 562.5
/// exported at 2x as 1125 rows where the picture had 1126; the harness at 1.25 caught the
/// off-by-one in both the export and the crop.
fn base_of(capture: &CaptureResult) -> Base {
    let (width, height) = capture.rect.to_physical(capture.scale);
    Base::new(
        capture.path.to_string_lossy().into_owned(),
        f64::from(width),
        f64::from(height),
        capture.scale,
    )
}

/// How big the window opens: the remembered size, or `spec/05` §1's share of the screen.
///
/// §1 measured CleanShot at 1322 x 934 on a 1710 x 1107 screen -- 77 % wide and 84 %
/// tall. The screen is the monitor the capture came from, by overlap, which is the rule
/// D50 settled for the pin: on one display any monitor is that one, and on two the
/// largest is the wrong one exactly when the capture was on the smaller.
fn window_size_for(capture: &CaptureResult) -> (i32, i32, bool) {
    if let Some(remembered) = crate::settings::editor_window() {
        return remembered;
    }
    let Some(display) = gtk::gdk::Display::default() else { return (1100, 760, false) };
    let monitors = display.monitors();
    let mut best: Option<(gtk::gdk::Rectangle, i64)> = None;
    for index in 0..monitors.n_items() {
        let Some(monitor) = monitors.item(index).and_downcast::<gtk::gdk::Monitor>() else {
            continue;
        };
        let g = monitor.geometry();
        let held = capture
            .rect
            .overlap_area(octosnap_core::Rect::new(g.x(), g.y(), g.width(), g.height()));
        if best.is_none_or(|(_, most)| held > most) {
            best = Some((g, held));
        }
    }
    let Some((screen, _)) = best else { return (1100, 760, false) };
    #[allow(clippy::cast_possible_truncation)]
    let size = (
        (f64::from(screen.width()) * 0.77).round() as i32,
        (f64::from(screen.height()) * 0.84).round() as i32,
    );
    (size.0.max(660), size.1.max(440), false)
}

/// The editor's stylesheet, installed once per display.
///
/// A provider at display scope, added every time an editor opens: GTK replaces a provider
/// added twice at the same priority rather than stacking it, and the alternative -- a
/// `OnceCell` guarding it -- would be one more piece of state for something idempotent.
/// The cards do the same thing for the same reason.
fn install_editor_css() {
    // Once per process, not once per window. A provider added to the display is never
    // taken away, so a provider per editor accumulated for the life of the app -- and
    // every addition restyles every widget on the display. Found while reading `spec/05`
    // §11 item 10's numbers (D60).
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
.octosnap-canvas {
    /* The area around the document. `@window_bg_color` rather than a fixed grey so a
       dark session gets a dark desk; the pattern that used to be here read as a missing
       texture, which is what an unexplained chequer looks like. */
    background-color: @window_bg_color;
}
/* `spec/05` §1: \"The **active tool is a filled accent-blue button**; every other tool is
   plain. There is no separate selection indicator.\" A checked toggle in a linked group is
   a shade of grey by default, which at a glance is the same as a hovered one. */
.octosnap-tools button:checked {
    background-color: @accent_bg_color;
    color: @accent_fg_color;
}
/* `spec/09` §3's 'Editor | tool switch | 80–120 ms | ease-out': the accent passing from one
   tool to the next, quicker than libadwaita's 200 ms for buttons in general. GTK leaves
   CSS transitions out with animations off (D129). */
.octosnap-tools button {
    transition: background 100ms ease-out, color 100ms ease-out;
}
.octosnap-tools button:checked:hover {
    background-color: shade(@accent_bg_color, 1.08);
}
",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
