// SPDX-License-Identifier: GPL-3.0-or-later

//! Undo, as `spec/05` §5.2 specifies it: commands, not snapshots.
//!
//! > Command objects (`Add`, `Remove`, `Change{before,after}`, `Reorder`, `CanvasChange`)
//! > with coalescing of continuous edits (drag, slider, typing) into one step on gesture
//! > end. Undo depth unlimited within a session.
//!
//! "Unlimited" is the reason this holds commands. A drag of a 5120x2880 capture's
//! rectangle is a few dozen bytes as a command and a whole document as a snapshot, and
//! `spec/05` §11 item 10 asks for memory to stay flat over twenty editor sessions.
//!
//! Coalescing is the other half, and it is what makes the model usable rather than merely
//! correct: a drag emits a `Change` per motion event, and fifty of those in the history
//! would mean fifty presses of Ctrl+Z to undo one gesture. So consecutive changes to the
//! same object fold into one, keeping the *first* `before` and the *latest* `after` --
//! which is exactly "one step on gesture end" without needing to be told when the gesture
//! ended.

use crate::geometry::Bounds;
use crate::object::{Object, ObjectId};
use crate::scene::{Base, Scene};

/// One undoable edit.
///
/// Each variant carries enough to go both ways without consulting the scene, so undo can
/// never be defeated by the scene having changed in between -- which it can, because
/// `spec/05` §4.10 rasterises a redaction on a worker thread and swaps it in.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Add(Object),
    /// The whole object, not its id: removing has to be undoable, and an id alone cannot
    /// put a rectangle back.
    Remove(Object),
    Change {
        id: ObjectId,
        before: Box<Object>,
        after: Box<Object>,
    },
    /// A z change on its own (`spec/05` §9's `Ctrl+]` / `Ctrl+[`).
    ///
    /// Distinct from `Change` although a `Change` could express it, because
    /// [`History::push`] coalesces consecutive changes to one object and reordering must
    /// not fold into a drag that happened to precede it -- pressing `Ctrl+]` four times
    /// is four steps the user expects to undo one at a time.
    Reorder {
        id: ObjectId,
        before: i32,
        after: i32,
    },
    /// `spec/05` §4.11's crop and §4.12's resize, which change the canvas rather than any
    /// object.
    CanvasChange {
        before: Bounds,
        after: Bounds,
    },
    /// `spec/05` §4.12's Resize, Rotate and Flip, which change the base image's rect and
    /// the way its pixels lie in it (D91).
    ///
    /// Never on its own: a transform moves the canvas and every object with it, so this
    /// arrives inside the [`Self::Batch`] `transform::plan` builds. It is a variant of its
    /// own for the reason `CanvasChange` is -- the base is not an object and an id cannot
    /// name it.
    BaseChange {
        before: Base,
        after: Base,
    },
    /// Several edits that are one step (`spec/05` §5.2).
    ///
    /// [`Self::absorb`] cannot do this job. It folds consecutive changes to *one* object,
    /// which is what a slider being dragged produces; a marquee-selected group being
    /// dragged produces one change per object and those never coalesce, so five moved
    /// rectangles were five presses of Ctrl+Z to put back. §5.2 says a gesture is a step,
    /// and for a gesture that touches several objects the step has to be able to say so.
    ///
    /// Undone in reverse, which matters as soon as a batch mixes kinds: a batch that adds
    /// an object and reorders another has to unwind in the order that leaves the scene as
    /// it was found.
    Batch(Vec<Command>),
}

impl Command {
    /// Which object this edit is about, or `None` for a canvas edit.
    ///
    /// Public because the editor needs it for something the history does not: after an
    /// undo, the object that came back is the one to re-select, and `spec/05` §4.1's
    /// select tool has to be told which. A canvas edit selects nothing.
    /// Not `const`: a batch has to look inside a `Vec`, and `Deref` is not const yet.
    #[must_use]
    pub fn subject(&self) -> Option<ObjectId> {
        match self {
            Self::Add(object) | Self::Remove(object) => Some(object.id),
            Self::Change { id, .. } | Self::Reorder { id, .. } => Some(*id),
            Self::CanvasChange { .. } | Self::BaseChange { .. } => None,
            // The first, and not "all of them": the caller wants one object to re-select
            // after an undo, and a batch's members are by construction one gesture over
            // one selection -- the editor already knows the rest of it.
            Self::Batch(commands) => commands.first().and_then(Self::subject),
        }
    }

