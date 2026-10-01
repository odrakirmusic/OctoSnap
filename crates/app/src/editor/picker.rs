// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §2's colour picker, both stages.
//!
//! > This is a bespoke picker, not the system colour panel. On GTK that means building it
//! > rather than reaching for `Gtk.ColorDialog`: a `GtkPopover` containing a swatch grid,
//! > a custom HSV drawing area, two sliders, and the numeric row.
//!
//! **The HSV square is two linear gradients, not a bitmap.** The obvious build is a
//! `GtkDrawingArea` filling 256x256 pixels per hue change, and `spec/05` §6's rule against
//! per-frame pixel work is about the canvas -- so it would have been allowed, and it would
//! still have been wrong. The saturation-value square is exactly *white to the hue*
//! horizontally, multiplied by *transparent to black* vertically, which `append_linear_gradient`
//! draws twice on the GPU with no pixels touched and no cache to invalidate.
//!
//! **The hue is kept beside the colour, not derived from it.** A grey has no hue and
//! [`Rgba::to_hsv`] answers 0 for one, so a picker that re-reads the hue every time turns
//! any colour red the moment the value slider passes through black. The `hue` cell here is
//! the fix, and the test in `scene::style` states the trap.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use gtk::graphene;
use gtk::gsk;
use gtk::subclass::prelude::*;
use octosnap_scene::{PALETTE, Rgba};

use super::preview::{Kind, Preview};

/// `spec/05` §2: "a second column of **ten** My Colors slots".
pub const MY_COLORS: usize = 10;

// --- the saturation/value square -----------------------------------------------------

mod square {
    use super::*;

    mod imp {
        use super::*;

        #[derive(Debug, Default)]
        pub struct Square {
            pub hue: Cell<f64>,
            pub saturation: Cell<f64>,
            pub value: Cell<f64>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for Square {
            const NAME: &'static str = "OctosnapHsvSquare";
            type Type = super::Square;
            type ParentType = gtk::Widget;
        }

        impl ObjectImpl for Square {
            fn constructed(&self) {
                self.parent_constructed();
                self.value.set(1.0);
            }
        }

        impl WidgetImpl for Square {
            fn measure(&self, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
                (160, 160, -1, -1)
            }

            fn snapshot(&self, snapshot: &gtk::Snapshot) {
                let widget = self.obj();
                #[allow(clippy::cast_possible_truncation)]
                let (w, h) = (widget.width() as f32, widget.height() as f32);
                if w <= 0.0 || h <= 0.0 {
                    return;
                }
                let bounds = graphene::Rect::new(0.0, 0.0, w, h);
                let round = gsk::RoundedRect::from_rect(bounds, 6.0);
                snapshot.push_rounded_clip(&round);

                // White to the pure hue, left to right: that is saturation.
                let hue = to_gdk(Rgba::from_hsv(self.hue.get(), 1.0, 1.0, 1.0));
                snapshot.append_linear_gradient(
                    &bounds,
                    &graphene::Point::new(0.0, 0.0),
                    &graphene::Point::new(w, 0.0),
                    &[
                        gsk::ColorStop::new(0.0, gtk::gdk::RGBA::WHITE),
                        gsk::ColorStop::new(1.0, hue),
                    ],
                );
                // Transparent to black, top to bottom: that is value. Two gradients
                // composited is the whole square -- no bitmap, no per-hue cache.
                snapshot.append_linear_gradient(
                    &bounds,
                    &graphene::Point::new(0.0, 0.0),
                    &graphene::Point::new(0.0, h),
                    &[
                        gsk::ColorStop::new(0.0, gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.0)),
                        gsk::ColorStop::new(1.0, gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 1.0)),
                    ],
                );
                snapshot.pop();

                let edge = gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.2);
                snapshot.append_border(&round, &[1.0; 4], &[edge; 4]);

