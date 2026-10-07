// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/07` §4.1's `thumb_path`: a small PNG of a filed capture, made off the main thread.
//!
//! The strip shows a row of these; the card shows the capture itself. Decoding a 5K
//! capture to draw it in a tile would cost the frame that `spec/10` §7 budgets for the
//! whole strip, and would cost it again on every open, so the thumbnail is made once, when
//! the capture is filed, on a worker. A box filter, because a thumbnail's job is to be
//! recognisable and a box over exact source boxes has no ringing and no seam.
//!
//! It is the tile's own picture (D109): the part of the capture a tile shows
//! (`core::history::tile_crop`), at twice the tile's size. A thumbnail of the shape before
//! -- the whole capture inside 256 px -- is made again the first time the strip asks for
//! it, so an existing history catches up by being looked at.

use std::path::{Path, PathBuf};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use octosnap_core::history::{tile_crop, tile_thumb_size};
use tracing::{debug, warn};

/// A capture as something GTK can draw: a PNG as it is, a GIF as its first frame.
///
/// Not `gtk::Picture::for_filename` for a GIF. GTK reads PNG, JPEG and TIFF itself, and a
/// picture made from a GIF's *file* came up as a blank 200 x 200 placeholder in the nested
/// session of 2026-09-23 -- a recording's card showed its black background where the
/// recording should have been, while `gdk::Texture` read the same file correctly. The
/// frame is composed by `gif_edit`, which reads gifski's frames exactly (see below).
pub fn texture_of(path: &Path) -> Option<gdk::Texture> {
    if octosnap_core::history::is_gif(path) {
        let (rgba, width, height) = match first_gif_frame(path) {
            Ok(frame) => frame,
            Err(e) => {
                warn!("could not read the GIF's first frame: {e}");
                return None;
            }
        };
        let (Ok(w), Ok(h)) = (i32::try_from(width), i32::try_from(height)) else {
            return None;
        };
        let bytes = glib::Bytes::from_owned(rgba);
        let format = gdk::MemoryFormat::R8g8b8a8;
        return Some(gdk::MemoryTexture::new(w, h, format, &bytes, width * 4).upcast());
    }
    match gdk::Texture::from_filename(path) {
        Ok(texture) => Some(texture),
        Err(e) => {
            warn!(path = %path.display(), "could not load the capture: {e}");
            None
        }
    }
}

/// A picture of a capture, by [`texture_of`]: empty rather than absent when it cannot be
/// read, so the widget tree around it is the same either way.
pub fn picture_of(path: &Path) -> gtk::Picture {
    match texture_of(path) {
        Some(texture) => gtk::Picture::for_paintable(&texture),
        None => gtk::Picture::new(),
    }
}

/// Writes `target` from `source` on a worker thread and logs how it went.
///
/// The logging happens on the worker itself, not through a `spawn_future_local` back on the
/// main loop: the result is only a log line, so there is nothing to bring home, and a
/// fire-and-forget local future would need the caller's thread to own a `MainContext` --
/// which a libtest thread calling `History::file` does not, so it panicked there under
/// parallelism. `gio::spawn_blocking` runs the closure whether or not its handle is awaited.
pub fn spawn(source: PathBuf, target: PathBuf) {
    gio::spawn_blocking(move || {
        let started = std::time::Instant::now();
        match write(&source, &target) {
            Ok(()) => debug!(ms = started.elapsed().as_millis(), "history thumbnail written"),
            Err(e) => warn!("could not write a history thumbnail: {e}"),
        }
    });
}

/// Decodes the capture -- a PNG, or a GIF's first frame -- cuts out what a tile shows,
/// brings it down to the tile thumbnail's size and encodes it as a PNG.
fn write(source: &Path, target: &Path) -> Result<(), String> {
    let (rgba, width, height) = if octosnap_core::history::is_gif(source) {
        first_gif_frame(source)?
    } else {
        decode_png(source)?
    };
    let w = u32::try_from(width).unwrap_or(u32::MAX);
    let h = u32::try_from(height).unwrap_or(u32::MAX);
    let (x, y, crop_w, crop_h) = tile_crop(w, h);
    let cropped = crop(&rgba, width, (x as usize, y as usize), (crop_w as usize, crop_h as usize));
    let (thumb_w, thumb_h) = tile_thumb_size(w, h);
    let (thumb_w, thumb_h) = (thumb_w as usize, thumb_h as usize);
    let small = downscale(&cropped, crop_w as usize, crop_h as usize, thumb_w, thumb_h);
    encode(target, &small, thumb_w, thumb_h)
}

