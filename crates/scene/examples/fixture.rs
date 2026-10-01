// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §11 item 1's scene, written as `objects.json`.
//!
//! > Open a 5120x2880 capture; add 30 objects including two blurs and a spotlight; drag
//! > any object at 60 fps.
//!
//! An example rather than a script in `docs/spikes/tools`, and that is the whole point:
//! it builds the objects through the real types, so the fixture cannot drift from
//! `objects.json`'s actual shape. A Python script hand-writing the JSON would be a second
//! definition of §5.1, and the first time a field was renamed the fixture would go on
//! loading and silently stop representing anything.
//!
//! Run it with:
//!
//! ```text
//! cargo run -p octosnap-scene --example fixture -- <out.json> [width] [height]
//! ```
//!
//! Deterministic, so two runs produce byte-identical files and a measurement can be
//! compared against yesterday's. The ids are ULIDs, which are not -- so they are
//! **seeded** here from the object's index rather than generated, which is the one place
//! this file deliberately departs from what the editor does.

use octosnap_scene::object::{Geometry, Object, ObjectId, TextAlign};
use octosnap_scene::style::{
    ArrowStyle, CounterStyle, RedactStyle, Rgba, SpotlightShape, Style, TextStyle,
};
use octosnap_scene::{Base, Bounds, Point, Scene};

/// `spec/05` §11 item 1's count, and the two kinds it names explicitly.
const OBJECTS: usize = 30;

