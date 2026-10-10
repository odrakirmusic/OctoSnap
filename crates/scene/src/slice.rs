// SPDX-License-Identifier: GPL-3.0-or-later

//! `ANN-31` (D183): a band cut out of the picture, and the gap closed.
//!
//! A band runs across the picture, removing rows, or down it, removing columns. What is past
//! it moves back by its width; what is inside it is gone, marks included; a mark that
//! reaches into it is shortened to its edge. Rotating and resizing move the same pixels into
//! a new rectangle (D91), but a cut removes pixels, so the editor writes the base image
//! again without the band and this module says which rows or columns of the *stored* image
//! those are, on whole pixels, and what everything else becomes. [`plan`] makes that one
//! step, so one Ctrl+Z puts the picture and every mark back.

use crate::command::Command;
use crate::counters;
use crate::geometry::{Bounds, Point};
use crate::object::{Geometry, Object, ObjectId};
use crate::redact::Pixels;
use crate::scene::{Base, Scene};

/// The least a band is, in document units: less is a click.
pub const LEAST: f64 = crate::tool::MIN_DRAG;

/// Which way a band runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Across the picture: rows go, and what is below moves up.
    Rows,
    /// Down the picture: columns go, and what is to the right moves left.
    Columns,
}

impl Axis {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rows => "rows",
            Self::Columns => "columns",
        }
    }

    const fn other(self) -> Self {
        match self {
            Self::Rows => Self::Columns,
            Self::Columns => Self::Rows,
        }
    }
}

/// A band of the document, `from` to `to` along `axis`, in document units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slice {
    pub axis: Axis,
    pub from: f64,
    pub to: f64,
}

/// The same band in the stored image: which of its axes, and which rows or columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stored {
    pub axis: Axis,
    pub first: usize,
    pub count: usize,
}

impl Slice {
    /// The band a drag from `a` to `b` describes over a picture `width` by `height`: across
    /// it when the drag went further down than across, and down it otherwise, kept inside
    /// the picture. `None` while it is too thin to be meant, or would take all of it.
    #[must_use]
    pub fn dragged(a: Point, b: Point, width: f64, height: f64) -> Option<Self> {
        let across = (b.y - a.y).abs() >= (b.x - a.x).abs();
        let (axis, a, b, extent) =
            if across { (Axis::Rows, a.y, b.y, height) } else { (Axis::Columns, a.x, b.x, width) };
        let from = a.min(b).clamp(0.0, extent);
        let to = a.max(b).clamp(0.0, extent);
        (to - from >= LEAST && to - from < extent).then_some(Self { axis, from, to })
    }

    /// How much the band takes away.
    #[must_use]
    pub fn width(self) -> f64 {
        self.to - self.from
    }

    /// The band over the picture, to draw: as long as the picture is, along the other axis.
    #[must_use]
    pub fn rect(self, base: &Base) -> Bounds {
        match self.axis {
            Axis::Rows => Bounds::new(0.0, self.from, base.width, self.width()),
            Axis::Columns => Bounds::new(self.from, 0.0, self.width(), base.height),
        }
    }

    /// Where a coordinate along the axis goes: unchanged before the band, onto its start
    /// inside it, back by the band's width after it.
    fn along(self, value: f64) -> f64 {
        if value <= self.from {
            value
        } else if value < self.to {
            self.from
        } else {
            value - self.width()
        }
    }

    /// Where a document point goes.
    #[must_use]
    pub fn point(self, point: Point) -> Point {
        match self.axis {
            Axis::Rows => Point::new(point.x, self.along(point.y)),
            Axis::Columns => Point::new(self.along(point.x), point.y),
        }
    }

    /// Where a rectangle goes: its corners do, so one that reaches into the band ends at
    /// its edge, and one across it is shorter by its width.
    #[must_use]
    pub fn bounds(self, bounds: Bounds) -> Bounds {
        let r = bounds.normalised();
        Bounds::from_corners(
            self.point(Point::new(r.x, r.y)),
            self.point(Point::new(r.x + r.width, r.y + r.height)),
        )
    }

