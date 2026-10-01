// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §8's `.octosnap` project: a zip with the document inside it.
//!
//! ```text
//! manifest.json    { "format": 1, "app": "octosnap", "created": …, "scale": 2.0, "canvas": {…}, "base": "base.png" }
//! base.png         original capture (alpha preserved)
//! objects.json     ordered objects (§5.1)
//! assets/…         inserted images, custom backgrounds referenced by objects
//! thumbnail.png    256 px preview for file managers
//! ```
//!
//! In this crate rather than in the app, because `spec/11` puts it here — "`scene` crate:
//! objects, commands/undo, render-node builders, hit-testing, export via `render_texture`,
//! **project file `.octosnap`**" — and because the container is entirely about the
//! document. The one part that needs a GPU is the **thumbnail**, and it arrives as bytes
//! the caller has already rendered; nothing here draws.
//!
//! Two rules carried over from the rest of the crate:
//!
//! **Images cross as files, never as byte arrays** (`spec/10` §3). [`write`] takes the
//! base image's *path* and copies it in; [`read`] extracts it to a directory the caller
//! names and hands back the path. So a 5K capture is never a `Vec<u8>` in the middle of
//! this crate, and the canvas goes on loading a texture from a file the way it always has.
//!
//! **`objects.json` is written by [`Scene::to_objects_json`]**, which is what
//! `spec/05` §11 item 4 compares byte for byte. A project is that same string in a zip
//! entry, so a round trip through a file is a round trip through the same serialisation —
//! there is no second encoder to disagree with the first.

use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::geometry::Bounds;
use crate::scene::{Base, Scene};
use crate::transform::Orientation;

/// The container's version, in `manifest.json`'s `format`.
///
/// One, and the first thing [`read`] checks. A project written by a later version is
/// refused by name rather than parsed optimistically into a document with pieces missing —
/// `spec/05` §8's whole promise is that reopening "restores full editability", and half a
/// document does not.
pub const FORMAT: u32 = 1;

// `spec/05` §8's extension, MIME type and thumbnail size come from `octosnap_core`,
// which the CLI and the `.desktop` file also read: what a project is *called* is shared
// vocabulary, and only what is *inside* it is this module's business.
pub use octosnap_core::project::{EXTENSION, LEGACY_EXTENSION, MIME_TYPE, THUMBNAIL_SIZE, is_project};

const MANIFEST: &str = "manifest.json";
const OBJECTS: &str = "objects.json";
const THUMBNAIL: &str = "thumbnail.png";
const ASSETS: &str = "assets/";

/// `spec/05` §8's `manifest.json`, field for field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub app: String,
    /// Seconds since the epoch, like `spec/05` §5.1's `created` on an object.
    pub created: u64,
    pub scale: f64,
    pub canvas: Bounds,
    /// The base image's name *inside the zip*, never a path into the user's filesystem.
    pub base: String,
    /// The base image's own rect in document units, which is the canvas only until
    /// `spec/05` §4.11's crop or §4.13's padding has moved one of them (D52, D89).
    ///
    /// Added with §4.12's transforms and defaulted on read, so a project written before
    /// it opens the way it always did -- with the canvas standing in for the base, which
    /// is right for every project that was never cropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_size: Option<Bounds>,
    /// `spec/05` §4.12's Rotate and Flip (D91).
    #[serde(default, skip_serializing_if = "Orientation::is_upright")]
    pub orientation: Orientation,
}

impl Manifest {
    /// The manifest for a scene, with the base named as it will be stored.
    #[must_use]
    pub fn of(scene: &Scene, created: u64) -> Self {
        Self {
            format: FORMAT,
            app: "octosnap".to_owned(),
            created,
            scale: scene.base.scale,
            canvas: scene.canvas,
            base: base_name(&scene.base.file),
            base_size: Some(scene.base.bounds()),
            orientation: scene.base.orientation,
        }
    }
}

