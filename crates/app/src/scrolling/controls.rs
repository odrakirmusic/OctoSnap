// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/07` §1.1's scrolling-capture controls, and the preview that grows beside them.
//!
//! > Controls appear below the selection: **Start**, then **Auto-scroll** (toggle;
//! > direction ↓ or →), **Pause**, **Done**, **Cancel**, **Help**. -- While capturing, a
//! > live **preview strip** beside the area shows the stitched result growing, with the
//! > current height/width in px.
//!
//! **One window, not two** (D74). §1.1 puts the controls below the selection and the strip
//! beside it, which is two placements, two roles in the extension and two windows to keep
//! in step with a capture that can end at any moment. The thing the two are *for* is one
//! thing -- "here is what you have, here is how to stop" -- so they share a pill, placed
//! outside the selection exactly as the recorder's is. The strip still grows and still
//! says its size in pixels.
//!
//! "Outside the selection" is the *placement*, and until 2026-09-16 this said it was the
//! result -- "still never inside the captured rectangle" -- which Mutter was under no
//! obligation to honour. It clamps a window to the monitor it lands on, so a selection
//! filling a monitor pushed the pill back inside the very rectangle it was placed beside,
//! and the capture came out with the controls in its corner. `Scroller::perch` asks where
//! the window actually landed and moves it again if the answer was inside (D94).
//!
//! Mapped and never `present()`ed, like the recorder's pill and a QAO card: the user is
//! scrolling a *different* window, and a controls bar that took focus would stop the very
//! scrolling it exists to watch.
//!
//! **Two faces in one place** (D152). Before Start the pill asks the one thing a capture
//! has to know -- which way the page will be scrolled -- as four named arrows, where the
//! strip will grow; once the capture runs, that place holds the strip and its size. Until
//! 2026-09-27 the strip's box stood there empty before Start, two-thirds of the pill, and
//! the choices were an unlabelled ⏩ toggle and a chevron beside it. The toggle was
//! auto-scroll, which went with its Pause on 2026-09-27 (D153): the user scrolls. The faces
//! are the pages of one `GtkStack`, which is as big as its bigger page, so the pill is
//! measured once, placed once and keeps that size: nothing moves under the pointer when
//! Start is pressed.
//!
//! **One primary at a time, last before the ×**, as the recorder's Stop is before its
//! Discard: Start, and then Done in the same place. Done before Start had nothing to keep
//! and acted as Cancel (`spec/13` #2, #4, #7, #19). Help is there before Start only: a
//! running capture needs nothing from it that the line under the strip does not say, and
//! its panel, which can lie on top of the pill, would hide the strip and Done.
//!
//! **The × asks twice while a capture runs** (`spec/13` #13): the first press turns it red
//! and says so under the strip, and a second within three seconds throws the capture
//! away. Not the recorder's dialog: that is a window of its own, placed by Mutter, and here
//! it could land over the very page being stitched.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, glib};
use octosnap_core::{Rect, ScrollDirection};
use tracing::{debug, info};

/// The preview's box in logical pixels. Tall and narrow, because what it shows is.
const PREVIEW_WIDTH: i32 = 132;
const PREVIEW_HEIGHT: i32 = 168;

/// How tall the picture of what Start will do is at least, in logical pixels.
const SKETCH_HEIGHT: i32 = 52;

/// How wide the picture needs to be at its own size, the sideways one's arrow included.
const SKETCH_WIDTH: f64 = 140.0;

/// The most the picture is grown by to fill the face, and the room it keeps above and
/// below itself when it is.
const SKETCH_ZOOM: f64 = 1.4;
const SKETCH_ROOM: f64 = 20.0;

/// What Help's panel adds round its words, both sides together and the same either way:
/// the stylesheet's padding on `octosnap-scroll-guide`, 12 a side, and libadwaita's border,
/// 1 a side. Taken from the pill's width, it makes the panel exactly as wide as the pill.
const GUIDE_FRAME: i32 = 26;

/// The gap between the pill and Help's panel, in logical pixels.
const GUIDE_GAP: i32 = 6;

/// `spec/09`'s blue: the outline the extension draws round the selection (D78), so the one
/// in the picture is the one on the screen.
const OUTLINE: (f64, f64, f64) = (53.0 / 255.0, 132.0 / 255.0, 228.0 / 255.0);

/// What the pill asks the controller to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    Start,
    Done,
    Cancel,
    /// One of the arrows, pressed before Start.
    Direction(ScrollDirection),
}

/// The pill's opacity while it nudges a live view into sending a frame: a change GTK has to
/// draw and nobody can see.
const NUDGED: f64 = 0.99;

/// How long the × stays asking before it goes back to being a plain ×.
const DISCARD_ASKS_FOR: Duration = Duration::from_secs(3);

