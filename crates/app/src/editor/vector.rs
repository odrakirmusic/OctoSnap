// SPDX-License-Identifier: GPL-3.0-or-later

//! `ANN-33` (D184): the document as SVG or PDF, with the marks as vectors over the picture.
//!
//! Two layers, from the one node tree the canvas and the PNG export draw (`spec/05` §6).
//! The picture -- the base image, a background, and the objects that belong to the picture:
//! redactions, inserted pictures and spotlights -- is flattened into one image at the
//! export scale. The marks over it -- shapes, arrows, strokes, text and badges -- are drawn
//! into cairo's SVG or PDF surface as paths and glyphs (`gsk::RenderNode::draw`). The
//! picture is flattened *with* its redactions on purpose: drawn as layers, the capture
//! would be in the file whole, under the thing that was meant to hide part of it.
//!
//! The page is the picture's size on the screen it was taken from: the capture's pixels
//! divided by its scale, in CSS pixels for SVG and at 96 to the inch for PDF. The image in
//! it keeps every pixel the PNG export would have.

use std::path::Path;
use std::time::Instant;

use gtk::prelude::*;
use gtk::{cairo, gdk, gsk};
use tracing::{info, warn};

use super::nodes::Part;
use super::window::Editor;

/// PDF's points per CSS pixel: 72 to the inch against 96.
const POINTS_PER_PIXEL: f64 = 0.75;

/// The two vector formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vector {
    Svg,
    Pdf,
}

impl Vector {
    /// The format a file's name asks for, if it is one of these.
    #[must_use]
    pub fn of(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "svg" => Some(Self::Svg),
            "pdf" => Some(Self::Pdf),
            _ => None,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Svg => "SVG",
            Self::Pdf => "PDF",
        }
    }
}

impl Editor {
    /// Writes the document to `path` as `format`. Answers whether it did.
    pub(super) fn export_vector(&self, path: &Path, format: Vector) -> bool {
        let started = Instant::now();
        let Some(scene) = self.canvas.scene() else { return false };
        let scale = self.export_scale();
        let Some(picture) = self.canvas.render_part(scale, Part::Picture) else {
            warn!("nothing to export");
            return false;
        };
        let marks = self.canvas.marks_node(&picture, scale);
        let canvas = scene.canvas;
        // Document units are the capture's pixels; its scale makes them the desktop's.
        let per_unit = 1.0 / self.capture.scale.max(f64::EPSILON);
        let page = Page { width: canvas.width, height: canvas.height, per_unit, scale };
        let written = match format {
            Vector::Svg => write_svg(path, &page, &picture, marks.as_ref()),
            Vector::Pdf => write_pdf(path, &page, &picture, marks.as_ref()),
        };
        match written {
            Ok(()) => {
                let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                // The canvas and both scales too, which `check-vector.py` needs to read
                // the file back against a PNG export of the same document.
                info!(
                    path = %path.display(),
                    format = format.name(),
                    width = format!("{:.1}", canvas.width * per_unit),
                    height = format!("{:.1}", canvas.height * per_unit),
                    canvas = format!(
                        "{:.2} {:.2} {:.2} {:.2}",
                        canvas.x, canvas.y, canvas.width, canvas.height
                    ),
                    capture_scale = format!("{:.4}", self.capture.scale),
                    picture_scale = format!("{scale:.4}"),
                    picture = format!("{}x{}", picture.width(), picture.height()),
                    marks = marks.is_some(),
                    bytes,
                    ms = format!("{:.1}", started.elapsed().as_secs_f64() * 1000.0),
                    "exported as vectors"
                );
                true
            }
            Err(e) => {
                warn!(path = %path.display(), format = format.name(), "the export failed: {e}");
                false
            }
        }
    }
}

/// The document's size, and how its units become the page's.
struct Page {
    /// In document units.
    width: f64,
    height: f64,
    /// The page's units -- CSS pixels -- per document unit.
    per_unit: f64,
    /// The picture's pixels per document unit.
    scale: f64,
}

fn write_svg(
    path: &Path,
    page: &Page,
    picture: &gdk::Texture,
    marks: Option<&gsk::RenderNode>,
) -> Result<(), String> {
    let mut surface =
        cairo::SvgSurface::new(page.width * page.per_unit, page.height * page.per_unit, Some(path))
            .map_err(|e| e.to_string())?;
    surface.set_document_unit(cairo::SvgUnit::Px);
    let cr = cairo::Context::new(&surface).map_err(|e| e.to_string())?;
    draw(&cr, page.per_unit, page.scale, picture, marks)?;
    drop(cr);
    surface.finish();
    surface.status().map_err(|e| e.to_string())
}

fn write_pdf(
    path: &Path,
    page: &Page,
    picture: &gdk::Texture,
    marks: Option<&gsk::RenderNode>,
) -> Result<(), String> {
    let per_unit = page.per_unit * POINTS_PER_PIXEL;
    let surface = cairo::PdfSurface::new(page.width * per_unit, page.height * per_unit, path)
        .map_err(|e| e.to_string())?;
    let _ = surface.set_metadata(cairo::PdfMetadata::Creator, "OctoSnap");
    let cr = cairo::Context::new(&surface).map_err(|e| e.to_string())?;
    draw(&cr, per_unit, page.scale, picture, marks)?;
    drop(cr);
    surface.finish();
    surface.status().map_err(|e| e.to_string())
}

