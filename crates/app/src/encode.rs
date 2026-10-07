// SPDX-License-Identifier: GPL-3.0-or-later

//! `ACT-06`: a screenshot written in the format the user chose (D164).
//!
//! Every still in the spool is a PNG. The extension writes one, an opened file becomes
//! one (`import`), and the editor renders one. A save used to copy those bytes under the
//! extension `shot-format` named, so a "JPEG" was a PNG called `.jpg`, a "WebP" a PNG
//! called `.webp`, and `shot-jpg-quality` was read by nothing. This is the step between the
//! spool and the export location: decode the PNG, write the format.
//!
//! Blocking, and the flow runs it on `gio::spawn_blocking` (`spec/10` §7). On the target
//! machine a 3420 x 2214 screenshot decodes in 69 ms and is written as a JPEG at 90 in
//! 111 ms or as a WebP in 42 ms, where the PNG encoder takes 43.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use octosnap_core::savepath::ImageFormat;

/// What JPEG puts where a picture is see-through, since it has no alpha to keep it in.
///
/// White, which is what the shadow and the rounded corners of a window capture
/// (`CAP-03`) were drawn to fall on: a shadow over white is a shadow, over black it
/// disappears, and the documents and pages a JPEG goes into are white more often than
/// not. A window that should sit on something else is what `window-background` and the
/// editor's background are for, and they arrive here already opaque.
pub const MATTE: [u8; 3] = [0xff, 0xff, 0xff];

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("the capture could not be read: {0}")]
    Decode(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("the {format} encoder failed: {message}")]
    Encode { format: &'static str, message: String },
    #[error("the encoder thread panicked")]
    Panicked,
}

/// What a file's first bytes say about it: its format, and its size when it is a PNG.
/// Twenty-four bytes are a PNG's signature and the width and height of its header.
///
/// A read of those bytes and nothing else, so the flow can ask on the main loop: it
/// decides there whether a save is a copy or an encode, and whether the format can
/// hold the picture, before it names the file.
#[must_use]
pub fn peek(path: &Path) -> (Option<ImageFormat>, Option<(u32, u32)>) {
    let mut head = [0u8; 24];
    let read = File::open(path).and_then(|mut file| {
        let mut filled = 0;
        while filled < head.len() {
            match file.read(&mut head[filled..])? {
                0 => break,
                n => filled += n,
            }
        }
        Ok(filled)
    });
    let Ok(filled) = read else { return (None, None) };
    let head = &head[..filled];
    let format = ImageFormat::sniff(head);
    let size = (format == Some(ImageFormat::Png) && head.len() == 24 && head[12..16] == *b"IHDR")
        .then(|| {
            let be = |at: usize| u32::from_be_bytes([head[at], head[at + 1], head[at + 2], head[at + 3]]);
            (be(16), be(20))
        });
    (format, size)
}

/// Writes the PNG at `source` to `destination` as `format`.
///
/// `quality` is JPEG's 1 to 100 and means nothing to the two lossless formats. `replace`
/// is whether a file already at `destination` is written over -- a Save As, whose chooser
/// asked -- or is an error -- a Save, which picked a free name and must not take one that
/// was filled since. Either way a failure leaves no half-written file: a replacement is
/// written beside the old file and renamed over it, so a failed one leaves the old one.
pub fn transcode(
    source: &Path,
    destination: &Path,
    format: ImageFormat,
    quality: u8,
    replace: bool,
) -> Result<(), EncodeError> {
    let (rgba, width, height) = crate::history::thumbnail::decode_png(source).map_err(EncodeError::Decode)?;
    let (Ok(w), Ok(h)) = (u32::try_from(width), u32::try_from(height)) else {
        return Err(EncodeError::Decode(format!("{width} x {height} is too large")));
    };
    if rgba.len() != width.saturating_mul(height).saturating_mul(4) {
        return Err(EncodeError::Decode(format!("{} bytes for {width} x {height}", rgba.len())));
    }
    write_atomically(destination, replace, |out| encode(&rgba, w, h, format, quality, out))
}

