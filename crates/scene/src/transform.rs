// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §4.12's Resize, Rotate and Flip, as arithmetic on the whole document.
//!
//! > Resize dialog: width/height in px with lock, or percent; resampling Lanczos3;
//! > applies to the base image and scales object coordinates. Rotate 90° left/right, flip
//! > horizontal: transform base and objects.
//!
//! **Nothing here resamples anything**, and that is the decision (`docs/decisions.md`
//! D91). The base image is a file plus the rectangle it occupies, and the renderer already
//! draws it into that rectangle; so a resize is a new rectangle and a rotate is a new way
//! of laying the same pixels into one. What that buys is the same thing D52 bought for the
//! crop: the operation is one undoable command, a resize to 40 % and back is the original
//! image rather than the original image resampled twice, and `spec/05` §6's "preview ==
//! export by construction" needs no second code path for a document that has been rotated.
//!
//! So the base carries an [`Orientation`] -- one of the eight ways a rectangle can be laid
//! down -- and everything else in the document is *moved*: every object's coordinates, the
//! canvas, and the rect a §4.13 background laid out. Moving them is what keeps the rest of
//! the editor from having to know this happened. A tool drawing an arrow after a rotate is
//! drawing in the same document space it always was.

use serde::{Deserialize, Serialize};

use crate::background::BackgroundParams;
use crate::command::Command;
use crate::geometry::{Bounds, Point};
use crate::object::{Geometry, Object};
use crate::scene::{Base, Scene};

/// How the base image's pixels lie in the document: `quarter` clockwise turns, then a
/// horizontal mirror.
///
/// The eight elements of the square's symmetry group, in the order the renderer applies
/// them -- mirror first in the texture's own space, then the turns -- which is why
/// composing a *new* operation onto an existing orientation is not simply addition. A
/// mirror applied to a document that is already turned reflects the turn as well, and
/// [`Orientation::mirror`] is that one line of algebra.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Orientation {
    /// 0, 1, 2 or 3 quarter turns clockwise.
    #[serde(default)]
    pub quarter: u8,
    #[serde(default)]
    pub mirrored: bool,
}

impl Orientation {
    pub const UPRIGHT: Self = Self { quarter: 0, mirrored: false };

    /// Whether the base is stored the way it is shown, which is the case worth not writing
    /// to a project file.
    #[must_use]
    pub const fn is_upright(&self) -> bool {
        self.quarter == 0 && !self.mirrored
    }

    /// Whether the turns swap the base image's width and height.
    #[must_use]
    pub const fn swaps_axes(&self) -> bool {
        self.quarter % 2 == 1
    }

    /// This orientation with `turns` more clockwise quarter turns in front of it.
    #[must_use]
    pub const fn turned(self, turns: u8) -> Self {
        Self { quarter: (self.quarter + turns) % 4, mirrored: self.mirrored }
    }

    /// Where a point of the **stored image** lands in the document, given the document's
    /// size in units.
    ///
    /// `spec/05` §4.7's smart highlighter is why this exists: the detector reads the base
    /// image's own pixels, and on a document that has been turned those are not the
    /// document's own axes. Reading the wrong rows would snap a stroke to a line that is
    /// not there, silently -- which is worse than not snapping at all.
    #[must_use]
    pub fn place(self, point: Point, width: f64, height: f64) -> Point {
        // The stored image's rect: the document's, with the axes put back.
        let (tw, _th) = if self.swaps_axes() { (height, width) } else { (width, height) };
        let x = if self.mirrored { tw - point.x } else { point.x };
        let y = point.y;
        match self.quarter {
            1 => Point::new(width - y, x),
            2 => Point::new(width - x, height - y),
            3 => Point::new(y, height - x),
            _ => Point::new(x, y),
        }
    }

    /// The inverse of [`Self::place`]: where a document point is in the stored image.
    #[must_use]
    pub fn unplace(self, point: Point, width: f64, height: f64) -> Point {
        let (tw, _th) = if self.swaps_axes() { (height, width) } else { (width, height) };
        let turned = match self.quarter {
            1 => Point::new(point.y, width - point.x),
            2 => Point::new(width - point.x, height - point.y),
            3 => Point::new(height - point.y, point.x),
            _ => point,
        };
        Point::new(if self.mirrored { tw - turned.x } else { turned.x }, turned.y)
    }