/// What can go wrong with a project file.
///
/// Distinct variants because the caller's answer differs for each: a missing entry is a
/// file that is not a project, a future `format` is a file this build is too old for, and
/// an I/O error is the disk. A single "could not open" would make all three look alike in
/// a notification.
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Zip(zip::result::ZipError),
    Json(serde_json::Error),
    /// A required entry is not in the archive.
    Missing(&'static str),
    /// `manifest.json` says a `format` this build does not know.
    Format(u32),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Zip(e) => write!(f, "{e}"),
            Self::Json(e) => write!(f, "{e}"),
            Self::Missing(entry) => write!(f, "the project has no {entry}"),
            Self::Format(version) => write!(
                f,
                "the project is format {version} and this build understands {FORMAT}"
            ),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<zip::result::ZipError> for Error {
    fn from(e: zip::result::ZipError) -> Self {
        Self::Zip(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// Writes `spec/05` §8's container.
///
/// `base` is the capture's path, read and stored under the name the manifest gives.
/// `thumbnail` is PNG bytes the caller rendered — `None` is allowed and leaves the entry
/// out, which is what an editor with no surface can honestly produce, and `spec/05` §11
/// item 8 asks for it to be there in the case that matters.
///
/// Stored **deflated**, and the base image is a PNG that is already compressed: the win is
/// on `objects.json`, which is pretty-printed for the reason §11 item 4 gives and
/// compresses by about ten to one.
pub fn write(
    path: &Path,
    scene: &Scene,
    base: &Path,
    thumbnail: Option<&[u8]>,
    created: u64,
) -> Result<(), Error> {
    let file = std::fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let manifest = Manifest::of(scene, created);
    zip.start_file(MANIFEST, options)?;
    zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;

    // `spec/05` §4.14's inserted images. Copied in and **renamed in the objects**, which
    // is the whole of §8's "a name inside the project's `assets/`, never a path into the
    // user's filesystem": a project that referred to `/home/someone/Pictures/logo.png`
    // would open on the machine it was made on and nowhere else.
    let (scene, assets) = with_assets(scene);
    zip.start_file(OBJECTS, options)?;
    zip.write_all(scene.to_objects_json()?.as_bytes())?;

    for (name, from) in &assets {
        // Stored rather than deflated, for the reason the base is: these are PNGs and
        // JPEGs, and deflating them again costs CPU to make them bigger.
        zip.start_file(
            format!("{ASSETS}{name}"),
            options.compression_method(zip::CompressionMethod::Stored),
        )?;
        let mut source = std::fs::File::open(from)?;
        std::io::copy(&mut source, &mut zip)?;
    }

    // The base image, copied rather than re-encoded: §8 says "original capture (alpha
    // preserved)", and a re-encode is exactly how alpha gets lost.
    //
    // **Stored, not deflated.** A PNG is already compressed, and deflating one again made
    // the 324 021-byte base in the first project written by this code come out at 324 121
    // -- a hundred bytes *larger*, for the CPU cost of compressing a third of a megabyte.
    // The thumbnail is a PNG too and is left deflated only because it is small enough for
    // the difference not to be worth a second code path.
    zip.start_file(
        &manifest.base,
        options.compression_method(zip::CompressionMethod::Stored),
    )?;
    let mut source = std::fs::File::open(base)?;
    std::io::copy(&mut source, &mut zip)?;

    if let Some(bytes) = thumbnail {
        zip.start_file(THUMBNAIL, options)?;
        zip.write_all(bytes)?;
    }

    zip.finish()?;
    Ok(())
}

/// The scene with every image object's file renamed to its name inside `assets/`, and the
/// list of files to copy in under those names.
///
/// De-duplicated by path, so a picture dropped twice is stored once; and made unique by
/// name, so two different files both called `logo.png` do not become one.
fn with_assets(scene: &Scene) -> (Scene, Vec<(String, PathBuf)>) {
    let mut out = scene.clone();
    let mut assets: Vec<(String, PathBuf)> = Vec::new();
    let ids: Vec<_> = out.objects().iter().map(|object| object.id).collect();
    for id in ids {
        let Some(object) = out.get_mut(id) else { continue };
        let crate::object::Geometry::Image { file, .. } = &mut object.geometry else { continue };
        let from = PathBuf::from(&*file);
        if let Some((name, _)) = assets.iter().find(|(_, path)| *path == from) {
            *file = name.clone();
            continue;
        }
        let stem = safe_name(&base_name(file)).unwrap_or_else(|| "image.png".to_owned());
        let mut name = stem.clone();
        let mut next = 1_u32;
        while assets.iter().any(|(taken, _)| *taken == name) {
            name = format!("{next}-{stem}");
            next = next.saturating_add(1);
        }
        *file = name.clone();
        assets.push((name, from));
    }
    (out, assets)
}

/// What [`read`] recovered.
#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    pub scene: Scene,
    /// Where the base image was extracted to. The canvas loads a texture from this.
    pub base: PathBuf,
    pub manifest: Manifest,
    /// Whether the archive carried `spec/05` §8's thumbnail, which §11 item 8 checks.
    pub has_thumbnail: bool,
}

/// Reads `spec/05` §8's container, extracting the base image into `into`.
///
/// The scene comes back with its `base.file` pointing at the extracted path, so the result
/// is a document the editor can open with no further arrangement — which is what §8's
/// "opening a project from a card or history restores full editability" asks for.
pub fn read(path: &Path, into: &Path) -> Result<Opened, Error> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))?;

    let manifest: Manifest = serde_json::from_str(&entry_to_string(&mut zip, MANIFEST)?)?;
    if manifest.format != FORMAT {
        return Err(Error::Format(manifest.format));
    }

    // The base's name is checked rather than trusted. A zip entry is attacker-controlled
    // text, and `..` in it is the path-traversal that turns "open this project" into
    // "write anywhere the user can" -- the reason §8 says assets are "a name inside the
    // project's `assets/`, never a path into the user's filesystem".
    let base_name = safe_name(&manifest.base).ok_or(Error::Missing("a usable base image"))?;
    let objects = entry_to_string(&mut zip, OBJECTS)?;
    let has_thumbnail = zip.index_for_name(THUMBNAIL).is_some();

    std::fs::create_dir_all(into)?;
    let base_path = into.join(&base_name);
    {
        let mut entry = zip
            .by_name(&manifest.base)
            .map_err(|_| Error::Missing("its base image"))?;
        let mut out = std::fs::File::create(&base_path)?;
        std::io::copy(&mut entry, &mut out)?;
    }

    // `spec/05` §8's `assets/`, extracted beside the base for the same reason: an object
    // referring to one names it, and the canvas loads it from a file.
    for index in 0..zip.len() {
        let Ok(mut entry) = zip.by_index(index) else { continue };
        let name = entry.name().to_owned();
        let Some(asset) = name.strip_prefix(ASSETS) else { continue };
        let Some(asset) = safe_name(asset) else { continue };
        if asset.is_empty() {
            continue;
        }
        let out_dir = into.join("assets");
        std::fs::create_dir_all(&out_dir)?;
        let mut out = std::fs::File::create(out_dir.join(&asset))?;
        std::io::copy(&mut entry, &mut out)?;
    }

    // The base's own size where the manifest records it, and the canvas where it does
    // not: every project written before §4.12's transforms existed has the two the same
    // unless it was cropped, and a cropped one reopened as its crop is the behaviour that
    // shipped.
    let rect = manifest.base_size.unwrap_or(manifest.canvas).normalised();
    let mut scene = Scene::new(Base::new(
        base_path.to_string_lossy().into_owned(),
        rect.width,
        rect.height,
        manifest.scale,
    ));
    scene.base.orientation = manifest.orientation;
    scene.load_objects_json(&objects)?;
    // §4.14's images come back pointing at where they were extracted, so a reopened
    // project is a document the canvas can draw with no further arrangement -- the same
    // promise `Opened::base` makes for the capture.
    let assets_dir = into.join("assets");
    let ids: Vec<_> = scene.objects().iter().map(|object| object.id).collect();
    for id in ids {
        let Some(object) = scene.get_mut(id) else { continue };
        let crate::object::Geometry::Image { file, .. } = &mut object.geometry else { continue };
        if let Some(name) = safe_name(file) {
            *file = assets_dir.join(name).to_string_lossy().into_owned();
        }
    }
    // The canvas from the manifest, not from the base image's size: `spec/05` §4.11's crop
    // can have made them different, and D52 is the whole decision that they are not the
    // same rectangle.
    scene.canvas = manifest.canvas;

    Ok(Opened { scene, base: base_path, manifest, has_thumbnail })
}

