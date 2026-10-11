// SPDX-License-Identifier: GPL-3.0-or-later

//! The desktop pets as the app shows them: on Settings' Pets page and in the welcome
//! window.
//!
//! The pets themselves live in the extension (D142). The app draws them from the sheets in
//! `data/pets/`, which the extension's own tricks wrote (`sheets.test.ts`), so a pet in
//! Settings breathes, blinks and jumps the way it does on the desktop. A sheet is a strip
//! of frames side by side and a loop of `[frame, milliseconds, lift]` steps, where the lift
//! is how many art pixels above the floor the frame stands, for a jump.
//!
//! They are crisp for the reason the shell's pets are: an art pixel is `round(base ×
//! scale)` physical pixels, a whole number at every scale, and a frame starts on the
//! physical grid. A frame is drawn as rectangles of one colour each, not as a texture
//! scaled with the nearest texel. GTK draws a texture scaled that way into an offscreen at
//! the logical size and then scales the offscreen smoothly to the screen's, so at 125 %
//! every art pixel came out with blended edges and an uneven width. Rectangles that start
//! and end on the grid leave nothing to resample.
//!
//! A pet moved without being drawn again keeps the place on the grid it was drawn for,
//! which at a fractional scale is off the grid where it now is. Scrolling does that, so a
//! pet is drawn again whenever its page scrolls, and anyway at every step of its loop.
//!
//! A pet pack's pet (`spec/14` §14, D196) comes from the extension, which reads the packs
//! on the host where the sandbox cannot: `GetPetPacks` hands over each one's preview,
//! already played by the shell's runner and painted, as a palette and frames of palette
//! indices. `set_packs` makes those into sheets like the five's, and every pet here is
//! drawn the same way after that.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, gio, glib, graphene};
use octosnap_core::protocol::{SHELL_BUS_NAME, SHELL_INTERFACE, SHELL_OBJECT_PATH};
use serde::Deserialize;
use tracing::{info, warn};

/// The pets in the extension's order (`PET_KINDS`), which is the order the `pets` key keeps.
pub const KINDS: [&str; 5] = ["octopus", "penguin", "frog", "mushroom", "potato"];

/// Where `octosnap.gresource.xml` puts the sheets.
const RESOURCES: &str = "/io/github/odrakirmusic/OctoSnap/pets";

/// One pet's sheet, as `pets.json` has it.
#[derive(Debug, Clone, Deserialize)]
pub struct Sheet {
    /// What the pet is called: "Omni" for the octopus.
    pub name: String,
    /// A frame's size, in art pixels.
    pub width: u32,
    pub height: u32,
    /// The art row the pet stands on, counted from the top.
    pub baseline: u32,
    /// How many frames the strip holds.
    pub frames: u32,
    /// `[frame, milliseconds, lift]`, played in order and then again.
    #[serde(rename = "loop")]
    pub steps: Vec<(u32, u32, u32)>,
}

/// Reads `pets.json`.
pub fn parse(json: &[u8]) -> Result<HashMap<String, Sheet>, serde_json::Error> {
    serde_json::from_slice(json)
}

/// A frame as rectangles of one colour, in art pixels: `(x, y, width, height, rgba)`.
pub type Rects = Vec<(u32, u32, u32, u32, [u8; 4])>;

/// Each frame of a strip, as rectangles. `rgba` is the strip, straight 8-bit RGBA, with the
/// sheet's frames side by side.
///
/// A row's runs of one colour, each carried down for as long as the rows under it repeat
/// it exactly. Clear pixels are left out.
pub fn frames_as_rects(rgba: &[u8], sheet: &Sheet) -> Vec<Rects> {
    let (width, height) = (sheet.width as usize, sheet.height as usize);
    let stride = width * sheet.frames as usize;
    let pixel = |x: usize, y: usize| -> [u8; 4] {
        let at = (y * stride + x) * 4;
        rgba.get(at..at + 4).and_then(|p| p.try_into().ok()).unwrap_or([0; 4])
    };
    (0..sheet.frames as usize)
        .map(|frame| {
            let mut done: Rects = Vec::new();
            let mut open: Rects = Vec::new();
            for y in 0..height {
                let mut next: Rects = Vec::new();
                let mut x = 0;
                while x < width {
                    let colour = pixel(frame * width + x, y);
                    let mut end = x + 1;
                    while end < width && pixel(frame * width + end, y) == colour {
                        end += 1;
                    }
                    if colour[3] != 0 {
                        #[allow(clippy::cast_possible_truncation)]
                        let (x, run, y) = (x as u32, (end - x) as u32, y as u32);
                        match open.iter().position(|r| r.0 == x && r.2 == run && r.4 == colour) {
                            Some(i) => {
                                let mut rect = open.swap_remove(i);
                                rect.3 += 1;
                                next.push(rect);
                            }
                            None => next.push((x, y, run, 1, colour)),
                        }
                    }
                    x = end;
                }
                done.append(&mut open);
                open = next;
            }
            done.append(&mut open);
            done
        })
        .collect()
}