    /// Whether `bounds` lie inside the band: what is on the slice goes with it.
    #[must_use]
    pub fn swallows(self, bounds: Bounds) -> bool {
        let r = bounds.normalised();
        let (low, high) = match self.axis {
            Axis::Rows => (r.y, r.y + r.height),
            Axis::Columns => (r.x, r.x + r.width),
        };
        low >= self.from && high <= self.to
    }

    /// This band on whole pixels of the stored image, `pixels` wide and high: the band to
    /// cut the document by, and the rows or columns of the file it is. Through the base's
    /// orientation, so a picture turned a quarter loses columns of its file for a band
    /// across it. `None` when it is less than a pixel, or all of them.
    #[must_use]
    pub fn on_pixels(self, base: &Base, pixels: (usize, usize)) -> Option<(Self, Stored)> {
        let (width, height) = (base.width, base.height);
        let orientation = base.orientation;
        let swaps = orientation.swaps_axes();
        let stored_axis = if swaps { self.axis.other() } else { self.axis };
        // The stored image's size in document units, and in pixels, along its axis.
        let (stored_width, stored_height) = if swaps { (height, width) } else { (width, height) };
        let (units, total) = match stored_axis {
            Axis::Rows => (stored_height, pixels.1),
            Axis::Columns => (stored_width, pixels.0),
        };
        if units <= 0.0 || total == 0 {
            return None;
        }
        // The stored image's coordinate along its axis, for a document coordinate along the
        // band's.
        let edge = |value: f64| {
            let point = match self.axis {
                Axis::Rows => Point::new(0.0, value),
                Axis::Columns => Point::new(value, 0.0),
            };
            let stored = orientation.unplace(point, width, height);
            match stored_axis {
                Axis::Rows => stored.y,
                Axis::Columns => stored.x,
            }
        };
        let (a, b) = (edge(self.from), edge(self.to));
        #[allow(clippy::cast_precision_loss)]
        let per_unit = total as f64 / units;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let pixel = |stored: f64| ((stored * per_unit).round().max(0.0) as usize).min(total);
        let (first, last) = (pixel(a.min(b)), pixel(a.max(b)));
        let count = last.saturating_sub(first);
        if count == 0 || count >= total {
            return None;
        }
        // Back from whole pixels to the document, so the marks move by exactly what the
        // picture loses.
        #[allow(clippy::cast_precision_loss)]
        let back = |pixel: usize| {
            let stored = pixel as f64 / per_unit;
            let point = match stored_axis {
                Axis::Rows => Point::new(0.0, stored),
                Axis::Columns => Point::new(stored, 0.0),
            };
            let document = orientation.place(point, width, height);
            match self.axis {
                Axis::Rows => document.y,
                Axis::Columns => document.x,
            }
        };
        let (c, d) = (back(first), back(last));
        let snapped = Self { axis: self.axis, from: c.min(d), to: c.max(d) };
        Some((snapped, Stored { axis: stored_axis, first, count }))
    }

    /// The base after the cut: the new file, shorter along the axis, laid down as before.
    #[must_use]
    pub fn base(self, base: &Base, file: String) -> Base {
        let (mut width, mut height) = (base.width, base.height);
        match self.axis {
            Axis::Rows => height = (height - self.width()).max(1.0),
            Axis::Columns => width = (width - self.width()).max(1.0),
        }
        Base { file, width, height, scale: base.scale, orientation: base.orientation }
    }

    /// Moves one object. Answers whether anything about it changed.
    pub fn apply(self, object: &mut Object) -> bool {
        let before = object.clone();
        let point = |p: &mut Point| *p = self.point(*p);
        let bounds = |b: &mut Bounds| *b = self.bounds(*b);
        match &mut object.geometry {
            Geometry::Arrow { start, end, ctrl, .. } => {
                point(start);
                point(end);
                if let Some(c) = ctrl {
                    point(c);
                }
            }
            Geometry::Line { start, end } => {
                point(start);
                point(end);
            }
            Geometry::Rect { bounds: b, .. }
            | Geometry::Ellipse { bounds: b }
            | Geometry::Spotlight { bounds: b, .. }
            | Geometry::Redact { bounds: b, .. }
            | Geometry::Image { bounds: b, .. }
            | Geometry::Crop { canvas_rect: b } => bounds(b),
            Geometry::Text { pos, tail, .. } => {
                point(pos);
                if let Some(tip) = tail {
                    point(tip);
                }
            }
            Geometry::Counter { center, tail, .. } => {
                point(center);
                if let Some(tip) = tail {
                    point(tip);
                }
            }
            Geometry::Path { points, .. } => points.iter_mut().for_each(point),
            // The lens keeps its size; the spot moves with the picture under it.
            Geometry::Magnifier { center, source, .. } => {
                point(center);
                point(source);
            }
            Geometry::Background { source, .. } => {
                if let Some(source) = source {
                    bounds(source);
                }
            }
        }
        *object != before
    }

