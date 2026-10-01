// SPDX-License-Identifier: GPL-3.0-or-later

//! One pinned screenshot (`spec/07` §3.1, §3.2).
//!
//! A borderless window holding the capture at 1:1, placed at the rect it came from. Like
//! a card it is never `present()`ed, and like a card the extension is what actually moves
//! it -- a Wayland client can position nothing, including itself.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use octosnap_core::pin;
use octosnap_core::qao::Size;
use octosnap_core::{CaptureResult, Rect};
use octosnap_shell::Placement;
use tracing::{debug, info, warn};

pub type PinId = u64;

/// What a control on a pin does. A plain function pointer: every one is a fixed call, and
/// the tables below are data rather than closures.
type PinAction = fn(&Rc<Pin>);

/// What a pin asks of whatever owns it.
pub trait PinHost {
    /// Place the window; a client cannot do this for itself.
    fn place(&self, id: PinId, path: String, placement: Placement);
    /// Move it by a delta (`spec/07` §3.1's arrow keys).
    fn nudge(&self, id: PinId, path: String, dx: i32, dy: i32);
    /// `spec/07` §3.1's shadow, which the compositor draws: the pin's corners and its
    /// opacity, or an opacity of 0 for none (D132).
    fn shade(&self, id: PinId, path: String, radius: i32, opacity: f64);
    /// The pointer entered or left, so the keyboard can be lent and given back.
    fn hovered(&self, id: PinId, path: String, entered: bool);
    fn copy(&self, id: PinId);
    fn save(&self, id: PinId);
    /// `spec/07` §3.1's Annotate: the editor on this pin's capture. The pin stays, and
    /// `PIN-08` updates it when the editor crops.
    fn annotate(&self, id: PinId);
    /// `spec/07` §2.1's pin source: read this pin's image and copy what it says. The pin
    /// stays exactly as it is -- reading is not an edit.
    fn recognise(&self, id: PinId);
    /// The capture was dragged out to another application (`PIN-05`). Without Alt the
    /// pin closes into the history; with Alt held it stays (`[D 4.5.1]`).
    fn dragged_out(&self, id: PinId, keep: bool);
    fn close(&self, id: PinId);
    fn close_all_pins(&self);
}

pub struct Pin {
    pub id: PinId,
    /// The capture on show, which `spec/05` §4.11 can replace: "a pinned window showing
    /// this image updates after crop" [D 4.7.5].
    capture: RefCell<CaptureResult>,
    /// The widget the capture is shown in, held so it can be handed a new file.
    picture: gtk::Picture,
    window: gtk::ApplicationWindow,
    body: gtk::Widget,
    /// The controls that only appear on hover, so a pin at rest is only the capture.
    chrome: gtk::Widget,
    /// The window's size in logical pixels. A `Cell` because `spec/05` §4.11's crop
    /// changes it: a pin showing a cropped image is a different shape.
    size: Cell<Size>,
    /// Where the window is *now*, which is where its capture came from only until
    /// something moves it.
    ///
    /// The app has to keep this because nothing else does. Hiding a pin destroys its
    /// toplevel and Mutter's record of where it was along with it, so a pin coming back
    /// from `spec/07` §3.1's Hide/Show is placed again from scratch, and this is the only
    /// surviving record of where "back" is.
    ///
    /// A rect rather than an origin because `spec/07` §3.1 also grants "resize from
    /// edges/corners keeping aspect". When that lands, the size a pin returns at is the
    /// size the user left it, and this is already the field that would say so.
    current: Cell<Rect>,
    /// The window-sized root, held so a new capture can resize it.
    root: gtk::Widget,
    /// The layer that `spec/07` §3.1's opacity fades: the capture and its backing. Not
    /// the window, so the chrome and the readout stay legible on a faint pin.
    image: gtk::Widget,
    /// The drag surface, held so lock mode can stop it taking a click that the
    /// compositor has already committed to delivering. See `set_locked`.
    handle: gtk::WindowHandle,
    /// The opacity readout, a counter so the newest change owns the hide, and the fade
    /// held for its lifetime.
    readout: OpacityReadout,
    readout_generation: Cell<u64>,
    readout_fade: RefCell<Option<adw::TimedAnimation>>,
    /// `spec/07` §3.1's lock mode: clicks pass through everywhere but the unlock handle.
    locked: Cell<bool>,
    /// Whether the pointer is inside, so that unlocking does not blank the controls under
    /// the pointer that just pressed one of them.
    hovered: Cell<bool>,
    opacity: Cell<f64>,
    /// `spec/07` §3.1's corners, border and shadow, as the settings had them when the pin
    /// was made; a pin already on screen keeps its look.
    look: crate::settings::PinLook,
    /// The capture's own size, which `zoom` scales: `spec/07` §3.1's resize as a Wayland
    /// client can have it (`core::pin::zoomed_size`).
    base_size: Cell<Size>,
    zoom: Cell<f64>,
    /// `spec/07` §3.1's "small Drag me grip on hover" (`PIN-05`), the image's sibling like
    /// the chrome so lock mode can switch it off without touching the unlock control.
    grip: gtk::Widget,
    host: Weak<dyn PinHost>,
}

impl std::fmt::Debug for Pin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pin")
            .field("id", &self.id)
            .field("at", &self.current.get())
            .field("locked", &self.locked.get())
            .finish_non_exhaustive()
    }
}