    /// Applies the edit. Answers whether the scene took it.
    ///
    /// `false` rather than an error type: every way this can fail is the same way -- the
    /// object is not in the scene any more -- and the caller's only sensible response is
    /// to drop the command, which is what [`History::apply`] does.
    fn redo(&self, scene: &mut Scene) -> bool {
        match self {
            Self::Add(object) => {
                scene.add(object.clone());
                true
            }
            Self::Remove(object) => scene.remove(object.id).is_some(),
            Self::Change { after, .. } => scene.replace((**after).clone()).is_some(),
            Self::Reorder { id, after, .. } => set_z(scene, *id, *after),
            Self::CanvasChange { after, .. } => {
                scene.canvas = *after;
                true
            }
            Self::BaseChange { after, .. } => {
                scene.base = after.clone();
                true
            }
            // All or nothing: a batch that half-applied would leave a step that undo
            // cannot reverse, which is worse than an edit that did not happen. The
            // members are checked before any is kept.
            Self::Batch(commands) => {
                if commands.iter().any(|c| !c.can_redo(scene)) {
                    return false;
                }
                for command in commands {
                    command.redo(scene);
                }
                true
            }
        }
    }

    /// Whether [`Self::redo`] would succeed, without changing anything.
    ///
    /// Only a batch needs to ask: every other variant can simply try, because a failure
    /// leaves the scene untouched. A batch's failure would leave it half-done.
    fn can_redo(&self, scene: &Scene) -> bool {
        match self {
            Self::Add(_) | Self::CanvasChange { .. } | Self::BaseChange { .. } => true,
            Self::Remove(object) => scene.get(object.id).is_some(),
            Self::Change { after, .. } => scene.get(after.id).is_some(),
            Self::Reorder { id, .. } => scene.get(*id).is_some(),
            Self::Batch(commands) => commands.iter().all(|c| c.can_redo(scene)),
        }
    }

    /// Puts the scene back the way it was.
    fn undo(&self, scene: &mut Scene) -> bool {
        match self {
            Self::Add(object) => scene.remove(object.id).is_some(),
            Self::Remove(object) => {
                scene.add(object.clone());
                true
            }
            Self::Change { before, .. } => scene.replace((**before).clone()).is_some(),
            Self::Reorder { id, before, .. } => set_z(scene, *id, *before),
            Self::CanvasChange { before, .. } => {
                scene.canvas = *before;
                true
            }
            Self::BaseChange { before, .. } => {
                scene.base = before.clone();
                true
            }
            Self::Batch(commands) => {
                for command in commands.iter().rev() {
                    command.undo(scene);
                }
                true
            }
        }
    }

    /// Folds `next` into this command if they are one continuous edit.
    ///
    /// Only `Change` coalesces, and only with a `Change` to the same object. `Add` does
    /// not absorb the drag that follows it even though a tool emits both -- the shape
    /// appearing and the shape being moved are two things the user can undo separately,
    /// and folding them would make the first Ctrl+Z delete a rectangle the user only meant
    /// to put back.
    /// A [`Self::Batch`] never folds, in either direction. It is emitted once, at gesture
    /// end, by a caller that already knows the gesture is over -- so there is nothing for
    /// coalescing to add, and folding two batches would need their selections to match,
    /// which is a question the history has no way to ask.
    fn absorb(&mut self, next: &Self) -> bool {
        match (&mut *self, next) {
            (
                Self::Change { id, after, .. },
                Self::Change { id: next_id, after: next_after, .. },
            ) => {
                if id != next_id {
                    return false;
                }
                // The first `before` is kept -- it is where the gesture started -- and the
                // latest `after` wins.
                *after = next_after.clone();
                true
            }
            // A batch folds into a batch of the same shape, member by member. `spec/05`
            // §4.13's background sliders need it: every value change is a `Change` on the
            // background object *and* a `CanvasChange`, because padding resizes the
            // canvas, and a drag from 100 to 240 is one step to undo rather than a
            // hundred and forty. The shape has to match so a batch that adds an object
            // never swallows the batch after it.
            (Self::Batch(mine), Self::Batch(theirs)) => {
                if mine.len() != theirs.len() {
                    return false;
                }
                if !mine.iter().zip(theirs).all(|(a, b)| a.folds_into(b)) {
                    return false;
                }
                for (a, b) in mine.iter_mut().zip(theirs) {
                    if !a.absorb(b) {
                        return false;
                    }
                }
                true
            }
            // A canvas edit inside a batch, which is the background's other half. Not
            // offered on its own: two crops in a row are two steps, and §4.11's Enter is
            // the only thing that applies one.
            (Self::CanvasChange { after, .. }, Self::CanvasChange { after: next_after, .. }) => {
                *after = *next_after;
                true
            }
            _ => false,
        }
    }