    /// The canvas after the cut. With a §4.13 background it is laid out again around the
    /// shorter picture; otherwise the rect the user has simply loses the band, and that rect
    /// is in the picture's coordinates, as the band is. A plain colour the panel laid out is
    /// a background like any other: over a crop its canvas is the background's, at the
    /// origin, and the band taken off it as if it were the picture's missed (D191).
    #[must_use]
    pub fn canvas(self, scene: &Scene) -> Bounds {
        if let Some((source, params)) = scene.frame() {
            return crate::background::layout(self.bounds(source), params).canvas;
        }
        self.bounds(scene.canvas)
    }
}

/// The stored image without the band: its rows or columns `first` to `first + count`.
#[must_use]
pub fn cut_pixels(pixels: &Pixels, stored: Stored) -> Pixels {
    let (width, height) = (pixels.width, pixels.height);
    let end = stored.first.saturating_add(stored.count);
    match stored.axis {
        Axis::Rows => {
            let row = width * 4;
            let keep = |y: &usize| *y < stored.first || *y >= end;
            let mut rgba = Vec::with_capacity(row * height.saturating_sub(stored.count));
            for y in (0..height).filter(keep) {
                if let Some(line) = pixels.rgba.get(y * row..(y + 1) * row) {
                    rgba.extend_from_slice(line);
                }
            }
            Pixels { width, height: rgba.len() / row.max(1), rgba }
        }
        Axis::Columns => {
            let kept = width.saturating_sub(stored.count.min(width));
            let mut rgba = Vec::with_capacity(kept * 4 * height);
            for y in 0..height {
                let line = y * width * 4;
                let before = line..line + stored.first.min(width) * 4;
                let after = line + end.min(width) * 4..line + width * 4;
                for span in [before, after] {
                    if let Some(part) = pixels.rgba.get(span) {
                        rgba.extend_from_slice(part);
                    }
                }
            }
            Pixels { width: kept, height, rgba }
        }
    }
}

/// The cut as one step: the base, every mark that moved, every mark on the band removed,
/// the badges after them numbered on (`ANN-10`), and the canvas.
#[must_use]
pub fn plan(scene: &Scene, slice: Slice, file: String) -> Option<Command> {
    if slice.width() <= 0.0 {
        return None;
    }
    let after = slice.base(&scene.base, file);
    let mut commands = vec![Command::BaseChange { before: scene.base.clone(), after }];
    let gone: Vec<ObjectId> = scene
        .objects()
        .iter()
        .filter(|o| {
            !matches!(o.geometry, Geometry::Background { .. } | Geometry::Crop { .. })
                && slice.swallows(o.bounds())
        })
        .map(|o| o.id)
        .collect();
    let renumbered = counters::closed_up(scene, &gone);
    for object in scene.objects() {
        if gone.contains(&object.id) {
            commands.push(Command::Remove(object.clone()));
            continue;
        }
        let mut moved = object.clone();
        let mut changed = slice.apply(&mut moved);
        if let Some(number) =
            renumbered.iter().find(|o| o.id == object.id).and_then(counters::number_of)
            && let Geometry::Counter { number: n, .. } = &mut moved.geometry
        {
            *n = number;
            changed = true;
        }
        if changed {
            commands.push(Command::Change {
                id: object.id,
                before: Box::new(object.clone()),
                after: Box::new(moved),
            });
        }
    }
    let canvas = slice.canvas(scene);
    if canvas != scene.canvas {
        commands.push(Command::CanvasChange { before: scene.canvas, after: canvas });
    }
    Some(Command::Batch(commands))
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "a failing assertion should panic")]
mod tests {
    use super::*;
    use crate::style::{CounterStyle, Style};
    use crate::transform::Orientation;