/// The last component of a path, as a name inside the archive.
fn base_name(file: &str) -> String {
    Path::new(file)
        .file_name()
        .map_or_else(|| "base.png".to_owned(), |n| n.to_string_lossy().into_owned())
}

/// A zip entry's name reduced to a single safe component, or `None`.
///
/// Refuses anything with a separator, `..`, or a leading dot. A zip is a list of arbitrary
/// strings written by whoever made the file, and `../../.bashrc` is a valid one.
fn safe_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.starts_with('.')
    {
        return None;
    }
    Some(trimmed.to_owned())
}

fn entry_to_string<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &'static str,
) -> Result<String, Error> {
    let mut entry = zip.by_name(name).map_err(|_| Error::Missing(name))?;
    let mut text = String::new();
    entry.read_to_string(&mut text)?;
    Ok(text)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::object::{Geometry, Object};
    use crate::style::{Rgba, Style};
    use crate::{Command, History, Point};

    fn scene() -> Scene {
        let mut scene = Scene::new(Base::new("capture.png", 1600.0, 900.0, 2.0));
        for index in 0..5 {
            #[allow(clippy::cast_precision_loss)]
            let at = Bounds::new(index as f64 * 40.0, 20.0, 100.0, 60.0);
            scene.add(Object::new(
                index,
                1_757_500_000,
                Style::new(Rgba::new(1.0, 0.2, 0.2, 1.0), 3, false),
                Geometry::Rect { bounds: at, filled: index % 2 == 0, radius: 4.0 },
            ));
        }
        scene.add(Object::new(
            9,
            1_757_500_001,
            Style::new(Rgba::new(0.0, 0.4, 1.0, 1.0), 2, true),
            Geometry::Path {
                points: vec![Point::new(1.0, 2.0), Point::new(3.5, 4.25)],
                smoothing: true,
                highlighter: false,
                band: None,
            },
        ));
        scene
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("octosnap-project-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_base(dir: &Path) -> PathBuf {
        // Not a real PNG: this module copies bytes and never decodes them, which is
        // exactly the property being relied on for "alpha preserved".
        let path = dir.join("capture.png");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\n not really, but byte-for-byte").unwrap();
        path
    }

    /// `spec/05` §11 item 8: "Save as project -> reopen -> all objects editable; thumbnail
    /// present." The editable half is that the objects come back **identical**, which is
    /// the same comparison §11 item 4 makes.
    #[test]
    fn a_project_round_trips_its_objects_byte_for_byte() {
        let dir = temp("round-trip");
        let base = write_base(&dir);
        let mut original = scene();
        original.base.file = base.to_string_lossy().into_owned();
        let project = dir.join("doc.octosnap");

        write(&project, &original, &base, Some(b"thumbnail bytes"), 1_757_500_000).unwrap();
        let opened = read(&project, &dir.join("out")).unwrap();

        assert_eq!(
            opened.scene.to_objects_json().unwrap(),
            original.to_objects_json().unwrap(),
            "the objects are the same bytes they were saved as",
        );
        assert_eq!(opened.scene.len(), 6);
        assert!(opened.has_thumbnail, "§11 item 8 asks for the thumbnail");
        assert_eq!(opened.manifest.format, FORMAT);
        assert_eq!(opened.manifest.scale, 2.0);
    }

    /// `spec/05` §4.14's inserted images are part of §8's "self-contained": they travel in
    /// `assets/` and come back pointing at where they were extracted, never at the path on
    /// the machine the project was made on.
    #[test]
    fn an_inserted_image_travels_with_the_project() {
        let dir = temp("assets");
        let base = write_base(&dir);
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let picture = elsewhere.join("logo.png");
        std::fs::write(&picture, b"pretend PNG").unwrap();

        let mut original = scene();
        original.base.file = base.to_string_lossy().into_owned();
        original.add(Object::new(
            20,
            1_757_500_002,
            Style::default(),
            Geometry::Image {
                bounds: Bounds::new(10.0, 20.0, 100.0, 50.0),
                file: picture.to_string_lossy().into_owned(),
            },
        ));
        let project = dir.join("doc.octosnap");
        write(&project, &original, &base, None, 1).unwrap();
        // The scene handed in is not modified: the rename happens on a copy.
        let Geometry::Image { file, .. } = &original.objects().last().unwrap().geometry else {
            panic!("not an image")
        };
        assert_eq!(file, &picture.to_string_lossy().into_owned());

        let out = dir.join("out");
        let opened = read(&project, &out).unwrap();
        let Geometry::Image { file, .. } = &opened.scene.objects().last().unwrap().geometry else {
            panic!("not an image")
        };
        assert_eq!(std::path::Path::new(file), out.join("assets").join("logo.png"));
        assert_eq!(std::fs::read(file).unwrap(), b"pretend PNG");
    }

    /// `spec/05` §4.12's transforms survive the round trip, which they only do because
    /// the manifest records the base's own rect and the way its pixels lie in it. Before
    /// §4.12 the base was reconstructed from the canvas, and a rotated or cropped document
    /// came back the shape of its canvas with its picture stretched to fit.
    #[test]
    fn a_rotated_and_cropped_document_reopens_the_shape_it_was_saved() {
        let dir = temp("orientation");
        let base = write_base(&dir);
        let mut original = scene();
        original.base.file = base.to_string_lossy().into_owned();
        original.base.width = 900.0;
        original.base.height = 1600.0;
        original.base.orientation = crate::transform::Orientation { quarter: 1, mirrored: true };
        original.canvas = Bounds::new(40.0, 60.0, 500.0, 700.0);
        let project = dir.join("doc.octosnap");

        write(&project, &original, &base, None, 1).unwrap();
        let opened = read(&project, &dir.join("out")).unwrap();

        assert_eq!((opened.scene.base.width, opened.scene.base.height), (900.0, 1600.0));
        assert_eq!(opened.scene.base.orientation, original.base.orientation);
        assert_eq!(opened.scene.canvas, original.canvas);
    }

    /// The base image is copied, never re-encoded: §8 says "original capture (alpha
    /// preserved)", and a re-encode is how alpha gets lost.
    #[test]
    fn the_base_image_comes_back_byte_identical() {
        let dir = temp("base-bytes");
        let base = write_base(&dir);
        let before = std::fs::read(&base).unwrap();
        let mut s = scene();
        s.base.file = base.to_string_lossy().into_owned();
        let project = dir.join("doc.octosnap");

        write(&project, &s, &base, None, 1).unwrap();
        let opened = read(&project, &dir.join("out")).unwrap();

        assert_eq!(std::fs::read(&opened.base).unwrap(), before);
        assert!(!opened.has_thumbnail, "none was written, so none is reported");
    }

    /// The base image is **stored**, not deflated: a PNG is already compressed, and
    /// re-compressing the first one this code wrote made 324 021 bytes into 324 121.
    /// `objects.json` is the entry that actually compresses, by about six to one.
    #[test]
    fn the_base_is_stored_and_the_objects_are_deflated() {
        let dir = temp("methods");
        let base = write_base(&dir);
        let mut s = scene();
        s.base.file = base.to_string_lossy().into_owned();
        let project = dir.join("doc.octosnap");
        write(&project, &s, &base, None, 1).unwrap();

        let file = std::fs::File::open(&project).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let mut seen = 0;
        for index in 0..zip.len() {
            let entry = zip.by_index(index).unwrap();
            match entry.name() {
                "objects.json" | "manifest.json" => {
                    assert_eq!(entry.compression(), zip::CompressionMethod::Deflated);
                    seen += 1;
                }
                name if name.ends_with(".png") => {
                    assert_eq!(
                        entry.compression(),
                        zip::CompressionMethod::Stored,
                        "{name} is already compressed",
                    );
                    seen += 1;
                }
                _ => {}
            }
        }
        assert_eq!(seen, 3, "manifest, objects and the base were all checked");
    }

    /// D52: the canvas is not the base image, so a cropped project has to carry the canvas
    /// rather than re-derive it from the picture.
    #[test]
    fn a_cropped_canvas_survives_the_round_trip() {
        let dir = temp("cropped");
        let base = write_base(&dir);
        let mut s = scene();
        s.base.file = base.to_string_lossy().into_owned();
        // Larger than the base and moved off its origin, which is what §4.11's expanding
        // crop produces.
        s.canvas = Bounds::new(-120.0, -40.0, 1900.0, 1000.0);
        let project = dir.join("doc.octosnap");

        write(&project, &s, &base, None, 1).unwrap();
        let opened = read(&project, &dir.join("out")).unwrap();
        assert_eq!(opened.scene.canvas, s.canvas);
        assert_ne!(opened.scene.canvas, opened.scene.base.bounds());
    }

    /// `spec/05` §11 item 8's "all objects editable", as far as this crate can state it:
    /// the reopened document takes an edit and undoes it.
    #[test]
    fn a_reopened_project_is_editable() {
        let dir = temp("editable");
        let base = write_base(&dir);
        let mut s = scene();
        s.base.file = base.to_string_lossy().into_owned();
        let project = dir.join("doc.octosnap");
        write(&project, &s, &base, None, 1).unwrap();

        let mut opened = read(&project, &dir.join("out")).unwrap().scene;
        let before = opened.to_objects_json().unwrap();
        let subject = opened.objects().first().unwrap().clone();
        let mut moved = subject.clone();
        moved.translate(17.0, 5.0);

        let mut history = History::new();
        assert!(history.apply(
            &mut opened,
            Command::Change {
                id: subject.id,
                before: Box::new(subject),
                after: Box::new(moved),
            },
        ));
        assert_ne!(opened.to_objects_json().unwrap(), before);
        assert!(history.undo(&mut opened));
        assert_eq!(opened.to_objects_json().unwrap(), before);
    }

    /// A zip entry's name is text somebody else wrote. `../../.bashrc` is a valid one.
    #[test]
    fn an_entry_name_cannot_escape_the_directory_it_is_extracted_into() {
        assert_eq!(safe_name("base.png"), Some("base.png".to_owned()));
        for hostile in ["../base.png", "/etc/passwd", "a/b.png", "..", ".bashrc", "", "   "] {
            assert_eq!(safe_name(hostile), None, "{hostile} must be refused");
        }
        // And the manifest's own field goes through it.
        assert_eq!(base_name("/home/someone/.cache/x/capture.png"), "capture.png");
    }

    /// A project from a later version is refused by name rather than half-parsed.
    #[test]
    fn a_future_format_is_refused_rather_than_guessed_at() {
        let dir = temp("future");
        let base = write_base(&dir);
        let mut s = scene();
        s.base.file = base.to_string_lossy().into_owned();
        let project = dir.join("doc.octosnap");
        write(&project, &s, &base, None, 1).unwrap();

        // Rewrite the manifest with a format from the future.
        let mut future = Manifest::of(&s, 1);
        future.format = FORMAT + 1;
        let rewritten = dir.join("future.octosnap");
        {
            let file = std::fs::File::create(&rewritten).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            zip.start_file("manifest.json", options).unwrap();
            zip.write_all(serde_json::to_string(&future).unwrap().as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        match read(&rewritten, &dir.join("out")) {
            Err(Error::Format(version)) => assert_eq!(version, FORMAT + 1),
            other => panic!("expected a format error, got {other:?}"),
        }
    }

    /// A file that is not a project at all says which piece is missing.
    #[test]
    fn something_that_is_not_a_project_says_so() {
        let dir = temp("not-a-project");
        let path = dir.join("hello.octosnap");
        std::fs::write(&path, b"this is not a zip").unwrap();
        assert!(matches!(read(&path, &dir.join("out")), Err(Error::Zip(_))));

        // A zip with nothing in it.
        let empty = dir.join("empty.octosnap");
        {
            let file = std::fs::File::create(&empty).unwrap();
            zip::ZipWriter::new(file).finish().unwrap();
        }
        assert!(matches!(read(&empty, &dir.join("out")), Err(Error::Missing("manifest.json"))));
    }
}
