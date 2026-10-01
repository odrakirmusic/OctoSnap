// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/07` §4.2's capture history: a strip along the top edge of the screen.
//!
//! > History is **not a window with a grid and a search field** … It is a wide horizontal
//! > strip anchored to the top edge of the screen … closer in spirit to the Quick Access
//! > Overlay than to a document browser. That choice keeps it a glance-and-grab surface
//! > rather than a place you go to manage files.
//!
//! So it is built the way a card is, not the way the editor is: an undecorated window the
//! extension places (a Wayland client cannot position itself), never `present()`ed, given
//! the keyboard on loan through `FocusWindow` so Escape and the arrows work, and gone the
//! moment it has done its job -- a restore, an annotate, a pin all close it, because
//! `spec/09` §1's "invoke → act → vanish" applies to a surface you glance at more than to
//! any other. Filter chips across the top, one row of thumbnails newest first with the
//! source app on each and a relative time beneath, a ring and a **Restore** pill on the
//! selection, and the rest of `HIS-01` -- Annotate on double-click, Pin, Copy, Save as…,
//! Delete, Clear history -- in a context menu and the overflow, where they do not compete
//! with the one action the strip exists for.
//!
//! **Every tile is the same** (D109), which is what the reference shows and what the first
//! strip did not do: a 16:10 card with the picture filling it, a badge in its corner saying
//! what kind of capture it is -- the capture toolbar's own icon where it has one -- and the
//! source app in the other. The row is a `GtkListView`: a new capture splices one tile in
//! rather than rebuilding the row, only the tiles near the screen are given a picture --
//! read on a worker, and kept between opens -- and the clock rewrites the time labels in
//! place instead of reading every picture again once a minute.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use octosnap_core::history::{Entry, Filter, THUMB_FILE, relative_time};
use octosnap_core::qao::{MARGIN, SHADOW_MARGIN};
use octosnap_shell::{Placement, ShellBridge};
use tracing::{debug, info, warn};

use super::History;

/// The window's title, which is how the extension recognises OctoSnap's floating
/// windows (`cards.ts`): a window it must not let keep the keyboard uninvited.
pub const TITLE: &str = "octosnap-history";

/// Every tile's size, the reference strip's 16:10 card (D109). Its thumbnails are made at
/// twice this (`core::history::THUMB_MAX`).
const TILE_WIDTH: i32 = 208;
const TILE_HEIGHT: i32 = 130;
/// The space between two tiles, half of it on each side of each.
const TILE_GAP: i32 = 22;
/// A row's height: the card, the Restore pill beneath it, and the row's padding. The
/// empty message stands as tall, so a chip with nothing under it does not fold the strip
/// up for the next chip to unfold again.
const ROW_HEIGHT: i32 = TILE_HEIGHT + 6 + 24 + 8;
/// How many tile pictures stay decoded between opens, at about 430 KB each: more than the
/// tiles near the screen hold (`Strip::load_near`), so scrolling back finds them here.
const PICTURES_KEPT: usize = 48;
/// How long a wheel click's scroll takes. A wheel moves the row a tile at a time, and a
/// jump of a tile reads as the row teleporting; this is the slide instead.
const WHEEL_MS: u32 = 180;
/// The strip's widest, so that on a 3440 px display it does not become a shelf.
const MAX_WIDTH: i32 = 1600;
/// The gap between the strip and the top of the work area.
const TOP_GAP: i32 = 8;
/// The body's corner radius, wider than a card's because the surface is wider.
const RADIUS: i32 = 16;
/// How often the relative times ("3 minutes ago") are re-read while the strip is up.
const CLOCK_S: u32 = 60;

thread_local! {
    /// The one strip. `open-history` toggles it: a second activation closes rather than
    /// stacking a second copy behind the first.
    static OPEN: RefCell<Option<Rc<Strip>>> = const { RefCell::new(None) };
    static STYLED: Cell<bool> = const { Cell::new(false) };
    /// Tile pictures by thumbnail path, so a strip opened again draws at once.
    static PICTURES: RefCell<Pictures> = RefCell::new(Pictures::default());
    /// The tiles waiting for a picture that is being read, by thumbnail path: a picture
    /// is read once however many tiles ask for it while it is on its way.
    static WAITING: RefCell<HashMap<PathBuf, Vec<Waiter>>> = RefCell::new(HashMap::new());
}

/// A tile waiting for a picture, and the binding it asked from: a tile rebound since then
/// wants something else.
type Waiter = (Weak<Tile>, u64);

/// The tile pictures read so far, least recently shown dropped first.
#[derive(Default)]
struct Pictures {
    textures: HashMap<PathBuf, gdk::Texture>,
    order: VecDeque<PathBuf>,
}

impl Pictures {
    fn get(&mut self, path: &PathBuf) -> Option<gdk::Texture> {
        let texture = self.textures.get(path)?.clone();
        if let Some(at) = self.order.iter().position(|p| p == path) {
            self.order.remove(at);
        }
        self.order.push_back(path.clone());
        Some(texture)
    }

    fn put(&mut self, path: PathBuf, texture: gdk::Texture) {
        if self.textures.insert(path.clone(), texture).is_none() {
            self.order.push_back(path);
        }
        while self.order.len() > PICTURES_KEPT {
            if let Some(oldest) = self.order.pop_front() {
                self.textures.remove(&oldest);
            }
        }
    }
}

/// `open-history`: shows the strip, or closes it if it is up.
pub fn toggle(app: &adw::Application) {
    if let Some(open) = OPEN.with(|o| o.borrow_mut().take()) {
        open.close();
        return;
    }
    let Some(history) = crate::history() else {
        warn!("open-history with no history store");
        return;
    };
    let strip = Strip::open(app, history);
    OPEN.with(|o| *o.borrow_mut() = Some(strip));
}

/// One tile the row has built: bound to an entry while it is on screen, waiting to be
/// bound to another when it is not -- a `GtkListView` recycles its widgets.
struct Tile {
    /// The list item the tile is the child of, from setup to teardown: its position is the
    /// tile's place in the row.
    item: glib::WeakRef<gtk::ListItem>,
    root: gtk::Box,
    picture: gtk::Picture,
    /// The kind's icon, large and faint, until the picture arrives -- or for good, when
    /// there is no picture to be had.
    placeholder: gtk::Image,
    kind: gtk::Image,
    /// A recording's length beside its kind's icon.
    length: gtk::Label,
    /// Where the source app's badge goes; emptied and refilled on every bind.
    app: gtk::Box,
    frame: gtk::Overlay,
    time: gtk::Label,
    /// The time label, or the Restore pill when selected.
    footer: gtk::Stack,
    /// The entry this tile shows; empty while it shows none.
    id: RefCell<String>,
    created_at: Cell<u64>,
    /// Moved on at every bind and unbind, so a picture that arrives for an entry the tile
    /// no longer shows is dropped.
    generation: Cell<u64>,
    /// Where the bound entry's picture comes from; `None` when it has none.
    wanted: RefCell<Option<Wanted>>,
    showing: Cell<Showing>,
}

/// Where a tile's picture comes from.
#[derive(Debug, Clone)]
struct Wanted {
    thumb: PathBuf,
    /// The capture, for making the thumbnail again when it is missing or the wrong shape.
    capture: PathBuf,
    size: (u32, u32),
}

/// Where a tile's picture has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Showing {
    /// None: the tile has just been bound, or is too far from the screen to hold one.
    Nothing,
    /// Being read.
    Waiting,
    Picture,
    /// There is none to be had, and asking again would not change that.
    Missing,
}

