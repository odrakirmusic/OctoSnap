// SPDX-License-Identifier: GPL-3.0-or-later

//! `ANN-10`'s badges number themselves (D182): one sequence, kept consecutive.
//!
//! The badges are put in order by their numbers -- a number held twice goes to the one
//! made first -- and the sequence counts on from the first one's number. Three things
//! change it, and each answer here is the badges whose numbers change, which the editor
//! records with the edit that caused them as one step, so one Ctrl+Z puts every badge back:
//!
//! - Deleting badges closes the gap they leave ([`closed_up`]).
//! - Giving a badge a number moves it to that place, and the others make room
//!   ([`moved`]). The badge gets the number typed, whatever it is: below the first, the
//!   sequence starts there; past the last, the sequence moves up to end there.
//! - The starting number moves the whole sequence ([`started_at`]).
//!
//! A badge placed, pasted or duplicated takes the next number ([`appended`]), and nothing
//! else moves. A document from before this, whose badges were numbered by hand, keeps its
//! numbers until one of the three happens -- and is then consecutive again.

use crate::object::{Geometry, Object, ObjectId};
use crate::scene::Scene;

/// A badge's number, or `None` for any other object.
#[must_use]
pub const fn number_of(object: &Object) -> Option<u32> {
    match object.geometry {
        Geometry::Counter { number, .. } => Some(number),
        _ => None,
    }
}

/// The badges in the order they count.
///
/// By number, and a number held twice by the one made first. The scene keeps its objects
/// in `z` order and the sort is stable, so equal numbers stay in the order they were drawn.
#[must_use]
pub fn sequence(scene: &Scene) -> Vec<&Object> {
    let mut badges: Vec<&Object> =
        scene.objects().iter().filter(|o| number_of(o).is_some()).collect();
    badges.sort_by_key(|o| number_of(o).unwrap_or(0));
    badges
}

/// Where the sequence starts: its first badge's number, or `None` without badges.
#[must_use]
pub fn first(scene: &Scene) -> Option<u32> {
    sequence(scene).first().and_then(|o| number_of(o))
}

/// The badges that are not leaving, numbered on from the first badge's number -- the
/// first one there was, so deleting badge 1 makes badge 2 the new 1. Nothing when no badge
/// is leaving: deleting a rectangle does not renumber anything.
#[must_use]
pub fn closed_up(scene: &Scene, leaving: &[ObjectId]) -> Vec<Object> {
    let badge_leaves = leaving.iter().any(|id| scene.get(*id).and_then(number_of).is_some());
    let Some(start) = first(scene).filter(|_| badge_leaves) else { return Vec::new() };
    let staying: Vec<&Object> =
        sequence(scene).into_iter().filter(|o| !leaving.contains(&o.id)).collect();
    numbered(&staying, start)
}

/// Badge `id` given `number`: it moves to that place in the sequence and the badges in
/// between make room. The badge always ends up with `number`; the sequence starts lower
/// when `number` is below its first, and moves up when it is past its last.
#[must_use]
pub fn moved(scene: &Scene, id: ObjectId, number: u32) -> Vec<Object> {
    let order = sequence(scene);
    let Some(start) = order.first().and_then(|o| number_of(o)) else { return Vec::new() };
    let Some(badge) = order.iter().copied().find(|o| o.id == id) else { return Vec::new() };
    let mut others: Vec<&Object> = order.iter().copied().filter(|o| o.id != id).collect();
    // The place it is asked for, kept inside the sequence. Below the first is the front;
    // past the last is the end.
    let wanted = u64::from(number).saturating_sub(u64::from(start));
    let at = usize::try_from(wanted).unwrap_or(usize::MAX).min(others.len());
    others.insert(at, badge);
    // So the badge at `at` is numbered `number`.
    let from = number.saturating_sub(u32::try_from(at).unwrap_or(u32::MAX));
    numbered(&others, from)
}

/// The whole sequence started at `start`.
#[must_use]
pub fn started_at(scene: &Scene, start: u32) -> Vec<Object> {
    numbered(&sequence(scene), start)
}

/// New badges numbered on from the sequence, in the order given. Every other object is left
/// as it is. Answers how many badges were numbered.
pub fn appended(scene: &Scene, objects: &mut [Object]) -> usize {
    let mut next = scene.next_counter_number();
    let mut count = 0;
    for object in objects {
        if let Geometry::Counter { number, .. } = &mut object.geometry {
            *number = next;
            next = next.saturating_add(1);
            count += 1;
        }
    }
    count
}

/// `order` numbered from `start`, as the badges whose number that changes.
fn numbered(order: &[&Object], start: u32) -> Vec<Object> {
    let mut next = start;
    let mut changed = Vec::new();
    for badge in order {
        if number_of(badge) != Some(next) {
            let mut after = (*badge).clone();
            if let Geometry::Counter { number, .. } = &mut after.geometry {
                *number = next;
            }
            changed.push(after);
        }
        next = next.saturating_add(1);
    }
    changed
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "a failing assertion should panic")]
mod tests {
    use super::*;
    use crate::geometry::Point;
    use crate::scene::Base;
    use crate::style::{CounterStyle, Style};

