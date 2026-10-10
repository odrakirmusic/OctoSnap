// SPDX-License-Identifier: GPL-3.0-or-later

//! `ANN-27` (D180): the picture's text, selected, copied and followed in the editor.
//!
//! Select Text, beside the canvas actions and in the canvas menu, reads the picture as
//! Redact Sensitive Info does, and shows its words faintly. Until it is turned off, or a
//! tool is chosen, a drag over the picture selects words rather than drawing: Ctrl+C copies
//! them, Ctrl+A takes them all, Escape lets go, and a click on a link or an address opens
//! it. The words come from `octosnap_ocr::live`, in the stored picture's pixels, and go
//! through the document's scale and orientation both ways, so a turned document selects
//! as it reads. Only the words the export shows take part (D188): one off the canvas or
//! outside a background's frame, under a redaction, a filled box, a pasted image, a label's
//! plate, a badge or a lens, or outside the shapes of a spotlight at full opacity, is not
//! marked, selected, copied or followed, as Copy Text, which reads the export, never sees
//! it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use octosnap_ocr::live::{Page, Pos};
use octosnap_scene::{Bounds, Orientation, Point, Visible};
use tracing::{debug, info, warn};

use super::canvas::TextMark;
use super::cursor::Hover;
use super::tools::CLICK_SLOP;
use super::window::Editor;

/// The window action, a switch, which the button and the canvas menu show.
pub(super) const ACTION: &str = "live-text";

/// Live text, while an editor is open.
#[derive(Debug, Default)]
pub struct LiveText {
    /// Whether a drag selects text.
    on: Cell<bool>,
    /// A read is under way.
    reading: Cell<bool>,
    /// The picture's words, once read, and the stored picture they were read from (its
    /// file). Kept when the mode is left, so coming back to it does not read the picture
    /// again, unless a cut has written it anew since (D188).
    page: RefCell<Option<(String, Page)>>,
    /// The words the export shows (D188), and what they were worked out from.
    shown: RefCell<Option<(Mask, Rc<Page>)>>,
    /// The selected words: from the press, to where the drag is.
    selection: Cell<Option<(Pos, Pos)>>,
}

/// What the words the export shows were worked out from: the stored picture, where it is
/// on the document, and what the export shows of it (D188).
#[derive(Debug, Clone, PartialEq)]
struct Mask {
    file: String,
    orientation: Orientation,
    size: (f64, f64),
    per_unit: f64,
    visible: Visible,
}

impl Editor {
    /// Installs `win.live-text`.
    pub(super) fn install_live_text(self: &Rc<Self>) {
        let action = gio::SimpleAction::new_stateful(ACTION, None, &false.to_variant());
        let editor = Rc::downgrade(self);
        action.connect_change_state(move |action, value| {
            let Some(on) = value.and_then(glib::Variant::get::<bool>) else { return };
            action.set_state(&on.to_variant());
            if let Some(editor) = editor.upgrade() {
                editor.live_text_changed(on);
            }
        });
        self.window.add_action(&action);
    }

    /// The button beside the canvas actions.
    pub(super) fn live_text_button(&self) -> gtk::Widget {
        let button = gtk::ToggleButton::builder()
            .icon_name("live-text-symbolic")
            .tooltip_text("Select Text in the Picture")
            .action_name(format!("win.{ACTION}"))
            .build();
        button.upcast()
    }

    /// Ctrl+Shift+T.
    pub(super) fn toggle_live_text(self: &Rc<Self>) {
        let on = self.live.on.get();
        self.window.change_action_state(ACTION, &(!on).to_variant());
    }

    pub(super) fn live_text_on(&self) -> bool {
        self.live.on.get()
    }

    /// Turns the mode off, as choosing a tool or a crop does.
    pub(super) fn leave_live_text(&self) {
        if self.live.on.get() {
            self.window.change_action_state(ACTION, &false.to_variant());
        }
    }

