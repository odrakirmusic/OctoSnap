// SPDX-License-Identifier: GPL-3.0-or-later

//! The pointer says what a press would do (D57).
//!
//! The user's rule, verbatim: "have a better difference between clicking on an object like
//! an arrow and distinguish between actually drawing a new arrow. This is a rule for all
//! tools." So there are exactly three families of cursor on the canvas, and they never
//! overlap:
//!
//! - **Drawing** -- a crosshair with the tool's own glyph beside it, drawn at runtime from
//!   the same symbolic icon the toolbar shows. Only ever shown where a press would *create*
//!   something. No cursor theme ships a pencil or an arrow-tool cursor, so these are
//!   rendered here: a `GdkCursor` from a callback, at whatever size and scale the display
//!   asks for.
//! - **Selecting and moving** -- the theme's own hand (`pointer`) over an object a press
//!   would select, the four-way `move` over the object that is already selected, and the
//!   eight resize arrows over its handles. All from the cursor theme, so they look like
//!   every other app's.
//! - **Everything else** -- the plain arrow on empty desk with the Select tool, the I-beam
//!   for the text tool, `grab`/`grabbing` for a Space pan.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::prelude::*;
use gtk::{gdk, graphene, gsk};
use octosnap_scene::Handle;
use octosnap_scene::tool::Tool;

/// What the pointer is over, decided by `tools.rs`; this file only draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hover {
    /// One of §4.1's eight resize handles.
    Resize(Handle),
    /// A line's end or an arrow's curve handle: it goes anywhere.
    Grip,
    /// The selected object's body: a press moves it.
    Selected,
    /// An object a press would select.
    Selectable,
    /// Empty picture under a tool that would draw with a press.
    Draw(Tool),
    /// Empty picture under the text tool.
    Text,
    /// Empty picture under the Select tool.
    Idle,
    /// Space is held; a press would pan.
    Pan,
    Panning,
}

/// The cursors, built once each.
#[derive(Default)]
pub struct Cursors {
    drawn: RefCell<HashMap<Tool, gdk::Cursor>>,
}

impl std::fmt::Debug for Cursors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cursors").field("drawn", &self.drawn.borrow().len()).finish()
    }
}

impl Cursors {
    /// The cursor for a hover, or `None` for the widget's default.
    pub fn for_hover(&self, hover: Hover) -> Option<gdk::Cursor> {
        let named = |name: &str| gdk::Cursor::from_name(name, None);
        match hover {
            Hover::Resize(handle) => named(handle.cursor()),
            Hover::Grip => named("crosshair"),
            Hover::Selected => named("move"),
            Hover::Selectable => named("pointer"),
            Hover::Text => named("text"),
            Hover::Idle => named("default"),
            Hover::Pan => named("grab"),
            Hover::Panning => named("grabbing"),
            Hover::Draw(tool) => self.drawing(tool),
        }
    }

    /// The crosshair-and-glyph cursor for a drawing tool, made on first use.
    fn drawing(&self, tool: Tool) -> Option<gdk::Cursor> {
        if let Some(cursor) = self.drawn.borrow().get(&tool) {
            return Some(cursor.clone());
        }
        let fallback = gdk::Cursor::from_name("crosshair", None);
        let cursor = gdk::Cursor::from_callback(
            move |_, size, scale, width, height, hotspot_x, hotspot_y| {
                let side = f64::from(size.max(24));
                *width = size.max(24);
                *height = size.max(24);
                // The crosshair's centre is the hotspot, in the upper-left part of the
                // image so the glyph has room in the lower-right without covering what
                // the user is aiming at.
                let centre = side * 0.34;
                #[allow(clippy::cast_possible_truncation)]
                {
                    *hotspot_x = centre.round() as i32;
                    *hotspot_y = centre.round() as i32;
                }
                draw_drawing_cursor(tool, side, centre, scale)
            },
            fallback.as_ref(),
        )
        .or(fallback)?;
        self.drawn.borrow_mut().insert(tool, cursor.clone());
        Some(cursor)
    }
}