    fn base(width: f64, height: f64) -> Base {
        Base::new("x.png", width, height, 1.0)
    }

    #[test]
    fn a_drag_down_is_a_band_across_and_a_drag_across_is_a_band_down() {
        let rows = Slice::dragged(Point::new(10.0, 50.0), Point::new(14.0, 20.0), 200.0, 100.0)
            .expect("a band");
        assert_eq!(rows, Slice { axis: Axis::Rows, from: 20.0, to: 50.0 });
        let columns = Slice::dragged(Point::new(10.0, 50.0), Point::new(90.0, 60.0), 200.0, 100.0)
            .expect("a band");
        assert_eq!(columns, Slice { axis: Axis::Columns, from: 10.0, to: 90.0 });
        // Kept inside the picture; a click is nothing, and so is all of it.
        let kept = Slice::dragged(Point::new(5.0, -40.0), Point::new(5.0, 30.0), 200.0, 100.0)
            .expect("a band");
        assert_eq!((kept.from, kept.to), (0.0, 30.0));
        assert!(Slice::dragged(Point::new(5.0, 5.0), Point::new(6.0, 8.0), 200.0, 100.0).is_none());
        assert!(
            Slice::dragged(Point::new(5.0, -10.0), Point::new(5.0, 140.0), 200.0, 100.0).is_none()
        );
    }

    #[test]
    fn what_is_past_the_band_moves_back_and_what_is_in_it_lands_on_its_edge() {
        let slice = Slice { axis: Axis::Rows, from: 40.0, to: 60.0 };
        assert_eq!(slice.point(Point::new(3.0, 30.0)), Point::new(3.0, 30.0));
        assert_eq!(slice.point(Point::new(3.0, 50.0)), Point::new(3.0, 40.0));
        assert_eq!(slice.point(Point::new(3.0, 90.0)), Point::new(3.0, 70.0));
        // A box across the band is shorter by it.
        assert_eq!(
            slice.bounds(Bounds::new(0.0, 30.0, 10.0, 40.0)),
            Bounds::new(0.0, 30.0, 10.0, 20.0)
        );
        assert!(slice.swallows(Bounds::new(0.0, 45.0, 10.0, 10.0)));
        assert!(!slice.swallows(Bounds::new(0.0, 35.0, 10.0, 10.0)));
    }

    #[test]
    fn the_band_is_cut_on_whole_pixels_of_the_stored_image() {
        // Two pixels a unit: 10.2 to 20.6 units is pixels 20 to 41, which is 10 to 20.5.
        let base = Base::new("x.png", 100.0, 50.0, 2.0);
        let slice = Slice { axis: Axis::Rows, from: 10.2, to: 20.6 };
        let (snapped, stored) = slice.on_pixels(&base, (200, 100)).expect("a band");
        assert_eq!(stored, Stored { axis: Axis::Rows, first: 20, count: 21 });
        assert_eq!((snapped.from, snapped.to), (10.0, 20.5));
    }

    #[test]
    fn a_turned_picture_loses_columns_of_its_file_for_a_band_across_it() {
        // Stored 40 x 100, turned a quarter: the document is 100 x 40.
        let mut base = Base::new("x.png", 100.0, 40.0, 1.0);
        base.orientation = Orientation { quarter: 1, mirrored: false };
        let slice = Slice { axis: Axis::Rows, from: 10.0, to: 15.0 };
        let (snapped, stored) = slice.on_pixels(&base, (40, 100)).expect("a band");
        // Turned clockwise, the file's left edge is the document's top.
        assert_eq!(stored, Stored { axis: Axis::Columns, first: 10, count: 5 });
        assert_eq!((snapped.from, snapped.to), (10.0, 15.0));
        // Turned the other way, it is the bottom.
        base.orientation = Orientation { quarter: 3, mirrored: false };
        let (snapped, stored) = slice.on_pixels(&base, (40, 100)).expect("a band");
        assert_eq!(stored, Stored { axis: Axis::Columns, first: 25, count: 5 });
        assert_eq!((snapped.from, snapped.to), (10.0, 15.0));
    }