/// A PNG as straight 8-bit RGBA, when it is one: the sheets are written that way.
fn decode(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buffer = vec![0u8; reader.output_buffer_size().ok_or("too large")?];
    let frame = reader.next_frame(&mut buffer).map_err(|e| e.to_string())?;
    if frame.color_type != png::ColorType::Rgba {
        return Err(format!("{:?}, not RGBA", frame.color_type));
    }
    buffer.truncate(frame.buffer_size());
    Ok(buffer)
}

/// The box every pet is drawn in so that they stand on one floor, in art pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Layout {
    /// The widest pet.
    width: u32,
    /// The art row the floor is on: room for the highest jump above the tallest pet.
    floor: u32,
    /// The whole box's height.
    height: u32,
}

impl Layout {
    fn of<'a>(sheets: impl Iterator<Item = &'a Sheet> + Clone) -> Self {
        let width = sheets.clone().map(|s| s.width).max().unwrap_or(0);
        let lift =
            sheets.clone().flat_map(|s| s.steps.iter().map(|step| step.2)).max().unwrap_or(0);
        let baseline = sheets.clone().map(|s| s.baseline).max().unwrap_or(0);
        let floor = lift + baseline;
        // Below the floor: whatever a pet draws under the row it stands on.
        let height = floor + sheets.map(|s| s.height.saturating_sub(s.baseline)).max().unwrap_or(0);
        Self { width, floor, height }
    }
}

/// The five's sheets, from the bundle.
#[derive(Debug)]
struct Sheets {
    sheets: HashMap<String, Rc<Sheet>>,
    frames: RefCell<HashMap<String, Option<Rc<Vec<Rects>>>>>,
}

impl Sheets {
    fn new(sheets: HashMap<String, Sheet>) -> Self {
        let sheets = sheets.into_iter().map(|(kind, sheet)| (kind, Rc::new(sheet))).collect();
        Self { sheets, frames: RefCell::default() }
    }

    /// A pet's frames, read from its strip the first time they are wanted.
    fn frames(&self, kind: &str) -> Option<Rc<Vec<Rects>>> {
        let sheet = self.sheets.get(kind)?;
        self.frames
            .borrow_mut()
            .entry(kind.to_owned())
            .or_insert_with(|| {
                let bytes = gio::resources_lookup_data(
                    &format!("{RESOURCES}/{kind}.png"),
                    gio::ResourceLookupFlags::NONE,
                )
                .map_err(|e| warn!(kind, "a pet's strip is missing: {e}"))
                .ok()?;
                let rgba = decode(&bytes)
                    .map_err(|e| warn!(kind, "a pet's strip does not read: {e}"))
                    .ok()?;
                Some(Rc::new(frames_as_rects(&rgba, sheet)))
            })
            .clone()
    }
}

/// A pack's pet, ready to draw.
#[derive(Debug)]
struct PackEntry {
    sheet: Rc<Sheet>,
    frames: Rc<Vec<Rects>>,
    species: String,
}