/// The scrolling capture's on-screen controls. Owns one undecorated window.
#[derive(Debug)]
pub struct Pill {
    window: gtk::ApplicationWindow,
    /// The choices before Start, and the strip once the capture runs, in one place.
    face: gtk::Stack,
    preview: gtk::Image,
    size: gtk::Label,
    hint: gtk::Label,
    /// Start, then Done, in one place.
    primary: gtk::Stack,
    cancel: gtk::Button,
    directions: adw::ToggleGroup,
    sketch: gtk::DrawingArea,
    help: gtk::MenuButton,
    direction: Cell<ScrollDirection>,
    running: Cell<bool>,
    /// While [`Pill::show_direction`] sets the arrows, which is not the user asking.
    syncing: Cell<bool>,
    /// `spec/07` §1.1 item 4's "very long".
    long: Cell<bool>,
    /// A frame the matcher would not place: a flick too big.
    trouble: Cell<bool>,
    /// The × has been pressed once while the capture runs, and is asking for a second.
    discard_asked: Cell<bool>,
    unask: RefCell<Option<glib::SourceId>>,
    /// Where the pill landed, the work area that holds it, and the selection: where Help's
    /// panel can go ([`Pill::set_room`]).
    room: Cell<Option<(Rect, Rect, Rect)>>,
}

impl Pill {
    /// Builds and maps the pill. `ask` is called with whatever the user pressed; the
    /// controller decides what it means.
    pub fn new(app: &adw::Application, direction: ScrollDirection, ask: impl Fn(Ask) + 'static) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("octosnap-scroll")
            .decorated(false)
            .resizable(false)
            .deletable(false)
            .build();
        window.add_css_class("octosnap-scroller");

        // The dark shape is this box, so the room inside it is the stylesheet's padding.
        // Until 2026-09-27 it was margins, which are outside a widget's background: the
        // strip and the buttons touched the pill's edges, and a transparent ring round it
        // took the clicks meant for the page.
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);

        let (setup, directions, sketch) = choices(direction);

        // `gtk::Image` rather than `gtk::Picture`, and this is the whole reason: a Picture's
        // natural size is its paintable's, so a strip of a capture 4 700 px long asked for a
        // window a thousand pixels tall and the pill grew down the screen until it ran out
        // (2026-09-15). An Image measures to `pixel-size` and fits the paintable inside it,
        // so the box stays put however long the capture gets -- and the texture is still
        // handed over at twice the box, which is what keeps it sharp at 2x.
        let preview = gtk::Image::new();
        preview.set_pixel_size(PREVIEW_HEIGHT);
        preview.set_size_request(PREVIEW_WIDTH, PREVIEW_HEIGHT);
        preview.add_css_class("octosnap-scroll-preview");
        preview.set_can_target(false);

        let size = gtk::Label::new(None);
        size.add_css_class("octosnap-scroll-size");

        let live = gtk::Box::new(gtk::Orientation::Vertical, 6);
        live.append(&preview);
        live.append(&size);

        // No transition. A pill that ends up in the shot is in every frame, and one that
        // was still fading into its second face would be a patch changing under the page
        // (D105).
        let face = gtk::Stack::new();
        face.add_named(&setup, Some("setup"));
        face.add_named(&live, Some("live"));
        face.set_visible_child_name("setup");

        // `spec/07` §1.1 item 1's hint, "Scroll the content, or use Auto-scroll". Here
        // rather than in a one-time onboarding popover because it is one line and the pill
        // has room for it; a dialog to dismiss would be one more thing between the user and
        // the page they came to capture. Before Start it says what to do next, in the
        // words the arrows above it are set to.
        let hint = gtk::Label::new(None);
        hint.add_css_class("octosnap-scroll-hint");
        hint.set_wrap(true);
        hint.set_max_width_chars(22);
        hint.set_justify(gtk::Justification::Center);

        // `spec/07` §1.1 item 2's **Help**, and [V]'s onboarding with it. A popover rather
        // than a dialog, and on demand rather than once at the start: the line above is
        // what a first capture needs, and a modal in front of the page the user came to
        // capture is the thing an onboarding popup is usually blamed for. First in the row
        // and apart from the buttons that act.
        //
        // Before Start only (D152): once the capture runs, whatever is over the selection is
        // in the live view and stitched in. Before it, the popover opens as a panel of the
        // pill's own width, in the pill's column and off the selection ([`guide_top`]).
        // Pointed at the ? instead, GTK put it wherever it fitted, which beside a selection
        // was over it -- and under the outline, which the shell draws above every window.
        let help = gtk::MenuButton::new();
        help.set_icon_name("help-browser-symbolic");
        help.set_tooltip_text(Some("Help"));
        help.set_popover(Some(&guide(direction)));
        // What keeps Start, Done and the × at the row's far end, with Help or without it.
        let apart = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        apart.set_hexpand(true);