    /// Whether [`Self::absorb`] would say yes, without changing anything.
    ///
    /// A batch has to know the answer for *every* member before it folds any of them,
    /// or a half-folded batch is left behind by the first member that says no.
    fn folds_into(&self, next: &Self) -> bool {
        match (self, next) {
            (Self::Change { id, .. }, Self::Change { id: next_id, .. }) => id == next_id,
            (Self::CanvasChange { .. }, Self::CanvasChange { .. }) => true,
            (Self::Batch(mine), Self::Batch(theirs)) => {
                mine.len() == theirs.len()
                    && mine.iter().zip(theirs).all(|(a, b)| a.folds_into(b))
            }
            _ => false,
        }
    }
}

fn set_z(scene: &mut Scene, id: ObjectId, z: i32) -> bool {
    let Some(object) = scene.get(id) else { return false };
    let mut moved = object.clone();
    moved.z = z;
    scene.replace(moved).is_some()
}

/// The undo stack (`spec/05` §5.2), unlimited within a session.
#[derive(Debug, Clone, Default)]
pub struct History {
    done: Vec<Command>,
    undone: Vec<Command>,
    /// How long `done` was when the last gesture ended.
    ///
    /// A barrier expressed as a length rather than as a variant that does nothing, which
    /// undo would then have to know to skip. Coalescing is only ever asked of the last
    /// command, so one number is enough: a command pushed at or before this point is
    /// closed and absorbs nothing.
    closed_at: usize,
}

impl History {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies a command and records it, coalescing with the previous one where they are
    /// one gesture.
    ///
    /// A command that the scene refused is not recorded, so undo cannot walk back through
    /// an edit that never happened.
    pub fn apply(&mut self, scene: &mut Scene, command: Command) -> bool {
        if !command.redo(scene) {
            return false;
        }
        // Any new edit ends the redo branch. `spec/05` §5.2 does not say so, and it is
        // what every editor does: after undoing and then drawing something else, "redo"
        // has nothing left to mean.
        self.undone.clear();
        // Two steps rather than a `match` guard: a guard cannot take the mutable borrow
        // `absorb` needs, and splitting the question from the act makes the condition --
        // "the last command is still inside the current gesture" -- readable on its own.
        let open = self.done.len() > self.closed_at;
        let folded = open
            && self.done.last_mut().is_some_and(|previous| previous.absorb(&command));
        if !folded {
            self.done.push(command);
        }
        true
    }

    /// Ends the current gesture, so the next edit starts a new undo step.
    ///
    /// `spec/05` §5.2 coalesces "into one step **on gesture end**", and this is the gesture
    /// end. Without it a drag, a pause, and a second drag of the same object would fold
    /// into one step -- the two are indistinguishable from the history's side, because the
    /// only difference between them is that the user let go.
    pub fn end_gesture(&mut self) {
        // Pushing a barrier would need a variant that does nothing, which would then have
        // to be skipped by undo. Marking the tail as closed is cheaper: `absorb` is only
        // ever asked of the last command, so a length remembered here is enough.
        self.closed_at = self.done.len();
    }

