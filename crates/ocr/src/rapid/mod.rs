// SPDX-License-Identifier: GPL-3.0-or-later

//! RapidOCR: PaddleOCR's detection and recognition models on ONNX Runtime.
//!
//! `spec/07` §2.2's default, and the owner's choice (`docs/decisions.md` D82). Two models
//! and no more: DBNet says where the text is, a CTC recogniser says what it says, and the
//! angle classifier PaddleOCR puts between them is left out -- a screenshot is never
//! upside down, and the classifier is a round trip through the runtime per line to
//! discover it.
//!
//! ONNX Runtime is loaded at run time rather than linked in. The static library is a
//! hundred megabytes of archive that would end up in every binary in the workspace and in
//! every test that touches this crate, and the Flatpak `spec/10` §10 names first wants a
//! shared library as a module anyway. When it is not there, [`Rapid::open`] says so and
//! the application refuses the *read* in those words (D97). It used to answer with
//! [`crate::engine::Null`] instead, which made "no runtime" indistinguishable from "no
//! language pack" and sent the user to download models they already had.
//!
//! **Nothing here is asked what is installed.** [`crate::packs::installed`] and
//! [`crate::packs::detector`] answer that from the directory, because the settings page and
//! the capture path both need it before a read and neither can pay for opening a model to
//! find out (D97). `Rapid` uses the same two functions for its own `packs()`.

pub mod detect;
pub mod recognise;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use ort::{
    session::{Session, builder::GraphOptimizationLevel},
    value::Tensor,
};

use crate::{Bounds, Gray, OcrError, Word, engine::{Engine, Pack, Script}, packs};

/// Where the model packs live, under `$XDG_DATA_HOME`.
pub const HOME: &str = "octosnap/ocr";

/// The detection model is one file for every script, and this is its directory.
///
/// The same name the manifest installs it under, taken from there rather than spelled
/// again: the half that downloads and the half that loads have to agree, and two string
/// literals in two files do not.
const DETECT: &str = packs::DETECT.tag;

/// How many lines go through the recogniser at once.
const BATCH: usize = 8;

/// The file names PaddleX exports under, which the packs keep. In `packs` for the same
/// reason `DETECT` is: the downloader writes them and this reads them.
use packs::{METADATA, MODEL};

/// A loaded recognition model and the alphabet that decodes it.
#[derive(Debug)]
struct Recogniser {
    session: Session,
    characters: Vec<String>,
}

/// The engine.
#[derive(Debug)]
pub struct Rapid {
    home: PathBuf,
    detect: Mutex<Session>,
    /// Loaded when first asked for, because a pack is 8 MB of weights and a capture asks
    /// about one script.
    recognisers: Mutex<HashMap<Script, Recogniser>>,
}

impl Rapid {
    /// Loads the runtime and the detection model.
    ///
    /// # Errors
    /// [`OcrError::NoRuntime`] if ONNX Runtime cannot be found, [`OcrError::Engine`] with
    /// [`RUNTIME`] if it cannot be loaded, and an error about the model if the detection
    /// model is not installed or will not load.
    pub fn open(home: &Path) -> Result<Self, OcrError> {
        runtime()?;
        let detect = session(&home.join(DETECT).join(MODEL))?;
        Ok(Self {
            home: home.to_path_buf(),
            detect: Mutex::new(detect),
            recognisers: Mutex::new(HashMap::new()),
        })
    }

    /// The pack directory: `$XDG_DATA_HOME/octosnap/ocr`, or `~/.local/share/...`.
    #[must_use]
    pub fn home() -> PathBuf {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .unwrap_or_else(|| PathBuf::from("."));
        data.join(HOME)
    }

    fn with<T>(
        &self,
        script: Script,
        use_it: impl FnOnce(&mut Recogniser) -> T,
    ) -> Result<T, OcrError> {
        let mut loaded = self.recognisers.lock().map_err(poisoned)?;
        if let std::collections::hash_map::Entry::Vacant(slot) = loaded.entry(script) {
            let directory = self.home.join(script.tag());
            let session = session(&directory.join(MODEL))?;
            let classes = classes(&session);
            let yml = std::fs::read_to_string(directory.join(METADATA))
                .map_err(|why| OcrError::Engine(script.tag().to_string(), why.to_string()))?;
            slot.insert(Recogniser { session, characters: recognise::characters(&yml, classes) });
        }
        let recogniser = loaded.get_mut(&script).ok_or(OcrError::Missing("the pack"))?;
        Ok(use_it(recogniser))
    }