/// One drawing cursor at `side` logical pixels and `scale`: a crosshair centred at
/// `centre` and the tool's glyph in the lower-right, black on a white halo so it reads
/// on any picture.
pub(super) fn draw_drawing_cursor(tool: Tool, side: f64, centre: f64, scale: f64) -> gdk::Texture {
    let snapshot = gtk::Snapshot::new();
    #[allow(clippy::cast_possible_truncation)]
    snapshot.scale(scale as f32, scale as f32);

    let black = gdk::RGBA::new(0.0, 0.0, 0.0, 0.9);
    let white = gdk::RGBA::new(1.0, 1.0, 1.0, 0.95);
    let arm = side * 0.26;
    let gap = side * 0.06;
    // The cross: white under black, so the black reads on a dark picture and the white
    // on a light one -- the same construction the theme's own crosshair uses.
    for (colour, weight) in [(white, 3.0f32), (black, 1.5f32)] {
        let builder = gsk::PathBuilder::new();
        #[allow(clippy::cast_possible_truncation)]
        for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            builder.move_to((centre + dx * gap) as f32, (centre + dy * gap) as f32);
            builder.line_to((centre + dx * arm) as f32, (centre + dy * arm) as f32);
        }
        let stroke = gsk::Stroke::new(weight);
        stroke.set_line_cap(gsk::LineCap::Round);
        snapshot.append_stroke(&builder.to_path(), &stroke, &colour);
    }

    // The tool's glyph, from the icon the toolbar shows for it, with a white halo.
    let glyph = side * 0.5;
    if let Some(display) = gdk::Display::default() {
        let theme = gtk::IconTheme::for_display(&display);
        #[allow(clippy::cast_possible_truncation)]
        let paintable = theme.lookup_icon(
            tool.icon(),
            &[],
            glyph.round() as i32,
            scale.ceil() as i32,
            gtk::TextDirection::None,
            gtk::IconLookupFlags::empty(),
        );
        snapshot.save();
        #[allow(clippy::cast_possible_truncation)]
        snapshot.translate(&graphene::Point::new((side - glyph) as f32, (side - glyph) as f32));
        snapshot.push_shadow(&[gsk::Shadow::new(white, 0.0, 0.0, 1.5)]);
        paintable.snapshot_symbolic(&snapshot, glyph, glyph, &[black]);
        snapshot.pop();
        snapshot.restore();
    }

    #[allow(clippy::cast_possible_truncation)]
    let pixels = (side * scale).ceil() as f32;
    let bounds = graphene::Rect::new(0.0, 0.0, pixels, pixels);
    snapshot.to_node().and_then(|node| render(&node, &bounds)).unwrap_or_else(|| blank(pixels))
}

/// Draws a cursor image with a software renderer: a few dozen pixels a side, and it needs
/// no surface -- which is the point, because a cursor is asked for before there is
/// anything to draw it on.
///
/// A renderer of its own every time, realized, used and unrealized here (D114). GSK
/// aborts the process when a renderer is disposed while still realized. The one renderer
/// this used to keep for the thread was never unrealized, so the thread-local's destructor
/// disposed it at exit, and every quit after an editor had shown a drawing cursor
/// aborted. Without a surface, realizing a software renderer only marks it realized, so
/// there is nothing to save by keeping one.
fn render(node: &gsk::RenderNode, bounds: &graphene::Rect) -> Option<gdk::Texture> {
    let renderer = gsk::CairoRenderer::new();
    if let Err(e) = renderer.realize(None::<&gdk::Surface>) {
        tracing::warn!("cursor renderer: {e}");
        return None;
    }
    let texture = renderer.render_texture(node, Some(bounds));
    renderer.unrealize();
    Some(texture)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn blank(pixels: f32) -> gdk::Texture {
    let side = pixels as usize;
    let bytes = gtk::glib::Bytes::from_owned(vec![0u8; side * side * 4]);
    gdk::MemoryTexture::new(side as i32, side as i32, gdk::MemoryFormat::R8g8b8a8, &bytes, side * 4)
        .upcast()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Set in the child's environment: the test draws, rather than starting a child.
    const CHILD: &str = "OCTOSNAP_CURSOR_TEST_CHILD";

    /// What the child prints once it has drawn. The parent needs it: a filter that matches
    /// no test at all also exits 0.
    const DREW: &str = "cursor images drawn:";

    /// 2026-09-24's quit abort (D114). The test runs itself again in a process of its own,
    /// because the failure is an abort and would take every other test in this binary down
    /// with it.
    ///
    /// GSK asserts that a renderer is no longer realized when it is disposed. The cursors'
    /// renderer was realized once and never unrealized, so whatever dropped the last
    /// reference to it aborted: the thread-local's destructor, as the process exited. The
    /// child draws on its test thread and then ends, and the end of that thread runs its
    /// thread-locals' destructors the same way. There is no display, and none is needed:
    /// the software renderer realizes without one, and the abort happens without one too.
    #[test]
    fn a_process_that_drew_cursor_images_ends_without_an_abort() {
        if std::env::var_os(CHILD).is_some() {
            draw_cursor_images();
            return;
        }
        let test = "a_process_that_drew_cursor_images_ends_without_an_abort";
        let path = module_path!().split_once("::").map_or("", |(_, path)| path);
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("{path}::{test}"), "--nocapture"])
            .env(CHILD, "1")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .output()
            .unwrap();
        // Both streams: the test harness reports on stdout, a panic or an abort on stderr.
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let status = output.status;
        assert!(status.success(), "the child ended with {status}:\n{stdout}\n{stderr}");
        assert!(stdout.contains(DREW), "the child drew nothing:\n{stdout}");
    }

    /// The child's half: a cursor image at every quarter scale from 1 to 2, through the
    /// renderer the cursors use, each exactly the size it was asked for.
    fn draw_cursor_images() {
        let black = gdk::RGBA::new(0.0, 0.0, 0.0, 1.0);
        let mut drawn = 0;
        for scale in [1.0f32, 1.25, 1.5, 1.75, 2.0] {
            let pixels = (24.0 * scale).ceil();
            let bounds = graphene::Rect::new(0.0, 0.0, pixels, pixels);
            let node = gsk::ColorNode::new(&black, &bounds);
            let texture = render(node.upcast_ref(), &bounds).expect("a software renderer");
            #[allow(clippy::cast_possible_truncation)]
            let side = pixels as i32;
            assert_eq!((texture.width(), texture.height()), (side, side), "at {scale}");
            drawn += 1;
        }
        println!("{DREW} {drawn}");
    }
}
