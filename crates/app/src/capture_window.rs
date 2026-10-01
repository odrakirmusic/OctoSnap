// SPDX-License-Identifier: GPL-3.0-or-later

//! The M0 capture window: `spec/11` M0 task 3's "plain window with the PNG".
//!
//! Deliberately not the Quick Access Overlay. The QAO is M2 and needs the extension to
//! place it (`spec/04`); this is an ordinary window whose only job is to prove the
//! capture crossed the boundary intact. It also prints the geometry it was handed,
//! because the single most common class of bug in this domain is a coordinate bug
//! (`spec/01` §1) and M0 is the cheapest place to catch one.

use adw::prelude::*;
use octosnap_core::CaptureResult;
use tracing::warn;

pub fn show(app: &adw::Application, capture: &CaptureResult) {
    let (physical_width, physical_height) = capture.rect.to_physical(capture.scale);

    if !capture.path.exists() {
        warn!(path = %capture.path.display(), "capture PNG does not exist on disk");
    }

    let picture = crate::history::thumbnail::picture_of(&capture.path);
    picture.set_can_shrink(true);
    picture.set_content_fit(gtk::ContentFit::ScaleDown);
    picture.set_vexpand(true);

    let details = format!(
        "{mode} · {display} · logical {lw}x{lh} at ({lx},{ly}) · scale {scale} · \
         expecting {pw}x{ph} physical",
        mode = capture.mode.as_wire(),
        display = if capture.display.is_empty() { "?" } else { &capture.display },
        lw = capture.rect.width,
        lh = capture.rect.height,
        lx = capture.rect.x,
        ly = capture.rect.y,
        scale = capture.scale,
        pw = physical_width,
        ph = physical_height,
    );

    let caption = gtk::Label::builder()
        .label(&details)
        .wrap(true)
        .xalign(0.0)
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    caption.add_css_class("dim-label");
    caption.add_css_class("caption");

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&picture);
    body.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    body.append(&caption);

    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&body));

    // Open at the capture's logical size where that is sane, so 1 logical unit shows as
    // 1 screen point, clamped so a fullscreen capture does not open a fullscreen window.
    let default_width = capture.rect.width.clamp(360, 1280);
    let default_height = capture.rect.height.clamp(240, 800);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("OctoSnap capture")
        .default_width(default_width)
        .default_height(default_height)
        .content(&view)
        .build();

    // present() is correct here and *only* here. The QAO and pins must never call it
    // (`spec/01` §5, "Never focus-steal"); they are shown with set_visible and raised by
    // the extension. This window is a normal, user-facing window, so it may focus.
    window.present();
}

/// Formats a capture for the log in one line, used by the service handler.
#[must_use]
pub fn describe(capture: &CaptureResult) -> String {
    let (pw, ph) = capture.rect.to_physical(capture.scale);
    format!(
        "{} {}x{}@{} -> {}x{} physical, {}",
        capture.mode.as_wire(),
        capture.rect.width,
        capture.rect.height,
        capture.scale,
        pw,
        ph,
        capture.path.display()
    )
}
