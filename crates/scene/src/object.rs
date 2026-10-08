// SPDX-License-Identifier: GPL-3.0-or-later

//! One annotation, and everything it is (`spec/05` §5.1).
//!
//! The shape of this type is what `objects.json` is, so `spec/05` §11 item 4 -- "undo 50
//! steps and redo 50 steps restores byte-identical `objects.json`" -- is a statement
//! about it: the field order here is the field order on disk, and `Object` is `Clone`
//! rather than reference-counted so a command can hold a whole "before" without the
//! caller having to know it did.

use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::background::BackgroundParams;
use crate::geometry::{Bounds, Point, Segment};
use crate::handle::Handle;
use crate::style::{
    ArrowHead, ArrowStyle, CounterStyle, RedactStyle, SpotlightShape, Style, TextStyle,
};

/// `spec/05` §5.1's `"id": "01J…"`: a ULID, so ids sort by creation and never collide
/// across a merge of two projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObjectId(Ulid);

impl ObjectId {
    /// A fresh id, stamped now.
    ///
    /// `ulid` 3.0 has no `Ulid::new()`: generation takes the clock explicitly, which is
    /// the better shape anyway -- `spec/05` §5.1 pairs the id with a `created` timestamp,
    /// and an id whose time component came from a different reading than the field beside
    /// it would sort in an order the file does not show.
    #[must_use]
    pub fn new() -> Self {
        Self(Ulid::from_datetime(std::time::SystemTime::now()))
    }

    /// For tests and for reading a project back, where the id already exists.
    #[must_use]
    pub const fn from_ulid(ulid: Ulid) -> Self {
        Self(ulid)
    }
}

impl Default for ObjectId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What an object *is*, without its measurements.
///
/// Separate from [`Geometry`] because most questions -- which options row to show, whether
/// this is a counter and therefore always on top, whether it exports -- are about the kind
/// and not the numbers, and matching a twelve-arm enum to answer a boolean makes those
/// call sites unreadable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObjectKind {
    Arrow,
    Line,
    Rect,
    Ellipse,
    Text,
    Path,
    Spotlight,
    Counter,
    Redact,
    Image,
    Background,
    Crop,
}

/// `spec/05` §5.1's per-type geometry.
///
/// **Adjacently** tagged -- `tag = "type", content = "geometry"` -- and then flattened
/// into [`Object`], which is the one combination that produces the shape §5.1 writes:
/// `"type"` beside `"id"` and `"z"` at the top level, and the per-type fields together
/// under `"geometry"`. Internal tagging would put `type` inside the geometry object;
/// flattening the fields themselves would spread `bounds`, `start` and `points` across
/// the top level and collide with the common fields -- which is what the first version
/// did, and what its own round-trip test caught: `Geometry::Arrow`'s `style` field
/// overwrote the object's `style` and the size and colour vanished from the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "geometry", rename_all = "kebab-case")]
pub enum Geometry {
    Arrow {
        start: Point,
        end: Point,
        /// One control point, present only for [`ArrowStyle::Curved`] -- `spec/05` §5.1
        /// writes it `ctrl?`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ctrl: Option<Point>,
        style: ArrowStyle,
        /// The tip's shape. Defaulted on read, so a document from before there was a
        /// choice opens with the triangle it was drawn with.
        #[serde(default)]
        head: ArrowHead,
    },
    Line {
        start: Point,
        end: Point,
    },
    Rect {
        bounds: Bounds,
        filled: bool,
        /// `spec/05` §2's "corner radius toggle", as the radius itself: a toggle is a
        /// choice between two numbers and storing the number means a reopened project
        /// does not depend on what that toggle happened to mean.
        radius: f64,
    },
    Ellipse {
        bounds: Bounds,
    },
    Text {
        pos: Point,
        text: String,
        style: TextStyle,
        font_size: f64,
        align: TextAlign,
        /// Set once the user has dragged a width, which turns the run from one line into
        /// a wrapped block (`spec/05` §4.5's "drag handles").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f64>,
    },
    Path {
        points: Vec<Point>,
        /// `spec/05` §2's pencil "smoothing on/off".
        smoothing: bool,
        /// `spec/05` §5.3: the highlighter renders with multiply blending, so it is a
        /// property of the path and not a separate object type.
        highlighter: bool,
        /// `spec/05` §4.7's smart highlighter: the height of the text line this stroke
        /// snapped to, in document units.
        ///
        /// Stored rather than re-detected, because the pixels the detector read are the
        /// base image's and a reopened project must draw the same band without them --
        /// and because the size control is what the *search* used, so it can no longer
        /// be what the width means. Absent is a freehand stroke, whose width is §4.7's
        /// `3 × size` as it always was.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        band: Option<f64>,
    },
    Spotlight {
        shape: SpotlightShape,
        bounds: Bounds,
        /// `spec/05` §2: "opacity **slider** (a real slider, unlike size)".
        opacity: f64,
    },
    Counter {
        center: Point,
        number: u32,
        style: CounterStyle,
        /// The badge's radius, from `spec/05` §2's counter settings menu.
        radius: f64,
    },
    Redact {
        bounds: Bounds,
        style: RedactStyle,
        intensity: f64,
        /// `spec/05` §11 item 6: "pixelate on the same region twice with different seeds
        /// produces different blocks". The seed is stored, so a reopened project
        /// re-rasterises to the same pixels rather than to new ones.
        seed: u64,
    },
    Image {
        bounds: Bounds,
        /// A name inside the project's `assets/`, never a path into the user's
        /// filesystem: `spec/05` §8 makes the project self-contained.
        file: String,
    },
    Background {
        params: BackgroundParams,
        /// The rect the parameters lay out, in the base image's own coordinates.
        ///
        /// The base image itself unless `spec/05` §4.11's crop had already narrowed the
        /// document when the background was applied -- and then it is the crop, because a
        /// user who cropped to a dialog and then chose a gradient wants the gradient
        /// around the dialog and not around the whole screen. Absent means the base,
        /// which is also what every project written before the tool existed means.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<Bounds>,
    },
    Crop {
        canvas_rect: Bounds,
    },
}

