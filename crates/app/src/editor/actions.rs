// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §7's bottom bar, and the export behind every button on it.
//!
//! Each action renders the document to a file and then hands that file to the **existing**
//! capture pipeline. That is the whole design, and it is worth stating because the obvious
//! alternative -- an editor that talks to the clipboard and the filename template itself --
//! would be a second implementation of everything `spec/04` §3 already does. The
//! clipboard route, `ACT-07`'s filename template, the `{n}` counter, `last_saved` for
//! "Show in Files": the editor gets all of it by handing over a `CaptureResult` whose
//! `path` points at the render.
//!
//! The editor is not generic over the shell bridge, and it does not need to be. `CaptureFlow`
//! is generic; the four things the bottom bar asks of it are not, so they arrive as
//! [`EditorActions`] -- closures the caller builds from the flow it already holds. That
//! keeps a type parameter out of every widget in this module for the sake of four calls.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use octosnap_core::{CaptureResult, Rect, print};
use octosnap_scene::project;
use octosnap_scene::tool::Modifiers;
use octosnap_scene::{Point, Rgba};
use tracing::{info, warn};

use super::window::{Closing, Editor};

/// `spec/05` §6: "render … at `export_scale`". One document pixel per image pixel.
///
/// The document is already in *physical* pixels -- `Base::new` is handed
/// `rect.width * scale` -- so this is §7's "at document scale, Retina-aware" without any
/// further arithmetic. `spec/05` §11 item 2's 2x export is the same call with 2.0.
pub const EXPORT_SCALE: f64 = 1.0;

impl Editor {
    /// The factor an export renders at: the document's own scale, or -- when `spec/08`
    /// §7's `ann-export-scale` says `1x` -- the one that brings a HiDPI capture down to
    /// one file pixel per logical pixel.
    #[must_use]
    pub(super) fn export_scale(&self) -> f64 {
        if crate::settings::export_at_1x() {
            EXPORT_SCALE / self.capture.scale.max(f64::EPSILON)
        } else {
            EXPORT_SCALE
        }
    }
}

/// What the bottom bar needs from the capture flow.
///
/// Closures rather than a trait, because every one of these is `async` behind the scenes
/// and an object-safe async trait needs a boxing crate the workspace does not have. The
/// caller spawns; the editor only decides when.
pub struct EditorActions {
    /// Each output action reports back through a [`Done`], because the editor has to
    /// know whether it worked: a Copy that succeeded is a "Copied" toast and a Save that
    /// succeeded one with the file's name (`spec/13` #6, #10). A failure is the flow's to
    /// notify -- it already does -- and the editor says nothing over it.
    pub copy: Box<dyn Fn(CaptureResult, Done)>,
    pub save: Box<dyn Fn(CaptureResult, Done)>,
    /// `spec/05` §7's Save as… once the *editor* has asked where.
    ///
    /// The editor asks, not the flow, and that changed when the project format arrived:
    /// §7 describes one chooser with a "format selector (PNG/JPG/WebP) **and project
    /// format**", and a project is written by `octosnap_scene::project` rather than by
    /// the capture flow. So the *choice* belongs to the editor and only the image branch
    /// comes back here — which is what keeps §7's filename template, `last saved` and
    /// "Show in Files" working for the half of the choice that is still an image.
    pub save_as_path: Box<dyn Fn(CaptureResult, PathBuf, Done)>,
    /// Synchronous, and answers whether a pin was made -- the pins host says so at once.
    pub pin: Box<dyn Fn(CaptureResult) -> bool>,
    /// `spec/05` §4.11: "A pinned window showing this image updates after crop"
    /// [D 4.7.5], which `spec/05` §11 item 7 ends with.
    ///
    /// The paths are the identity and the capture is the new content: a pin was created
    /// either from the card that opened this editor or from this editor's own Pin button,
    /// so both the original spool path and the render's path have to be offered. Answers
    /// nothing, because the editor does not care how many pins there were.
    pub refresh_pins: Box<dyn Fn(Vec<PathBuf>, CaptureResult)>,
    /// `spec/07` §2.1's Copy Text, "from a card/pin/**editor** menu".
    ///
    /// The *rendered* document and not the capture file, because the editor is where the
    /// user cropped to the paragraph they wanted -- reading the original capture would
    /// hand back the whole screen. The read copies to the clipboard for itself, and hands
    /// its outcome back so the editor can say how it went where the user is looking.
    pub recognise: Box<dyn Fn(CaptureResult, ReadDone)>,
    /// `spec/05` §2's pipette "via the extension, from anywhere on screen", through
    /// `spec/10` §3.1's `PickColor`. The editor hands over what to do with the answer:
    /// `Ok(Some)` is a colour, `Ok(None)` the user backing out, and `Err` says whether the
    /// extension simply predates the method -- which is the case the editor falls back to
    /// the canvas pipette from, rather than reporting a fault.
    pub pick_screen_color: Box<dyn Fn(PickDone)>,
    /// What the window left behind when it closed (D108): the capture back in the stack
    /// as a card, or -- after Final Close -- in the history. Called from the window's
    /// close-request, once, with the render already on disk.
    pub closed: Box<dyn Fn(Closed)>,
}

/// How an editor ended (D108).
#[derive(Debug)]
pub enum Closed {
    /// ×, Ctrl+W: the capture goes back to the stack as a card, edits and all.
    Preview(Returned),
    /// Final Close, or a Pin that took the render: no card, and the history keeps it.
    Final(Returned),
}

/// What an editor hands back as it closes.
#[derive(Debug)]
pub enum Returned {
    /// Nothing was changed -- or a pin took the render -- so the capture goes back as the
    /// file it was.
    Unchanged,
    /// The document, rendered as a capture of its own and carried with it.
    Edited(Box<Document>),
    /// A render with nothing to carry: the GIF editor's trim, a capture of its own whose
    /// only edit is what it kept.
    Rendered(Box<CaptureResult>),
}

/// An edited document on its way out of an editor: a render the stack can show, and
/// everything an editor needs to carry on from it.
///
/// The render is a capture in its own right -- `<id>.png` in the spool with its JSON twin
/// -- because from here on it *is* one: a card shows it, Copy and Save hand it over, a
/// drag drops it, and the history files it when its card goes. The rest is what makes
/// the card's Annotate pick up where the × left off rather than on a flattened picture:
/// the capture the document is drawn on, the objects, and the undo stack.
#[derive(Debug)]
pub struct Document {
    pub render: CaptureResult,
    /// Where the render was saved, when the last Save was of exactly this state.
    pub saved_to: Option<PathBuf>,
    /// The capture the editor was opened on: the scene's base image.
    pub capture: CaptureResult,
    pub scene: octosnap_scene::Scene,
    pub history: octosnap_scene::History,
    /// [`Editor`]'s `saved_at` and `saved_file`, so the unsaved dot means the same thing
    /// after a reopen as before it.
    pub saved_at: usize,
    pub saved_file: Option<(usize, PathBuf)>,
}

/// How an output action ended: the path it wrote, if it wrote one, or why it failed.
pub type Outcome = Result<Option<PathBuf>, String>;
/// What the editor does with an [`Outcome`] when it arrives.
pub type Done = Box<dyn FnOnce(Outcome)>;
/// What the editor does with a Copy Text read when it has ended.
pub type ReadDone = Box<dyn FnOnce(crate::flow::Outcome)>;

/// How long a toast stays: long enough to read it, and with a button long enough to press
/// it. Both editors' toasts use it.
#[must_use]
pub fn toast_seconds(with_button: bool) -> u32 {
    if with_button { 4 } else { 2 }
}

/// What the screen pipette came back with: a colour, a cancel, or why not.
pub type PickAnswer = Result<Option<Rgba>, PickFailure>;
/// What the editor does with a [`PickAnswer`] when it arrives.
pub type PickDone = Box<dyn FnOnce(PickAnswer)>;

/// Why the shell could not pick a colour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickFailure {
    /// The extension is running but has no `PickColor`: it is older than this build and
    /// has not been reloaded by a logout yet.
    Unsupported,
    Other(String),
}

impl std::fmt::Debug for EditorActions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EditorActions")
    }
}

