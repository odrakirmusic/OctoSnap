// SPDX-License-Identifier: GPL-3.0-or-later

//! The document: a base image, a canvas, and the objects on it (`spec/05` §3, §5.3).

use serde::{Deserialize, Serialize};

use crate::background::BackgroundParams;
use crate::geometry::{Bounds, Point};
use crate::object::{Cover, Geometry, Object, ObjectId, ObjectKind};
use crate::style::{Rgba, SpotlightShape};
use crate::transform::Orientation;

/// The capture the editor opened, as the scene refers to it.
///
/// A size and a name rather than pixels: `spec/10` §3 is explicit that "images cross the
/// boundary as files, never as byte arrays", and the same discipline pays off inside the
/// app. This crate reasons about *where* things are, which needs the base image's
/// dimensions and nothing else; the texture is uploaded once by the canvas widget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Base {
    /// `base.png` inside a project (`spec/05` §8), or the spool file when the editor was
    /// opened from a card.
    pub file: String,
    pub width: f64,
    pub height: f64,
    /// The scale the capture was taken at, so `spec/05` §11 item 2's "export at 1x and
    /// 2x" has a document scale to be relative to.
    pub scale: f64,
    /// `spec/05` §4.12's Rotate and Flip: how the file's pixels lie in the document
    /// (D91). Defaulted on read, so a project written before the menu existed opens the
    /// way it was saved -- upright.
    #[serde(default, skip_serializing_if = "Orientation::is_upright")]
    pub orientation: Orientation,
}

impl Base {
    #[must_use]
    pub fn new(file: impl Into<String>, width: f64, height: f64, scale: f64) -> Self {
        Self { file: file.into(), width, height, scale, orientation: Orientation::UPRIGHT }
    }

    #[must_use]
    pub const fn bounds(&self) -> Bounds {
        Bounds::new(0.0, 0.0, self.width, self.height)
    }
}

/// The whole editable document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub base: Base,
    /// What the export covers. Starts as the base image's rect and changes with
    /// `spec/05` §4.11's crop -- which may be *larger* than the base, since §11 item 7
    /// asks for "crop beyond the image expands the canvas with the detected colour".
    pub canvas: Bounds,
    objects: Vec<Object>,
    /// `spec/05` §4.9: counters auto-increment, with a configurable start.
    next_number: u32,
}

/// How far past the canvas, the picture's frame or a spotlight's shape a word's box may
/// reach and still be inside it, in document units: a box the reading drew to the
/// picture's last pixel lands on its edge give or take the rounding of a scale (D188).
const TEXT_EDGE: f64 = 0.5;

impl Scene {
    #[must_use]
    pub fn new(base: Base) -> Self {
        let canvas = base.bounds();
        Self { base, canvas, objects: Vec::new(), next_number: 1 }
    }