    /// Undoes one step. Answers whether there was one.
    pub fn undo(&mut self, scene: &mut Scene) -> bool {
        let Some(command) = self.done.pop() else { return false };
        if !command.undo(scene) {
            // The scene has moved on and this step no longer applies. Dropping it is the
            // only option that leaves the history consistent with what is on screen.
            return false;
        }
        self.closed_at = self.closed_at.min(self.done.len());
        self.undone.push(command);
        true
    }

    /// Redoes one step.
    pub fn redo(&mut self, scene: &mut Scene) -> bool {
        let Some(command) = self.undone.pop() else { return false };
        if !command.redo(scene) {
            return false;
        }
        self.done.push(command);
        // A redone step is closed: it was a whole step when it was undone.
        self.closed_at = self.done.len();
        true
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    /// How many steps deep the history is, which is what the tests count.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.done.len()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::object::Geometry;
    use crate::scene::Base;
    use crate::style::{Rgba, Style};

    fn scene() -> Scene {
        Scene::new(Base::new("base.png", 1000.0, 1000.0, 1.0))
    }

    fn style() -> Style {
        Style::new(Rgba::new(1.0, 0.0, 0.0, 1.0), 3, false)
    }

    fn rect(z: i32, x: f64) -> Object {
        Object::new(z, 0, style(), Geometry::Rect {
            bounds: Bounds::new(x, 0.0, 100.0, 100.0),
            filled: false,
            radius: 0.0,
        })
    }

    /// Moved by `dx`, as a `Change` -- which is what one motion event of a drag produces.
    fn dragged(object: &Object, dx: f64) -> Command {
        let mut after = object.clone();
        after.translate(dx, 0.0);
        Command::Change {
            id: object.id,
            before: Box::new(object.clone()),
            after: Box::new(after),
        }
    }

    #[test]
    fn add_and_undo_leave_the_scene_as_it_was() {
        let mut s = scene();
        let mut h = History::new();
        assert!(!h.can_undo());

        let object = rect(0, 0.0);
        let id = object.id;
        assert!(h.apply(&mut s, Command::Add(object)));
        assert_eq!(s.len(), 1);
        assert!(h.can_undo());

        assert!(h.undo(&mut s));
        assert!(s.is_empty());
        assert!(h.can_redo());
        assert!(h.redo(&mut s));
        assert_eq!(s.get(id).map(|o| o.id), Some(id));
    }

    /// `spec/05` §5.2's coalescing: a drag emits a `Change` per motion event, and fifty
    /// of those must be one press of Ctrl+Z, not fifty.
    #[test]
    fn a_drag_is_one_undo_step() {
        let mut s = scene();
        let mut h = History::new();
        let object = rect(0, 0.0);
        let id = object.id;
        h.apply(&mut s, Command::Add(object.clone()));
        h.end_gesture();

        let mut moving = object;
        for _ in 0..50 {
            let step = dragged(&moving, 1.0);
            let Command::Change { after, .. } = &step else { panic!("a change") };
            moving = (**after).clone();
            assert!(h.apply(&mut s, step));
        }
        assert_eq!(h.depth(), 2, "the add, and the whole drag");
        // The rect was drawn at x=0 with a level-3 stroke, so its bounds start at -1.5;
        // fifty single-pixel steps put them at 48.5.
        assert_eq!(s.get(id).unwrap().bounds().normalised().x, 48.5);

        assert!(h.undo(&mut s));
        assert_eq!(s.get(id).unwrap().bounds().normalised().x, -1.5,
                   "back where the drag started, in one step");
        assert_eq!(h.depth(), 1, "the add is still there");
    }

    /// And the reason `end_gesture` exists: two drags of the same object, with the user
    /// letting go in between, are two steps. From the history's side the only difference
    /// is that call.
    #[test]
    fn letting_go_between_two_drags_makes_them_two_steps() {
        let mut s = scene();
        let mut h = History::new();
        let object = rect(0, 0.0);
        h.apply(&mut s, Command::Add(object.clone()));
        h.end_gesture();

        h.apply(&mut s, dragged(&object, 10.0));
        h.end_gesture();
        let mut moved = object.clone();
        moved.translate(10.0, 0.0);
        h.apply(&mut s, dragged(&moved, 10.0));

        assert_eq!(h.depth(), 3);
        h.undo(&mut s);
        assert_eq!(h.depth(), 2, "one drag undone, the other still on the stack");
    }

    /// An `Add` does not absorb the drag that follows it, even though a tool emits both:
    /// the first Ctrl+Z must undo the move, not delete the shape.
    #[test]
    fn drawing_then_moving_are_two_steps() {
        let mut s = scene();
        let mut h = History::new();
        let object = rect(0, 0.0);
        h.apply(&mut s, Command::Add(object.clone()));
        h.apply(&mut s, dragged(&object, 25.0));
        assert_eq!(h.depth(), 2);
        h.undo(&mut s);
        assert_eq!(s.len(), 1, "the shape is still there");
    }

    /// Reordering is its own variant precisely so it does not fold: `spec/05` §9 binds
    /// `Ctrl+]` to it and four presses are four steps.
    #[test]
    fn reordering_does_not_coalesce_with_itself() {
        let mut s = scene();
        let mut h = History::new();
        let object = rect(0, 0.0);
        let id = object.id;
        s.add(object);
        for z in 1..=4 {
            assert!(h.apply(&mut s, Command::Reorder { id, before: z - 1, after: z }));
        }
        assert_eq!(h.depth(), 4);
        assert_eq!(s.get(id).unwrap().z, 4);
        h.undo(&mut s);
        assert_eq!(s.get(id).unwrap().z, 3, "one press undone, not all four");
    }

    #[test]
    fn a_canvas_change_goes_both_ways() {
        let mut s = scene();
        let mut h = History::new();
        let before = s.canvas;
        let cropped = Bounds::new(100.0, 100.0, 400.0, 300.0);
        h.apply(&mut s, Command::CanvasChange { before, after: cropped });
        assert_eq!(s.canvas, cropped);
        h.undo(&mut s);
        assert_eq!(s.canvas, before);
        assert_eq!(Command::CanvasChange { before, after: cropped }.subject(), None);
    }

    /// A new edit after an undo ends the redo branch, which is what every editor does and
    /// what "redo" would otherwise have no meaning for.
    #[test]
    fn drawing_after_an_undo_ends_the_redo_branch() {
        let mut s = scene();
        let mut h = History::new();
        h.apply(&mut s, Command::Add(rect(0, 0.0)));
        h.undo(&mut s);
        assert!(h.can_redo());
        h.apply(&mut s, Command::Add(rect(0, 500.0)));
        assert!(!h.can_redo());
    }

    /// `spec/05` §11 item 4: "undo 50 steps and redo 50 steps restores byte-identical
    /// objects.json". The acceptance test itself needs an editor; this is the half that
    /// does not, and it is the half that can be wrong.
    #[test]
    fn fifty_undos_and_fifty_redos_restore_the_document_byte_for_byte() {
        let mut s = scene();
        let mut h = History::new();
        for step in 0..50 {
            h.apply(&mut s, Command::Add(rect(step, f64::from(step) * 10.0)));
            h.end_gesture();
        }
        let full = s.to_objects_json().unwrap();
        assert_eq!(h.depth(), 50);

        for _ in 0..50 {
            assert!(h.undo(&mut s), "fifty steps to undo");
        }
        assert!(s.is_empty());
        assert!(!h.can_undo());

        for _ in 0..50 {
            assert!(h.redo(&mut s), "and fifty to redo");
        }
        assert_eq!(s.to_objects_json().unwrap(), full);
    }

    /// Mixed edits, not just adds: a change and a reorder have to come back in the right
    /// order too, or the fiftieth undo would restore the wrong z.
    #[test]
    fn a_mixed_history_round_trips_in_order() {
        let mut s = scene();
        let mut h = History::new();
        let object = rect(0, 0.0);
        let id = object.id;
        h.apply(&mut s, Command::Add(object.clone()));
        h.end_gesture();
        h.apply(&mut s, dragged(&object, 40.0));
        h.end_gesture();
        h.apply(&mut s, Command::Reorder { id, before: 0, after: 7 });
        h.end_gesture();
        let after_all = s.to_objects_json().unwrap();

        while h.undo(&mut s) {}
        assert!(s.is_empty(), "every step undone leaves an empty scene");
        while h.redo(&mut s) {}
        assert_eq!(s.to_objects_json().unwrap(), after_all);
    }

    #[test]
    fn undoing_with_nothing_to_undo_is_false_rather_than_a_panic() {
        let mut s = scene();
        let mut h = History::new();
        assert!(!h.undo(&mut s));
        assert!(!h.redo(&mut s));
    }

    /// A command the scene refuses is not recorded, so undo cannot walk back through an
    /// edit that never happened.
    #[test]
    fn an_edit_the_scene_refuses_is_not_recorded() {
        let mut s = scene();
        let mut h = History::new();
        let absent = rect(0, 0.0);
        assert!(!h.apply(&mut s, Command::Change {
            id: absent.id,
            before: Box::new(absent.clone()),
            after: Box::new(absent.clone()),
        }));
        assert!(!h.can_undo());
        assert!(!h.apply(&mut s, Command::Remove(absent)));
        assert!(!h.can_undo());
    }

    #[test]
    fn every_command_names_the_object_it_is_about() {
        let object = rect(0, 0.0);
        let id = object.id;
        assert_eq!(Command::Add(object.clone()).subject(), Some(id));
        assert_eq!(Command::Remove(object.clone()).subject(), Some(id));
        assert_eq!(
            Command::Change {
                id,
                before: Box::new(object.clone()),
                after: Box::new(object),
            }
            .subject(),
            Some(id)
        );
        assert_eq!(Command::Reorder { id, before: 0, after: 1 }.subject(), Some(id));
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "a failing assertion should panic")]
mod batch_tests {
    use super::*;
    use crate::geometry::Bounds;
    use crate::object::Geometry;
    use crate::scene::Base;
    use crate::style::Style;

