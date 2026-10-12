// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §4.5's text tool: the fonts, and the inline editing overlay.
//!
//! > Click places a text box at the pointer and starts inline editing (a `Gtk.TextView`
//! > overlay positioned in canvas coordinates, scaled with zoom); Enter commits,
//! > Shift+Enter inserts a newline, Esc cancels/commits (commit if non-empty).
//!
//! A **real** `GtkTextView`, and §6 says why rather than leaving it to taste: "so IME/emoji
//! input works (CleanShot fixed a Chinese IME crash for exactly this reason)". Drawing a
//! caret onto the canvas and handling key events would be a text editor written from
//! scratch, and it would be the one part of the editor a person could not type Japanese
//! into.

use gtk::pango;
use octosnap_scene::TextStyle;

/// `spec/05` §4.5's font choice, as a Pango description.
///
/// > Font: system UI font (Adwaita Sans / Inter) for Standard, `monospace` for
/// > Monospaced; keep a font picker out of v1.
///
/// **"Rounded" is a documented deviation.** §4.5 lists it as a verified style but gives no
/// rendering for it, and the obvious reading -- a rounded typeface, as macOS has in SF
/// Rounded -- has no guaranteed equivalent on Linux: `fc-list` on the development machine
/// finds no rounded family at all. So the families below are tried in order for the benefit
/// of a machine that has one, and where none resolves the style falls back to the UI font
/// at **Medium** weight. That is visibly softer than Standard rather than identical to it,
/// which is the honest approximation; rendering it exactly as Standard would make one of
/// seven menu entries do nothing.
#[must_use]
pub fn font_for(style: TextStyle, size: f64) -> pango::FontDescription {
    let mut description = pango::FontDescription::new();
    match style {
        TextStyle::Monospaced | TextStyle::MonospacedBox => description.set_family("monospace"),
        TextStyle::Rounded | TextStyle::RoundedBox => {
            // Fontconfig walks a comma-separated list and takes the first it has.
            description.set_family("Nunito,Quicksand,Comfortaa,Varela Round,SF Rounded,sans");
            description.set_weight(pango::Weight::Medium);
        }
        _ => description.set_family("sans"),
    }
    // Absolute, not `set_size`: `spec/05` §4.5's presets are in points of the *document*
    // and must not be re-scaled by the display's DPI. The canvas's zoom is the only thing
    // allowed to change how big they look.
    description.set_absolute_size(size * f64::from(pango::SCALE));
    description
}

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use octosnap_scene::{Command, Geometry, Object, Point, TextAlign};

use super::window::Editor;

/// Where the editing overlay lives while it is open.
pub type TextLayer = RefCell<Option<Editing>>;

/// One text object being typed into.
#[derive(Debug)]
pub struct Editing {
    view: gtk::TextView,
    /// The document point the text's top-left corner sits at.
    pos: Point,
    style: TextStyle,
    font_size: f64,
    align: TextAlign,
    /// The wrap width the box was given -- by a drag, a side handle, or an earlier
    /// commit. `None` is an auto-width box that grows with its text.
    width: Option<f64>,
    /// The wrap width in force right now: `width`, or the room left to the canvas's edge
    /// once an auto-width box has grown that far. What the commit stores, so the label
    /// keeps the line breaks the user watched appear.
    effective_width: Option<f64>,
    /// The object being edited, or `None` while placing a new one.
    ///
    /// The difference is the command: an existing object produces a `Change`, a new one an
    /// `Add`, and an existing one emptied produces a `Remove` -- which is what §4.5's
    /// "commit if non-empty" means for text that already existed.
    subject: Option<Box<Object>>,
    /// The label as it reads while it is typed, drawn by the canvas: the subject with its
    /// new words, or a new label put in as the draft. The view over it draws only the
    /// caret and the selection, so what is typed is the label itself -- its plate, its
    /// outline, its colours -- and nothing changes when the edit ends (D197).
    shown: Object,
}

impl Editor {
    /// Whether a text edit is open, which several gestures need to know before acting.
    #[must_use]
    pub(super) fn text_editing(&self) -> bool {
        self.editing.borrow().is_some()
    }