        let start = gtk::Button::with_label("Start");
        start.add_css_class("suggested-action");
        start.add_css_class("octosnap-scroll-primary");
        let done = gtk::Button::with_label("Done");
        done.add_css_class("suggested-action");
        done.add_css_class("octosnap-scroll-primary");
        // Homogeneous, as a stack is by default: the slot is as wide as the wider label, so
        // the swap does not move anything.
        let primary = gtk::Stack::new();
        primary.add_named(&start, Some("start"));
        primary.add_named(&done, Some("done"));
        primary.set_visible_child_name("start");
        let cancel = gtk::Button::from_icon_name("window-close-symbolic");
        cancel.set_tooltip_text(Some("Cancel"));
        cancel.add_css_class("octosnap-scroll-cancel");

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        row.append(&help);
        row.append(&apart);
        row.append(&primary);
        row.append(&cancel);

        column.append(&face);
        column.append(&hint);
        column.append(&row);
        window.set_child(Some(&column));
        // Two lines high whatever it says. The pill is placed once, from its size then, and
        // a hint that wrapped would grow it -- into the shot, or out from under the pointer
        // on its way to the ×'s second press.
        let two_lines = hint.create_pango_layout(Some("Ag\nAg")).pixel_size().1;
        hint.set_size_request(-1, two_lines);

        let pill = Rc::new(Self {
            window,
            face,
            preview,
            size,
            hint,
            primary,
            cancel: cancel.clone(),
            directions: directions.clone(),
            sketch: sketch.clone(),
            help,
            direction: Cell::new(direction),
            running: Cell::new(false),
            syncing: Cell::new(false),
            long: Cell::new(false),
            trouble: Cell::new(false),
            discard_asked: Cell::new(false),
            unask: RefCell::new(None),
            room: Cell::new(None),
        });
        pill.refresh_hint();
        {
            let weak = Rc::downgrade(&pill);
            pill.help.set_create_popup_func(move |button| {
                if let Some(pill) = weak.upgrade() {
                    pill.fit_guide(button);
                }
            });
        }
        {
            let weak = Rc::downgrade(&pill);
            sketch.set_draw_func(move |area, cr, width, height| {
                let Some(pill) = weak.upgrade() else { return };
                draw_sketch(cr, f64::from(width), f64::from(height), pill.direction.get(), area.color());
            });
        }
        pill.window.set_visible(true);

        let ask = Rc::new(ask);
        {
            let ask = Rc::clone(&ask);
            start.connect_clicked(move |_| ask(Ask::Start));
        }
        {
            let ask = Rc::clone(&ask);
            done.connect_clicked(move |_| ask(Ask::Done));
        }
        {
            let ask = Rc::clone(&ask);
            // Weak, because the handler is on a widget the pill owns: holding a strong
            // reference here would be a cycle, and the pill would outlive the capture.
            let weak = Rc::downgrade(&pill);
            directions.connect_active_name_notify(move |group| {
                let Some(pill) = weak.upgrade() else { return };
                if pill.syncing.get() {
                    return;
                }
                if let Some(way) = group.active_name().as_deref().and_then(ScrollDirection::from_wire) {
                    ask(Ask::Direction(way));
                }
            });
        }
        {
            let weak = Rc::downgrade(&pill);
            cancel.connect_clicked(move |_| {
                let Some(pill) = weak.upgrade() else { return };
                // Before Start there is nothing stitched to lose.
                if pill.running.get() && !pill.discard_asked.get() {
                    pill.ask_discard();
                    return;
                }
                ask(Ask::Cancel);
            });
        }