thread_local! {
    static SHEETS: OnceCell<Option<Rc<Sheets>>> = const { OnceCell::new() };
    /// The packs' pets, by id, from the last `set_packs`.
    static PACKS: RefCell<HashMap<String, PackEntry>> = RefCell::new(HashMap::new());
    /// The box for every pet there is now, worked out again when the packs change.
    static LAYOUT: Cell<Option<Layout>> = const { Cell::new(None) };
    /// The packs' pets' ids, in the order the extension gave them: by id.
    static PACK_ORDER: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// The sheets, read once. `None`, said once in the log, when the bundle has none.
fn sheets() -> Option<Rc<Sheets>> {
    SHEETS.with(|cell| {
        cell.get_or_init(|| {
            let bytes = gio::resources_lookup_data(
                &format!("{RESOURCES}/pets.json"),
                gio::ResourceLookupFlags::NONE,
            )
            .map_err(|e| warn!("the pets' sheets are missing: {e}"))
            .ok()?;
            let sheets =
                parse(&bytes).map_err(|e| warn!("the pets' sheets do not read: {e}")).ok()?;
            Some(Rc::new(Sheets::new(sheets)))
        })
        .clone()
    })
}

/// The box every pet there is stands in: the five's and the packs' together.
fn layout() -> Option<Layout> {
    if let Some(layout) = LAYOUT.with(Cell::get) {
        return Some(layout);
    }
    let sheets = sheets()?;
    let layout = PACKS.with(|packs| {
        let packs = packs.borrow();
        Layout::of(
            sheets.sheets.values().map(Rc::as_ref).chain(packs.values().map(|p| p.sheet.as_ref())),
        )
    });
    LAYOUT.with(|cell| cell.set(Some(layout)));
    Some(layout)
}

/// A pet's sheet and its frames: one of the five's, or a pack's.
fn entry(kind: &str) -> Option<(Rc<Sheet>, Rc<Vec<Rects>>)> {
    if let Some(found) = PACKS
        .with(|packs| packs.borrow().get(kind).map(|p| (Rc::clone(&p.sheet), Rc::clone(&p.frames))))
    {
        return Some(found);
    }
    let sheets = sheets()?;
    Some((Rc::clone(sheets.sheets.get(kind)?), sheets.frames(kind)?))
}

/// A pet's sheet without its frames: what the loop's timer needs.
fn sheet(kind: &str) -> Option<Rc<Sheet>> {
    PACKS
        .with(|packs| packs.borrow().get(kind).map(|p| Rc::clone(&p.sheet)))
        .or_else(|| sheets().and_then(|sheets| sheets.sheets.get(kind).cloned()))
}

/// What a pet is called, or its kind when the sheets do not say.
pub fn name(kind: &str) -> String {
    sheet(kind).map_or_else(|| kind.to_owned(), |sheet| sheet.name.clone())
}

/// What a pet is, for its card and its accessible name: its kind for the five, and what its
/// pack says for a pack's.
pub fn species(kind: &str) -> String {
    PACKS
        .with(|packs| packs.borrow().get(kind).map(|p| p.species.clone()))
        .unwrap_or_else(|| kind.to_owned())
}

/// A pack's pet as the extension hands it over (`GetPetPacks`, `spec/14` §14): its preview,
/// already played by the shell's runner and painted.
#[derive(Debug, Clone, Deserialize)]
pub struct PackPet {
    /// Its pack's id, which the `pets` key calls it by.
    pub id: String,
    pub name: String,
    pub species: String,
    /// Its pack's folder.
    #[serde(default)]
    pub from: String,
    pub width: u32,
    pub height: u32,
    pub baseline: u32,
    /// `#rrggbb` colours, which the frames number from 1.
    pub palette: Vec<String>,
    /// Two hex digits a pixel, rows top to bottom: `00` clear, `n` the `n`th colour.
    pub frames: Vec<String>,
    #[serde(rename = "loop")]
    pub steps: Vec<(u32, u32, u32)>,
}

/// A pack the extension could not read, and why.
#[derive(Debug, Clone, Deserialize)]
pub struct PackProblem {
    pub id: String,
    #[serde(default)]
    pub from: String,
    pub problem: String,
}

/// What `GetPetPacks` says: the user's packs' folder, the pets, and the packs left out.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Packs {
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub pets: Vec<PackPet>,
    #[serde(default)]
    pub problems: Vec<PackProblem>,
}

/// Reads `GetPetPacks`' JSON.
pub fn parse_packs(json: &str) -> Result<Packs, serde_json::Error> {
    serde_json::from_str(json)
}

/// The most a pack's pet may be each way, in art pixels: the format's own limit.
const PACK_SIDE: usize = 64;