impl Editor {
    /// Renders the document and writes it beside the capture in the spool.
    ///
    /// Beside the capture rather than in `/tmp`: the spool is the app's own cache, the
    /// file has to outlive a drag that the user may take their time over, and
    /// `spec/04` §7's history looks for captures there. One file per annotated capture,
    /// overwritten on each export, so annotating for ten minutes leaves one.
    pub(super) fn export_png(&self, scale: f64) -> Option<PathBuf> {
        let started = std::time::Instant::now();
        let texture = self.canvas.render_texture(scale)?;
        let stem = self.capture.path.file_stem()?.to_string_lossy().into_owned();
        let path = self.capture.path.with_file_name(format!("{stem}-annotated.png"));
        if !Self::encode_png(&texture, &path) {
            return None;
        }
        info!(
            path = %path.display(),
            width = texture.width(),
            height = texture.height(),
            scale,
            total_ms = format!("{:.1}", started.elapsed().as_secs_f64() * 1000.0),
            "exported"
        );
        Some(path)
    }

    /// Encodes a rendered texture as a PNG, the way `spec/05` §6 asks for.
    ///
    /// > **Export** = render the same node tree offscreen with
    /// > `gsk::Renderer::render_texture(node, bounds)` at `export_scale`, then encode
    /// > (PNG via `png`/`image`, JPEG quality setting, WebP via `libwebp` binding).
    ///
    /// `GdkTexture::save_to_png` was here first and is what the performance fixture
    /// caught: of a 4747 ms export of §11 item 1's 5K document with thirty objects,
    /// **4511 ms was that one call** — the render was 119 ms and the GPU read-back 113 ms.
    /// It compresses at maximum effort and offers no setting to turn that down, which is
    /// the wrong trade for a screenshot: `Compression::Fast` on the same 14.7-megapixel
    /// image is a file a few per cent larger, produced in a fraction of the time, and
    /// nobody is archiving PNGs of their screen at maximum entropy density.
    ///
    /// Downloaded as `R8g8b8a8`, which is **straight** RGBA rather than Cairo's
    /// premultiplied ARGB32 — the same reason `Canvas::sample` asks for that format. PNG
    /// stores straight alpha, so premultiplied bytes would darken every semi-transparent
    /// pixel: a 55 % highlighter stroke and a filled rectangle with opacity are exactly
    /// the objects that would come out wrong, and only against a light background.
    ///
    /// Row by row, because a downloaded texture's stride is padded to the GPU's liking
    /// and is not `width * 4`.
    pub(crate) fn encode_png(texture: &gtk::gdk::Texture, path: &std::path::Path) -> bool {
        let (width, height) = (texture.width(), texture.height());
        let Ok((usable_w, usable_h)) = usize::try_from(width).and_then(|w| {
            usize::try_from(height).map(|h| (w, h))
        }) else {
            warn!(width, height, "a texture with no size cannot be encoded");
            return false;
        };

        let (bytes, stride) = {
            let mut downloader = gtk::gdk::TextureDownloader::new(texture);
            downloader.set_format(gtk::gdk::MemoryFormat::R8g8b8a8);
            downloader.download_bytes()
        };
        let row_bytes = usable_w.saturating_mul(4);
        if stride < row_bytes || bytes.len() < stride.saturating_mul(usable_h) {
            warn!(stride, row_bytes, len = bytes.len(), "the download is short");
            return false;
        }

        let file = match std::fs::File::create(path) {
            Ok(file) => file,
            Err(e) => {
                warn!(path = %path.display(), "could not create the export: {e}");
                return false;
            }
        };
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width.unsigned_abs(), height.unsigned_abs());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = match encoder.write_header() {
            Ok(writer) => writer,
            Err(e) => {
                warn!(path = %path.display(), "could not write the PNG header: {e}");
                return false;
            }
        };
        let mut stream = match writer.stream_writer() {
            Ok(stream) => stream,
            Err(e) => {
                warn!(path = %path.display(), "could not start the PNG stream: {e}");
                return false;
            }
        };
        use std::io::Write;
        for row in 0..usable_h {
            let start = row * stride;
            let Some(line) = bytes.get(start..start + row_bytes) else {
                warn!(row, "the download ended early");
                return false;
            };
            if let Err(e) = stream.write_all(line) {
                warn!(path = %path.display(), "could not write row {row}: {e}");
                return false;
            }
        }
        if let Err(e) = stream.finish() {
            warn!(path = %path.display(), "could not finish the PNG: {e}");
            return false;
        }
        true
    }

    /// `spec/05` §7: "Print | Ctrl+P | GTK print dialog".
    ///
    /// The document rendered at the capture's own scale -- every pixel it has -- painted
    /// onto the page through Cairo. A screenshot is fitted inside the printable area with
    /// its proportions kept and centred; a capture much longer than the page is set to the
    /// page's width and cut over as many pages as it needs (`spec/07` §1 item 6), which
    /// `octosnap_core::print` lays out.
    pub(super) fn print_rendered(self: &Rc<Self>) {
        self.print(None);
    }

    /// The same operation, exported to a PDF instead of shown as a dialog. A print dialog
    /// needs a person; this is how `editor-test.sh` checks the page.
    pub(super) fn print_to_pdf(self: &Rc<Self>, path: &std::path::Path) -> bool {
        self.print(Some(path))
    }

    fn print(self: &Rc<Self>, pdf: Option<&std::path::Path>) -> bool {
        let scale = self.capture.scale.max(1.0);
        let Some(texture) = self.canvas.render_texture(scale) else {
            crate::notify::action_failed(&self.app, "Print", "the document could not be rendered");
            return false;
        };
        let Some(surface) = cairo_surface(&texture) else {
            crate::notify::action_failed(&self.app, "Print", "the render could not be read back");
            return false;
        };
        let (width, height) = (f64::from(texture.width()), f64::from(texture.height()));
        let scrolling = self.capture.mode == octosnap_core::CaptureMode::Scrolling;
        // Laid out once the dialog has settled the paper, which `begin-print` is for: the
        // number of pages depends on it.
        let layout: Rc<Cell<Option<print::Layout>>> = Rc::new(Cell::new(None));

        let operation = gtk::PrintOperation::new();
        operation.set_n_pages(1);
        operation.set_embed_page_setup(true);
        let stem = self.capture.path.file_stem().map(|s| s.to_string_lossy().into_owned());
        operation.set_job_name(&format!("OctoSnap {}", stem.unwrap_or_default()));
        {
            let layout = Rc::clone(&layout);
            operation.connect_begin_print(move |operation, context| {
                let planned =
                    print::Layout::new((width, height), (context.width(), context.height()), scrolling);
                operation.set_n_pages(i32::try_from(planned.pages).unwrap_or(i32::MAX));
                layout.set(Some(planned));
            });
        }
        {
            let layout = Rc::clone(&layout);
            operation.connect_draw_page(move |_, context, page| {
                let cr = context.cairo_context();
                let (page_w, page_h) = (context.width(), context.height());
                if page_w <= 0.0 || page_h <= 0.0 || width <= 0.0 || height <= 0.0 {
                    return;
                }
                let planned = layout.get().unwrap_or_else(|| {
                    print::Layout::new((width, height), (page_w, page_h), scrolling)
                });
                let (x, y) = planned.origin(u32::try_from(page).unwrap_or(0));
                // Each page shows its own part and nothing of its neighbours'.
                cr.rectangle(0.0, 0.0, page_w, page_h);
                cr.clip();
                cr.translate(x, y);
                cr.scale(planned.scale, planned.scale);
                if cr.set_source_surface(&surface, 0.0, 0.0).is_ok()
                    && let Err(e) = cr.paint()
                {
                    warn!("could not paint the page: {e}");
                }
            });
        }
        let action = match pdf {
            Some(path) => {
                operation.set_export_filename(path);
                gtk::PrintOperationAction::Export
            }
            None => gtk::PrintOperationAction::PrintDialog,
        };
        match operation.run(action, Some(&self.window)) {
            Ok(result) => {
                let pages = layout.get().map_or(1, |l| l.pages);
                info!(
                    result = ?result,
                    width,
                    height,
                    pages,
                    pdf = pdf.map(|p| p.display().to_string()).unwrap_or_default(),
                    "print"
                );
                if result == gtk::PrintOperationResult::Apply {
                    let said = match (pdf.and_then(|p| p.file_name()), pages) {
                        (Some(name), 1) => format!("Saved a PDF as {}", name.to_string_lossy()),
                        (Some(name), n) => {
                            format!("Saved a PDF of {n} pages as {}", name.to_string_lossy())
                        }
                        (None, 1) => "Sent to the printer".to_owned(),
                        (None, n) => format!("Sent {n} pages to the printer"),
                    };
                    self.toast(&said, None);
                }
                true
            }
            Err(e) => {
                warn!("print failed: {e}");
                crate::notify::action_failed(&self.app, "Print", &e.to_string());
                false
            }
        }
    }

    /// The document as `objects.json`, written to a path. `spec/05` §11 item 4 compares
    /// two of these for byte identity, and the harness has no other way to see one.
    pub(super) fn dump_objects(&self, path: &std::path::Path) -> bool {
        let Some(scene) = self.canvas.scene() else {
            warn!("no document is open yet; nothing to dump");
            return false;
        };
        match scene.to_objects_json() {
            Ok(json) => match std::fs::write(path, json) {
                Ok(()) => {
                    info!(path = %path.display(), objects = scene.len(), "objects dumped");
                    true
                }
                Err(e) => {
                    warn!(path = %path.display(), "could not write the objects: {e}");
                    false
                }
            },
            Err(e) => {
                warn!("objects.json did not serialise: {e}");
                false
            }
        }
    }

    /// The render, described as a capture so the flow can copy, save or pin it.
    ///
    /// The original capture with three things changed: the path, and the rect and scale --
    /// which matter as soon as `spec/05` §4.11's crop makes the canvas a different size
    /// from the base image (D52). `rect` stays *logical*, as `CaptureResult` documents, so
    /// a pin opens at the right size and the filename template's `{width}` is the number
    /// the user would measure.
    pub(super) fn rendered_capture(&self, scale: f64) -> Option<CaptureResult> {
        let path = self.export_png(scale)?;
        self.described(path, scale)
    }

    /// The capture this editor was opened on, re-described for a render of the document
    /// at `scale` that lives at `path`.
    fn described(&self, path: PathBuf, scale: f64) -> Option<CaptureResult> {
        let scene = self.canvas.scene()?;
        let base_scale = self.capture.scale.max(f64::EPSILON);
        // The rect stays **logical** whatever the export factor; what the factor changes
        // is the file's own scale. A pin opened on this capture is then the canvas's
        // logical size at any export scale, and `path` is `rect` x `scale` as the
        // struct promises.
        #[allow(clippy::cast_possible_truncation)]
        let rect = Rect {
            x: self.capture.rect.x,
            y: self.capture.rect.y,
            width: (scene.canvas.width / base_scale).round() as i32,
            height: (scene.canvas.height / base_scale).round() as i32,
        };
        Some(CaptureResult { path, rect, scale: base_scale * scale, ..self.capture.clone() })
    }

    /// Renders the document to a path the caller names.
    ///
    /// Separate from [`Self::export_png`], which writes to the spool for the bottom bar's
    /// own use. This is the export as a *parameter*: any scale, any destination.
    pub(super) fn export_to(&self, scale: f64, path: &std::path::Path) -> bool {
        // `spec/05` §6's third budget: "export of a 5K image with 30 objects <= 400 ms".
        // Timed around the render *and* the encode, because that is what the budget
        // describes -- a caller waits for a file, not for a texture.
        let started = std::time::Instant::now();
        let Some(texture) = self.canvas.render_texture(scale) else {
            warn!("nothing to export");
            return false;
        };
        let rendered = started.elapsed();
        // The two halves are timed apart because the first measurement said 119 ms of
        // render and four and a half *seconds* of something else, and "the encode is
        // slow" and "the GPU read-back is slow" are different problems with different
        // fixes. It was the encode, by 40 to 1.
        let encoded = std::time::Instant::now();
        if !Self::encode_png(&texture, path) {
            return false;
        }
        info!(
            path = %path.display(),
            width = texture.width(),
            height = texture.height(),
            scale,
            render_ms = format!("{:.1}", rendered.as_secs_f64() * 1000.0),
            encode_ms = format!("{:.1}", encoded.elapsed().as_secs_f64() * 1000.0),
            total_ms = format!("{:.1}", started.elapsed().as_secs_f64() * 1000.0),
            budget_ms = "400.0",
            "exported"
        );
        true
    }

    /// `spec/05` §8's project, written from this editor.
    ///
    /// The thumbnail is rendered here and passed in as bytes, which is the one thing the
    /// container cannot do for itself: §8 wants "a 256 px preview for file managers" and
    /// only a realised window has a renderer. Everything else about the format is in
    /// `octosnap_scene::project`, where it can be tested without a display.
    ///
    /// `spec/05` §7 lists the project beside PNG/JPG/WebP in "Save as…", and saving one is
    /// **persisting the document** in §7's Close sense — more so than an export, since it
    /// is the only form that keeps the objects.
    pub(super) fn save_project(self: &Rc<Self>, path: &std::path::Path) -> bool {
        let Some(scene) = self.canvas.scene() else { return false };
        let started = std::time::Instant::now();
        let thumbnail = self.thumbnail_png();
        let created = glib::real_time().unsigned_abs() / 1_000_000;
        match project::write(path, &scene, &self.capture.path, thumbnail.as_deref(), created) {
            Ok(()) => {
                info!(
                    path = %path.display(),
                    objects = scene.len(),
                    thumbnail = thumbnail.is_some(),
                    total_ms = format!("{:.1}", started.elapsed().as_secs_f64() * 1000.0),
                    "project saved"
                );
                self.mark_saved();
                true
            }
            Err(e) => {
                warn!(path = %path.display(), "could not write the project: {e}");
                crate::notify::action_failed(&self.app, "Save as project", &e.to_string());
                false
            }
        }
    }

    /// `spec/05` §8's "thumbnail.png 256 px preview for file managers".
    ///
    /// Rendered from the document at whatever scale makes its longest side 256, so a
    /// portrait capture and a landscape one both come out inside the box rather than one
    /// of them being stretched. `None` when the window has no renderer, which is the same
    /// condition every other export has and is not worth failing the save over — a project
    /// without a thumbnail is still a project.
    fn thumbnail_png(&self) -> Option<Vec<u8>> {
        let scene = self.canvas.scene()?;
        let longest = scene.canvas.width.max(scene.canvas.height);
        if longest <= 0.0 {
            return None;
        }
        let scale = (f64::from(project::THUMBNAIL_SIZE) / longest).min(1.0);
        let texture = self.canvas.render_texture(scale)?;
        let path = std::env::temp_dir().join(format!("octosnap-thumb-{}.png", std::process::id()));
        if !Self::encode_png(&texture, &path) {
            return None;
        }
        let bytes = std::fs::read(&path).ok();
        // Best effort: a leftover thumbnail in the temp directory is harmless, and failing
        // the save because it could not be removed would not be.
        let _ = std::fs::remove_file(&path);
        bytes
    }

    /// The window's own action group (`win.`), for `spec/05` §11 item 2 and for the
    /// export-scale UI §7 implies.
    ///
    /// On the *window* rather than the application, and that is what makes it reachable
    /// without a registry of open editors. GTK exports a window's actions at the window's
    /// own object path -- the same path `Editor::object_path` logs and the same one the
    /// extension is handed for a geometry read -- so a caller that can see the editor can
    /// drive it, and an editor that has closed simply is not there.
    ///
    /// § 11 item 2 is "export at 2x matches the canvas at 100 %", which cannot be checked
    /// at all without a scale that comes from outside. `editor-test.sh` exports both and
    /// compares them.
    pub(super) fn install_actions(self: &Rc<Self>) {
        let Some(kind) = glib::VariantTy::new("(ds)").ok() else { return };
        let export = gio::SimpleAction::new("export", Some(kind));
        let editor = Rc::downgrade(self);
        export.connect_activate(move |_, parameter| {
            let Some(editor) = editor.upgrade() else { return };
            let Some((scale, path)) = parameter.and_then(|p| p.get::<(f64, String)>()) else {
                warn!("export needs a scale and a path");
                return;
            };
            editor.export_to(scale, std::path::Path::new(&path));
        });
        self.window.add_action(&export);

        // `spec/05` §11 item 1 is "open a 5120x2880 capture; add 30 objects including two
        // blurs and a spotlight; drag any object at 60 fps", and it cannot be checked
        // without a thirty-object scene arriving from outside -- exactly the argument
        // above for `export` taking a scale. Thirty gestures driven through the injector
        // would measure the injector.
        //
        // `Canvas::load_objects` deliberately does not touch the history: a fixture is
        // not an edit, and a thirty-deep undo stack before the measurement starts would
        // be a different document from the one §11 item 1 describes.
        let load = gio::SimpleAction::new("load-objects", Some(glib::VariantTy::STRING));
        {
            let editor = Rc::downgrade(self);
            load.connect_activate(move |_, parameter| {
                let Some(editor) = editor.upgrade() else { return };
                let Some(path) = parameter.and_then(|p| p.get::<String>()) else {
                    warn!("load-objects needs a path");
                    return;
                };
                match std::fs::read_to_string(&path) {
                    Ok(json) => match editor.canvas.load_objects(&json) {
                        Some(Ok(objects)) => info!(path = %path, objects, "objects loaded"),
                        Some(Err(e)) => {
                            warn!(path = %path, "objects.json did not parse: {e}");
                        }
                        None => warn!("no document is open yet; nothing to load into"),
                    },
                    Err(e) => warn!(path = %path, "could not read the objects: {e}"),
                }
            });
        }
        self.window.add_action(&load);

        // §6's budget, bracketed. Without a reset the numbers cover every frame since the
        // editor opened -- including the ones that drew an empty canvas while the texture
        // was still uploading, which is the cheapest frame there is and drags the mean
        // below the thing being measured.
        let stats = gio::SimpleAction::new("frame-stats", None);
        {
            let editor = Rc::downgrade(self);
            stats.connect_activate(move |_, _| {
                let Some(editor) = editor.upgrade() else { return };
                let (hits, misses) = editor.canvas.cache_counts();
                match editor.canvas.build_stats() {
                    Some((frames, mean, p95, max)) => {
                        let per_frame = |n: u64| {
                            u64::try_from(frames).map_or(0.0, |f| {
                                #[allow(clippy::cast_precision_loss)]
                                if f == 0 { 0.0 } else { n as f64 / f as f64 }
                            })
                        };
                        info!(
                            frames,
                            mean_ms = format!("{mean:.3}"),
                            p95_ms = format!("{p95:.3}"),
                            max_ms = format!("{max:.3}"),
                            budget_ms = "2.000",
                            hits_per_frame = format!("{:.1}", per_frame(hits)),
                            misses_per_frame = format!("{:.1}", per_frame(misses)),
                            "node build"
                        );
                    }
                    None => info!(frames = 0, "node build"),
                }
                editor.canvas.reset_build_stats();
            });
        }
        self.window.add_action(&stats);

        // `spec/05` §8's project, saved to a path the caller names. Same argument as
        // `export`: §11 item 8 is "save as project -> reopen -> all objects editable;
        // thumbnail present", and neither half is reachable without a path from outside.
        let save_project =
            gio::SimpleAction::new("save-project", Some(glib::VariantTy::STRING));
        {
            let editor = Rc::downgrade(self);
            save_project.connect_activate(move |_, parameter| {
                let Some(editor) = editor.upgrade() else { return };
                let Some(path) = parameter.and_then(|p| p.get::<String>()) else {
                    warn!("save-project needs a path");
                    return;
                };
                editor.save_project(std::path::Path::new(&path));
            });
        }
        self.window.add_action(&save_project);

        // `dump-cursor (tool, path)`: writes one drawing cursor's image as a PNG. A
        // cursor cannot be seen in a nested shell -- its stage has no cursor sprite
        // (`docs/spikes/16`) -- so this is how `editor-test.sh` checks that the drawn
        // cursors (D57) are real images with a glyph in them rather than blank squares.
        let Some(kind) = glib::VariantTy::new("(ss)").ok() else { return };
        let dump = gio::SimpleAction::new("dump-cursor", Some(kind));
        dump.connect_activate(move |_, parameter| {
            let Some((tool, path)) = parameter.and_then(|p| p.get::<(String, String)>()) else {
                return;
            };
            let Some(tool) = octosnap_scene::Tool::ALL.into_iter().find(|t| t.label() == tool)
            else {
                warn!(tool, "dump-cursor: no such tool");
                return;
            };
            let texture = super::cursor::draw_drawing_cursor(tool, 32.0, 32.0 * 0.34, 2.0);
            match texture.save_to_png(&path) {
                Ok(()) => info!(tool = tool.label(), path, "cursor dumped"),
                Err(e) => warn!("could not dump the cursor: {e}"),
            }
        });
        self.window.add_action(&dump);

        // `print-to-file (path)`: §7's Print, exported to a PDF. The dialog needs a person.
        let print = gio::SimpleAction::new("print-to-file", Some(glib::VariantTy::STRING));
        {
            let editor = Rc::downgrade(self);
            print.connect_activate(move |_, parameter| {
                let Some(editor) = editor.upgrade() else { return };
                let Some(path) = parameter.and_then(|p| p.get::<String>()) else {
                    warn!("print-to-file needs a path");
                    return;
                };
                editor.print_to_pdf(std::path::Path::new(&path));
            });
        }
        self.window.add_action(&print);

        // `transform (name)`: `spec/05` §4.12's five, by name.
        //
        // The menu behind §4.11's `Image size` readout is the way a person reaches these,
        // and a menu inside a mode is not something a harness can open: entering crop mode
        // is a keypress, and the row that applies a rotate is a `GtkListBoxRow`. So this
        // is the same argument `dump-window` makes one line down -- without it the only
        // way to check that a rotate moves the arrows with the picture is to be sitting in
        // front of it. It is also the hook a shortcut or `spec/07`'s URL scheme would use
        // if §4.12 ever grows either.
        let transform = gio::SimpleAction::new("transform", Some(glib::VariantTy::STRING));
        {
            let editor = Rc::downgrade(self);
            transform.connect_activate(move |_, parameter| {
                let Some(editor) = editor.upgrade() else { return };
                let Some(name) = parameter.and_then(|p| p.get::<String>()) else {
                    warn!("transform needs a name");
                    return;
                };
                let Some(transform) = named_transform(&name, &editor) else {
                    warn!(name, "not one of spec/05 §4.12's transforms");
                    return;
                };
                editor.apply_transform(transform);
            });
        }
        self.window.add_action(&transform);

        // `add-image (path)`: `spec/05` §4.14's insert, without a pointer to drop with.
        //
        // The path may carry a zone after a `|`, which is how the four edge stitches are
        // reachable: `…/logo.png|left`. Bare means the middle of the picture, which is
        // what the Add image button does.
        let add_image = gio::SimpleAction::new("add-image", Some(glib::VariantTy::STRING));
        {
            let editor = Rc::downgrade(self);
            add_image.connect_activate(move |_, parameter| {
                let Some(editor) = editor.upgrade() else { return };
                let Some(argument) = parameter.and_then(|p| p.get::<String>()) else {
                    warn!("add-image needs a path");
                    return;
                };
                let (path, zone) = argument.split_once('|').unwrap_or((argument.as_str(), "in"));
                editor.add_image_at(std::path::Path::new(path), zone);
            });
        }
        self.window.add_action(&add_image);

        // `highlight (x0,y0,x1,y1[|ctrl])`: §4.7's snap, driven without a pointer.
        //
        // The same argument `transform` makes. The snap only happens inside a drag, and a
        // drag is the one thing a D-Bus harness cannot produce: `GtkGestureDrag` wants
        // real motion events from a real seat. So this builds the `Draft` the pointer
        // would have built, feeds it the two ends of the stroke, and commits it through
        // exactly the path a release takes -- `Draft::object`, `snap_highlighter`,
        // `Command::Add` -- which is the only way to check that the detector is reading
        // the right pixels at the right scale, and still reading them after a rotate.
        // `|ctrl` is the modifier §4.7 uses to skip the snap.
        let highlight = gio::SimpleAction::new("highlight", Some(glib::VariantTy::STRING));
        {
            let editor = Rc::downgrade(self);
            highlight.connect_activate(move |_, parameter| {
                let Some(editor) = editor.upgrade() else { return };
                let Some(argument) = parameter.and_then(|p| p.get::<String>()) else {
                    warn!("highlight needs x0,y0,x1,y1");
                    return;
                };
                let (numbers, held) = argument.split_once('|').unwrap_or((&argument, ""));
                let ends: Vec<f64> =
                    numbers.split(',').filter_map(|n| n.trim().parse().ok()).collect();
                let [x0, y0, x1, y1] = ends[..] else {
                    warn!(argument, "highlight needs four numbers");
                    return;
                };
                let modifiers =
                    Modifiers { ctrl: held.contains("ctrl"), ..Modifiers::default() };
                editor.highlight_between(Point::new(x0, y0), Point::new(x1, y1), modifiers);
            });
        }
        self.window.add_action(&highlight);

        // `trim-shadow`: `spec/08` §2's shadow switch, on whatever is open.
        //
        // The same argument as the three above. The trim runs when a *window* capture is
        // opened, and a window capture needs the shell extension and a window to point
        // at, so on a development machine the path is unreachable -- which is exactly the
        // kind of code that is wrong without anyone finding out. This ignores the mode
        // and not the setting: `window-shadow` still has to be off for anything to
        // happen, because that is the behaviour being checked.
        let trim = gio::SimpleAction::new("trim-shadow", None);
        {
            let editor = Rc::downgrade(self);
            trim.connect_activate(move |_, _| {
                if let Some(editor) = editor.upgrade() {
                    editor.trim_window_shadow();
                }
            });
        }
        self.window.add_action(&trim);

        // `dump-objects (path)`: the document as `objects.json`, for §11 item 4.
        // The window as it looks, chrome included, without a compositor screenshot.
        //
        // `spec/05` §11's list is about the *document*, and `render_texture` already
        // answers for that. This is for the half of the editor the document does not
        // cover: whether §4.13's sidebar laid out, whether a row wrapped, whether an icon
        // resolved. On a real GNOME session `org.gnome.Shell.Screenshot` answers "not
        // allowed", so without this the only way to look at the editor is to be sitting
        // in front of it.
        let dump_window = gio::SimpleAction::new("dump-window", Some(glib::VariantTy::STRING));
        {
            let editor = Rc::downgrade(self);
            dump_window.connect_activate(move |_, parameter| {
                let Some(editor) = editor.upgrade() else { return };
                let Some(path) = parameter.and_then(|p| p.get::<String>()) else {
                    warn!("dump-window needs a path");
                    return;
                };
                let editor_for_dump = Rc::downgrade(&editor);
                // Raised and given a frame first. A `GtkWidgetPaintable` hands back the
                // widget's *last drawn* node tree, and a window the compositor has
                // stopped drawing -- occluded, or never focused -- has none: "the window
                // produced no nodes", which is a blank file rather than a failure anyone
                // would read as one.
                editor.window.present();
                glib::timeout_add_local_once(std::time::Duration::from_millis(120), move || {
                    let Some(editor) = editor_for_dump.upgrade() else { return };
                    match editor.dump_window(std::path::Path::new(&path)) {
                        Some(()) => info!(path, "window dumped"),
                        None => warn!(path, "the window could not be rendered"),
                    }
                });
            });
        }
        self.window.add_action(&dump_window);

        let dump_objects = gio::SimpleAction::new("dump-objects", Some(glib::VariantTy::STRING));
        {
            let editor = Rc::downgrade(self);
            dump_objects.connect_activate(move |_, parameter| {
                let Some(editor) = editor.upgrade() else { return };
                let Some(path) = parameter.and_then(|p| p.get::<String>()) else {
                    warn!("dump-objects needs a path");
                    return;
                };
                editor.dump_objects(std::path::Path::new(&path));
            });
        }
        self.window.add_action(&dump_objects);

        // `close` and `final-close`: the window's two ways out (D108), for a harness that
        // cannot aim at the title bar's × or be sure which window has the keyboard. §11
        // item 10 opens and closes twenty editors with the second, which leaves no cards.
        for (name, run) in [
            ("close", Editor::close_to_preview as fn(&Rc<Editor>)),
            ("final-close", Editor::final_close),
        ] {
            let action = gio::SimpleAction::new(name, None);
            let editor = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(editor) = editor.upgrade() {
                    run(&editor);
                }
            });
            self.window.add_action(&action);
        }

        // `pick-screen-color`: §2's screen pipette without the popover in front of it. The
        // eyedropper button lives in a popover the harness cannot aim at, and the answer
        // is logged either way.
        let pick = gio::SimpleAction::new("pick-screen-color", None);
        {
            let editor = Rc::downgrade(self);
            pick.connect_activate(move |_, _| {
                let Some(editor) = editor.upgrade() else { return };
                (editor.actions.pick_screen_color)(Box::new(|answer| match answer {
                    Ok(Some(color)) => info!(color = color.to_hex(), "pipette sampled screen"),
                    Ok(None) => info!("pipette cancelled"),
                    Err(PickFailure::Unsupported) => {
                        warn!("the shell has no PickColor yet; log out once to load the new extension");
                    }
                    Err(PickFailure::Other(reason)) => warn!(reason, "pipette failed"),
                }));
            });
        }
        self.window.add_action(&pick);

        // `spec/07` §2.1's Copy Text, on the Copy button's menu.
        let recognise = gio::SimpleAction::new("recognise", None);
        {
            let editor = Rc::downgrade(self);
            recognise.connect_activate(move |_, _| {
                let Some(editor) = editor.upgrade() else { return };
                editor.copy_text();
            });
        }
        self.window.add_action(&recognise);
    }

    /// `spec/07` §2.1's Copy Text from the editor.
    ///
    /// Reads the **rendered** document at 1x, which is two decisions. Rendered, because
    /// the crop and the redactions are the point: a user who blacked out a password does
    /// not want it back through the text menu. And 1x, because the recogniser wants pixels
    /// and not logical units -- `spec/07` §2.2's detector upscales what it is given, so
    /// handing it the document's own resolution is handing it everything there is.
    pub(super) fn copy_text(self: &Rc<Self>) {
        let Some(capture) = self.rendered_capture(EXPORT_SCALE) else {
            crate::notify::action_failed(
                &self.app,
                "Copy Text",
                "the document could not be rendered",
            );
            return;
        };
        self.toast("Reading the text…", None);
        let editor = Rc::downgrade(self);
        let app = self.app.clone();
        (self.actions.recognise)(
            capture,
            Box::new(move |outcome| match editor.upgrade() {
                Some(editor) => editor.toast_read(&outcome),
                // Closed while it read: the notification is the one place left to say it.
                None => {
                    crate::notify::capture_outcome(&app, &outcome, true);
                }
            }),
        );
    }

    /// How a Copy Text read ended, in the editor rather than in a notification: the user
    /// asked from here and is looking here (`spec/13` #6). The buttons are the
    /// notification's -- **Show** for the text, **Open Settings** for a missing pack.
    fn toast_read(&self, outcome: &crate::flow::Outcome) {
        use crate::flow::Recognised;
        /// A button's label, its action, and the action's string target if it takes one.
        type Button = (&'static str, &'static str, Option<&'static str>);
        let (title, button): (String, Option<Button>) = match &outcome.recognised {
            Some(Recognised::Copied(_)) => ("Text copied".to_owned(), Some(("Show", "app.show-text", None))),
            Some(Recognised::Nothing) => ("No text found".to_owned(), None),
            Some(Recognised::Failed(why)) if why == crate::flow::NO_PACK => (
                "No language pack is installed".to_owned(),
                Some(("Open Settings", "app.open-settings", Some("advanced"))),
            ),
            Some(Recognised::Failed(why)) => (format!("Could not read the text: {why}"), None),
            None => return,
        };
        let toast = adw::Toast::new(&title);
        toast.set_timeout(toast_seconds(button.is_some()));
        if let Some((label, action, target)) = button {
            toast.set_button_label(Some(label));
            toast.set_action_name(Some(action));
            if let Some(target) = target {
                toast.set_action_target_value(Some(&target.to_variant()));
            }
        }
        info!(title, button = button.map(|b| b.0).unwrap_or_default(), "toast");
        self.toasts.add_toast(toast);
    }

    /// `spec/05` §1: "actions at bottom".
    ///
    /// §1's reading of the reference: "The bottom bar carries the zoom percentage as a menu
    /// on the left, the **Drag Me** handle centred with grip glyphs on both sides, and four
    /// output icons on the right." Three groups spread across the bar, in an
    /// `adw::WrapBox` rather than the `GtkCenterBox` that was here: with Final Close the
    /// buttons alone are wider than D57's 683 px -- the bar was already 694 px at its
    /// minimum before it, so a window tiled to half a 1366 px screen pushed Save off the
    /// edge -- and a wrap box puts them on a second line instead, right-aligned under the
    /// handle, the way the toolbar at the top already wraps (D108).
    pub(super) fn build_bottom_bar(self: &Rc<Self>) -> gtk::Widget {
        let bar = adw::WrapBox::new();
        bar.set_child_spacing(8);
        bar.set_line_spacing(6);
        bar.set_justify(adw::JustifyMode::Spread);
        bar.set_justify_last_line(true);
        bar.set_margin_start(12);
        bar.set_margin_end(12);
        bar.set_margin_top(6);
        bar.set_margin_bottom(6);

        let start = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        start.append(&self.build_zoom_menu());
        // The document's size in physical pixels, which used to be the window's subtitle
        // and is more use beside the zoom it is the 100 % of.
        let (w, h) = self.capture.rect.to_physical(self.capture.scale);
        self.size.set_label(&format!("{w} × {h}"));
        self.size.add_css_class("dim-label");
        self.size.add_css_class("numeric");
        start.append(&self.size);
        // `spec/13` #11: the marks that have not left the editor, as a dot. Hidden until
        // there are any; `refresh_unsaved` shows it.
        start.append(&self.unsaved);
        bar.append(&start);

        bar.append(&self.build_drag_button());

        let end = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        // On the right of its line whichever line that is: alone on a second line, a
        // group left to itself would sit at the start.
        end.set_halign(gtk::Align::End);
        // The user's own name for it (2026-09-23), and the one close that leaves no card.
        let final_close = gtk::Button::with_label("Final Close");
        final_close.set_tooltip_text(Some(
            "Close without leaving a preview; the capture goes to the history (Ctrl+Shift+W)",
        ));
        {
            let editor = Rc::downgrade(self);
            final_close.connect_clicked(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.final_close();
                }
            });
        }
        // Apart from the outputs: it leaves the editor, they do not (`spec/13` #4).
        final_close.set_margin_end(12);
        end.append(&final_close);
        // §7's list, minus the ones whose feature is not here: Upload is M7, Share and
        // OCR are Tier 2/3. A button that only writes to the journal is indistinguishable
        // from a broken one.
        // Labelled like its neighbours (`spec/13` #13): one icon-only button among four
        // labelled ones stands out for no reason.
        let print = gtk::Button::with_label("Print");
        print.set_tooltip_text(Some("Print (Ctrl+P)"));
        {
            let editor = Rc::downgrade(self);
            print.connect_clicked(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.print_rendered();
                }
            });
        }
        end.append(&print);

        let pin = gtk::Button::with_label("Pin");
        pin.set_tooltip_text(Some("Pin the rendered result and close (Ctrl+Shift+P); Alt keeps the editor"));
        {
            let editor = Rc::downgrade(self);
            pin.connect_clicked(move |_| {
                if let Some(editor) = editor.upgrade() {
                    let keep = editor.alt_held();
                    editor.pin_rendered(keep);
                }
            });
        }
        end.append(&pin);

        let save_as = gtk::Button::with_label("Save as…");
        save_as.set_tooltip_text(Some("Choose where to put it (Ctrl+Shift+S)"));
        {
            let editor = Rc::downgrade(self);
            save_as.connect_clicked(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.save_rendered_as();
                }
            });
        }
        end.append(&save_as);

        // A split button rather than a sixth plain one. `spec/07` §2.1 wants Copy Text
        // "from a card/pin/editor menu", and the editor had no menu of any kind -- while
        // the bar was already wide enough to wrap at the window's own minimum. Hanging the
        // text read off Copy costs no width and puts it where its neighbour is.
        let copy = adw::SplitButton::builder()
            .label("Copy")
            .tooltip_text("Copy the rendered image (Ctrl+C)")
            .build();
        {
            let editor = Rc::downgrade(self);
            copy.connect_clicked(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.copy_rendered();
                }
            });
        }
        let menu = gio::Menu::new();
        menu.append(Some("Copy Text"), Some("win.recognise"));
        copy.set_menu_model(Some(&menu));
        end.append(&copy);

        let save = gtk::Button::with_label("Save");
        save.add_css_class("suggested-action");
        save.set_tooltip_text(Some("Save it (Ctrl+S)"));
        {
            let editor = Rc::downgrade(self);
            save.connect_clicked(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.save_rendered();
                }
            });
        }
        end.append(&save);
        bar.append(&end);

        bar.upcast()
    }

    /// `spec/05` §1's "zoom percentage as a menu on the left".
    ///
    /// The label follows the canvas through `connect_zoom_changed`, so a wheel zoom, a
    /// key, and a refit on resize all update it; the menu offers §9's two named zooms and
    /// a ladder of fixed ones. The canvas keeps the document in view at every one of them
    /// (`Canvas::constrain_origin`), so none of these can strand the picture off-screen.
    fn build_zoom_menu(self: &Rc<Self>) -> gtk::Widget {
        let button = gtk::MenuButton::builder()
            .label(zoom_label(self.canvas.zoom()))
            .tooltip_text("Zoom (Ctrl+scroll, Ctrl+0 fits, Ctrl+1 is 100 %)")
            .build();
        button.add_css_class("numeric");
        button.set_width_request(80);

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        let entries: [(&str, Option<f64>); 7] = [
            ("Fit to window", None),
            ("25 %", Some(0.25)),
            ("50 %", Some(0.5)),
            ("100 %", Some(1.0)),
            ("200 %", Some(2.0)),
            ("400 %", Some(4.0)),
            ("800 %", Some(8.0)),
        ];
        for (text, _) in entries {
            let row = gtk::ListBoxRow::new();
            let label = gtk::Label::new(Some(text));
            label.set_xalign(0.0);
            label.set_margin_start(8);
            label.set_margin_end(8);
            label.set_margin_top(4);
            label.set_margin_bottom(4);
            row.set_child(Some(&label));
            list.append(&row);
        }
        {
            let editor = Rc::downgrade(self);
            list.connect_row_activated(move |list, row| {
                let Some(editor) = editor.upgrade() else { return };
                let index = usize::try_from(row.index()).unwrap_or(0);
                match entries.get(index).map(|(_, zoom)| *zoom) {
                    Some(None) => editor.canvas.glide_to_fit(editor.follow_view()),
                    Some(Some(zoom)) => {
                        let centre = (
                            f64::from(editor.canvas.width()) / 2.0,
                            f64::from(editor.canvas.height()) / 2.0,
                        );
                        editor.canvas.zoom_smoothly(zoom, centre, editor.follow_view());
                    }
                    None => return,
                }
                if let Some(popover) = list.ancestor(gtk::Popover::static_type()) {
                    popover.downcast_ref::<gtk::Popover>().map(gtk::Popover::popdown);
                }
            });
        }
        button.set_popover(Some(&gtk::Popover::builder().child(&list).build()));
        {
            let button = button.clone();
            self.canvas.connect_zoom_changed(move |zoom| button.set_label(&zoom_label(zoom)));
        }
        button.upcast()
    }

    /// `spec/05` §7: "Drag the *rendered* result (as PNG file) to any app".
    ///
    /// The render happens at drag-begin and not before: a bar built at startup would
    /// export an unannotated document, and re-exporting on every edit would encode a PNG
    /// per stroke. The same reasoning the cards use for their own drag source.
    fn build_drag_button(self: &Rc<Self>) -> gtk::Widget {
        let button = gtk::Button::with_label("Drag me");
        button.set_tooltip_text(Some("Drag the rendered image into another app"));
        let source = gtk::DragSource::new();
        source.set_actions(gtk::gdk::DragAction::COPY);
        {
            let editor = Rc::downgrade(self);
            source.connect_prepare(move |_, _, _| {
                let editor = editor.upgrade()?;
                let path = editor.export_png(editor.export_scale())?;
                let file = gio::File::for_path(&path);
                // Order is the contract, as it is for a card: a file first, so a file
                // manager gets a file, and the texture second for apps that want pixels.
                let mut providers =
                    vec![gtk::gdk::ContentProvider::for_value(&file.to_value())];
                if let Some(texture) = editor.canvas.render_texture(editor.export_scale()) {
                    providers.push(gtk::gdk::ContentProvider::for_value(&texture.to_value()));
                }
                Some(gtk::gdk::ContentProvider::new_union(&providers))
            });
        }
        button.add_controller(source);
        button.upcast()
    }

    /// `spec/05` §7: "Copies the rendered image (at document scale, Retina-aware) + file".
    ///
    /// And stays open, which is [P→V] against §7's "closes unless Alt held" (D108). The
    /// close was `spec/09` §1's "invoke → act → vanish", and on hardware it read as the
    /// editor throwing the document away the moment it was copied -- "when I CTRL+C, dont
    /// close it" (2026-09-23). Copying is often the middle of the work -- paste it, look,
    /// fix an arrow, copy again -- and the window is one click from gone either way.
    pub(super) fn copy_rendered(self: &Rc<Self>) {
        let Some(capture) = self.rendered_capture(self.export_scale()) else {
            crate::notify::action_failed(&self.app, "Copy", "the document could not be rendered");
            return;
        };
        let editor = Rc::downgrade(self);
        (self.actions.copy)(
            capture,
            Box::new(move |outcome| {
                let Some(editor) = editor.upgrade() else { return };
                if outcome.is_err() {
                    return;
                }
                editor.mark_saved();
                editor.toast("Copied", None);
            }),
        );
    }

    /// `spec/05` §7: "Saves in place … or to the default folder with the template".
    ///
    /// The window stays: §7 gives Alt another meaning here ("Alt bypasses the dialog"),
    /// so it cannot also mean "keep the window", and a save is often the middle of a
    /// session rather than its end. The toast names the file and offers `spec/04` §7's
    /// "Show in Files", which is the app's `reveal-last` -- the flow has just recorded
    /// this path as the last saved.
    pub(super) fn save_rendered(self: &Rc<Self>) {
        let Some(capture) = self.rendered_capture(self.export_scale()) else {
            crate::notify::action_failed(&self.app, "Save", "the document could not be rendered");
            return;
        };
        let editor = Rc::downgrade(self);
        (self.actions.save)(
            capture,
            Box::new(move |outcome| {
                let Some(editor) = editor.upgrade() else { return };
                if let Ok(path) = outcome {
                    editor.mark_saved_to(path.as_deref());
                    editor.toast_saved(path.as_deref());
                }
            }),
        );
    }

    /// One "Saved as …" toast, with *Show in Files* when there is a file to show.
    fn toast_saved(&self, path: Option<&std::path::Path>) {
        match path.and_then(|p| p.file_name()) {
            Some(name) => self.toast(
                &format!("Saved as {}", name.to_string_lossy()),
                Some(("Show in Files", "app.reveal-last")),
            ),
            None => self.toast("Saved", None),
        }
    }

    /// Renders the whole window to a PNG. The `dump-window` action's body.
    fn dump_window(&self, path: &std::path::Path) -> Option<()> {
        let (width, height) = (self.window.width(), self.window.height());
        if width <= 0 || height <= 0 {
            return None;
        }
        let Some(renderer) = self.window.native().and_then(|native| native.renderer()) else {
            warn!("dump-window: the window has no renderer");
            return None;
        };
        let paintable = gtk::WidgetPaintable::new(Some(&self.window));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
        let Some(node) = snapshot.to_node() else {
            warn!(width, height, "dump-window: the window produced no nodes");
            return None;
        };
        #[allow(clippy::cast_precision_loss)]
        let bounds = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
        let texture = renderer.render_texture(&node, Some(&bounds));
        if let Err(why) = texture.save_to_png(path) {
            warn!(path = %path.display(), "dump-window: {why}");
            return None;
        }
        Some(())
    }

    /// A short confirmation inside the editor (`spec/05` §10's "Copy/Save success toast";
    /// `spec/13` #6). Two seconds -- the table's 1.5 s rounded up to the unit `AdwToast`
    /// counts in -- and one line in the log, which is how the harness sees it.
    ///
    /// Four with a button: two seconds is time to read a word, not to find a button and
    /// reach it with the pointer.
    pub(super) fn toast(&self, title: &str, button: Option<(&str, &str)>) {
        let toast = adw::Toast::new(title);
        toast.set_timeout(toast_seconds(button.is_some()));
        if let Some((label, action)) = button {
            toast.set_button_label(Some(label));
            toast.set_action_name(Some(action));
        }
        info!(title, button = button.map(|b| b.0).unwrap_or_default(), "toast");
        self.toasts.add_toast(toast);
    }

    /// Whether Alt is down right now, for Pin: "closes unless Alt held" (D61) has to be
    /// read at the click, and a `clicked` signal carries no modifiers.
    pub(super) fn alt_held(&self) -> bool {
        gtk::prelude::WidgetExt::display(&self.window)
            .default_seat()
            .and_then(|seat| seat.keyboard())
            .is_some_and(|keyboard| {
                keyboard.modifier_state().contains(gtk::gdk::ModifierType::ALT_MASK)
            })
    }

    /// `spec/05` §7: "File chooser with format selector (PNG/JPG/WebP) and project format".
    ///
    /// The editor owns this chooser rather than delegating it, because the project format
    /// is not something the capture flow can write: it is a document, not a rendering. So
    /// the filter decides which of two entirely different things happens, and the image
    /// branch hands the chosen path back to the flow so §7's filename template, `last
    /// saved` and "Show in Files" all keep working.
    ///
    /// The **name** decides, not the filter, and that is deliberate: a chooser's filter is
    /// a hint about what to show, and a user who types `notes.octosnap` under the PNG
    /// filter has said what they want more clearly than the dropdown has.
    pub(super) fn save_rendered_as(self: &Rc<Self>) {
        let images = gtk::FileFilter::new();
        images.set_name(Some("Image"));
        images.add_mime_type("image/png");
        images.add_mime_type("image/jpeg");
        images.add_mime_type("image/webp");
        let projects = gtk::FileFilter::new();
        projects.set_name(Some("OctoSnap project"));
        projects.add_pattern(&format!("*.{}", project::EXTENSION));
        projects.add_pattern(&format!("*.{}", project::LEGACY_EXTENSION));
        projects.add_mime_type(project::MIME_TYPE);

        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&images);
        filters.append(&projects);

        let stem = self
            .capture
            .path
            .file_stem()
            .map_or_else(|| "Screenshot".to_owned(), |s| s.to_string_lossy().into_owned());
        // `shot-format`'s extension, which the flow then writes (D164); the name the user
        // ends up with is what decides.
        let format = crate::capture_flow().map(|flow| flow.image_format()).unwrap_or_default();
        let dialog = gtk::FileDialog::builder()
            .title("Save As")
            .initial_name(format!("{stem}.{}", format.extension()))
            .filters(&filters)
            .default_filter(&images)
            .modal(false)
            .build();

        let editor = Rc::downgrade(self);
        let window = self.window.clone();
        glib::spawn_future_local(async move {
            let chosen = dialog.save_future(Some(&window)).await;
            let Some(editor) = editor.upgrade() else { return };
            let path = match chosen {
                Ok(file) => match file.path() {
                    Some(path) => path,
                    None => {
                        warn!("the chooser returned a file with no path");
                        return;
                    }
                },
                // Dismissed, which is not a failure.
                Err(e) => {
                    info!("the save chooser was dismissed: {e}");
                    return;
                }
            };

            if project::is_project(&path) {
                if editor.save_project(&path) {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                    editor.toast(
                        &format!("Saved the project as {}", name.unwrap_or_default()),
                        None,
                    );
                }
                return;
            }
            let Some(capture) = editor.rendered_capture(editor.export_scale()) else {
                crate::notify::action_failed(
                    &editor.app,
                    "Save as",
                    "the document could not be rendered",
                );
                return;
            };
            let weak = Rc::downgrade(&editor);
            (editor.actions.save_as_path)(
                capture,
                path,
                Box::new(move |outcome| {
                    let Some(editor) = weak.upgrade() else { return };
                    if let Ok(path) = outcome {
                        editor.mark_saved_to(path.as_deref());
                        editor.toast_saved(path.as_deref());
                    }
                }),
            );
        });
    }

    /// `spec/05` §7: "Pin the rendered result at the capture location".
    ///
    /// Closes unless Alt is held: the pin *is* the result, on the screen where the picture
    /// came from, and the editor has handed the marks over (D61). §7 says nothing either
    /// way, so this is [P] and the card's own Pin -- which closes the card, D47 -- is the
    /// precedent. The close is a final one (D108): the pin holds the render now, and a
    /// card with the same picture beside it would be the capture in two places.
    pub(super) fn pin_rendered(self: &Rc<Self>, keep: bool) {
        let Some(capture) = self.rendered_capture(self.export_scale()) else {
            crate::notify::action_failed(&self.app, "Pin", "the document could not be rendered");
            return;
        };
        if !(self.actions.pin)(capture) {
            return;
        }
        self.mark_saved();
        if keep {
            self.toast("Pinned", None);
        } else {
            info!("pinned; the editor closes");
            self.closing.set(Closing::Pinned);
            self.window.close();
        }
    }

    /// Remembers that the document has left the editor, for the unsaved dot.
    fn mark_saved(&self) {
        self.saved_at.set(self.history.borrow().depth());
        self.refresh_unsaved();
    }

    /// [`Self::mark_saved`], and where to: a card the editor leaves behind for exactly
    /// this state is a card whose picture is already on disk, and offers Trash rather
    /// than a second Save (D47).
    fn mark_saved_to(&self, path: Option<&std::path::Path>) {
        self.mark_saved();
        if let Some(path) = path {
            *self.saved_file.borrow_mut() = Some((self.saved_at.get(), path.to_path_buf()));
        }
    }

    /// Whether marks exist that have not left the editor -- §7's Close question, asked
    /// without closing.
    pub(super) fn has_unsaved(&self) -> bool {
        let depth = self.history.borrow().depth();
        depth != 0 && depth != self.saved_at.get()
    }

    /// The unsaved indicator (`spec/13` #11): a dot beside the document size and a
    /// leading bullet on the window's title, which is where the shell's overview and
    /// task switcher show it -- the editor's own title bar carries the tools instead.
    pub(super) fn refresh_unsaved(&self) {
        let unsaved = self.has_unsaved();
        if self.unsaved.is_visible() != unsaved {
            tracing::debug!(unsaved, "unsaved indicator");
        }
        self.unsaved.set_visible(unsaved);
        self.window.set_title(Some(if unsaved { "• Annotate" } else { "Annotate" }));
    }

    /// `spec/05` §7's Close, Ctrl+W: the window goes and the capture goes back to the
    /// stack as a card (D108) -- which is what the title bar's × does too, through the
    /// same close-request.
    pub(super) fn close_to_preview(self: &Rc<Self>) {
        self.closing.set(Closing::ToPreview);
        self.window.close();
    }

    /// Final Close, Ctrl+Shift+W: the window goes and leaves no card behind (D108).
    ///
    /// No question first, and that is the point of the button rather than a gap in it.
    /// §7's Close asked "Discard the annotations?" because closing threw them away; here
    /// nothing is thrown away -- the capture, and a render of the document if there is
    /// one, go where every closed card goes, the history -- so there is nothing to ask.
    pub(super) fn final_close(self: &Rc<Self>) {
        self.closing.set(Closing::Final);
        self.window.close();
    }

    /// What the window leaves behind (D108), from its close-request, exactly once.
    ///
    /// `spec/05` §7 made Close a question -- "prompts only if unsaved and the capture is
    /// not otherwise persisted" -- and a question was the wrong shape for what the user
    /// asked for on 2026-09-23: "closing the editor with X just re-adds it as a preview
    /// with all its editing done within it". So closing never discards and never asks. An
    /// untouched document goes back as the capture it was; an edited one as a render that
    /// is a capture of its own, carrying the document -- objects, undo stack and all --
    /// so the card's Annotate picks up exactly where this left off.
    pub(super) fn hand_back(self: &Rc<Self>) {
        if self.handed_back.replace(true) {
            return;
        }
        // Whatever is half-done lands or goes first: a text box being typed into is part
        // of the document, and a crop that was never applied is not.
        if self.text_editing() {
            self.commit_text_edit();
        }
        self.cancel_crop();
        let closing = self.closing.get();
        let returned = if closing == Closing::Pinned || self.untouched() {
            Returned::Unchanged
        } else if let Some(document) = self.keep_document() {
            Returned::Edited(Box::new(document))
        } else {
            crate::notify::action_failed(
                &self.app,
                "Close",
                "the annotations could not be rendered, so the capture went back without them",
            );
            Returned::Unchanged
        };
        info!(?closing, edited = matches!(returned, Returned::Edited(_)), "editor closing");
        (self.actions.closed)(match closing {
            Closing::ToPreview => Closed::Preview(returned),
            Closing::Final | Closing::Pinned => Closed::Final(returned),
        });
    }

    /// The document as a capture of its own, and the document itself (D108).
    ///
    /// The render goes into the spool beside every other capture rather than beside the
    /// one it was drawn on: a project's base image lives in the cache, and a capture the
    /// janitor cannot see is a capture nothing ever files. It is described as the capture
    /// it came from, with the four fields that would make it act like a fresh one cleared
    /// -- no fly-in, no plan, no window to trim a shadow off -- and Shift set, which
    /// `spec/05` §4.13 reads as "skip the automatic background": this picture already has
    /// whatever background the document had, and one restored from the history and opened
    /// again must not get a second.
    fn keep_document(&self) -> Option<Document> {
        let spool = crate::history::History::default_spool();
        if let Err(e) = std::fs::create_dir_all(&spool) {
            warn!(spool = %spool.display(), "could not create the spool: {e}");
            return None;
        }
        let path = spool.join(format!("{}.png", octosnap_core::capture::fresh_id()));
        let scale = self.export_scale();
        let texture = self.canvas.render_texture(scale)?;
        if !Self::encode_png(&texture, &path) {
            return None;
        }
        let described = self.described(path, scale)?;
        let render = CaptureResult {
            meta_path: described.path.with_extension("json"),
            mode: match described.mode {
                octosnap_core::CaptureMode::Window => octosnap_core::CaptureMode::Area,
                mode => mode,
            },
            confirmed_at: None,
            animation_ms: 0,
            requested_action: None,
            modifiers: super::window::SHIFT_MASK,
            ..described
        };
        crate::import::write_twin(&render);
        let depth = self.history.borrow().depth();
        let saved_file = self.saved_file.borrow().clone();
        let saved_to =
            saved_file.as_ref().filter(|(at, _)| *at == depth).map(|(_, path)| path.clone());
        info!(path = %render.path.display(), depth, "the document went back as a render");
        Some(Document {
            render,
            saved_to,
            capture: self.capture.clone(),
            scene: self.canvas.scene()?,
            history: std::mem::take(&mut *self.history.borrow_mut()),
            saved_at: self.saved_at.get(),
            saved_file,
        })
    }
}