    #[must_use]
    pub fn objects(&self) -> &[Object] {
        &self.objects
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    #[must_use]
    pub fn get(&self, id: ObjectId) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }

    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        self.objects.iter_mut().find(|o| o.id == id)
    }

    /// The `z` a new object should take: one above everything.
    ///
    /// `saturating_add` because a session that has added two billion objects should get a
    /// crowded top layer rather than a panic. Counters ignore this entirely -- `spec/05`
    /// §5.3 puts them above everything by kind, not by z.
    #[must_use]
    pub fn top_z(&self) -> i32 {
        self.objects.iter().map(|o| o.z).max().map_or(0, |z| z.saturating_add(1))
    }

    /// Adds an object, keeping the list in `z` order.
    ///
    /// Sorted on insert rather than on read because [`Self::render_order`] is called every
    /// frame and an insert happens once per gesture. The sort is stable, so two objects
    /// at the same z keep the order they were created in -- which is the only sensible
    /// tiebreak and it has to be deterministic for `spec/05` §11 item 4's byte-identical
    /// `objects.json`.
    pub fn add(&mut self, object: Object) {
        self.objects.push(object);
        self.objects.sort_by_key(|o| o.z);
    }

    /// Removes an object and hands it back, so a command can put it there again.
    pub fn remove(&mut self, id: ObjectId) -> Option<Object> {
        let at = self.objects.iter().position(|o| o.id == id)?;
        Some(self.objects.remove(at))
    }

    /// Replaces an object in place, answering with what was there.
    ///
    /// The z is re-sorted, because a `Change` command can carry a z -- `spec/05` §9 binds
    /// `Ctrl+]` and `Ctrl+[` to reordering, and that is a change like any other.
    pub fn replace(&mut self, object: Object) -> Option<Object> {
        let at = self.objects.iter().position(|o| o.id == object.id)?;
        let previous = std::mem::replace(&mut self.objects[at], object);
        self.objects.sort_by_key(|o| o.z);
        Some(previous)
    }

    /// The next counter number, consuming it (`spec/05` §4.9's auto-increment).
    pub fn take_number(&mut self) -> u32 {
        let number = self.next_number;
        self.next_number = self.next_number.saturating_add(1);
        number
    }

    /// `spec/05` §4.9's starting number: the first badge's, while there are badges
    /// (`ANN-10`, D182), and the one stored for the next badge when there are none.
    ///
    /// Read from the badges because they are what a reopened project has -- the stored
    /// number is the session's, and a document whose badges start at 5 still starts at 5
    /// the next time it is opened.
    #[must_use]
    pub fn starting_number(&self) -> u32 {
        crate::counters::first(self).unwrap_or(self.next_number)
    }

    /// `spec/05` §2's "starting number (0 allowed)", for the badges placed when there are
    /// none. The editor moves the badges there are with `counters::started_at`.
    pub fn set_next_number(&mut self, number: u32) {
        self.next_number = number;
    }

    /// The number the next badge placed would carry, without consuming anything.
    ///
    /// Counted from the badges in the scene rather than from a cursor that only moves
    /// forward, and that is what makes undo behave. `spec/05` §4.9 calls the numbering "a
    /// property of the document rather than of each badge", so placing three badges,
    /// undoing one and placing another has to produce a 3 again -- a consuming counter
    /// answers 4 and leaves a gap the user cannot close. It also lets the *preview* ask
    /// the question every frame, which a consuming counter cannot survive.
    ///
    /// [`Self::take_number`] remains for a caller that genuinely wants a monotonic
    /// sequence; nothing in the editor does.
    ///
    /// Counted on from [`Self::starting_number`], so the badges' own numbers decide where
    /// a reopened document carries on (`ANN-10`).
    #[must_use]
    pub fn next_counter_number(&self) -> u32 {
        let placed = self
            .objects
            .iter()
            .filter(|o| matches!(o.kind(), crate::object::ObjectKind::Counter))
            .count();
        self.starting_number().saturating_add(u32::try_from(placed).unwrap_or(u32::MAX))
    }

    /// The order objects are drawn in, bottom to top, exactly as `spec/05` §5.3 lists it.
    ///
    /// Ids rather than draw calls, and in this crate rather than in the canvas widget,
    /// because `spec/05` §6 promises "preview == export by construction". That only holds
    /// if both paths walk the *same* order, and the cheapest way to guarantee that is for
    /// there to be one list that both ask for. A widget and an exporter that each sorted
    /// for themselves would agree until one of them was edited.
    ///
    /// The five groups are not a refinement of `z` -- they override it. A redaction at
    /// z=1 still covers a rectangle at z=50 that happens to sit under it, because
    /// §5.3 puts redactions in group 3 and shapes in group 5; and a counter is on top
    /// however early it was made, which §4.9 states outright ("always stays on top").
    /// Within a group, `z` decides.
    #[must_use]
    pub fn render_order(&self) -> Vec<ObjectId> {
        self.render_list().iter().map(|o| o.id).collect()
    }

    /// [`Self::render_order`]'s objects rather than their ids.
    ///
    /// The same list and the same sort; this is the one a *renderer* wants. Ids were the
    /// only form for a while, and the walk paid for it twice over: [`Self::get`] is a
    /// linear search, so thirty ids cost nine hundred comparisons to turn back into
    /// objects, and the render path asked for the order four times per frame — twice in
    /// the walk and twice more inside the spotlight mask. With `spec/05` §6's budget at
    /// 2 ms that was measurable (D54).
    ///
    /// `render_order` stays, because it is what `spec/05` §6's "one list that both paths
    /// ask for" means when the other path is a project file rather than a renderer, and
    /// it is now defined in terms of this so the two cannot disagree.
    #[must_use]
    pub fn render_list(&self) -> Vec<&Object> {
        let mut ordered = self.stack();
        // A hidden mark is not drawn, nor hit, nor under a redaction (`ANN-30`).
        ordered.retain(|o| o.shown());
        ordered
    }

    /// Every object in the order they are drawn, hidden ones included: what `ANN-30`'s
    /// layers panel lists, so a hidden mark can be shown again.
    #[must_use]
    pub fn stack(&self) -> Vec<&Object> {
        let mut ordered: Vec<&Object> = self.objects.iter().collect();
        // Stable, so `z` ties keep creation order and the output is reproducible.
        ordered.sort_by_key(|o| (render_group(o.kind()), o.z));
        ordered
    }

    /// Every spotlight, whose union is `spec/05` §5.3's single mask layer.
    ///
    /// One layer and not one per object: two overlapping spotlights must brighten one
    /// region, not darken the overlap twice, which is what compositing them separately
    /// would do.
    #[must_use]
    pub fn spotlight_union(&self) -> Option<Bounds> {
        self.objects
            .iter()
            .filter(|o| o.kind() == ObjectKind::Spotlight && o.shown())
            .map(Object::bounds)
            .reduce(Bounds::union)
    }

    /// The objects a redaction at `z` has to rasterise, in draw order.
    ///
    /// `spec/05` §5.3: "redactions rasterize everything below them at their z". So a
    /// redaction is not a filter over the base image -- it is a filter over whatever has
    /// been drawn so far, including an earlier redaction and any image object.
    #[must_use]
    pub fn beneath(&self, id: ObjectId) -> Vec<ObjectId> {
        let Some(subject) = self.get(id) else { return Vec::new() };
        let key = (render_group(subject.kind()), subject.z);
        self.render_order()
            .into_iter()
            .take_while(|other| *other != id)
            .filter(|other| self.get(*other).is_some_and(|o| (render_group(o.kind()), o.z) < key))
            .collect()
    }

    /// `spec/05` §4.13's background object, if the document has one: every background but
    /// the fill a crop past the image leaves ([`Object::is_crop_fill`], §11 item 7). That
    /// fill is the picture's, so the panel shows None over it, and lays its own background
    /// out in its slot (D192, [`crate::background::apply`]).
    ///
    /// One, and the first in render order if a project somehow carries two: a background
    /// *is* the canvas, and two canvases is not a state the panel can show. Answering
    /// with the id as well because every edit the panel makes is a `Change` on it.
    #[must_use]
    pub fn background(&self) -> Option<(ObjectId, &BackgroundParams)> {
        let object = self.panel_background()?;
        match &object.geometry {
            Geometry::Background { params, .. } => Some((object.id, params)),
            _ => None,
        }
    }

    /// The first background in render order that is not the crop's own fill.
    fn panel_background(&self) -> Option<&Object> {
        self.render_list()
            .into_iter()
            .find(|o| o.kind() == ObjectKind::Background && !o.is_crop_fill())
    }

    /// The rect a background lays out and the parameters it lays it out with.
    ///
    /// The source is the document as it was when the tool was first used: the base image,
    /// or the crop if one had already narrowed it.
    #[must_use]
    pub fn background_source(&self) -> Option<(Bounds, &BackgroundParams)> {
        self.render_list().into_iter().find_map(|object| match &object.geometry {
            Geometry::Background { params, source, .. } => {
                Some((source.unwrap_or_else(|| self.base.bounds()), params))
            }
            _ => None,
        })
    }

    /// What a background applied right now would lay out.
    ///
    /// The canvas, which is the base image until `spec/05` §4.11's crop has narrowed it or
    /// widened it past the image, since §4.13 has a background applied after a crop lay out
    /// the crop. Read by the panel when it adds the object, and stored in it from then on so
    /// that a later crop narrows the *export* rather than re-laying out the picture: with one
    /// there, the rect it lays out ([`Self::frame`]). The crop's own fill lays nothing out,
    /// and read as the base image's rect it left what was on the area the crop added outside
    /// the frame (D192).
    #[must_use]
    pub fn picture_rect(&self) -> Bounds {
        self.frame().map_or(self.canvas, |(source, _)| source)
    }

    /// Where the base image and everything drawn on it sits inside the canvas.
    ///
    /// Identity unless a background is moving or shrinking the picture, which is the
    /// whole reason this exists: `spec/05` §4.13's padding and inset make the *canvas*
    /// and the *picture* two coordinate spaces, and every object in this scene is in the
    /// picture's. One function answers it for the renderer, the export and the pointer
    /// alike, because three answers is three chances for a click to land somewhere the
    /// mark did not.
    #[must_use]
    pub fn placement(&self) -> Placement {
        let Some((source, params)) = self.background_source() else { return Placement::NONE };
        let out = crate::background::layout(source, params);
        // The transform takes the *source* rect to where the layout put it, not the base
        // image's corner: a background applied after a crop lays out the crop, and the
        // part of the capture outside it is clipped away by the picture's own frame.
        Placement {
            x: out.image.x - source.x * out.scale,
            y: out.image.y - source.y * out.scale,
            scale: out.scale,
        }
    }

    /// The canvas in the picture's coordinates, which are every object's. What is drawn over
    /// the whole canvas from inside the picture's transform, as a spotlight's scrim is, has
    /// to cover this, and the canvas's own rect is somewhere else once a background has
    /// moved the picture: over a crop, far from it (D189).
    #[must_use]
    pub fn canvas_in_picture(&self) -> Bounds {
        self.placement().invert_bounds(self.canvas)
    }

    /// The background that frames the picture, and the rect it lays out (`spec/05` §4.13):
    /// the picture shows in that rect where the layout puts it, its corners rounded and its
    /// shadow cast, and none of it shows outside. Every background but the crop's own fill
    /// ([`Object::is_crop_fill`]), whose picture is the whole canvas: framed by the base
    /// image's rect, it clipped away what was put on the area the crop added (D191).
    #[must_use]
    pub fn frame(&self) -> Option<(Bounds, &BackgroundParams)> {
        let object = self.panel_background()?;
        match &object.geometry {
            Geometry::Background { params, source, .. } => {
                Some((source.unwrap_or_else(|| self.base.bounds()), params))
            }
            _ => None,
        }
    }

    /// The crop's own fill, where the panel's background keeps it in its slot (D192): the
    /// frame's rect ([`Self::frame`]) and the colour painted over it, under the base image.
    /// The area a crop past the image added is the picture's, and shows inside the frame as
    /// the crop left it.
    #[must_use]
    pub fn frame_fill(&self) -> Option<(Bounds, Rgba)> {
        let object = self.panel_background()?;
        match &object.geometry {
            Geometry::Background { source, fill: Some(fill), .. } => {
                Some((source.unwrap_or_else(|| self.base.bounds()), *fill))
            }
            _ => None,
        }
    }

    /// The picture as the canvas shows it, in the canvas's coordinates: a background's frame
    /// ([`Self::frame`]), or the base image's rect. A crop rect is the canvas's, so this is
    /// where `spec/05` §4.11's crop measures the picture: what its edges snap to, what it
    /// expands past and what its Reset goes back to (D190).
    #[must_use]
    pub fn picture_in_canvas(&self) -> Bounds {
        let picture = self.frame().map_or_else(|| self.base.bounds(), |(source, _)| source);
        self.placement().apply_bounds(picture.normalised())
    }

    /// What the export shows of the picture (D188, [`Visible`]). `measured` gives a mark's
    /// bounds as drawn, which for a label only the app can; it is asked only of a label
    /// with a plate.
    #[must_use]
    pub fn visible(&self, measured: impl Fn(&Object) -> Bounds) -> Visible {
        // A background's frame is the picture's own rect, its corners rounded by its radius.
        let frame = self.frame().and_then(|(source, params)| {
            let out = crate::background::layout(source, params);
            (out.scale > 0.0).then(|| (source.normalised(), out.radius / out.scale))
        });
        let covers = self
            .objects
            .iter()
            .filter_map(|object| object.paints_over(|| measured(object)))
            .collect();
        // The scrim is as opaque as the first spotlight drawn, as the renderer has it, and at
        // full opacity it paints the picture black but for the spotlights' shapes.
        let shown = self.render_list();
        let opaque = shown.iter().find_map(|object| match object.geometry {
            Geometry::Spotlight { opacity, .. } => Some(opacity >= 1.0),
            _ => None,
        });
        let holes = (opaque == Some(true)).then(|| {
            shown
                .iter()
                .filter_map(|object| match object.geometry {
                    Geometry::Spotlight { shape, bounds, .. } => Some((shape, bounds.normalised())),
                    _ => None,
                })
                .collect()
        });
        Visible { canvas: self.canvas_in_picture(), frame, covers, holes }
    }

    /// The document as `objects.json` (`spec/05` §8).
    ///
    /// Pretty-printed, because `spec/05` §11 item 4 compares two of these for byte
    /// identity and a diff a person can read is worth the bytes. Serialisation order is
    /// the scene's own order, which [`Self::add`] keeps sorted -- so the comparison is
    /// about the objects and not about the order a `HashMap` felt like today.
    pub fn to_objects_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.objects)
    }

    /// Replaces every object from `objects.json`.
    pub fn load_objects_json(&mut self, json: &str) -> Result<(), serde_json::Error> {
        let mut objects: Vec<Object> = serde_json::from_str(json)?;
        objects.sort_by_key(|o| o.z);
        self.objects = objects;
        Ok(())
    }
}

