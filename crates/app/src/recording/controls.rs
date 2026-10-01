// SPDX-License-Identifier: GPL-3.0-or-later
//! `spec/06` §3's controls pill: a small window with the elapsed time, Stop and Trash,
//! placed by the extension *outside* the recorded rectangle (role `recorder`) so it never
//! ends up in the GIF. A GIF has no pause, resume, restart or audio (D68), so the pill is
//! three things wide.
//!
//! The window is mapped but never `present()`ed, like a QAO card (`spec/01` §5's
//! "never focus-steal"): the extension positions it and it accepts clicks where it lands.

use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;

use octosnap_media::text;

/// `spec/09` §3: the controls fade in over 150 ms.
const APPEAR_MS: u32 = 150;

/// The recorder's on-screen controls. Owns one undecorated window.
#[derive(Debug)]
pub struct Pill {
    window: gtk::ApplicationWindow,
    time: gtk::Label,
}

impl Pill {
    /// Builds and maps the pill. `on_stop` and `on_trash` fire when their buttons are
    /// clicked; the controller decides what they mean (Trash confirms first).
    pub fn new(
        app: &adw::Application,
        on_stop: impl Fn() + 'static,
        on_trash: impl Fn() + 'static,
    ) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("octosnap-recorder")
            .decorated(false)
            .resizable(false)
            .deletable(false)
            .build();
        window.add_css_class("octosnap-recorder");

        // The room round the controls is the stylesheet's padding, not margins: the dark
        // shape is this box, and a margin is outside it, so the buttons met its edge inside
        // a transparent ring that took the clicks meant for what was under it (D152).
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);

        // The red dot `spec/06` §3 puts beside the timer, matching the panel indicator.
        let dot = gtk::Image::from_icon_name("media-record-symbolic");
        dot.add_css_class("octosnap-rec-dot");
        dot.set_can_target(false);

        let time = gtk::Label::new(Some(&text::elapsed_label(0)));
        time.add_css_class("octosnap-rec-time");
        time.set_width_chars(5);

        let stop = gtk::Button::from_icon_name("media-playback-stop-symbolic");
        stop.set_tooltip_text(Some("Stop"));
        stop.add_css_class("octosnap-rec-stop");
        stop.connect_clicked(move |_| on_stop());

        let trash = gtk::Button::from_icon_name("user-trash-symbolic");
        trash.set_tooltip_text(Some("Discard"));
        trash.add_css_class("octosnap-rec-trash");
        trash.connect_clicked(move |_| on_trash());

        row.append(&dot);
        row.append(&time);
        row.append(&stop);
        row.append(&trash);
        window.set_child(Some(&row));

        // `spec/09` §3's "Recorder | controls appear | 150 ms | ease-out", played on map
        // because libadwaita skips an animation on a widget that is not mapped yet. The
        // map handler holding the animation is not a cycle: an animation holds its widget
        // and its target's object only weakly. With animations off it lands at once.
        window.set_opacity(0.0);
        let target = adw::PropertyAnimationTarget::new(&window, "opacity");
        let appear = adw::TimedAnimation::builder()
            .widget(&window)
            .value_from(0.0)
            .value_to(1.0)
            .duration(APPEAR_MS)
            .easing(adw::Easing::EaseOutCubic)
            .target(&target)
            .build();
        window.connect_map(move |_| appear.play());

        // Mapped, not presented: the extension gives it a place and it takes clicks there
        // without ever stealing focus from what the user is recording.
        window.set_visible(true);

        Rc::new(Self { window, time })
    }

    /// The GTK object path the extension addresses this window by (`docs/spikes/01-02`),
    /// the same shape a card uses.
    #[must_use]
    pub fn object_path(&self) -> Option<String> {
        let app = self.window.application()?;
        let base = app.dbus_object_path()?;
        Some(format!("{base}/window/{}", self.window.id()))
    }

    /// The window, so the confirmation dialog has a parent to present on.
    #[must_use]
    pub fn window(&self) -> &gtk::ApplicationWindow {
        &self.window
    }

    /// Updates the timer to the recording's own clock.
    pub fn set_elapsed(&self, elapsed: Duration) {
        let ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        self.time.set_text(&text::elapsed_label(ms));
    }

    /// Takes the pill off the screen. Idempotent.
    pub fn close(&self) {
        self.window.destroy();
    }
}

impl Drop for Pill {
    fn drop(&mut self) {
        // A pill dropped without `close` -- the controller was torn down -- must not leave
        // a window behind.
        self.window.destroy();
    }
}
