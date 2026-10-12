// SPDX-License-Identifier: GPL-3.0-or-later

//! `ANN-30` (D186): the layers panel's list, and what its edits are.
//!
//! The list is the document's marks in the order they are drawn, the topmost first, hidden
//! ones included so they can be shown again; the background and the crop are left out,
//! because they are the canvas and not things on it. Every mark belongs to a group --
//! redactions and pictures under the spotlight, the spotlight under the shapes, badges over
//! everything (`spec/05` §5.3) -- and the list keeps a group's marks together, so a mark
//! moved in it moves among its own group only, and to the group's end when it is dragged
//! past it. What a move changes is the marks' `z`, as Bring to Front does: one `Reorder`
//! for each mark whose place in the group changed, all one step.

use crate::command::Command;
use crate::object::{Geometry, Object, ObjectId, ObjectKind};
use crate::scene::{Scene, render_group};

/// The marks, topmost first.
#[must_use]
pub fn rows(scene: &Scene) -> Vec<ObjectId> {
    scene.stack().into_iter().rev().filter(|o| listed(o)).map(|o| o.id).collect()
}

/// Whether a mark is in the list: everything but the canvas itself.
fn listed(object: &Object) -> bool {
    !matches!(object.kind(), ObjectKind::Background | ObjectKind::Crop)
}

/// The edits that put `id` at `index` of [`rows`], kept inside its group: the group's
/// marks take the group's own `z`s again, in their new order. Empty when it is there
/// already, or is not in the list.
#[must_use]
pub fn move_to(scene: &Scene, id: ObjectId, index: usize) -> Vec<Command> {
    let Some(object) = scene.get(id).filter(|o| listed(o)) else { return Vec::new() };
    let group = render_group(object.kind());
    let rows = rows(scene);
    let members: Vec<&Object> = rows
        .iter()
        .filter_map(|row| scene.get(*row))
        .filter(|o| render_group(o.kind()) == group)
        .collect();
    let Some(first) = rows.iter().position(|row| *row == members[0].id) else {
        return Vec::new();
    };
    let Some(from) = members.iter().position(|o| o.id == id) else { return Vec::new() };
    let to = index.saturating_sub(first).min(members.len() - 1);
    if to == from {
        return Vec::new();
    }
    let mut order = members.clone();
    let moving = order.remove(from);
    order.insert(to, moving);
    // The group's own `z`s, highest first, made strictly falling so no two tie: a tie is
    // drawn in the order the marks were made, which is not the order just asked for.
    let mut zs: Vec<i32> = members.iter().map(|o| o.z).collect();
    zs.sort_unstable_by(|a, b| b.cmp(a));
    for i in 1..zs.len() {
        zs[i] = zs[i].min(zs[i - 1].saturating_sub(1));
    }
    order
        .iter()
        .zip(zs)
        .filter(|(o, z)| o.z != *z)
        .map(|(o, z)| Command::Reorder { id: o.id, before: o.z, after: z })
        .collect()
}

/// [`move_to`] one place up the list (towards the top, `up`) or down it.
#[must_use]
pub fn step(scene: &Scene, id: ObjectId, up: bool) -> Vec<Command> {
    let rows = rows(scene);
    let Some(at) = rows.iter().position(|row| *row == id) else { return Vec::new() };
    let to = if up { at.saturating_sub(1) } else { at + 1 };
    if to == at || to >= rows.len() {
        return Vec::new();
    }
    move_to(scene, id, to)
}