/// Where the picture sits inside the canvas (`spec/05` §4.13).
///
/// A translation and a uniform scale, which is all a background can do to the picture:
/// padding moves it, the ratio and the alignment move it, the inset shrinks it. Never a
/// rotation and never two different scales, so a circle stays a circle -- the one thing
/// the background tool must not take away from the annotations already on the capture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
}

impl Placement {
    /// No background, or one that leaves the picture where it was.
    pub const NONE: Self = Self { x: 0.0, y: 0.0, scale: 1.0 };

    /// Whether it does anything at all, which is what lets the renderer skip a transform
    /// and the pointer skip two multiplications on every motion event.
    #[must_use]
    pub fn is_identity(self) -> bool {
        self == Self::NONE
    }

    /// A point in the picture's space, in the canvas's.
    #[must_use]
    pub fn apply(self, point: Point) -> Point {
        Point::new(self.x + point.x * self.scale, self.y + point.y * self.scale)
    }

    /// A point in the canvas's space, back in the picture's.
    ///
    /// The inverse of [`Self::apply`]. A zero scale cannot be inverted and cannot be
    /// produced either -- `layout` keeps a pixel of picture at every inset -- so the
    /// guard answers the origin rather than an infinity that would travel.
    #[must_use]
    pub fn invert(self, point: Point) -> Point {
        if self.scale <= 0.0 {
            return Point::new(0.0, 0.0);
        }
        Point::new((point.x - self.x) / self.scale, (point.y - self.y) / self.scale)
    }