/// Straight 8-bit RGBA, `width * height * 4` bytes of it, into `out` as `format`.
pub fn encode(
    rgba: &[u8],
    width: u32,
    height: u32,
    format: ImageFormat,
    quality: u8,
    out: &mut dyn Write,
) -> Result<(), EncodeError> {
    let failed = |message: String| EncodeError::Encode { format: name(format), message };
    match format {
        ImageFormat::Png => {
            let mut encoder = png::Encoder::new(out, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            // The editor's export and the scrolling capture make the same call.
            encoder.set_compression(png::Compression::Fast);
            let mut writer = encoder.write_header().map_err(|e| failed(e.to_string()))?;
            writer.write_image_data(rgba).map_err(|e| failed(e.to_string()))?;
            writer.finish().map_err(|e| failed(e.to_string()))
        }
        ImageFormat::Jpg => {
            let (Ok(w), Ok(h)) = (u16::try_from(width), u16::try_from(height)) else {
                return Err(failed(format!("{width} x {height} is past JPEG's 65535 a side")));
            };
            let rgb = flatten(rgba, MATTE);
            // Optimised Huffman tables are a few per cent off the file for a second pass
            // over the coefficients. The chroma is the encoder's own choice and the right
            // one for text: full resolution from 90 up, which is the default, and halved
            // below it, where the user has asked for a small file over a sharp one.
            let mut encoder = jpeg_encoder::Encoder::new(out, quality.clamp(1, 100));
            encoder.set_optimized_huffman_tables(true);
            encoder.encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb).map_err(|e| failed(e.to_string()))
        }
        ImageFormat::Webp => {
            if width == 0 || height == 0 || width > format.max_side() || height > format.max_side() {
                return Err(failed(format!("{width} x {height} is past WebP's 16384 a side")));
            }
            // Always with its alpha: an opaque screenshot written without it measured the
            // same size to the kilobyte, so stripping it would be a copy for nothing.
            image_webp::WebPEncoder::new(out)
                .encode(rgba, width, height, image_webp::ColorType::Rgba8)
                .map_err(|e| failed(e.to_string()))
        }
    }
}

/// Straight RGBA composited over `matte` and the alpha dropped: what JPEG can hold.
#[must_use]
pub fn flatten(rgba: &[u8], matte: [u8; 3]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);
    for &[r, g, b, a] in rgba.as_chunks::<4>().0 {
        let alpha = u32::from(a);
        for (channel, behind) in [r, g, b].iter().zip(matte) {
            let mixed = (u32::from(*channel) * alpha + u32::from(behind) * (255 - alpha) + 127) / 255;
            rgb.push(u8::try_from(mixed).unwrap_or(u8::MAX));
        }
    }
    rgb
}

/// The format's name in a sentence.
const fn name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpg => "JPEG",
        ImageFormat::Webp => "WebP",
    }
}

/// Runs `write` into `destination` so that a failure leaves nothing behind.
///
/// Without `replace` the destination is created new -- an error if something is there --
/// and removed if the write fails. With it, the write goes to a hidden file beside the
/// destination that is renamed over it at the end, which is what `gio::File::copy` with
/// `OVERWRITE` did for the same Save As before there was an encoder.
fn write_atomically(
    destination: &Path,
    replace: bool,
    write: impl FnOnce(&mut dyn Write) -> Result<(), EncodeError>,
) -> Result<(), EncodeError> {
    let target = if replace { part_beside(destination) } else { destination.to_path_buf() };
    let file = File::options().write(true).create_new(!replace).create(true).truncate(true).open(&target)?;
    let mut out = BufWriter::new(file);
    let written = write(&mut out)
        .and_then(|()| out.flush().map_err(EncodeError::from))
        .and_then(|()| if replace { std::fs::rename(&target, destination).map_err(EncodeError::from) } else { Ok(()) });
    if written.is_err() {
        // Best effort: the write's own error is the one worth reporting.
        let _ = std::fs::remove_file(&target);
    }
    written
}