    fn scene_with(count: usize) -> (Scene, Vec<Object>) {
        let mut scene = Scene::new(Base::new("x.png", 100.0, 100.0, 1.0));
        let objects: Vec<Object> = (0..count)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let offset = i as f64 * 20.0;
                Object::new(
                    i32::try_from(i).unwrap_or(0),
                    0,
                    Style::default(),
                    Geometry::Rect {
                        bounds: Bounds::new(offset, offset, 10.0, 10.0),
                        filled: false,
                        radius: 0.0,
                    },
                )
            })
            .collect();
        for object in &objects {
            scene.add(object.clone());
        }
        (scene, objects)
    }

    fn moved(object: &Object, dx: f64, dy: f64) -> Object {
        let mut copy = object.clone();
        copy.translate(dx, dy);
        copy
    }

    fn change(before: &Object, after: Object) -> Command {
        Command::Change {
            id: before.id,
            before: Box::new(before.clone()),
            after: Box::new(after),
        }
    }

    #[test]
    fn moving_five_objects_is_one_press_of_ctrl_z() {
        // The reason `Batch` exists: five `Change`s never coalesce, so this was five
        // undos for one drag.
        let (mut scene, objects) = scene_with(5);
        let before = scene.to_objects_json().expect("serialises");

        let batch = Command::Batch(
            objects
                .iter()
                .map(|o| Command::Change {
                    id: o.id,
                    before: Box::new(o.clone()),
                    after: Box::new(moved(o, 30.0, 40.0)),
                })
                .collect(),
        );
        let mut history = History::new();
        assert!(history.apply(&mut scene, batch));
        history.end_gesture();
        assert_eq!(history.depth(), 1, "a batch is one step");

        assert!(history.undo(&mut scene));
        assert_eq!(scene.to_objects_json().expect("serialises"), before, "one undo, all back");
        assert!(!history.can_undo(), "the batch left something behind");
    }

    #[test]
    fn a_batch_undoes_in_reverse() {
        // A batch that adds an object and then reorders another has to unwind the other
        // way round, or the reorder is undone against a scene that still has the addition.
        let (mut scene, objects) = scene_with(2);
        let extra = Object::new(
            9,
            0,
            Style::default(),
            Geometry::Ellipse { bounds: Bounds::new(0.0, 0.0, 5.0, 5.0) },
        );
        let before = scene.to_objects_json().expect("serialises");
        let batch = Command::Batch(vec![
            Command::Add(extra.clone()),
            Command::Reorder { id: objects[0].id, before: 0, after: 7 },
        ]);
        let mut history = History::new();
        assert!(history.apply(&mut scene, batch));
        assert_eq!(scene.len(), 3);
        assert!(history.undo(&mut scene));
        assert_eq!(scene.to_objects_json().expect("serialises"), before);
    }

    #[test]
    fn a_batch_whose_member_cannot_apply_changes_nothing() {
        // Half a step is worse than no step: undo could not reverse it.
        let (mut scene, objects) = scene_with(2);
        let gone = Object::new(
            5,
            0,
            Style::default(),
            Geometry::Ellipse { bounds: Bounds::new(0.0, 0.0, 5.0, 5.0) },
        );
        let before = scene.to_objects_json().expect("serialises");
        let batch = Command::Batch(vec![
            Command::Change {
                id: objects[0].id,
                before: Box::new(objects[0].clone()),
                after: Box::new(moved(&objects[0], 5.0, 5.0)),
            },
            // Never in the scene, so this member cannot apply.
            Command::Change {
                id: gone.id,
                before: Box::new(gone.clone()),
                after: Box::new(gone.clone()),
            },
        ]);
        let mut history = History::new();
        assert!(!history.apply(&mut scene, batch), "a broken batch was accepted");
        assert_eq!(scene.to_objects_json().expect("serialises"), before, "the scene moved anyway");
        assert!(!history.can_undo(), "a refused batch was recorded");
    }

    /// `spec/05` §4.13's sliders: one drag of Padding is a `Change` plus a `CanvasChange`
    /// per value, and it has to be one press of Ctrl+Z at the end of it.
    #[test]
    fn a_background_slider_drag_folds_into_one_step() {
        let (mut s, objects) = scene_with(1);
        let mut h = History::new();
        let object = objects[0].clone();
        let id = object.id;
        let started = s.canvas;

        for step in 1..=40 {
            let grown = Bounds::new(0.0, 0.0, 100.0 + f64::from(step), 100.0);
            let was = s.canvas;
            h.apply(&mut s, Command::Batch(vec![
                change(&object, moved(&object, f64::from(step), 0.0)),
                Command::CanvasChange { before: was, after: grown },
            ]));
        }
        h.end_gesture();
        assert_eq!(h.depth(), 1, "the drag did not fold");

        assert!(h.undo(&mut s));
        assert_eq!(s.canvas, started, "the canvas did not come all the way back");
        assert_eq!(s.get(id), Some(&object), "the object did not come back either");
        assert!(h.redo(&mut s));
        assert_eq!(s.canvas.width, 140.0);
        assert_eq!(s.get(id), Some(&moved(&object, 40.0, 0.0)));
    }

    /// And it folds only into a batch of the same shape, so the batch that *added* the
    /// background does not swallow the first slider move after it.
    #[test]
    fn a_batch_of_a_different_shape_is_its_own_step() {
        let (mut s, objects) = scene_with(1);
        let mut h = History::new();
        let object = objects[0].clone();
        let was = s.canvas;
        h.apply(&mut s, Command::Batch(vec![
            Command::Remove(object.clone()),
            Command::CanvasChange { before: was, after: Bounds::new(0.0, 0.0, 9.0, 9.0) },
        ]));
        h.apply(&mut s, Command::Batch(vec![
            Command::Add(object.clone()),
            Command::CanvasChange {
                before: Bounds::new(0.0, 0.0, 9.0, 9.0),
                after: Bounds::new(0.0, 0.0, 10.0, 10.0),
            },
        ]));
        assert_eq!(h.depth(), 2);
    }

    #[test]
    fn a_batch_names_its_first_subject_for_reselection() {
        let (_, objects) = scene_with(3);
        let batch = Command::Batch(
            objects.iter().map(|o| Command::Remove(o.clone())).collect(),
        );
        assert_eq!(batch.subject(), Some(objects[0].id));
        assert_eq!(Command::Batch(Vec::new()).subject(), None);
    }
}