    /// A rectangle in the picture's space, in the canvas's.
    #[must_use]
    pub fn apply_bounds(self, bounds: Bounds) -> Bounds {
        Bounds::new(
            self.x + bounds.x * self.scale,
            self.y + bounds.y * self.scale,
            bounds.width * self.scale,
            bounds.height * self.scale,
        )
    }

    /// A rectangle in the canvas's space, back in the picture's: the inverse of
    /// [`Self::apply_bounds`].
    #[must_use]
    pub fn invert_bounds(self, bounds: Bounds) -> Bounds {
        let r = bounds.normalised();
        Bounds::from_corners(
            self.invert(Point::new(r.x, r.y)),
            self.invert(Point::new(r.x + r.width, r.y + r.height)),
        )
    }
}

/// What the export shows of the picture (D188), in the picture's coordinates: the marks'
/// and those of the words a reading of the picture finds. [`Scene::visible`] works it out,
/// and live text (`ANN-27`) asks it of each word, so that what the export does not show is
/// not marked, copied or followed, as Copy Text, which reads the export, never sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Visible {
    /// The canvas ([`Scene::canvas_in_picture`]).
    canvas: Bounds,
    /// A background's frame (`spec/05` §4.13): the picture's rect and its corners' radius.
    /// Outside it the export shows the background (D189).
    frame: Option<(Bounds, f64)>,
    /// What the marks paint over the picture ([`Object::paints_over`]).
    covers: Vec<Cover>,
    /// The spotlights' shapes, when the scrim around them is opaque and paints the rest of
    /// the picture black.
    holes: Option<Vec<(SpotlightShape, Bounds)>>,
}

impl Visible {
    /// Whether the export hides any of `area`: the area is not wholly on the canvas, inside
    /// the frame's corners and inside one of an opaque spotlight's shapes, or a cover
    /// reaches into it. An edge that meets the area within half a unit leaves it whole,
    /// since the reading's boxes reach the picture's last pixel, and so does a cover that
    /// only touches it.
    #[must_use]
    pub fn hides(&self, area: Bounds) -> bool {
        let corners = area.corners();
        // Every corner inside `room` with its corners rounded to `radius`, give or take the
        // edge's half unit. The shapes are convex, so the corners in are the area in.
        let within = |room: Bounds, radius: f64| {
            let room = room.inflated(TEXT_EDGE);
            let radius = if radius > 0.0 { radius + TEXT_EDGE } else { 0.0 };
            corners.iter().all(|&corner| room.rounded_contains(radius, corner))
        };
        let in_hole = |&(shape, hole): &(SpotlightShape, Bounds)| match shape {
            SpotlightShape::Rectangle => within(hole, 0.0),
            SpotlightShape::Rounded => within(hole, SpotlightShape::ROUNDED_RADIUS),
            SpotlightShape::Ellipse => {
                let room = hole.inflated(TEXT_EDGE);
                corners.iter().all(|&corner| room.ellipse_contains(corner))
            }
        };
        !within(self.canvas, 0.0)
            || self.frame.is_some_and(|(frame, radius)| !within(frame, radius))
            || self.covers.iter().any(|cover| cover.overlaps(area))
            || self.holes.as_ref().is_some_and(|holes| !holes.iter().any(in_hole))
    }
}

impl ObjectKind {
    /// Whether an object of this kind is part of the picture, and so is held by `spec/05`
    /// §4.13's frame -- its rounded corners and the shadow it casts -- or is drawn over the
    /// whole canvas (D166).
    ///
    /// The crop, an inserted image, a redaction and a spotlight are the picture's own: a
    /// stitched image takes its rounded corners (D89), and a redaction or a spotlight is
    /// about what the picture shows. A shape, a line, a text or a counter is an annotation,
    /// and one drawn on a background's margin has to show there: clipped to the picture,
    /// an arrow drawn on the gradient vanished as it was drawn. In render order every
    /// framed kind comes before every annotation, so the renderer closes the frame once,
    /// between the two, without changing what is drawn over what.
    #[must_use]
    pub const fn framed(self) -> bool {
        matches!(render_group(self), 1..=3)
    }
}

/// `spec/05` §5.3's groups, bottom to top. The number is the layer, not a `z`.
pub(crate) const fn render_group(kind: ObjectKind) -> u8 {
    match kind {
        ObjectKind::Background => 0,
        // The base image is group 1 and is not an object; the crop is the transform
        // applied to it, so it belongs with it.
        ObjectKind::Crop => 1,
        ObjectKind::Image | ObjectKind::Redact => 2,
        ObjectKind::Spotlight => 3,
        ObjectKind::Rect
        | ObjectKind::Ellipse
        | ObjectKind::Line
        | ObjectKind::Arrow
        | ObjectKind::Path
        | ObjectKind::Text
        // `ANN-32`'s lens is a mark, over the picture it enlarges.
        | ObjectKind::Magnifier => 4,
        ObjectKind::Counter => 5,
    }
}