pub struct Strip {
    app: adw::Application,
    history: Rc<History>,
    window: gtk::ApplicationWindow,
    body: gtk::Box,
    list: gtk::ListView,
    scroller: gtk::ScrolledWindow,
    /// The entries the row shows, as the list view's model.
    store: gio::ListStore,
    /// The same entries, for comparing against the next refresh and for the keys.
    shown: RefCell<Vec<Entry>>,
    pages: gtk::Stack,
    empty: Empty,
    filter: Cell<Filter>,
    selected: RefCell<Vec<String>>,
    /// Every tile the row has built, bound or not.
    tiles: RefCell<Vec<Rc<Tile>>>,
    /// Whether a refresh is already waiting for the main loop: the janitor filing seven
    /// captures at launch is one refresh, not seven.
    refresh_queued: Cell<bool>,
    /// The same for a look at which tiles are near the screen, which a slide asks for on
    /// every frame.
    load_queued: Cell<bool>,
    /// The wheel's slide in flight and where it is going, so a second click carries on
    /// from the first one's destination rather than from wherever the slide had got to.
    wheel: RefCell<Option<(adw::TimedAnimation, f64)>>,
    /// Where the extension is asked to put the strip, once the monitor is known.
    target: Cell<Option<(i32, i32)>>,
    /// Whether the keyboard is on loan to the strip, so it can be given back on close.
    lent: Cell<bool>,
    clock: RefCell<Option<glib::SourceId>>,
}

/// `spec/09` §4b's empty history, "always seen on day one": what the strip is for and how
/// to fill it, rather than a line of grey text.
///
/// The shortcut is read once, when the strip is built. The user who changes it has
/// Settings open and the strip closed, and the next open reads it again.
struct Empty {
    page: gtk::Box,
    title: gtk::Label,
    description: gtk::Label,
    /// How to take a capture, in the words of the keys the user has.
    how: Hint,
    /// The welcome window, for when there is no extension to take a capture with.
    set_up: gtk::Button,
}

impl Empty {
    fn new() -> Self {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 6);
        page.set_valign(gtk::Align::Center);
        page.set_halign(gtk::Align::Center);
        let icon = gtk::Image::from_icon_name("io.github.odrakirmusic.OctoSnap-symbolic");
        icon.set_pixel_size(40);
        icon.add_css_class("dim-label");
        icon.set_margin_bottom(4);
        let title = gtk::Label::new(None);
        title.add_css_class("title-4");
        let description = gtk::Label::new(None);
        description.add_css_class("dim-label");
        description.set_wrap(true);
        description.set_justify(gtk::Justification::Center);
        description.set_max_width_chars(64);
        let set_up = gtk::Button::builder()
            .label("Set Up\u{2026}")
            .action_name("app.welcome")
            .halign(gtk::Align::Center)
            .margin_top(6)
            .css_classes(["pill", "suggested-action"])
            .visible(false)
            .build();
        page.append(&icon);
        page.append(&title);
        page.append(&description);
        page.append(&set_up);
        Self { page, title, description, how: Hint::find(), set_up }
    }

    fn show(&self, filter: Filter, kept_for: &str) {
        match filter {
            Filter::All => {
                self.title.set_text("No captures yet");
                self.description
                    .set_text(&format!("Captures you close stay here for {kept_for}. {}", self.how.sentence()));
                self.set_up.set_visible(matches!(self.how, Hint::SetUp));
            }
            other => {
                // The chip's word in a sentence, where only an acronym keeps its capitals.
                let noun = match other {
                    Filter::Gifs => other.label().to_owned(),
                    _ => other.label().to_lowercase(),
                };
                self.title.set_text(&format!("No {noun} yet"));
                self.description.set_text(&format!("{} you close stay here for {kept_for}.", other.label()));
                self.set_up.set_visible(false);
            }
        }
    }
}

/// How to take the first capture, for the empty strip.
enum Hint {
    /// Capture Area's first binding, as GTK labels it. `spec/08` §2's own default is
    /// Ctrl+Alt+Super+A, and the Print keys, when OctoSnap has them, put Shift+Print there.
    Key(String),
    /// The extension with no key for Capture Area: its menu in the top bar.
    Menu,
    /// No extension, so neither a key nor a menu: every capture is the extension's.
    SetUp,
}

impl Hint {
    fn find() -> Self {
        let Some(ext) = crate::settings::extension_settings() else {
            return Self::SetUp;
        };
        ext.settings_schema()
            .filter(|schema| schema.has_key("capture-area"))
            .and_then(|_| ext.strv("capture-area").first().map(ToString::to_string))
            .and_then(|accel| gtk::accelerator_parse(&accel))
            .map_or(Self::Menu, |(key, mods)| Self::Key(gtk::accelerator_get_label(key, mods).to_string()))
    }

    fn sentence(&self) -> String {
        match self {
            Self::Key(key) => format!("{key} captures an area."),
            Self::Menu => "Take one from OctoSnap's menu in the top bar.".to_owned(),
            Self::SetUp => "OctoSnap's GNOME Shell extension takes them, and it is not set up yet.".to_owned(),
        }
    }
}

impl std::fmt::Debug for Strip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Strip")
            .field("filter", &self.filter.get())
            .field("shown", &self.shown.borrow().len())
            .field("selected", &self.selected.borrow().len())
            .finish_non_exhaustive()
    }
}

impl Strip {
    fn open(app: &adw::Application, history: Rc<History>) -> Rc<Self> {
        install_style();

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title(TITLE)
            .decorated(false)
            .resizable(false)
            .deletable(false)
            .build();
        window.add_css_class("octosnap-history");

        // The body carries the look; the window around it is transparent so the shadow
        // band `SHADOW_MARGIN` reserves is genuinely empty, as it is on a card.
        let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
        body.add_css_class("octosnap-history-body");
        body.set_margin_top(SHADOW_MARGIN);
        body.set_margin_bottom(SHADOW_MARGIN);
        body.set_margin_start(SHADOW_MARGIN);
        body.set_margin_end(SHADOW_MARGIN);

        let header = gtk::CenterBox::new();
        let title = gtk::Label::new(Some("History"));
        title.add_css_class("heading");
        title.add_css_class("dim-label");
        header.set_start_widget(Some(&title));
        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_center_widget(Some(&chips));
        let overflow = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("More")
            .has_frame(false)
            .can_focus(false)
            .build();
        header.set_end_widget(Some(&overflow));
        body.append(&header);

        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let list = gtk::ListView::new(
            Some(gtk::NoSelection::new(Some(store.clone()))),
            None::<gtk::ListItemFactory>,
        );
        list.set_orientation(gtk::Orientation::Horizontal);
        list.add_css_class("octosnap-history-row");
        // Nothing in the strip takes the keyboard: it is the strip's (see `wire_keys`).
        list.set_can_focus(false);
        list.set_focusable(false);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&list)
            .build();
        let empty = Empty::new();
        let pages = gtk::Stack::new();
        pages.set_size_request(-1, ROW_HEIGHT);
        pages.add_named(&scroller, Some("row"));
        pages.add_named(&empty.page, Some("empty"));
        body.append(&pages);