/// The strip's picture of a filed capture, read on a worker (D109).
///
/// The thumbnail when one of the tile's shape is on disk, which is nearly always; made
/// from the capture -- and written, so the next open finds it -- when it is missing or of
/// the shape before. `size` is the capture's own, which says what shape to expect.
/// `None` when neither file can be read, and the tile keeps its kind's placeholder.
pub async fn tile_texture(
    thumb: PathBuf,
    source: PathBuf,
    size: (u32, u32),
) -> Option<gdk::Texture> {
    let loaded = gio::spawn_blocking(move || {
        let started = std::time::Instant::now();
        if !is_tile_thumb(&thumb, size) {
            if let Err(e) = write(&source, &thumb) {
                warn!("could not make a tile thumbnail: {e}");
                return None;
            }
            debug!(ms = started.elapsed().as_millis(), "tile thumbnail made");
        }
        gdk::Texture::from_filename(&thumb)
            .inspect_err(|e| warn!(path = %thumb.display(), "could not read the thumbnail: {e}"))
            .ok()
    })
    .await;
    loaded.ok().flatten()
}

/// Whether `path` is a thumbnail of the tile's shape for a capture of `size`: its header
/// read, not its pixels. Within two pixels either way, because the capture's recorded size
/// and its file's can round differently, and a thumbnail that could never match would be
/// made again on every open.
fn is_tile_thumb(path: &Path, size: (u32, u32)) -> bool {
    let Ok(file) = std::fs::File::open(path) else { return false };
    let Ok(reader) = png::Decoder::new(std::io::BufReader::new(file)).read_info() else {
        return false;
    };
    let (want_w, want_h) = tile_thumb_size(size.0, size.1);
    let info = reader.info();
    info.width.abs_diff(want_w) <= 2 && info.height.abs_diff(want_h) <= 2
}

/// The `size` rectangle of an RGBA picture `width` pixels wide, from `at`.
fn crop(rgba: &[u8], width: usize, at: (usize, usize), size: (usize, usize)) -> Vec<u8> {
    let (x, y) = at;
    let (w, h) = size;
    if x == 0 && w == width && rgba.len() >= (y + h) * width * 4 {
        return rgba[y * width * 4..(y + h) * width * 4].to_vec();
    }
    let mut out = Vec::with_capacity(w * h * 4);
    for row in y..y + h {
        let start = (row * width + x) * 4;
        match rgba.get(start..start + w * 4) {
            Some(line) => out.extend_from_slice(line),
            None => out.resize(out.len() + w * 4, 0),
        }
    }
    out
}

/// A GIF's first frame composed, the picture its card shows (M5's recordings). Through
/// `gif_edit` rather than gdk-pixbuf, for the same reason the GIF editor is: gifski's
/// frames are differences with disposal, and only that reader composes them exactly. A
/// recording with a reel shows its reel's first frame, whether or not its GIF has been
/// written yet (D113).
fn first_gif_frame(source: &Path) -> Result<(Vec<u8>, usize, usize), String> {
    let source = octosnap_media::reel::source_of(source);
    let mut frames = octosnap_media::gif_edit::Frames::open(&source).map_err(|e| e.to_string())?;
    frames
        .next_frame()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{}: a GIF with no frames", source.display()))?;
    let (width, height) = (frames.width() as usize, frames.height() as usize);
    Ok((frames.bytes().to_vec(), width, height))
}

/// A PNG as straight 8-bit RGBA, whatever it was. A save in another format starts here
/// too (`encode`, D164).
pub(crate) fn decode_png(source: &Path) -> Result<(Vec<u8>, usize, usize), String> {
    let file = std::fs::File::open(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    // Every PNG becomes 8-bit samples of one to four channels, whatever it was.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| format!("{}: {e}", source.display()))?;
    let (width, height) = {
        let info = reader.info();
        (usize::try_from(info.width).unwrap_or(0), usize::try_from(info.height).unwrap_or(0))
    };
    if width == 0 || height == 0 {
        return Err(format!("{}: an empty image", source.display()));
    }
    // An upper bound -- four channels of eight bits -- rather than the decoder's own
    // number, so the buffer is right whatever the image turns out to be.
    let mut buffer = vec![0u8; width.saturating_mul(height).saturating_mul(4)];
    let frame = reader.next_frame(&mut buffer).map_err(|e| format!("{}: {e}", source.display()))?;
    let channels = match frame.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Grayscale => 1,
        png::ColorType::Indexed => return Err(format!("{}: still indexed after expansion", source.display())),
    };
    Ok((to_rgba(&buffer[..frame.buffer_size()], width, height, channels), width, height))
}

/// Straight RGBA from whatever channel count the decoder produced.
fn to_rgba(samples: &[u8], width: usize, height: usize, channels: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(width * height * 4);
    for pixel in samples.chunks_exact(channels).take(width * height) {
        match channels {
            4 => out.extend_from_slice(pixel),
            3 => {
                out.extend_from_slice(pixel);
                out.push(255);
            }
            2 => out.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]),
            _ => out.extend_from_slice(&[pixel[0], pixel[0], pixel[0], 255]),
        }
    }
    out
}