/// What `spec/05` §4.1's selection chrome offers on one object.
///
/// > Selection chrome: 1 px accent outline around the object bounds + **8 handles
/// > (corner + edge) for resizable objects**; line/arrow objects show **2 end handles
/// > (+1 curve handle for curved arrows)**.
///
/// Three cases and not a boolean, because the *count* is part of the answer: the canvas
/// draws what this says and the hit test reads the same list, so a curved arrow's third
/// handle cannot be drawn and then be unclickable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grips {
    /// The eight, on the object's own bounds.
    Box,
    /// The two ends of a line, plus a curve handle when there is one. The points are
    /// [`Object::points`], in that order.
    Ends(usize),
    /// Nothing to drag. The background and the crop *are* the canvas.
    None,
}

/// `spec/05` §4.5's text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextAlign {
    #[default]
    Start,
    Center,
    End,
}

impl TextAlign {
    /// In the order the text row's menu lists them (D167).
    pub const ALL: [Self; 3] = [Self::Start, Self::Center, Self::End];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Start => "Left",
            Self::Center => "Centre",
            Self::End => "Right",
        }
    }

    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Start => "format-justify-left-symbolic",
            Self::Center => "format-justify-center-symbolic",
            Self::End => "format-justify-right-symbolic",
        }
    }
}

/// One annotation: `spec/05` §5.1's common fields plus its geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Object {
    pub id: ObjectId,
    /// Stacking position. Not the index in the scene's vector: `spec/05` §5.3 renders
    /// some kinds out of `z` order regardless (redactions rasterise what is below *their*
    /// z, counters are always on top), so the order objects are held in and the order
    /// they are drawn in are two different questions and both need answering.
    pub z: i32,
    /// Seconds since the Unix epoch, from `spec/05` §5.1's `"created"`.
    pub created: u64,
    /// Flattened, so `color`, `size` and `shadow` sit at the top level as §5.1 has them.
    #[serde(flatten)]
    pub style: Style,
    #[serde(default)]
    pub locked: bool,
    /// Flattened, and adjacently tagged, so this contributes `type` and `geometry` --
    /// two keys, at the top level, exactly as §5.1 writes them. Neither collides with a
    /// common field, which is the whole reason the tagging is adjacent.
    #[serde(flatten)]
    pub geometry: Geometry,
}

impl Object {
    /// A new object at `z`, stamped now.
    #[must_use]
    pub fn new(z: i32, created: u64, style: Style, geometry: Geometry) -> Self {
        Self { id: ObjectId::new(), z, created, style, locked: false, geometry }
    }

    #[must_use]
    pub const fn kind(&self) -> ObjectKind {
        match self.geometry {
            Geometry::Arrow { .. } => ObjectKind::Arrow,
            Geometry::Line { .. } => ObjectKind::Line,
            Geometry::Rect { .. } => ObjectKind::Rect,
            Geometry::Ellipse { .. } => ObjectKind::Ellipse,
            Geometry::Text { .. } => ObjectKind::Text,
            Geometry::Path { .. } => ObjectKind::Path,
            Geometry::Spotlight { .. } => ObjectKind::Spotlight,
            Geometry::Counter { .. } => ObjectKind::Counter,
            Geometry::Redact { .. } => ObjectKind::Redact,
            Geometry::Image { .. } => ObjectKind::Image,
            Geometry::Background { .. } => ObjectKind::Background,
            Geometry::Crop { .. } => ObjectKind::Crop,
        }
    }