/// A pack's pet as a sheet and its strip, straight RGBA with its frames side by side, as
/// `frames_as_rects` reads the five's. Refused, with why, when anything in it does not add
/// up: the extension made it, but a pet nothing checked is not drawn.
pub fn pack_strip(pet: &PackPet) -> Result<(Sheet, Vec<u8>), String> {
    let (width, height) = (pet.width as usize, pet.height as usize);
    if !(1..=PACK_SIDE).contains(&width) || !(1..=PACK_SIDE).contains(&height) {
        return Err(format!("it is {width} × {height}"));
    }
    if pet.frames.is_empty() || pet.frames.len() > 256 {
        return Err(format!("it has {} frames", pet.frames.len()));
    }
    if pet.steps.is_empty() {
        return Err("it has no loop".to_owned());
    }
    if pet.palette.len() > 255 {
        return Err(format!("it has {} colours", pet.palette.len()));
    }
    let mut palette = Vec::with_capacity(pet.palette.len());
    for colour in &pet.palette {
        let rgb = colour
            .strip_prefix('#')
            .filter(|hex| hex.len() == 6)
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .ok_or_else(|| format!("{colour} is not a colour"))?;
        let [_, r, g, b] = rgb.to_be_bytes();
        palette.push([r, g, b, 255]);
    }
    let stride = width * pet.frames.len();
    let mut rgba = vec![0u8; stride * height * 4];
    for (f, frame) in pet.frames.iter().enumerate() {
        let digits = frame.as_bytes();
        if digits.len() != width * height * 2 {
            return Err(format!(
                "frame {f} is {} digits, not {}",
                digits.len(),
                width * height * 2
            ));
        }
        for (p, pair) in digits.as_chunks::<2>().0.iter().enumerate() {
            let n = std::str::from_utf8(pair)
                .ok()
                .and_then(|pair| usize::from_str_radix(pair, 16).ok())
                .ok_or_else(|| format!("frame {f} has something that is not a colour's number"))?;
            if n == 0 {
                continue;
            }
            let colour = palette
                .get(n - 1)
                .ok_or_else(|| format!("frame {f} uses colour {n} of {}", palette.len()))?;
            let (x, y) = (p % width, p / width);
            let at = (y * stride + f * width + x) * 4;
            rgba[at..at + 4].copy_from_slice(colour);
        }
    }
    let frames = u32::try_from(pet.frames.len()).map_err(|e| e.to_string())?;
    if let Some(&(frame, _, lift)) =
        pet.steps.iter().find(|&&(frame, _, lift)| frame >= frames || lift > 64)
    {
        return Err(format!("its loop has frame {frame} lifted {lift}"));
    }
    let sheet = Sheet {
        name: pet.name.clone(),
        width: pet.width,
        height: pet.height,
        baseline: pet.baseline.min(pet.height),
        frames,
        steps: pet.steps.clone(),
    };
    Ok((sheet, rgba))
}

/// The packs' pets from now on, in place of the ones before; the ids of those that can be
/// drawn, in the order given. A pet whose id is taken -- one of the five's, or an earlier
/// pack's -- is left out.
pub fn set_packs(pets: &[PackPet]) -> Vec<String> {
    let mut entries = HashMap::new();
    let mut ids = Vec::new();
    for pet in pets {
        if KINDS.contains(&pet.id.as_str()) || entries.contains_key(&pet.id) {
            warn!(id = pet.id, "a pack's pet with a name already taken, left out");
            continue;
        }
        match pack_strip(pet) {
            Ok((sheet, rgba)) => {
                let frames = Rc::new(frames_as_rects(&rgba, &sheet));
                entries.insert(
                    pet.id.clone(),
                    PackEntry { sheet: Rc::new(sheet), frames, species: pet.species.clone() },
                );
                ids.push(pet.id.clone());
            }
            Err(e) => warn!(id = pet.id, "a pack's pet the app cannot draw: {e}"),
        }
    }
    PACKS.with(|packs| *packs.borrow_mut() = entries);
    PACK_ORDER.with(|order| order.borrow_mut().clone_from(&ids));
    LAYOUT.with(|cell| cell.set(None));
    ids
}

/// Every pet there is, in the order the `pets` key keeps: the five, then the packs' pets.
pub fn kinds() -> Vec<String> {
    KINDS
        .iter()
        .map(|&kind| kind.to_owned())
        .chain(PACK_ORDER.with(|order| order.borrow().clone()))
        .collect()
}

/// The pet packs, from the extension (`GetPetPacks`). `None` when it has no such method, as
/// one from before D196 has not, or does not answer: then Settings shows the five.
pub async fn fetch_packs() -> Option<Packs> {
    let connection = gio::Application::default()?.dbus_connection()?;
    let reply = connection
        .call_future(
            Some(SHELL_BUS_NAME),
            SHELL_OBJECT_PATH,
            SHELL_INTERFACE,
            "GetPetPacks",
            None,
            glib::VariantTy::new("(s)").ok(),
            gio::DBusCallFlags::NONE,
            // The first call plays each pack's preview in the shell.
            5000,
        )
        .await;
    let reply = match reply {
        Ok(reply) => reply,
        Err(e) if e.matches(gio::DBusError::UnknownMethod) => {
            info!("the extension reads no pet packs: it is older than the app");
            return None;
        }
        Err(e) => {
            warn!("the extension did not hand over the pet packs: {e}");
            return None;
        }
    };
    let (json,) = reply.get::<(String,)>()?;
    let packs = parse_packs(&json).map_err(|e| warn!("the pet packs do not read: {e}")).ok()?;
    info!(
        pets = packs.pets.len(),
        problems = packs.problems.len(),
        "the extension handed over the pet packs"
    );
    Some(packs)
}

