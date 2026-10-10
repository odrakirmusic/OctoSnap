// SPDX-License-Identifier: GPL-3.0-or-later

//! `ANN-26` (D178): Redact Sensitive Info, the Pixelate tool's button.
//!
//! The picture is read on a worker, as a text capture is, and every e-mail address, phone
//! number, IBAN, card number, IP address, key, token and password `octosnap_ocr::sensitive`
//! finds is covered with an ordinary redaction in the Pixelate tool's current style. They
//! arrive as one undo step and selected, so a wrong one is a Delete away and all of them a
//! Ctrl+Z.
//!
//! Each redaction is made the way a drag makes one, through [`Draft`], so it is the object
//! the tool would have drawn over the same rectangle: its seed, its style and its raster
//! are the tool's.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use octosnap_ocr::sensitive::{self, Found, Kind};
use octosnap_scene::tool::{Draft, Modifiers, Tool};
use octosnap_scene::{Bounds, Command, Geometry, Point};
use tracing::{debug, info};

use super::window::Editor;

/// The window action, which the button, the canvas menu and the harness all run.
pub(super) const ACTION: &str = "auto-redact";

impl Editor {
    /// Installs `win.auto-redact`.
    pub(super) fn install_auto_redact(self: &Rc<Self>) {
        let action = gio::SimpleAction::new(ACTION, None);
        let editor = Rc::downgrade(self);
        action.connect_activate(move |action, _| {
            if let Some(editor) = editor.upgrade() {
                editor.auto_redact(action);
            }
        });
        self.window.add_action(&action);
    }

    /// The Pixelate tool's button for it.
    pub(super) fn auto_redact_button(&self) -> gtk::Widget {
        let button = gtk::Button::with_label("Redact Sensitive Info");
        button.set_action_name(Some(&format!("win.{ACTION}")));
        button.set_tooltip_text(Some(
            "Cover e-mail addresses, phone numbers, card numbers, IP addresses, keys and \
             passwords",
        ));
        button.upcast()
    }

    /// Reads the picture and covers what it finds. The action is off while it reads, so a
    /// second press cannot cover everything twice.
    fn auto_redact(self: &Rc<Self>, action: &gio::SimpleAction) {
        if !crate::ocr::ready() {
            info!(found = 0, why = "no language pack", "auto-redact");
            let toast = adw::Toast::new(crate::flow::NO_PACK);
            toast.set_timeout(super::actions::toast_seconds(true));
            toast.set_button_label(Some("Language Packs"));
            toast.set_action_name(Some("app.open-settings"));
            toast.set_action_target_value(Some(&"language-packs".to_variant()));
            self.toasts.add_toast(toast);
            return;
        }
        let (Some(pixels), Some(per_unit)) =
            (self.canvas.base_pixels(), self.canvas.base_pixel_scale())
        else {
            return;
        };
        let script = crate::settings::ocr_config().script;
        let (width, height, rgba) = (pixels.width, pixels.height, pixels.rgba.clone());
        action.set_enabled(false);
        let started = std::time::Instant::now();
        let editor = Rc::downgrade(self);
        let action = action.clone();
        glib::spawn_future_local(async move {
            let read = crate::ocr::words(width, height, rgba, script).await;
            action.set_enabled(true);
            let Some(editor) = editor.upgrade() else { return };
            match read {
                Ok(words) => editor.cover(&sensitive::find(&words), per_unit, started),
                Err(why) => {
                    info!(found = 0, why, "auto-redact");
                    editor.toast(&format!("Could not read the picture: {why}"), None);
                }
            }
        });
    }

    /// One redaction for each find, as one undo step, selected.
    fn cover(self: &Rc<Self>, found: &[Found], per_unit: f64, started: std::time::Instant) {
        let Some(scene) = self.canvas.scene() else { return };
        // The finds are in the stored image's pixels, and the document may have been
        // turned since (`spec/05` §4.12): through its units, then its orientation, as the
        // highlighter's snap comes back.
        let orientation = scene.base.orientation;
        let (width, height) = (scene.base.width, scene.base.height);
        let to_document = |x: i32, y: i32| {
            let at = Point::new(f64::from(x) / per_unit, f64::from(y) / per_unit);
            orientation.place(at, width, height)
        };
        // A second press covers what the first did not, and nothing twice: a find already
        // under a redaction, found or drawn, is left alone.
        let redactions: Vec<Bounds> = scene
            .objects()
            .iter()
            .filter_map(|object| match object.geometry {
                Geometry::Redact { bounds, .. } => Some(bounds.normalised().inflated(1.0)),
                _ => None,
            })
            .collect();
        let settings = self.memory_of(Tool::Pixelate);
        let created = super::tools::now_seconds();
        let mut z = scene.top_z();
        let mut commands = Vec::new();
        let mut ids = Vec::new();
        let mut kinds: Vec<Kind> = Vec::new();
        let mut already = 0_usize;
        for find in found {
            let b = find.bounds;
            let (near, far) = (to_document(b.left(), b.top()), to_document(b.right(), b.bottom()));
            if redactions.iter().any(|r| r.contains(near) && r.contains(far)) {
                already += 1;
                continue;
            }
            let Some(mut draft) = Draft::begin(Tool::Pixelate, settings, near, Modifiers::none())
            else {
                continue;
            };
            draft.extend(far, Modifiers::none());
            z = z.saturating_add(1);
            let Some(object) = draft.object(z, created, scene.next_counter_number()) else {
                continue;
            };
            // In the picture's pixels, for `redact-test.sh` and for whoever asks why.
            debug!(kind = ?find.kind, x = b.x, y = b.y, width = b.width, height = b.height, "auto-redact covers");
            ids.push(object.id);
            commands.push(Command::Add(object));
            kinds.push(find.kind);
        }
        let ms = started.elapsed().as_millis();
        // Read by `editor-test.sh`.
        info!(found = found.len(), covered = commands.len(), already, ?kinds, ms, "auto-redact");
        if commands.is_empty() {
            let title = if already > 0 {
                "Everything found is covered already"
            } else {
                "Nothing sensitive found"
            };
            self.toast(title, None);
            return;
        }
        let covered = commands.len();
        self.commit_batch(commands);
        self.canvas.set_selection(ids);
        self.toast(&summary(covered, &kinds), None);
    }
}

/// "Covered 3: 2 e-mail addresses, 1 phone number".
fn summary(covered: usize, kinds: &[Kind]) -> String {
    let mut counted: Vec<(Kind, usize)> = Vec::new();
    for &kind in kinds {
        match counted.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, n)) => *n += 1,
            None => counted.push((kind, 1)),
        }
    }
    let parts: Vec<String> =
        counted
            .into_iter()
            .map(|(kind, n)| {
                if n == 1 {
                    format!("1 {}", kind.label())
                } else {
                    format!("{n} {}", kind.plural())
                }
            })
            .collect();
    format!("Covered {covered}: {}", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_toast_counts_each_kind() {
        let kinds = [Kind::Email, Kind::Phone, Kind::Email, Kind::Ip];
        assert_eq!(
            summary(4, &kinds),
            "Covered 4: 2 e-mail addresses, 1 phone number, 1 IP address"
        );
        assert_eq!(summary(2, &[Kind::Iban, Kind::Iban]), "Covered 2: 2 IBANs");
        assert_eq!(summary(1, &[Kind::Secret]), "Covered 1: 1 key or token");
        assert_eq!(summary(2, &[Kind::Secret, Kind::Secret]), "Covered 2: 2 keys or tokens");
    }
}