        pill
    }

    /// The ×'s first press while a capture runs: red, and a line under the strip saying a
    /// second press throws the capture away. Three seconds, then a plain × again.
    fn ask_discard(self: &Rc<Self>) {
        self.discard_asked.set(true);
        self.cancel.add_css_class("destructive-action");
        self.cancel.set_tooltip_text(Some("Discard the capture"));
        self.refresh_hint();
        info!("the scrolling capture's discard was asked once");
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local_once(DISCARD_ASKS_FOR, move || {
            if let Some(pill) = weak.upgrade() {
                pill.unask.borrow_mut().take();
                pill.unask_discard();
            }
        });
        if let Some(old) = self.unask.borrow_mut().replace(source) {
            old.remove();
        }
    }

    fn unask_discard(&self) {
        self.discard_asked.set(false);
        self.cancel.remove_css_class("destructive-action");
        self.cancel.set_tooltip_text(Some("Cancel"));
        self.refresh_hint();
    }

    /// The GTK object path the extension addresses this window by, the same shape a card
    /// and the recorder's pill use.
    #[must_use]
    pub fn object_path(&self) -> Option<String> {
        let app = self.window.application()?;
        let base = app.dbus_object_path()?;
        Some(format!("{base}/window/{}", self.window.id()))
    }

    /// How big the pill is, in logical pixels.
    ///
    /// Asked of the widget rather than of the compositor, and for the reason
    /// `Placement::Stacked` carries a `size` field at all: a window asked to be placed as
    /// soon as it is mapped has not had its first buffer processed, so Mutter's
    /// `get_frame_rect()` can still answer 0x0. The client made the window and knows how
    /// big it is; the compositor knows where it landed. Neither is asked for the other's
    /// half.
    #[must_use]
    pub fn size(&self) -> (i32, i32) {
        let natural = |orientation| self.window.measure(orientation, -1).1;
        (natural(gtk::Orientation::Horizontal), natural(gtk::Orientation::Vertical))
    }

    /// Shows the chosen direction on the arrows, in the line under them, and in Help.
    pub fn show_direction(&self, direction: ScrollDirection) {
        self.direction.set(direction);
        self.syncing.set(true);
        self.directions.set_active_name(Some(direction.as_wire()));
        self.syncing.set(false);
        self.sketch.queue_draw();
        // [V] `shouldShowHorizontalScrollingCaptureGuide`: the horizontal mode is the one
        // people have to be told about, so Help says a different thing for it.
        self.help.set_popover(Some(&guide(direction)));
        self.refresh_hint();
    }

    /// Where the pill landed, on which work area, beside which selection: what Help's panel
    /// needs to open off the selection (D152). Until it is told, the panel opens under the
    /// pill.
    pub fn set_room(&self, landed: Rect, work_area: Rect, selection: Rect) {
        self.room.set(Some((landed, work_area, selection)));
    }

    /// Sizes Help's panel to the pill and puts it where [`guide_top`] says, each time it
    /// opens: its words, and so its height, follow the direction.
    ///
    /// Worked out here rather than left to GTK's flip, because a panel that fits neither
    /// under the pill nor over it is not flipped into place: Mutter dismisses it the moment
    /// it is mapped, and at 200 % on a 1920 x 1200 panel Help never opened (2026-09-27).
    fn fit_guide(&self, button: &gtk::MenuButton) {
        let Some(popover) = button.popover() else { return };
        let Some(pill) = self.window.child() else { return };
        let Some(bounds) = pill.compute_bounds(button) else { return };
        let (x, y) = (bounds.x().round() as i32, bounds.y().round() as i32);
        let (width, height) = (bounds.width().round() as i32, bounds.height().round() as i32);
        let Some(words) = popover.child() else { return };
        words.set_size_request(width - GUIDE_FRAME, -1);
        let tall = words.measure(gtk::Orientation::Vertical, width - GUIDE_FRAME).1 + GUIDE_FRAME;
        let top = self
            .room
            .get()
            .map_or(height + GUIDE_GAP, |(landed, work_area, selection)| guide_top(landed, work_area, selection, tall));
        // Anchored to the pill's top edge, which is inside the pill as an anchor has to be,
        // and moved from there by the offset, which can take it anywhere.
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x, y, width, 1)));
        popover.set_offset(0, top - 1);
        debug!(top, width, height = tall, "the scrolling pill's help opens");
    }

    /// Moves the pill from "waiting to start" into "capturing".
    pub fn started(&self) {
        self.running.set(true);
        self.help.popdown();
        self.help.set_visible(false);
        self.face.set_visible_child_name("live");
        self.primary.set_visible_child_name("done");
        self.refresh_hint();
        self.log_layout("running");
    }

    /// `spec/07` §1.2's "retry with a smaller step", said to the person doing the
    /// scrolling: the engine cannot retry anything, so the only thing that can make the
    /// next frame placeable is the user scrolling back into the overlap.
    pub fn set_trouble(&self, trouble: bool) {
        if self.trouble.replace(trouble) != trouble {
            self.refresh_hint();
        }
    }

    /// The one line under the strip, derived rather than set from four places.
    ///
    /// Nothing here says the page has ended. D94 had a line for it -- "That looks like the
    /// end of the page" -- and it was reached by a frame that differed without having
    /// scrolled, which a user who has paused to read produces just as readily as a page
    /// that has run out (D98). The frame count above this line is what the pill has to
    /// say about a capture that has stopped growing, and it says it without guessing.
    fn refresh_hint(&self) {
        let text = if self.discard_asked.get() {
            "Press \u{d7} again to throw it away".to_owned()
        } else if self.trouble.get() {
            "That scroll was too big \u{2014} scroll back a little".to_owned()
        } else if self.long.get() {
            "This capture is getting very long".to_owned()
        } else if !self.running.get() {
            before_start(self.direction.get())
        } else {
            "Scroll the content, then press Done".to_owned()
        };
        if self.hint.text() != text {
            debug!(%text, "the scrolling pill says");
            self.hint.set_text(&text);
        }
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running.get()
    }

    /// `spec/07` §1.1 item 3: the stitched result so far, and its size in pixels.
    pub fn show(&self, texture: Option<&gdk::Texture>, width: u32, height: u32, long: bool) {
        if let Some(texture) = texture {
            self.preview.set_paintable(Some(texture));
        }
        self.size.set_text(&format!("{width} \u{d7} {height} px"));
        // `spec/07` §1.1 item 4: "a warning appears when the result is very long".
        if long {
            self.size.add_css_class("octosnap-scroll-long");
        } else {
            self.size.remove_css_class("octosnap-scroll-long");
        }
        self.long.set(long);
        self.refresh_hint();
    }

    /// Where each control is in the window, and how big the window is, in the log: the
    /// pill never takes the keyboard, so a harness has to aim at it, and where things sit
    /// is GTK's to decide. `scroll-pill-test.sh` reads these lines.
    pub fn log_layout(&self, state: &str) {
        let at = |widget: &gtk::Widget| {
            widget.compute_bounds(&self.window).map_or_else(String::new, |b| {
                format!(
                    "{},{},{},{}",
                    b.x().round() as i32,
                    b.y().round() as i32,
                    b.width().round() as i32,
                    b.height().round() as i32
                )
            })
        };
        debug!(
            state,
            width = self.window.width(),
            height = self.window.height(),
            face = %self.face.visible_child_name().unwrap_or_default(),
            help = %at(self.help.upcast_ref()),
            directions = %at(self.directions.upcast_ref()),
            primary = %at(self.primary.upcast_ref()),
            cancel = %at(self.cancel.upcast_ref()),
            "scrolling pill layout"
        );
    }

    /// Gives the compositor something to paint, and so a live view that has been sent
    /// nothing a frame (D107): the pill's opacity, a hundredth either way of whole.
    ///
    /// Drawing it again as it was is not enough. GTK compares what it would draw with what
    /// it drew, finds nothing new, and sends the compositor nothing to paint.
    pub fn nudge(&self) {
        let opacity = self.window.opacity();
        self.window.set_opacity(if opacity < 1.0 { 1.0 } else { NUDGED });
    }

    /// Whole again, after [`Pill::nudge`].
    pub fn settle(&self) {
        self.window.set_opacity(1.0);
    }

    /// Takes the pill off the screen. Idempotent.
    pub fn close(&self) {
        if let Some(source) = self.unask.borrow_mut().take() {
            source.remove();
        }
        self.window.destroy();
    }
}