/// What a mark's row says: a label's first line, a badge's number, or what the mark is.
#[must_use]
pub fn label(object: &Object) -> String {
    match &object.geometry {
        Geometry::Text { text, .. } => {
            let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
            if line.is_empty() {
                "Text".to_owned()
            } else if line.chars().count() > LABEL_MOST {
                let cut: String = line.chars().take(LABEL_MOST - 1).collect();
                format!("{}…", cut.trim_end())
            } else {
                line.to_owned()
            }
        }
        Geometry::Counter { number, style, .. } => format!("Badge {}", style.format(*number)),
        Geometry::Arrow { .. } => "Arrow".to_owned(),
        Geometry::Line { .. } => "Line".to_owned(),
        Geometry::Rect { filled: true, .. } => "Filled Rectangle".to_owned(),
        Geometry::Rect { .. } => "Rectangle".to_owned(),
        Geometry::Ellipse { .. } => "Ellipse".to_owned(),
        Geometry::Path { highlighter: true, .. } => "Highlighter".to_owned(),
        Geometry::Path { .. } => "Pencil".to_owned(),
        Geometry::Spotlight { .. } => "Spotlight".to_owned(),
        Geometry::Magnifier { .. } => "Magnifier".to_owned(),
        Geometry::Redact { .. } => "Redaction".to_owned(),
        Geometry::Image { .. } => "Picture".to_owned(),
        Geometry::Background { .. } => "Background".to_owned(),
        Geometry::Crop { .. } => "Crop".to_owned(),
    }
}

/// The most characters a label's row shows of it.
const LABEL_MOST: usize = 32;