    /// This orientation with a horizontal mirror in front of it.
    ///
    /// `M · R^q · M^m = R^-q · M^(m+1)`: reflecting a turned document turns it the other
    /// way. Getting this wrong is invisible until the user rotates *and then* flips, which
    /// is exactly the sequence nobody tries until the feature ships.
    #[must_use]
    pub const fn mirror(self) -> Self {
        Self { quarter: (4 - self.quarter) % 4, mirrored: !self.mirrored }
    }
}

/// One of `spec/05` §4.12's operations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Transform {
    /// The base image's new size in document units. §4.12's dialog offers it as a width, a
    /// height or a percentage; all three arrive here as the size they mean.
    ///
    /// The **base** and not the canvas, because §4.12 says "applies to the base image" --
    /// and because with §4.13's padding on, the canvas is a number the background tool
    /// derives rather than one the user can set directly.
    Resize { width: f64, height: f64 },
    RotateLeft,
    RotateRight,
    FlipHorizontal,
    /// Not in §4.12, which lists "flip horizontal" only. [P], and one line: a menu with
    /// three of the four is a menu with a hole in it.
    FlipVertical,
}

impl Transform {
    /// §4.12's dialog as a percentage: the same size, scaled.
    #[must_use]
    pub fn percent(base: &Base, percent: f64) -> Self {
        Self::Resize { width: base.width * percent / 100.0, height: base.height * percent / 100.0 }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Resize { .. } => "Resize",
            Self::RotateLeft => "Rotate left",
            Self::RotateRight => "Rotate right",
            Self::FlipHorizontal => "Flip horizontal",
            Self::FlipVertical => "Flip vertical",
        }
    }
}

/// What a [`Transform`] does to a point, and to everything made of points.
///
/// Built once per operation and then applied to the base, the canvas and every object, so
/// none of them can drift from the others: the failure this shape prevents is a rotate
/// that moves the picture and forgets the arrow on it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mapping {
    sx: f64,
    sy: f64,
    /// Clockwise quarter turns, applied after the scale.
    quarter: u8,
    mirrored: bool,
    /// The translation that puts the mapped base image back at the document's origin,
    /// which is the one place [`Base`] can be: it has a size and no position.
    tx: f64,
    ty: f64,
}

impl Mapping {
    /// The mapping a transform implies for this scene, or `None` if it would collapse the
    /// document to nothing.
    #[must_use]
    pub fn of(scene: &Scene, transform: Transform) -> Option<Self> {
        let (bw, bh) = (scene.base.width, scene.base.height);
        if !(bw > 0.0 && bh > 0.0) {
            return None;
        }
        let mut mapping = match transform {
            Transform::Resize { width, height } => {
                if !(width > 0.0 && height > 0.0 && width.is_finite() && height.is_finite()) {
                    return None;
                }
                Self { sx: width / bw, sy: height / bh, ..Self::identity() }
            }
            Transform::RotateRight => Self { quarter: 1, ..Self::identity() },
            Transform::RotateLeft => Self { quarter: 3, ..Self::identity() },
            Transform::FlipHorizontal => Self { mirrored: true, ..Self::identity() },
            // A vertical flip is a half turn and a horizontal one, which is why there is
            // no third bit to store: `R² · M` **is** the vertical mirror.
            Transform::FlipVertical => Self { quarter: 2, mirrored: true, ..Self::identity() },
        };
        // The base image always sits at the origin, so whatever the linear part did to its
        // rect has to be translated back.
        let a = mapping.linear(Point::new(0.0, 0.0));
        let b = mapping.linear(Point::new(bw, bh));
        mapping.tx = -a.x.min(b.x);
        mapping.ty = -a.y.min(b.y);
        Some(mapping)
    }

    const fn identity() -> Self {
        Self { sx: 1.0, sy: 1.0, quarter: 0, mirrored: false, tx: 0.0, ty: 0.0 }
    }

    fn linear(self, point: Point) -> Point {
        let (x, y) = (point.x * self.sx, point.y * self.sy);
        let (x, y) = match self.quarter {
            1 => (-y, x),
            2 => (-x, -y),
            3 => (y, -x),
            _ => (x, y),
        };
        Point::new(if self.mirrored { -x } else { x }, y)
    }

    /// Where a document point ends up.
    #[must_use]
    pub fn point(self, point: Point) -> Point {
        let moved = self.linear(point);
        Point::new(moved.x + self.tx, moved.y + self.ty)
    }