impl Drop for Pill {
    fn drop(&mut self) {
        self.window.destroy();
    }
}

/// The face before Start: what it is, which way, and a picture of that.
fn choices(direction: ScrollDirection) -> (gtk::Box, adw::ToggleGroup, gtk::DrawingArea) {
    let title = gtk::Label::new(Some("Scrolling capture"));
    title.add_css_class("octosnap-scroll-title");
    title.set_xalign(0.0);

    // Which way: the four, each an arrow above its name, as the overlay's toolbar shows its
    // modes (`spec/03` §4), and the one that is set lit. A press each, and the choice in
    // sight, where the menu behind a chevron took two and showed it in neither. The one
    // choice there is: who scrolls was the other until auto-scroll went (D153). `osd`,
    // libadwaita's own styling for a toggle group on a dark panel, as GNOME's screenshot
    // tool sets one.
    let directions = adw::ToggleGroup::new();
    directions.add_css_class("osd");
    directions.add_css_class("octosnap-scroll-ways");
    directions.set_homogeneous(true);
    for way in ScrollDirection::ALL {
        directions.add(tile(way.as_wire(), icon_for(way), way.label()));
    }
    directions.set_active_name(Some(direction.as_wire()));

    // The rest of the face, which is as tall as the strip it stands in for: what Start will
    // do, drawn, where the box stood empty until 2026-09-27.
    let sketch = gtk::DrawingArea::new();
    sketch.set_content_height(SKETCH_HEIGHT);
    sketch.set_vexpand(true);
    sketch.set_can_target(false);

    let setup = gtk::Box::new(gtk::Orientation::Vertical, 10);
    setup.append(&title);
    setup.append(&directions);
    setup.append(&sketch);
    (setup, directions, sketch)
}