    /// `spec/05` §4.5: "Click places a text box at the pointer and starts inline editing".
    ///
    /// `subject` is the object being re-edited, from §4.1's "Double-click a text object
    /// edits it", or `None` for a new one.
    pub(super) fn begin_text_edit(
        self: &Rc<Self>,
        pos: Point,
        width: Option<f64>,
        subject: Option<Object>,
    ) {
        // Whatever was already being typed is finished first. Two overlays would leave one
        // of them unreachable, and a click that opens a second box is a click the user
        // meant to end the first with.
        self.commit_text_edit();
        // And whatever gesture the same press started is abandoned. A double-click on a
        // label arrives as a press the drag gesture answers with `Moving` *and* a second
        // press the click gesture answers with this; left in place, the move would end on
        // the object as blanked for editing and commit the blank as a change.
        *self.tools.gesture.borrow_mut() = None;
        self.canvas.set_draft(None);

        let settings = self.settings();
        let (style, font_size, align, width, text) = match &subject {
            Some(object) => match &object.geometry {
                Geometry::Text { text, style, font_size, align, width, .. } => {
                    (*style, *font_size, *align, *width, text.clone())
                }
                // Not a text object; nothing to edit.
                _ => return,
            },
            None => (settings.text, settings.font_size, settings.align, width, String::new()),
        };
        let shown = match &subject {
            Some(object) => object.clone(),
            None => {
                let z = self.canvas.scene().map_or(0, |scene| scene.top_z().saturating_add(1));
                Object::new(
                    z,
                    now_seconds(),
                    settings.style,
                    Geometry::Text {
                        pos,
                        text: String::new(),
                        style,
                        font_size,
                        align,
                        width,
                        tail: None,
                        ink: Some(settings.style.color),
                        background: settings.text_background,
                    },
                )
            }
        };
        // The handles of the label being typed into are the edit's frame now.
        self.canvas.set_selection(Vec::new());

        let view = gtk::TextView::new();
        view.set_wrap_mode(if width.is_some() {
            gtk::WrapMode::WordChar
        } else {
            gtk::WrapMode::None
        });
        view.buffer().set_text(&text);
        view.set_monospace(style.is_monospaced());
        // Lined up as the committed text will be (D167), so a centred text does not jump
        // to the left while it is being typed.
        view.set_justification(match align {
            TextAlign::Start => gtk::Justification::Left,
            TextAlign::Center => gtk::Justification::Center,
            TextAlign::End => gtk::Justification::Right,
        });
        // The document's font at the document's size. The zoom is applied as a transform
        // on the layer, not by scaling the font, so the glyphs the user types are laid out
        // exactly as `Canvas::text_layout` lays out the label under them -- which is what
        // puts the caret between the right two letters.
        apply_style(&view, style, font_size);
        // The view stands over the label rather than for it: transparent, its glyphs too,
        // so the canvas's own drawing of the label shows through, and the caret and the
        // selection are all it adds. Its padding is the plate's, in document units like
        // everything under the layer's transform. The first version drew the text itself,
        // as a text view can -- flat, without the outline or the plate's own colours, so
        // a label looked one way while it was typed and another once it was done.
        view.add_css_class("octosnap-text-edit");
        #[allow(clippy::cast_possible_truncation)]
        let pad = (style.plate_padding_em() * font_size).round() as i32;
        view.set_left_margin(pad);
        view.set_right_margin(pad);
        view.set_top_margin(pad);
        view.set_bottom_margin(pad);
        view.set_pixels_above_lines(0);
        view.set_pixels_below_lines(0);
        self.install_text_css(style, font_size);

        {
            let editor = Rc::downgrade(self);
            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed(move |_, key, _, state| {
                let Some(editor) = editor.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let shift = state.contains(gtk::gdk::ModifierType::SHIFT_MASK);
                match key {
                    // §4.5: "Enter commits, Shift+Enter inserts a newline".
                    gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter if !shift => {
                        editor.commit_text_edit();
                        glib::Propagation::Stop
                    }
                    // §4.5: "Esc cancels/commits (commit if non-empty)". So Escape is not
                    // a cancel at all when there is something to keep -- which is the
                    // opposite of what Escape usually means, and is what the spec asks
                    // for: a user who types a label and presses Escape wanted the label.
                    gtk::gdk::Key::Escape => {
                        editor.commit_text_edit();
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            });
            view.add_controller(keys);
        }

        self.text_layer.put(&view, 0.0, 0.0);
        self.text_layer.set_visible(true);
        *self.editing.borrow_mut() = Some(Editing {
            view: view.clone(),
            pos,
            style,
            font_size,
            align,
            width,
            effective_width: width,
            subject: subject.map(Box::new),
            shown,
        });
        // Sized to its text now and after every keystroke, from the renderer's own layout,
        // and the label under it showing what has been typed.
        self.fit_text_edit();
        self.show_text_edit();
        {
            let editor = Rc::downgrade(self);
            view.buffer().connect_changed(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.fit_text_edit();
                    editor.show_text_edit();
                }
            });
        }
        self.reposition_text_edit();
        view.grab_focus();
        tracing::debug!(x = pos.x, y = pos.y, size = font_size, width = ?width, "text edit opened");
    }