    #[test]
    fn rows_and_columns_are_cut_out_of_the_pixels() {
        // 3 x 3, each pixel's red its index.
        let rgba: Vec<u8> = (0..9u8).flat_map(|i| [i, 0, 0, 255]).collect();
        let pixels = Pixels { width: 3, height: 3, rgba };
        let rows = cut_pixels(&pixels, Stored { axis: Axis::Rows, first: 1, count: 1 });
        let reds: Vec<u8> = rows.rgba.chunks(4).map(|p| p[0]).collect();
        assert_eq!((rows.width, rows.height, reds), (3, 2, vec![0, 1, 2, 6, 7, 8]));
        let columns = cut_pixels(&pixels, Stored { axis: Axis::Columns, first: 0, count: 2 });
        let reds: Vec<u8> = columns.rgba.chunks(4).map(|p| p[0]).collect();
        assert_eq!((columns.width, columns.height, reds), (1, 3, vec![2, 5, 8]));
    }

    #[test]
    fn the_plan_is_one_step_that_moves_removes_and_renumbers() {
        let mut scene = Scene::new(base(200.0, 100.0));
        let badge = |z: i32, y: f64, number: u32| {
            Object::new(
                z,
                0,
                Style::default(),
                Geometry::Counter {
                    center: Point::new(50.0, y),
                    number,
                    style: CounterStyle::Arabic,
                    radius: 5.0,
                    tail: None,
                },
            )
        };
        let (one, two, three) = (badge(0, 10.0, 1), badge(1, 50.0, 2), badge(2, 90.0, 3));
        let (two_id, three_id) = (two.id, three.id);
        for b in [one, two, three] {
            scene.add(b);
        }
        let slice = Slice { axis: Axis::Rows, from: 40.0, to: 60.0 };
        let Some(Command::Batch(commands)) = plan(&scene, slice, "cut.png".into()) else {
            panic!("a batch")
        };
        assert!(matches!(&commands[0], Command::BaseChange { after, .. } if after.height == 80.0));
        assert!(commands.iter().any(|c| matches!(c, Command::Remove(o) if o.id == two_id)));
        let moved = commands.iter().find_map(|c| match c {
            Command::Change { id, after, .. } if *id == three_id => Some(after.clone()),
            _ => None,
        });
        let Some(Geometry::Counter { center, number, .. }) = moved.map(|o| o.geometry) else {
            panic!("the third badge changed")
        };
        assert_eq!((center.y, number), (70.0, 2), "moved up by the band, and numbered on");
        assert!(commands.iter().any(|c| matches!(c, Command::CanvasChange { after, .. }
            if *after == Bounds::new(0.0, 0.0, 200.0, 80.0))));
        // Applied and undone, it is the scene it was.
        let mut history = crate::command::History::default();
        let before = scene.clone();
        assert!(history.apply(&mut scene, Command::Batch(commands)));
        assert_eq!(scene.base.height, 80.0);
        assert!(history.undo(&mut scene));
        assert_eq!(scene, before);
    }

    /// D191: a plain colour the panel laid out over a crop, with no margin, is laid out again
    /// around the shorter picture like any other background. Its canvas is in the
    /// background's coordinates, and cut as if it were in the picture's, it kept the band's
    /// height of colour under the picture.
    #[test]
    fn a_plain_colour_over_a_crop_is_laid_out_again_around_the_shorter_picture() {
        use crate::background::{Background, BackgroundParams, layout};
        use crate::style::Rgba;
        let mut scene = Scene::new(base(400.0, 300.0));
        let source = Bounds::new(100.0, 50.0, 200.0, 150.0);
        let params = BackgroundParams {
            background: Background::Color { color: Rgba::new(0.0, 0.5, 0.5, 1.0) },
            padding: 0.0,
            ..BackgroundParams::default()
        };
        scene.canvas = layout(source, &params).canvas;
        let geometry = Geometry::Background { params, source: Some(source), fill: None };
        scene.add(Object::new(0, 0, Style::default(), geometry));
        // Rows 160 to 180 of the capture, which are the crop's 110 to 130.
        let slice = Slice { axis: Axis::Rows, from: 160.0, to: 180.0 };
        let command = plan(&scene, slice, "cut.png".into()).expect("a cut");
        let mut history = crate::command::History::default();
        assert!(history.apply(&mut scene, command));
        assert_eq!(scene.canvas, Bounds::new(0.0, 0.0, 200.0, 130.0));
    }
}