    /// The object's extent in image coordinates, **including the stroke**.
    ///
    /// Half a stroke width falls outside the geometry on each side, and leaving it out
    /// makes a thick line's selection outline cut through the line. `spec/05` §10 animates
    /// that outline, so it is visible enough to be noticed.
    ///
    /// Text is the one kind this cannot answer honestly: its height comes from Pango, in
    /// the app. The estimate here is the font size for a single line, which is right for
    /// hit testing a click on the first line and wrong for anything else -- the app
    /// replaces it with the measured layout, the way the card measures its own controls.
    #[must_use]
    pub fn bounds(&self) -> Bounds {
        let stroke = self.style.stroke_width();
        match &self.geometry {
            Geometry::Arrow { start, end, ctrl, .. } => {
                // The box around the *curve*, not around the control point: the control
                // point is selection chrome and a quadratic never reaches it (D57).
                let shaft = match ctrl {
                    Some(c) => crate::geometry::quad_flatten(*start, *c, *end, crate::geometry::CURVE_LEGS)
                        .into_iter()
                        .fold(Bounds::new(start.x, start.y, 0.0, 0.0), |b, p| {
                            b.union(Bounds::new(p.x, p.y, 0.0, 0.0))
                        }),
                    None => Segment::new(*start, *end).bounds(),
                };
                // The head reaches further than the shaft, and by exactly what the
                // renderer draws -- see `style::arrowhead_reach`.
                shaft.inflated(crate::style::arrowhead_reach(stroke).max(stroke / 2.0))
            }
            Geometry::Line { start, end } => {
                Segment::new(*start, *end).bounds().inflated(stroke / 2.0)
            }
            Geometry::Rect { bounds, .. } | Geometry::Ellipse { bounds } => {
                bounds.inflated(stroke / 2.0)
            }
            Geometry::Text { pos, text, font_size, width, .. } => {
                let lines = text.lines().count().max(1) as f64;
                let w = width.unwrap_or_else(|| {
                    // 0.6 em per character is the usual rule of thumb for a proportional
                    // face; the app measures for real.
                    let longest = text.lines().map(str::chars).map(Iterator::count).max().unwrap_or(0);
                    longest as f64 * font_size * 0.6
                });
                Bounds::new(pos.x, pos.y, w, lines * font_size * 1.2)
            }
            // A stroke with no points has no bounds, and is **not** inflated: growing an
            // empty rect by half a stroke invents a 1 px box at the image's origin, which
            // a marquee dragged over the top-left corner would then select. `None` folds
            // to empty; only a path that has points gets the stroke added.
            Geometry::Path { points, highlighter, band, .. } => points
                .iter()
                .copied()
                .fold(None::<Bounds>, |acc, p| {
                    let dot = Bounds::new(p.x, p.y, 0.0, 0.0);
                    Some(acc.map_or(dot, |b| b.union(dot)))
                })
                .map_or(Bounds::new(0.0, 0.0, 0.0, 0.0), |b| {
                    // A highlighter is three times as wide (`spec/05` §4.7), so its
                    // bounds are too. The selection chrome and the hit test both read
                    // this, and a box drawn at a third of the wash's width sits inside
                    // the stroke it is supposed to surround. A stroke that snapped to a
                    // line of text is as wide as the line it covers instead.
                    let half = if *highlighter {
                        crate::tool::highlighter_width(stroke, *band) / 2.0
                    } else {
                        stroke / 2.0
                    };
                    b.inflated(half)
                }),
            Geometry::Spotlight { bounds, .. }
            | Geometry::Redact { bounds, .. }
            | Geometry::Image { bounds, .. }
            | Geometry::Crop { canvas_rect: bounds } => *bounds,
            Geometry::Counter { center, radius, .. } => {
                Bounds::new(center.x - radius, center.y - radius, radius * 2.0, radius * 2.0)
            }
            // The background is the canvas, so it has no bounds of its own; the scene's
            // canvas rect is the answer and only the scene knows it.
            Geometry::Background { .. } => Bounds::new(0.0, 0.0, 0.0, 0.0),
        }
    }

    /// Moves the object by a delta, which is what a drag and `spec/05` §9's arrow keys
    /// both do.
    ///
    /// One method rather than each tool moving its own fields, because "move" has to mean
    /// the same thing for every kind or a multi-object drag would shear.
    pub fn translate(&mut self, dx: f64, dy: f64) {
        let shift = |p: &mut Point| *p = p.offset(dx, dy);
        let shift_bounds = |b: &mut Bounds| {
            b.x += dx;
            b.y += dy;
        };
        match &mut self.geometry {
            Geometry::Arrow { start, end, ctrl, .. } => {
                shift(start);
                shift(end);
                if let Some(c) = ctrl {
                    shift(c);
                }
            }
            Geometry::Line { start, end } => {
                shift(start);
                shift(end);
            }
            Geometry::Rect { bounds, .. }
            | Geometry::Ellipse { bounds }
            | Geometry::Spotlight { bounds, .. }
            | Geometry::Redact { bounds, .. }
            | Geometry::Image { bounds, .. }
            | Geometry::Crop { canvas_rect: bounds } => shift_bounds(bounds),
            Geometry::Text { pos, .. } => shift(pos),
            Geometry::Path { points, .. } => points.iter_mut().for_each(shift),
            Geometry::Counter { center, .. } => shift(center),
            // Nothing to move: the background *is* the canvas.
            Geometry::Background { .. } => {}
        }
    }