impl Pin {
    /// Builds and maps the window. `None` when the capture has no area to show.
    pub fn new(
        app: &adw::Application,
        id: PinId,
        capture: CaptureResult,
        host: &Rc<dyn PinHost>,
    ) -> Option<Rc<Self>> {
        let work = work_area(capture.rect);
        let opening = pin::opening_rect(capture.rect, work);
        if opening.width <= 0 || opening.height <= 0 {
            warn!(id, "a capture with no area cannot be pinned");
            return None;
        }
        let size = Size::new(opening.width, opening.height);

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title(format!("octosnap-pin:{id}"))
            .decorated(false)
            .resizable(false)
            .deletable(false)
            .default_width(size.width)
            .default_height(size.height)
            .build();
        window.add_css_class("octosnap-pin");

        let picture = gtk::Picture::for_filename(&capture.path);
        picture.set_content_fit(gtk::ContentFit::Fill);
        picture.set_can_shrink(true);

        // Dragging the *image* moves the pin (`spec/07` §3.1). `GtkWindowHandle` asks the
        // compositor to run the move, which is the only way a Wayland client can be
        // dragged at all -- doing it by hand would mean a `move_frame` per motion event,
        // each a D-Bus round trip, and the pin would lag behind the pointer.
        //
        // The handle wraps only the picture, and the chrome is its **sibling** in the
        // overlay rather than its descendant. That is not tidiness: `set_can_target(false)`
        // is what stops the handle dragging in lock mode, GTK's picking does not descend
        // into a widget that cannot be targeted, and with the chrome inside the handle
        // turning one off would take the unlock control with it -- which is D45's
        // one-way door by a different route. See `set_locked`.
        let handle = gtk::WindowHandle::new();
        handle.set_child(Some(&picture));

        // The layer `spec/07` §3.1's opacity applies to: the capture and the backing it
        // is composited against, and nothing else.
        //
        // The opacity used to be the *window's*, which is the obvious place for it and
        // takes the controls with it: at 10 % the close and lock buttons are 10 % visible,
        // and so would any readout of the value be -- unreadable at exactly the setting
        // that most needs reading. Fading this layer instead leaves the chrome and the
        // opacity readout at full strength while the capture still shows the desktop
        // through it, which is the whole point of the setting.
        let image = gtk::Overlay::new();
        image.set_child(Some(&handle));
        image.add_css_class("octosnap-pin-body");
        image.set_overflow(gtk::Overflow::Hidden);

        // Window-sized, and the parent of everything that must stay legible. Also what
        // the input region, the controllers and `find_named` are measured against, since
        // it is the window's own child and so shares its coordinates.
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&image));
        overlay.set_size_request(size.width, size.height);
        overlay.add_css_class("octosnap-pin-root");
        overlay.set_focusable(true);
        // `spec/09` §3's "Pin | appear": the first frame is drawn with this and the next
        // ones transition out of it (`on_mapped`, the stylesheet below). Left off with
        // animations off, so a reduced-motion pin is simply there (D129).
        if adw::is_animations_enabled(&overlay) {
            overlay.add_css_class(ARRIVING);
        }

        let chrome = build_chrome();
        chrome.set_opacity(0.0);
        overlay.add_overlay(&chrome);

        let grip = build_grip();
        grip.set_opacity(0.0);
        overlay.add_overlay(&grip);

        let readout = build_opacity_readout();
        overlay.add_overlay(&readout.root);

        // `spec/07` §3.1's look, "both switchable": the corners and the hairline border
        // here, and the shadow in the compositor (`shadow_look`).
        let look = crate::settings::pin_look();
        if !look.rounded {
            image.add_css_class("square");
        }
        if look.bordered {
            overlay.add_css_class("bordered");
        }

        window.set_child(Some(&overlay));

        let pin = Rc::new(Self {
            id,
            capture: RefCell::new(capture),
            picture: picture.clone(),
            window: window.clone(),
            root: overlay.clone().upcast(),
            body: overlay.clone().upcast(),
            image: image.clone().upcast(),
            handle: handle.clone(),
            readout,
            readout_generation: Cell::new(0),
            readout_fade: RefCell::new(None),
            chrome: chrome.upcast(),
            size: Cell::new(size),
            current: Cell::new(opening),
            locked: Cell::new(false),
            hovered: Cell::new(false),
            opacity: Cell::new(1.0),
            look,
            base_size: Cell::new(size),
            zoom: Cell::new(1.0),
            grip: grip.upcast(),
            host: Rc::downgrade(host),
        });

        pin.wire_hover();
        pin.wire_clicks();
        pin.wire_scroll();
        pin.wire_shortcuts();
        pin.wire_chrome();
        pin.wire_drag();

        {
            // `Weak`, for the reason the card's map handler gives: a strong `Rc` on the
            // pin's own window is a cycle, and a closed pin would keep its texture for the
            // session (D60). The pins host holds the pin while it is shown.
            let pin = Rc::downgrade(&pin);
            window.connect_map(move |_| {
                if let Some(pin) = pin.upgrade() {
                    pin.on_mapped();
                }
            });
        }

        // Never `present()`: `spec/01` §5. A pin appearing must not take the keyboard any
        // more than a card must -- the extension's watcher covers both, because it keys
        // on the window title and `cards.ts` recognises this one too.
        window.set_visible(true);
        Some(pin)
    }

    /// The capture on show.
    #[must_use]
    pub fn capture(&self) -> CaptureResult {
        self.capture.borrow().clone()
    }

    /// Replaces the image, keeping the pin where the user left it.
    ///
    /// `spec/05` §4.11: "A pinned window showing this image updates after crop"
    /// [D 4.7.5]. The editor renders its document and hands the result over, so a pin
    /// tracks the *annotated* picture rather than the original capture -- which is what a
    /// user who has just cropped is looking at the pin to check.
    ///
    /// The **position** is kept and the **size** is taken from the new capture, and that
    /// asymmetry is the whole behaviour: where a pin sits is something the user chose by
    /// dragging it, and how big it is is a property of the picture. Placing it at the new
    /// capture's rect instead would send every pin back to where its screenshot was
    /// taken, which is the mistake `on_mapped` has a paragraph about.
    ///
    /// `false` when the new capture has no area, in which case nothing is changed --
    /// better a pin showing the previous image than an empty window.
    pub fn set_capture(self: &Rc<Self>, capture: CaptureResult) -> bool {
        if capture.rect.width <= 0 || capture.rect.height <= 0 {
            warn!(id = self.id, "a capture with no area cannot replace a pin's image");
            return false;
        }

        // `set_filename` and not a new `GtkPicture`: the picture is inside the window
        // handle that `spec/07` §3.1's drag depends on, and replacing the widget would
        // mean rebuilding that -- and with it the lock mode's `set_can_target` state,
        // which D45 is about.
        self.picture.set_filename(Some(&capture.path));
        let base = Size::new(capture.rect.width, capture.rect.height);
        *self.capture.borrow_mut() = capture;
        // A new picture is shown at 1:1, whatever zoom the old one had: its size is a
        // property of the picture, and this is a different picture.
        self.base_size.set(base);
        self.zoom.set(1.0);
        let size = self.resize_to(base);
        info!(
            id = self.id,
            width = size.width,
            height = size.height,
            "pin image replaced"
        );
        true
    }

    /// Gives the window a new size at the position it has, and asks the compositor to
    /// keep it there. Answers the size actually taken, which the work area may have
    /// clamped.
    fn resize_to(self: &Rc<Self>, wanted: Size) -> Size {
        let at = self.current.get();
        let work = work_area(Rect::new(at.x, at.y, wanted.width, wanted.height));
        let sized = pin::opening_rect(Rect::new(at.x, at.y, wanted.width, wanted.height), work);
        let size = Size::new(sized.width, sized.height);
        self.size.set(size);
        self.current.set(sized);
        self.root.set_size_request(size.width, size.height);
        self.window.set_default_size(size.width, size.height);

        // The input region is measured from the size, so a locked pin whose image changed
        // shape has to be told again -- otherwise its click-through hole is where the
        // corner *used* to be, which is D45's failure with different numbers.
        if self.locked.get() {
            self.set_locked(true);
        }

        // And the compositor has to be asked to put it back: a resize on Wayland is the
        // client changing its own surface, and Mutter is free to place the new one
        // wherever it likes.
        if let Some(path) = self.object_path()
            && let Some(host) = self.host.upgrade()
        {
            host.place(self.id, path, Placement::At { x: sized.x, y: sized.y });
        }
        size
    }

    /// `spec/07` §3.1's "resize from edges/corners keeping aspect", as a zoom of the
    /// capture's own size: Shift+scroll, the `+` and `-` keys, `0` for 1:1. The compositor
    /// runs every interactive resize on Wayland and offers no aspect constraint, so a
    /// resize that keeps the aspect can only be the client asking for a size -- which is
    /// what this does, in `core::pin`'s steps (D66).
    fn set_zoom(self: &Rc<Self>, zoom: f64) {
        let zoom = zoom.clamp(pin::ZOOM_MIN, pin::ZOOM_MAX);
        if (zoom - self.zoom.get()).abs() < 1e-9 {
            return;
        }
        self.zoom.set(zoom);
        let size = self.resize_to(pin::zoomed_size(self.base_size.get(), zoom));
        info!(id = self.id, zoom, width = size.width, height = size.height, "pin zoomed");
    }

    /// `spec/07` §3.1's "double-click toggles 1:1 / fit" [P]: a zoomed pin goes back to
    /// its capture's size; a pin at 1:1 that the work area cannot hold shrinks to fit.
    fn toggle_fit(self: &Rc<Self>) {
        if (self.zoom.get() - 1.0).abs() > 1e-9 {
            self.set_zoom(1.0);
            return;
        }
        let at = self.current.get();
        let fit = pin::fit_zoom(self.base_size.get(), work_area(at));
        if (fit - 1.0).abs() < 1e-9 {
            debug!(id = self.id, "pin already fits the work area at 1:1");
            return;
        }
        self.set_zoom(fit);
    }

    pub fn object_path(&self) -> Option<String> {
        let app = self.window.application()?;
        let base = app.dbus_object_path()?;
        Some(format!("{base}/window/{}", self.window.id()))
    }

    pub fn set_hidden(&self, hidden: bool) {
        self.window.set_visible(!hidden);
    }

    /// Records where the compositor says the window actually is.
    ///
    /// Every move a pin makes is run by the compositor and answered with a landed rect:
    /// `PlaceWindow` and `MoveWindowBy` both return one, because `move_frame` is clamped
    /// silently and the asked-for rect is therefore a guess. This is where those answers
    /// are kept, and `on_mapped` is what spends them.
    pub fn remember(&self, at: Rect) {
        self.current.set(at);
    }

    pub fn destroy(&self) {
        self.window.set_visible(false);
        self.window.destroy();
    }

    /// Places the pin once the compositor has its first frame.
    ///
    /// The same wait as a card's, and for the same reasons: the window is not in Mutter's
    /// stack until then, and `get_frame_rect()` answers `0x0`. `PlaceWindow` is
    /// asynchronous on the extension side and does the waiting there too, but the app has
    /// to hold off until the window is at least findable.
    ///
    /// This runs on *every* map, not only the first, and that is not a quirk to work
    /// around -- it is the only chance a pin gets to be positioned at all. `map` fires
    /// again each time `spec/07` §3.1's Hide/Show brings the pins back, and by then the
    /// toplevel is a new one the compositor has put wherever it liked.
    ///
    /// Which is why it places at `current` and not at the opening rect. Placing at the
    /// opening rect every time meant one Hide/Show cycle silently undid every arrow key
    /// and every drag, returning each pin to where its capture had been taken.
    fn on_mapped(self: &Rc<Self>) {
        let Some(surface) = self.window.surface() else {
            warn!(id = self.id, "pin has no surface at map time");
            return;
        };
        let clock = surface.frame_clock();
        let pin = Rc::downgrade(self);
        let handler = Rc::new(RefCell::new(None));
        let once = Rc::clone(&handler);
        *handler.borrow_mut() = Some(clock.connect_after_paint(move |clock| {
            if let Some(id) = once.borrow_mut().take() {
                clock.disconnect(id);
            } else {
                return;
            }
            let Some(pin) = pin.upgrade() else { return };
            // The first frame is up, so the arrival can start from it. Only the first
            // map's: a pin brought back by Hide/Show returns as it was left.
            pin.root.remove_css_class(ARRIVING);
            let Some(path) = pin.object_path() else { return };
            let at = pin.current.get();
            if let Some(host) = pin.host.upgrade() {
                host.place(
                    pin.id,
                    path,
                    // An explicit point, not a stack slot: `spec/07` §3.1 wants the pin
                    // "exactly where it was captured", and on the first map that is
                    // exactly what `current` holds. Nothing else gets a say in it.
                    Placement::At { x: at.x, y: at.y },
                );
            }
        }));
    }

    /// `spec/07` §3.1: the grip and the lock icon appear "on hover". At rest a pin is
    /// only the capture, which is the whole illusion.
    fn wire_hover(self: &Rc<Self>) {
        let motion = gtk::EventControllerMotion::new();
        {
            let pin = Rc::downgrade(self);
            motion.connect_enter(move |_, _, _| {
                if let Some(pin) = pin.upgrade() {
                    pin.hovered.set(true);
                    pin.show_chrome();
                    pin.lend_keyboard(true);
                }
            });
        }
        {
            let pin = Rc::downgrade(self);
            motion.connect_leave(move |_| {
                if let Some(pin) = pin.upgrade() {
                    pin.hovered.set(false);
                    pin.show_chrome();
                    pin.lend_keyboard(false);
                }
            });
        }
        self.body.add_controller(motion);
    }

    fn wire_clicks(self: &Rc<Self>) {
        // `spec/07` §3.1: "double-click toggles 1:1 / fit". In the capture phase and
        // claimed, so the window handle underneath does not also start a move on the
        // second press.
        let double = gtk::GestureClick::new();
        double.set_button(gdk::BUTTON_PRIMARY);
        double.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let pin = Rc::downgrade(self);
            double.connect_pressed(move |gesture, presses, _, _| {
                if presses != 2 {
                    return;
                }
                gesture.set_state(gtk::EventSequenceState::Claimed);
                if let Some(pin) = pin.upgrade() {
                    pin.toggle_fit();
                }
            });
        }
        self.body.add_controller(double);

        // `spec/07` §3.1: "Esc/middle-click/Ctrl+W closes".
        let middle = gtk::GestureClick::new();
        middle.set_button(gdk::BUTTON_MIDDLE);
        {
            let pin = Rc::downgrade(self);
            middle.connect_released(move |_, _, _, _| {
                if let Some(pin) = pin.upgrade()
                    && let Some(host) = pin.host.upgrade()
                {
                    host.close(pin.id);
                }
            });
        }
        self.body.add_controller(middle);

        // In the capture phase and claimed too: in the bubble phase the window handle under
        // the picture saw the press first and asked the compositor for its window menu, so
        // a right-click showed Mutter's Take Screenshot, Hide, Always on Top… and never
        // this menu (D136).
        let secondary = gtk::GestureClick::new();
        secondary.set_button(gdk::BUTTON_SECONDARY);
        secondary.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let pin = Rc::downgrade(self);
            secondary.connect_pressed(move |gesture, _, x, y| {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                if let Some(pin) = pin.upgrade() {
                    pin.open_menu(x, y);
                }
            });
        }
        self.body.add_controller(secondary);
    }

    /// `spec/07` §3.1: "opacity with two-finger scroll / Ctrl+scroll (10-100 %)".
    fn wire_scroll(self: &Rc<Self>) {
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let pin = Rc::downgrade(self);
        scroll.connect_scroll(move |controller, _, dy| {
            let Some(pin) = pin.upgrade() else { return glib::Propagation::Proceed };
            // Shift+scroll is the size (`spec/07` §3.1's resize); every other scroll is
            // the opacity the spec gives the plain wheel.
            if controller.current_event_state().contains(gdk::ModifierType::SHIFT_MASK) {
                pin.set_zoom(pin::adjust_zoom(pin.zoom.get(), -dy));
                return glib::Propagation::Stop;
            }
            // Scrolling *down* makes it fainter, which is the direction every volume and
            // zoom control on the desktop already uses.
            let next = pin::adjust_opacity(pin.opacity.get(), -dy);
            pin.set_pin_opacity(next);
            glib::Propagation::Stop
        });
        self.body.add_controller(scroll);
    }

    fn set_pin_opacity(self: &Rc<Self>, value: f64) {
        self.opacity.set(value);
        // The image layer, not the window: see the note where it is built. The controls
        // that change the value have to stay readable while it changes.
        self.image.set_opacity(value);
        self.show_opacity_readout(value);
        debug!(id = self.id, value, "pin opacity");
        // The shadow fades with the capture, or a faint pin would float in a dark frame.
        if let Some((radius, opacity)) = self.shadow_look()
            && let Some(path) = self.object_path()
            && let Some(host) = self.host.upgrade()
        {
            host.shade(self.id, path, radius, opacity);
        }
    }

    /// What the compositor should draw under this pin: its corner radius and its opacity,
    /// or `None` when the user turned the shadow off (D132).
    pub(super) fn shadow_look(&self) -> Option<(i32, f64)> {
        self.look.shadow.then(|| {
            let radius = if self.look.rounded { pin::RADIUS } else { 0 };
            (radius, self.opacity.get())
        })
    }

    /// Shows the opacity as a number and a bar, and takes it away again.
    ///
    /// Asked for from hardware on 2026-09-09: "please add like a disappearing progress bar
    /// of how much the opacity is when changing it." `spec/07` §3.1 specifies the range
    /// and the gesture and says nothing about feedback, which left Ctrl+scroll as a
    /// gesture whose only report was the thing it changed -- and near the ends of the
    /// range two notches look identical.
    ///
    /// A generation counter rather than a cancelled source, for D32's reason:
    /// `SourceId::remove` panics on a source that has already fired, from inside a GLib
    /// callback, which aborts rather than unwinds. Every scroll bumps the counter and
    /// schedules its own hide; a hide that finds the counter moved on does nothing. So
    /// there is no handle to keep and none to cancel.
    fn show_opacity_readout(self: &Rc<Self>, value: f64) {
        self.readout.set(value);
        self.readout.root.set_opacity(1.0);

        let generation = self.readout_generation.get().wrapping_add(1);
        self.readout_generation.set(generation);

        let pin = Rc::downgrade(self);
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(u64::from(READOUT_HOLD_MS)),
            move || {
                let Some(pin) = pin.upgrade() else { return };
                if pin.readout_generation.get() != generation {
                    // A later scroll owns the readout now.
                    return;
                }
                let animation = adw::TimedAnimation::builder()
                    .widget(&pin.readout.root)
                    .value_from(1.0)
                    .value_to(0.0)
                    .duration(READOUT_FADE_MS)
                    .easing(adw::Easing::EaseOutQuad)
                    .target(&adw::PropertyAnimationTarget::new(&pin.readout.root, "opacity"))
                    .build();
                animation.play();
                *pin.readout_fade.borrow_mut() = Some(animation);
            },
        );
    }

    /// `spec/07` §3.1's keyboard: arrows move, Shift moves further, Esc and Ctrl+W close.
    fn wire_shortcuts(self: &Rc<Self>) {
        let controller = gtk::ShortcutController::new();
        controller.set_scope(gtk::ShortcutScope::Local);

        let moves: [(&str, i32, i32, bool); 8] = [
            ("Left", -1, 0, false),
            ("Right", 1, 0, false),
            ("Up", 0, -1, false),
            ("Down", 0, 1, false),
            ("<Shift>Left", -1, 0, true),
            ("<Shift>Right", 1, 0, true),
            ("<Shift>Up", 0, -1, true),
            ("<Shift>Down", 0, 1, true),
        ];
        for (accel, sx, sy, fast) in moves {
            let Some(trigger) = gtk::ShortcutTrigger::parse_string(accel) else { continue };
            let pin = Rc::downgrade(self);
            let callback = gtk::CallbackAction::new(move |_, _| {
                if let Some(pin) = pin.upgrade() {
                    let step = pin::nudge(fast);
                    if let Some(path) = pin.object_path()
                        && let Some(host) = pin.host.upgrade()
                    {
                        host.nudge(pin.id, path, sx * step, sy * step);
                    }
                }
                glib::Propagation::Stop
            });
            controller.add_shortcut(gtk::Shortcut::new(Some(trigger), Some(callback)));
        }

        // `spec/07` §3.1 has lock mode "toggled with a shortcut or the hover control", and
        // Ctrl+L is ours. It matters more than a convenience: it is the one way out that
        // does not depend on a button being drawn in the right place.
        //
        // The rest of `spec/07` §3.1 by key, for hands that are already on the keyboard
        // and for the harness, which cannot scroll: `[` and `]` are the opacity, `-`, `+`
        // and `0` the size, Ctrl+E the editor.
        let actions: [(&str, PinAction); 14] = [
            ("Escape", |p| p.ask(|h, id| h.close(id))),
            ("<Control>w", |p| p.ask(|h, id| h.close(id))),
            ("<Control>c", |p| p.ask(|h, id| h.copy(id))),
            ("<Control>s", |p| p.ask(|h, id| h.save(id))),
            ("<Control>e", |p| p.ask(|h, id| h.annotate(id))),
            ("<Control>l", |p| p.set_locked(!p.locked.get())),
            ("bracketleft", |p| p.set_pin_opacity(pin::adjust_opacity(p.opacity.get(), -1.0))),
            ("bracketright", |p| p.set_pin_opacity(pin::adjust_opacity(p.opacity.get(), 1.0))),
            ("minus", |p| p.set_zoom(pin::adjust_zoom(p.zoom.get(), -1.0))),
            ("KP_Subtract", |p| p.set_zoom(pin::adjust_zoom(p.zoom.get(), -1.0))),
            ("plus", |p| p.set_zoom(pin::adjust_zoom(p.zoom.get(), 1.0))),
            ("equal", |p| p.set_zoom(pin::adjust_zoom(p.zoom.get(), 1.0))),
            ("KP_Add", |p| p.set_zoom(pin::adjust_zoom(p.zoom.get(), 1.0))),
            ("0", |p| p.set_zoom(1.0)),
        ];
        for (accel, action) in actions {
            let Some(trigger) = gtk::ShortcutTrigger::parse_string(accel) else { continue };
            let pin = Rc::downgrade(self);
            let callback = gtk::CallbackAction::new(move |_, _| {
                if let Some(pin) = pin.upgrade() {
                    action(&pin);
                }
                glib::Propagation::Stop
            });
            controller.add_shortcut(gtk::Shortcut::new(Some(trigger), Some(callback)));
        }

        self.window.add_controller(controller);
    }

    /// Borrows the keyboard while the pointer is over the pin, the same policy the cards
    /// use (`spec/04` §3).
    ///
    /// A pin needs the keyboard: `spec/07` §3.1 gives it arrow keys, Esc, Ctrl+W, Ctrl+C
    /// and Ctrl+S. It must not *take* it, for the same reason a card must not -- `ACT-01`
    /// lets `pin` be an after-capture action, so a pin can appear with no click behind it
    /// at all, while the user is typing somewhere else. Borrowing on hover gives the
    /// shortcuts exactly when they would be used and never otherwise.
    fn lend_keyboard(self: &Rc<Self>, entered: bool) {
        let Some(path) = self.object_path() else { return };
        if let Some(host) = self.host.upgrade() {
            host.hovered(self.id, path, entered);
        }
    }

    fn ask(self: &Rc<Self>, f: impl FnOnce(&Rc<dyn PinHost>, PinId)) {
        if let Some(host) = self.host.upgrade() {
            f(&host, self.id);
        }
    }

    fn wire_chrome(self: &Rc<Self>) {
        let buttons: [(&str, PinAction); 2] = [
            ("pin-close", |p| p.ask(|h, id| h.close(id))),
            ("pin-lock", |p| p.set_locked(!p.locked.get())),
        ];
        for (name, action) in buttons {
            let Some(widget) = find_named(&self.body, name) else { continue };
            let Ok(button) = widget.downcast::<gtk::Button>() else { continue };
            let pin = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(pin) = pin.upgrade() {
                    action(&pin);
                }
            });
        }
    }

    /// `spec/07` §3.1's lock mode: "click-through; only a small unlock handle at the
    /// top-right remains reactive".
    ///
    /// An input region, not `set_sensitive(false)`. Insensitive widgets still consume the
    /// click — the window swallows it and nothing happens, which is the opposite of what
    /// lock mode is for. The region is the only mechanism that lets the pointer reach the
    /// application underneath.
    ///
    /// The chrome is laid out *before* the region is applied, and from the same rectangle,
    /// because the two disagreeing is what made lock mode a one-way door: see
    /// `pin::reachable_when_locked`.
    ///
    /// **And the region alone is not enough for a pointer that is already inside.**
    /// Reported from hardware on 2026-09-09: "when locking, mouse staying on the pinned
    /// card, for one click, you can still move it". An input region is a property of the
    /// surface that the compositor re-reads on the next commit, and a pointer that has
    /// already entered keeps its delivery until then — so the click that follows the lock
    /// still reached `GtkWindowHandle` and still asked the compositor for a move. The
    /// region was right; it was simply not yet in force.
    ///
    /// So the handle is also made untargetable, which is the same lesson as D49's
    /// `isAlive`: **a guard has to prevent the call, not arrive after it.** GTK routes the
    /// stray event nowhere rather than to a drag, and the region does the click-through it
    /// was always for. It takes both, and the handle wraps only the picture precisely so
    /// that switching it off does not take the unlock control with it — GTK's picking does
    /// not descend into a widget that cannot be targeted.
    pub fn set_locked(self: &Rc<Self>, locked: bool) {
        self.locked.set(locked);
        self.lay_out_chrome(locked);
        self.handle.set_can_target(!locked);

        let Some(surface) = self.window.surface() else { return };
        let region = if locked {
            pin::unlock_handle(self.size.get())
        } else {
            // The whole window again. An empty region would be click-through everywhere,
            // including the handle that turns this off.
            Rect::new(0, 0, self.size.get().width, self.size.get().height)
        };
        surface.set_input_region(Some(&gtk::cairo::Region::create_rectangle(
            &gtk::cairo::RectangleInt::new(region.x, region.y, region.width, region.height),
        )));

        // On the root, not on the image layer: `locked` styles both the capture's outline
        // and the unlock button, and since the chrome became the image's *sibling* the
        // root is the only node that is an ancestor of both.
        if locked {
            self.body.add_css_class("locked");
            self.verify_unlock_control();
        } else {
            self.body.remove_css_class("locked");
        }
        self.show_chrome();
        debug!(id = self.id, locked, ?region, "pin lock");
    }

    /// `spec/07` §3.1: the controls appear "on hover", so at rest a pin is only the
    /// capture — with one exception. A locked pin keeps its handle visible whether or not
    /// the pointer is there, because it is the only way out and hiding it would leave a
    /// patch of screen that ignores you and no clue what to do about it.
    fn show_chrome(&self) {
        self.chrome.set_opacity(if self.locked.get() || self.hovered.get() { 1.0 } else { 0.0 });
        // The grip is the one control lock mode has no use for: everywhere but the
        // unlock handle is click-through, so a grip that stayed drawn would be a control
        // that cannot be taken. Hidden while locked, shown with the rest on hover.
        self.grip.set_opacity(if !self.locked.get() && self.hovered.get() { 1.0 } else { 0.0 });
        self.grip.set_can_target(!self.locked.get());
    }

    /// `spec/07` §3.1's "small Drag me grip" (`PIN-05`): drags the capture out as a file
    /// and an image, the way a card's thumbnail does. `dnd-finished` rather than
    /// `drag-end`, for the card's reason: only a receiver that took it counts. Alt held at
    /// the drop keeps the pin (`[D 4.5.1]`); otherwise the pin's job is done and it goes to
    /// the history.
    fn wire_drag(self: &Rc<Self>) {
        let source = gtk::DragSource::new();
        source.set_actions(gdk::DragAction::COPY);
        {
            let pin = Rc::downgrade(self);
            source.connect_prepare(move |_, _, _| {
                let pin = pin.upgrade()?;
                let capture = pin.capture();
                let file = gio::File::for_path(&capture.path);
                let mut providers = vec![gdk::ContentProvider::for_value(&file.to_value())];
                if let Ok(texture) = gdk::Texture::from_filename(&capture.path) {
                    providers.push(gdk::ContentProvider::for_value(&texture.to_value()));
                }
                Some(gdk::ContentProvider::new_union(&providers))
            });
        }
        {
            let pin = Rc::downgrade(self);
            source.connect_drag_begin(move |source, drag| {
                let Some(pin) = pin.upgrade() else { return };
                if let Some(paintable) = pin.picture.paintable() {
                    let size = pin.size.get();
                    source.set_icon(Some(&paintable), size.width / 2, size.height / 2);
                }
                let inner = Rc::downgrade(&pin);
                drag.connect_dnd_finished(move |_| {
                    let Some(pin) = inner.upgrade() else { return };
                    let keep = alt_held(&pin.window);
                    info!(id = pin.id, keep, "pin dragged out");
                    if let Some(host) = pin.host.upgrade() {
                        host.dragged_out(pin.id, keep);
                    }
                });
            });
        }
        self.grip.add_controller(source);
    }

    /// Puts the hover controls where the current mode can reach them.
    ///
    /// Unlocked, that is `spec/07` §3.1's pair — lock and close, inset from the corner.
    /// Locked, it is one button filling the reactive square exactly: Close goes away
    /// because there is nowhere left to draw it that takes a click, and the survivor grows
    /// into the whole handle so there is no dead 4 px rim around the only way out.
    fn lay_out_chrome(&self, locked: bool) {
        if let Some(close) = find_named(&self.body, "pin-close") {
            close.set_visible(!locked);
        }
        let handle = pin::unlock_handle(self.size.get());
        let margin = if locked { 0 } else { pin::CHROME_MARGIN };
        self.chrome.set_margin_top(margin);
        self.chrome.set_margin_end(margin);
        let Some(button) = find_named(&self.body, "pin-lock").and_downcast::<gtk::Button>() else {
            return;
        };
        if locked {
            button.set_size_request(handle.width, handle.height);
        } else {
            button.set_size_request(pin::CHROME_BUTTON, pin::CHROME_BUTTON);
        }
        if let Some(image) = button.child().and_downcast::<gtk::Image>() {
            image.set_icon_name(Some(if locked {
                "changes-allow-symbolic"
            } else {
                "changes-prevent-symbolic"
            }));
        }
        button.set_tooltip_text(Some(if locked { "Unlock (Ctrl+L)" } else { "Lock (Ctrl+L)" }));
    }

    /// Reads back where the unlock control actually landed, once the frame that moved it
    /// has been laid out and painted.
    ///
    /// The invariant is about the *drawn* button, not the one this file asked for: a
    /// stylesheet with padding, or one more control added to the row, would push it out of
    /// the reactive region without touching `lay_out_chrome`. Measuring turns a silently
    /// dead button into a line in the log.
    ///
    /// After paint, not `add_tick_callback`. A tick runs in the frame clock's update
    /// phase, *before* layout, so a widget measured there is still where the previous
    /// frame left it — which this checker duly reported, naming coordinates the button had
    /// already left. A verifier that reads stale geometry is worse than none: it cries
    /// about correct code and would say nothing if the layout it measured were the wrong
    /// one all along.
    fn verify_unlock_control(self: &Rc<Self>) {
        let Some(button) = find_named(&self.body, "pin-lock") else { return };
        let Some(surface) = self.window.surface() else { return };
        let body = self.body.clone();
        let pin = Rc::downgrade(self);
        let handler = Rc::new(RefCell::new(None));
        let once = Rc::clone(&handler);
        *handler.borrow_mut() = Some(surface.frame_clock().connect_after_paint(move |clock| {
            let Some(id) = once.borrow_mut().take() else { return };
            clock.disconnect(id);
            let Some(pin) = pin.upgrade() else { return };
            if !pin.locked.get() {
                return;
            }
            let Some(bounds) = button.compute_bounds(&body) else { return };
            #[allow(clippy::cast_possible_truncation)]
            let drawn = Rect::new(
                bounds.x().round() as i32,
                bounds.y().round() as i32,
                bounds.width().round() as i32,
                bounds.height().round() as i32,
            );
            if pin::reachable_when_locked(pin.size.get(), drawn) {
                debug!(id = pin.id, ?drawn, "unlock control reachable");
            } else {
                warn!(
                    id = pin.id,
                    ?drawn,
                    handle = ?pin::unlock_handle(pin.size.get()),
                    "unlock control unreachable: a locked pin cannot be unlocked by pointer"
                );
            }
        }));
    }

    /// `spec/07` §3.1's context menu, trimmed to what this build can do.
    fn open_menu(self: &Rc<Self>, x: f64, y: f64) {
        let menu = gio::Menu::new();
        let first = gio::Menu::new();
        first.append(Some("Copy"), Some("pin.copy"));
        first.append(Some("Save"), Some("pin.save"));
        first.append(Some("Annotate"), Some("pin.annotate"));
        first.append(Some("Copy Text"), Some("pin.recognise"));
        menu.append_section(None, &first);

        let second = gio::Menu::new();
        second.append(
            Some(if self.locked.get() { "Unlock" } else { "Lock" }),
            Some("pin.lock"),
        );
        // `spec/07` §3.1's "Opacity ▸": the five stops a menu can offer; the wheel and
        // the bracket keys have the ten in between.
        let opacity = gio::Menu::new();
        for percent in [100_i32, 75, 50, 25, 10] {
            opacity.append(Some(&format!("{percent} %")), Some(&format!("pin.opacity({percent})")));
        }
        second.append_submenu(Some("Opacity"), &opacity);
        if (self.zoom.get() - 1.0).abs() > 1e-9 {
            second.append(Some("Actual Size"), Some("pin.actual-size"));
        }
        menu.append_section(None, &second);

        let last = gio::Menu::new();
        last.append(Some("Close"), Some("pin.close"));
        last.append(Some("Close All Pinned"), Some("pin.close-all"));
        menu.append_section(None, &last);

        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(&self.body);
        popover.set_has_arrow(false);
        #[allow(clippy::cast_possible_truncation)]
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));

        let group = gio::SimpleActionGroup::new();
        let entries: [(&str, PinAction); 8] = [
            ("copy", |p| p.ask(|h, id| h.copy(id))),
            ("save", |p| p.ask(|h, id| h.save(id))),
            ("annotate", |p| p.ask(|h, id| h.annotate(id))),
            ("recognise", |p| p.ask(|h, id| h.recognise(id))),
            ("lock", |p| p.set_locked(!p.locked.get())),
            ("actual-size", |p| p.set_zoom(1.0)),
            ("close", |p| p.ask(|h, id| h.close(id))),
            ("close-all", |p| p.ask(|h, _| h.close_all_pins())),
        ];
        for (name, action) in entries {
            let entry = gio::SimpleAction::new(name, None);
            let pin = Rc::downgrade(self);
            entry.connect_activate(move |_, _| {
                if let Some(pin) = pin.upgrade() {
                    action(&pin);
                }
            });
            group.add_action(&entry);
        }
        {
            let opacity = gio::SimpleAction::new("opacity", Some(glib::VariantTy::INT32));
            let pin = Rc::downgrade(self);
            opacity.connect_activate(move |_, parameter| {
                let Some(percent) = parameter.and_then(glib::Variant::get::<i32>) else { return };
                if let Some(pin) = pin.upgrade() {
                    pin.set_pin_opacity(f64::from(percent) / 100.0);
                }
            });
            group.add_action(&opacity);
        }
        self.body.insert_action_group("pin", Some(&group));
        // On the next turn: GTK closes the menu before it runs the item that was clicked,
        // and the item finds its action through this parent (D136).
        popover.connect_closed(|popover| {
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
        popover.popup();
    }
}