    /// Where a rectangle ends up. Axis-aligned throughout, so its two opposite corners are
    /// the whole answer.
    #[must_use]
    pub fn bounds(self, bounds: Bounds) -> Bounds {
        let r = bounds.normalised();
        Bounds::from_corners(
            self.point(Point::new(r.x, r.y)),
            self.point(Point::new(r.x + r.width, r.y + r.height)),
        )
    }

    /// What a length becomes: the area-preserving mean of the two axes.
    ///
    /// One number for two factors, because the things it is applied to -- a corner radius,
    /// a counter's badge, a font size, the width a snapped highlighter stroke covers -- are
    /// round or square and cannot be anything else. A non-uniform resize is the only case
    /// where the choice shows, and the geometric mean is the one that keeps a badge the
    /// same *area* fraction of the picture it was on.
    #[must_use]
    pub fn length(self) -> f64 {
        (self.sx * self.sy).abs().sqrt()
    }

    /// The base after the transform: the same file, a new rect, a new orientation.
    #[must_use]
    pub fn base(self, base: &Base) -> Base {
        let (width, height) = (base.width * self.sx, base.height * self.sy);
        let (width, height) = if self.quarter % 2 == 1 { (height, width) } else { (width, height) };
        let orientation = if self.mirrored {
            base.orientation.mirror().turned(self.quarter)
        } else {
            base.orientation.turned(self.quarter)
        };
        Base {
            file: base.file.clone(),
            width,
            height,
            // Pixels per document unit, and the pixels did not change: a document scaled
            // to half its size has twice the detail per unit, which is what a redaction's
            // rasteriser reads to decide how finely to render.
            scale: base.scale / self.length().max(f64::EPSILON),
            orientation,
        }
    }

    /// Moves one object. Answers whether anything about it changed.
    pub fn apply(self, object: &mut Object) -> bool {
        let before = object.clone();
        let move_point = |p: &mut Point| *p = self.point(*p);
        let move_bounds = |b: &mut Bounds| *b = self.bounds(*b);
        let length = self.length();
        match &mut object.geometry {
            Geometry::Arrow { start, end, ctrl, .. } => {
                move_point(start);
                move_point(end);
                if let Some(c) = ctrl {
                    move_point(c);
                }
            }
            Geometry::Line { start, end } => {
                move_point(start);
                move_point(end);
            }
            Geometry::Rect { bounds, radius, .. } => {
                move_bounds(bounds);
                *radius *= length;
            }
            Geometry::Ellipse { bounds }
            | Geometry::Spotlight { bounds, .. }
            | Geometry::Redact { bounds, .. }
            | Geometry::Image { bounds, .. }
            | Geometry::Crop { canvas_rect: bounds } => move_bounds(bounds),
            // The position moves and the *box* follows it: a text run's origin is its
            // top-left corner, which is a different corner after a turn.
            Geometry::Text { pos, font_size, width, text, .. } => {
                let lines = text.lines().count().max(1) as f64;
                let w = width.unwrap_or(*font_size * 0.6 * 20.0);
                let box_before = Bounds::new(pos.x, pos.y, w, lines * *font_size * 1.2);
                let moved = self.bounds(box_before);
                *pos = Point::new(moved.x, moved.y);
                *font_size *= length;
                if let Some(width) = width {
                    *width = (*width * length).max(f64::EPSILON);
                }
            }
            Geometry::Path { points, band, .. } => {
                points.iter_mut().for_each(move_point);
                if let Some(band) = band {
                    *band *= length;
                }
            }
            Geometry::Counter { center, radius, .. } => {
                move_point(center);
                *radius *= length;
            }
            // §4.13's background is the canvas, so it has no rect of its own -- but it
            // remembers the one it laid out (D89), and that one is in the base image's
            // coordinates and moves with everything else.
            Geometry::Background { params, source } => {
                if let Some(source) = source {
                    move_bounds(source);
                }
                *params = self.params(params);
            }
        }
        *object != before
    }