/// Settings' Open Pets Folder: the extension makes the user's packs' folder if it is
/// missing, and opens it in the file manager (`OpenPetFolder`).
pub fn open_pet_folder() {
    let Some(connection) = gio::Application::default().and_then(|app| app.dbus_connection()) else {
        return;
    };
    glib::spawn_future_local(async move {
        if let Err(e) = connection
            .call_future(
                Some(SHELL_BUS_NAME),
                SHELL_OBJECT_PATH,
                SHELL_INTERFACE,
                "OpenPetFolder",
                None,
                None,
                gio::DBusCallFlags::NONE,
                5000,
            )
            .await
        {
            warn!("the pets folder could not be opened: {e}");
        }
    });
}

/// Calls `changed` whenever the extension has read the packs again (`PetPacksChanged`), for
/// as long as the subscription returned is kept.
pub fn watch_packs(changed: impl Fn() + 'static) -> Option<gio::SignalSubscription> {
    let connection = gio::Application::default()?.dbus_connection()?;
    Some(connection.subscribe_to_signal(
        Some(SHELL_BUS_NAME),
        Some(SHELL_INTERFACE),
        Some("PetPacksChanged"),
        Some(SHELL_OBJECT_PATH),
        None,
        gio::DBusSignalFlags::NONE,
        move |_| changed(),
    ))
}