/// A box filter: every target pixel is the mean of the source box it covers.
fn downscale(rgba: &[u8], width: usize, height: usize, to_w: usize, to_h: usize) -> Vec<u8> {
    if (to_w, to_h) == (width, height) {
        return rgba.to_vec();
    }
    let mut out = Vec::with_capacity(to_w * to_h * 4);
    for ty in 0..to_h {
        let y0 = ty * height / to_h;
        let y1 = ((ty + 1) * height / to_h).max(y0 + 1).min(height);
        for tx in 0..to_w {
            let x0 = tx * width / to_w;
            let x1 = ((tx + 1) * width / to_w).max(x0 + 1).min(width);
            let mut sum = [0u64; 4];
            for y in y0..y1 {
                let row = y * width * 4;
                for x in x0..x1 {
                    let at = row + x * 4;
                    for (channel, total) in sum.iter_mut().enumerate() {
                        *total += u64::from(rgba[at + channel]);
                    }
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u64;
            for total in sum {
                out.push(u8::try_from((total + count / 2) / count).unwrap_or(255));
            }
        }
    }
    out
}

fn encode(target: &Path, rgba: &[u8], width: usize, height: usize) -> Result<(), String> {
    let file = std::fs::File::create(target).map_err(|e| format!("{}: {e}", target.display()))?;
    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(file),
        u32::try_from(width).unwrap_or(u32::MAX),
        u32::try_from(height).unwrap_or(u32::MAX),
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(|e| format!("{}: {e}", target.display()))?;
    writer.write_image_data(rgba).map_err(|e| format!("{}: {e}", target.display()))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_crop_takes_the_rectangle_it_names() {
        // A 3 x 2 picture whose pixels are numbered 0..6 in the red channel.
        let rgba: Vec<u8> = (0..6).flat_map(|n| [n, 0, 0, 255]).collect();
        let red = |ns: &[u8]| -> Vec<u8> { ns.iter().flat_map(|&n| [n, 0, 0, 255]).collect() };
        assert_eq!(crop(&rgba, 3, (1, 0), (2, 2)), red(&[1, 2, 4, 5]));
        assert_eq!(crop(&rgba, 3, (0, 1), (3, 1)), red(&[3, 4, 5]));
        // A rectangle past the edge is padded rather than panicking.
        assert_eq!(crop(&rgba, 3, (0, 2), (1, 1)), vec![0, 0, 0, 0]);
    }

    /// The picture a tile of a tall capture shows is its top, at the thumbnail's size.
    #[test]
    fn a_tall_capture_makes_a_tile_thumbnail_of_its_top() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (source, target) = (temp.path().join("tall.png"), temp.path().join("thumb.png"));
        // 800 x 2000: the top 500 rows white, the rest black.
        let rgba: Vec<u8> = (0..2000usize)
            .flat_map(|row| {
                let v = if row < 500 { 255 } else { 0 };
                std::iter::repeat_n([v, v, v, 255], 800).flatten()
            })
            .collect();
        encode(&source, &rgba, 800, 2000).expect("source written");
        write(&source, &target).expect("thumbnail written");
        let (thumb, w, h) = decode_png(&target).expect("thumbnail read");
        assert_eq!((w, h), (416, 260));
        assert!(is_tile_thumb(&target, (800, 2000)));
        assert!(!is_tile_thumb(&target, (300, 300)), "a thumbnail of another size is remade");
        // A thumbnail of the shape before -- a 16:10 screen fitted inside 256 px -- is too.
        let old = temp.path().join("old.png");
        encode(&old, &vec![0; 256 * 160 * 4], 256, 160).expect("old thumbnail written");
        assert!(!is_tile_thumb(&old, (1920, 1200)), "a thumbnail from before D109 is remade");
        // The crop is the top 500 rows: all white.
        let white = thumb.as_chunks::<4>().0.iter().all(|px| px[0] == 255);
        assert!(white, "the top of the page, not its middle");
    }

    #[test]
    fn a_box_filter_averages_the_source_box() {
        // 2 x 2 down to 1 x 1: the mean of four pixels, channel by channel.
        let rgba = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255];
        assert_eq!(downscale(&rgba, 2, 2, 1, 1), vec![128, 128, 128, 255]);
        // Same size is a copy.
        assert_eq!(downscale(&rgba, 2, 2, 2, 2), rgba.to_vec());
    }

    #[test]
    fn every_channel_count_becomes_straight_rgba() {
        assert_eq!(to_rgba(&[9, 8, 7, 6], 1, 1, 4), vec![9, 8, 7, 6]);
        assert_eq!(to_rgba(&[9, 8, 7], 1, 1, 3), vec![9, 8, 7, 255]);
        assert_eq!(to_rgba(&[9, 6], 1, 1, 2), vec![9, 9, 9, 6]);
        assert_eq!(to_rgba(&[9], 1, 1, 1), vec![9, 9, 9, 255]);
    }
}