/// A picture of what Start will do, for the way that is set: a page, the part of it the
/// outline holds, and the arrow the rest arrives along. Drawn for Down and Right and turned
/// over for Up and Left, as the capture itself is (`spec/07` §1.3, "four directions, one
/// algorithm"). `ink` is the pill's own text colour.
fn draw_sketch(cr: &gtk::cairo::Context, width: f64, height: f64, direction: ScrollDirection, ink: gdk::RGBA) {
    // The picture is `SKETCH_HEIGHT` tall, grown into what the face leaves it as far as
    // the width allows, and in the middle of it. The face has room to spare since the
    // modes went (D153).
    let zoom = ((height - SKETCH_ROOM) / f64::from(SKETCH_HEIGHT)).min(width / SKETCH_WIDTH).clamp(1.0, SKETCH_ZOOM);
    let top = ((height - f64::from(SKETCH_HEIGHT) * zoom) / 2.0).max(0.0).round();
    cr.translate(0.0, top);
    cr.scale(zoom, zoom);
    let (width, height) = (width / zoom, (height / zoom).min(f64::from(SKETCH_HEIGHT)));
    if direction.backwards() {
        if direction.horizontal() {
            cr.translate(width, 0.0);
            cr.scale(-1.0, 1.0);
        } else {
            cr.translate(0.0, height);
            cr.scale(1.0, -1.0);
        }
    }
    let ink = (f64::from(ink.red()), f64::from(ink.green()), f64::from(ink.blue()));
    let horizontal = direction.horizontal();
    // The outline, and the page it sits on, which runs on past it and out of the picture.
    let (sw, sh) = if horizontal { (56.0, 30.0) } else { (60.0, 24.0) };
    let sx = if horizontal { (width / 2.0 - 64.0).round() } else { ((width - sw) / 2.0).round() };
    let sy = 3.0;
    let (px, py, pw, ph) = if horizontal {
        (sx + 3.0, sy + 5.0, width - sx, sh - 10.0)
    } else {
        (sx + 5.0, sy + 3.0, sw - 10.0, height - sy)
    };
    let page = if horizontal {
        gtk::cairo::LinearGradient::new(px, 0.0, px + pw, 0.0)
    } else {
        gtk::cairo::LinearGradient::new(0.0, py, 0.0, py + ph)
    };
    page.add_color_stop_rgba(0.0, ink.0, ink.1, ink.2, 0.16);
    page.add_color_stop_rgba(1.0, ink.0, ink.1, ink.2, 0.0);
    if cr.set_source(&page).is_ok() {
        rounded(cr, px, py, pw, ph, 2.0);
        let _ = cr.fill();
    }
    // What is on the page: bright where the outline holds it, fading past it. Lines of
    // text down a page that goes down; rows of cells along one that goes sideways, which is
    // how a wide table or a timeline reads.
    let (inside, reach) = if horizontal { (sx + sw, width) } else { (sy + sh, height) };
    let alpha = |at: f64| {
        if at + 2.0 <= inside - 2.0 {
            0.7
        } else {
            0.32 * (1.0 - (at - inside) / (reach - inside)).clamp(0.0, 1.0)
        }
    };
    if horizontal {
        let cells = [10.0, 6.0, 12.0, 8.0, 9.0, 7.0, 11.0];
        for (row, y) in [py + 3.0, py + 9.0, py + 15.0].into_iter().enumerate() {
            let mut at = px + 4.0;
            for cell in cells.iter().cycle().skip(row * 2).take(40) {
                if at + cell > reach {
                    break;
                }
                // Each cell faded by where it ends, so the one the outline cuts is dim.
                cr.set_source_rgba(ink.0, ink.1, ink.2, alpha(at + cell - 2.0));
                cr.rectangle(at, y, *cell, 2.0);
                let _ = cr.fill();
                at += cell + 4.0;
            }
        }
    } else {
        let lengths = [0.8, 0.55, 0.7, 0.4, 0.75, 0.5, 0.65, 0.45];
        for (i, length) in lengths.iter().cycle().take(40).enumerate() {
            let at = py + 3.0 + 5.0 * i as f64;
            if at + 2.0 > reach {
                break;
            }
            cr.set_source_rgba(ink.0, ink.1, ink.2, alpha(at));
            cr.rectangle(px + 5.0, at, (pw - 10.0) * length, 2.0);
            let _ = cr.fill();
        }
    }
    cr.set_source_rgb(OUTLINE.0, OUTLINE.1, OUTLINE.2);
    cr.set_line_width(1.5);
    rounded(cr, sx + 0.75, sy + 0.75, sw - 1.5, sh - 1.5, 2.0);
    let _ = cr.stroke();
    // Which way the rest of the page comes from.
    cr.set_source_rgba(ink.0, ink.1, ink.2, 0.85);
    if horizontal {
        let ay = sy + sh + 9.5;
        arrow(cr, (sx + 8.0, ay), (sx + sw + 44.0, ay));
    } else {
        let ax = sx + sw + 14.5;
        arrow(cr, (ax, sy + 5.0), (ax, height - 3.0));
    }
}

/// A rectangle with round corners, as a path.
fn rounded(cr: &gtk::cairo::Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + width - radius, y + radius, radius, -FRAC_PI_2, 0.0);
    cr.arc(x + width - radius, y + height - radius, radius, 0.0, FRAC_PI_2);
    cr.arc(x + radius, y + height - radius, radius, FRAC_PI_2, PI);
    cr.arc(x + radius, y + radius, radius, PI, 3.0 * FRAC_PI_2);
    cr.close_path();
}