    /// §4.13's parameters after the transform.
    ///
    /// The lengths scale, the ratio turns over with the page, and the 3 × 3 alignment
    /// follows the corner it was pointing at -- a picture in the top-left of a landscape
    /// canvas is in the top-right after a clockwise turn, and a user who rotates a
    /// composed background expects the composition to rotate with it.
    #[must_use]
    pub fn params(self, params: &BackgroundParams) -> BackgroundParams {
        let length = self.length();
        BackgroundParams {
            background: params.background.clone(),
            padding: params.padding * length,
            inset: params.inset * length,
            corner_radius: params.corner_radius,
            ratio: params.ratio.map(|r| {
                if self.quarter % 2 == 1 {
                    crate::background::Ratio::new(r.height, r.width)
                } else {
                    r
                }
            }),
            alignment: self.alignment(params.alignment),
            auto_balance: params.auto_balance,
            shadow_intensity: params.shadow_intensity,
        }
    }

    /// Where a 3 × 3 alignment index lands.
    fn alignment(self, index: u8) -> u8 {
        let index = index.min(8);
        let (mut row, mut col) = (index / 3, index % 3);
        for _ in 0..self.quarter {
            let was = (row, col);
            row = was.1;
            col = 2 - was.0;
        }
        if self.mirrored {
            col = 2 - col;
        }
        row * 3 + col
    }

    /// The canvas after the transform.
    ///
    /// Two answers, because the canvas has two owners. With a §4.13 background on, the
    /// canvas is *derived* -- it is whatever `background::layout` says the padded,
    /// inset, ratio-grown picture needs -- so it is recomputed from the moved parameters.
    /// Without one, or with the plain fill a crop beyond the image leaves behind (D52),
    /// the canvas is a rect the user dragged and it simply moves.
    #[must_use]
    pub fn canvas(self, scene: &Scene) -> Bounds {
        if let Some((source, params)) = scene.background_source()
            && !params.is_plain()
        {
            let moved = self.params(params);
            return crate::background::layout(self.bounds(source), &moved).canvas;
        }
        self.bounds(scene.canvas)
    }
}