/// The picture, then the marks over it, in document units scaled to the page's.
fn draw(
    cr: &cairo::Context,
    per_unit: f64,
    scale: f64,
    picture: &gdk::Texture,
    marks: Option<&gsk::RenderNode>,
) -> Result<(), String> {
    cr.scale(per_unit, per_unit);
    let image = image_of(picture)?;
    cr.save().map_err(|e| e.to_string())?;
    cr.scale(1.0 / scale, 1.0 / scale);
    cr.set_source_surface(&image, 0.0, 0.0).map_err(|e| e.to_string())?;
    cr.paint().map_err(|e| e.to_string())?;
    cr.restore().map_err(|e| e.to_string())?;
    if let Some(marks) = marks {
        marks.draw(cr);
    }
    Ok(())
}

/// A texture as cairo's image: premultiplied, in the byte order cairo's ARGB32 is.
fn image_of(texture: &gdk::Texture) -> Result<cairo::ImageSurface, String> {
    let (width, height) = (texture.width(), texture.height());
    let format = cairo::Format::ARgb32;
    let stride = format
        .stride_for_width(u32::try_from(width).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let mut downloader = gdk::TextureDownloader::new(texture);
    downloader.set_format(if cfg!(target_endian = "little") {
        gdk::MemoryFormat::B8g8r8a8Premultiplied
    } else {
        gdk::MemoryFormat::A8r8g8b8Premultiplied
    });
    let (bytes, from_stride) = downloader.download_bytes();
    let (rows, row) = (
        usize::try_from(height).map_err(|e| e.to_string())?,
        usize::try_from(stride).map_err(|e| e.to_string())?,
    );
    let used = usize::try_from(width).map_err(|e| e.to_string())? * 4;
    let mut data = vec![0_u8; row * rows];
    for y in 0..rows {
        let from = bytes.get(y * from_stride..y * from_stride + used).ok_or("a short download")?;
        data[y * row..y * row + used].copy_from_slice(from);
    }
    cairo::ImageSurface::create_for_data(data, format, width, height, stride)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use gtk::graphene;

    #[test]
    fn the_extension_names_the_format() {
        assert_eq!(Vector::of(Path::new("/a/b.svg")), Some(Vector::Svg));
        assert_eq!(Vector::of(Path::new("Shot.PDF")), Some(Vector::Pdf));
        assert_eq!(Vector::of(Path::new("shot.png")), None);
        assert_eq!(Vector::of(Path::new("svg")), None);
    }

    /// A 4 x 2 red picture taken at a scale of 2, and a blue mark over its right half.
    fn document() -> (Page, gdk::Texture, gsk::RenderNode) {
        let pixels: Vec<u8> = [255, 0, 0, 255].repeat(8);
        let bytes = gtk::glib::Bytes::from_owned(pixels);
        let picture =
            gdk::MemoryTexture::new(4, 2, gdk::MemoryFormat::R8g8b8a8, &bytes, 16).upcast();
        let mark = gsk::ColorNode::new(
            &gdk::RGBA::new(0.0, 0.0, 1.0, 1.0),
            &graphene::Rect::new(2.0, 0.0, 2.0, 2.0),
        )
        .upcast();
        (Page { width: 4.0, height: 2.0, per_unit: 0.5, scale: 1.0 }, picture, mark)
    }

    /// The page is the picture's size on its screen, the picture keeps all its pixels,
    /// and the mark is drawn, not pasted.
    #[test]
    fn an_svg_is_one_image_under_the_marks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.svg");
        let (page, picture, mark) = document();
        write_svg(&path, &page, &picture, Some(&mark)).unwrap();
        let svg = std::fs::read_to_string(&path).unwrap();
        assert!(svg.contains(r#"width="2px""#) && svg.contains(r#"height="1px""#), "{svg}");
        assert_eq!(svg.matches("<image").count(), 1, "{svg}");
        assert!(svg.contains(r#"width="4""#) && svg.contains(r#"height="2""#), "{svg}");
        assert!(svg.contains("<path") || svg.contains("<rect"), "{svg}");
        assert!(svg.contains("rgb(0%, 0%, 100%)") || svg.contains("#0000ff"), "{svg}");
    }

    /// One image, every pixel of the picture; the mark is not a second one. (The page's
    /// size is in a compressed object stream, which `check-vector.py` reads.)
    #[test]
    fn a_pdf_is_one_image_under_the_marks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.pdf");
        let (page, picture, mark) = document();
        write_pdf(&path, &page, &picture, Some(&mark)).unwrap();
        let pdf = std::fs::read(&path).unwrap();
        let text = String::from_utf8_lossy(&pdf);
        assert!(pdf.starts_with(b"%PDF-"));
        assert_eq!(text.matches("/Subtype /Image").count(), 1, "{text}");
        assert!(text.contains("/Width 4") && text.contains("/Height 2"), "{text}");
    }

    /// cairo's ARGB32 is premultiplied, in native byte order, with its own stride.
    #[test]
    fn a_texture_becomes_cairos_image_pixel_for_pixel() {
        let (_, picture, _) = document();
        let mut image = image_of(&picture).unwrap();
        let stride = usize::try_from(image.stride()).unwrap();
        let data = image.data().unwrap();
        let pixel = u32::from_ne_bytes(data[stride..stride + 4].try_into().unwrap());
        assert_eq!(pixel, 0xffff_0000);
    }
}
