// SPDX-License-Identifier: GPL-3.0-or-later

//! One grabbed frame, as straight 8-bit RGBA.
//!
//! The stitcher works on pixels and nothing else -- no texture, no surface, no main loop --
//! because everything it does is arithmetic over rows and every rule in `spec/07` §1.2 is
//! testable on a synthetic page that never went near a compositor. The one concession to
//! the outside world is [`Frame::read`]: the extension hands frames over as *paths*
//! (`GrabFrame(handle) -> s path`), so something has to open them, and the alternative --
//! passing a decoder in -- would put the decode in every caller instead of once here.

use std::path::Path;

use crate::StitchError;

/// A frame's pixels, row-major, four bytes each.
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

/// Size rather than pixels: a `{:?}` of a 1920x1200 frame is nine megabytes of hex and
/// nobody has ever wanted to read one.
impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame").field("width", &self.width).field("height", &self.height).finish()
    }
}

impl Frame {
    /// A frame from pixels that are already RGBA.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, StitchError> {
        let wanted = usize::try_from(width)
            .ok()
            .and_then(|w| usize::try_from(height).ok().map(|h| (w, h)))
            .and_then(|(w, h)| w.checked_mul(h))
            .and_then(|n| n.checked_mul(4))
            .ok_or(StitchError::Empty)?;
        if width == 0 || height == 0 || pixels.len() != wanted {
            return Err(StitchError::Empty);
        }
        Ok(Self { width, height, pixels })
    }

    /// A frame of one colour, which is what a test page is made of.
    #[must_use]
    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let pixels = rgba.repeat((width as usize) * (height as usize));
        Self { width, height, pixels }
    }

    /// A PNG from disk, whatever channel count and depth it was written with.
    ///
    /// The same shape as the history's thumbnailer uses, for the same reason: the decoder
    /// normalises to eight-bit samples but not to four channels, so the channel count is
    /// still a question after `read_info` and the expansion happens here.
    pub fn read(path: &Path) -> Result<Self, StitchError> {
        let file = std::fs::File::open(path).map_err(|e| StitchError::Io(path.display().to_string(), e.to_string()))?;
        let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info().map_err(|e| StitchError::Io(path.display().to_string(), e.to_string()))?;
        let (width, height) = {
            let info = reader.info();
            (info.width, info.height)
        };
        if width == 0 || height == 0 {
            return Err(StitchError::Empty);
        }
        // Four channels of eight bits is the upper bound after the transformation, so one
        // buffer is right whatever the file turns out to hold.
        let mut buffer = vec![0u8; (width as usize) * (height as usize) * 4];
        let frame = reader
            .next_frame(&mut buffer)
            .map_err(|e| StitchError::Io(path.display().to_string(), e.to_string()))?;
        let channels = match frame.color_type {
            png::ColorType::Rgba => 4,
            png::ColorType::Rgb => 3,
            png::ColorType::GrayscaleAlpha => 2,
            png::ColorType::Grayscale => 1,
            png::ColorType::Indexed => {
                return Err(StitchError::Io(path.display().to_string(), "still indexed after expansion".into()));
            }
        };
        let pixels = to_rgba(&buffer[..frame.buffer_size()], (width as usize) * (height as usize), channels);
        Self::new(width, height, pixels)
    }

    /// The stitched result, as a PNG.
    pub fn write(&self, path: &Path) -> Result<(), StitchError> {
        let file = std::fs::File::create(path).map_err(|e| StitchError::Io(path.display().to_string(), e.to_string()))?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        // A scrolling capture is tall and the user is watching a progress strip, so the
        // fast filter beats the small file -- the same call the editor's export makes.
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder
            .write_header()
            .map_err(|e| StitchError::Io(path.display().to_string(), e.to_string()))?;
        writer
            .write_image_data(&self.pixels)
            .map_err(|e| StitchError::Io(path.display().to_string(), e.to_string()))
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// One row's bytes. Empty past the last row rather than a panic: the matcher indexes
    /// rows from offsets it computed, and a bug there should read nothing, not abort a
    /// capture the user has been scrolling through for a minute.
    #[must_use]
    pub fn row(&self, y: u32) -> &[u8] {
        let stride = (self.width as usize) * 4;
        let start = (y as usize) * stride;
        self.pixels.get(start..start + stride).unwrap_or(&[])
    }

    /// Rows `[from, to)` as their own frame.
    pub fn rows(&self, from: u32, to: u32) -> Result<Self, StitchError> {
        let (from, to) = (from.min(self.height), to.min(self.height));
        if to <= from {
            return Err(StitchError::Empty);
        }
        let stride = (self.width as usize) * 4;
        let pixels = self.pixels[(from as usize) * stride..(to as usize) * stride].to_vec();
        Self::new(self.width, to - from, pixels)
    }

    /// The frame with rows and columns swapped.
    ///
    /// `spec/07` §1.2 ends "horizontal mode is the transpose", and it means that
    /// literally: one algorithm, applied to a frame turned on its side and turned back at
    /// the end. Nine megabytes of copying per frame at 1920x1200, against a second
    /// implementation of signatures, fixed-row detection, cross-correlation and the
    /// append -- each of which would then need its own tests and could drift from the
    /// vertical one. The copy is the cheaper of the two by a distance.
    #[must_use]
    pub fn transpose(&self) -> Self {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut pixels = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let from = (y * w + x) * 4;
                let to = (x * h + y) * 4;
                pixels[to..to + 4].copy_from_slice(&self.pixels[from..from + 4]);
            }
        }
        Self { width: self.height, height: self.width, pixels }
    }

    /// The frame with its rows in the other order.
    ///
    /// The twin of [`Self::transpose`], and there for the same reason: a capture that
    /// grows *upwards* -- a user who starts at the bottom of a page -- is the same
    /// algorithm on a frame turned over, and the alternative is a second append that
    /// prepends and a second set of tests to go with it.
    #[must_use]
    pub fn flip(&self) -> Self {
        let stride = (self.width as usize) * 4;
        let mut pixels = Vec::with_capacity(self.pixels.len());
        for y in (0..self.height as usize).rev() {
            pixels.extend_from_slice(&self.pixels[y * stride..(y + 1) * stride]);
        }
        Self { width: self.width, height: self.height, pixels }
    }

    /// The frame shrunk to fit inside `width` x `height`, keeping its aspect.
    ///
    /// A box filter -- every output pixel is the mean of the input pixels it covers --
    /// because `spec/07` §1.1's preview strip is a very tall image in a very small box,
    /// where the shrink is 20x or more and point sampling would show one row in twenty
    /// and make a page of text look like a page of noise. Never upscales: a capture
    /// smaller than the box is shown at its own size rather than blurred up to fill it.
    #[must_use]
    pub fn thumbnail(&self, width: u32, height: u32) -> Self {
        let (width, height) = (width.max(1), height.max(1));
        let scale = (f64::from(width) / f64::from(self.width))
            .min(f64::from(height) / f64::from(self.height))
            .min(1.0);
        let out_w = ((f64::from(self.width) * scale).round() as u32).max(1);
        let out_h = ((f64::from(self.height) * scale).round() as u32).max(1);
        if out_w == self.width && out_h == self.height {
            return self.clone();
        }
        let mut pixels = Vec::with_capacity((out_w as usize) * (out_h as usize) * 4);
        for y in 0..out_h {
            let y0 = (u64::from(y) * u64::from(self.height) / u64::from(out_h)) as u32;
            let y1 = (((u64::from(y) + 1) * u64::from(self.height) / u64::from(out_h)) as u32).max(y0 + 1);
            for x in 0..out_w {
                let x0 = (u64::from(x) * u64::from(self.width) / u64::from(out_w)) as u32;
                let x1 = (((u64::from(x) + 1) * u64::from(self.width) / u64::from(out_w)) as u32).max(x0 + 1);
                let mut sums = [0u64; 4];
                let mut count = 0u64;
                for row in y0..y1.min(self.height) {
                    let line = self.row(row);
                    for column in x0..x1.min(self.width) {
                        let at = (column as usize) * 4;
                        for (channel, sum) in sums.iter_mut().enumerate() {
                            *sum += u64::from(line.get(at + channel).copied().unwrap_or(0));
                        }
                        count += 1;
                    }
                }
                let divisor = count.max(1);
                for sum in sums {
                    pixels.push(u8::try_from(sum / divisor).unwrap_or(255));
                }
            }
        }
        Self { width: out_w, height: out_h, pixels }
    }

    /// The newest `width` x `height` of the frame, at the scale that box implies.
    ///
    /// [P], and the reason is what [`Self::thumbnail`] does to a long capture: fitting
    /// 600 x 5 841 into 264 x 336 leaves a thread seventeen pixels wide, and by the time a
    /// scrolling capture is worth previewing it is always that long. So the strip is a
    /// *window* onto the canvas instead -- scaled to the box's width, showing as much of
    /// the growing end as the box's height holds. The scale then never changes while the
    /// capture runs, so the preview reads as content arriving rather than as a picture
    /// shrinking, which is what `spec/07` §1.1 item 3's "growing" describes.
    ///
    /// `direction` says which way it grows, and so which way is windowed and which end
    /// is shown: rows for a capture going down or up, columns for one going sideways, and
    /// the start for the two that grow backwards. Not the canvas's shape: a wide selection
    /// scrolled down starts wider than the box and stays so for its first few frames, and
    /// read by shape it was windowed across -- a preview of the right-hand columns of a
    /// page of flush-left type, which is blank (2026-09-23).
    #[must_use]
    pub fn strip(&self, width: u32, height: u32, direction: crate::Direction) -> Self {
        let (width, height) = (width.max(1), height.max(1));
        let sideways = direction.horizontal();
        let from_start = direction.backwards();
        let (along, box_along, across, box_across) = if sideways {
            (self.width, width, self.height, height)
        } else {
            (self.height, height, self.width, width)
        };
        let scale = (f64::from(box_across) / f64::from(across)).min(1.0);
        let keep = ((f64::from(box_along) / scale).round() as u32).clamp(1, along);
        let from = if from_start { 0 } else { along - keep };
        let window = if sideways {
            self.columns(from, from + keep)
        } else {
            self.rows(from, from + keep).unwrap_or_else(|_| self.clone())
        };
        window.thumbnail(width, height)
    }

    /// The frame narrowed to `from..to` of its columns.
    fn columns(&self, from: u32, to: u32) -> Self {
        let to = to.min(self.width);
        let from = from.min(to);
        let out_w = to - from;
        let stride = (self.width as usize) * 4;
        let mut pixels = Vec::with_capacity((out_w as usize) * (self.height as usize) * 4);
        for y in 0..self.height as usize {
            let start = y * stride + (from as usize) * 4;
            pixels.extend_from_slice(&self.pixels[start..start + (out_w as usize) * 4]);
        }
        Self { width: out_w, height: self.height, pixels }
    }

    /// Appends another frame's rows below these. The widths must agree.
    pub fn stack(&mut self, other: &Self) -> Result<(), StitchError> {
        if other.width != self.width {
            return Err(StitchError::Mismatch { width: other.width, height: other.height });
        }
        self.height += other.height;
        self.pixels.extend_from_slice(&other.pixels);
        Ok(())
    }
}

