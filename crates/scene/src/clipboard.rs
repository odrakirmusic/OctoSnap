// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §4.1's object clipboard: "Ctrl+C/Ctrl+V copies objects (internal clipboard;
//! when nothing is selected Ctrl+C copies the image)".
//!
//! Internal, as §4.1 says, and not the desktop's clipboard. What is copied is annotations,
//! which mean nothing to any other application, and the desktop's clipboard is where the
//! *image* goes when nothing is selected. Putting objects there would push the user's last
//! copied screenshot off it for the sake of a format nobody else reads.
//!
//! Pasting behaves like Ctrl+D (`spec/05` §4.1: "duplicates offset by 12 units"): each copy
//! gets a new identity and lands 12 units down and to the right. A second paste of the same
//! copy lands 12 further on, so repeated pastes cascade instead of stacking exactly on top of
//! each other, where only the last one could be seen or grabbed.
//!
//! A cut's first paste lands where the objects were, and the cascade starts from there:
//! the originals are gone, so there is nothing for it to hide under, and cutting and
//! pasting is how objects are moved -- into another capture's editor too, at the same
//! place.

use crate::object::{Object, ObjectId, ObjectKind};
use crate::scene::Scene;

/// How far one paste moves its copies, in document units: the same step as Ctrl+D.
pub const PASTE_OFFSET: f64 = 12.0;

/// The copied objects, and how far the next paste moves them.
#[derive(Debug, Default, Clone)]
pub struct Clipboard {
    /// In the order they were stacked in when copied, lowest first.
    objects: Vec<Object>,
    /// In steps of [`PASTE_OFFSET`]: one after a copy, none after a cut, and one more after
    /// each paste.
    next: u32,
}

impl Clipboard {
    #[must_use]
    pub const fn new() -> Self {
        Self { objects: Vec::new(), next: 1 }
    }

    /// Whether a paste would add anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// How many objects a paste would add.
    #[must_use]
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Copies these objects out of `scene`, replacing whatever was copied before. Answers
    /// how many were taken.
    ///
    /// Two kinds are never copied, because a document can have only one of each and a
    /// second would mean something else. The background (`spec/05` §4.13) is the document's
    /// frame, not a mark on it. A crop (§4.11) is the document's extent. Everything else
    /// comes along, locked objects included: locking stops a mark from being moved by
    /// accident, and copying it moves nothing.
    pub fn copy(&mut self, scene: &Scene, ids: &[ObjectId]) -> usize {
        let mut taken: Vec<Object> = ids
            .iter()
            .filter_map(|id| scene.get(*id))
            .filter(|object| copyable(object.kind()))
            .cloned()
            .collect();
        // Stacking order, so a paste puts them back one above the other as they were. The
        // sort is stable, and the scene keeps equal z values in creation order, so ties keep
        // the order they were drawn in.
        taken.sort_by_key(|object| object.z);
        let count = taken.len();
        if count > 0 {
            self.objects = taken;
            self.next = 1;
        }
        count
    }

    /// [`Clipboard::copy`], for objects the caller is about to delete: the first paste puts
    /// them back where they were.
    pub fn cut(&mut self, scene: &Scene, ids: &[ObjectId]) -> usize {
        let count = self.copy(scene, ids);
        if count > 0 {
            self.next = 0;
        }
        count
    }

    /// The objects one paste into `scene` adds: new ids, moved by one more step than the
    /// last paste, and stacked above everything in `scene` in the order they were copied
    /// in.
    ///
    /// Nothing about `scene` is changed. The caller adds these as one undo step, which is
    /// what §5.2 makes a gesture.
    pub fn paste(&mut self, scene: &Scene) -> Vec<Object> {
        if self.objects.is_empty() {
            return Vec::new();
        }
        let offset = PASTE_OFFSET * f64::from(self.next);
        self.next = self.next.saturating_add(1);
        let mut z = scene.top_z();
        self.objects
            .iter()
            .map(|original| {
                let mut copy = original.clone();
                // A new identity, as Ctrl+D gives one: `spec/05` §5.1's id is a ULID, and two
                // objects sharing one would make `Scene::get` answer either of them.
                copy.id = ObjectId::new();
                copy.translate(offset, offset);
                copy.z = z;
                z = z.saturating_add(1);
                copy
            })
            .collect()
    }
}