        // A plain wrapper between the window and the body: the stylesheet makes the
        // window and its direct child transparent so the shadow band is empty, and a
        // body that *was* the direct child lost its own background to that rule -- the
        // first strip was chips floating over the wallpaper.
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 0);
        wrapper.append(&body);
        window.set_child(Some(&wrapper));

        let strip = Rc::new(Self {
            app: app.clone(),
            history,
            window: window.clone(),
            body,
            list: list.clone(),
            scroller: scroller.clone(),
            store,
            shown: RefCell::new(Vec::new()),
            pages,
            empty,
            filter: Cell::new(Filter::All),
            selected: RefCell::new(Vec::new()),
            tiles: RefCell::new(Vec::new()),
            refresh_queued: Cell::new(false),
            load_queued: Cell::new(false),
            wheel: RefCell::new(None),
            target: Cell::new(None),
            lent: Cell::new(false),
            clock: RefCell::new(None),
        });

        list.set_factory(Some(&strip.tile_factory()));
        {
            // A scroll brings tiles near the screen, and the first allocation says how wide
            // the screen is.
            let adjustment = scroller.hadjustment();
            let weak = Rc::downgrade(&strip);
            adjustment.connect_value_changed(move |_| {
                if let Some(strip) = weak.upgrade() {
                    strip.queue_load();
                }
            });
            let weak = Rc::downgrade(&strip);
            adjustment.connect_changed(move |_| {
                if let Some(strip) = weak.upgrade() {
                    strip.queue_load();
                }
            });
        }
        strip.build_chips(&chips);
        overflow.set_menu_model(Some(&strip.overflow_menu()));
        strip.wire_keys();
        strip.wire_wheel();
        strip.refresh();
        // The newest capture starts selected, as `history-panel.png` shows it, so the
        // strip opens ready for the one keystroke it is most often opened for.
        if let Some(first) = strip.shown.borrow().first().map(|e| e.id.clone()) {
            strip.select(&first, false);
        }

        {
            let weak = Rc::downgrade(&strip);
            strip.history.connect_changed(move || {
                let Some(strip) = weak.upgrade() else { return false };
                strip.queue_refresh();
                true
            });
        }
        {
            // The times move on; nothing else does. Rewriting a label per tile on screen
            // is all a minute costs now -- it used to be the whole row rebuilt and every
            // picture read again.
            let weak = Rc::downgrade(&strip);
            let id = glib::timeout_add_seconds_local(CLOCK_S, move || match weak.upgrade() {
                Some(strip) => {
                    strip.tick();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            });
            *strip.clock.borrow_mut() = Some(id);
        }
        {
            // `Weak`, as every window handler here is: the strip owns the window.
            let weak = Rc::downgrade(&strip);
            window.connect_map(move |_| {
                if let Some(strip) = weak.upgrade() {
                    strip.on_mapped();
                }
            });
        }

        // Sized and shown once the monitor is known, because the width is the work
        // area's and the position is the extension's to compute from it. The window is
        // built first so that a second `open-history` in the meantime finds one to close.
        let weak = Rc::downgrade(&strip);
        glib::spawn_future_local(async move {
            let work = match crate::capture_flow() {
                Some(flow) => flow.bridge().monitors().await.ok().and_then(|monitors| {
                    monitors
                        .iter()
                        .find(|m| m.current)
                        .or_else(|| monitors.iter().find(|m| m.primary))
                        .or_else(|| monitors.first())
                        .map(|m| m.work_area)
                }),
                None => None,
            };
            let Some(strip) = weak.upgrade() else { return };
            // Without an extension to ask, a plausible screen; Mutter clamps the rest.
            let work = work.unwrap_or(octosnap_core::Rect::new(0, 0, 1920, 1200));
            let width = (work.width - 2 * MARGIN).clamp(320, MAX_WIDTH);
            strip.body.set_size_request(width, -1);
            let window_width = width + 2 * SHADOW_MARGIN;
            strip.window.set_default_size(window_width, -1);
            strip.target.set(Some((
                work.x + (work.width - window_width) / 2,
                (work.y + TOP_GAP - SHADOW_MARGIN).max(work.y),
            )));
            // Never `present()`: `spec/01` §5. The keyboard arrives by `FocusWindow` once
            // the strip is placed, which the extension can account for.
            strip.window.set_visible(true);
            info!(width, entries = strip.history.len(), "history strip opened");
        });
        strip
    }

    /// Closes the strip and gives the keyboard back.
    pub fn close(self: &Rc<Self>) {
        OPEN.with(|o| {
            let mut open = o.borrow_mut();
            if open.as_ref().is_some_and(|s| Rc::ptr_eq(s, self)) {
                *open = None;
            }
        });
        if let Some(id) = self.clock.borrow_mut().take() {
            id.remove();
        }
        let window = self.window.clone();
        let finish = move || {
            window.set_visible(false);
            window.destroy();
            info!("history strip closed");
        };
        // The keyboard goes back *before* the window goes: `FocusWindow` looks the window
        // up by its path, and asking after the destroy is an exception in the shell's
        // log for a window that no longer exists.
        if self.lent.replace(false)
            && let (Some(flow), Some(path)) = (crate::capture_flow(), self.object_path())
        {
            glib::spawn_future_local(async move {
                if let Err(e) = flow.bridge().focus_window(&path, false).await {
                    debug!("could not give the keyboard back: {e}");
                }
                finish();
            });
            return;
        }
        finish();
    }

    fn object_path(&self) -> Option<String> {
        let base = self.app.dbus_object_path()?;
        Some(format!("{base}/window/{}", self.window.id()))
    }

    /// Placed once the first frame has been drawn, the way a pin is, then lent the keyboard.
    fn on_mapped(self: &Rc<Self>) {
        let Some(surface) = self.window.surface() else { return };
        let clock = surface.frame_clock();
        let weak = Rc::downgrade(self);
        let handler = Rc::new(RefCell::new(None));
        let once = Rc::clone(&handler);
        *handler.borrow_mut() = Some(clock.connect_after_paint(move |clock| {
            if let Some(id) = once.borrow_mut().take() {
                clock.disconnect(id);
            } else {
                return;
            }
            let Some(strip) = weak.upgrade() else { return };
            strip.place();
        }));
    }

    fn place(self: &Rc<Self>) {
        let (Some(path), Some((x, y)), Some(flow)) = (self.object_path(), self.target.get(), crate::capture_flow()) else {
            return;
        };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            match flow.bridge().place_window(&path, "history", &Placement::At { x, y }, 0).await {
                Ok(landed) => debug!(?landed, "history strip placed"),
                Err(e) => {
                    warn!("could not place the history strip: {e}");
                    return;
                }
            }
            match flow.bridge().focus_window(&path, true).await {
                Ok(focused) => {
                    if let Some(strip) = weak.upgrade() {
                        strip.lent.set(focused);
                    }
                }
                Err(e) => debug!("could not lend the keyboard to the strip: {e}"),
            }
        });
    }

    // --- the chips and the overflow -------------------------------------------------

    fn build_chips(self: &Rc<Self>, into: &gtk::Box) {
        let mut first: Option<gtk::ToggleButton> = None;
        for filter in Filter::ALL {
            // This build records GIFs and nothing else, so a Videos chip could only ever
            // show an empty strip. It comes back with the video recorder (`spec/06`).
            if filter == Filter::Videos {
                continue;
            }
            // The chip carries its kind's icon, the same one its tiles wear (D109).
            let chip = match filter.icon() {
                Some(icon) => {
                    let content = adw::ButtonContent::builder()
                        .icon_name(icon)
                        .label(filter.label())
                        .build();
                    gtk::ToggleButton::builder().child(&content).build()
                }
                None => gtk::ToggleButton::with_label(filter.label()),
            };
            chip.add_css_class("octosnap-history-chip");
            chip.set_can_focus(false);
            chip.set_active(filter == self.filter.get());
            match &first {
                Some(group) => chip.set_group(Some(group)),
                None => first = Some(chip.clone()),
            }
            let weak = Rc::downgrade(self);
            chip.connect_toggled(move |chip| {
                if !chip.is_active() {
                    return;
                }
                if let Some(strip) = weak.upgrade() {
                    strip.filter.set(filter);
                    strip.selected.borrow_mut().clear();
                    strip.refresh();
                    strip.scroller.hadjustment().set_value(0.0);
                    // The newest of the kind starts selected, as the newest of all does
                    // when the strip opens: Return restores it either way.
                    let first = strip.shown.borrow().first().map(|e| e.id.clone());
                    if let Some(first) = first {
                        strip.select(&first, false);
                    }
                    debug!(filter = filter.label(), "history filter");
                }
            });
            into.append(&chip);
        }
    }

    fn overflow_menu(self: &Rc<Self>) -> gio::Menu {
        let menu = gio::Menu::new();
        menu.append(Some("Select All"), Some("strip.select-all"));
        menu.append(Some("Clear History\u{2026}"), Some("strip.clear"));
        let group = gio::SimpleActionGroup::new();
        for (name, run) in [
            ("select-all", (|s: &Rc<Strip>| s.select_all()) as fn(&Rc<Strip>)),
            ("clear", |s| s.confirm_clear()),
        ] {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(strip) = weak.upgrade() {
                    run(&strip);
                }
            });
            group.add_action(&action);
        }
        self.window.insert_action_group("strip", Some(&group));
        menu
    }

    // --- the row --------------------------------------------------------------------

    /// A refresh on the next turn of the main loop, once however many changes arrive
    /// before it.
    fn queue_refresh(&self) {
        if self.refresh_queued.replace(true) {
            return;
        }
        let weak = self.weak();
        glib::idle_add_local_once(move || {
            if let Some(strip) = weak.upgrade() {
                strip.refresh();
            }
        });
    }

    /// Brings the row up to date with the store, touching only what changed.
    ///
    /// The entries the old row and the new one start and end with stay where they are,
    /// bound to the tiles already showing them: a capture filed while the strip is up is
    /// one tile spliced in at the front, not a row rebuilt and every picture read again.
    fn refresh(&self) {
        self.refresh_queued.set(false);
        let filter = self.filter.get();
        let entries: Vec<Entry> =
            self.history.entries().into_iter().filter(|e| filter.matches(e.kind)).collect();
        // A selection that no longer exists is dropped rather than kept as a ghost.
        self.selected.borrow_mut().retain(|id| entries.iter().any(|e| &e.id == id));

        if entries.is_empty() {
            self.empty.show(filter, self.history.retention().label());
            self.pages.set_visible_child_name("empty");
        } else {
            self.pages.set_visible_child_name("row");
        }

        let old = self.shown.replace(Vec::new());
        let head = old.iter().zip(&entries).take_while(|(a, b)| a == b).count();
        let tail = old[head..]
            .iter()
            .rev()
            .zip(entries[head..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let removed = old.len() - head - tail;
        let added: Vec<glib::BoxedAnyObject> = entries[head..entries.len() - tail]
            .iter()
            .cloned()
            .map(glib::BoxedAnyObject::new)
            .collect();
        if removed > 0 || !added.is_empty() {
            let at = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
            self.store.splice(at(head), at(removed), &added);
            debug!(at = head, removed, added = added.len(), "history row spliced");
        }
        *self.shown.borrow_mut() = entries;
        self.apply_selection();
    }

    /// The row's tiles: each built once by `setup`, pointed at an entry by `bind` and away
    /// from it by `unbind` -- a list view makes as many as the screen shows and recycles
    /// them as the row scrolls.
    fn tile_factory(self: &Rc<Self>) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        {
            let weak = Rc::downgrade(self);
            factory.connect_setup(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>();
                let (Some(strip), Some(item)) = (weak.upgrade(), item) else { return };
                item.set_activatable(false);
                item.set_selectable(false);
                item.set_focusable(false);
                let tile = strip.build_tile(item);
                item.set_child(Some(&tile.root));
                strip.tiles.borrow_mut().push(tile);
            });
        }
        {
            let weak = Rc::downgrade(self);
            factory.connect_bind(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>();
                let (Some(strip), Some(item)) = (weak.upgrade(), item) else { return };
                let (Some(tile), Some(object)) =
                    (strip.tile_of(item), item.item().and_downcast::<glib::BoxedAnyObject>())
                else {
                    return;
                };
                strip.bind_tile(&tile, &object.borrow::<Entry>());
            });
        }
        {
            let weak = Rc::downgrade(self);
            factory.connect_unbind(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>();
                let (Some(strip), Some(item)) = (weak.upgrade(), item) else { return };
                if let Some(tile) = strip.tile_of(item) {
                    tile.unbind();
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            factory.connect_teardown(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>();
                let (Some(strip), Some(item)) = (weak.upgrade(), item) else { return };
                let child = item.child();
                strip
                    .tiles
                    .borrow_mut()
                    .retain(|tile| Some(tile.root.upcast_ref()) != child.as_ref());
            });
        }
        factory
    }

    /// The tile a list item holds.
    fn tile_of(&self, item: &gtk::ListItem) -> Option<Rc<Tile>> {
        let child = item.child()?;
        let tiles = self.tiles.borrow();
        tiles.iter().find(|tile| tile.root.upcast_ref::<gtk::Widget>() == &child).cloned()
    }

    /// One tile, with nothing in it yet: the same widgets for every kind of capture, which
    /// is most of what makes them look the same (D109).
    fn build_tile(self: &Rc<Self>, item: &gtk::ListItem) -> Rc<Tile> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.add_css_class("octosnap-history-item");
        root.set_valign(gtk::Align::Start);

        // The picture fills the card. Its thumbnail is already the card's shape -- the
        // middle of a wide capture, the top of a tall one -- so `Cover` crops nothing.
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Cover);
        picture.set_can_shrink(true);
        picture.add_css_class("octosnap-history-thumb");
        picture.add_css_class("instant");
        let placeholder = gtk::Image::new();
        placeholder.set_pixel_size(34);
        placeholder.add_css_class("octosnap-history-placeholder");
        placeholder.set_halign(gtk::Align::Center);
        placeholder.set_valign(gtk::Align::Center);
        // The placeholder is the overlay's child and the picture lies over it, because an
        // overlay measures its child and not what lies over it: a picture's natural height
        // is its texture's, 260 px, and with the picture as the child the row asked for
        // that and the strip stood twice as tall as its tiles.
        let frame = gtk::Overlay::new();
        frame.set_child(Some(&placeholder));
        frame.add_overlay(&picture);
        frame.add_css_class("octosnap-history-tile");
        frame.set_overflow(gtk::Overflow::Hidden);
        frame.set_size_request(TILE_WIDTH, TILE_HEIGHT);
        frame.set_halign(gtk::Align::Center);

        // The kind, top left, and a recording's length beside it.
        let kind = gtk::Image::new();
        kind.set_pixel_size(14);
        let length = gtk::Label::new(None);
        length.add_css_class("numeric");
        length.set_visible(false);
        let badge = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        badge.add_css_class("octosnap-history-kind");
        badge.append(&kind);
        badge.append(&length);
        badge.set_halign(gtk::Align::Start);
        badge.set_valign(gtk::Align::Start);
        badge.set_can_target(false);
        frame.add_overlay(&badge);
        // The source app, bottom right, as the reference has it.
        let app = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        app.set_halign(gtk::Align::End);
        app.set_valign(gtk::Align::End);
        frame.add_overlay(&app);
        root.append(&frame);

        let footer = gtk::Stack::new();
        footer.set_hhomogeneous(false);
        footer.set_vhomogeneous(true);
        let time = gtk::Label::new(None);
        time.add_css_class("dim-label");
        time.add_css_class("octosnap-history-time");
        footer.add_named(&time, Some("time"));
        let restore = gtk::Button::with_label("\u{21a9} Restore");
        restore.set_can_focus(false);
        restore.add_css_class("suggested-action");
        restore.add_css_class("pill");
        restore.add_css_class("octosnap-history-restore");
        restore.set_halign(gtk::Align::Center);
        footer.add_named(&restore, Some("restore"));
        footer.set_visible_child_name("time");
        root.append(&footer);

        let tile = Rc::new(Tile {
            item: item.downgrade(),
            root: root.clone(),
            picture,
            placeholder,
            kind,
            length,
            app,
            frame,
            time,
            footer,
            id: RefCell::new(String::new()),
            created_at: Cell::new(0),
            generation: Cell::new(0),
            wanted: RefCell::new(None),
            showing: Cell::new(Showing::Nothing),
        });

        // Every handler reads the tile's id when it fires: the tile is recycled, and the
        // entry it showed when the handler was connected is long gone by then.
        let id_of = |tile: &Weak<Tile>| {
            tile.upgrade().map(|tile| tile.id.borrow().clone()).filter(|id| !id.is_empty())
        };
        {
            let (weak, tile) = (Rc::downgrade(self), Rc::downgrade(&tile));
            restore.connect_clicked(move |_| {
                if let (Some(strip), Some(id)) = (weak.upgrade(), id_of(&tile)) {
                    strip.restore(std::slice::from_ref(&id));
                }
            });
        }
        {
            // One click selects (Ctrl toggles), two open the editor (`HIS-01`).
            let click = gtk::GestureClick::new();
            click.set_button(gdk::BUTTON_PRIMARY);
            let (weak, tile) = (Rc::downgrade(self), Rc::downgrade(&tile));
            click.connect_pressed(move |gesture, presses, _, _| {
                let (Some(strip), Some(id)) = (weak.upgrade(), id_of(&tile)) else { return };
                if presses == 2 {
                    strip.annotate(&id);
                    return;
                }
                let toggle = gesture.current_event_state().contains(gdk::ModifierType::CONTROL_MASK);
                strip.select(&id, toggle);
            });
            root.add_controller(click);
        }
        {
            let menu = gtk::GestureClick::new();
            menu.set_button(gdk::BUTTON_SECONDARY);
            let (weak, tile) = (Rc::downgrade(self), Rc::downgrade(&tile));
            // Weak too: the gesture is the root's own, and a root held from inside it was
            // a cycle that kept every tile the row had built, about 10 MB a strip (D139).
            let anchor = root.downgrade();
            menu.connect_pressed(move |_, _, x, y| {
                let (Some(strip), Some(id), Some(anchor)) = (weak.upgrade(), id_of(&tile), anchor.upgrade())
                else {
                    return;
                };
                strip.select(&id, false);
                strip.open_menu(&anchor, &id, x, y);
            });
            root.add_controller(menu);
        }
        tile
    }

    /// Points a tile at an entry. Its picture is drawn at once if it is in memory, and
    /// otherwise read once the tile turns out to be near the screen (`load_near`).
    fn bind_tile(self: &Rc<Self>, tile: &Rc<Tile>, entry: &Entry) {
        tile.generation.set(tile.generation.get() + 1);
        *tile.id.borrow_mut() = entry.id.clone();
        tile.created_at.set(entry.created_at);
        tile.time.set_text(&entry.age_label(glib::real_time().unsigned_abs()));
        tile.kind.set_icon_name(Some(entry.kind.icon()));
        tile.placeholder.set_icon_name(Some(entry.kind.icon()));
        let length = entry.duration_ms.map(octosnap_media::text::duration_label);
        tile.length.set_visible(length.is_some());
        tile.length.set_text(length.as_deref().unwrap_or_default());
        while let Some(child) = tile.app.first_child() {
            tile.app.remove(&child);
        }
        if let Some(badge) = app_badge(entry) {
            tile.app.append(&badge);
        }
        tile.frame.set_tooltip_text(Some(&tooltip(entry)));
        tile.show_selected(self.selected.borrow().contains(&entry.id));

        let thumb =
            entry.thumb_path.clone().or_else(|| entry.dir().map(|dir| dir.join(THUMB_FILE)));
        let Some(thumb) = thumb else {
            tile.wanted.replace(None);
            tile.show(None, true, Showing::Missing);
            return;
        };
        let cached = PICTURES.with(|pictures| pictures.borrow_mut().get(&thumb));
        tile.wanted.replace(Some(Wanted {
            thumb,
            capture: entry.path.clone(),
            size: (entry.width, entry.height),
        }));
        match cached {
            Some(texture) => tile.show(Some(&texture), true, Showing::Picture),
            None => {
                tile.show(None, true, Showing::Nothing);
                self.queue_load();
            }
        }
    }

    /// A look at which tiles are near the screen, once, on the next turn of the main loop.
    fn queue_load(self: &Rc<Self>) {
        if self.load_queued.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(strip) = weak.upgrade() {
                strip.load_near();
            }
        });
    }

    /// Gives the tiles near the screen their pictures, nearest first, and takes them from
    /// the tiles far from it (D109).
    ///
    /// A `GtkListView` keeps up to two hundred rows built however few are on screen, so
    /// every tile of a strip that size is bound the moment it opens, and reading every
    /// bound tile's picture was a hundred and fifty files decoded before the strip had
    /// settled. Every row is the same width, so which rows are near is counted off the
    /// scroll position rather than measured: a screen's width either side is read ahead,
    /// and a picture more than two screens away is let go, to come back from the cache.
    fn load_near(&self) {
        self.load_queued.set(false);
        let adjustment = self.scroller.hadjustment();
        let page = adjustment.page_size();
        if page <= 0.0 {
            // Not allocated yet; the allocation's `changed` asks again.
            return;
        }
        let mut from = adjustment.value() - adjustment.lower();
        if self.list.direction() == gtk::TextDirection::Rtl {
            // A horizontal list runs from the right in a right-to-left locale, and its
            // adjustment the other way.
            from = adjustment.upper() - page - adjustment.value();
        }
        // In tiles from the start of the row: tile `n` spans `n..n + 1`.
        let pitch = f64::from(TILE_WIDTH + TILE_GAP);
        let near = (from - page) / pitch..(from + 2.0 * page) / pitch;
        let kept = (from - 2.0 * page) / pitch..(from + 3.0 * page) / pitch;
        let centre = (from + page / 2.0) / pitch;
        let meets =
            |range: &std::ops::Range<f64>, at: f64| at + 1.0 > range.start && at < range.end;
        let mut wanted = Vec::new();
        let mut parked = 0;
        for tile in self.tiles.borrow().iter() {
            let Some(at) = tile.position().map(f64::from) else { continue };
            match tile.showing.get() {
                Showing::Nothing if meets(&near, at) => {
                    wanted.push(((at + 0.5 - centre).abs(), Rc::clone(tile)));
                }
                Showing::Picture if !meets(&kept, at) => {
                    tile.show(None, true, Showing::Nothing);
                    parked += 1;
                }
                _ => {}
            }
        }
        if wanted.is_empty() && parked == 0 {
            return;
        }
        wanted.sort_by(|a, b| a.0.total_cmp(&b.0));
        debug!(asked = wanted.len(), parked, "history pictures");
        for (_, tile) in wanted {
            tile.load();
        }
    }

    /// The minute's tick: every time on screen, rewritten in place.
    fn tick(&self) {
        let now = glib::real_time().unsigned_abs();
        for tile in self.tiles.borrow().iter() {
            if !tile.id.borrow().is_empty() {
                tile.time.set_text(&relative_time(tile.created_at.get(), now));
            }
        }
    }

    /// A wheel scrolls the row the only way it goes -- sideways -- and slides a tile at a
    /// time rather than jumping (D109). A touchpad's vertical swipe is turned sideways
    /// too, pixel for pixel; its sideways swipe is the scroller's own, kinetics and all.
    fn wire_wheel(self: &Rc<Self>) {
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        scroll.connect_scroll(move |controller, _, dy| {
            let Some(strip) = weak.upgrade() else { return glib::Propagation::Proceed };
            if dy == 0.0 {
                return glib::Propagation::Proceed;
            }
            let wheel = controller.unit() == gdk::ScrollUnit::Wheel;
            let step = if wheel { f64::from(TILE_WIDTH + TILE_GAP) } else { 1.0 };
            strip.scroll_by(dy * step, wheel);
            glib::Propagation::Stop
        });
        self.scroller.add_controller(scroll);
    }

    /// Moves the row by `delta` pixels, sliding there when `slide`.
    fn scroll_by(&self, delta: f64, slide: bool) {
        let adjustment = self.scroller.hadjustment();
        let end = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
        // A click during a slide carries on from where the slide was going.
        let from = match self.wheel.borrow().as_ref() {
            Some((animation, to)) if slide && animation.state() == adw::AnimationState::Playing => {
                *to
            }
            _ => adjustment.value(),
        };
        let to = (from + delta).clamp(adjustment.lower(), end);
        if let Some((animation, _)) = self.wheel.borrow_mut().take() {
            animation.pause();
        }
        if !slide {
            adjustment.set_value(to);
            return;
        }
        let target = adw::PropertyAnimationTarget::new(&adjustment, "value");
        let animation =
            adw::TimedAnimation::new(&self.scroller, adjustment.value(), to, WHEEL_MS, target);
        animation.set_easing(adw::Easing::EaseOutCubic);
        animation.play();
        *self.wheel.borrow_mut() = Some((animation, to));
    }

    fn weak(&self) -> Weak<Self> {
        OPEN.with(|o| o.borrow().as_ref().map_or_else(Weak::new, Rc::downgrade))
    }

    // --- selection ------------------------------------------------------------------

    fn select(&self, id: &str, toggle: bool) {
        {
            let mut selected = self.selected.borrow_mut();
            if toggle {
                if let Some(at) = selected.iter().position(|s| s == id) {
                    selected.remove(at);
                } else {
                    selected.push(id.to_owned());
                }
            } else {
                selected.clear();
                selected.push(id.to_owned());
            }
        }
        self.apply_selection();
    }

    fn select_all(&self) {
        let all: Vec<String> = self.shown.borrow().iter().map(|e| e.id.clone()).collect();
        *self.selected.borrow_mut() = all;
        self.apply_selection();
    }

    /// The arrow keys: one tile along, or the first when nothing is selected -- scrolled
    /// into view, since the row only builds the tiles it shows.
    fn move_selection(&self, delta: isize) {
        let shown = self.shown.borrow();
        if shown.is_empty() {
            return;
        }
        let current = self
            .selected
            .borrow()
            .last()
            .and_then(|id| shown.iter().position(|e| &e.id == id));
        let next = match current {
            Some(at) => {
                let len = isize::try_from(shown.len()).unwrap_or(1);
                let moved = isize::try_from(at).unwrap_or(0) + delta;
                usize::try_from(moved.rem_euclid(len)).unwrap_or(0)
            }
            None if delta < 0 => shown.len() - 1,
            None => 0,
        };
        let id = shown[next].id.clone();
        drop(shown);
        self.select(&id, false);
        let at = u32::try_from(next).unwrap_or(0);
        self.list.scroll_to(at, gtk::ListScrollFlags::NONE, None::<gtk::ScrollInfo>);
    }

    /// Shows the selection on the tiles that are on screen; a tile bound later reads it
    /// in `bind_tile`.
    fn apply_selection(&self) {
        let selected = self.selected.borrow();
        for tile in self.tiles.borrow().iter() {
            let id = tile.id.borrow();
            tile.show_selected(!id.is_empty() && selected.contains(&*id));
        }
        debug!(selected = selected.len(), "history selection");
    }

    // --- actions --------------------------------------------------------------------

    /// `spec/07` §4.2's Restore: the entry leaves the history and becomes a card again.
    fn restore(self: &Rc<Self>, ids: &[String]) {
        let Some(overlay) = crate::overlay() else { return };
        let mut restored = 0;
        for id in ids {
            if let Some((capture, saved_to)) = self.history.take(id)
                && overlay.show(&capture, saved_to.as_deref())
            {
                restored += 1;
            }
        }
        info!(restored, "restored from the history strip");
        self.close();
    }

    /// `HIS-01`: "double-click → Annotate". The capture comes back to the spool for the
    /// editor, which holds its file for as long as it is open and hands it back when it
    /// closes: to the stack as a card, or -- Final Close -- to the history again (D108).
    fn annotate(self: &Rc<Self>, id: &str) {
        let Some(overlay) = crate::overlay() else { return };
        if let Some((capture, saved_to)) = self.history.take(id) {
            overlay.open_editor_with(&capture, saved_to.as_deref());
            info!(id, "opened the editor from the history strip");
        }
        self.close();
    }

    fn pin(self: &Rc<Self>, id: &str) {
        let Some(pins) = crate::pins() else { return };
        if let Some((capture, saved_to)) = self.history.take(id) {
            if pins.pin(&capture, saved_to.as_deref()) {
                info!(id, "pinned from the history strip");
            } else {
                // Taken out of the history for a pin that never appeared: it goes back, or
                // the capture would be in the spool and nowhere the user can see it.
                self.history.file(&capture, saved_to.as_deref(), None, false);
                crate::notify::action_failed(&self.app, "Pin", "The pin could not be shown.");
            }
        }
        self.close();
    }

    /// A recording whose GIF is still frames has to be written before it can be copied or
    /// saved, which can take a minute. Its card is where that shows (D113), so it goes back
    /// to the stack to do it. `true` when it went.
    fn through_a_card(self: &Rc<Self>, id: &str, then: crate::qao::CardThen) -> bool {
        let pending = self
            .history
            .capture_of(id)
            .is_some_and(|capture| crate::recording::render::pending(&capture.path));
        if !pending {
            return false;
        }
        let Some(overlay) = crate::overlay() else { return false };
        if let Some((capture, saved_to)) = self.history.take(id) {
            if overlay.show_and(&capture, saved_to.as_deref(), then) {
                info!(id, ?then, "a GIF still in frames went to a card to be written");
            } else {
                self.history.file(&capture, saved_to.as_deref(), None, false);
                let action = match then {
                    crate::qao::CardThen::Copy => "Copy",
                    crate::qao::CardThen::SaveAs => "Save",
                };
                crate::notify::action_failed(&self.app, action, "The GIF's card could not be shown.");
            }
        }
        self.close();
        true
    }

    fn copy(self: &Rc<Self>, id: &str) {
        if self.through_a_card(id, crate::qao::CardThen::Copy) {
            return;
        }
        let (Some(flow), Some(capture)) = (crate::capture_flow(), self.history.capture_of(id)) else {
            return;
        };
        let weak = Rc::downgrade(self);
        let app = self.app.clone();
        glib::spawn_future_local(async move {
            match flow.copy(&capture).await {
                Ok(()) => {
                    info!("copied from the history strip");
                    if let Some(strip) = weak.upgrade() {
                        strip.close();
                    }
                }
                // The strip stays, so the entry is still there to try again.
                Err(e) => {
                    warn!("could not copy from the history: {e}");
                    crate::notify::action_failed(&app, "Copy", &e.to_string());
                }
            }
        });
    }

    fn save_as(self: &Rc<Self>, id: &str) {
        if self.through_a_card(id, crate::qao::CardThen::SaveAs) {
            return;
        }
        let (Some(flow), Some(capture)) = (crate::capture_flow(), self.history.capture_of(id)) else {
            return;
        };
        let weak = Rc::downgrade(self);
        let window = self.window.clone();
        let app = self.app.clone();
        glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder()
                .title(crate::qao::save_title(&capture))
                .initial_name(flow.suggested_name(&capture))
                .modal(true)
                .build();
            match dialog.save_future(Some(&window)).await {
                Ok(file) => {
                    let Some(path) = file.path() else { return };
                    match flow.save_capture_as(&capture, &path).await {
                        Ok(path) => info!(path = %path.display(), "saved from the history strip"),
                        Err(e) => {
                            warn!("could not save from the history: {e}");
                            crate::notify::action_failed(&app, "Save", &e.to_string());
                            return;
                        }
                    }
                    if let Some(strip) = weak.upgrade() {
                        strip.close();
                    }
                }
                Err(e) => info!("the save chooser was dismissed: {e}"),
            }
        });
    }

    fn delete_selected(&self) {
        let ids: Vec<String> = self.selected.borrow().clone();
        for id in &ids {
            self.history.remove(id);
        }
        info!(deleted = ids.len(), "deleted from the history strip");
    }

    /// `HIS-01`'s Clear history, behind one question: the files are the user's captures
    /// and there is no undo for a directory that has been emptied.
    fn confirm_clear(self: &Rc<Self>) {
        let count = self.history.len();
        if count == 0 {
            return;
        }
        let dialog = adw::AlertDialog::builder()
            .heading("Clear the history?")
            .body(format!(
                "{count} capture{} will be removed from the history. Copies you saved to \
                 your pictures folder stay where they are.",
                if count == 1 { "" } else { "s" }
            ))
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("clear", "Clear");
        dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let history = Rc::clone(&self.history);
        dialog.connect_response(None, move |_, response| {
            if response == "clear" {
                history.clear();
            }
        });
        dialog.present(Some(&self.window));
    }

    fn open_menu(self: &Rc<Self>, anchor: &gtk::Box, id: &str, x: f64, y: f64) {
        // A GIF's menu is its card's: Trim where an image has Annotate, and no Pin, which
        // a looping GIF is not sensibly (D68).
        let gif = self.history.capture_of(id).is_some_and(|c| octosnap_core::history::is_gif(&c.path));
        let menu = gio::Menu::new();
        let first = gio::Menu::new();
        first.append(Some("Restore"), Some("tile.restore"));
        first.append(Some(if gif { "Trim" } else { "Annotate" }), Some("tile.annotate"));
        if !gif {
            first.append(Some("Pin to the Screen"), Some("tile.pin"));
        }
        menu.append_section(None, &first);
        let second = gio::Menu::new();
        second.append(Some("Copy"), Some("tile.copy"));
        second.append(Some("Save As\u{2026}"), Some("tile.save-as"));
        menu.append_section(None, &second);
        let last = gio::Menu::new();
        last.append(Some("Delete from History"), Some("tile.delete"));
        menu.append_section(None, &last);

        let group = gio::SimpleActionGroup::new();
        type Run = fn(&Rc<Strip>, &str);
        let entries: [(&str, Run); 6] = [
            ("restore", |s, id| s.restore(&[id.to_owned()])),
            ("annotate", |s, id| s.annotate(id)),
            ("pin", |s, id| s.pin(id)),
            ("copy", |s, id| s.copy(id)),
            ("save-as", |s, id| s.save_as(id)),
            ("delete", |s, _| s.delete_selected()),
        ];
        for (name, run) in entries {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            let id = id.to_owned();
            action.connect_activate(move |_, _| {
                if let Some(strip) = weak.upgrade() {
                    run(&strip, &id);
                }
            });
            group.add_action(&action);
        }
        anchor.insert_action_group("tile", Some(&group));

        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(anchor);
        popover.set_has_arrow(false);
        #[allow(clippy::cast_possible_truncation)]
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        // On the next turn: GTK closes the menu before it runs the item that was clicked,
        // and the item finds its action through this parent (D136).
        popover.connect_closed(|popover| {
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
        popover.popup();
    }

    // --- keys -----------------------------------------------------------------------

    /// The strip's keys, seen **before** the focused widget. GTK hands a new window's
    /// focus to its first focusable child, and a focused button consumes Return itself,
    /// so in the bubble phase the very key the strip exists for -- Return to restore --
    /// went to the "All" chip and toggled a chip that was already on. Capture phase, and
    /// nothing in the strip takes focus (`set_can_focus(false)` below): the keyboard is
    /// the strip's, not a control's.
    fn wire_keys(self: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(strip) = weak.upgrade() else { return glib::Propagation::Proceed };
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            match key {
                gdk::Key::Escape => strip.close(),
                gdk::Key::Left => strip.move_selection(-1),
                gdk::Key::Right => strip.move_selection(1),
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    let ids = strip.selected.borrow().clone();
                    if !ids.is_empty() {
                        strip.restore(&ids);
                    }
                }
                gdk::Key::Delete | gdk::Key::BackSpace => strip.delete_selected(),
                gdk::Key::a if ctrl => strip.select_all(),
                // The card's Copy key, for the one thumbnail selected: a clipboard holds
                // one picture, and which of several to copy would be a guess.
                gdk::Key::c if ctrl => {
                    let ids = strip.selected.borrow().clone();
                    if let [id] = ids.as_slice() {
                        strip.copy(id);
                    }
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.window.add_controller(keys);
    }
}

impl Tile {
    /// The tile's place in the row, while it is bound.
    fn position(&self) -> Option<u32> {
        let position = self.item.upgrade()?.position();
        (position != gtk::INVALID_LIST_POSITION && !self.id.borrow().is_empty()).then_some(position)
    }

    /// Shows the bound entry's picture: at once from the cache, or once it has been read.
    fn load(self: &Rc<Self>) {
        let Some(wanted) = self.wanted.borrow().clone() else { return };
        match PICTURES.with(|pictures| pictures.borrow_mut().get(&wanted.thumb)) {
            Some(texture) => self.show(Some(&texture), true, Showing::Picture),
            None => {
                self.showing.set(Showing::Waiting);
                request_picture(self, wanted);
            }
        }
    }

    /// The picture, or the kind's placeholder when there is none. `instant` for a picture
    /// already in memory -- a tile scrolled back into view -- which appears at once; one
    /// that had to be read fades in.
    fn show(&self, texture: Option<&gdk::Texture>, instant: bool, showing: Showing) {
        self.showing.set(showing);
        self.picture.set_paintable(texture);
        self.placeholder.set_visible(texture.is_none());
        if instant {
            self.picture.add_css_class("instant");
        } else {
            self.picture.remove_css_class("instant");
        }
        if texture.is_some() {
            self.picture.add_css_class("loaded");
        } else {
            self.picture.remove_css_class("loaded");
        }
    }

    fn show_selected(&self, on: bool) {
        if on {
            self.root.add_css_class("selected");
        } else {
            self.root.remove_css_class("selected");
        }
        self.footer.set_visible_child_name(if on { "restore" } else { "time" });
    }

    /// Lets go of the entry: a picture still on its way for it is dropped when it lands.
    fn unbind(&self) {
        self.generation.set(self.generation.get() + 1);
        self.id.borrow_mut().clear();
        self.wanted.replace(None);
        self.show(None, true, Showing::Nothing);
    }
}

/// Reads a tile's picture on a worker and gives it to every tile still showing that
/// entry when it arrives -- once, however many tiles asked while it was on its way.
fn request_picture(tile: &Rc<Tile>, wanted: Wanted) {
    let Wanted { thumb, capture, size } = wanted;
    let generation = tile.generation.get();
    let first = WAITING.with(|waiting| {
        let mut waiting = waiting.borrow_mut();
        let tiles = waiting.entry(thumb.clone()).or_default();
        tiles.push((Rc::downgrade(tile), generation));
        tiles.len() == 1
    });
    if !first {
        return;
    }
    glib::spawn_future_local(async move {
        let texture = super::thumbnail::tile_texture(thumb.clone(), capture, size).await;
        if let Some(texture) = &texture {
            PICTURES.with(|pictures| pictures.borrow_mut().put(thumb.clone(), texture.clone()));
        }
        let tiles = WAITING.with(|waiting| waiting.borrow_mut().remove(&thumb)).unwrap_or_default();
        for (tile, generation) in tiles {
            if let Some(tile) = tile.upgrade()
                && tile.generation.get() == generation
            {
                let showing = if texture.is_some() { Showing::Picture } else { Showing::Missing };
                tile.show(texture.as_ref(), false, showing);
            }
        }
    });
}

/// What a tile cannot say: `HIS-03`'s window title, the kind in words, and the size.
fn tooltip(entry: &Entry) -> String {
    let mut tip = format!(
        "{} \u{b7} {}\u{a0}\u{d7}\u{a0}{} px",
        entry.kind.label(),
        entry.width,
        entry.height
    );
    if !entry.source.title.is_empty() {
        tip = format!("{}\n{tip}", entry.source.title);
    }
    if let Some(saved) = &entry.saved_path {
        tip = format!("{tip}\nSaved as {}", saved.display());
    }
    tip
}

thread_local! {
    /// Every installed application's icon by app id, read once: `AppInfo::all` walks
    /// every desktop file, and walking them again for each app a strip meets was a stall
    /// the first time each one scrolled into view.
    static APP_ICONS: RefCell<Option<HashMap<String, gio::Icon>>> = const { RefCell::new(None) };
}

/// The icon of the application whose id the extension reported, by its desktop file.
fn app_icon(app_id: &str) -> Option<gio::Icon> {
    if app_id.is_empty() {
        return None;
    }
    APP_ICONS.with(|icons| {
        icons
            .borrow_mut()
            .get_or_insert_with(|| {
                gio::AppInfo::all()
                    .into_iter()
                    .filter_map(|info| {
                        let id = info.id()?;
                        let id = id.strip_suffix(".desktop").unwrap_or(&id).to_owned();
                        Some((id, info.icon()?))
                    })
                    .collect()
            })
            .get(app_id)
            .cloned()
    })
}

/// `spec/07` §4.2's "source app badges … showing which application the capture came
/// from": the app's own icon, or its name where the desktop has no icon for it.
fn app_badge(entry: &Entry) -> Option<gtk::Widget> {
    let icon = app_icon(&entry.source.app_id);
    let badge = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    badge.add_css_class("octosnap-history-badge");
    match (icon, entry.badge()) {
        (Some(icon), _) => {
            let image = gtk::Image::from_gicon(&icon);
            image.set_pixel_size(20);
            badge.append(&image);
        }
        (None, Some(name)) => {
            let label = gtk::Label::new(Some(name));
            label.add_css_class("caption");
            badge.append(&label);
        }
        (None, None) => return None,
    }
    if let Some(name) = entry.badge() {
        badge.set_tooltip_text(Some(name));
    }
    Some(badge.upcast())
}

/// The strip's stylesheet, once per process.
fn install_style() {
    if STYLED.with(|s| s.replace(true)) {
        return;
    }
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&format!(
        "
window.octosnap-history,
window.octosnap-history > * {{
    background: none;
    background-color: transparent;
    box-shadow: none;
}}

window.octosnap-history .octosnap-history-body {{
    border-radius: {RADIUS}px;
    background-color: @window_bg_color;
    color: @window_fg_color;
    padding: 10px 14px 12px 14px;
    box-shadow: 0 4px {blur}px rgba(0, 0, 0, 0.35);
    outline: 1px solid alpha(@borders, 0.8);
    outline-offset: -1px;
}}

.octosnap-history-chip {{
    border-radius: 999px;
    padding: 1px 12px;
    min-height: 26px;
}}

.octosnap-history-chip:checked {{
    background-color: @accent_bg_color;
    color: @accent_fg_color;
}}

/* The row is a list view, whose rows GTK would otherwise light up on hover and select;
   the tiles do that themselves, the same way for every kind (D109). */
listview.octosnap-history-row,
listview.octosnap-history-row > row,
listview.octosnap-history-row > row:hover,
listview.octosnap-history-row > row:selected,
listview.octosnap-history-row > row:focus-visible {{
    background: none;
    box-shadow: none;
    outline: none;
}}

listview.octosnap-history-row > row {{
    padding: 6px {half_gap}px 2px {half_gap}px;
    border-radius: 0;
}}

.octosnap-history-tile {{
    border-radius: 12px;
    background-color: alpha(@view_fg_color, 0.07);
    outline: 1px solid alpha(@borders, 0.6);
    outline-offset: -1px;
    transition: filter 120ms ease-out;
}}

.octosnap-history-item:hover .octosnap-history-tile {{
    filter: brightness(1.08);
}}

.octosnap-history-item.selected .octosnap-history-tile {{
    outline: 3px solid @accent_bg_color;
    outline-offset: 2px;
}}

.octosnap-history-thumb {{
    opacity: 0;
    transition: opacity 180ms ease-out;
}}

.octosnap-history-thumb.loaded {{
    opacity: 1;
}}

.octosnap-history-thumb.instant {{
    transition: none;
}}

.octosnap-history-placeholder {{
    opacity: 0.3;
}}

.octosnap-history-kind {{
    background-color: rgba(0, 0, 0, 0.62);
    color: white;
    border-radius: 7px;
    padding: 3px 6px;
    margin: 7px;
}}

.octosnap-history-kind label {{
    font-size: 0.8em;
    font-weight: bold;
}}

.octosnap-history-badge {{
    background-color: rgba(255, 255, 255, 0.94);
    border-radius: 7px;
    padding: 3px;
    margin: 7px;
    color: #1e1e1e;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.3);
}}

.octosnap-history-time {{
    font-size: 0.9em;
}}

.octosnap-history-restore {{
    min-height: 24px;
    padding: 0 12px;
}}
",
        blur = SHADOW_MARGIN - 4,
        half_gap = TILE_GAP / 2,
    ));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