/// The hover controls: a close button and a lock toggle, top-right.
///
/// Top-right because that is where the unlock handle has to be in lock mode
/// (`spec/07` §3.1), and a control that moves when you lock the thing would be a control
/// you have to hunt for at exactly the moment you want it.
/// How long the opacity readout stays after the last notch, and how long it takes to go.
///
/// Long enough to read the number after a burst of scrolling has stopped, short enough
/// not to sit on the capture. `spec/09` §3's motion table has no entry for this -- it is
/// not in the spec at all -- so the fade matches the hover timings either side of it.
const READOUT_HOLD_MS: u32 = 900;
const READOUT_FADE_MS: u32 = 200;

/// `spec/07` §3.1's opacity, as a number and a bar.
///
/// A struct rather than a bare widget because the label and the bar are set together and
/// forgetting one is the obvious mistake: a bar at 40 % beside the text "60 %" is worse
/// than no readout.
#[derive(Debug, Clone)]
struct OpacityReadout {
    root: gtk::Widget,
    label: gtk::Label,
    bar: gtk::LevelBar,
}

impl OpacityReadout {
    /// Sets both halves from one value, so they cannot disagree.
    fn set(&self, value: f64) {
        #[allow(clippy::cast_possible_truncation)]
        let percent = (value * 100.0).round() as i32;
        self.label.set_label(&format!("{percent} %"));
        self.bar.set_value(value);
    }
}