    fn live_text_changed(self: &Rc<Self>, on: bool) {
        if !on {
            let was = self.live.on.replace(false);
            self.live.selection.set(None);
            self.canvas.set_text_marks(Vec::new());
            if was {
                self.set_hover(Hover::Idle);
                // Read by `redact-test.sh`.
                info!("live text off");
            }
            return;
        }
        if !crate::ocr::ready() {
            info!(why = "no language pack", "live text");
            self.window.change_action_state(ACTION, &false.to_variant());
            let toast = adw::Toast::new(crate::flow::NO_PACK);
            toast.set_timeout(super::actions::toast_seconds(true));
            toast.set_button_label(Some("Language Packs"));
            toast.set_action_name(Some("app.open-settings"));
            toast.set_action_target_value(Some(&"language-packs".to_variant()));
            self.toasts.add_toast(toast);
            return;
        }
        // The mode is a tool of its own: nothing is drawn while it is on, and a label being
        // typed is finished first, as a click elsewhere would finish it.
        if self.text_editing() {
            self.commit_text_edit();
        }
        self.set_tool(octosnap_scene::Tool::Select);
        self.deselect();
        self.live.on.set(true);
        let file = self.canvas.scene().map(|scene| scene.base.file);
        let read = self.live.page.borrow().as_ref().map(|(file, _)| file.clone());
        if read.is_some() && read == file {
            self.show_live_marks();
            return;
        }
        self.read_live_text();
    }

