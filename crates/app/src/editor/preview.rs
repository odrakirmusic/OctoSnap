// SPDX-License-Identifier: GPL-3.0-or-later

//! The little pictures `spec/05` §2's options row is made of.
//!
//! Two of §2's controls are described in terms of what they *look* like rather than what
//! they contain: the colour button is "a menu button showing the current swatch", and the
//! size popover is "six diagonal stroke previews, thinnest at the top, thickest at the
//! bottom". Neither is an icon -- both depend on values chosen at runtime -- so both are
//! drawn.
//!
//! One widget with two modes rather than two widgets, because the boilerplate around a
//! `WidgetImpl` is most of the code and the two need the same of it: a size, a snapshot,
//! and a way to say "you are the selected one".

use std::cell::{Cell, RefCell};

use adw::prelude::*;
use gtk::glib;
use gtk::graphene;
use gtk::subclass::prelude::*;
use octosnap_scene::{Rgba, stroke_width};

/// The checker behind a translucent swatch, so 30 % red does not read as pink.
const CHECKER: f32 = 4.0;

/// What a preview is showing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// A colour chip, for the palette and the current-colour button.
    Color(Rgba),
    /// A diagonal stroke at a size level, for §2's size popover.
    Stroke(u8),
    /// An unfilled **My Colors** slot. §2: "drawn as **dashed empty circles** until
    /// filled".
    Empty,
    /// §2's "rainbow-wheel button that expands to the full picker".
    ///
    /// A real wheel, from `append_conic_gradient`. The first version borrowed the hue
    /// *bar* for the job, and a bar's minimum width is 160 px -- so the compact palette
    /// opened three times wider than its swatches and nothing in the swatch code was
    /// wrong.
    Wheel,
}