/// The readout, centred and click-through.
///
/// A `GtkLevelBar` rather than a `GtkProgressBar`: this is a level, not progress, and the
/// level bar is the one that reads as a setting's current position. Both are themed, and
/// the difference matters on a control the user is dragging through a range.
fn build_opacity_readout() -> OpacityReadout {
    let label = gtk::Label::new(Some("100 %"));
    label.add_css_class("octosnap-pin-opacity-value");

    let bar = gtk::LevelBar::builder()
        .min_value(pin::OPACITY_MIN)
        .max_value(pin::OPACITY_MAX)
        .value(pin::OPACITY_MAX)
        .mode(gtk::LevelBarMode::Continuous)
        .build();
    bar.set_size_request(96, 4);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.add_css_class("octosnap-pin-opacity");
    root.append(&label);
    root.append(&bar);
    root.set_halign(gtk::Align::Center);
    root.set_valign(gtk::Align::Center);
    // Feedback, never a control: a pin whose middle stopped being draggable because a
    // readout was sitting there would be a worse pin than one with no readout.
    root.set_can_target(false);
    root.set_opacity(0.0);

    OpacityReadout { root: root.upcast(), label, bar }
}

fn build_chrome() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, pin::CHROME_GAP);
    row.set_halign(gtk::Align::End);
    row.set_valign(gtk::Align::Start);
    row.set_margin_top(pin::CHROME_MARGIN);
    row.set_margin_end(pin::CHROME_MARGIN);
    row.add_css_class("octosnap-pin-chrome");

    // Lock *last*, so it is the one in the corner — the corner being the only place a
    // click still lands once it has been pressed. Close takes the inner slot, against the
    // usual convention, because the convention assumes a window whose corner is free.
    // Ordering the row the other way round is what broke lock mode: Close ended up in the
    // reactive square and Lock 28 px outside it, so locking a pin put a close button
    // exactly where the unlock button appeared to be.
    //
    // It also means locking moves nothing. The survivor grows from 20 px inset by 4 to the
    // full 24 px square, which shifts its centre by two pixels; a Lock control that jumped
    // across the row the instant you pressed it would be a control you then had to find.
    row.append(&chrome_button("pin-close", "window-close-symbolic", "Close"));
    row.append(&chrome_button("pin-lock", "changes-prevent-symbolic", "Lock (Ctrl+L)"));
    row
}