    /// Which handles `spec/05` §4.1's chrome puts on this object.
    #[must_use]
    pub fn grips(&self) -> Grips {
        match &self.geometry {
            // "line/arrow objects show 2 end handles (+1 curve handle for curved arrows)"
            Geometry::Arrow { ctrl, .. } => Grips::Ends(if ctrl.is_some() { 3 } else { 2 }),
            Geometry::Line { .. } => Grips::Ends(2),
            Geometry::Background { .. } | Geometry::Crop { .. } => Grips::None,
            _ => Grips::Box,
        }
    }

    /// The draggable points of a [`Grips::Ends`] object, in the order its handles are
    /// drawn: start, end, and the curve control if there is one.
    ///
    /// Empty for everything else, so a caller that asks the wrong question gets nothing
    /// to drag rather than a point that means something different.
    #[must_use]
    pub fn points(&self) -> Vec<Point> {
        match &self.geometry {
            Geometry::Arrow { start, end, ctrl, .. } => match ctrl {
                Some(c) => vec![*start, *end, *c],
                None => vec![*start, *end],
            },
            Geometry::Line { start, end } => vec![*start, *end],
            _ => Vec::new(),
        }
    }

    /// Moves one of [`Self::points`]. Answers whether there was such a point.
    pub fn move_point(&mut self, index: usize, to: Point) -> bool {
        match &mut self.geometry {
            Geometry::Arrow { start, end, ctrl, .. } => match (index, ctrl) {
                (0, _) => {
                    *start = to;
                    true
                }
                (1, _) => {
                    *end = to;
                    true
                }
                (2, Some(c)) => {
                    *c = to;
                    true
                }
                _ => false,
            },
            Geometry::Line { start, end } => match index {
                0 => {
                    *start = to;
                    true
                }
                1 => {
                    *end = to;
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// Resizes so that the box the handles are on becomes `to`.
    ///
    /// `from` is passed in rather than read from [`Self::bounds`], and that is the whole
    /// reason this is testable: a text object's real box needs a Pango layout, which only
    /// the widget has, so the caller measures and this does arithmetic. For every other
    /// kind the two agree, and the tests pass `object.bounds()`.
    ///
    /// `handle` matters to exactly one kind. `spec/05` §4.5 gives a text object two
    /// different resizes -- "dragging a text object's corner handle sets any value in
    /// between" for the *size*, and "drag handles" turning a run into a wrapped block for
    /// the *width* -- so a corner scales the glyphs and a side sets the wrap. Everything
    /// else ignores it, because a rectangle does not care which corner was pulled once
    /// the resulting box is known.
    pub fn resize(&mut self, from: Bounds, to: Bounds, handle: Handle) {
        let from = from.normalised();
        let to = to.normalised();
        if from.is_empty() {
            return;
        }
        let stroke = self.style.stroke_width();
        match &mut self.geometry {
            // The stroke straddles the edge, so the *visible* box is half a stroke wider
            // than the geometry on every side -- and the handle is on the visible box.
            // Deflating is what keeps the corner under the pointer.
            Geometry::Rect { bounds, .. } | Geometry::Ellipse { bounds } => {
                *bounds = to.inflated(-stroke / 2.0);
            }
            Geometry::Spotlight { bounds, .. }
            | Geometry::Redact { bounds, .. }
            | Geometry::Image { bounds, .. } => *bounds = to,
            Geometry::Counter { center, radius, .. } => {
                // A badge is a disc, so the smaller extent decides -- dragging a corner
                // outwards must not leave an ellipse.
                *radius = (to.width.min(to.height) / 2.0).max(1.0);
                *center = to.center();
            }
            Geometry::Path { points, highlighter, .. } => {
                let half = if *highlighter {
                    stroke * crate::tool::HIGHLIGHTER_WIDTH_FACTOR / 2.0
                } else {
                    stroke / 2.0
                };
                // Both boxes without the stroke, so the mapping is between the points
                // themselves and a one-point stroke does not collapse.
                let raw_from = from.inflated(-half);
                let raw_to = to.inflated(-half);
                if raw_from.width <= 0.0 || raw_from.height <= 0.0 {
                    return;
                }
                let sx = raw_to.width / raw_from.width;
                let sy = raw_to.height / raw_from.height;
                for point in points.iter_mut() {
                    *point = Point::new(
                        raw_to.x + (point.x - raw_from.x) * sx,
                        raw_to.y + (point.y - raw_from.y) * sy,
                    );
                }
            }
            Geometry::Text { pos, font_size, width, style, .. } => {
                // The box the handles sit on is the *plate*, which is the glyphs plus
                // `plate_padding_em` on every side (`spec/05` §4.5), so the glyph origin
                // and the wrap width are both inside it by a padding -- at the *new* font
                // size, because the padding is in ems. The first version set `pos` to the
                // box's corner and the plate crept down and right on every resize.
                let (dx, dy) = handle.direction();
                let pad_before = style.plate_padding_em() * *font_size;
                if dx != 0 && dy == 0 {
                    // A side handle sets the wrap width and leaves the glyphs alone.
                    *width = Some((to.width - 2.0 * pad_before).max(1.0));
                    *pos = Point::new(to.x + pad_before, pos.y);
                } else {
                    // A corner or a top/bottom handle scales the glyphs. The *height*
                    // ratio drives it because a font size is a height; scaling by the
                    // width would make a long line shrink when it was made taller.
                    let scale = to.height / from.height;
                    *font_size = (*font_size * scale).clamp(1.0, 1000.0);
                    let pad_after = style.plate_padding_em() * *font_size;
                    if let Some(w) = width.as_mut() {
                        *w = (*w * scale).max(1.0);
                    }
                    *pos = Point::new(to.x + pad_after, to.y + pad_after);
                }
            }
            // Lines and arrows are dragged by their ends, not by a box -- see
            // [`Self::grips`] -- and the background and the crop are the canvas.
            Geometry::Arrow { .. }
            | Geometry::Line { .. }
            | Geometry::Background { .. }
            | Geometry::Crop { .. } => {}
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod resize_tests {
    use super::*;
    use crate::style::Rgba;

    fn style(size: u8) -> Style {
        Style::new(Rgba::new(1.0, 0.0, 0.0, 1.0), size, false)
    }

    fn object(geometry: Geometry) -> Object {
        Object::new(0, 0, style(3), geometry)
    }

    fn close(a: Bounds, b: Bounds) -> bool {
        (a.x - b.x).abs() < 1e-9
            && (a.y - b.y).abs() < 1e-9
            && (a.width - b.width).abs() < 1e-9
            && (a.height - b.height).abs() < 1e-9
    }

    /// `spec/05` §4.1's chrome, per kind: eight handles for a box, two ends for a line,
    /// three for a curved arrow, none for the canvas's own objects.
    #[test]
    fn the_chrome_matches_the_kind() {
        let b = Bounds::new(0.0, 0.0, 100.0, 50.0);
        let p = Point::new(0.0, 0.0);
        let q = Point::new(10.0, 10.0);
        assert_eq!(
            object(Geometry::Rect { bounds: b, filled: false, radius: 0.0 }).grips(),
            Grips::Box,
        );
        assert_eq!(object(Geometry::Line { start: p, end: q }).grips(), Grips::Ends(2));
        assert_eq!(
            object(Geometry::Arrow {
                start: p,
                end: q,
                ctrl: None,
                style: ArrowStyle::Standard,
                head: ArrowHead::default(),
            })
            .grips(),
            Grips::Ends(2),
        );
        assert_eq!(
            object(Geometry::Arrow {
                start: p,
                end: q,
                ctrl: Some(Point::new(5.0, 0.0)),
                style: ArrowStyle::Curved,
                head: ArrowHead::default(),
            })
            .grips(),
            Grips::Ends(3),
            "the curve handle is the third",
        );
        assert_eq!(
            object(Geometry::Background { params: BackgroundParams::default(), source: None }).grips(),
            Grips::None,
        );
    }

    /// The invariant every box kind has to keep: after a resize, the box the handles are
    /// on **is** the box that was asked for. Checked through `bounds()` rather than
    /// through each geometry's own field, because the stroke's half-width is exactly the
    /// discrepancy this is here to catch.
    #[test]
    fn resizing_puts_the_visible_box_where_it_was_asked_for() {
        let before = Bounds::new(100.0, 100.0, 200.0, 100.0);
        let after = Bounds::new(50.0, 60.0, 400.0, 300.0);
        let kinds = [
            Geometry::Rect { bounds: before, filled: false, radius: 0.0 },
            Geometry::Rect { bounds: before, filled: true, radius: 8.0 },
            Geometry::Ellipse { bounds: before },
            Geometry::Spotlight {
                shape: SpotlightShape::Rectangle,
                bounds: before,
                opacity: 0.5,
            },
            Geometry::Redact {
                bounds: before,
                style: RedactStyle::Pixelate,
                intensity: 0.5,
                seed: 1,
            },
            Geometry::Image { bounds: before, file: "a.png".into() },
        ];
        for geometry in kinds {
            let mut o = object(geometry);
            let from = o.bounds();
            o.resize(from, after, Handle::BottomRight);
            assert!(
                close(o.bounds(), after),
                "{:?} landed at {:?} instead of {after:?}",
                o.kind(),
                o.bounds(),
            );
        }
    }

    /// A freehand stroke's points are remapped into the new box, and the *stroke* is
    /// taken out of the arithmetic on both sides -- otherwise a wide highlighter shrinks
    /// away from its own handles.
    #[test]
    fn a_path_is_remapped_into_the_new_box() {
        let mut o = object(Geometry::Path {
            points: vec![Point::new(0.0, 0.0), Point::new(100.0, 0.0), Point::new(100.0, 50.0)],
            smoothing: false,
            highlighter: false,
            band: None,
        });
        let from = o.bounds();
        let after = Bounds::new(200.0, 200.0, from.width * 2.0, from.height * 2.0);
        o.resize(from, after, Handle::BottomRight);
        assert!(close(o.bounds(), after), "{:?}", o.bounds());
        let Geometry::Path { points, .. } = &o.geometry else { panic!("not a path") };
        assert_eq!(points.len(), 3, "no point was added or lost");
        // The corner point is still the corner. Deliberately not "the point box doubled":
        // the *visible* box doubled, and the stroke's half-width is a fixed 1.5 units at
        // both sizes, so the points span 203 where they used to span 100. Asserting the
        // raw box doubled would be asserting that the stroke scales, which it does not --
        // `spec/05` §2's size levels are the only thing that changes a stroke's width.
        let inner = after.inflated(-o.style.stroke_width() / 2.0);
        assert!((points[0].x - inner.x).abs() < 1e-9, "{points:?} vs {inner:?}");
        assert!((points[2].x - (inner.x + inner.width)).abs() < 1e-9, "{points:?}");
        assert!((points[2].y - (inner.y + inner.height)).abs() < 1e-9, "{points:?}");
    }

    /// `spec/05` §4.5: "Dragging a text object's corner handle sets any value in between,
    /// which is why the stored preference is a float such as 37.744."
    #[test]
    fn a_text_corner_handle_scales_the_font_and_a_side_sets_the_wrap() {
        let text = |width| Geometry::Text {
            pos: Point::new(10.0, 20.0),
            text: "hello".into(),
            style: TextStyle::Standard,
            font_size: 24.0,
            align: TextAlign::Start,
            width,
        };

        let mut corner = object(text(None));
        let from = Bounds::new(10.0, 20.0, 100.0, 30.0);
        corner.resize(from, Bounds::new(10.0, 20.0, 150.0, 45.0), Handle::BottomRight);
        let Geometry::Text { font_size, width, .. } = corner.geometry else { panic!() };
        assert!((font_size - 36.0).abs() < 1e-9, "{font_size} is not 24 x 1.5");
        assert_eq!(width, None, "a corner drag does not invent a wrap width");

        let mut side = object(text(None));
        side.resize(from, Bounds::new(10.0, 20.0, 250.0, 30.0), Handle::Right);
        let Geometry::Text { font_size, width, .. } = side.geometry else { panic!() };
        assert!((font_size - 24.0).abs() < 1e-9, "a side drag leaves the glyphs alone");
        assert_eq!(width, Some(250.0), "and turns the run into a wrapped block");
    }

    /// A badge is a disc, so the smaller extent decides and a corner drag cannot leave an
    /// ellipse behind.
    #[test]
    fn a_counter_stays_round() {
        let mut o = object(Geometry::Counter {
            center: Point::new(50.0, 50.0),
            number: 1,
            style: CounterStyle::Arabic,
            radius: 10.0,
        });
        let from = o.bounds();
        o.resize(from, Bounds::new(0.0, 0.0, 100.0, 40.0), Handle::BottomRight);
        let Geometry::Counter { center, radius, .. } = o.geometry else { panic!() };
        assert!((radius - 20.0).abs() < 1e-9, "the smaller extent decides: {radius}");
        assert!((center.x - 50.0).abs() < 1e-9 && (center.y - 20.0).abs() < 1e-9);
    }

    /// A plated label's handles are on its plate; its glyphs start a padding inside.
    #[test]
    fn a_plated_labels_glyphs_stay_a_padding_inside_the_box_it_was_resized_to() {
        let mut o = object(Geometry::Text {
            pos: Point::new(100.0, 100.0),
            text: "hi".to_owned(),
            style: TextStyle::RoundedBox,
            font_size: 20.0,
            align: TextAlign::Start,
            width: None,
        });
        // The plate is the glyphs plus 0.4 em (8 units) on every side: pretend the glyph
        // box measured 40 x 24, so the plate is 56 x 40 at (92, 92).
        let plate = Bounds::new(92.0, 92.0, 56.0, 40.0);
        o.resize(plate, Bounds::new(92.0, 92.0, 56.0, 80.0), Handle::BottomRight);
        let Geometry::Text { pos, font_size, .. } = o.geometry else { panic!("text") };
        assert!((font_size - 40.0).abs() < 1e-9, "doubled height doubles the size: {font_size}");
        // The new padding is 16, so the glyphs start at 92 + 16 on both axes.
        assert!((pos.x - 108.0).abs() < 1e-9 && (pos.y - 108.0).abs() < 1e-9, "{pos:?}");

        let mut o = object(Geometry::Text {
            pos: Point::new(100.0, 100.0),
            text: "hi".to_owned(),
            style: TextStyle::RoundedBox,
            font_size: 20.0,
            align: TextAlign::Start,
            width: None,
        });
        o.resize(plate, Bounds::new(92.0, 92.0, 116.0, 40.0), Handle::Right);
        let Geometry::Text { pos, width, .. } = o.geometry else { panic!("text") };
        assert_eq!(width, Some(100.0), "the wrap width is the plate's minus two paddings");
        assert_eq!(pos, Point::new(100.0, 100.0), "a side handle moves nothing else");
    }

    /// Lines and arrows are dragged by their ends. A box resize must leave them alone
    /// rather than doing something plausible-looking to a shape that has no box.
    #[test]
    fn ends_objects_ignore_a_box_resize_and_move_by_their_points() {
        let mut o = object(Geometry::Arrow {
            start: Point::new(0.0, 0.0),
            end: Point::new(100.0, 100.0),
            ctrl: Some(Point::new(20.0, 80.0)),
            style: ArrowStyle::Curved,
            head: ArrowHead::default(),
        });
        let untouched = o.clone();
        o.resize(o.bounds(), Bounds::new(500.0, 500.0, 50.0, 50.0), Handle::TopLeft);
        assert_eq!(o.geometry, untouched.geometry, "a box resize is not for an arrow");

        assert_eq!(o.points().len(), 3);
        assert!(o.move_point(2, Point::new(60.0, 10.0)));
        assert_eq!(o.points()[2], Point::new(60.0, 10.0));
        assert!(!o.move_point(3, Point::new(0.0, 0.0)), "there is no fourth point");

        let mut line = object(Geometry::Line {
            start: Point::new(0.0, 0.0),
            end: Point::new(10.0, 0.0),
        });
        assert!(line.move_point(1, Point::new(10.0, 40.0)));
        assert_eq!(line.points(), vec![Point::new(0.0, 0.0), Point::new(10.0, 40.0)]);
        assert!(!line.move_point(2, Point::new(0.0, 0.0)));
    }

    /// A degenerate "before" box divides by zero if it is trusted. It happens: a stroke
    /// drawn as a perfectly straight horizontal line has no height at all.
    #[test]
    fn a_flat_box_is_declined_rather_than_dividing_by_zero() {
        let mut o = object(Geometry::Path {
            points: vec![Point::new(0.0, 0.0), Point::new(100.0, 0.0)],
            smoothing: false,
            highlighter: false,
            band: None,
        });
        let untouched = o.clone();
        o.resize(Bounds::new(0.0, 0.0, 100.0, 0.0), Bounds::new(0.0, 0.0, 200.0, 50.0), Handle::BottomRight);
        assert_eq!(o.geometry, untouched.geometry);
        // And an empty target is survivable: the points collapse, they do not become NaN.
        let from = untouched.bounds();
        o.resize(from, Bounds::new(0.0, 0.0, 0.0, 0.0), Handle::BottomRight);
        let Geometry::Path { points, .. } = &o.geometry else { panic!() };
        assert!(points.iter().all(|p| p.x.is_finite() && p.y.is_finite()), "{points:?}");
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::style::Rgba;

    fn style(size: u8) -> Style {
        Style::new(Rgba::new(1.0, 0.0, 0.0, 1.0), size, false)
    }

    fn line(from: (f64, f64), to: (f64, f64), size: u8) -> Object {
        Object::new(
            0,
            0,
            style(size),
            Geometry::Line { start: Point::new(from.0, from.1), end: Point::new(to.0, to.1) },
        )
    }

    #[test]
    fn every_geometry_reports_its_own_kind() {
        // The point of the test is that the match is exhaustive and not shifted by one:
        // an arm returning its neighbour's kind would break the options row silently.
        let cases = [
            (Geometry::Line { start: Point::new(0.0, 0.0), end: Point::new(1.0, 1.0) },
             ObjectKind::Line),
            (Geometry::Ellipse { bounds: Bounds::new(0.0, 0.0, 1.0, 1.0) }, ObjectKind::Ellipse),
            (Geometry::Counter { center: Point::new(0.0, 0.0), number: 1,
                                 style: CounterStyle::Arabic, radius: 12.0 },
             ObjectKind::Counter),
            (Geometry::Background { params: BackgroundParams::default(), source: None },
             ObjectKind::Background),
        ];
        for (geometry, kind) in cases {
            assert_eq!(Object::new(0, 0, style(3), geometry).kind(), kind);
        }
    }

    /// Half a stroke falls outside the geometry, and leaving it out makes a thick line's
    /// selection outline cut through the line.
    #[test]
    fn bounds_include_the_stroke() {
        let thin = line((0.0, 0.0), (100.0, 0.0), 1).bounds();
        let thick = line((0.0, 0.0), (100.0, 0.0), 6).bounds();
        assert!(thick.height > thin.height, "a heavier stroke reaches further");
        assert!((thin.height - 1.0).abs() < 1e-9, "level 1 is a 1 px stroke, so half each side");
    }

    #[test]
    fn a_path_bounds_every_point_it_has() {
        let path = Object::new(
            0,
            0,
            style(1),
            Geometry::Path {
                points: vec![Point::new(10.0, 10.0), Point::new(50.0, 90.0), Point::new(30.0, 5.0)],
                smoothing: true,
                highlighter: false,
                band: None,
            },
        );
        let b = path.bounds();
        assert!(b.contains(Point::new(10.0, 10.0)));
        assert!(b.contains(Point::new(50.0, 90.0)));
        assert!(b.contains(Point::new(30.0, 5.0)));
    }

    /// A pencil stroke that never moved: one point, and no panic.
    #[test]
    fn an_empty_path_has_bounds_rather_than_a_panic() {
        let empty = Object::new(
            0,
            0,
            style(1),
            Geometry::Path { points: vec![], smoothing: false, highlighter: false, band: None },
        );
        assert!(empty.bounds().is_empty());
    }

    /// "Move" has to mean the same thing for every kind, or a multi-object drag shears.
    #[test]
    fn translating_moves_every_part_of_every_kind() {
        let mut arrow = Object::new(
            0,
            0,
            style(3),
            Geometry::Arrow {
                start: Point::new(0.0, 0.0),
                end: Point::new(10.0, 10.0),
                ctrl: Some(Point::new(5.0, 0.0)),
                style: ArrowStyle::Curved,
                head: ArrowHead::default(),
            },
        );
        arrow.translate(100.0, 50.0);
        let Geometry::Arrow { start, end, ctrl, .. } = arrow.geometry else {
            panic!("still an arrow")
        };
        assert_eq!(start, Point::new(100.0, 50.0));
        assert_eq!(end, Point::new(110.0, 60.0));
        assert_eq!(ctrl, Some(Point::new(105.0, 50.0)), "the control point moves with it");
    }

    #[test]
    fn translating_a_path_moves_all_of_its_points() {
        let mut path = Object::new(
            0,
            0,
            style(1),
            Geometry::Path {
                points: vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0)],
                smoothing: false,
                highlighter: true,
                band: None,
            },
        );
        path.translate(-5.0, 5.0);
        let Geometry::Path { points, .. } = &path.geometry else { panic!("still a path") };
        assert_eq!(points, &[Point::new(-5.0, 5.0), Point::new(5.0, 5.0)]);
    }

    /// `spec/05` §5.1's shape, and the reason this is asserted rather than assumed: the
    /// project format is this struct, so a renamed field is a project that will not open.
    #[test]
    fn an_object_serialises_as_the_spec_writes_it() {
        let object = Object {
            id: ObjectId::from_ulid(Ulid::nil()),
            z: 12,
            created: 1_725_360_000,
            style: Style::new(Rgba::new(1.0, 0.23, 0.19, 1.0), 3, true),
            locked: false,
            geometry: Geometry::Arrow {
                start: Point::new(1.0, 2.0),
                end: Point::new(3.0, 4.0),
                ctrl: None,
                style: ArrowStyle::Standard,
                head: ArrowHead::default(),
            },
        };
        let json = serde_json::to_value(&object).unwrap();
        // `spec/05` §5.1, key by key: the common fields flat, the geometry nested, and
        // `type` beside them rather than inside either.
        assert_eq!(json["id"], "00000000000000000000000000");
        assert_eq!(json["type"], "arrow");
        assert_eq!(json["z"], 12);
        assert_eq!(json["created"], 1_725_360_000u64);
        assert_eq!(json["color"], serde_json::json!([1.0, 0.23, 0.19, 1.0]));
        assert_eq!(json["size"], 3);
        assert_eq!(json["shadow"], true);
        assert_eq!(json["locked"], false);
        assert_eq!(json["geometry"]["start"], serde_json::json!({"x": 1.0, "y": 2.0}));
        assert_eq!(json["geometry"]["style"], "standard", "the arrow's own style, unshadowed");
        assert!(json["geometry"].get("ctrl").is_none(), "an absent control point is absent");
        assert_eq!(serde_json::from_value::<Object>(json).unwrap(), object);
    }
}

#[cfg(test)]
mod highlighter_bounds_tests {
    use super::*;
    use crate::geometry::Point;
    use crate::style::Style;
    use crate::tool::HIGHLIGHTER_WIDTH_FACTOR;

    fn stroke(highlighter: bool) -> Object {
        Object::new(
            0,
            0,
            Style::default(),
            Geometry::Path {
                points: vec![Point::new(0.0, 50.0), Point::new(100.0, 50.0)],
                smoothing: false,
                highlighter,
                band: None,
            },
        )
    }

    #[test]
    fn a_highlighters_bounds_are_three_strokes_tall() {
        // `spec/05` §4.7's "3 x size" is part of the object's extent, not only of the
        // stroke the renderer builds: the selection chrome and the hit test read this.
        let pencil = stroke(false).bounds();
        let marker = stroke(true).bounds();
        let width = Style::default().stroke_width();
        assert!((pencil.height - width).abs() < 1e-9, "pencil: {pencil:?}");
        assert!(
            (marker.height - width * HIGHLIGHTER_WIDTH_FACTOR).abs() < 1e-9,
            "marker: {marker:?}"
        );
        // And it grows in both directions from the path, not downwards from it.
        assert!((marker.center().y - 50.0).abs() < 1e-9, "off-centre: {marker:?}");
    }

    #[test]
    fn a_snapped_stroke_is_as_tall_as_the_line_it_covers() {
        // `spec/05` §4.7's smart highlighter: the band replaces `3 x size` as the width,
        // and the chrome has to agree or a snapped stroke's outline cuts through it.
        let mut snapped = stroke(true);
        if let Geometry::Path { band, .. } = &mut snapped.geometry {
            *band = Some(18.0);
        }
        let bounds = snapped.bounds();
        assert!((bounds.height - 18.0).abs() < 1e-9, "{bounds:?}");
        assert!((bounds.center().y - 50.0).abs() < 1e-9, "off-centre: {bounds:?}");
    }
}