    /// Sizes the overlay to the text in it, the way the renderer will draw it.
    ///
    /// `GtkTextView` is built to live in a scrolled window, so left to itself it asks for
    /// almost no width and scrolls -- which is what made the first version's box tiny.
    /// Here the same Pango layout the canvas draws with says how big the text is, and the
    /// view is asked to be exactly that plus its plate padding, plus a little room for
    /// the caret at the end of an auto-width line.
    ///
    /// An auto-width box that reaches the canvas's right edge starts wrapping there,
    /// rather than growing off the picture: the room to the edge becomes its width for as
    /// long as the text needs it, and the commit stores that width so the line breaks the
    /// user watched appear are the ones the label keeps.
    pub(super) fn fit_text_edit(&self) {
        let mut editing = self.editing.borrow_mut();
        let Some(editing) = editing.as_mut() else { return };
        let buffer = editing.view.buffer();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
        // An empty box is still a line tall and a few ems wide, so the user sees where
        // they are about to type -- "you click; it's not shown where".
        let shown = if text.is_empty() { "M" } else { text.as_str() };
        let pad = editing.style.plate_padding_em() * editing.font_size;
        let caret_room =
            if text.is_empty() { editing.font_size * 1.5 } else { editing.font_size * 0.6 };
        let natural = self
            .canvas
            .layout_for(shown, editing.style, editing.font_size, editing.align, None)
            .pixel_size();
        let canvas_right =
            self.canvas.scene().map_or(f64::INFINITY, |scene| scene.canvas.x + scene.canvas.width);
        let room = (canvas_right - editing.pos.x - pad).max(editing.font_size * 3.0);
        let wrap = match editing.width {
            Some(width) => Some(width),
            None if f64::from(natural.0) + caret_room > room => Some(room),
            None => None,
        };
        editing.effective_width = wrap;
        editing.view.set_wrap_mode(if wrap.is_some() {
            gtk::WrapMode::WordChar
        } else {
            gtk::WrapMode::None
        });
        let layout =
            self.canvas.layout_for(shown, editing.style, editing.font_size, editing.align, wrap);
        let (w, h) = layout.pixel_size();
        let width = wrap.unwrap_or(f64::from(w) + caret_room);
        #[allow(clippy::cast_possible_truncation)]
        editing.view.set_size_request(
            (width + 2.0 * pad).ceil() as i32,
            (f64::from(h) + 2.0 * pad).ceil() as i32,
        );
    }