/// `spec/07` §3.1's "small Drag me grip on hover": bottom-left, where nothing else is,
/// with the word on it because a bare grip icon on a picture reads as part of the picture.
fn build_grip() -> gtk::Box {
    let grip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let icon = gtk::Image::from_icon_name("list-drag-handle-symbolic");
    icon.set_pixel_size(12);
    grip.append(&icon);
    grip.append(&gtk::Label::new(Some("Drag me")));
    grip.set_halign(gtk::Align::Start);
    grip.set_valign(gtk::Align::End);
    grip.set_margin_start(pin::CHROME_MARGIN + 2);
    grip.set_margin_bottom(pin::CHROME_MARGIN + 2);
    grip.set_tooltip_text(Some("Drag the picture into another application; hold Alt to keep the pin"));
    grip.add_css_class("octosnap-pin-grip");
    grip
}

/// Whether Alt is held on the window's seat, read at the moment of a drop.
fn alt_held(window: &gtk::ApplicationWindow) -> bool {
    gtk::prelude::WidgetExt::display(window)
        .default_seat()
        .and_then(|seat| seat.keyboard())
        .is_some_and(|keyboard| keyboard.modifier_state().contains(gdk::ModifierType::ALT_MASK))
}

fn chrome_button(name: &str, icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::new();
    let image = gtk::Image::from_icon_name(icon);
    image.set_pixel_size(14);
    button.set_child(Some(&image));
    button.set_widget_name(name);
    button.set_tooltip_text(Some(tooltip));
    button.add_css_class("octosnap-pin-button");
    button.set_size_request(pin::CHROME_BUTTON, pin::CHROME_BUTTON);
    button
}