/// A line with an open head at `to`, in the source already set.
fn arrow(cr: &gtk::cairo::Context, from: (f64, f64), to: (f64, f64)) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = dx.hypot(dy).max(1.0);
    let (ux, uy) = (dx / length, dy / length);
    let head = 4.0;
    cr.set_line_width(1.5);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    cr.move_to(from.0, from.1);
    cr.line_to(to.0, to.1);
    cr.move_to(to.0 - (ux + uy) * head, to.1 - (uy - ux) * head);
    cr.line_to(to.0, to.1);
    cr.line_to(to.0 - (ux - uy) * head, to.1 - (uy + ux) * head);
    let _ = cr.stroke();
}

/// A direction as a tile: its arrow above its name. The name is also the toggle's label,
/// which a screen reader reads when a child stands in for it; no tooltip, which would only
/// say the name again.
fn tile(wire: &str, icon: &str, name: &str) -> adw::Toggle {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
    content.append(&gtk::Image::from_icon_name(icon));
    content.append(&gtk::Label::new(Some(name)));
    adw::Toggle::builder().name(wire).label(name).child(&content).build()
}

/// Where Help's panel starts, relative to the top of the pill, for a panel `height` tall
/// and as wide as the pill.
///
/// In the pill's column and never beside it, since beside the pill is the selection: under
/// the pill, or over it, whichever is wholly on the work area and off the selection. When
/// neither is, on top of the pill itself: from its top down, or from its bottom up,
/// whichever keeps off the selection -- the pill is only above or below the selection when
/// there was no room beside it. Failing all four, it is kept on the work area.
fn guide_top(pill: Rect, work_area: Rect, selection: Rect, height: i32) -> i32 {
    let fits = |top: i32| {
        let panel = Rect::new(pill.x, pill.y + top, pill.width, height);
        let on_work_area = panel.y >= work_area.y && panel.y + panel.height <= work_area.y + work_area.height;
        on_work_area && panel.overlap_area(selection) == 0
    };
    let under = pill.height + GUIDE_GAP;
    let over = -GUIDE_GAP - height;
    let from_top = 0;
    let from_bottom = pill.height - height;
    if let Some(top) = [under, over, from_top, from_bottom].into_iter().find(|&top| fits(top)) {
        return top;
    }
    let highest = work_area.y - pill.y;
    let lowest = work_area.y + work_area.height - height - pill.y;
    from_top.min(lowest).max(highest)
}

/// What the line under the choices says before Start: the next thing to do, in the words
/// the arrows are set to. Nothing is captured until Start, so "Scroll the content" -- which
/// it said until 2026-09-27 -- asked for a scroll that captured nothing.
fn before_start(direction: ScrollDirection) -> String {
    format!("Press Start, then scroll {}", direction.label().to_lowercase())
}

/// How to reach Start and Done without the pointer (D137).
const KEYS: &str = "Without the pointer: press the Scrolling Capture shortcut again, or choose \
                    Start and then Finish from OctoSnap's menu in the top bar.";

/// `spec/07` §1.1's onboarding, as a popover: a dark panel of the pill's width, with no
/// arrow, which the pill's Help sizes and points each time it opens.
fn guide(direction: ScrollDirection) -> gtk::Popover {
    // The room round the words is the stylesheet's padding, which [`GUIDE_FRAME`] counts.
    let column = gtk::Box::new(gtk::Orientation::Vertical, 8);

    let title = gtk::Label::new(Some("Scrolling capture"));
    title.add_css_class("heading");
    title.set_xalign(0.0);
    column.append(&title);

    // Narrow by itself, so the width the panel is given is the pill's, and the words wrap
    // to it.
    let body = gtk::Label::new(Some(&guide_text(direction)));
    body.set_wrap(true);
    body.set_max_width_chars(16);
    body.set_xalign(0.0);
    column.append(&body);

    // Dark, as the pill it opens from is (the stylesheet's `octosnap-scroll-guide`): a light
    // sheet out of a dark pill read as something else's.
    let popover = gtk::Popover::new();
    popover.add_css_class("octosnap-scroll-guide");
    popover.set_has_arrow(false);
    popover.set_child(Some(&column));
    // For `scroll-pill-test.sh`, which cannot see a popover go.
    popover.connect_closed(|_| debug!("the scrolling pill's help closes"));
    popover
}