    /// The overlay's frame, caret and selection, as a stylesheet for this one edit.
    ///
    /// Everything else is transparent: the label under it is the canvas's (D197). The
    /// frame is solid where a selected label's is dashed -- the same box, saying "typing"
    /// rather than "selected" -- outside the view, so it moves no glyph, and in the
    /// selection's orange. The sizes are in document units, under the layer's zoom, so
    /// they are divided by it to stay the same on screen.
    fn install_text_css(&self, style: TextStyle, font_size: f64) {
        let Some(display) = gtk::gdk::Display::default() else { return };
        if let Some(old) = self.text_css.borrow_mut().take() {
            gtk::style_context_remove_provider_for_display(&display, &old);
        }
        let radius = style.plate_radius_em() * font_size;
        let per_pixel = 1.0 / (self.canvas.zoom() * self.canvas.picture_scale()).max(0.01);
        let (frame, gap) = (1.5 * per_pixel, 3.0 * per_pixel);
        let css = format!(
            "textview.octosnap-text-edit, textview.octosnap-text-edit > text {{
                 background-color: transparent;
                 color: transparent;
             }}
             textview.octosnap-text-edit {{
                 border-radius: {radius:.1}px;
                 outline: {frame:.2}px solid #ED5B00;
                 outline-offset: {gap:.2}px;
                 caret-color: #ED5B00;
             }}
             textview.octosnap-text-edit > text > selection {{
                 background-color: alpha(#ED5B00, 0.3);
                 color: transparent;
             }}"
        );
        let provider = gtk::CssProvider::new();
        provider.load_from_string(&css);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
        *self.text_css.borrow_mut() = Some(provider);
    }

    /// The options row's change to the label being typed (D197): its colour, its size, its
    /// style or its alignment, shown at once. Answers whether a label is being typed, so
    /// the row's caller knows the change is spent here -- the selection is empty while
    /// it is, and the change would otherwise only reach the tool's memory. The label used
    /// to take the row's colour when it was committed, from whatever it was then; with the
    /// label drawn while it is typed, it has to take it as it is chosen.
    pub(super) fn restyle_text_edit(
        &self,
        control: octosnap_scene::tool::Control,
        settings: &octosnap_scene::tool::ToolSettings,
    ) -> bool {
        let (style, font_size, align, view) = {
            let mut editing = self.editing.borrow_mut();
            let Some(editing) = editing.as_mut() else { return false };
            if !control.apply(settings, &mut editing.shown) {
                return true;
            }
            let Geometry::Text { style, font_size, align, .. } = editing.shown.geometry else {
                return true;
            };
            editing.style = style;
            editing.font_size = font_size;
            editing.align = align;
            (style, font_size, align, editing.view.clone())
        };
        if let Some(tag) = view.buffer().tag_table().lookup("octosnap-text") {
            tag.set_font_desc(Some(&font_for(style, font_size)));
        }
        view.set_monospace(style.is_monospaced());
        view.set_justification(match align {
            TextAlign::Start => gtk::Justification::Left,
            TextAlign::Center => gtk::Justification::Center,
            TextAlign::End => gtk::Justification::Right,
        });
        #[allow(clippy::cast_possible_truncation)]
        let pad = (style.plate_padding_em() * font_size).round() as i32;
        view.set_left_margin(pad);
        view.set_right_margin(pad);
        view.set_top_margin(pad);
        view.set_bottom_margin(pad);
        self.install_text_css(style, font_size);
        self.fit_text_edit();
        self.show_text_edit();
        self.reposition_text_edit();
        view.grab_focus();
        true
    }

    /// A press anywhere off the label being typed ends the edit, with any button (D198).
    ///
    /// The layer the text view stands in covers the whole canvas while it is up, so the
    /// canvas's own press and right-click handlers never heard a click away -- only Esc
    /// ended the edit, which the owner found the hard way. The press is the layer's when
    /// it misses the view, and the view's own when it lands in it, where it places the
    /// caret.
    pub(super) fn wire_text_layer(self: &Rc<Self>) {
        let click = gtk::GestureClick::new();
        click.set_button(0);
        let editor = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(editor) = editor.upgrade() else { return };
            let view = editor.editing.borrow().as_ref().map(|editing| editing.view.clone());
            let Some(view) = view else { return };
            // A left or a right click, the owner's two; the middle button is the view's pan.
            if gesture.current_button() == gtk::gdk::BUTTON_MIDDLE {
                return;
            }
            #[allow(clippy::cast_possible_truncation)]
            let at = gtk::graphene::Point::new(x as f32, y as f32);
            let bounds = view.compute_bounds(&editor.text_layer);
            let inside = bounds.is_some_and(|bounds| bounds.contains_point(&at));
            tracing::debug!(
                x,
                y,
                inside,
                bounds = ?bounds.map(|b| (b.x(), b.y(), b.width(), b.height())),
                "text layer press"
            );
            if inside {
                return;
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);
            editor.finish_text_edit(false);
        });
        self.text_layer.add_controller(click);
    }

    /// Puts the overlay where the document says it should be, at the current zoom.
    ///
    /// Called after every zoom and pan as well as at the start: the overlay is positioned
    /// in *document* coordinates and drawn in widget ones, so anything that changes the
    /// mapping moves it.
    pub(super) fn reposition_text_edit(&self) {
        let editing = self.editing.borrow();
        let Some(editing) = editing.as_ref() else { return };
        // The zoom *and* whatever `spec/05` §4.13's background is doing to the picture:
        // an inset shrinks everything drawn on the capture, and a text edit in progress
        // has to shrink with it or the glyphs would jump on commit.
        let zoom = self.canvas.zoom() * self.canvas.picture_scale();
        // The view's corner is the *plate's* corner: the glyphs start a padding inside,
        // which is where `pos` is and where the committed object will draw them.
        let pad = editing.style.plate_padding_em() * editing.font_size;
        let (x, y) = self.canvas.to_widget(Point::new(editing.pos.x - pad, editing.pos.y - pad));
        // A single transform that carries both, because `GtkFixed::set_child_transform`
        // **replaces** the child's position rather than composing with it -- D39, found
        // when the same call moved a card's body out from under its input region.
        #[allow(clippy::cast_possible_truncation)]
        let transform = gtk::gsk::Transform::new()
            .translate(&gtk::graphene::Point::new(x as f32, y as f32))
            .scale(zoom as f32, zoom as f32);
        self.text_layer.set_child_transform(&editing.view, Some(&transform));
    }

    /// The label under the overlay, saying what has been typed so far (D197): the draft
    /// for a new one, the displayed scene for one being re-edited. The history does not
    /// hear of it; `commit_text_edit` decides what it was.
    fn show_text_edit(&self) {
        let shown = {
            let mut editing = self.editing.borrow_mut();
            let Some(editing) = editing.as_mut() else { return };
            let buffer = editing.view.buffer();
            let typed = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
            let effective = editing.effective_width;
            if let Geometry::Text { text, width, .. } = &mut editing.shown.geometry {
                *text = typed.trim_end_matches('\n').to_owned();
                *width = effective;
            }
            (editing.shown.clone(), editing.subject.is_some())
        };
        match shown {
            (object, true) => {
                if let Some(mut scene) = self.canvas.scene() {
                    scene.replace(object);
                    self.canvas.update_scene(scene);
                }
            }
            (object, false) => self.canvas.set_draft(Some(object)),
        }
    }

    /// Ends the edit, keeping the text if there is any (`spec/05` §4.5), and leaves the
    /// label selected (D197).
    pub(super) fn commit_text_edit(self: &Rc<Self>) {
        self.finish_text_edit(true);
    }

    /// [`Self::commit_text_edit`], leaving the label selected or not. A click away from
    /// the box ends the edit and leaves nothing selected, so the next click places the
    /// next label; Enter and Esc leave it selected, for the keys that act on a selection
    /// (D197).
    pub(super) fn finish_text_edit(self: &Rc<Self>, select: bool) {
        let Some(editing) = self.editing.borrow_mut().take() else { return };
        tracing::debug!(select, "text edit closed");
        self.canvas.set_draft(None);
        let buffer = editing.view.buffer();
        let text = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .trim_end_matches('\n')
            .to_owned();
        self.text_layer.remove(&editing.view);
        self.text_layer.set_visible(false);
        if let (Some(display), Some(css)) =
            (gtk::gdk::Display::default(), self.text_css.borrow_mut().take())
        {
            gtk::style_context_remove_provider_for_display(&display, &css);
        }
        // Focus back on the canvas, so the next key is a shortcut again. Removing the
        // focused view left the window with no focus widget, and in the harness the keys
        // typed after a commit -- `r`, then `v` -- reached nothing for ten seconds: the
        // Return that ended the edit had landed on whichever toolbar button GTK picked
        // and opened its popover, which then swallowed the letters.
        self.canvas.grab_focus();

        // Put the original back before deciding anything. Every branch below either
        // replaces it, removes it, or wanted it unchanged -- and unchanged is the one case
        // a `Change` cannot express, so it has to be handled by restoring rather than by
        // applying.
        if let Some(before) = &editing.subject
            && let Some(mut scene) = self.canvas.scene()
        {
            scene.replace((**before).clone());
            self.canvas.update_scene(scene);
        }

        let command = match (editing.subject, text.is_empty()) {
            // A new box left empty: nothing happened, and nothing should be undoable.
            (None, true) => None,
            (None, false) => {
                let Some(scene) = self.canvas.scene() else { return };
                // The label that was shown, with its id: the canvas's cache already has
                // its node.
                let mut label = editing.shown.clone();
                label.z = scene.top_z().saturating_add(1);
                if let Geometry::Text { text: shown, width, .. } = &mut label.geometry {
                    *shown = text;
                    *width = editing.effective_width;
                }
                Some(Command::Add(label))
            }
            // An existing label emptied is a deletion. §4.5's "commit if non-empty" read
            // the other way: an empty commit of something that used to say something is
            // the user removing it, and leaving an invisible object behind would be worse
            // -- it would still be selectable and still be in `objects.json`.
            (Some(before), true) => Some(Command::Remove(*before)),
            (Some(before), false) => {
                // The label as it was shown, so what the options row changed while it was
                // typed is kept with the text: its colour, background, size, style and
                // alignment (D198). From `before`, they went with the edit.
                let mut after = editing.shown.clone();
                if let Geometry::Text { text: existing, width, .. } = &mut after.geometry {
                    *existing = text;
                    // A label that grew to the canvas's edge while being edited keeps
                    // the wrap it was given there.
                    if width.is_none() {
                        *width = editing.effective_width;
                    }
                }
                (after != *before).then(|| Command::Change {
                    id: before.id,
                    before,
                    after: Box::new(after),
                })
            }
        };
        let Some(command) = command else {
            tracing::debug!("text edit closed with nothing to keep");
            return;
        };
        let subject = command.subject();
        self.commit(command);
        if let Some(id) = subject
            && select
        {
            self.canvas.set_selection(vec![id]);
        }
    }

    /// §4.1: "Double-click a text object edits it."
    pub(super) fn edit_text_under(self: &Rc<Self>, at: Point) -> bool {
        let Some(scene) = self.canvas.scene() else { return false };
        let Some(id) = octosnap_scene::hit::topmost(&scene, at) else { return false };
        let Some(object) = scene.get(id) else { return false };
        if !matches!(object.geometry, Geometry::Text { .. }) {
            return false;
        }
        let object = object.clone();
        let pos = match object.geometry {
            Geometry::Text { pos, .. } => pos,
            _ => return false,
        };
        self.begin_text_edit(pos, None, Some(object));
        true
    }
}