/// A rendered texture as a Cairo image surface, for the printer.
///
/// `B8g8r8a8Premultiplied` is Cairo's `ARGB32` on a little-endian machine, which is what
/// lets the bytes be handed over without a conversion pass. Premultiplied is right here
/// and wrong for the PNG encoder -- see `encode_png` -- because Cairo composites
/// premultiplied and PNG stores straight.
fn cairo_surface(texture: &gtk::gdk::Texture) -> Option<gtk::cairo::ImageSurface> {
    let (bytes, stride) = {
        let mut downloader = gtk::gdk::TextureDownloader::new(texture);
        downloader.set_format(gtk::gdk::MemoryFormat::B8g8r8a8Premultiplied);
        downloader.download_bytes()
    };
    let stride = i32::try_from(stride).ok()?;
    gtk::cairo::ImageSurface::create_for_data(
        bytes.to_vec(),
        gtk::cairo::Format::ARgb32,
        texture.width(),
        texture.height(),
        stride,
    )
    .inspect_err(|e| warn!("could not wrap the render for printing: {e}"))
    .ok()
}

/// "63 %", the way the reference writes it (`39%` on the bottom bar of `annotate-editor.png`).
fn zoom_label(zoom: f64) -> String {
    format!("{:.0} %", zoom * 100.0)
}

/// One of `spec/05` §4.12's transforms by name, with `resize` reading its size from the
/// name itself: `resize:1280x720`, or `resize:50%`.
fn named_transform(
    name: &str,
    editor: &Editor,
) -> Option<octosnap_scene::transform::Transform> {
    use octosnap_scene::transform::Transform;
    if let Some(size) = name.strip_prefix("resize:") {
        let base = editor.canvas.scene()?.base;
        if let Some(percent) = size.strip_suffix('%') {
            return Some(Transform::percent(&base, percent.parse().ok()?));
        }
        let (width, height) = size.split_once('x')?;
        return Some(Transform::Resize {
            width: width.parse().ok()?,
            height: height.parse().ok()?,
        });
    }
    Some(match name {
        "rotate-left" => Transform::RotateLeft,
        "rotate-right" => Transform::RotateRight,
        "flip-horizontal" => Transform::FlipHorizontal,
        "flip-vertical" => Transform::FlipVertical,
        _ => return None,
    })
}