    /// Which script to read with, when the caller did not say.
    ///
    /// `spec/07` §2.1 wants auto-detection "by default (script detection + model
    /// confidence)". The second half of that is the whole of this: every installed pack
    /// reads the first few lines, and the one that is surest of itself wins. Script
    /// detection proper would be a third model to download to decide which of the two the
    /// user has to run, which is a worse trade than reading twice.
    fn guess(&self, image: &Gray, boxes: &[Bounds]) -> Script {
        let installed: Vec<Script> =
            Script::ALL.into_iter().filter(|s| packs::installed(&self.home, *s)).collect();
        let Some(first) = installed.first().copied() else { return Script::Latin };
        if installed.len() == 1 {
            return first;
        }
        let sample: Vec<Bounds> = boxes.iter().take(4).copied().collect();
        installed
            .into_iter()
            .map(|script| {
                let sure = self
                    .with(script, |recogniser| read_lines(recogniser, image, &sample))
                    .map(|words| confidence(&words))
                    .unwrap_or(0.0);
                (script, sure)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(first, |(script, _)| script)
    }
}

impl Engine for Rapid {
    fn packs(&self) -> Vec<Pack> {
        packs::all()
            .iter()
            .map(|pack| Pack {
                script: pack.script,
                bytes: pack.bytes,
                installed: packs::installed(&self.home, pack.script),
            })
            .collect()
    }

    fn read(&self, image: &Gray, script: Option<Script>) -> Result<Vec<Word>, OcrError> {
        let (width, height, ratio) = detect::scout(image.width(), image.height());
        let mut boxes = self.look(image, width, height, ratio)?;
        // One throwaway look to find out how big the type is, then the real one at the
        // scale that answer implies. See `detect::scout`.
        if let Some(wanted) = detect::again(&boxes, ratio) {
            let (width, height, ratio) = detect::fit(image.width(), image.height(), wanted);
            boxes = self.look(image, width, height, ratio)?;
        }
        if boxes.is_empty() {
            return Ok(Vec::new());
        }
        let script = match script {
            Some(script) => script,
            None => self.guess(image, &boxes),
        };
        self.with(script, |recogniser| read_lines(recogniser, image, &boxes))
    }
}

impl Rapid {
    /// One detection pass, at the size given.
    fn look(
        &self,
        image: &Gray,
        width: u32,
        height: u32,
        ratio: f64,
    ) -> Result<Vec<Bounds>, OcrError> {
        let values = detect::tensor(image, width, height);
        let shape = [1i64, 3, i64::from(height), i64::from(width)];
        let mut session = self.detect.lock().map_err(poisoned)?;
        let name = input(&session);
        let outputs = session
            .run(ort::inputs![name => Tensor::from_array((shape, values)).map_err(ort_failed)?])
            .map_err(ort_failed)?;
        let value = outputs.values().next().ok_or(OcrError::Missing("a probability map"))?;
        let (_, map) = value.try_extract_tensor::<f32>().map_err(ort_failed)?;
        let found = detect::boxes(map, width, height, (image.width(), image.height()));
        tracing::debug!(width, height, ratio, boxes = found.len(), "a detection pass");
        Ok(found)
    }
}

/// Every box, read, a batch of lines at a time.
///
/// One line a run is the obvious shape and costs about 30 ms a line, nearly all of it the
/// round trip rather than the arithmetic: the model is 8 MB and the input is a fiftieth of
/// a megapixel. A page of forty lines is then over `spec/10` §7's whole budget before
/// detection has been paid for. Batched, the lines that are alike in shape go through
/// together and the round trip is paid once for eight of them.
///
/// Sorted by aspect first, because a batch is as wide as its widest member and every
/// narrower line in it is padded out to that width -- pairing a word with a paragraph
/// wastes more arithmetic than the batching saves.
fn read_lines(recogniser: &mut Recogniser, image: &Gray, boxes: &[Bounds]) -> Vec<Word> {
    let lines: Vec<(usize, Gray)> = boxes
        .iter()
        .enumerate()
        .filter_map(|(at, bounds)| image.crop(*bounds).map(|line| (at, line)))
        .collect();
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by(|a, b| aspect(&lines[*a].1).total_cmp(&aspect(&lines[*b].1)));

    let mut words = Vec::with_capacity(lines.len());
    for batch in order.chunks(BATCH) {
        let shaped: Vec<(usize, Vec<f32>, u32)> = batch
            .iter()
            .map(|at| {
                let (index, line) = &lines[*at];
                let (values, width) = recognise::tensor(line);
                (*index, values, width)
            })
            .collect();
        let Some(widest) = shaped.iter().map(|(_, _, width)| *width).max() else { continue };
        let rows = recognise::ROWS as usize;
        let mut values = vec![0.0f32; shaped.len() * 3 * rows * (widest as usize)];
        for (slot, (_, line, width)) in values
            .chunks_exact_mut(3 * rows * (widest as usize))
            .zip(&shaped)
        {
            // Zero is mid-grey once normalised, which is what PaddleOCR pads with.
            for (plane, source) in slot
                .chunks_exact_mut(rows * (widest as usize))
                .zip(line.chunks_exact(rows * (*width as usize)))
            {
                for (row, from) in plane
                    .chunks_exact_mut(widest as usize)
                    .zip(source.chunks_exact(*width as usize))
                {
                    row[..from.len()].copy_from_slice(from);
                }
            }
        }
        let shape = [shaped.len() as i64, 3, recognise::ROWS as i64, i64::from(widest)];
        let Ok(tensor) = Tensor::from_array((shape, values)) else { continue };
        let name = input(&recogniser.session);
        let Ok(outputs) = recogniser.session.run(ort::inputs![name => tensor]) else { continue };
        let Some(value) = outputs.values().next() else { continue };
        let Ok((size, logits)) = value.try_extract_tensor::<f32>() else { continue };
        let dimensions: Vec<i64> = size.iter().copied().collect();
        let [_, steps, classes] = dimensions.as_slice() else { continue };
        let (steps, classes) = (*steps as usize, *classes as usize);
        for (slot, (index, _, _)) in shaped.iter().enumerate() {
            let Some(rows) = logits.get(slot * steps * classes..(slot + 1) * steps * classes) else {
                continue;
            };
            let (text, sure) = recognise::decode(rows, steps, classes, &recogniser.characters);
            if !text.trim().is_empty() {
                words.push(Word::new(text, boxes[*index], sure));
            }
        }
    }
    words
}

fn aspect(line: &Gray) -> f64 {
    f64::from(line.width()) / f64::from(line.height().max(1))
}

fn confidence(words: &[Word]) -> f32 {
    if words.is_empty() {
        return 0.0;
    }
    words.iter().map(|word| word.confidence).sum::<f32>() / words.len() as f32
}

/// The first input's name, which is `x` on every PaddleX export but is asked rather than
/// assumed -- a model that renames it should fail to read, not fail to load.
fn input(session: &Session) -> String {
    session.inputs().first().map_or_else(|| "x".to_string(), |outlet| outlet.name().to_string())
}

/// How many classes the recogniser's output has, which is what says whether the model was
/// trained with a space character.
fn classes(session: &Session) -> usize {
    session
        .outputs()
        .first()
        .and_then(|outlet| match outlet.dtype() {
            ort::value::ValueType::Tensor { shape, .. } => shape.last().copied(),
            _ => None,
        })
        .filter(|last| *last > 0)
        .map_or(0, |last| last as usize)
}

fn session(model: &Path) -> Result<Session, OcrError> {
    if !model.is_file() {
        return Err(OcrError::Missing("the model"));
    }
    let failed = |why: String| OcrError::Engine(model.display().to_string(), why);
    let builder = Session::builder().map_err(|why| failed(why.to_string()))?;
    let builder = builder
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|why| failed(why.to_string()))?;
    // Half the cores, at most four. More is measurably slower on a model this small -- a
    // 2.0 s read becomes 4.2 s at sixteen threads on a twelve-core machine -- and a
    // screenshot tool that pins every core while somebody waits is worse than a slow one.
    let mut builder =
        builder.with_intra_threads(threads()).map_err(|why| failed(why.to_string()))?;
    builder.commit_from_file(model).map_err(|why| failed(why.to_string()))
}

fn threads() -> usize {
    std::thread::available_parallelism().map_or(2, |cores| (cores.get() / 2).clamp(1, 4))
}

/// What [`OcrError::Engine`] names when ONNX Runtime itself is the trouble.
///
/// From [`Rapid::open`] it means a library was found and would not load -- one older than
/// this build's API level, or not a library at all. None found is [`OcrError::NoRuntime`].
pub const RUNTIME: &str = "onnxruntime";

/// ONNX Runtime, found and loaded once for the process.
///
/// **A library that is not there is looked for again by the next read** (D172), so an
/// install that a read asked for counts from the next one. Until D172 every failure was
/// kept in the same `OnceLock` as the success, and the app is a service that runs until the
/// session ends: the install did not count until the next login.
///
/// **A library that is there and would not load is kept, failure and all**, and that is
/// `ort`'s doing. Its `OnceLock` (2.0.0-rc.13, `util/once_lock_std.rs`) completes the
/// `Once` when the load fails, with no library in it, and every later call into `ort` then
/// looks a null handle up for `OrtGetApiBase` and panics -- inside its environment's lock,
/// so the process aborts on its way out (D172 found it by hiding the library in a mount
/// namespace and asking twice). So `ort` is asked once, and only about a file that exists.
fn runtime() -> Result<(), OcrError> {
    static TRIED: OnceLock<Result<(), String>> = OnceLock::new();
    let engine = |why: String| OcrError::Engine(RUNTIME.to_string(), why);
    if let Some(tried) = TRIED.get() {
        return tried.clone().map_err(engine);
    }
    // Not kept: `ort` has not been asked.
    let library = library().ok_or(OcrError::NoRuntime)?;
    let tried = match ort::init_from(&library) {
        // `commit` answers false when something already committed an environment, which
        // is not a failure: the runtime is loaded either way, and this runs once per
        // process.
        Ok(environment) => {
            let _ = environment.commit();
            Ok(())
        }
        Err(why) => Err(format!("{}: {why}", library.display())),
    };
    TRIED.get_or_init(|| tried).clone().map_err(engine)
}

/// The file name a linker would use, which is the one a `-dev` package installs.
const SONAME: &str = "libonnxruntime.so";

/// Where to look for ONNX Runtime, in the order a packager would want it looked for.
///
/// Each directory is asked for [`SONAME`] and then for the newest `libonnxruntime.so.N`
/// beside it, because **the unversioned name is a development symlink**: Ubuntu's
/// `libonnxruntime1.23` ships `libonnxruntime.so.1.23` and nothing else, so a machine with
/// the runtime installed and no `-dev` package had a `dlopen` failure and a read that
/// said "no text-recognition engine". Any 1.17 or newer answers the API level this build
/// pins, so the newest one present is the right one to take.
///
/// Only ever a file that exists (D172). The last resort used to be the bare [`SONAME`],
/// for the loader to search, and a search that failed left `ort` unable to load anything
/// for the rest of the process ([`runtime`]). `LD_LIBRARY_PATH`'s directories stand in for
/// it: they are where the loader looked that this list did not.
fn library() -> Option<PathBuf> {
    let named = |path: PathBuf| path.is_file().then_some(path);
    // Two environment variables, both an exact file: ours, and the one the `ort` crate's
    // own dynamic loading documents, which somebody who has already made this work on
    // another project will have set.
    let from_env = |key: &str| std::env::var_os(key).map(PathBuf::from).and_then(named);
    from_env("OCTOSNAP_ONNXRUNTIME")
        .or_else(|| from_env("ORT_DYLIB_PATH"))
        // Beside the model packs, where a copy can be put by hand on a system that has
        // none. Nothing downloads it there: the packs are models, not the runtime.
        .or_else(|| in_directory(&Rapid::home().with_file_name("onnxruntime")))
        // The Flatpak's own prefix, then the system's: Arch's, Debian's multiarch one,
        // Fedora's and openSUSE's `lib64` (D172: Fedora's `onnxruntime` installs only
        // `/usr/lib64/libonnxruntime.so.1.22.2`, which nothing here looked in), then where
        // a hand-built copy lands.
        .or_else(|| in_directory(Path::new("/app/lib")))
        .or_else(|| in_directory(Path::new("/usr/lib")))
        .or_else(|| in_directory(&PathBuf::from("/usr/lib").join(multiarch())))
        .or_else(|| in_directory(Path::new("/usr/lib64")))
        .or_else(|| in_directory(Path::new("/usr/local/lib")))
        .or_else(|| in_directory(Path::new("/usr/local/lib64")))
        .or_else(|| {
            let paths = std::env::var_os("LD_LIBRARY_PATH")?;
            std::env::split_paths(&paths)
                .filter(|directory| directory.is_absolute())
                .find_map(|directory| in_directory(&directory))
        })
}

/// Debian and Ubuntu's multiarch directory name for this build's architecture.
fn multiarch() -> String {
    format!("{}-linux-gnu", std::env::consts::ARCH)
}

/// [`SONAME`] in `directory`, or the highest-versioned library beside it.
fn in_directory(directory: &Path) -> Option<PathBuf> {
    let plain = directory.join(SONAME);
    if plain.is_file() {
        return Some(plain);
    }
    let versioned = format!("{SONAME}.");
    let mut best: Option<(Vec<u32>, PathBuf)> = None;
    for entry in std::fs::read_dir(directory).ok()?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(suffix) = name.strip_prefix(&versioned) else { continue };
        if !entry.path().is_file() {
            continue;
        }
        // "1.23.2" sorts after "1.23" and after "1.9", which a string compare would not.
        let version: Vec<u32> = suffix.split('.').map_while(|part| part.parse().ok()).collect();
        if version.is_empty() {
            continue;
        }
        if best.as_ref().is_none_or(|(highest, _)| version > *highest) {
            best = Some((version, entry.path()));
        }
    }
    best.map(|(_, path)| path)
}

fn ort_failed(why: ort::Error) -> OcrError {
    OcrError::Engine(RUNTIME.to_string(), why.to_string())
}

fn poisoned<T>(_: std::sync::PoisonError<T>) -> OcrError {
    OcrError::Engine("the engine".to_string(), "a previous read panicked".to_string())
}
