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
use octosnap_scene::{Command, Geometry, Object, ObjectId, Point, TextAlign};

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
            None => (settings.text, settings.font_size, TextAlign::default(), width, String::new()),
        };
        let color = subject.as_ref().map_or(settings.style.color, |object| object.style.color);

        let view = gtk::TextView::new();
        view.set_wrap_mode(if width.is_some() {
            gtk::WrapMode::WordChar
        } else {
            gtk::WrapMode::None
        });
        view.buffer().set_text(&text);
        view.set_monospace(style.is_monospaced());
        // The document's font at the document's size. The zoom is applied as a transform
        // on the layer, not by scaling the font, so the glyphs the user types are laid out
        // exactly as `Canvas::text_layout` will lay them out when the edit commits --
        // otherwise the text jumps the moment they press Enter.
        apply_style(&view, style, font_size, color);
        // The view *is* the object while it is open: the plate is its background, drawn
        // by `install_text_css` with the object's colour and radius, and the plate's
        // padding is the view's margins -- in document units, like everything under the
        // layer's transform. The first version had a themed white text view of 24 px
        // that scrolled the text out of sight as it was typed; the user's words were
        // "the basic box is so small it cuts off text" and "background is not fitting".
        view.add_css_class("octosnap-text-edit");
        #[allow(clippy::cast_possible_truncation)]
        let pad = (style.plate_padding_em() * font_size).round() as i32;
        view.set_left_margin(pad);
        view.set_right_margin(pad);
        view.set_top_margin(pad);
        view.set_bottom_margin(pad);
        view.set_pixels_above_lines(0);
        view.set_pixels_below_lines(0);
        self.install_text_css(style, font_size, color);

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

        // The object being re-edited is blanked in the *displayed* scene while the
        // overlay stands in for it, or the same text draws twice -- once by the canvas and
        // once by the `TextView` above it. The history is untouched: `commit_text_edit`
        // puts the original back before applying anything, so an undo has the real object
        // to return to.
        if let Some(object) = &subject {
            self.blank_text(object.id);
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
        });
        // Sized to its text now and after every keystroke, from the renderer's own layout.
        self.fit_text_edit();
        {
            let editor = Rc::downgrade(self);
            view.buffer().connect_changed(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.fit_text_edit();
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
        let caret_room = if text.is_empty() {
            editing.font_size * 1.5
        } else {
            editing.font_size * 0.6
        };
        let natural = self
            .canvas
            .layout_for(shown, editing.style, editing.font_size, editing.align, None)
            .pixel_size();
        let canvas_right = self
            .canvas
            .scene()
            .map_or(f64::INFINITY, |scene| scene.canvas.x + scene.canvas.width);
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
        let layout = self.canvas.layout_for(shown, editing.style, editing.font_size, editing.align, wrap);
        let (w, h) = layout.pixel_size();
        let width = wrap.unwrap_or(f64::from(w) + caret_room);
        #[allow(clippy::cast_possible_truncation)]
        editing.view.set_size_request(
            (width + 2.0 * pad).ceil() as i32,
            (f64::from(h) + 2.0 * pad).ceil() as i32,
        );
    }

    /// The overlay's plate, outline and caret, as a stylesheet for this one edit.
    ///
    /// A provider per edit rather than a tag background: a tag paints a rectangle behind
    /// each line, and §4.5's plate is one rounded rectangle behind all of them with
    /// padding around -- which is what the committed object draws, and the overlay has to
    /// look like the object it is standing in for. A dashed accent outline (outside the
    /// box, so it moves no glyph) says "this is being edited" on any picture; it is the
    /// one thing the object does not have.
    fn install_text_css(&self, style: TextStyle, font_size: f64, color: octosnap_scene::Rgba) {
        let Some(display) = gtk::gdk::Display::default() else { return };
        if let Some(old) = self.text_css.borrow_mut().take() {
            gtk::style_context_remove_provider_for_display(&display, &old);
        }
        let (plate, ink) = if style.has_plate() {
            (color.to_hex(), color.contrasting().to_hex())
        } else {
            ("00000000".to_owned(), color.to_hex())
        };
        let radius = style.plate_radius_em() * font_size;
        let css = format!(
            "textview.octosnap-text-edit, textview.octosnap-text-edit > text {{
                 background-color: transparent;
             }}
             textview.octosnap-text-edit {{
                 background-color: #{plate};
                 border-radius: {radius:.1}px;
                 outline: 1px dashed alpha(@accent_color, 0.85);
                 outline-offset: 3px;
                 caret-color: #{ink};
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

    /// Ends the edit, keeping the text if there is any (`spec/05` §4.5).
    pub(super) fn commit_text_edit(self: &Rc<Self>) {
        let Some(editing) = self.editing.borrow_mut().take() else { return };
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
                Some(Command::Add(Object::new(
                    scene.top_z().saturating_add(1),
                    now_seconds(),
                    self.settings().style,
                    Geometry::Text {
                        pos: editing.pos,
                        text,
                        style: editing.style,
                        font_size: editing.font_size,
                        align: editing.align,
                        width: editing.effective_width,
                    },
                )))
            }
            // An existing label emptied is a deletion. §4.5's "commit if non-empty" read
            // the other way: an empty commit of something that used to say something is
            // the user removing it, and leaving an invisible object behind would be worse
            // -- it would still be selectable and still be in `objects.json`.
            (Some(before), true) => Some(Command::Remove(*before)),
            (Some(before), false) => {
                let mut after = (*before).clone();
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
        if let Some(id) = subject {
            self.canvas.set_selection(vec![id]);
        }
    }

    /// Empties a text object in the displayed scene, without touching the history.
    ///
    /// `draw_text` returns early on empty text, so this is all it takes to make the object
    /// invisible while the overlay stands in for it.
    fn blank_text(&self, id: ObjectId) {
        let Some(mut scene) = self.canvas.scene() else { return };
        let Some(object) = scene.get(id) else { return };
        let mut blanked = object.clone();
        if let Geometry::Text { text, .. } = &mut blanked.geometry {
            text.clear();
        }
        scene.replace(blanked);
        self.canvas.update_scene(scene);
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

/// The document's font and colours on a `GtkTextView`, as a `GtkTextTag`.
///
/// A tag and not a CSS provider. `GtkWidget::style_context` has been deprecated since
/// 4.10 and GTK 4 has no per-widget provider to replace it, so the alternative would be a
/// display-wide stylesheet carrying a generated class per font size -- for a widget that
/// lives only as long as someone is typing.
///
/// The tag carries [`font_for`]'s **exact** description, which is the one the renderer
/// uses. That is what stops the glyphs jumping when the edit commits: the overlay and the
/// committed object are laid out by the same font at the same size, so the only thing that
/// changes on Enter is which widget is drawing them.
///
/// The tag is re-applied as text arrives -- a tag covers a range, and a range does not
/// grow when the user types past the end of it.
fn apply_style(view: &gtk::TextView, style: TextStyle, size: f64, color: octosnap_scene::Rgba) {
    let buffer = view.buffer();
    let tag = gtk::TextTag::new(Some("octosnap-text"));
    tag.set_font_desc(Some(&font_for(style, size)));
    // §4.5's auto-contrast applies while typing too: the plate is the view's own
    // background (`install_text_css`), so the glyphs are whichever of black or white
    // reads on it, exactly as the committed object draws them.
    if style.has_plate() {
        tag.set_foreground_rgba(Some(&to_gdk(color.contrasting())));
    } else {
        tag.set_foreground_rgba(Some(&to_gdk(color)));
    }
    buffer.tag_table().add(&tag);

    let restyle = move |buffer: &gtk::TextBuffer| {
        let (start, end) = (buffer.start_iter(), buffer.end_iter());
        buffer.apply_tag_by_name("octosnap-text", &start, &end);
    };
    restyle(&buffer);
    buffer.connect_changed(restyle);
}

fn to_gdk(color: octosnap_scene::Rgba) -> gtk::gdk::RGBA {
    #[allow(clippy::cast_possible_truncation)]
    gtk::gdk::RGBA::new(color.r as f32, color.g as f32, color.b as f32, color.a as f32)
}

/// `spec/05` §5.1's `created`, in seconds since the epoch.
fn now_seconds() -> u64 {
    glib::real_time().unsigned_abs() / 1_000_000
}