/// Whether a mark can be hidden. A redaction cannot: hidden, it would be a capture with its
/// secret showing, one export away from being sent -- the reason a vector export burns
/// redactions into the picture (D184) -- so it is always drawn.
#[must_use]
pub fn hideable(object: &Object) -> bool {
    listed(object) && object.kind() != ObjectKind::Redact
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::geometry::{Bounds, Point};
    use crate::style::{CounterStyle, Rgba, Style};

    fn add(scene: &mut Scene, z: i32, geometry: Geometry) -> ObjectId {
        let object =
            Object::new(z, 0, Style::new(Rgba::new(1.0, 0.0, 0.0, 1.0), 3, false), geometry);
        let id = object.id;
        scene.add(object);
        id
    }

    fn rect(x: f64) -> Geometry {
        Geometry::Rect { bounds: Bounds::new(x, 0.0, 10.0, 10.0), filled: false, radius: 0.0 }
    }

    fn badge(n: u32) -> Geometry {
        Geometry::Counter {
            center: Point::new(f64::from(n) * 30.0, 0.0),
            number: n,
            style: CounterStyle::default(),
            radius: 12.0,
            tail: None,
        }
    }

    fn redaction() -> Geometry {
        Geometry::Redact {
            bounds: Bounds::new(0.0, 0.0, 40.0, 40.0),
            style: crate::style::RedactStyle::default(),
            intensity: 0.5,
            seed: 1,
        }
    }

    /// Three rectangles, a badge, a redaction, and the scene's order applied.
    fn document() -> (Scene, [ObjectId; 5]) {
        let mut scene = Scene::new(crate::Base::new("base.png", 100.0, 100.0, 1.0));
        let a = add(&mut scene, 1, rect(0.0));
        let b = add(&mut scene, 2, rect(20.0));
        let c = add(&mut scene, 3, rect(40.0));
        let badge = add(&mut scene, 4, badge(1));
        let redaction = add(&mut scene, 5, redaction());
        (scene, [a, b, c, badge, redaction])
    }

    fn apply(scene: &mut Scene, commands: Vec<Command>) {
        assert!(crate::History::new().apply(scene, Command::Batch(commands)));
    }

    #[test]
    fn the_list_is_the_drawing_order_topmost_first() {
        let (scene, [a, b, c, badge, redaction]) = document();
        // Badges over shapes over redactions, whatever the `z`s say.
        assert_eq!(rows(&scene), vec![badge, c, b, a, redaction]);
    }

    #[test]
    fn a_mark_moves_within_its_group() {
        let (mut scene, [a, b, c, badge, redaction]) = document();
        // The bottom rectangle to the top of the rectangles: row 1, under the badge.
        let commands = move_to(&scene, a, 1);
        assert_eq!(commands.len(), 3, "{commands:?}");
        apply(&mut scene, commands);
        assert_eq!(rows(&scene), vec![badge, a, c, b, redaction]);
        // Dragged to the very top it stops at its group's top, under the badge.
        let commands = move_to(&scene, b, 0);
        apply(&mut scene, commands);
        assert_eq!(rows(&scene), vec![badge, b, a, c, redaction]);
        // And dragged past the bottom, at its group's bottom, over the redaction.
        let commands = move_to(&scene, b, 9);
        apply(&mut scene, commands);
        assert_eq!(rows(&scene), vec![badge, a, c, b, redaction]);
        assert!(move_to(&scene, b, 3).is_empty(), "it is there already");
    }

    #[test]
    fn a_step_goes_one_place_and_not_out_of_the_group() {
        let (mut scene, [a, b, c, badge, redaction]) = document();
        let commands = step(&scene, b, true);
        apply(&mut scene, commands);
        assert_eq!(rows(&scene), vec![badge, b, c, a, redaction]);
        assert!(step(&scene, b, true).is_empty(), "the badge's group is above");
        assert!(step(&scene, badge, true).is_empty(), "the top");
        assert!(step(&scene, redaction, false).is_empty(), "the bottom");
    }

    /// Two marks on the same `z` are drawn in the order they were made; a move gives every
    /// mark of the group a `z` of its own, so the order asked for is the order drawn.
    #[test]
    fn a_move_undoes_a_tie() {
        let mut scene = Scene::new(crate::Base::new("base.png", 100.0, 100.0, 1.0));
        let a = add(&mut scene, 7, rect(0.0));
        let b = add(&mut scene, 7, rect(20.0));
        let c = add(&mut scene, 7, rect(40.0));
        assert_eq!(rows(&scene), vec![c, b, a]);
        let commands = move_to(&scene, a, 0);
        apply(&mut scene, commands);
        assert_eq!(rows(&scene), vec![a, c, b]);
        let mut zs: Vec<i32> = scene.objects().iter().map(|o| o.z).collect();
        zs.sort_unstable();
        zs.dedup();
        assert_eq!(zs.len(), 3);
    }

    #[test]
    fn a_hidden_mark_is_listed_and_not_drawn_but_a_redaction_always_is() {
        let (mut scene, [a, _, _, _, redaction]) = document();
        for id in [a, redaction] {
            let mut hidden = scene.get(id).unwrap().clone();
            hidden.hidden = true;
            scene.replace(hidden);
        }
        assert!(rows(&scene).contains(&a));
        assert!(!scene.render_order().contains(&a));
        assert!(scene.render_order().contains(&redaction), "a redaction is always drawn");
        assert!(!hideable(scene.get(redaction).unwrap()));
        assert!(hideable(scene.get(a).unwrap()));
        assert_eq!(crate::hit::topmost(&scene, Point::new(1.0, 5.0)), Some(redaction));
    }

    #[test]
    fn a_row_says_what_the_mark_is() {
        let (scene, [a, _, _, badge, redaction]) = document();
        assert_eq!(label(scene.get(a).unwrap()), "Rectangle");
        assert_eq!(label(scene.get(badge).unwrap()), "Badge 1");
        assert_eq!(label(scene.get(redaction).unwrap()), "Redaction");
        let mut scene = scene;
        let words = add(
            &mut scene,
            9,
            Geometry::Text {
                pos: Point::new(0.0, 0.0),
                text: "\n  Click the big blue button to save everything you did today\nmore"
                    .to_owned(),
                style: crate::style::TextStyle::default(),
                font_size: 20.0,
                align: crate::object::TextAlign::Start,
                width: None,
                tail: None,
                ink: None,
                background: None,
            },
        );
        assert_eq!(label(scene.get(words).unwrap()), "Click the big blue button to sa…");
    }

    /// `objects.json` keeps no `hidden` for a mark that is not: a document from before,
    /// and one with nothing hidden, are written as they always were.
    #[test]
    fn only_a_hidden_mark_writes_hidden() {
        let (scene, [a, ..]) = document();
        let mut object = scene.get(a).unwrap().clone();
        assert!(!serde_json::to_string(&object).unwrap().contains("hidden"));
        object.hidden = true;
        let json = serde_json::to_string(&object).unwrap();
        assert!(json.contains(r#""hidden":true"#), "{json}");
        assert_eq!(serde_json::from_str::<Object>(&json).unwrap(), object);
    }
}