/// The cards' stylesheet, once per process.
pub fn install_style() {
    thread_local! {
        static STYLED: Cell<bool> = const { Cell::new(false) };
    }
    if STYLED.with(|styled| styled.replace(true)) {
        return;
    }
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        "
button.octosnap-pet-card {
    padding: 8px 6px 10px 6px;
}

button.octosnap-pet-card:checked {
    background-color: alpha(@accent_bg_color, 0.14);
    box-shadow: inset 0 0 0 2px @accent_bg_color;
}

.octosnap-pet-check {
    border-radius: 999px;
    padding: 3px;
    background-color: @accent_bg_color;
    color: @accent_fg_color;
}
",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct PetPreview {
        pub kind: RefCell<String>,
        /// Physical pixels to the art pixel at 100 %.
        pub base: Cell<u32>,
        /// Where in its loop the pet is.
        pub step: Cell<usize>,
        /// The scale the widget last measured itself at.
        pub measured: Cell<f64>,
        pub timer: RefCell<Option<glib::SourceId>>,
        /// What the pet follows while it is on screen: the animations setting, its page's
        /// scrolling, and the scale of the surface it is drawn on.
        pub watches: RefCell<Vec<(glib::Object, glib::SignalHandlerId)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PetPreview {
        const NAME: &'static str = "OctosnapPetPreview";
        type Type = super::PetPreview;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for PetPreview {
        fn dispose(&self) {
            self.stop();
            self.unwatch();
        }
    }

    impl WidgetImpl for PetPreview {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let Some(layout) = layout() else { return (0, 0, -1, -1) };
            let art = match orientation {
                gtk::Orientation::Horizontal => layout.width,
                _ => layout.height,
            };
            let scale = self.scale();
            self.measured.set(scale);
            // Whole logical pixels, rounded up: the box always holds the pet.
            #[allow(clippy::cast_possible_truncation)]
            let size = (f64::from(art) * self.unit(scale)).ceil() as i32;
            (size, size, -1, -1)
        }

        fn map(&self) {
            self.parent_map();
            self.watch();
            self.play();
        }

        fn unmap(&self) {
            self.stop();
            self.unwatch();
            self.parent_unmap();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(layout) = layout() else { return };
            let Some((sheet, frames)) = entry(&self.kind.borrow()) else { return };
            let (frame, _, lift) = sheet.steps.get(self.step.get()).copied().unwrap_or((0, 0, 0));
            let Some(rects) = frames.get(frame as usize) else { return };

            let widget = self.obj();
            let scale = self.scale();
            let unit = self.unit(scale);
            // The box at the foot of the widget, the pet centred across it and standing on
            // its floor, then the whole moved onto the physical grid.
            let (width, height) = (f64::from(widget.width()), f64::from(widget.height()));
            let left = (width - f64::from(sheet.width) * unit) / 2.0;
            let top = height - f64::from(layout.height) * unit
                + f64::from(layout.floor.saturating_sub(sheet.baseline + lift)) * unit;
            let (dx, dy) = self.on_grid(left, top, scale);
            let (left, top) = (left + dx, top + dy);

            for &(x, y, w, h, [r, g, b, a]) in rects {
                let at = |n: u32| f64::from(n) * unit;
                #[allow(clippy::cast_possible_truncation)]
                let bounds = graphene::Rect::new(
                    (left + at(x)) as f32,
                    (top + at(y)) as f32,
                    at(w) as f32,
                    at(h) as f32,
                );
                let colour = gdk::RGBA::new(
                    f32::from(r) / 255.0,
                    f32::from(g) / 255.0,
                    f32::from(b) / 255.0,
                    f32::from(a) / 255.0,
                );
                snapshot.append_color(&colour, &bounds);
            }
        }
    }

    impl PetPreview {
        /// The scale of the surface the widget is drawn on, fractional where the session's
        /// is.
        fn scale(&self) -> f64 {
            let widget = self.obj();
            widget
                .native()
                .and_then(|native| native.surface())
                .map_or_else(|| f64::from(widget.scale_factor()), |surface| surface.scale())
        }

        /// One art pixel, in logical pixels: a whole number of physical ones.
        fn unit(&self, scale: f64) -> f64 {
            (f64::from(self.base.get().max(1)) * scale).round().max(1.0) / scale
        }

        /// How far to move a point of the widget so that it lands on the physical grid.
        fn on_grid(&self, x: f64, y: f64, scale: f64) -> (f64, f64) {
            let widget = self.obj();
            let Some(native) = widget.native() else { return (0.0, 0.0) };
            let Some(origin) = widget.compute_point(&native, &graphene::Point::zero()) else {
                return (0.0, 0.0);
            };
            let (sx, sy) = native.surface_transform();
            let snap = |logical: f64| {
                let physical = logical * scale;
                (physical.round() - physical) / scale
            };
            (snap(f64::from(origin.x()) + sx + x), snap(f64::from(origin.y()) + sy + y))
        }

        /// Starts the loop from wherever the pet is in it, or holds the resting frame when
        /// the desktop asks for no animations.
        pub(super) fn play(&self) {
            self.stop();
            let widget = self.obj();
            if !widget.is_mapped() {
                return;
            }
            if !widget.settings().is_gtk_enable_animations() {
                self.step.set(0);
                widget.queue_draw();
                return;
            }
            self.schedule();
        }

        fn schedule(&self) {
            let ms = sheet(&self.kind.borrow())
                .and_then(|sheet| sheet.steps.get(self.step.get()).copied())
                .map_or(650, |step| step.1);
            let weak = self.obj().downgrade();
            let timer = glib::timeout_add_local_once(
                Duration::from_millis(u64::from(ms.max(16))),
                move || {
                    let Some(widget) = weak.upgrade() else { return };
                    let imp = widget.imp();
                    // Fired, so there is nothing left to remove.
                    imp.timer.take();
                    imp.advance();
                },
            );
            if let Some(old) = self.timer.replace(Some(timer)) {
                old.remove();
            }
        }

        fn advance(&self) {
            let steps = sheet(&self.kind.borrow()).map_or(0, |sheet| sheet.steps.len());
            if steps == 0 {
                return;
            }
            self.step.set((self.step.get() + 1) % steps);
            self.obj().queue_draw();
            self.schedule();
        }

        fn stop(&self) {
            if let Some(timer) = self.timer.take() {
                timer.remove();
            }
        }

        fn watch(&self) {
            self.unwatch();
            let widget = self.obj();
            let mut watches = self.watches.borrow_mut();
            let settings = widget.settings();
            let weak = widget.downgrade();
            let handler = settings.connect_gtk_enable_animations_notify(move |_| {
                if let Some(widget) = weak.upgrade() {
                    widget.imp().play();
                }
            });
            watches.push((settings.upcast(), handler));
            if let Some(scroller) = widget
                .ancestor(gtk::ScrolledWindow::static_type())
                .and_downcast::<gtk::ScrolledWindow>()
            {
                let adjustment = scroller.vadjustment();
                let weak = widget.downgrade();
                let handler = adjustment.connect_value_changed(move |_| {
                    if let Some(widget) = weak.upgrade() {
                        widget.queue_draw();
                    }
                });
                watches.push((adjustment.upcast(), handler));
            }
            if let Some(surface) = widget.native().and_then(|native| native.surface()) {
                let weak = widget.downgrade();
                let handler = surface.connect_scale_notify(move |_| {
                    if let Some(widget) = weak.upgrade() {
                        widget.queue_resize();
                    }
                });
                watches.push((surface.upcast(), handler));
            }
            // The scale may have changed while the widget was away.
            if (self.scale() - self.measured.get()).abs() > f64::EPSILON {
                widget.queue_resize();
            }
        }

        fn unwatch(&self) {
            for (object, handler) in self.watches.take() {
                object.disconnect(handler);
            }
        }
    }
}