                // §2's "ring cursor". White with a dark inner ring, so it is visible on
                // white, on black and on a saturated hue -- the three places it goes.
                #[allow(clippy::cast_possible_truncation)]
                let cx = (self.saturation.get() as f32) * w;
                #[allow(clippy::cast_possible_truncation)]
                let cy = (1.0 - self.value.get() as f32) * h;
                for (radius, width, color) in [
                    (6.5, 2.0, gtk::gdk::RGBA::WHITE),
                    (8.0, 1.0, gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.45)),
                ] {
                    let builder = gsk::PathBuilder::new();
                    builder.add_circle(&graphene::Point::new(cx, cy), radius);
                    snapshot.append_stroke(
                        &builder.to_path(),
                        &gsk::Stroke::new(width),
                        &color,
                    );
                }
            }
        }
    }

    glib::wrapper! {
        pub struct Square(ObjectSubclass<imp::Square>)
            @extends gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl Square {
        #[must_use]
        pub fn new() -> Self {
            glib::Object::builder().build()
        }

        pub fn set_hsv(&self, hue: f64, saturation: f64, value: f64) {
            let imp = self.imp();
            imp.hue.set(hue);
            imp.saturation.set(saturation);
            imp.value.set(value);
            self.queue_draw();
        }

        /// Saturation and value from a point in the widget, clamped to it.
        ///
        /// Clamped rather than ignored: a drag that leaves the square should pin the
        /// cursor to the edge and keep tracking, which is what makes pure white and fully
        /// saturated both reachable without pixel-perfect aim.
        #[must_use]
        pub fn at(&self, x: f64, y: f64) -> (f64, f64) {
            let w = f64::from(self.width()).max(1.0);
            let h = f64::from(self.height()).max(1.0);
            ((x / w).clamp(0.0, 1.0), (1.0 - y / h).clamp(0.0, 1.0))
        }
    }
}

// --- the hue and alpha bars ----------------------------------------------------------

mod bar {
    use super::*;

    /// Which of `spec/05` §2's two sliders this is.
    #[derive(Debug, Clone, Copy, PartialEq, Default)]
    pub enum Mode {
        #[default]
        Hue,
        /// §2: "an alpha slider **over a checkerboard**", so the colour is needed too.
        Alpha(Rgba),
    }


    mod imp {
        use super::*;

        #[derive(Debug, Default)]
        pub struct Bar {
            pub mode: RefCell<Mode>,
            /// 0..1 along the bar, whatever the mode means by it.
            pub position: Cell<f64>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for Bar {
            const NAME: &'static str = "OctosnapColorBar";
            type Type = super::Bar;
            type ParentType = gtk::Widget;
        }

        impl ObjectImpl for Bar {}

        impl WidgetImpl for Bar {
            fn measure(&self, orientation: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
                let size = if orientation == gtk::Orientation::Horizontal { 160 } else { 18 };
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
                let round = gsk::RoundedRect::from_rect(bounds, h / 2.0);
                snapshot.push_rounded_clip(&round);
                match *self.mode.borrow() {
                    Mode::Hue => {
                        // Six stops plus the wrap back to red, which is the whole wheel.
                        let stops: Vec<gsk::ColorStop> = (0..=6u8)
                            .map(|i| {
                                let t = f32::from(i) / 6.0;
                                let hue = f64::from(i) * 60.0;
                                gsk::ColorStop::new(t, to_gdk(Rgba::from_hsv(hue, 1.0, 1.0, 1.0)))
                            })
                            .collect();
                        snapshot.append_linear_gradient(
                            &bounds,
                            &graphene::Point::new(0.0, 0.0),
                            &graphene::Point::new(w, 0.0),
                            &stops,
                        );
                    }
                    Mode::Alpha(color) => {
                        draw_checker(snapshot, w, h);
                        let mut clear = to_gdk(color);
                        clear.set_alpha(0.0);
                        snapshot.append_linear_gradient(
                            &bounds,
                            &graphene::Point::new(0.0, 0.0),
                            &graphene::Point::new(w, 0.0),
                            &[
                                gsk::ColorStop::new(0.0, clear),
                                gsk::ColorStop::new(1.0, to_gdk(color.opaque())),
                            ],
                        );
                    }
                }
                snapshot.pop();
                let edge = gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.2);
                snapshot.append_border(&round, &[1.0; 4], &[edge; 4]);

