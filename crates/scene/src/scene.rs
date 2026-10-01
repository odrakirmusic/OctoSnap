// SPDX-License-Identifier: GPL-3.0-or-later

//! The document: a base image, a canvas, and the objects on it (`spec/05` §3, §5.3).

use serde::{Deserialize, Serialize};

use crate::background::BackgroundParams;
use crate::geometry::{Bounds, Point};
use crate::object::{Geometry, Object, ObjectId, ObjectKind};
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
        Self {
            file: file.into(),
            width,
            height,
            scale,
            orientation: Orientation::UPRIGHT,
        }
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

    /// `spec/05` §4.9's starting number, as stored.
    #[must_use]
    pub const fn starting_number(&self) -> u32 {
        self.next_number
    }

    /// `spec/05` §2's "starting number (0 allowed)".
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
    #[must_use]
    pub fn next_counter_number(&self) -> u32 {
        let placed = self
            .objects
            .iter()
            .filter(|o| matches!(o.kind(), crate::object::ObjectKind::Counter))
            .count();
        self.next_number.saturating_add(u32::try_from(placed).unwrap_or(u32::MAX))
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
            .filter(|o| o.kind() == ObjectKind::Spotlight)
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
            .filter(|other| {
                self.get(*other).is_some_and(|o| (render_group(o.kind()), o.z) < key)
            })
            .collect()
    }

    /// `spec/05` §4.13's background object, if the document has one.
    ///
    /// One, and the first in render order if a project somehow carries two: a background
    /// *is* the canvas, and two canvases is not a state the panel can show. Answering
    /// with the id as well because every edit the panel makes is a `Change` on it.
    #[must_use]
    pub fn background(&self) -> Option<(ObjectId, &BackgroundParams)> {
        self.render_list().into_iter().find_map(|object| match &object.geometry {
            Geometry::Background { params, .. } => Some((object.id, params)),
            _ => None,
        })
    }

    /// The rect a background lays out and the parameters it lays it out with.
    ///
    /// The source is the document as it was when the tool was first used: the base image,
    /// or the crop if one had already narrowed it.
    #[must_use]
    pub fn background_source(&self) -> Option<(Bounds, &BackgroundParams)> {
        self.render_list().into_iter().find_map(|object| match &object.geometry {
            Geometry::Background { params, source } => {
                Some((source.unwrap_or_else(|| self.base.bounds()), params))
            }
            _ => None,
        })
    }

    /// What a background applied right now would lay out.
    ///
    /// The canvas, which is the base image until `spec/05` §4.11's crop has narrowed it.
    /// Read by the panel when it adds the object, and stored in it from then on so that
    /// a later crop narrows the *export* rather than re-laying out the picture.
    #[must_use]
    pub fn picture_rect(&self) -> Bounds {
        match self.background_source() {
            Some((source, _)) => source,
            None => self.canvas,
        }
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
}

/// `spec/05` §5.3's groups, bottom to top. The number is the layer, not a `z`.
const fn render_group(kind: ObjectKind) -> u8 {
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
        | ObjectKind::Text => 4,
        ObjectKind::Counter => 5,
    }
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
        Object::new(z, 0, style(), Geometry::Rect {
            bounds: Bounds::new(x, 0.0, 100.0, 100.0),
            filled: false,
            radius: 0.0,
        })
    }

    fn counter_at(z: i32) -> Object {
        Object::new(z, 0, style(), Geometry::Counter {
            center: Point::new(50.0, 50.0),
            number: 1,
            style: CounterStyle::Arabic,
            radius: 12.0,
        })
    }

    fn redact_at(z: i32) -> Object {
        Object::new(z, 0, style(), Geometry::Redact {
            bounds: Bounds::new(0.0, 0.0, 100.0, 100.0),
            style: RedactStyle::Pixelate,
            intensity: 0.5,
            seed: 7,
        })
    }

    fn spotlight_at(z: i32, x: f64) -> Object {
        Object::new(z, 0, style(), Geometry::Spotlight {
            shape: SpotlightShape::Rectangle,
            bounds: Bounds::new(x, 0.0, 100.0, 100.0),
            opacity: 0.6,
        })
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
        let background = Object::new(50, 0, style(), Geometry::Background {
            params: crate::background::BackgroundParams::default(),
            source: None,
        });
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
        let third = scene.objects().iter().rev().find(|o| o.kind() == crate::object::ObjectKind::Counter).map(|o| o.id);
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