    fn scene_of(numbers: &[u32]) -> (Scene, Vec<ObjectId>) {
        let mut scene = Scene::new(Base::new("x.png", 100.0, 100.0, 1.0));
        let mut ids = Vec::new();
        for (z, number) in numbers.iter().enumerate() {
            let badge = Object::new(
                i32::try_from(z).expect("a small z"),
                0,
                Style::default(),
                Geometry::Counter {
                    center: Point::new(0.0, 0.0),
                    number: *number,
                    style: CounterStyle::Arabic,
                    radius: 20.0,
                    tail: None,
                },
            );
            ids.push(badge.id);
            scene.add(badge);
        }
        (scene, ids)
    }

    /// The numbers after `changes`, in the order the badges were made.
    fn after(
        scene: &Scene,
        ids: &[ObjectId],
        changes: &[Object],
        leaving: &[ObjectId],
    ) -> Vec<u32> {
        ids.iter()
            .filter(|id| !leaving.contains(id))
            .map(|id| {
                let now = changes.iter().find(|o| o.id == *id).or_else(|| scene.get(*id));
                now.and_then(number_of).expect("a badge")
            })
            .collect()
    }

    #[test]
    fn deleting_a_badge_closes_the_gap() {
        let (scene, ids) = scene_of(&[1, 2, 3, 4]);
        let leaving = [ids[1]];
        let changes = closed_up(&scene, &leaving);
        assert_eq!(after(&scene, &ids, &changes, &leaving), [1, 2, 3]);
        assert_eq!(changes.len(), 2, "only the badges after the gap change");
    }

    #[test]
    fn deleting_the_first_keeps_the_start() {
        let (scene, ids) = scene_of(&[5, 6, 7]);
        let leaving = [ids[0]];
        let changes = closed_up(&scene, &leaving);
        assert_eq!(after(&scene, &ids, &changes, &leaving), [5, 6]);
    }

    #[test]
    fn deleting_something_else_renumbers_nothing() {
        // Hand-numbered, as a document from before this may be.
        let (mut scene, _) = scene_of(&[1, 5, 9]);
        let rect = Object::new(
            9,
            0,
            Style::default(),
            Geometry::Ellipse { bounds: crate::geometry::Bounds::new(0.0, 0.0, 9.0, 9.0) },
        );
        let id = rect.id;
        scene.add(rect);
        assert!(closed_up(&scene, &[id]).is_empty());
    }

    #[test]
    fn deleting_every_badge_changes_nothing() {
        let (scene, ids) = scene_of(&[1, 2]);
        assert!(closed_up(&scene, &ids).is_empty());
    }

    #[test]
    fn a_badge_given_an_earlier_number_moves_there() {
        let (scene, ids) = scene_of(&[1, 2, 3, 4]);
        let changes = moved(&scene, ids[3], 2);
        assert_eq!(after(&scene, &ids, &changes, &[]), [1, 3, 4, 2]);
    }

    #[test]
    fn a_badge_given_a_later_number_moves_there() {
        let (scene, ids) = scene_of(&[1, 2, 3, 4]);
        let changes = moved(&scene, ids[0], 3);
        assert_eq!(after(&scene, &ids, &changes, &[]), [3, 1, 2, 4]);
    }

    #[test]
    fn a_number_below_the_first_starts_the_sequence_there() {
        let (scene, ids) = scene_of(&[1, 2, 3]);
        let changes = moved(&scene, ids[1], 0);
        assert_eq!(after(&scene, &ids, &changes, &[]), [1, 0, 2]);
    }

    #[test]
    fn a_number_past_the_last_moves_the_sequence_up_to_it() {
        let (scene, ids) = scene_of(&[1, 2, 3]);
        let changes = moved(&scene, ids[0], 10);
        assert_eq!(after(&scene, &ids, &changes, &[]), [10, 8, 9]);
        // One badge alone takes whatever it is given.
        let (scene, ids) = scene_of(&[1]);
        let changes = moved(&scene, ids[0], 7);
        assert_eq!(after(&scene, &ids, &changes, &[]), [7]);
    }

    #[test]
    fn the_same_number_changes_nothing() {
        let (scene, ids) = scene_of(&[1, 2, 3]);
        assert!(moved(&scene, ids[1], 2).is_empty());
    }

    #[test]
    fn the_start_moves_the_whole_sequence() {
        let (scene, ids) = scene_of(&[1, 2, 3]);
        let changes = started_at(&scene, 0);
        assert_eq!(after(&scene, &ids, &changes, &[]), [0, 1, 2]);
    }

    #[test]
    fn numbers_held_twice_count_in_the_order_they_were_made() {
        // Two 2s, as a copy made before this kept its number: the first one drawn is 2.
        let (scene, ids) = scene_of(&[1, 2, 2]);
        let changes = started_at(&scene, 1);
        assert_eq!(after(&scene, &ids, &changes, &[]), [1, 2, 3]);
    }

    #[test]
    fn hand_numbered_badges_are_consecutive_after_a_delete() {
        let (scene, ids) = scene_of(&[1, 5, 9]);
        let leaving = [ids[1]];
        let changes = closed_up(&scene, &leaving);
        assert_eq!(after(&scene, &ids, &changes, &leaving), [1, 2]);
    }

    #[test]
    fn copies_take_the_next_numbers() {
        let (scene, _) = scene_of(&[1, 2]);
        let (copies, _) = scene_of(&[1, 2]);
        let mut copies: Vec<Object> = copies.objects().to_vec();
        assert_eq!(appended(&scene, &mut copies), 2);
        let numbers: Vec<u32> = copies.iter().filter_map(number_of).collect();
        assert_eq!(numbers, [3, 4]);
    }
}