glib::wrapper! {
    /// A pet playing its loop from its sheet, standing on the widget's floor.
    pub struct PetPreview(ObjectSubclass<imp::PetPreview>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PetPreview {
    /// `kind` at `base` physical pixels to the art pixel at 100 %: the extension's Small is
    /// 2. Pets in one row share a box, so they stand on one floor whichever they are.
    #[must_use]
    pub fn new(kind: &str, base: u32) -> Self {
        let preview: Self = glib::Object::builder().build();
        let imp = preview.imp();
        imp.base.set(base);
        *imp.kind.borrow_mut() = kind.to_owned();
        // Pets side by side would otherwise breathe in step, which no two animals do.
        let steps = sheet(kind).map_or(0, |sheet| sheet.steps.len());
        if steps > 0 {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_possible_wrap,
                clippy::cast_sign_loss
            )]
            imp.step.set(glib::random_int_range(0, steps as i32) as usize);
        }
        preview.update_property(&[gtk::accessible::Property::Label(&format!(
            "{} the {}",
            name(kind),
            species(kind)
        ))]);
        preview
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{KINDS, PackPet, pack_strip, parse, parse_packs};

    /// A pack's pet as `GetPetPacks` hands it over: the shape `packWire.ts` writes, which
    /// `pack.test.ts` reads back on the extension's side.
    const WIRE: &str = r##"{
        "folder": "/home/u/.local/share/octosnap/pets",
        "pets": [{
            "id": "mochi", "name": "Mochi", "species": "cat", "from": "/home/u/.local/share/octosnap/pets/mochi",
            "width": 3, "height": 2, "baseline": 2,
            "palette": ["#ff0000", "#00ff00"],
            "frames": ["010201000000", "000000020201"],
            "loop": [[0, 650, 0], [1, 110, 2]]
        }],
        "problems": [{ "id": "broken", "from": "/x/broken", "problem": "it has no sheet.png" }]
    }"##;

    fn mochi() -> PackPet {
        parse_packs(WIRE).expect("it reads").pets.remove(0)
    }

    /// The wire's fields, read: the folder, a pet, a pack left out with why.
    #[test]
    fn the_packs_read_as_the_extension_writes_them() {
        let packs = parse_packs(WIRE).expect("it reads");
        assert_eq!(packs.folder, "/home/u/.local/share/octosnap/pets");
        assert_eq!(packs.problems.len(), 1);
        assert_eq!(packs.problems[0].problem, "it has no sheet.png");
        let pet = &packs.pets[0];
        assert_eq!(
            (pet.id.as_str(), pet.name.as_str(), pet.species.as_str()),
            ("mochi", "Mochi", "cat")
        );
        assert_eq!(pet.steps, [(0, 650, 0), (1, 110, 2)]);
        // An extension that says nothing about packs is no packs, not an error.
        assert!(parse_packs("{}").expect("it reads").pets.is_empty());
    }

    /// A pack's pet becomes a strip `frames_as_rects` reads as it reads the five's, and the
    /// rectangles drawn for each frame are exactly its pixels.
    #[test]
    fn a_pack_pets_frames_are_its_pixels_exactly() {
        let (sheet, rgba) = pack_strip(&mochi()).expect("a strip");
        assert_eq!((sheet.width, sheet.height, sheet.frames, sheet.baseline), (3, 2, 2, 2));
        let (red, green) = ([255, 0, 0, 255], [0, 255, 0, 255]);
        let rects = super::frames_as_rects(&rgba, &sheet);
        assert_eq!(rects[0], [(0, 0, 1, 1, red), (1, 0, 1, 1, green), (2, 0, 1, 1, red)]);
        assert_eq!(rects[1], [(0, 1, 2, 1, green), (2, 1, 1, 1, red)]);
    }

    /// A pet nothing checked is not drawn: every way its numbers can fail to add up.
    #[test]
    fn a_pack_pet_that_does_not_add_up_is_refused() {
        type Spoil = fn(&mut PackPet);
        let cases: [(&str, Spoil); 7] = [
            ("too big", |p| p.width = 65),
            ("no frames", |p| p.frames.clear()),
            ("not a colour", |p| p.palette[0] = "red".to_owned()),
            ("a frame too short", |p| p.frames[0].truncate(10)),
            ("a colour past the palette", |p| p.frames[0].replace_range(0..2, "03")),
            ("not a number", |p| p.frames[0].replace_range(0..2, "zz")),
            ("a loop past the frames", |p| p.steps.push((2, 100, 0))),
        ];
        for (what, spoil) in cases {
            let mut pet = mochi();
            spoil(&mut pet);
            assert!(pack_strip(&pet).is_err(), "{what}");
        }
        let mut no_loop = mochi();
        no_loop.steps.clear();
        assert!(pack_strip(&no_loop).is_err(), "no loop");
    }

    const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/pets");

    /// The `pets` key names pets by kind, the Pets page offers one card for each kind here,
    /// and the extension knows them by its own list. A pet added there and not here would
    /// be a pet no card could bring out.
    #[test]
    fn the_app_knows_the_extensions_pets_in_their_order() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../extension/src/pets/art/index.ts"
        ));
        let list = &source[source.find("export const PET_KINDS").expect("the list")..];
        let list = &list[list.find("= [").expect("its start") + 3..];
        let list = &list[..list.find(']').expect("its end")];
        let kinds: Vec<&str> = list.split(',').map(|kind| kind.trim().trim_matches('\'')).collect();
        assert_eq!(kinds, KINDS);
    }

    /// Every pet has a sheet, its strip holds the frames the sheet says, and every step of
    /// its loop names one of them. The extension's `sheets.test.ts` writes the files; this
    /// holds the app's reading of them.
    #[test]
    fn every_pet_has_a_sheet_whose_strip_holds_its_frames() {
        let json = std::fs::read(format!("{DATA}/pets.json")).expect("pets.json");
        let sheets = parse(&json).expect("it reads");
        assert_eq!(sheets.len(), KINDS.len());
        for kind in KINDS {
            let sheet = sheets.get(kind).unwrap_or_else(|| panic!("a sheet for the {kind}"));
            let file = std::fs::File::open(format!("{DATA}/{kind}.png")).expect("its strip");
            let reader =
                png::Decoder::new(std::io::BufReader::new(file)).read_info().expect("a PNG");
            let info = reader.info();
            assert_eq!(
                (info.width, info.height),
                (sheet.width * sheet.frames, sheet.height),
                "the {kind}'s strip"
            );
            assert!(!sheet.steps.is_empty(), "the {kind} has a loop");
            for &(frame, ms, lift) in &sheet.steps {
                assert!(
                    frame < sheet.frames,
                    "the {kind}'s loop names frame {frame} of {}",
                    sheet.frames
                );
                assert!(ms > 0, "the {kind}'s loop has a step of no time");
                assert!(lift < sheet.height, "the {kind} jumps {lift} art pixels");
            }
        }
    }

    /// A frame is drawn as rectangles, so they must cover every pixel the strip draws,
    /// each once and in its own colour, and nothing it leaves clear.
    #[test]
    fn a_frames_rectangles_are_its_pixels_exactly() {
        let json = std::fs::read(format!("{DATA}/pets.json")).expect("pets.json");
        let sheets = parse(&json).expect("it reads");
        for kind in KINDS {
            let sheet = &sheets[kind];
            let bytes = std::fs::read(format!("{DATA}/{kind}.png")).expect("its strip");
            let rgba = super::decode(&bytes).expect("RGBA");
            let (width, height) = (sheet.width as usize, sheet.height as usize);
            let stride = width * sheet.frames as usize;
            for (frame, rects) in super::frames_as_rects(&rgba, sheet).iter().enumerate() {
                let mut drawn = vec![None; width * height];
                for &(x, y, w, h, colour) in rects {
                    for yy in y..y + h {
                        for xx in x..x + w {
                            let cell = &mut drawn[yy as usize * width + xx as usize];
                            assert!(
                                cell.is_none(),
                                "the {kind}'s frame {frame} draws ({xx}, {yy}) twice"
                            );
                            *cell = Some(colour);
                        }
                    }
                }
                for y in 0..height {
                    for x in 0..width {
                        let at = (y * stride + frame * width + x) * 4;
                        let pixel: [u8; 4] = rgba[at..at + 4].try_into().expect("four bytes");
                        let want = (pixel[3] != 0).then_some(pixel);
                        assert_eq!(
                            drawn[y * width + x],
                            want,
                            "the {kind}'s frame {frame} at ({x}, {y})"
                        );
                    }
                }
            }
        }
    }

    /// A strip the bundle leaves out is a card with no pet on it, and nothing but the log
    /// says so.
    #[test]
    fn the_bundle_carries_every_pets_strip() {
        let bundle =
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/resources/octosnap.gresource.xml"));
        assert!(bundle.contains("<file>pets/pets.json</file>"));
        for kind in KINDS {
            assert!(
                bundle.contains(&format!("<file>pets/{kind}.png</file>")),
                "the {kind}'s strip"
            );
        }
    }
}