/// `spec/05` §4.12 as one undoable step (`spec/05` §5.2).
///
/// A [`Command::Batch`] of the base, every object that moved, and the canvas -- so one
/// Ctrl+Z puts a rotated document back, and `spec/05` §11 item 4's byte-identical
/// `objects.json` holds because every member carries its own `before`.
///
/// `None` when the transform would change nothing, which is what a resize to the size the
/// document already is asks for.
#[must_use]
pub fn plan(scene: &Scene, transform: Transform) -> Option<Command> {
    let mapping = Mapping::of(scene, transform)?;
    let mut commands = Vec::new();

    let after = mapping.base(&scene.base);
    if after != scene.base {
        commands.push(Command::BaseChange { before: scene.base.clone(), after });
    }
    for object in scene.objects() {
        let mut moved = object.clone();
        if mapping.apply(&mut moved) {
            commands.push(Command::Change {
                id: object.id,
                before: Box::new(object.clone()),
                after: Box::new(moved),
            });
        }
    }
    let canvas = mapping.canvas(scene);
    if canvas != scene.canvas {
        commands.push(Command::CanvasChange { before: scene.canvas, after: canvas });
    }
    (!commands.is_empty()).then_some(Command::Batch(commands))
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::object::Geometry;
    use crate::style::Style;

    fn scene() -> Scene {
        Scene::new(Base::new("base.png", 800.0, 600.0, 1.0))
    }

    fn rect(scene: &mut Scene, bounds: Bounds) -> crate::object::ObjectId {
        let object = Object::new(
            0,
            0,
            Style::default(),
            Geometry::Rect { bounds, filled: false, radius: 6.0 },
        );
        let id = object.id;
        scene.add(object);
        id
    }

    fn apply(scene: &mut Scene, transform: Transform) {
        let command = plan(scene, transform).expect("nothing to do");
        let mut history = crate::command::History::new();
        assert!(history.apply(scene, command), "the scene refused the transform");
    }

    #[test]
    fn a_resize_scales_the_base_the_canvas_and_the_objects() {
        let mut scene = scene();
        let id = rect(&mut scene, Bounds::new(100.0, 200.0, 300.0, 100.0));
        apply(&mut scene, Transform::Resize { width: 400.0, height: 300.0 });
        assert_eq!((scene.base.width, scene.base.height), (400.0, 300.0));
        assert_eq!(scene.canvas, Bounds::new(0.0, 0.0, 400.0, 300.0));
        let Geometry::Rect { bounds, radius, .. } = scene.get(id).expect("gone").geometry else {
            panic!("not a rect")
        };
        assert_eq!(bounds, Bounds::new(50.0, 100.0, 150.0, 50.0));
        assert!((radius - 3.0).abs() < 1e-9, "the corner radius did not follow: {radius}");
    }

    #[test]
    fn a_resize_keeps_the_pixels_and_says_so_in_the_scale() {
        let mut scene = scene();
        apply(&mut scene, Transform::Resize { width: 400.0, height: 300.0 });
        // Half the units, the same pixels: twice the detail per unit.
        assert!((scene.base.scale - 2.0).abs() < 1e-9, "scale: {}", scene.base.scale);
    }

    #[test]
    fn a_quarter_turn_swaps_the_document_and_moves_what_is_on_it() {
        let mut scene = scene();
        // A rect in the top-left corner, ten units in from each edge.
        let id = rect(&mut scene, Bounds::new(10.0, 20.0, 30.0, 40.0));
        apply(&mut scene, Transform::RotateRight);
        assert_eq!((scene.base.width, scene.base.height), (600.0, 800.0));
        assert_eq!(scene.canvas, Bounds::new(0.0, 0.0, 600.0, 800.0));
        let Geometry::Rect { bounds, .. } = scene.get(id).expect("gone").geometry else {
            panic!("not a rect")
        };
        // Clockwise: the top-left corner becomes the top-right, so x is measured from the
        // far edge and the width and height swap.
        assert_eq!(bounds, Bounds::new(600.0 - 60.0, 10.0, 40.0, 30.0));
        assert_eq!(scene.base.orientation, Orientation { quarter: 1, mirrored: false });
    }

    #[test]
    fn four_turns_are_where_you_started() {
        let mut scene = scene();
        let id = rect(&mut scene, Bounds::new(10.0, 20.0, 30.0, 40.0));
        let was = scene.get(id).cloned().expect("gone");
        for _ in 0..4 {
            apply(&mut scene, Transform::RotateRight);
        }
        assert_eq!(scene.get(id), Some(&was));
        assert_eq!(scene.base.orientation, Orientation::UPRIGHT);
        assert_eq!((scene.base.width, scene.base.height), (800.0, 600.0));
    }

    #[test]
    fn a_flip_is_its_own_undo() {
        let mut scene = scene();
        let id = rect(&mut scene, Bounds::new(10.0, 20.0, 30.0, 40.0));
        let was = scene.get(id).cloned().expect("gone");
        apply(&mut scene, Transform::FlipHorizontal);
        let Geometry::Rect { bounds, .. } = scene.get(id).expect("gone").geometry else {
            panic!("not a rect")
        };
        assert_eq!(bounds, Bounds::new(800.0 - 40.0, 20.0, 30.0, 40.0));
        apply(&mut scene, Transform::FlipHorizontal);
        assert_eq!(scene.get(id), Some(&was));
    }

    #[test]
    fn a_mirror_after_a_turn_reflects_the_turn() {
        // The algebra `Orientation::mirror` is about, stated as the group it belongs to:
        // turning then flipping is not flipping then turning.
        let turned_then_flipped = Orientation::UPRIGHT.turned(1).mirror();
        let flipped_then_turned = Orientation::UPRIGHT.mirror().turned(1);
        assert_ne!(turned_then_flipped, flipped_then_turned);
        assert_eq!(turned_then_flipped, Orientation { quarter: 3, mirrored: true });
    }

    #[test]
    fn placing_a_stored_pixel_and_taking_it_back_is_the_identity() {
        // Every one of the eight, on a rectangle whose sides differ so a swapped axis
        // cannot hide.
        let (width, height) = (800.0, 600.0);
        for quarter in 0..4 {
            for mirrored in [false, true] {
                let orientation = Orientation { quarter, mirrored };
                let (tw, th) =
                    if orientation.swaps_axes() { (height, width) } else { (width, height) };
                for point in [
                    Point::new(0.0, 0.0),
                    Point::new(tw, th),
                    Point::new(tw / 4.0, th / 3.0),
                ] {
                    let placed = orientation.place(point, width, height);
                    assert!(
                        placed.x >= -1e-9 && placed.x <= width + 1e-9,
                        "{orientation:?} put {point:?} at {placed:?}, outside the document"
                    );
                    assert!(placed.y >= -1e-9 && placed.y <= height + 1e-9, "{placed:?}");
                    let back = orientation.unplace(placed, width, height);
                    assert!(
                        (back.x - point.x).abs() < 1e-9 && (back.y - point.y).abs() < 1e-9,
                        "{orientation:?}: {point:?} -> {placed:?} -> {back:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_quarter_turn_puts_the_stored_images_top_left_at_the_documents_top_right() {
        let turned = Orientation { quarter: 1, mirrored: false };
        assert_eq!(turned.place(Point::new(0.0, 0.0), 600.0, 800.0), Point::new(600.0, 0.0));
    }

    #[test]
    fn a_vertical_flip_is_a_half_turn_and_a_horizontal_one() {
        let mut one = scene();
        let a = rect(&mut one, Bounds::new(10.0, 20.0, 30.0, 40.0));
        apply(&mut one, Transform::FlipVertical);

        let mut two = scene();
        let b = rect(&mut two, Bounds::new(10.0, 20.0, 30.0, 40.0));
        apply(&mut two, Transform::RotateRight);
        apply(&mut two, Transform::RotateRight);
        apply(&mut two, Transform::FlipHorizontal);

        assert_eq!(one.get(a).map(|o| o.geometry.clone()), two.get(b).map(|o| o.geometry.clone()));
        assert_eq!(one.base.orientation, two.base.orientation);
    }

    #[test]
    fn undo_puts_a_rotated_document_back_in_one_step() {
        let mut scene = scene();
        let id = rect(&mut scene, Bounds::new(10.0, 20.0, 30.0, 40.0));
        let was = scene.clone();
        let command = plan(&scene, Transform::RotateLeft).expect("nothing to do");
        let mut history = crate::command::History::new();
        assert!(history.apply(&mut scene, command));
        assert_eq!(history.depth(), 1, "a rotate should be one step");
        assert!(history.undo(&mut scene));
        assert_eq!(scene.base, was.base);
        assert_eq!(scene.canvas, was.canvas);
        assert_eq!(scene.get(id), was.get(id));
    }

    #[test]
    fn a_turn_rotates_a_backgrounds_alignment_and_its_ratio() {
        let mut scene = scene();
        let params = BackgroundParams {
            background: crate::background::Background::Gradient { id: 3 },
            padding: 100.0,
            ratio: Some(crate::background::Ratio::new(16, 9)),
            // Top-left.
            alignment: 0,
            ..BackgroundParams::default()
        };
        let object = Object::new(
            -1,
            0,
            Style::default(),
            Geometry::Background { params, source: Some(scene.base.bounds()) },
        );
        let id = object.id;
        scene.add(object);
        scene.canvas = crate::background::layout(
            scene.base.bounds(),
            &BackgroundParams {
                background: crate::background::Background::Gradient { id: 3 },
                padding: 100.0,
                ratio: Some(crate::background::Ratio::new(16, 9)),
                alignment: 0,
                ..BackgroundParams::default()
            },
        )
        .canvas;

        apply(&mut scene, Transform::RotateRight);
        let Geometry::Background { params, source } = &scene.get(id).expect("gone").geometry else {
            panic!("not a background")
        };
        assert_eq!(params.ratio, Some(crate::background::Ratio::new(9, 16)));
        // Top-left became top-right.
        assert_eq!(params.alignment, 2);
        assert_eq!(source.map(Bounds::normalised), Some(Bounds::new(0.0, 0.0, 600.0, 800.0)));
        // The canvas is whatever the moved parameters lay out, which is the invariant the
        // background tool maintains everywhere else.
        let turned = Bounds::new(0.0, 0.0, 600.0, 800.0);
        assert_eq!(scene.canvas, crate::background::layout(turned, params).canvas);
    }

    #[test]
    fn a_resize_to_the_same_size_is_not_an_edit() {
        let scene = scene();
        assert!(plan(&scene, Transform::Resize { width: 800.0, height: 600.0 }).is_none());
    }

    #[test]
    fn a_resize_to_nothing_is_refused() {
        let scene = scene();
        assert!(plan(&scene, Transform::Resize { width: 0.0, height: 600.0 }).is_none());
        assert!(plan(&scene, Transform::Resize { width: f64::NAN, height: 600.0 }).is_none());
    }

    #[test]
    fn a_percentage_is_the_size_it_means() {
        let base = Base::new("base.png", 800.0, 600.0, 1.0);
        assert_eq!(
            Transform::percent(&base, 50.0),
            Transform::Resize { width: 400.0, height: 300.0 }
        );
    }
}