impl Default for Kind {
    fn default() -> Self {
        Self::Color(Rgba::new(0.0, 0.0, 0.0, 1.0))
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct Preview {
        pub kind: RefCell<Kind>,
        /// `spec/05` §2: the active palette swatch "carries a **ring** rather than a
        /// checkmark".
        pub active: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Preview {
        const NAME: &'static str = "OctosnapPreview";
        type Type = super::Preview;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Preview {
        fn constructed(&self) {
            self.parent_constructed();
            // Centred, so a chip stays round. Inside a `GtkButton` inside a vertical
            // `GtkBox` the default `Fill` stretched every swatch into a 250 px lozenge --
            // visible the first time the palette was photographed, and invisible to
            // `measure`, which was returning 20x20 quite correctly.
            let obj = self.obj();
            obj.set_halign(gtk::Align::Center);
            obj.set_valign(gtk::Align::Center);
        }
    }

    impl WidgetImpl for Preview {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            // A chip is square; a stroke preview is wide enough for the diagonal to read
            // as a diagonal rather than as a corner.
            let (w, h) = match *self.kind.borrow() {
                Kind::Color(_) | Kind::Empty | Kind::Wheel => (20, 20),
                Kind::Stroke(_) => (56, 22),
            };
            let size = if orientation == gtk::Orientation::Horizontal { w } else { h };
            (size, size, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            #[allow(clippy::cast_possible_truncation)]
            let (w, h) = (widget.width() as f32, widget.height() as f32);
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            let bounds = graphene::Rect::new(0.0, 0.0, w, h);
            let accent = adw::StyleManager::default().accent_color_rgba();

            match *self.kind.borrow() {
                Kind::Color(color) => {
                    let radius = w.min(h) / 2.0;
                    let round = gtk::gsk::RoundedRect::from_rect(bounds, radius);
                    snapshot.push_rounded_clip(&round);
                    if color.a < 1.0 {
                        draw_checker(snapshot, w, h);
                    }
                    snapshot.append_color(&to_rgba(color), &bounds);
                    snapshot.pop();
                    // A hairline on every chip, not only the active one: white on white
                    // is otherwise invisible, and white is the tenth palette entry.
                    let edge = gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.22);
                    snapshot.append_border(&round, &[1.0; 4], &[edge; 4]);
                    if self.active.get() {
                        // Outside the chip, so the colour is not covered by its own
                        // indicator -- which is what a checkmark would do.
                        let ring = graphene::Rect::new(-2.0, -2.0, w + 4.0, h + 4.0);
                        snapshot.append_border(
                            &gtk::gsk::RoundedRect::from_rect(ring, radius + 2.0),
                            &[2.0; 4],
                            &[accent; 4],
                        );
                    }
                }
                Kind::Wheel => {
                    let radius = w.min(h) / 2.0;
                    let centre = graphene::Point::new(w / 2.0, h / 2.0);
                    let ring = gtk::gsk::RoundedRect::from_rect(bounds, radius);
                    snapshot.push_rounded_clip(&ring);
                    let stops: Vec<gtk::gsk::ColorStop> = (0..=6u8)
                        .map(|i| {
                            gtk::gsk::ColorStop::new(
                                f32::from(i) / 6.0,
                                to_rgba(Rgba::from_hsv(f64::from(i) * 60.0, 1.0, 1.0, 1.0)),
                            )
                        })
                        .collect();
                    snapshot.append_conic_gradient(&bounds, &centre, 0.0, &stops);
                    snapshot.pop();
                    let edge = gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.22);
                    snapshot.append_border(&ring, &[1.0; 4], &[edge; 4]);
                }
                Kind::Empty => {
                    // A dashed ring and nothing inside it, so an empty slot reads as a
                    // place for a colour rather than as a white one -- which is what a
                    // plain outline would look like next to the palette's white swatch.
                    let radius = w.min(h) / 2.0 - 1.0;
                    let centre = graphene::Point::new(w / 2.0, h / 2.0);
                    let builder = gtk::gsk::PathBuilder::new();
                    builder.add_circle(&centre, radius);
                    let spec = gtk::gsk::Stroke::new(1.0);
                    spec.set_dash(&[2.5, 2.5]);
                    let mut ink = widget.color();
                    ink.set_alpha(0.45);
                    snapshot.append_stroke(&builder.to_path(), &spec, &ink);
                }
                Kind::Stroke(level) => {
                    // §2: "the active one filled in the accent colour".
                    let color = if self.active.get() {
                        accent
                    } else {
                        widget.color()
                    };
                    #[allow(clippy::cast_possible_truncation)]
                    let width = stroke_width(level) as f32;
                    let inset = width / 2.0 + 2.0;
                    let builder = gtk::gsk::PathBuilder::new();
                    builder.move_to(inset, h - inset);
                    builder.line_to(w - inset, inset);
                    let spec = gtk::gsk::Stroke::new(width);
                    spec.set_line_cap(gtk::gsk::LineCap::Round);
                    snapshot.append_stroke(&builder.to_path(), &spec, &color);
                }
            }
        }
    }

    fn draw_checker(snapshot: &gtk::Snapshot, w: f32, h: f32) {
        let light = gtk::gdk::RGBA::new(0.85, 0.85, 0.86, 1.0);
        let dark = gtk::gdk::RGBA::new(0.70, 0.70, 0.72, 1.0);
        snapshot.append_color(&light, &graphene::Rect::new(0.0, 0.0, w, h));
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let cols = (w / CHECKER).ceil() as i32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let rows = (h / CHECKER).ceil() as i32;
        for row in 0..rows {
            for col in 0..cols {
                if (row + col) % 2 == 0 {
                    continue;
                }
                #[allow(clippy::cast_precision_loss)]
                snapshot.append_color(
                    &dark,
                    &graphene::Rect::new(col as f32 * CHECKER, row as f32 * CHECKER, CHECKER, CHECKER),
                );
            }
        }
    }

    fn to_rgba(color: Rgba) -> gtk::gdk::RGBA {
        #[allow(clippy::cast_possible_truncation)]
        gtk::gdk::RGBA::new(color.r as f32, color.g as f32, color.b as f32, color.a as f32)
    }
}

glib::wrapper! {
    /// A colour chip or a stroke preview.
    pub struct Preview(ObjectSubclass<imp::Preview>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Preview {
    #[must_use]
    pub fn new(kind: Kind) -> Self {
        let preview: Self = glib::Object::builder().build();
        preview.set_kind(kind);
        preview
    }

    pub fn set_kind(&self, kind: Kind) {
        let changed = *self.imp().kind.borrow() != kind;
        if changed {
            *self.imp().kind.borrow_mut() = kind;
            // `queue_resize` and not `queue_draw`: a chip and a stroke preview measure
            // differently, so a mode change is a size change.
            self.queue_resize();
        }
    }

    pub fn set_active(&self, active: bool) {
        if self.imp().active.get() != active {
            self.imp().active.set(active);
            self.queue_draw();
        }
    }
}