                // The same two-ring cursor as the square, for the same reason.
                #[allow(clippy::cast_possible_truncation)]
                let cx = (self.position.get() as f32).clamp(0.0, 1.0) * w;
                let centre = graphene::Point::new(cx.clamp(h / 2.0, w - h / 2.0), h / 2.0);
                for (radius, width, color) in [
                    (h / 2.0 - 2.0, 2.5, gtk::gdk::RGBA::WHITE),
                    (h / 2.0 - 0.5, 1.0, gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.45)),
                ] {
                    let builder = gsk::PathBuilder::new();
                    builder.add_circle(&centre, radius);
                    snapshot.append_stroke(&builder.to_path(), &gsk::Stroke::new(width), &color);
                }
            }
        }

        fn draw_checker(snapshot: &gtk::Snapshot, w: f32, h: f32) {
            const SIZE: f32 = 5.0;
            let light = gtk::gdk::RGBA::new(0.85, 0.85, 0.86, 1.0);
            let dark = gtk::gdk::RGBA::new(0.70, 0.70, 0.72, 1.0);
            snapshot.append_color(&light, &graphene::Rect::new(0.0, 0.0, w, h));
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let cols = (w / SIZE).ceil() as i32;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let rows = (h / SIZE).ceil() as i32;
            for row in 0..rows {
                for col in 0..cols {
                    if (row + col) % 2 == 0 {
                        continue;
                    }
                    #[allow(clippy::cast_precision_loss)]
                    snapshot.append_color(
                        &dark,
                        &graphene::Rect::new(col as f32 * SIZE, row as f32 * SIZE, SIZE, SIZE),
                    );
                }
            }
        }
    }

    glib::wrapper! {
        pub struct Bar(ObjectSubclass<imp::Bar>)
            @extends gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl Bar {
        #[must_use]
        pub fn new(mode: Mode) -> Self {
            let bar: Self = glib::Object::builder().build();
            *bar.imp().mode.borrow_mut() = mode;
            bar
        }

        pub fn set_mode(&self, mode: Mode) {
            if *self.imp().mode.borrow() != mode {
                *self.imp().mode.borrow_mut() = mode;
                self.queue_draw();
            }
        }

        pub fn set_position(&self, position: f64) {
            self.imp().position.set(position.clamp(0.0, 1.0));
            self.queue_draw();
        }

        #[must_use]
        pub fn at(&self, x: f64) -> f64 {
            (x / f64::from(self.width()).max(1.0)).clamp(0.0, 1.0)
        }
    }
}

fn to_gdk(color: Rgba) -> gtk::gdk::RGBA {
    #[allow(clippy::cast_possible_truncation)]
    gtk::gdk::RGBA::new(color.r as f32, color.g as f32, color.b as f32, color.a as f32)
}

// --- the picker itself ---------------------------------------------------------------

/// A callback the picker hands a colour to.
type ColorSink = RefCell<Option<Box<dyn Fn(Rgba)>>>;
/// A callback with nothing to say but "the eyedropper was pressed".
type Ping = RefCell<Option<Box<dyn Fn()>>>;

/// `spec/05` §2's two-stage colour control.
pub struct Picker {
    popover: gtk::Popover,
    stack: gtk::Stack,
    square: square::Square,
    hue_bar: bar::Bar,
    alpha_bar: bar::Bar,
    round: Preview,
    hex: gtk::Entry,
    fields: [gtk::SpinButton; 4],
    saved: gtk::Box,
    color: Cell<Rgba>,
    /// The hue the user chose, kept because a grey cannot answer for it.
    hue: Cell<f64>,
    /// Guards the fields against each other: every one of them writes the colour, and
    /// writing the colour rewrites all of them.
    syncing: Cell<bool>,
    on_change: ColorSink,
    on_pick: Ping,
}

