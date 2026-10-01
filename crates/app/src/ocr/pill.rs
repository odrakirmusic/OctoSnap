// SPDX-License-Identifier: GPL-3.0-or-later

//! What a text capture puts on screen while it reads (`docs/decisions.md` D96).
//!
//! The same shape as `spec/06` §3's recorder bar and `spec/07` §1.1's scrolling controls:
//! a small dark pill in an undecorated window, mapped but never `present()`ed, positioned
//! by the extension's `PlaceWindow` (`spec/01` §5's "never focus-steal"). Three
//! differences, and each of them is the read's doing:
//!
//! 1. **It has no buttons.** The read runs in `gio::spawn_blocking` between two ONNX
//!    sessions that cannot be interrupted, so a Cancel would abandon the answer without
//!    stopping the work -- a button that lies about what it does.
//! 2. **It goes *inside* the rectangle.** The other two pills are kept out of theirs at
//!    real cost, because their pixels are still being taken; this one's were taken before
//!    it existed. The middle of the area the user just drew is the one place that is
//!    certainly on screen, certainly on the right monitor, and certainly where they are
//!    looking.
//! 3. **It is click-through.** Sitting over the page means sitting over whatever the user
//!    wants to do next, and it cannot offer them anything in exchange.

use std::rc::Rc;

use adw::prelude::*;
use octosnap_core::Rect;

/// `spec/07` §2.1's voice, in the present tense: the notification that follows says
/// "Text copied".
const SAYS: &str = "Reading text\u{2026}";

/// The reading indicator. Owns one undecorated window.
#[derive(Debug)]
pub struct Pill {
    window: gtk::ApplicationWindow,
}

impl Pill {
    /// Builds and maps the pill. It takes no input and answers to nothing; the only way
    /// it leaves the screen is [`Pill::close`].
    pub fn new(app: &adw::Application) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("octosnap-reader")
            .decorated(false)
            .resizable(false)
            .deletable(false)
            .build();
        window.add_css_class("octosnap-reader");

        // The room round the spinner and the words is the stylesheet's padding, not
        // margins: the dark shape is this box, and a margin is outside it (D152).
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);

        let spinner = adw::Spinner::new();
        spinner.set_size_request(16, 16);

        let says = gtk::Label::new(Some(SAYS));
        says.add_css_class("octosnap-reading");

        row.append(&spinner);
        row.append(&says);
        window.set_child(Some(&row));

        // Nothing here is clickable, so nothing here should swallow a click. An empty
        // input region hands every press to the window underneath -- which, since the
        // pill sits over the page that was just read, is where the user was working.
        window.connect_map(|window| {
            if let Some(surface) = window.surface() {
                surface.set_input_region(Some(&gtk::cairo::Region::create()));
            }
        });

        // Mapped, not presented, like the recorder's pill: the extension gives it a place
        // and it never takes focus from what the user is reading.
        window.set_visible(true);

        Rc::new(Self { window })
    }

    /// The GTK object path the extension addresses this window by (`docs/spikes/01-02`).
    #[must_use]
    pub fn object_path(&self) -> Option<String> {
        let app = self.window.application()?;
        let base = app.dbus_object_path()?;
        Some(format!("{base}/window/{}", self.window.id()))
    }

    /// The size the pill wants, for centring it on the area that was read.
    ///
    /// Asked of GTK rather than of the compositor for `place.ts`'s reason: a window that
    /// has just been mapped has no frame rect yet, and `PlaceWindow` would centre a
    /// nothing.
    #[must_use]
    pub fn size(&self) -> (i32, i32) {
        let natural = |orientation| self.window.measure(orientation, -1).1;
        (natural(gtk::Orientation::Horizontal), natural(gtk::Orientation::Vertical))
    }

    /// Takes the pill off the screen. Idempotent.
    pub fn close(&self) {
        self.window.destroy();
    }
}

impl Drop for Pill {
    fn drop(&mut self) {
        // A pill dropped without `close` -- the read was abandoned, or the app is going
        // down -- must not leave a window behind.
        self.window.destroy();
    }
}

/// Where a pill of `pill` goes so that it sits in the middle of `over`.
///
/// Whole pixels and no rounding bias: the compositor clamps the result to the work area
/// anyway (`place.ts`), so the arithmetic only has to put the pill where the eye is.
#[must_use]
pub fn centred(over: Rect, pill: (i32, i32)) -> (i32, i32) {
    (over.x + (over.width - pill.0) / 2, over.y + (over.height - pill.1) / 2)
}

/// Where the pill goes when there is nothing on screen to point at.
///
/// A read of a file (`spec/07` §2.1's `capture-text?filepath=`) has no rectangle at all,
/// and a read of a *card* has one that says where the pixels were taken from rather than
/// where they are now -- and a card is 207x125 pt, which this pill would cover whole. So
/// both land here: centred near the foot of the pointer's monitor, which is where GNOME
/// itself puts a transient word about something the system is doing.
///
/// An eighth of the work area rather than a fixed inset, so the pill sits in the same
/// place on a 4K panel as on a laptop screen.
#[must_use]
pub fn on_a_monitor(work_area: Rect, pill: (i32, i32)) -> (i32, i32) {
    (
        work_area.x + (work_area.width - pill.0) / 2,
        work_area.y + work_area.height - work_area.height / 8 - pill.1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pill_sits_in_the_middle_of_what_was_read() {
        assert_eq!(centred(Rect::new(100, 200, 400, 300), (160, 40)), (220, 330));
    }

    #[test]
    fn a_pill_wider_than_the_area_still_covers_its_middle() {
        // Half of it hangs off each side; Mutter pulls it back on screen from there.
        assert_eq!(centred(Rect::new(0, 0, 100, 40), (200, 40)), (-50, 0));
    }

    #[test]
    fn a_read_with_nowhere_to_point_puts_the_pill_near_the_foot_of_the_screen() {
        // A 1920x1080 panel under a 32 px shell panel.
        let (x, y) = on_a_monitor(Rect::new(0, 32, 1920, 1048), (200, 40));
        assert_eq!(x, 860, "the pill was not centred across the monitor");
        assert_eq!(y, 1080 - 131 - 40, "the pill was not near the foot of the monitor");
        assert!(y > 32 + 1048 / 2, "the pill drifted into the middle of the screen");
    }

    #[test]
    fn the_foot_of_a_second_monitor_is_measured_from_that_monitor() {
        let right = Rect::new(1920, 0, 2560, 1440);
        let (x, y) = on_a_monitor(right, (200, 40));
        assert!(x > 1920 && x + 200 < 1920 + 2560, "the pill left the monitor it belongs to");
        assert!(y > 1440 / 2 && y + 40 < 1440, "the pill left the monitor it belongs to");
    }
}