/// A capture `width` by `height` as `spec/05` §4.13's panel leaves it: a background laid out
/// around `source` with `params`, and the canvas that lays out. For the tests of what has to
/// keep to the picture on a background.
#[cfg(test)]
pub(crate) fn laid_out(
    width: f64,
    height: f64,
    source: Bounds,
    params: crate::background::BackgroundParams,
) -> Scene {
    let mut scene = Scene::new(Base::new("base.png", width, height, 1.0));
    scene.canvas = crate::background::layout(source, &params).canvas;
    let geometry = Geometry::Background { params, source: Some(source), fill: None };
    scene.add(Object::new(0, 0, crate::style::Style::default(), geometry));
    scene
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::geometry::Point;
    use crate::object::Geometry;
    use crate::style::{CounterStyle, RedactStyle, Rgba, SpotlightShape, Style};

    fn base() -> Base {
        Base::new("base.png", 2560.0, 1440.0, 2.0)
    }

    fn scene() -> Scene {
        Scene::new(base())
    }

    fn style() -> Style {
        Style::new(Rgba::new(1.0, 0.0, 0.0, 1.0), 3, false)
    }

    fn rect_at(z: i32, x: f64) -> Object {
        Object::new(
            z,
            0,
            style(),
            Geometry::Rect {
                bounds: Bounds::new(x, 0.0, 100.0, 100.0),
                filled: false,
                radius: 0.0,
            },
        )
    }

    fn counter_at(z: i32) -> Object {
        Object::new(
            z,
            0,
            style(),
            Geometry::Counter {
                center: Point::new(50.0, 50.0),
                number: 1,
                style: CounterStyle::Arabic,
                radius: 12.0,
                tail: None,
            },
        )
    }

    fn redact_at(z: i32) -> Object {
        Object::new(
            z,
            0,
            style(),
            Geometry::Redact {
                bounds: Bounds::new(0.0, 0.0, 100.0, 100.0),
                style: RedactStyle::Pixelate,
                intensity: 0.5,
                seed: 7,
            },
        )
    }

    fn spotlight_at(z: i32, x: f64) -> Object {
        Object::new(
            z,
            0,
            style(),
            Geometry::Spotlight {
                shape: SpotlightShape::Rectangle,
                bounds: Bounds::new(x, 0.0, 100.0, 100.0),
                opacity: 0.6,
            },
        )
    }

    #[test]
    fn a_new_scene_takes_its_canvas_from_the_base_image() {
        let s = scene();
        assert_eq!(s.canvas, Bounds::new(0.0, 0.0, 2560.0, 1440.0));
        assert!(s.is_empty());
        assert_eq!(s.top_z(), 0, "the first object goes to z 0");
    }

    #[test]
    fn each_new_object_goes_above_the_last() {
        let mut s = scene();
        s.add(rect_at(s.top_z(), 0.0));
        assert_eq!(s.top_z(), 1);
        s.add(rect_at(s.top_z(), 200.0));
        assert_eq!(s.top_z(), 2);
        assert_eq!(s.len(), 2);
    }

    /// `spec/05` §4.9: a counter "always stays on top". Not by z -- by kind, so an early
    /// counter still draws over a late rectangle.
    #[test]
    fn a_counter_drawn_first_still_renders_last() {
        let mut s = scene();
        let counter = counter_at(0);
        let rect = rect_at(100, 0.0);
        let (counter_id, rect_id) = (counter.id, rect.id);
        s.add(counter);
        s.add(rect);
        assert_eq!(s.render_order(), vec![rect_id, counter_id]);
    }

    /// And the same rule from the other end: a redaction with a low z covers a shape with
    /// a high one, because `spec/05` §5.3 puts them in different groups.
    #[test]
    fn a_redaction_covers_shapes_whatever_their_z() {
        let mut s = scene();
        let redact = redact_at(0);
        let rect = rect_at(99, 0.0);
        let (redact_id, rect_id) = (redact.id, rect.id);
        s.add(rect);
        s.add(redact);
        assert_eq!(s.render_order(), vec![redact_id, rect_id]);
    }

    #[test]
    fn within_a_group_z_decides() {
        let mut s = scene();
        let low = rect_at(1, 0.0);
        let high = rect_at(2, 0.0);
        let (low_id, high_id) = (low.id, high.id);
        s.add(high);
        s.add(low);
        assert_eq!(s.render_order(), vec![low_id, high_id]);
    }

    /// The order is `spec/05` §5.3's, all five groups at once, and this is the test that
    /// would catch a group being renumbered.
    #[test]
    fn the_render_order_is_the_spec_order() {
        let mut s = scene();
        let background = Object::new(
            50,
            0,
            style(),
            Geometry::Background {
                params: crate::background::BackgroundParams::default(),
                source: None,
                fill: None,
            },
        );
        let redact = redact_at(40);
        let spotlight = spotlight_at(30, 0.0);
        let rect = rect_at(20, 0.0);
        let counter = counter_at(10);
        let expected = vec![background.id, redact.id, spotlight.id, rect.id, counter.id];
        // Added in exactly the wrong order, and with z descending, so nothing about the
        // result can come from the insertion order or from z alone.
        for object in [counter, rect, spotlight, redact, background] {
            s.add(object);
        }
        assert_eq!(s.render_order(), expected);
    }

    /// D166: the renderer closes the picture's frame once, between the framed objects and
    /// the annotations, so in render order no framed object may come after an annotation,
    /// whatever their z.
    #[test]
    fn the_picture_s_own_objects_render_before_every_annotation() {
        use ObjectKind::{
            Arrow, Counter, Crop, Ellipse, Image, Line, Path, Rect, Redact, Spotlight, Text,
        };
        let mut s = scene();
        let background = Object::new(
            90,
            0,
            style(),
            Geometry::Background {
                params: crate::background::BackgroundParams::default(),
                source: None,
                fill: None,
            },
        );
        let image = Object::new(
            80,
            0,
            style(),
            Geometry::Image {
                bounds: Bounds::new(0.0, 0.0, 10.0, 10.0),
                file: "assets/a.png".into(),
            },
        );
        let objects = [counter_at(1), rect_at(2, 0.0), redact_at(70), spotlight_at(60, 0.0), image];
        for object in objects.into_iter().chain([background]) {
            s.add(object);
        }
        let kinds: Vec<ObjectKind> = s.render_list().iter().map(|o| o.kind()).collect();
        assert_eq!(kinds[0], ObjectKind::Background);
        assert!(!ObjectKind::Background.framed(), "the background is drawn before the frame");
        let rest = &kinds[1..];
        let annotations = rest.iter().position(|k| !k.framed()).expect("an annotation");
        assert!(rest[..annotations].iter().all(|k| k.framed()));
        assert!(rest[annotations..].iter().all(|k| !k.framed()), "{kinds:?}");
        for kind in [Arrow, Line, Rect, Ellipse, Path, Text, Counter] {
            assert!(!kind.framed(), "{kind:?} is drawn over the whole canvas");
        }
        for kind in [Crop, Image, Redact, Spotlight] {
            assert!(kind.framed(), "{kind:?} is the picture's own");
        }
    }

    /// Two overlapping spotlights brighten one region rather than darkening the overlap
    /// twice, which is why `spec/05` §5.3 masks with their union.
    #[test]
    fn spotlights_are_one_mask_layer() {
        let mut s = scene();
        assert_eq!(s.spotlight_union(), None, "no spotlights, no mask");
        s.add(spotlight_at(0, 0.0));
        s.add(spotlight_at(1, 300.0));
        let union = s.spotlight_union().expect("two spotlights");
        assert_eq!(union, Bounds::new(0.0, 0.0, 400.0, 100.0));
    }

    /// `spec/05` §5.3: a redaction rasterises "everything below them at their z" -- which
    /// includes an earlier redaction, and excludes the shapes above it.
    #[test]
    fn a_redaction_rasterises_what_is_below_it_and_no_more() {
        let mut s = scene();
        let first = redact_at(0);
        let second = redact_at(1);
        let above = rect_at(99, 0.0);
        let (first_id, second_id) = (first.id, second.id);
        s.add(first);
        s.add(second);
        s.add(above);
        assert_eq!(s.beneath(second_id), vec![first_id]);
        assert!(s.beneath(first_id).is_empty(), "the lowest redaction has nothing below it");
    }

    #[test]
    fn beneath_an_object_that_is_not_there_is_empty_rather_than_a_panic() {
        assert!(scene().beneath(ObjectId::new()).is_empty());
    }

    /// `spec/05` §11 item 4's byte identity, in the smallest form that can hold: the same
    /// objects written twice produce the same bytes, whatever order they arrived in.
    #[test]
    fn objects_json_round_trips_byte_for_byte() {
        let mut s = scene();
        for z in 0..10 {
            s.add(rect_at(z, f64::from(z) * 10.0));
        }
        let first = s.to_objects_json().unwrap();

        let mut reloaded = scene();
        reloaded.load_objects_json(&first).unwrap();
        assert_eq!(reloaded.to_objects_json().unwrap(), first);
        assert_eq!(reloaded.objects(), s.objects());
    }

    #[test]
    fn counters_number_themselves_and_can_be_told_where_to_start() {
        let mut s = scene();
        assert_eq!(s.take_number(), 1);
        assert_eq!(s.take_number(), 2);
        // `spec/05` §2: "starting number (0 allowed)".
        s.set_next_number(0);
        assert_eq!(s.take_number(), 0);
        assert_eq!(s.take_number(), 1);
    }

    #[test]
    fn replace_hands_back_what_was_there_and_re_sorts() {
        let mut s = scene();
        let original = rect_at(0, 0.0);
        let id = original.id;
        s.add(original.clone());
        s.add(rect_at(1, 200.0));

        let mut raised = original;
        raised.z = 99;
        let previous = s.replace(raised).expect("it is in the scene");
        assert_eq!(previous.z, 0);
        assert_eq!(s.render_order().last(), Some(&id), "it is on top now");
    }

    #[test]
    fn removing_hands_the_object_back_so_it_can_be_put_there_again() {
        let mut s = scene();
        let object = rect_at(0, 0.0);
        let id = object.id;
        s.add(object.clone());
        assert_eq!(s.remove(id), Some(object));
        assert!(s.is_empty());
        assert_eq!(s.remove(id), None, "removing it twice is not an error");
    }

    /// D188: a word the export does not show is not the document's text.
    #[test]
    fn text_off_the_canvas_or_under_what_paints_over_it_is_hidden() {
        let word = Bounds::new(20.0, 20.0, 60.0, 16.0);
        let filled = |x: f64, hidden: bool| {
            let geometry = Geometry::Rect {
                bounds: Bounds::new(x, 0.0, 100.0, 100.0),
                filled: true,
                radius: 0.0,
            };
            let mut object = Object::new(0, 0, style(), geometry);
            object.hidden = hidden;
            object
        };
        let hides = |objects: Vec<Object>, canvas: Option<Bounds>| {
            let mut scene = Scene::new(Base::new("x.png", 400.0, 300.0, 1.0));
            for object in objects {
                scene.add(object);
            }
            if let Some(canvas) = canvas {
                scene.canvas = canvas;
            }
            scene.visible(Object::bounds).hides(word)
        };
        assert!(!hides(vec![], None));
        // An outline over it, and a filled box beside it, leave it readable.
        assert!(!hides(vec![rect_at(0, 0.0), filled(200.0, false)], None));
        // A filled box over it hides it, unless the box is hidden itself.
        assert!(hides(vec![filled(50.0, false)], None));
        assert!(!hides(vec![filled(50.0, true)], None));
        // A redaction hides it, and its eye cannot take that back.
        let mut redaction = redact_at(0);
        redaction.hidden = true;
        assert!(hides(vec![redaction], None));
        // A crop through it hides it; one a rounding short of its edge does not.
        assert!(hides(vec![], Some(Bounds::new(0.0, 0.0, 70.0, 300.0))));
        assert!(!hides(vec![], Some(Bounds::new(0.0, 0.0, 79.75, 300.0))));
    }

    #[test]
    fn a_badge_a_plate_or_a_lens_hides_the_words_under_it() {
        use crate::object::TextAlign;
        use crate::style::TextStyle;

        // The word runs from (20, 20) to (80, 36).
        let word = Bounds::new(20.0, 20.0, 60.0, 16.0);
        let hides = |geometry: Geometry| {
            let mut scene = Scene::new(Base::new("x.png", 400.0, 300.0, 1.0));
            scene.add(Object::new(0, 0, style(), geometry));
            scene.visible(Object::bounds).hides(word)
        };
        let badge = |x: f64, y: f64| Geometry::Counter {
            center: Point::new(x, y),
            number: 1,
            style: CounterStyle::Arabic,
            radius: 12.0,
            tail: None,
        };
        // A badge over the word hides it. One whose square reaches the word's corner and
        // whose disc does not, 14 units off with a radius of 12, leaves it.
        assert!(hides(badge(50.0, 28.0)));
        assert!(!hides(badge(90.0, 46.0)));
        // A label's plate hides what is under it; a label with none leaves it readable.
        let label = |style: TextStyle| Geometry::Text {
            pos: Point::new(30.0, 22.0),
            text: "Note".to_owned(),
            style,
            font_size: 12.0,
            align: TextAlign::default(),
            width: None,
            tail: None,
            ink: None,
            background: None,
        };
        assert!(hides(label(TextStyle::Box)));
        assert!(hides(label(TextStyle::RoundedBox)));
        assert!(!hides(label(TextStyle::Standard)));
        assert!(!hides(label(TextStyle::Outline)));
        // A lens hides what is under it, ring and all, and not what is beside it.
        let lens = |x: f64| Geometry::Magnifier {
            center: Point::new(x, 28.0),
            radius: 20.0,
            source: Point::new(300.0, 200.0),
            zoom: 2.0,
        };
        assert!(hides(lens(50.0)));
        assert!(hides(lens(101.0)));
        assert!(!hides(lens(110.0)));
    }

    /// D188, D189: a background over a crop shows the crop alone, inside its frame's
    /// corners, and the canvas, taken into the picture's coordinates, can narrow it.
    #[test]
    fn a_background_shows_the_words_inside_its_frame_alone() {
        use crate::background::{BackgroundParams, layout};

        // The capture was cropped to (100, 50, 200, 150), then given a margin of 20: the
        // picture sits at (20, 20) on a canvas of 240 x 190.
        let source = Bounds::new(100.0, 50.0, 200.0, 150.0);
        let framed = |corner_radius: f64, canvas: Option<Bounds>| {
            let params = BackgroundParams {
                padding: 20.0,
                corner_radius,
                shadow_intensity: 0.0,
                ..BackgroundParams::default()
            };
            let mut scene = Scene::new(Base::new("x.png", 400.0, 300.0, 1.0));
            scene.canvas = canvas.unwrap_or_else(|| layout(source, &params).canvas);
            let geometry = Geometry::Background { params, source: Some(source), fill: None };
            scene.add(Object::new(0, 0, style(), geometry));
            scene
        };
        let square = framed(0.0, None);
        assert_eq!(square.canvas_in_picture(), Bounds::new(80.0, 30.0, 240.0, 190.0));
        assert_eq!(square.placement().apply_bounds(square.canvas_in_picture()), square.canvas);
        let square = square.visible(Object::bounds);
        // In the crop a word shows. Either side of it, where the capture went on, one does
        // not, though the canvas reaches there: the margin shows the background.
        let inside = Bounds::new(150.0, 100.0, 60.0, 16.0);
        assert!(!square.hides(inside));
        assert!(square.hides(Bounds::new(84.0, 100.0, 12.0, 16.0)));
        assert!(square.hides(Bounds::new(290.0, 100.0, 20.0, 16.0)));
        // A word in the crop's corner shows at square corners, and not at round ones.
        let corner = Bounds::new(102.0, 52.0, 20.0, 10.0);
        assert!(!square.hides(corner));
        let round = framed(0.25, None).visible(Object::bounds);
        assert!(round.hides(corner));
        assert!(!round.hides(inside));
        // A canvas narrower than the background's, as a crop after it makes, ends on the
        // picture at 200.
        let narrowed = framed(0.0, Some(Bounds::new(0.0, 0.0, 120.0, 190.0)));
        let narrowed = narrowed.visible(Object::bounds);
        assert!(!narrowed.hides(Bounds::new(150.0, 100.0, 40.0, 16.0)));
        assert!(narrowed.hides(Bounds::new(190.0, 100.0, 20.0, 16.0)));
    }

    /// D190, D191: a background the panel laid out frames the picture where its layout puts
    /// it, and the canvas shows the picture there. The crop's own fill frames nothing, and
    /// without a frame the picture is the base image, whatever a crop made of the canvas.
    #[test]
    fn a_panel_s_background_frames_the_picture_and_the_crop_s_own_fill_does_not() {
        use crate::background::{BackgroundParams, layout};
        let plain = |scene: &mut Scene| {
            let params = BackgroundParams::plain(Rgba::new(0.5, 0.5, 0.5, 1.0));
            let geometry = Geometry::Background { params, source: None, fill: None };
            scene.add(Object::new(-1, 0, style(), geometry));
        };
        let capture = Bounds::new(0.0, 0.0, 400.0, 300.0);
        // No background: no frame, and the picture is the capture, cropped or not.
        let mut bare = Scene::new(Base::new("x.png", 400.0, 300.0, 1.0));
        assert!(bare.frame().is_none());
        bare.canvas = Bounds::new(100.0, 50.0, 200.0, 150.0);
        assert_eq!(bare.picture_in_canvas(), capture);
        // A crop beyond the image, filled: still no frame.
        bare.canvas = Bounds::new(-50.0, 0.0, 450.0, 300.0);
        plain(&mut bare);
        assert!(bare.frame().is_none());
        assert_eq!(bare.picture_in_canvas(), capture);
        // The panel's, at a margin of 20 over a crop: the frame is the crop, at (20, 20).
        let source = Bounds::new(100.0, 50.0, 200.0, 150.0);
        let params = BackgroundParams { padding: 20.0, ..BackgroundParams::default() };
        let framed = laid_out(400.0, 300.0, source, params.clone());
        assert_eq!(framed.frame().map(|(rect, _)| rect), Some(source));
        assert_eq!(framed.picture_in_canvas(), Bounds::new(20.0, 20.0, 200.0, 150.0));
        assert_eq!(framed.picture_in_canvas(), layout(source, &params).image);
        // A plain colour the panel laid out frames the picture as any other does.
        let colour = BackgroundParams::plain(Rgba::new(0.0, 0.0, 0.0, 1.0));
        let coloured = laid_out(400.0, 300.0, source, colour);
        assert_eq!(coloured.picture_in_canvas(), Bounds::new(0.0, 0.0, 200.0, 150.0));
        // A background with no source that is not the crop's fill lays out the capture.
        let mut old = Scene::new(Base::new("x.png", 400.0, 300.0, 1.0));
        let geometry = Geometry::Background { params: params.clone(), source: None, fill: None };
        old.add(Object::new(-1, 0, style(), geometry));
        assert_eq!(old.frame().map(|(rect, _)| rect), Some(capture));
    }

    /// D192: a crop past the image widens the canvas and leaves its own fill, which is the
    /// picture's and lays nothing out. A background applied now lays out the canvas, the area
    /// the crop added with it, and the panel has none of its own to show. One the panel laid
    /// out in the fill's slot keeps the fill's colour, painted over its frame's rect.
    #[test]
    fn over_the_crop_s_own_fill_a_background_lays_out_the_canvas() {
        use crate::background::BackgroundParams;
        let grey = Rgba::new(0.5, 0.5, 0.5, 1.0);
        // A capture 400 by 300, cropped 200 past its right edge and filled grey.
        let mut scene = Scene::new(Base::new("x.png", 400.0, 300.0, 1.0));
        let wider = Bounds::new(0.0, 0.0, 600.0, 300.0);
        scene.canvas = wider;
        let params = BackgroundParams::plain(grey);
        let geometry = Geometry::Background { params, source: None, fill: None };
        scene.add(Object::new(-1, 0, Style::new(grey, 1, false), geometry));
        assert!(scene.background().is_none(), "the crop's fill is not the panel's");
        assert_eq!(scene.picture_rect(), wider);
        assert_eq!(scene.frame_fill(), None);
        // The panel's background at a margin of 100, in the fill's slot.
        let params = BackgroundParams { padding: 100.0, ..BackgroundParams::default() };
        let mut panel = scene.objects()[0].clone();
        panel.geometry = Geometry::Background { params, source: Some(wider), fill: Some(grey) };
        scene.replace(panel);
        assert!(scene.background().is_some());
        assert_eq!(scene.frame().map(|(rect, _)| rect), Some(wider));
        assert_eq!(scene.frame_fill(), Some((wider, grey)));
        assert_eq!(scene.picture_rect(), wider);
    }

    /// D188: a spotlight at full opacity paints the picture black but for the spotlights'
    /// shapes, and the words outside them do not show; at less, every word still does.
    #[test]
    fn an_opaque_spotlight_shows_the_words_in_its_shapes_alone() {
        let spot = |z: i32, shape: SpotlightShape, bounds: Bounds, opacity: f64| {
            Object::new(z, 0, style(), Geometry::Spotlight { shape, bounds, opacity })
        };
        let visible = |objects: Vec<Object>| {
            let mut scene = Scene::new(Base::new("x.png", 400.0, 300.0, 1.0));
            for object in objects {
                scene.add(object);
            }
            scene.visible(Object::bounds)
        };
        let word = Bounds::new(20.0, 20.0, 60.0, 16.0);
        let beside = Bounds::new(200.0, 20.0, 60.0, 16.0);
        let held = Bounds::new(10.0, 10.0, 100.0, 40.0);
        let opaque = visible(vec![spot(0, SpotlightShape::Rectangle, held, 1.0)]);
        assert!(!opaque.hides(word));
        assert!(opaque.hides(beside));
        // Half in the shape is half black.
        assert!(opaque.hides(Bounds::new(90.0, 20.0, 60.0, 16.0)));
        // A translucent scrim leaves every word readable, and so does a hidden spotlight,
        // which is not drawn.
        assert!(!visible(vec![spot(0, SpotlightShape::Rectangle, held, 0.6)]).hides(beside));
        let mut hidden = spot(0, SpotlightShape::Rectangle, held, 1.0);
        hidden.hidden = true;
        assert!(!visible(vec![hidden]).hides(beside));
        // The scrim is as opaque as the first spotlight drawn, and every spotlight's shape
        // is a hole in it.
        let far = Bounds::new(300.0, 200.0, 50.0, 50.0);
        let first_translucent = vec![
            spot(0, SpotlightShape::Rectangle, held, 0.6),
            spot(1, SpotlightShape::Rectangle, far, 1.0),
        ];
        assert!(!visible(first_translucent).hides(beside));
        let two = visible(vec![
            spot(0, SpotlightShape::Rectangle, held, 1.0),
            spot(1, SpotlightShape::Rectangle, Bounds::new(190.0, 10.0, 100.0, 40.0), 0.6),
        ]);
        assert!(!two.hides(word) && !two.hides(beside));
        // An ellipse shows what is inside it and not its box's corners, and a rounded one
        // not the corners its radius cuts.
        let ellipse = visible(vec![spot(
            0,
            SpotlightShape::Ellipse,
            Bounds::new(0.0, 0.0, 120.0, 60.0),
            1.0,
        )]);
        assert!(!ellipse.hides(word));
        assert!(ellipse.hides(Bounds::new(2.0, 2.0, 20.0, 10.0)));
        let corner = Bounds::new(11.0, 11.0, 10.0, 5.0);
        assert!(!opaque.hides(corner));
        assert!(visible(vec![spot(0, SpotlightShape::Rounded, held, 1.0)]).hides(corner));
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "a failing assertion should panic")]
mod counter_number_tests {
    use super::*;
    use crate::geometry::Point;
    use crate::object::Geometry;
    use crate::style::{CounterStyle, Style};