/// Straight RGBA from whatever channel count the decoder produced.
fn to_rgba(samples: &[u8], pixels: usize, channels: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels * 4);
    for pixel in samples.chunks_exact(channels).take(pixels) {
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

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::Direction;

    #[test]
    fn a_frame_refuses_pixels_that_are_not_its_size() {
        assert!(Frame::new(2, 2, vec![0; 16]).is_ok());
        assert!(Frame::new(2, 2, vec![0; 15]).is_err());
        assert!(Frame::new(0, 2, vec![]).is_err());
    }

    #[test]
    fn a_transpose_twice_is_the_frame_it_started_as() {
        let mut pixels = Vec::new();
        for i in 0..(3 * 5 * 4) {
            pixels.push(u8::try_from(i % 251).unwrap_or(0));
        }
        let frame = Frame::new(3, 5, pixels).expect("a 3x5 frame");
        let sideways = frame.transpose();
        assert_eq!((sideways.width(), sideways.height()), (5, 3));
        assert_eq!(sideways.transpose(), frame);
    }

    #[test]
    fn a_flip_twice_is_the_frame_it_started_as() {
        let frame = crate::testing::window(6, 9, 0, 1);
        assert_eq!(frame.flip().flip(), frame);
        assert_eq!(frame.flip().row(0), frame.row(8));
        assert_eq!(frame.flip().row(8), frame.row(0));
    }

    #[test]
    fn a_thumbnail_keeps_the_aspect_averages_and_never_grows() {
        let tall = Frame::filled(100, 400, [80, 120, 160, 255]);
        let thumb = tall.thumbnail(50, 50);
        // 400 is the long side, so the fit is 50/400 and the width follows.
        assert_eq!((thumb.width(), thumb.height()), (13, 50));
        // A flat frame averages to itself, whatever the box.
        assert_eq!(&thumb.row(0)[..4], &[80, 120, 160, 255]);
        // Smaller than the box: left alone rather than blurred up.
        let small = Frame::filled(10, 20, [1, 2, 3, 255]);
        assert_eq!(small.thumbnail(200, 200), small);
    }

    #[test]
    fn a_thumbnail_averages_rather_than_picking_one_row() {
        // Two rows, black and white: a box filter halves them to mid-grey, and point
        // sampling would answer with one of the two.
        let mut pixels = [0u8, 0, 0, 255].repeat(4);
        pixels.extend_from_slice(&[255u8, 255, 255, 255].repeat(4));
        let frame = Frame::new(4, 2, pixels).expect("a frame");
        let thumb = frame.thumbnail(4, 1);
        assert_eq!(&thumb.row(0)[..4], &[127, 127, 127, 255]);
    }

    #[test]
    fn rows_are_a_slice_and_stack_puts_them_back() {
        let frame = Frame::filled(4, 6, [9, 8, 7, 255]);
        let mut top = frame.rows(0, 2).expect("the first two rows");
        let rest = frame.rows(2, 6).expect("the rest");
        assert_eq!(top.height(), 2);
        top.stack(&rest).expect("same width");
        assert_eq!(top, frame);
    }

    /// A canvas built as a run of distinct row colours, so a crop can say which rows it is.
    fn banded(width: u32, rows: u32) -> Frame {
        let mut pixels = Vec::with_capacity((width as usize) * (rows as usize) * 4);
        for y in 0..rows {
            let shade = (y % 256) as u8;
            pixels.extend_from_slice(&[shade, shade, shade, 255].repeat(width as usize));
        }
        Frame::new(width, rows, pixels).expect("a banded canvas")
    }

    #[test]
    fn a_strip_keeps_the_scale_and_shows_the_growing_end() {
        // 600 wide in a 264-wide box is a scale of 0.44, so 336 of box holds 764 rows --
        // where a thumbnail of the whole canvas would have been 35 pixels wide.
        let canvas = banded(600, 5841);
        let strip = canvas.strip(264, 336, Direction::Down);
        assert_eq!((strip.width(), strip.height()), (264, 336));
        assert_eq!(canvas.thumbnail(264, 336).width(), 35);
        let tail = canvas.rows(5841 - 764, 5841).expect("the last 764 rows");
        assert_eq!(strip, tail.thumbnail(264, 336));
    }

    #[test]
    fn a_strip_of_a_backwards_capture_shows_the_top() {
        let canvas = banded(600, 5841);
        assert_eq!(canvas.strip(264, 336, Direction::Up).row(0)[0], 0);
        assert_ne!(canvas.strip(264, 336, Direction::Down).row(0)[0], 0);
    }

    /// The first frames of a wide selection scrolled down: wider than the box, and still
    /// windowed by rows -- every column of the page, scaled to the box's width.
    #[test]
    fn a_strip_of_a_wide_capture_going_down_keeps_every_column() {
        let mut pixels = Vec::with_capacity(870 * 640 * 4);
        for _ in 0..640 {
            for x in 0..870u32 {
                let shade = (x * 255 / 869) as u8;
                pixels.extend_from_slice(&[shade, shade, shade, 255]);
            }
        }
        let canvas = Frame::new(870, 640, pixels).expect("a wide canvas");
        let strip = canvas.strip(264, 336, Direction::Down);
        assert_eq!(strip.width(), 264, "the whole width, scaled to the box");
        assert!(strip.row(0)[0] < 8, "the left edge is in it");
        assert!(strip.row(0)[(263 * 4) as usize] > 247, "and the right edge");
    }

    #[test]
    fn a_strip_of_a_horizontal_capture_windows_the_columns() {
        let canvas = banded(600, 5841).transpose();
        assert_eq!((canvas.width(), canvas.height()), (5841, 600));
        let strip = canvas.strip(336, 264, Direction::Right);
        assert_eq!((strip.width(), strip.height()), (336, 264));
    }

    #[test]
    fn a_strip_of_a_canvas_smaller_than_its_box_is_the_whole_canvas() {
        let canvas = Frame::filled(40, 30, [1, 2, 3, 255]);
        assert_eq!(canvas.strip(264, 336, Direction::Down), canvas);
    }

    #[test]
    fn a_png_round_trips_through_the_disk() {
        let dir = std::env::temp_dir().join(format!("octosnap-stitch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let path = dir.join("frame.png");
        let frame = Frame::filled(7, 3, [12, 34, 56, 255]);
        frame.write(&path).expect("write");
        assert_eq!(Frame::read(&path).expect("read"), frame);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