    fn read_live_text(self: &Rc<Self>) {
        if self.live.reading.replace(true) {
            return;
        }
        let (Some(pixels), Some(file)) =
            (self.canvas.base_pixels(), self.canvas.scene().map(|scene| scene.base.file))
        else {
            self.live.reading.set(false);
            return;
        };
        let script = crate::settings::ocr_config().script;
        let (width, height, rgba) = (pixels.width, pixels.height, pixels.rgba.clone());
        let started = std::time::Instant::now();
        let editor = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let read = crate::ocr::words(width, height, rgba, script).await;
            let Some(editor) = editor.upgrade() else { return };
            editor.live.reading.set(false);
            let ms = started.elapsed().as_millis();
            match read {
                Ok(words) => {
                    let page = Page::new(&words);
                    let count: usize = page.lines.iter().map(|l| l.words.len()).sum();
                    // Read by `redact-test.sh`.
                    info!(
                        lines = page.lines.len(),
                        words = count,
                        links = page.links.len(),
                        ms,
                        "live text"
                    );
                    let empty = page.is_empty();
                    *editor.live.page.borrow_mut() = Some((file, page));
                    *editor.live.shown.borrow_mut() = None;
                    if empty {
                        editor.toast("No text in the picture", None);
                        editor.leave_live_text();
                    } else if editor.live.on.get() {
                        editor.show_live_marks();
                    }
                }
                Err(why) => {
                    info!(why, "live text");
                    editor.toast(&format!("Could not read the picture: {why}"), None);
                    editor.leave_live_text();
                }
            }
        });
    }

    /// The words faintly, the selection strongly, and the links underlined.
    fn show_live_marks(&self) {
        let Some(page) = self.shown_page() else {
            self.canvas.set_text_marks(Vec::new());
            return;
        };
        let selected: Vec<Pos> =
            self.live.selection.get().map(|(a, b)| page.between(a, b)).unwrap_or_default();
        let mut marks = Vec::new();
        for (l, line) in page.lines.iter().enumerate() {
            for (w, word) in line.words.iter().enumerate() {
                let at = Pos { line: l, word: w };
                let b = word.bounds;
                let bounds = Bounds::new(
                    f64::from(b.x),
                    f64::from(b.y),
                    f64::from(b.width),
                    f64::from(b.height),
                );
                let mark = if selected.contains(&at) { TextMark::Selected } else { TextMark::Word };
                marks.push((bounds, mark));
                if page.link_at(at).is_some() {
                    marks.push((bounds, TextMark::Link));
                }
            }
        }
        self.canvas.set_text_marks(marks);
    }

    /// Where a document point is in the stored picture, in its pixels.
    fn to_picture(&self, at: Point) -> Option<(f64, f64)> {
        let scene = self.canvas.scene()?;
        let per_unit = self.canvas.base_pixel_scale()?;
        let base = &scene.base;
        let stored = base.orientation.unplace(at, base.width, base.height);
        Some((stored.x * per_unit, stored.y * per_unit))
    }

    /// The words the export shows: the page as read, without those it hides (D188,
    /// [`Visible`]). Worked out again when what it shows changes, as an undo with the mode
    /// on can change it, and the selection, which was of the old words, goes. `None` before
    /// the read, and for a page read from a picture a cut has since written anew.
    fn shown_page(&self) -> Option<Rc<Page>> {
        let scene = self.canvas.scene()?;
        let per_unit = self.canvas.base_pixel_scale()?;
        let base = &scene.base;
        let mask = Mask {
            file: base.file.clone(),
            orientation: base.orientation,
            size: (base.width, base.height),
            per_unit,
            // A label's plate as the canvas measures it, which the scene cannot.
            visible: scene.visible(|object| self.canvas.measured_bounds(object)),
        };
        if let Some((kept, page)) = self.live.shown.borrow().as_ref()
            && *kept == mask
        {
            return Some(Rc::clone(page));
        }
        let shown = {
            let read = self.live.page.borrow();
            let (_, page) = read.as_ref().filter(|(file, _)| *file == base.file)?;
            let place = |x: i32, y: i32| {
                let stored = Point::new(f64::from(x) / per_unit, f64::from(y) / per_unit);
                base.orientation.place(stored, base.width, base.height)
            };
            Rc::new(page.without(|word| {
                let b = word.bounds;
                let area =
                    Bounds::from_corners(place(b.x, b.y), place(b.x + b.width, b.y + b.height));
                mask.visible.hides(area)
            }))
        };
        let words: usize = shown.lines.iter().map(|l| l.words.len()).sum();
        // Read by `redact-test.sh`: how many, never which.
        debug!(words, links = shown.links.len(), "live text shows");
        if self.live.shown.replace(Some((mask, Rc::clone(&shown)))).is_some() {
            self.live.selection.set(None);
        }
        Some(shown)
    }

    /// A press with the mode on: the word under it is selected, and a drag from it, or
    /// from beside the words, selects from there. The word the drag starts from, or `None`
    /// while the picture is still being read.
    pub(super) fn live_press(&self, at: Point) -> Option<Pos> {
        let (x, y) = self.to_picture(at)?;
        let (on, from) = {
            let page = self.shown_page()?;
            let on = page.at(x, y);
            (on, on.or_else(|| page.nearest(x, y)))
        };
        self.live.selection.set(on.map(|pos| (pos, pos)));
        self.show_live_marks();
        from
    }

    /// The drag has reached `at`: the selection runs from where it started to there. A
    /// press beside the words that has not yet travelled a click's slop selects nothing,
    /// so a click there only lets go of the selection.
    pub(super) fn live_drag(&self, from: Pos, press: Point, at: Point) {
        if self.live.selection.get().is_none()
            && press.distance_to(at) * self.canvas.zoom() <= CLICK_SLOP
        {
            return;
        }
        let to = {
            let Some(page) = self.shown_page() else { return };
            let Some((x, y)) = self.to_picture(at) else { return };
            page.nearest(x, y)
        };
        if let Some(to) = to {
            self.live.selection.set(Some((from, to)));
            self.show_live_marks();
        }
    }

    /// The button came up. A click on a link opens it; anything else leaves the selection
    /// as the drag made it.
    pub(super) fn live_release(&self, press: Point, at: Point) {
        let travelled = press.distance_to(at) * self.canvas.zoom();
        let target = if travelled <= CLICK_SLOP {
            let picture = self.to_picture(press);
            self.shown_page().zip(picture).and_then(|(page, (x, y))| {
                page.at(x, y).and_then(|pos| page.link_at(pos)).map(|link| link.target.clone())
            })
        } else {
            None
        };
        if let Some(target) = target {
            self.live.selection.set(None);
            self.show_live_marks();
            self.open_link(&target);
            return;
        }
        if let Some((a, b)) = self.live.selection.get() {
            let words = self.shown_page().map_or(0, |page| page.between(a, b).len());
            info!(words, "live text selected");
        }
    }

    fn open_link(&self, target: &str) {
        // Which kind, not the address: the log is not the place for what was in the picture.
        let kind = if target.starts_with("mailto:") { "address" } else { "link" };
        info!(kind, "live text opens");
        let launcher = gtk::UriLauncher::new(target);
        launcher.launch(Some(&self.window), None::<&gio::Cancellable>, |result| {
            if let Err(e) = result {
                warn!("could not open what live text found: {e}");
            }
        });
    }

    /// Ctrl+C with the mode on and words selected: their text. Answers whether it copied,
    /// so that with nothing selected Ctrl+C copies the picture as it always does.
    pub(super) fn live_copy(&self) -> bool {
        if !self.live.on.get() {
            return false;
        }
        let Some((a, b)) = self.live.selection.get() else { return false };
        let text = {
            let Some(page) = self.shown_page() else { return false };
            page.text(a, b, crate::settings::ocr_config().breaks)
        };
        if text.is_empty() {
            return false;
        }
        super::tools::forget_copied_objects("the editor copied text");
        self.window.clipboard().set_text(&text);
        // Read by `redact-test.sh`.
        info!(chars = text.chars().count(), "live text copied");
        self.toast("Text copied", None);
        true
    }

    /// Ctrl+A with the mode on: every word.
    pub(super) fn live_select_all(&self) -> bool {
        if !self.live.on.get() {
            return false;
        }
        let all = self.shown_page().and_then(|page| page.all());
        self.live.selection.set(all);
        self.show_live_marks();
        true
    }

    /// Escape with the mode on: the selection first, then the mode. A toast that is showing
    /// takes an Escape before this does: libadwaita's overlay dismisses it.
    pub(super) fn live_escape(&self) -> bool {
        if !self.live.on.get() {
            return false;
        }
        let let_go = if self.live.selection.take().is_some() {
            self.show_live_marks();
            "selection"
        } else {
            self.leave_live_text();
            "mode"
        };
        // Read by `redact-test.sh`.
        debug!(let_go, "live text escape");
        true
    }

    /// The cursor over the picture with the mode on: a pointer over a link, an I-beam over
    /// a word.
    pub(super) fn live_hover(&self, at: Point) -> Option<Hover> {
        if !self.live.on.get() {
            return None;
        }
        let Some(page) = self.shown_page() else { return Some(Hover::Idle) };
        let Some((x, y)) = self.to_picture(at) else { return Some(Hover::Idle) };
        Some(match page.at(x, y) {
            Some(pos) if page.link_at(pos).is_some() => Hover::Link,
            Some(_) => Hover::Text,
            None => Hover::Idle,
        })
    }

    /// The link under a document point, for the canvas menu.
    pub(super) fn live_link_at(&self, at: Point) -> Option<String> {
        let (x, y) = self.to_picture(at)?;
        let page = self.shown_page()?;
        page.at(x, y).and_then(|pos| page.link_at(pos)).map(|link| link.target.clone())
    }

    /// The canvas menu's Open Link or Write an E-mail.
    pub(super) fn live_open_link_at(&self, at: Point) {
        if let Some(target) = self.live_link_at(at) {
            self.open_link(&target);
        }
    }

    /// Whether there are words selected, for the canvas menu.
    pub(super) fn live_has_selection(&self) -> bool {
        self.live.selection.get().is_some()
    }
}