/// `.<name>.part` in the destination's folder: hidden, and on the same filesystem, so the
/// rename that finishes the write is atomic.
fn part_beside(destination: &Path) -> PathBuf {
    let name = destination.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    destination.with_file_name(format!(".{name}.part"))
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
pub(crate) mod tests {
    use super::*;

    /// A `width` x `height` PNG at `path`: a gradient, with its left column see-through
    /// when `alpha` -- what a window capture's shadow is to an encoder.
    pub(crate) fn write_png(path: &Path, width: u32, height: u32, alpha: bool) {
        let rgba = picture(width, height, alpha);
        let mut file = File::create(path).expect("create the PNG");
        encode(&rgba, width, height, ImageFormat::Png, 0, &mut file).expect("encode the PNG");
    }

    fn picture(width: u32, height: u32, alpha: bool) -> Vec<u8> {
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let a = if alpha && x == 0 { 0 } else { 255 };
                rgba.extend_from_slice(&[(x * 7) as u8, (y * 5) as u8, 0x80, a]);
            }
        }
        rgba
    }

    /// The signature check the flow's tests make of every file they save: the bytes are
    /// what the name says.
    pub(crate) fn assert_is(path: &Path, format: ImageFormat) {
        let bytes = std::fs::read(path).expect("read the saved file");
        assert_eq!(ImageFormat::sniff(&bytes), Some(format), "{}", path.display());
        assert_eq!(ImageFormat::of_path(path), Some(format), "{}", path.display());
        match format {
            ImageFormat::Png => {}
            // Ends where a JPEG ends, so the encoder finished rather than stopped.
            ImageFormat::Jpg => assert_eq!(bytes[bytes.len() - 2..], [0xff, 0xd9]),
            // The RIFF size is the file's, less the eight bytes before it.
            ImageFormat::Webp => {
                let riff = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
                assert_eq!(riff as usize, bytes.len() - 8);
                assert_eq!(&bytes[12..16], b"VP8L", "lossless");
            }
        }
    }

    /// A JPEG's frame header: its width, its height and its number of components.
    fn jpeg_frame(bytes: &[u8]) -> (u16, u16, u8) {
        let mut at = 2;
        while at + 9 < bytes.len() {
            assert_eq!(bytes[at], 0xff, "a marker at {at}");
            let marker = bytes[at + 1];
            let length = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
            // SOF0 to SOF2: baseline, extended, progressive.
            if (0xc0..=0xc2).contains(&marker) {
                let be = |i: usize| u16::from_be_bytes([bytes[at + i], bytes[at + i + 1]]);
                return (be(7), be(5), bytes[at + 9]);
            }
            at += 2 + length;
        }
        panic!("no frame header");
    }

    #[test]
    fn a_png_becomes_each_format_and_says_so() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = dir.path().join("01SOURCE.png");
        write_png(&source, 40, 30, true);
        for format in [ImageFormat::Png, ImageFormat::Jpg, ImageFormat::Webp] {
            let destination = dir.path().join(format!("Shot.{}", format.extension()));
            transcode(&source, &destination, format, 90, false).expect("transcode");
            assert_is(&destination, format);
        }
        let jpeg = std::fs::read(dir.path().join("Shot.jpg")).expect("read");
        assert_eq!(jpeg_frame(&jpeg), (40, 30, 3), "a colour JPEG of the picture's size");
    }

    /// WebP is lossless here: what is read back is what went in, alpha and all.
    #[test]
    fn webp_keeps_every_pixel_and_the_alpha() {
        for alpha in [true, false] {
            let rgba = picture(33, 17, alpha);
            let mut bytes = Vec::new();
            encode(&rgba, 33, 17, ImageFormat::Webp, 0, &mut bytes).expect("encode");
            let mut decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(&bytes)).expect("a WebP");
            assert_eq!(decoder.dimensions(), (33, 17));
            assert!(decoder.has_alpha());
            let mut back = vec![0; decoder.output_buffer_size().expect("size")];
            decoder.read_image(&mut back).expect("decode");
            assert_eq!(back, rgba, "alpha {alpha}");
        }
    }

    #[test]
    fn the_quality_setting_reaches_the_jpeg() {
        let rgba = picture(64, 64, false);
        let size = |quality| {
            let mut bytes = Vec::new();
            encode(&rgba, 64, 64, ImageFormat::Jpg, quality, &mut bytes).expect("encode");
            bytes.len()
        };
        assert!(size(30) < size(90), "{} < {}", size(30), size(90));
        assert!(size(90) < size(100), "{} < {}", size(90), size(100));
    }

    #[test]
    fn see_through_flattens_onto_the_matte_and_opaque_stays() {
        let rgba = [10, 20, 30, 0, 10, 20, 30, 255, 0, 0, 0, 128];
        assert_eq!(flatten(&rgba, MATTE), vec![255, 255, 255, 10, 20, 30, 127, 127, 127]);
    }

    #[test]
    fn the_size_is_peeked_from_the_header_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let png = dir.path().join("a.png");
        write_png(&png, 1234, 56, false);
        assert_eq!(peek(&png), (Some(ImageFormat::Png), Some((1234, 56))));
        let short = dir.path().join("short.png");
        std::fs::write(&short, b"\x89PNG\r\n\x1a\n fake").expect("write");
        assert_eq!(peek(&short), (Some(ImageFormat::Png), None));
        assert_eq!(peek(&dir.path().join("none.png")), (None, None));
    }

    #[test]
    fn a_save_never_takes_a_name_that_was_filled_since() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = dir.path().join("01SOURCE.png");
        write_png(&source, 8, 8, false);
        let taken = dir.path().join("Shot.jpg");
        std::fs::write(&taken, b"someone else's").expect("write");
        assert!(transcode(&source, &taken, ImageFormat::Jpg, 90, false).is_err());
        assert_eq!(std::fs::read(&taken).expect("read"), b"someone else's");
        // A Save As was asked about it, and replaces it.
        transcode(&source, &taken, ImageFormat::Jpg, 90, true).expect("replace");
        assert_is(&taken, ImageFormat::Jpg);
    }

    /// A failed write leaves no part file and no half a picture -- and a failed
    /// replacement leaves the file it was replacing.
    #[test]
    fn a_failed_write_leaves_nothing_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = dir.path().join("not-a.png");
        std::fs::write(&source, b"\x89PNG\r\n\x1a\n fake").expect("write");
        let fresh = dir.path().join("Shot.webp");
        assert!(matches!(transcode(&source, &fresh, ImageFormat::Webp, 90, false), Err(EncodeError::Decode(_))));
        assert!(!fresh.exists());

        let kept = dir.path().join("Kept.jpg");
        std::fs::write(&kept, b"the old file").expect("write");
        let rgba = picture(4, 4, false);
        let refused = write_atomically(&kept, true, |out| {
            out.write_all(b"half")?;
            encode(&rgba, 4, 99, ImageFormat::Jpg, 90, out)
        });
        assert!(refused.is_err());
        assert_eq!(std::fs::read(&kept).expect("read"), b"the old file");
        let left: Vec<_> = std::fs::read_dir(dir.path()).expect("list").flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 2, "{left:?}");
    }
}
