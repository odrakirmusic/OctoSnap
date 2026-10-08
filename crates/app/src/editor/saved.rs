// SPDX-License-Identifier: GPL-3.0-or-later

//! What of the document has left the editor: the unsaved dot's question (`spec/13` #11),
//! and whether a card the editor leaves behind has its picture on disk already (D47).
//!
//! Both are kept as documents, not as undo depths (D168). A depth counts steps, and an
//! undo followed by a new mark brings it back to the number it was saved at with a
//! different picture: a Save at depth 3, Ctrl+Z and a new rectangle put the dot out, and
//! the × left a card offering Trash for a file that lacked the rectangle. Two scenes are
//! equal exactly when they draw the same picture -- a `Scene` holds no rasters or caches,
//! and a command undoes and redoes by putting whole objects back -- which is the question
//! both of these ask. D167 answered it the same way for the project a render carries.

use std::path::{Path, PathBuf};

use octosnap_scene::Scene;

/// The document at the last Copy, Save or Pin, and at the last Save that wrote a file.
#[derive(Debug, Clone, Default)]
pub struct Saved {
    last: Option<Scene>,
    file: Option<(Scene, PathBuf)>,
}

impl Saved {
    /// `scene` has left the editor, or is to count as having left it: a background the
    /// preferences applied is not a mark the user made.
    pub fn mark(&mut self, scene: Scene) {
        self.last = Some(scene);
    }

    /// [`Self::mark`] for a Save, which also remembers the file when it names one.
    pub fn mark_to(&mut self, scene: Scene, path: Option<&Path>) {
        if let Some(path) = path {
            self.file = Some((scene.clone(), path.to_path_buf()));
        }
        self.last = Some(scene);
    }

    /// Whether `scene` is what last left the editor.
    #[must_use]
    pub fn holds(&self, scene: &Scene) -> bool {
        self.last.as_ref() == Some(scene)
    }

    /// The file that holds `scene`'s picture, when the last Save was of exactly `scene`.
    #[must_use]
    pub fn file_of(&self, scene: &Scene) -> Option<&Path> {
        self.file.as_ref().filter(|(saved, _)| saved == scene).map(|(_, path)| path.as_path())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use octosnap_scene::{Base, Bounds, Command, Geometry, History, Object, Rgba, Style};

    fn rect(x: f64) -> Command {
        Command::Add(Object::new(
            0,
            0,
            Style::new(Rgba::new(1.0, 0.0, 0.0, 1.0), 3, false),
            Geometry::Rect { bounds: Bounds::new(x, 0.0, 100.0, 100.0), filled: false, radius: 0.0 },
        ))
    }

    /// The report's steps: Save at depth 3, Ctrl+Z, a new rectangle. The depth is 3
    /// again and the picture is not the saved one, so the dot is on and no card may
    /// claim the file.
    #[test]
    fn an_undo_and_a_new_mark_are_not_the_saved_document() {
        let mut scene = Scene::new(Base::new("base.png", 1000.0, 1000.0, 1.0));
        let mut history = History::new();
        for x in [0.0, 200.0, 400.0] {
            assert!(history.apply(&mut scene, rect(x)));
            history.end_gesture();
        }
        let mut saved = Saved::default();
        saved.mark_to(scene.clone(), Some(Path::new("/tmp/Shot.png")));
        let at = history.depth();
        assert!(saved.holds(&scene));
        assert_eq!(saved.file_of(&scene), Some(Path::new("/tmp/Shot.png")));

        assert!(history.undo(&mut scene));
        assert!(history.apply(&mut scene, rect(600.0)));
        assert_eq!(history.depth(), at, "the depth alone cannot tell the two apart");
        assert!(!saved.holds(&scene), "the new rectangle never left the editor");
        assert_eq!(saved.file_of(&scene), None, "the file lacks the new rectangle");
    }

    /// And the other way: an undo and a redo come back to exactly the saved document, so
    /// the dot goes out again and the card has its file.
    #[test]
    fn an_undo_and_a_redo_are_the_saved_document() {
        let mut scene = Scene::new(Base::new("base.png", 1000.0, 1000.0, 1.0));
        let mut history = History::new();
        for x in [0.0, 200.0] {
            assert!(history.apply(&mut scene, rect(x)));
            history.end_gesture();
        }
        let mut saved = Saved::default();
        saved.mark_to(scene.clone(), Some(Path::new("/tmp/Shot.png")));

        assert!(history.undo(&mut scene));
        assert!(!saved.holds(&scene));
        assert!(history.redo(&mut scene));
        assert!(saved.holds(&scene));
        assert_eq!(saved.file_of(&scene), Some(Path::new("/tmp/Shot.png")));
    }

    /// A Copy after a Save moves the dot's document on and leaves the file where it was:
    /// the file still holds the picture it was saved with, and only that one.
    #[test]
    fn a_copy_keeps_the_file_for_the_document_it_holds() {
        let mut scene = Scene::new(Base::new("base.png", 1000.0, 1000.0, 1.0));
        let mut history = History::new();
        assert!(history.apply(&mut scene, rect(0.0)));
        let mut saved = Saved::default();
        saved.mark_to(scene.clone(), Some(Path::new("/tmp/Shot.png")));
        let on_disk = scene.clone();

        assert!(history.apply(&mut scene, rect(200.0)));
        saved.mark(scene.clone());
        assert!(saved.holds(&scene));
        assert_eq!(saved.file_of(&scene), None);
        assert_eq!(saved.file_of(&on_disk), Some(Path::new("/tmp/Shot.png")));

        // A Save that names no file is still a Save for the dot, and keeps the old file.
        assert!(history.apply(&mut scene, rect(400.0)));
        saved.mark_to(scene.clone(), None);
        assert!(saved.holds(&scene));
        assert_eq!(saved.file_of(&on_disk), Some(Path::new("/tmp/Shot.png")));
    }
}