impl Picker {
    /// Builds the popover. `with_alpha` follows §2's one row that has opacity.
    pub fn new(color: Rgba, with_alpha: bool, saved_colors: Vec<String>) -> Rc<Self> {
        let (hue, _, _) = color.to_hsv();
        let picker = Rc::new(Self {
            popover: gtk::Popover::new(),
            stack: gtk::Stack::new(),
            square: square::Square::new(),
            hue_bar: bar::Bar::new(bar::Mode::Hue),
            alpha_bar: bar::Bar::new(bar::Mode::Alpha(color)),
            round: Preview::new(Kind::Color(color)),
            hex: gtk::Entry::new(),
            fields: [
                gtk::SpinButton::with_range(0.0, 255.0, 1.0),
                gtk::SpinButton::with_range(0.0, 255.0, 1.0),
                gtk::SpinButton::with_range(0.0, 255.0, 1.0),
                // §2: "four numeric fields labelled R G B Alpha where **alpha is 0-100**
                // rather than 0-255".
                gtk::SpinButton::with_range(0.0, 100.0, 1.0),
            ],
            saved: gtk::Box::new(gtk::Orientation::Vertical, 4),
            color: Cell::new(color),
            hue: Cell::new(hue),
            syncing: Cell::new(false),
            on_change: RefCell::new(None),
            on_pick: RefCell::new(None),
        });

        picker.stack.add_named(&picker.build_compact(with_alpha), Some("compact"));
        picker.stack.add_named(&picker.build_expanded(with_alpha), Some("expanded"));
        // The stack takes the visible page's size, not the largest page's. Homogeneous is
        // the default and it made the compact palette as wide as the expanded picker --
        // a column of 20 px swatches in a 270 px popover, with nothing wrong in the
        // swatch code. §2's two stages are different sizes on purpose; the chevron
        // "collapses back", which only reads as collapsing if the popover shrinks.
        picker.stack.set_hhomogeneous(false);
        picker.stack.set_vhomogeneous(false);
        picker.stack.set_visible_child_name("compact");
        picker.popover.set_child(Some(&picker.stack));
        picker.fill_saved(&saved_colors);
        picker.sync();
        picker
    }

    #[must_use]
    pub fn popover(&self) -> &gtk::Popover {
        &self.popover
    }

    pub fn connect_changed(&self, f: impl Fn(Rgba) + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(f));
    }

    /// Called when the eyedropper is pressed. The editor owns the sampling, because the
    /// pixels are the canvas's.
    pub fn connect_pick(&self, f: impl Fn() + 'static) {
        *self.on_pick.borrow_mut() = Some(Box::new(f));
    }

    #[must_use]
    pub fn color(&self) -> Rgba {
        self.color.get()
    }

    /// Sets the colour from outside -- the pipette, or a palette click.
    pub fn set_color(&self, color: Rgba) {
        // The hue is only re-read when the colour comes from elsewhere, and even then only
        // if it has one: sampling a grey off the screenshot must not reset the square.
        let (hue, saturation, _) = color.to_hsv();
        if saturation > 1e-6 {
            self.hue.set(hue);
        }
        self.color.set(color);
        self.sync();
        self.notify();
    }

    fn notify(&self) {
        if let Some(f) = self.on_change.borrow().as_ref() {
            f(self.color.get());
        }
    }

    /// Pushes the current colour into every control that shows it.
    fn sync(&self) {
        if self.syncing.replace(true) {
            return;
        }
        let color = self.color.get();
        let (_, saturation, value) = color.to_hsv();
        self.square.set_hsv(self.hue.get(), saturation, value);
        self.hue_bar.set_position(self.hue.get() / 360.0);
        self.alpha_bar.set_mode(bar::Mode::Alpha(color));
        self.alpha_bar.set_position(color.a);
        self.round.set_kind(Kind::Color(color));
        // §2: "a **Hex** field written without a leading `#`".
        self.hex.set_text(&color.to_hex());
        let (r, g, b) = color.to_rgb8();
        self.fields[0].set_value(f64::from(r));
        self.fields[1].set_value(f64::from(g));
        self.fields[2].set_value(f64::from(b));
        self.fields[3].set_value((color.a * 100.0).round());
        self.syncing.set(false);
    }

    fn set_from_hsv(self: &Rc<Self>, saturation: f64, value: f64) {
        let color = Rgba::from_hsv(self.hue.get(), saturation, value, self.color.get().a);
        self.color.set(color);
        self.sync();
        self.notify();
    }
}