/// Whether an object of this kind can be copied. See [`Clipboard::copy`].
#[must_use]
pub const fn copyable(kind: ObjectKind) -> bool {
    !matches!(kind, ObjectKind::Background | ObjectKind::Crop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::BackgroundParams;
    use crate::geometry::{Bounds, Point};
    use crate::object::Geometry;
    use crate::scene::Base;
    use crate::style::{Rgba, Style};

    fn scene() -> Scene {
        Scene::new(Base::new("base.png", 1000.0, 800.0, 1.0))
    }

    fn style() -> Style {
        Style::new(Rgba::new(1.0, 0.0, 0.0, 1.0), 3, false)
    }

    fn rect(z: i32, x: f64) -> Object {
        Object::new(z, 0, style(), Geometry::Rect {
            bounds: Bounds::new(x, 10.0, 100.0, 50.0),
            filled: false,
            radius: 0.0,
        })
    }

    fn origin(object: &Object) -> (f64, f64) {
        match &object.geometry {
            Geometry::Rect { bounds, .. } => (bounds.x, bounds.y),
            Geometry::Line { start, .. } => (start.x, start.y),
            other => panic!("no origin for {other:?}"),
        }
    }

    #[test]
    fn nothing_copied_pastes_nothing() {
        let mut clipboard = Clipboard::new();
        assert!(clipboard.is_empty());
        assert!(clipboard.paste(&scene()).is_empty());
    }

    #[test]
    fn a_paste_is_new_objects_twelve_units_on() {
        let mut scene = scene();
        let original = rect(0, 40.0);
        scene.add(original.clone());
        let mut clipboard = Clipboard::new();
        assert_eq!(clipboard.copy(&scene, &[original.id]), 1);

        let pasted = clipboard.paste(&scene);
        assert_eq!(pasted.len(), 1);
        let copy = &pasted[0];
        assert_ne!(copy.id, original.id, "a copy has an identity of its own");
        assert_eq!(origin(copy), (52.0, 22.0));
        assert_eq!(copy.style, original.style);
        assert!(copy.z > original.z, "a paste lands on top");
        // The scene is the caller's to change, as one undo step.
        assert_eq!(scene.len(), 1);
    }

    #[test]
    fn a_cut_pastes_back_in_place_and_then_cascades() {
        let mut scene = scene();
        let original = rect(0, 40.0);
        scene.add(original.clone());
        let mut clipboard = Clipboard::new();
        assert_eq!(clipboard.cut(&scene, &[original.id]), 1);
        let first = clipboard.paste(&scene);
        assert_eq!(origin(&first[0]), origin(&original), "the first paste is where it was");
        assert_ne!(first[0].id, original.id);
        let second = clipboard.paste(&scene);
        assert_eq!(origin(&second[0]), (52.0, 22.0));
    }

    #[test]
    fn repeated_pastes_cascade() {
        let mut scene = scene();
        let original = rect(0, 0.0);
        scene.add(original.clone());
        let mut clipboard = Clipboard::new();
        clipboard.copy(&scene, &[original.id]);

        let mut seen = Vec::new();
        for _ in 0..3 {
            let pasted = clipboard.paste(&scene);
            seen.push(origin(&pasted[0]));
            for object in pasted {
                scene.add(object);
            }
        }
        assert_eq!(seen, vec![(12.0, 22.0), (24.0, 34.0), (36.0, 46.0)]);
        let ids: std::collections::HashSet<_> = scene.objects().iter().map(|o| o.id).collect();
        assert_eq!(ids.len(), 4, "every paste is new objects");
    }

    #[test]
    fn a_new_copy_starts_the_cascade_again() {
        let mut scene = scene();
        let (a, b) = (rect(0, 0.0), rect(1, 300.0));
        scene.add(a.clone());
        scene.add(b.clone());
        let mut clipboard = Clipboard::new();
        clipboard.copy(&scene, &[a.id]);
        clipboard.paste(&scene);
        clipboard.paste(&scene);

        clipboard.copy(&scene, &[b.id]);
        assert_eq!(origin(&clipboard.paste(&scene)[0]), (312.0, 22.0));
    }

    #[test]
    fn copies_keep_their_stacking_order_above_everything() {
        let mut scene = scene();
        let low = rect(2, 0.0);
        let high = rect(7, 100.0);
        let top = rect(9, 500.0);
        for object in [&low, &high, &top] {
            scene.add(object.clone());
        }
        let mut clipboard = Clipboard::new();
        // Asked for in the reverse order: what counts is how they were stacked.
        clipboard.copy(&scene, &[high.id, low.id]);

        let pasted = clipboard.paste(&scene);
        assert_eq!(origin(&pasted[0]), (12.0, 22.0), "the lower one first");
        assert_eq!(origin(&pasted[1]), (112.0, 22.0));
        assert!(pasted[0].z > top.z && pasted[1].z > pasted[0].z);
    }

    #[test]
    fn the_background_and_the_crop_stay_behind() {
        let mut scene = scene();
        let mark = rect(1, 0.0);
        let background = Object::new(0, 0, style(), Geometry::Background {
            params: BackgroundParams::default(),
            source: None,
        });
        scene.add(mark.clone());
        scene.add(background.clone());
        let mut clipboard = Clipboard::new();
        assert_eq!(clipboard.copy(&scene, &[background.id, mark.id]), 1);
        assert_eq!(clipboard.len(), 1);
        assert!(!copyable(ObjectKind::Crop));
    }

    #[test]
    fn a_copy_of_nothing_keeps_what_was_copied() {
        let mut scene = scene();
        let mark = rect(0, 0.0);
        scene.add(mark.clone());
        let mut clipboard = Clipboard::new();
        clipboard.copy(&scene, &[mark.id]);
        // An id the scene no longer has: a stale selection, say.
        assert_eq!(clipboard.copy(&scene, &[ObjectId::new()]), 0);
        assert_eq!(clipboard.len(), 1, "a copy that took nothing is not a copy");
    }

    #[test]
    fn a_line_moves_by_both_ends() {
        let mut scene = scene();
        let line = Object::new(0, 0, style(), Geometry::Line {
            start: Point::new(10.0, 10.0),
            end: Point::new(60.0, 90.0),
        });
        scene.add(line.clone());
        let mut clipboard = Clipboard::new();
        clipboard.copy(&scene, &[line.id]);
        let pasted = clipboard.paste(&scene);
        let Geometry::Line { start, end } = pasted[0].geometry else {
            panic!("a line pastes as a line");
        };
        assert_eq!((start.x, start.y, end.x, end.y), (22.0, 22.0, 72.0, 102.0));
    }
}