/// Help's words for a direction. The pill never takes the keyboard, which the page needs,
/// so its buttons have a second way in, and Help ends with it (D137).
fn guide_text(direction: ScrollDirection) -> String {
    let (scroll, added) = match direction {
        ScrollDirection::Down => ("scroll the page down", "under"),
        ScrollDirection::Up => ("scroll the page up", "above"),
        ScrollDirection::Right => (
            "scroll the page to the right \u{2014} Shift and the wheel, or two fingers on a trackpad",
            "to the right of",
        ),
        ScrollDirection::Left => (
            "scroll the page to the left \u{2014} Shift and the wheel, or two fingers on a trackpad",
            "to the left of",
        ),
    };
    format!(
        "Press Start, then {scroll}. Each time the page settles, the new part is added {added} \
         what you already have.\n\nDone keeps what is stitched; \u{d7} keeps nothing.\n\n{KEYS}"
    )
}

/// The arrow for a direction: OctoSnap's own (`resources/octosnap.gresource.xml`), since a
/// theme's `go-down` can be a chevron, which reads as "this opens a menu".
#[must_use]
pub const fn icon_for(direction: ScrollDirection) -> &'static str {
    match direction {
        ScrollDirection::Down => "scroll-down-symbolic",
        ScrollDirection::Up => "scroll-up-symbolic",
        ScrollDirection::Right => "scroll-right-symbolic",
        ScrollDirection::Left => "scroll-left-symbolic",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn before_start_says_start_first_and_names_the_way() {
        for way in ScrollDirection::ALL {
            let line = before_start(way);
            assert!(line.starts_with("Press Start, then scroll "), "{line}");
            assert!(line.ends_with(&format!(" {}", way.label().to_lowercase())), "{line}");
            // One line of the hint's width.
            assert!(line.chars().count() <= 32, "{line}");
        }
    }

    #[test]
    fn help_tells_the_sideways_scroll_only_for_the_sideways_ways() {
        for way in ScrollDirection::ALL {
            let text = guide_text(way);
            assert_eq!(text.contains("Shift and the wheel"), way.horizontal(), "{text}");
            assert!(text.starts_with("Press Start, then scroll the page "), "{text}");
            assert!(!text.to_lowercase().contains("auto"), "{text}");
            assert!(text.ends_with(KEYS));
        }
        assert!(guide_text(ScrollDirection::Up).contains("added above what"));
        assert!(guide_text(ScrollDirection::Left).contains("added to the left of what"));
    }

    /// The nested shell's monitor at 100 % and at 200 %, under a 32 px top bar, and the
    /// selection the harness makes on it.
    const WORK_100: Rect = Rect::new(0, 32, 1920, 1168);
    const WORK_200: Rect = Rect::new(0, 32, 960, 568);

    #[test]
    fn help_opens_under_a_pill_beside_the_selection_when_it_fits() {
        let pill = Rect::new(1292, 200, 234, 284);
        let selection = Rect::new(320, 200, 960, 800);
        assert_eq!(guide_top(pill, WORK_100, selection, 384), 284 + GUIDE_GAP);
    }

    #[test]
    fn help_opens_over_a_pill_above_the_selection() {
        // No room beside a selection the monitor's width, nor under it: the pill is above.
        let pill = Rect::new(0, 500, 234, 284);
        let selection = Rect::new(0, 796, 1920, 404);
        assert_eq!(guide_top(pill, WORK_100, selection, 384), -GUIDE_GAP - 384);
    }

    #[test]
    fn help_goes_on_top_of_the_pill_where_it_fits_nowhere_else() {
        // 200 %: 284 of pill and 384 of panel are more than the 568 there is.
        let pill = Rect::new(652, 100, 234, 284);
        let selection = Rect::new(160, 100, 480, 400);
        assert_eq!(guide_top(pill, WORK_200, selection, 384), 0);
        // A pill above the selection, with no room over it: from the pill's bottom up, not
        // down onto the selection.
        let pill = Rect::new(0, 204, 234, 284);
        let selection = Rect::new(0, 500, 1920, 700);
        assert_eq!(guide_top(pill, WORK_100, selection, 384), 284 - 384);
    }

    #[test]
    fn help_stays_on_the_work_area_when_nothing_fits() {
        // A panel taller than the room there is: from the work area's top.
        let pill = Rect::new(652, 100, 234, 284);
        let selection = Rect::new(0, 32, 960, 568);
        let top = guide_top(pill, WORK_200, selection, 700);
        assert_eq!(pill.y + top, WORK_200.y);
        // One that fits on top of the pill but would spill off the bottom is moved up.
        let pill = Rect::new(652, 400, 234, 200);
        let top = guide_top(pill, WORK_200, selection, 300);
        assert_eq!(pill.y + top + 300, WORK_200.y + WORK_200.height);
    }

    #[test]
    fn every_way_has_its_own_arrow() {
        let names: std::collections::BTreeSet<_> = ScrollDirection::ALL.iter().map(|&way| icon_for(way)).collect();
        assert_eq!(names.len(), ScrollDirection::ALL.len());
        assert!(names.iter().all(|name| name.ends_with("-symbolic")));
    }
}