impl Picker {
    /// §2's compact stage: "a single vertical column of ten swatches … Below a hairline
    /// sits a rainbow-wheel button that expands to the full picker."
    fn build_compact(self: &Rc<Self>, with_alpha: bool) -> gtk::Widget {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
        pad(&column);
        for (name, hex) in PALETTE {
            let Some(color) = Rgba::from_hex(hex) else { continue };
            column.append(&self.swatch_button(color, name, with_alpha));
        }
        column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // §2's "rainbow-wheel button", as an actual wheel: a conic gradient of the hue
        // circle. Borrowing the hue *bar* for it was the first attempt and it made the
        // popover three times too wide, because a slider's minimum width is 160 px.
        let wheel = Preview::new(Kind::Wheel);
        let expand = gtk::Button::builder()
            .child(&wheel)
            .has_frame(false)
            .tooltip_text("Custom colour")
            .build();
        {
            let picker = Rc::downgrade(self);
            expand.connect_clicked(move |_| {
                if let Some(picker) = picker.upgrade() {
                    picker.stack.set_visible_child_name("expanded");
                }
            });
        }
        column.append(&expand);
        column.upcast()
    }

    /// One palette chip, ringed when it is the current colour.
    fn swatch_button(self: &Rc<Self>, color: Rgba, name: &str, with_alpha: bool) -> gtk::Widget {
        let chip = Preview::new(Kind::Color(color));
        chip.set_active(same_hue(color, self.color.get()));
        let button = gtk::Button::builder()
            .child(&chip)
            .tooltip_text(name)
            .has_frame(false)
            .build();
        let picker = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            let Some(picker) = picker.upgrade() else { return };
            // §4.4: "palette colours default to 100 %". The one row with an opacity slider
            // keeps whatever the user set there.
            let alpha = if with_alpha { picker.color.get().a } else { 1.0 };
            picker.set_color(Rgba { a: alpha, ..color });
            picker.popover.popdown();
        });
        button.upcast()
    }

    /// §2's expanded stage, in the order §2 lists it: the ten presets, the ten My Colors
    /// slots, then the square, the two sliders, the round preview, Hex, the eyedropper,
    /// R G B Alpha, and Add to My Colors.
    fn build_expanded(self: &Rc<Self>, with_alpha: bool) -> gtk::Widget {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        pad(&root);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let back = gtk::Button::builder()
            .icon_name("go-previous-symbolic")
            .has_frame(false)
            .tooltip_text("Back to the palette")
            .build();
        {
            let picker = Rc::downgrade(self);
            back.connect_clicked(move |_| {
                if let Some(picker) = picker.upgrade() {
                    picker.stack.set_visible_child_name("compact");
                }
            });
        }
        header.append(&back);
        let title = gtk::Label::new(Some("Custom colour"));
        title.add_css_class("heading");
        header.append(&title);
        root.append(&header);

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 10);

        // Two columns of ten: the fixed presets, then the user's own.
        let presets = gtk::Box::new(gtk::Orientation::Vertical, 4);
        for (name, hex) in PALETTE {
            let Some(color) = Rgba::from_hex(hex) else { continue };
            presets.append(&self.swatch_button(color, name, with_alpha));
        }
        body.append(&presets);
        body.append(&self.saved);

        let right = gtk::Box::new(gtk::Orientation::Vertical, 8);

        // The square, dragged and clicked.
        {
            let drag = gtk::GestureDrag::new();
            let picker = Rc::downgrade(self);
            let start = Cell::new((0.0, 0.0));
            drag.connect_drag_begin(move |_, x, y| {
                start.set((x, y));
                let Some(picker) = picker.upgrade() else { return };
                let (s, v) = picker.square.at(x, y);
                picker.set_from_hsv(s, v);
            });
            let picker = Rc::downgrade(self);
            drag.connect_drag_update(move |gesture, dx, dy| {
                let Some(picker) = picker.upgrade() else { return };
                let Some((sx, sy)) = gesture.start_point() else { return };
                let (s, v) = picker.square.at(sx + dx, sy + dy);
                picker.set_from_hsv(s, v);
            });
            self.square.add_controller(drag);
        }
        right.append(&self.square);

        // The hue slider, then the alpha slider. §2 has both; the alpha one appears for
        // every tool here because the picker is one widget -- but only the row that §2
        // gives opacity opens it with a meaningful value, and the swatch buttons above
        // reset alpha to 100 % for the others.
        self.wire_bar(&self.hue_bar, true);
        right.append(&self.hue_bar);
        if with_alpha {
            self.wire_bar(&self.alpha_bar, false);
            right.append(&self.alpha_bar);
        }

        // The round preview, the Hex field and the eyedropper, on one line.
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        line.append(&self.round);
        self.hex.set_max_width_chars(8);
        self.hex.set_width_chars(8);
        self.hex.set_placeholder_text(Some("RRGGBB"));
        self.hex.set_tooltip_text(Some("Hex, without the #"));
        {
            let picker = Rc::downgrade(self);
            // On activate rather than on every keystroke: half a hex string is a valid
            // shorter one, so `E0` would be read as a colour the moment it was typed.
            self.hex.connect_activate(move |entry| {
                let Some(picker) = picker.upgrade() else { return };
                let text = entry.text();
                match Rgba::from_hex(text.trim()) {
                    Some(color) => picker.set_color(Rgba { a: picker.color.get().a, ..color }),
                    // Put the real value back rather than leaving nonsense in the field.
                    None => picker.sync(),
                }
            });
        }
        line.append(&self.hex);
        let dropper = gtk::Button::builder()
            .icon_name("color-select-symbolic")
            .tooltip_text("Pick a colour from the picture")
            .build();
        {
            let picker = Rc::downgrade(self);
            dropper.connect_clicked(move |_| {
                let Some(picker) = picker.upgrade() else { return };
                if let Some(f) = picker.on_pick.borrow().as_ref() {
                    f();
                }
            });
        }
        line.append(&dropper);
        right.append(&line);

        // R G B Alpha.
        let numbers = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        for (index, label) in ["R", "G", "B", "Alpha"].into_iter().enumerate() {
            if index == 3 && !with_alpha {
                break;
            }
            let cell = gtk::Box::new(gtk::Orientation::Vertical, 2);
            let caption = gtk::Label::new(Some(label));
            caption.add_css_class("caption");
            caption.add_css_class("dim-label");
            cell.append(&caption);
            let field = &self.fields[index];
            field.set_width_chars(3);
            field.set_max_width_chars(4);
            {
                let picker = Rc::downgrade(self);
                field.connect_value_changed(move |_| {
                    let Some(picker) = picker.upgrade() else { return };
                    if picker.syncing.get() {
                        return;
                    }
                    picker.set_from_fields();
                });
            }
            cell.append(field);
            numbers.append(&cell);
        }
        right.append(&numbers);

        let add = gtk::Button::builder().label("+ Add to My Colors").build();
        add.add_css_class("suggested-action");
        {
            let picker = Rc::downgrade(self);
            add.connect_clicked(move |_| {
                if let Some(picker) = picker.upgrade() {
                    picker.save_current();
                }
            });
        }
        right.append(&add);

        body.append(&right);
        root.append(&body);
        root.upcast()
    }

    fn wire_bar(self: &Rc<Self>, bar: &bar::Bar, is_hue: bool) {
        let drag = gtk::GestureDrag::new();
        let apply = {
            // The bar weakly too: the gesture is the bar's own, and a bar held from inside
            // it was a ring that kept every bar a picker had built (D139).
            let (picker, bar) = (Rc::downgrade(self), bar.downgrade());
            move |x: f64| {
                let (Some(picker), Some(bar)) = (picker.upgrade(), bar.upgrade()) else { return };
                let t = bar.at(x);
                if is_hue {
                    picker.hue.set(t * 360.0);
                    let (_, saturation, value) = picker.color.get().to_hsv();
                    // A grey has no saturation, so dragging the hue over one would do
                    // nothing visible. Give it some, which is what the user is asking for
                    // by touching the hue slider at all.
                    let saturation = if saturation < 1e-6 { 1.0 } else { saturation };
                    let value = if value < 1e-6 { 1.0 } else { value };
                    picker.set_from_hsv(saturation, value);
                } else {
                    let color = Rgba { a: t, ..picker.color.get() };
                    picker.color.set(color);
                    picker.sync();
                    picker.notify();
                }
            }
        };
        {
            let apply = apply.clone();
            drag.connect_drag_begin(move |_, x, _| apply(x));
        }
        drag.connect_drag_update(move |gesture, dx, _| {
            if let Some((sx, _)) = gesture.start_point() {
                apply(sx + dx);
            }
        });
        bar.add_controller(drag);
    }

    fn set_from_fields(self: &Rc<Self>) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let channel = |i: usize| self.fields[i].value().round() as u8;
        let alpha = self.fields[3].value() / 100.0;
        let color = Rgba { a: alpha, ..self.color.get() }
            .with_rgb8(channel(0), channel(1), channel(2));
        // Typed channels are a colour arriving from elsewhere, so the hue is re-read --
        // but only if it has one, which is what `set_color` decides.
        self.set_color(color);
    }

    /// §2's My Colors column: ten slots, "dashed empty circles until filled".
    fn fill_saved(self: &Rc<Self>, colors: &[String]) {
        while let Some(child) = self.saved.first_child() {
            self.saved.remove(&child);
        }
        for slot in 0..MY_COLORS {
            let color = colors.get(slot).and_then(|hex| Rgba::from_hex(hex));
            match color {
                Some(color) => self.saved.append(&self.swatch_button(color, "Saved colour", true)),
                None => {
                    let empty = Preview::new(Kind::Empty);
                    let button = gtk::Button::builder()
                        .child(&empty)
                        .has_frame(false)
                        .sensitive(false)
                        .tooltip_text("Empty slot")
                        .build();
                    self.saved.append(&button);
                }
            }
        }
    }

    /// Adds the current colour to `my-colors`, newest first, capped at ten.
    ///
    /// The list arithmetic is [`octosnap_scene::remember_color`], which is tested: a
    /// colour already saved has to *move* rather than appear twice, and a full list has to
    /// drop its oldest rather than refuse the newest.
    fn save_current(self: &Rc<Self>) {
        let colors = octosnap_scene::remember_color(
            &crate::settings::my_colors(),
            &self.color.get().to_hex(),
            MY_COLORS,
        );
        crate::settings::set_my_colors(&colors);
        self.fill_saved(&colors);
    }
}

fn pad(widget: &impl IsA<gtk::Widget>) {
    let widget = widget.as_ref();
    widget.set_margin_top(8);
    widget.set_margin_bottom(8);
    widget.set_margin_start(8);
    widget.set_margin_end(8);
}

/// Whether two colours are the same swatch, ignoring opacity.
fn same_hue(a: Rgba, b: Rgba) -> bool {
    (a.r - b.r).abs() < 1e-6 && (a.g - b.g).abs() < 1e-6 && (a.b - b.b).abs() < 1e-6
}