fn find_named(root: &gtk::Widget, name: &str) -> Option<gtk::Widget> {
    if root.widget_name() == name {
        return Some(root.clone());
    }
    let mut child = root.first_child();
    while let Some(current) = child {
        if let Some(found) = find_named(&current, name) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}

/// The work area a pin is clamped into.
///
/// GDK rather than the extension, for the same reason the overlay reads it here: this
/// decides whether a pin needs pulling a few pixels inside, and a D-Bus round trip on
/// the pin path to learn a number the compositor is about to re-check anyway would cost
/// more than it is worth.
fn work_area(capture: Rect) -> Rect {
    let Some(display) = gdk::Display::default() else {
        return Rect::new(0, 0, 1920, 1080);
    };
    let monitors = display.monitors();
    let mut best: Option<(Rect, i64)> = None;
    let mut fallback: Option<Rect> = None;
    for index in 0..monitors.n_items() {
        if let Some(monitor) = monitors.item(index).and_downcast::<gdk::Monitor>() {
            let g = monitor.geometry();
            let rect = Rect::new(g.x(), g.y(), g.width(), g.height());
            // The monitor the capture is actually on, by how much of it each one holds.
            //
            // It used to be the *largest* monitor, "as a stand-in for the one the capture
            // came from". On one display that stand-in is exact and on two it is a bug:
            // `opening_rect` clamps the pin into this rect, so a capture taken on the
            // smaller display was clamped to the larger display's origin and the pin
            // opened on the wrong screen -- 960 px sideways, measured on a 960x600 + 1920
            // x1200 rig. `docs/decisions.md` D50.
            let held = capture.overlap_area(rect);
            if held > 0 && best.is_none_or(|(_, most)| held > most) {
                best = Some((rect, held));
            }
            // For a capture on no monitor at all, which the compositor should not produce
            // but a stale rect from a disconnected display would.
            if fallback.is_none_or(|f| rect.width * rect.height > f.width * f.height) {
                fallback = Some(rect);
            }
        }
    }
    best.map(|(rect, _)| rect)
        .or(fallback)
        .unwrap_or_else(|| Rect::new(0, 0, 1920, 1080))
}

/// The class a pin arrives out of: `spec/09` §3's "Pin | appear | 200 ms | ease-out-cubic |
/// scale 0.97→1 + fade", as a CSS transition, which is a transform GTK applies to the
/// widget as a whole without anything in the pin's tree having to move (D129).
const ARRIVING: &str = "arriving";

/// `spec/07` §3.1's window chrome, as our own artwork and numbers.
pub fn install_style() {
    let Some(display) = gdk::Display::default() else { return };
    let radius = pin::RADIUS;
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&format!(
        "
window.octosnap-pin,
window.octosnap-pin > * {{
    background: none;
    background-color: transparent;
    box-shadow: none;
}}

/* `spec/09` §3's appear. The curve is ease-out-cubic, as CSS writes it. */
.octosnap-pin-root {{
    transition: opacity 200ms cubic-bezier(0.33, 1, 0.68, 1),
                transform 200ms cubic-bezier(0.33, 1, 0.68, 1);
}}

.octosnap-pin-root.arriving {{
    opacity: 0;
    transform: scale(0.97);
}}

.octosnap-pin-body {{
    border-radius: {radius}px;
    background-color: black;
    outline: 1px solid rgba(0, 0, 0, 0.55);
    outline-offset: -1px;
}}

/* `spec/07` §3.1's look, 'both switchable': no corners, and a hairline border that
   separates a pale capture from a pale desktop the way a card's does. */
.octosnap-pin-body.square {{
    border-radius: 0;
}}

.octosnap-pin-root.bordered .octosnap-pin-body {{
    outline: 1px solid rgba(0, 0, 0, 0.9);
}}

/* The Drag me grip: a small dark pill, like the opacity readout, in the corner the
   chrome leaves free. */
.octosnap-pin-grip {{
    padding: 2px 8px;
    border-radius: 10px;
    background-color: rgba(0, 0, 0, 0.62);
    color: #ffffff;
    font-size: 0.8em;
}}

/* `spec/07` §3.1's lock mode is invisible by design -- the pin is meant to look like part
   of the screen -- but a locked pin that looks identical to an unlocked one leaves the
   user with no way to tell why their clicks stopped landing on it. One hairline. */
.octosnap-pin-root.locked .octosnap-pin-body {{
    outline: 1px solid alpha(@accent_color, 0.7);
}}

/* The opacity readout (`spec/07` §3.1's Ctrl+scroll), which has to stay legible on a pin
   faded to 10 %. It can, because the fade is applied to the capture's own layer rather
   than to the window -- see where `image` is built. */
.octosnap-pin-opacity {{
    padding: 8px 12px;
    border-radius: 12px;
    background-color: rgba(0, 0, 0, 0.72);
    color: #ffffff;
}}

.octosnap-pin-opacity-value {{
    font-size: 0.9em;
    font-weight: bold;
    /* Tabular figures, so the number does not shift sideways as it counts through the
       range -- which is the one thing a readout must not do while being read. */
    font-feature-settings: \"tnum\";
}}

.octosnap-pin-opacity levelbar block.filled {{
    background-color: #ffffff;
    border-radius: 2px;
}}

.octosnap-pin-opacity levelbar trough {{
    background-color: rgba(255, 255, 255, 0.25);
    border: none;
    border-radius: 2px;
    padding: 0;
}}

.octosnap-pin-button {{
    padding: 0;
    border-radius: 10px;
    background-color: rgba(0, 0, 0, 0.45);
    color: #ffffff;
    border: none;
    box-shadow: none;
}}

.octosnap-pin-button:hover {{
    background-color: rgba(255, 255, 255, 0.25);
}}

/* The one control lock mode leaves behind, filling the reactive corner exactly. It wears
   the accent the locked outline wears, because the two are one signal: this patch of pin
   is the only part still listening, and here is where it listens. Its outer corner
   follows the pin's own {radius} px rather than the button's 10, so a control flush with
   the edge does not fight the shape it is sitting in. */
.octosnap-pin-root.locked .octosnap-pin-button {{
    border-radius: 10px {radius}px 10px 10px;
    background-color: alpha(@accent_color, 0.85);
}}
"
    ));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