    fn badge(z: i32, number: u32) -> Object {
        Object::new(
            z,
            0,
            Style::default(),
            Geometry::Counter {
                center: Point::new(0.0, 0.0),
                number,
                style: CounterStyle::Arabic,
                radius: 20.0,
                tail: None,
            },
        )
    }

    #[test]
    fn undoing_a_badge_gives_its_number_back() {
        let mut scene = Scene::new(Base::new("x.png", 100.0, 100.0, 1.0));
        assert_eq!(scene.next_counter_number(), 1);
        for n in 1..=3 {
            let number = scene.next_counter_number();
            assert_eq!(number, n);
            scene.add(badge(n32(n), number));
        }
        assert_eq!(scene.next_counter_number(), 4);

        // An undo removes the third badge, and the next one placed is a 3 again -- not a
        // 4 with a hole where the 3 was.
        let third = scene
            .objects()
            .iter()
            .rev()
            .find(|o| o.kind() == crate::object::ObjectKind::Counter)
            .map(|o| o.id);
        scene.remove(third.expect("a badge"));
        assert_eq!(scene.next_counter_number(), 3);
    }

    #[test]
    fn the_starting_number_shifts_the_whole_sequence() {
        let mut scene = Scene::new(Base::new("x.png", 100.0, 100.0, 1.0));
        scene.set_next_number(0);
        assert_eq!(scene.next_counter_number(), 0, "`spec/05` §2: 0 allowed");
        scene.add(badge(0, 0));
        assert_eq!(scene.next_counter_number(), 1);
    }

    /// A reopened document has no stored starting number, only its badges: the next one
    /// carries on from them (`ANN-10`).
    #[test]
    fn a_reopened_document_carries_on_from_its_badges() {
        let mut scene = Scene::new(Base::new("x.png", 100.0, 100.0, 1.0));
        for (z, number) in [5, 6, 7].into_iter().enumerate() {
            scene.add(badge(i32::try_from(z).unwrap_or(0), number));
        }
        assert_eq!(scene.starting_number(), 5);
        assert_eq!(scene.next_counter_number(), 8);
    }

    #[test]
    fn other_objects_do_not_advance_the_sequence() {
        let mut scene = Scene::new(Base::new("x.png", 100.0, 100.0, 1.0));
        scene.add(Object::new(
            0,
            0,
            Style::default(),
            Geometry::Ellipse { bounds: Bounds::new(0.0, 0.0, 10.0, 10.0) },
        ));
        assert_eq!(scene.next_counter_number(), 1);
    }

    fn n32(n: u32) -> i32 {
        i32::try_from(n).unwrap_or(i32::MAX)
    }
}