/// How many points the two freehand strokes carry.
///
/// Long on purpose. §6's budget is "build <= 2 ms of nodes per frame during a drag", and a
/// path is the only object whose node cost grows with the *gesture* rather than with its
/// bounds -- thirty rectangles are thirty cheap nodes, and one pencil stroke across a 5K
/// capture is a few hundred segments in a single `gsk::Path`. A fixture of thirty
/// rectangles would pass a budget the real worst case fails.
const PATH_POINTS: usize = 320;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(out) = args.next() else {
        eprintln!("usage: fixture <out.json> [width] [height]");
        std::process::exit(2);
    };
    let width: f64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(5120.0);
    let height: f64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(2880.0);

    let mut scene = Scene::new(Base::new("fixture.png", width, height, 2.0));
    for (index, object) in objects(width, height).into_iter().enumerate() {
        let _ = index;
        scene.add(object);
    }

    let json = match scene.to_objects_json() {
        Ok(json) => json,
        Err(e) => {
            eprintln!("could not serialise the fixture: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = std::fs::write(&out, format!("{json}\n")) {
        eprintln!("could not write {out}: {e}");
        std::process::exit(1);
    }
    let redactions = scene
        .objects()
        .iter()
        .filter(|o| matches!(o.geometry, Geometry::Redact { .. }))
        .count();
    let spotlights = scene
        .objects()
        .iter()
        .filter(|o| matches!(o.geometry, Geometry::Spotlight { .. }))
        .count();
    println!(
        "{out}: {} objects on {width}x{height}, {redactions} redactions, {spotlights} spotlight, \
         {PATH_POINTS}-point strokes",
        scene.len(),
    );
}

/// `spec/05` §11 item 1's thirty, spread over the canvas so nothing overlaps by accident.
///
/// Every kind the render walk can draw appears at least once, because the budget is about
/// the *tree* and a fixture that omits a kind cannot measure it. The two redactions and
/// the spotlight are the ones §11 item 1 names; the rest is a spread.
fn objects(width: f64, height: f64) -> Vec<Object> {
    let mut objects = Vec::with_capacity(OBJECTS);
    // A grid to place things on, so the fixture looks like a marked-up screenshot rather
    // than a pile at the origin.
    let cols = 6.0;
    let rows = 5.0;
    let cell_w = width / cols;
    let cell_h = height / rows;
    let cell = |index: usize| {
        #[allow(clippy::cast_precision_loss)]
        let i = index as f64;
        let col = i % cols;
        let row = (i / cols).floor();
        Bounds::new(
            col * cell_w + cell_w * 0.1,
            row * cell_h + cell_h * 0.1,
            cell_w * 0.8,
            cell_h * 0.8,
        )
    };

    for index in 0..OBJECTS {
        let at = cell(index);
        #[allow(clippy::cast_possible_truncation)]
        let size = (index % 6 + 1) as u8;
        let style = Style::new(palette(index), size, index % 4 == 0);
        let geometry = match index {
            // The two `spec/05` §11 item 1 names first, so a fixture truncated by hand
            // still has them.
            // 500 x 300 units on a 2x base is the **1000 x 600 px** region `spec/05`
            // §6's third budget is written about, so the app's `redaction rasterized`
            // line for this object is the budget's own measurement. On a whole unit, or
            // the raster -- which snaps outward to the pixel grid -- comes out 1001 x 601.
            0 => Geometry::Redact {
                bounds: Bounds::new(at.x.round(), at.y.round(), 500.0, 300.0),
                style: RedactStyle::SecureBlur,
                intensity: 0.6,
                seed: 0x5eed_0001,
            },
            1 => Geometry::Redact {
                bounds: at,
                style: RedactStyle::Pixelate,
                intensity: 0.8,
                seed: 0x5eed_0002,
            },
            2 => Geometry::Spotlight {
                shape: SpotlightShape::Rounded,
                bounds: at,
                opacity: 0.55,
            },
            // The two long strokes: the worst case for node building.
            3 => Geometry::Path {
                points: stroke(at, PATH_POINTS, false),
                smoothing: true,
                highlighter: false,
                band: None,
            },
            4 => Geometry::Path {
                points: stroke(at, PATH_POINTS, true),
                smoothing: false,
                highlighter: true,
                band: None,
            },
            _ => match index % 7 {
                0 => Geometry::Rect { bounds: at, filled: index % 14 == 0, radius: 12.0 },
                1 => Geometry::Ellipse { bounds: at },
                2 => Geometry::Arrow {
                    start: Point::new(at.x, at.y),
                    end: Point::new(at.x + at.width, at.y + at.height),
                    // Every other arrow is curved, so `ctrl` is exercised.
                    ctrl: (index % 2 == 0).then(|| at.center()),
                    style: arrow_style(index),
                    head: octosnap_scene::ArrowHead::default(),
                },
                3 => Geometry::Line {
                    start: Point::new(at.x, at.y + at.height),
                    end: Point::new(at.x + at.width, at.y),
                },
                4 => Geometry::Counter {
                    center: at.center(),
                    #[allow(clippy::cast_possible_truncation)]
                    number: (index as u32) % 40 + 1,
                    style: counter_style(index),
                    radius: 31.0,
                },
                5 => Geometry::Text {
                    pos: Point::new(at.x, at.y),
                    text: format!("Object {index} \u{2014} \u{1f427} \u{6f22}\u{5b57}"),
                    style: text_style(index),
                    font_size: 36.0,
                    align: TextAlign::Start,
                    width: None,
                },
                _ => Geometry::Path {
                    points: stroke(at, 24, false),
                    smoothing: true,
                    highlighter: false,
                    band: None,
                },
            },
        };
        let mut object = Object::new(
            #[allow(clippy::cast_possible_truncation)]
            {
                index as i32
            },
            // A fixed creation time, so two runs of this produce identical bytes.
            1_757_500_000,
            style,
            geometry,
        );
        object.id = seeded_id(index);
        objects.push(object);
    }
    objects
}

/// A deterministic id, so the fixture is byte-stable across runs.
///
/// `ObjectId::new` is a fresh ULID and therefore carries the clock; a fixture that
/// changed every time could not be diffed and could not be committed.
fn seeded_id(index: usize) -> ObjectId {
    // A ULID is 128 bits; the index in the low bits and a fixed marker in the high ones,
    // so the ids sort in the order the objects were made -- which is what a real ULID
    // would do.
    #[allow(clippy::cast_possible_truncation)]
    let low = index as u128;
    ObjectId::from_ulid(ulid::Ulid(0x0192_0000_0000_0000_0000_0000_0000_0000 | low))
}

/// A stroke that wanders across `at`, which is what a pencil gesture produces.
fn stroke(at: Bounds, points: usize, flat: bool) -> Vec<Point> {
    (0..points)
        .map(|i| {
            #[allow(clippy::cast_precision_loss)]
            let t = i as f64 / (points.max(2) - 1) as f64;
            let wobble = if flat { 0.0 } else { (t * 9.0).sin() * at.height * 0.35 };
            Point::new(
                at.x + at.width * t,
                at.y + at.height * 0.5 + wobble,
            )
        })
        .collect()
}

/// Ten colours, walked, so no two neighbours share one.
fn palette(index: usize) -> Rgba {
    let (_, hex) = octosnap_scene::PALETTE
        .get(index % octosnap_scene::PALETTE.len())
        .copied()
        .unwrap_or(("Red", "E01B24"));
    Rgba::from_hex(hex).unwrap_or(Rgba::new(1.0, 0.2, 0.2, 1.0))
}

fn arrow_style(index: usize) -> ArrowStyle {
    let all = ArrowStyle::ALL;
    all.get(index % all.len()).copied().unwrap_or(ArrowStyle::Standard)
}

fn counter_style(index: usize) -> CounterStyle {
    let all = CounterStyle::ALL;
    all.get(index % all.len()).copied().unwrap_or(CounterStyle::Arabic)
}

fn text_style(index: usize) -> TextStyle {
    let all = TextStyle::ALL;
    all.get(index % all.len()).copied().unwrap_or(TextStyle::Standard)
}