/// The document's font on a `GtkTextView`, as a `GtkTextTag`.
///
/// A tag and not a CSS provider. `GtkWidget::style_context` has been deprecated since
/// 4.10 and GTK 4 has no per-widget provider to replace it, so the alternative would be a
/// display-wide stylesheet carrying a generated class per font size -- for a widget that
/// lives only as long as someone is typing.
///
/// The tag carries [`font_for`]'s **exact** description, which is the one the renderer
/// uses. That is what puts the caret where the canvas draws the letters: the view and the
/// label under it are laid out by the same font at the same size. Its glyphs are
/// transparent -- the label's are the ones seen (D197).
///
/// The tag is re-applied as text arrives -- a tag covers a range, and a range does not
/// grow when the user types past the end of it.
fn apply_style(view: &gtk::TextView, style: TextStyle, size: f64) {
    let buffer = view.buffer();
    let tag = gtk::TextTag::new(Some("octosnap-text"));
    tag.set_font_desc(Some(&font_for(style, size)));
    tag.set_foreground_rgba(Some(&gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.0)));
    buffer.tag_table().add(&tag);

    let restyle = move |buffer: &gtk::TextBuffer| {
        let (start, end) = (buffer.start_iter(), buffer.end_iter());
        buffer.apply_tag_by_name("octosnap-text", &start, &end);
    };
    restyle(&buffer);
    buffer.connect_changed(restyle);
}

/// `spec/05` §5.1's `created`, in seconds since the epoch.
fn now_seconds() -> u64 {
    glib::real_time().unsigned_abs() / 1_000_000
}
