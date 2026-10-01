// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §4.10's redactions as pixel operations, and §11 item 6's two promises.
//!
//! Pure: straight RGBA8 in, straight RGBA8 out, no display and nobody else's allocation
//! policy. The canvas renders what lies beneath a redaction into a buffer, hands it here
//! on a worker thread -- `spec/05` §6: "rasterize the exact region on a worker thread …
//! into a small `MemoryTexture` … and swap it in" -- and draws the answer as a texture.
//! The export walks the same texture, which makes a redaction the one object whose
//! preview is *literally* its export rather than a second drawing of the same data.
//!
//! Everything here is in **physical pixels** of the buffer, with `scale` saying how many
//! of them make one document unit. A block that is 12 units wide is 12 pixels at 1x and 24
//! at 2x, so §11 item 2's "export at 2x matches the canvas at 100 %" holds for a pixelated
//! region the way it holds for a stroked one.
//!
//! §11 item 6 is the reason two of these are not the same operation with different
//! numbers. "Pixelate on the same region twice with different seeds produces different
//! blocks" needs the block *grid* to depend on the seed, not only the block contents; and
//! "secure blur is not reversible by upscaling" needs the fine structure of the source to
//! be gone before anything is blurred, because a blur is a linear filter and a linear
//! filter has an inverse. The tests at the bottom state both as measurements.

use crate::style::RedactStyle;

/// A straight-alpha RGBA8 buffer, rows top to bottom, no padding between rows.
#[derive(Clone, PartialEq, Eq)]
pub struct Pixels {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pixels({}x{}, {} bytes)", self.width, self.height, self.rgba.len())
    }
}

/// How much of a source buffer is margin around the region it was rendered for.
///
/// A blur's edge pixels are wrong unless the source reaches past the region, so the
/// canvas renders `margin` pixels more on each side where it can -- and fewer where the
/// canvas ends -- and says here how much to cut away again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Inset {
    pub left: usize,
    pub top: usize,
    pub right: usize,
    pub bottom: usize,
}

impl Pixels {
    /// A transparent buffer.
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, rgba: vec![0; width.saturating_mul(height).saturating_mul(4)] }
    }

    /// Wraps a buffer, or answers `None` when its length does not match its size.
    #[must_use]
    pub fn from_rgba(width: usize, height: usize, rgba: Vec<u8>) -> Option<Self> {
        (rgba.len() == width.checked_mul(height)?.checked_mul(4)?)
            .then_some(Self { width, height, rgba })
    }

    /// The pixel at `(x, y)`, or transparent black outside the buffer.
    #[must_use]
    pub fn at(&self, x: usize, y: usize) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0; 4];
        }
        let i = (y * self.width + x) * 4;
        self.rgba.get(i..i + 4).and_then(|p| p.try_into().ok()).unwrap_or([0; 4])
    }

    fn set(&mut self, x: usize, y: usize, px: [u8; 4]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let i = (y * self.width + x) * 4;
        if let Some(slot) = self.rgba.get_mut(i..i + 4) {
            slot.copy_from_slice(&px);
        }
    }

    /// The buffer with `inset` cut away on each side.
    #[must_use]
    pub fn crop(&self, inset: Inset) -> Self {
        let width = self.width.saturating_sub(inset.left + inset.right);
        let height = self.height.saturating_sub(inset.top + inset.bottom);
        let mut out = Self::new(width, height);
        for y in 0..height {
            let from = ((y + inset.top) * self.width + inset.left) * 4;
            let to = y * width * 4;
            if let (Some(src), Some(dst)) =
                (self.rgba.get(from..from + width * 4), out.rgba.get_mut(to..to + width * 4))
            {
                dst.copy_from_slice(src);
            }
        }
        out
    }

    /// Mean colour over a rectangle, clipped to the buffer.
    fn mean(&self, x0: usize, x1: usize, y0: usize, y1: usize) -> [u8; 4] {
        let (x1, y1) = (x1.min(self.width), y1.min(self.height));
        if x0 >= x1 || y0 >= y1 {
            return [0; 4];
        }
        let mut sum = [0u64; 4];
        for y in y0..y1 {
            let row = (y * self.width + x0) * 4;
            if let Some(px) = self.rgba.get(row..row + (x1 - x0) * 4) {
                for p in px.as_chunks::<4>().0 {
                    for (s, c) in sum.iter_mut().zip(p) {
                        *s += u64::from(*c);
                    }
                }
            }
        }
        let count = ((x1 - x0) * (y1 - y0)) as u64;
        sum.map(|s| u8::try_from((s + count / 2) / count).unwrap_or(u8::MAX))
    }

    fn fill(&mut self, x0: usize, x1: usize, y0: usize, y1: usize, px: [u8; 4]) {
        for y in y0..y1.min(self.height) {
            for x in x0..x1.min(self.width) {
                self.set(x, y, px);
            }
        }
    }
}

// --- §4.10's numbers -------------------------------------------------------------------

/// §4.10 writes its formulas for a "level" -- "block size `4 + 4×intensity` px", "radius
/// `3 × intensity` px" -- and `Control::RedactIntensity` is a slider from 0 to 1. Level 1
/// at the slider's left, 10 at its right, so a nudge on the slider is a visible step and
/// the middle of it is §4.10's "roughly 1/8" for the secure blur.
#[must_use]
pub fn level(intensity: f64) -> f64 {
    1.0 + 9.0 * intensity.clamp(0.0, 1.0)
}

/// §4.10: "block size `4 + 4×intensity` px", in physical pixels at `scale`.
#[must_use]
pub fn block_size(intensity: f64, scale: f64) -> f64 {
    (4.0 + 4.0 * level(intensity)) * scale.max(f64::EPSILON)
}

/// §4.10: "ordinary Gaussian, radius `3 × intensity` px", in physical pixels at `scale`.
#[must_use]
pub fn blur_radius(intensity: f64, scale: f64) -> f64 {
    3.0 * level(intensity) * scale.max(f64::EPSILON)
}

/// §4.10: "downscale to roughly 1/8". The cell one small pixel stands for, in physical
/// pixels: 3.2 units at the slider's left, 14 at its right, 8.6 in the middle.
#[must_use]
pub fn secure_cell(intensity: f64, scale: f64) -> f64 {
    (2.0 + 1.2 * level(intensity)) * scale.max(f64::EPSILON)
}

/// How far outside the region the source has to reach for the region's edge to be right.
///
/// Only the smooth blur samples its surroundings: three box passes standing in for a
/// Gaussian of `sigma = radius / 2` reach about one and a half radii. The others are
/// decided inside the region -- a block grid or a cell grid anchored on its corner -- and
/// deliberately so, because a redaction that read pixels *outside* what it covers would
/// be a redaction whose result depends on where the user happens to have drawn it.
#[must_use]
pub fn margin(style: RedactStyle, intensity: f64, scale: f64) -> usize {
    match style {
        RedactStyle::Blur => {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let reach = (blur_radius(intensity, scale) * 1.5).ceil().max(0.0) as usize;
            reach
        }
        RedactStyle::Pixelate | RedactStyle::SecureBlur | RedactStyle::BlackOut => 0,
    }
}

/// The redaction, applied.
///
/// `source` is the region plus `inset` of margin; the answer is exactly the region.
#[must_use]
pub fn rasterize(
    style: RedactStyle,
    intensity: f64,
    seed: u64,
    scale: f64,
    source: &Pixels,
    inset: Inset,
) -> Pixels {
    match style {
        RedactStyle::BlackOut => {
            let region = source.crop(inset);
            let mut out = Pixels::new(region.width, region.height);
            out.fill(0, region.width, 0, region.height, [0, 0, 0, 255]);
            out
        }
        RedactStyle::Pixelate => pixelate(&source.crop(inset), block_size(intensity, scale), seed),
        RedactStyle::SecureBlur => {
            secure_blur(&source.crop(inset), secure_cell(intensity, scale), seed)
        }
        RedactStyle::Blur => gaussian_blur(source, blur_radius(intensity, scale)).crop(inset),
    }
}

// --- Pixelate ----------------------------------------------------------------------------

/// §4.10: "a deterministic per-object noise of about ±6 %".
const JITTER: f64 = 0.06;

/// §4.10: "block size … with **randomization**, a deterministic per-object noise of about
/// ±6 % plus a random block phase, so de-pixelation attacks fail".
///
/// The phase shifts the whole grid by a random fraction of a block, and every column and
/// row boundary is then jittered on its own -- so two redactions of the same region with
/// different seeds average different sets of pixels, and a tool that assumes a regular
/// grid of `block` pixels has nothing to lock on to. Both draws come from the seed, so a
/// reopened project gets its blocks back exactly.
fn pixelate(source: &Pixels, block: f64, seed: u64) -> Pixels {
    let mut rng = SplitMix::new(seed);
    let columns = jittered_boundaries(source.width, block, &mut rng);
    let rows = jittered_boundaries(source.height, block, &mut rng);
    let mut out = source.clone();
    for pair in rows.windows(2) {
        let (y0, y1) = (pair[0], pair[1]);
        for pair in columns.windows(2) {
            let (x0, x1) = (pair[0], pair[1]);
            let mean = source.mean(x0, x1, y0, y1);
            out.fill(x0, x1, y0, y1, mean);
        }
    }
    out
}

/// Boundaries from `0` to `len` inclusive: a random phase, then widths jittered ±6 %.
fn jittered_boundaries(len: usize, block: f64, rng: &mut SplitMix) -> Vec<usize> {
    let block = block.max(1.0);
    let mut edges = vec![0];
    // The phase: the first block is cut short by a random fraction of a block.
    let mut x = -rng.unit() * block;
    #[allow(clippy::cast_precision_loss)]
    let end = len as f64;
    loop {
        x += block * (1.0 + (rng.unit() * 2.0 - 1.0) * JITTER);
        if x >= end - 0.5 {
            break;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let edge = x.round().max(0.0) as usize;
        if edges.last().is_none_or(|last| edge > *last) {
            edges.push(edge);
        }
    }
    if edges.last() != Some(&len) {
        edges.push(len);
    }
    edges
}

// --- Blur (secure) -------------------------------------------------------------------------

/// The noise added to each small pixel, per channel, in 8-bit levels.
const SECURE_NOISE: f32 = 10.0;

/// Premultiplied channels, 0..255, in the order the buffer keeps them.
type Px = [f32; 4];

/// §4.10: "downscale to roughly 1/8, add noise, blur, upscale. Irreversible".
///
/// Irreversible is the specification, and the order is what delivers it. The downscale
/// is an average over a cell, and averaging is where the information leaves: two sources
/// that differ only inside a cell produce the same small image, and nothing downstream
/// can tell them apart -- which is the test at the bottom, and the whole meaning of "not
/// reversible by upscaling". The noise then stops the small image being the *exact* mean
/// of anything, so even the cell averages are not readable back out; the blur softens
/// the cell grid; the bilinear upscale is the only step that is smooth to look at.
fn secure_blur(source: &Pixels, cell: f64, seed: u64) -> Pixels {
    let cell = cell.max(2.0);
    if source.width == 0 || source.height == 0 {
        return source.clone();
    }
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (cols, rows) =
        ((source.width as f64 / cell).ceil() as usize, (source.height as f64 / cell).ceil() as usize);
    let (cols, rows) = (cols.max(1), rows.max(1));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    let edge = |i: usize, len: usize| ((i as f64) * cell).round().min(len as f64) as usize;

    // Downscale: one small pixel per cell, premultiplied so a transparent corner of a
    // window capture averages as "nothing there" and not as black paint.
    let mut small = vec![[0.0f32; 4]; cols * rows];
    for row in 0..rows {
        let (y0, y1) = (edge(row, source.height), edge(row + 1, source.height));
        for col in 0..cols {
            let (x0, x1) = (edge(col, source.width), edge(col + 1, source.width));
            small[row * cols + col] = premultiplied_mean(source, x0, x1, y0, y1);
        }
    }

    // Noise, seeded, so the same object always gets the same noise.
    let mut rng = SplitMix::new(seed);
    for px in &mut small {
        for channel in px.iter_mut().take(3) {
            #[allow(clippy::cast_possible_truncation)]
            let jitter = (rng.unit() * 2.0 - 1.0) as f32 * SECURE_NOISE;
            *channel = (*channel + jitter).clamp(0.0, 255.0);
        }
    }

    // One 3x3 box over the small image, edges clamped.
    let mut blurred = small.clone();
    for row in 0..rows {
        for col in 0..cols {
            let mut sum = [0.0f32; 4];
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (r, c) = (clamp_index(row, dy, rows), clamp_index(col, dx, cols));
                    for (s, v) in sum.iter_mut().zip(small[r * cols + c]) {
                        *s += v;
                    }
                }
            }
            blurred[row * cols + col] = sum.map(|s| s / 9.0);
        }
    }

    // Bilinear upscale back to the region. The weights along each axis depend on that
    // axis alone, so they are worked out once per column and once per row rather than
    // once per pixel -- the difference between a budget met and one missed.
    let along = |len: usize, count: usize| -> Vec<(usize, usize, f32)> {
        (0..len)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let u = ((i as f64 + 0.5) / cell - 0.5).clamp(0.0, (count - 1) as f64);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let lo = u.floor() as usize;
                let hi = (lo + 1).min(count - 1);
                #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
                let frac = (u - lo as f64) as f32;
                (lo, hi, frac)
            })
            .collect()
    };
    let columns = along(source.width, cols);
    let rows_w = along(source.height, rows);
    let mut out = Pixels::new(source.width, source.height);
    for (y, &(r0, r1, fv)) in rows_w.iter().enumerate() {
        let top = &blurred[r0 * cols..(r0 + 1) * cols];
        let bottom = &blurred[r1 * cols..(r1 + 1) * cols];
        for (x, &(c0, c1, fu)) in columns.iter().enumerate() {
            let mut px = [0.0f32; 4];
            for i in 0..4 {
                let a = top[c0][i] * (1.0 - fu) + top[c1][i] * fu;
                let b = bottom[c0][i] * (1.0 - fu) + bottom[c1][i] * fu;
                px[i] = a * (1.0 - fv) + b * fv;
            }
            out.set(x, y, unpremultiply(px));
        }
    }
    out
}

fn clamp_index(at: usize, delta: i64, len: usize) -> usize {
    let moved = i64::try_from(at).unwrap_or(0) + delta;
    usize::try_from(moved.clamp(0, i64::try_from(len).unwrap_or(1) - 1)).unwrap_or(0)
}

/// Mean over a rectangle in premultiplied channels (0..255).
fn premultiplied_mean(source: &Pixels, x0: usize, x1: usize, y0: usize, y1: usize) -> Px {
    let (x1, y1) = (x1.min(source.width), y1.min(source.height));
    if x0 >= x1 || y0 >= y1 {
        return [0.0; 4];
    }
    let mut sum = [0.0f32; 4];
    for y in y0..y1 {
        let from = (y * source.width + x0) * 4;
        if let Some(row) = source.rgba.get(from..from + (x1 - x0) * 4) {
            for p in row.as_chunks::<4>().0 {
                for (s, v) in sum.iter_mut().zip(premultiply(*p)) {
                    *s += v;
                }
            }
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let count = ((x1 - x0) * (y1 - y0)) as f32;
    sum.map(|s| s / count)
}

fn premultiply(px: [u8; 4]) -> Px {
    let a = f32::from(px[3]);
    let k = a / 255.0;
    [f32::from(px[0]) * k, f32::from(px[1]) * k, f32::from(px[2]) * k, a]
}

fn unpremultiply(px: Px) -> [u8; 4] {
    let a = px[3].clamp(0.0, 255.0);
    let to_byte = |v: f32| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let b = v.round().clamp(0.0, 255.0) as u8;
        b
    };
    if a < 0.5 {
        return [0, 0, 0, 0];
    }
    let k = 255.0 / a;
    [to_byte(px[0] * k), to_byte(px[1] * k), to_byte(px[2] * k), to_byte(a)]
}

// --- Blur (smooth) ------------------------------------------------------------------------

/// §4.10: "ordinary Gaussian, radius `3 × intensity` px. Looks better, protects less".
///
/// Three box blurs of the widths Kutskir's construction gives for the wanted sigma, which
/// is within a level or two of a true Gaussian and runs in one pass per box regardless of
/// radius. Premultiplied, for the same reason as the secure blur: a blur across a
/// transparent corner must fade out, not darken. The source carries [`margin`] of
/// surroundings, so the edge pixels of the region see what a blur of the whole picture
/// would have seen there.
///
/// Both passes walk the buffer row by row -- the vertical one keeps a running sum per
/// column and advances a row at a time -- because a column-major walk over a 1000x600
/// buffer misses the cache on every step and was the difference between 55 ms and the
/// 50 ms budget.
fn gaussian_blur(source: &Pixels, radius: f64) -> Pixels {
    let sigma = radius / 2.0;
    if sigma < 0.5 || source.width == 0 || source.height == 0 {
        return source.clone();
    }
    let mut channels: Vec<Px> =
        source.rgba.as_chunks::<4>().0.iter().map(|p| premultiply(*p)).collect();
    let mut scratch = channels.clone();
    for width in boxes_for_gauss(sigma, 3) {
        let reach = width / 2;
        box_blur_rows(&channels, &mut scratch, source.width, source.height, reach);
        box_blur_columns(&scratch, &mut channels, source.width, source.height, reach);
    }
    let mut out = Pixels::new(source.width, source.height);
    for (slot, px) in out.rgba.as_chunks_mut::<4>().0.iter_mut().zip(channels) {
        *slot = unpremultiply(px);
    }
    out
}

/// Kutskir's "boxes for gauss": `n` odd box widths whose repeated application has the
/// wanted standard deviation.
fn boxes_for_gauss(sigma: f64, n: usize) -> Vec<usize> {
    #[allow(clippy::cast_precision_loss)]
    let nf = n as f64;
    let ideal = (12.0 * sigma * sigma / nf + 1.0).sqrt();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mut lower = ideal.floor() as usize;
    if lower.is_multiple_of(2) {
        lower = lower.saturating_sub(1);
    }
    let lower = lower.max(1);
    let upper = lower + 2;
    #[allow(clippy::cast_precision_loss)]
    let lf = lower as f64;
    let m = ((12.0 * sigma * sigma - nf * lf * lf - 4.0 * nf * lf - 3.0 * nf) / (-4.0 * lf - 4.0))
        .round();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let m = m.clamp(0.0, nf) as usize;
    (0..n).map(|i| if i < m { lower } else { upper }).collect()
}

fn add(sum: &mut Px, px: Px) {
    for (s, v) in sum.iter_mut().zip(px) {
        *s += v;
    }
}

fn shift(sum: &mut Px, entering: Px, leaving: Px) {
    for ((s, inc), out) in sum.iter_mut().zip(entering).zip(leaving) {
        *s += inc - out;
    }
}

/// One horizontal box pass with a running sum and clamped edges.
fn box_blur_rows(src: &[Px], dst: &mut [Px], width: usize, height: usize, reach: usize) {
    if reach == 0 {
        dst.copy_from_slice(src);
        return;
    }
    #[allow(clippy::cast_precision_loss)]
    let norm = 1.0 / (2 * reach + 1) as f32;
    for y in 0..height {
        let row = &src[y * width..(y + 1) * width];
        let out = &mut dst[y * width..(y + 1) * width];
        let clamped = |x: usize, offset: usize, back: bool| -> Px {
            let i = if back { x.saturating_sub(offset) } else { (x + offset).min(width - 1) };
            row[i]
        };
        let mut sum = [0.0f32; 4];
        for i in 0..=reach {
            add(&mut sum, clamped(0, i, true));
        }
        for i in 1..=reach {
            add(&mut sum, clamped(0, i, false));
        }
        for (x, slot) in out.iter_mut().enumerate() {
            *slot = sum.map(|s| s * norm);
            shift(&mut sum, clamped(x, reach + 1, false), clamped(x, reach, true));
        }
    }
}

/// One vertical box pass with a running sum per column, advancing a row at a time.
fn box_blur_columns(src: &[Px], dst: &mut [Px], width: usize, height: usize, reach: usize) {
    if reach == 0 {
        dst.copy_from_slice(src);
        return;
    }
    #[allow(clippy::cast_precision_loss)]
    let norm = 1.0 / (2 * reach + 1) as f32;
    let row = |y: usize| -> &[Px] {
        let y = y.min(height - 1);
        &src[y * width..(y + 1) * width]
    };
    let mut sums: Vec<Px> = vec![[0.0; 4]; width];
    // The window centred on row 0: `reach + 1` copies of the clamped top row, then the
    // `reach` rows below it.
    for _ in 0..=reach {
        for (sum, px) in sums.iter_mut().zip(row(0)) {
            add(sum, *px);
        }
    }
    for y in 1..=reach {
        for (sum, px) in sums.iter_mut().zip(row(y)) {
            add(sum, *px);
        }
    }
    for y in 0..height {
        let out = &mut dst[y * width..(y + 1) * width];
        for (slot, sum) in out.iter_mut().zip(&sums) {
            *slot = sum.map(|s| s * norm);
        }
        let entering = row(y + reach + 1);
        let leaving = row(y.saturating_sub(reach));
        for ((sum, inc), gone) in sums.iter_mut().zip(entering).zip(leaving) {
            shift(sum, *inc, *gone);
        }
    }
}

// --- the generator -----------------------------------------------------------------------

/// `SplitMix64`. Not cryptographic and not meant to be: it has to be *fixed* -- the same
/// seed on any machine, in any build, on any day gives the same blocks -- and `RandomState`
/// or anything reading the clock would break §5.1's promise that a reopened project
/// re-rasterises to the pixels it was saved with.
struct SplitMix(u64);

impl SplitMix {
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let mantissa = (self.next() >> 11) as f64;
        #[allow(clippy::cast_precision_loss)]
        let one = (1u64 << 53) as f64;
        mantissa / one
    }
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
mod tests {
    use super::*;

    /// A source with structure at every frequency: a diagonal gradient, thin stripes and
    /// a little deterministic noise -- a screenshot's worth of things to hide.
    fn busy(width: usize, height: usize) -> Pixels {
        let mut px = Pixels::new(width, height);
        let mut rng = SplitMix::new(7);
        for y in 0..height {
            for x in 0..width {
                let gradient = ((x + y) * 255 / (width + height)) as u8;
                let stripe = if x % 6 < 3 { 60 } else { 0 };
                let noise = (rng.unit() * 20.0) as u8;
                px.set(x, y, [
                    gradient.saturating_add(stripe),
                    gradient.saturating_add(noise),
                    255 - gradient,
                    255,
                ]);
            }
        }
        px
    }

    fn differing_pixels(a: &Pixels, b: &Pixels) -> usize {
        let (a, b) = (a.rgba.as_chunks::<4>().0, b.rgba.as_chunks::<4>().0);
        a.iter().zip(b).filter(|(p, q)| p != q).count()
    }

    fn mean_abs_diff(a: &Pixels, b: &Pixels) -> f64 {
        let total: u64 = a
            .rgba
            .iter()
            .zip(&b.rgba)
            .map(|(p, q)| u64::from(p.abs_diff(*q)))
            .sum();
        total as f64 / a.rgba.len() as f64
    }

    fn distinct_colours(px: &Pixels) -> usize {
        let mut seen: Vec<[u8; 4]> = px.rgba.as_chunks::<4>().0.to_vec();
        seen.sort_unstable();
        seen.dedup();
        seen.len()
    }

    #[test]
    fn the_slider_maps_onto_the_specs_levels() {
        assert!((level(0.0) - 1.0).abs() < 1e-9);
        assert!((level(1.0) - 10.0).abs() < 1e-9);
        // `4 + 4 x level`, and twice the pixels at 2x.
        assert!((block_size(0.0, 1.0) - 8.0).abs() < 1e-9);
        assert!((block_size(1.0, 2.0) - 88.0).abs() < 1e-9);
        assert!((blur_radius(1.0, 1.0) - 30.0).abs() < 1e-9);
        // Out-of-range values are clamped, not extrapolated.
        assert!((level(7.0) - 10.0).abs() < 1e-9);
        assert!((level(-3.0) - 1.0).abs() < 1e-9);
    }

    /// `spec/05` §11 item 6: "Pixelate on the same region twice with different seeds
    /// produces different blocks."
    #[test]
    fn two_seeds_pixelate_the_same_region_differently() {
        let source = busy(200, 120);
        let a = rasterize(RedactStyle::Pixelate, 0.5, 1, 1.0, &source, Inset::default());
        let b = rasterize(RedactStyle::Pixelate, 0.5, 2, 1.0, &source, Inset::default());
        let differing = differing_pixels(&a, &b);
        assert!(
            differing > 200 * 120 / 4,
            "only {differing} of {} pixels differ between two seeds",
            200 * 120
        );
    }

    /// `spec/05` §5.1 stores the seed "so a reopened project re-rasterises to the same
    /// pixels rather than to new ones".
    #[test]
    fn the_same_seed_is_reproducible() {
        let source = busy(90, 70);
        for style in [RedactStyle::Pixelate, RedactStyle::SecureBlur, RedactStyle::Blur] {
            let a = rasterize(style, 0.4, 99, 1.0, &source, Inset::default());
            let b = rasterize(style, 0.4, 99, 1.0, &source, Inset::default());
            assert_eq!(a, b, "{style:?} is not deterministic");
        }
    }

    #[test]
    fn pixelate_replaces_detail_with_flat_blocks() {
        let source = busy(160, 100);
        let out = rasterize(RedactStyle::Pixelate, 0.5, 5, 1.0, &source, Inset::default());
        let before = distinct_colours(&source);
        let after = distinct_colours(&out);
        // A 160x100 grid of ~26 px blocks is a few dozen blocks; the source has thousands
        // of colours.
        assert!(after < before / 20, "{after} colours after against {before} before");
        assert!(after > 4, "a pixelation with {after} colours is a fill, not blocks");
        // And the picture is still recognisably *that* picture: block means track the
        // gradient, so the overall difference is small even though every pixel moved.
        assert!(mean_abs_diff(&source, &out) < 40.0);
    }

    #[test]
    fn the_block_grid_covers_the_region_exactly_once() {
        let mut rng = SplitMix::new(3);
        for len in [1usize, 5, 17, 100, 1000] {
            let edges = jittered_boundaries(len, 12.0, &mut rng);
            assert_eq!(edges[0], 0);
            assert_eq!(*edges.last().unwrap(), len);
            assert!(edges.windows(2).all(|w| w[0] < w[1]), "not increasing: {edges:?}");
        }
    }

    /// `spec/05` §11 item 6: "secure blur is not reversible by upscaling".
    ///
    /// Stated as a measurement: two sources that differ only *inside* each cell -- the
    /// same means, different fine structure -- come out the same. Whatever an attacker
    /// upscales, they cannot tell which of the two they are looking at, so nothing about
    /// the fine structure survives. The smooth blur, run on the same pair, keeps them
    /// apart: that is the "protects less" in §4.10 and the reason the two are separate
    /// variants (D-record in the M3 notes).
    #[test]
    fn secure_blur_forgets_what_is_inside_a_cell() {
        let (w, h) = (128usize, 96usize);
        // Cell 8.6 px at the slider's middle; make the two patterns' period divide the
        // cells poorly on purpose, so the means agree only *statistically* -- as they
        // would for real text -- and the assertion still has to hold.
        let mut stripes = Pixels::new(w, h);
        let mut checks = Pixels::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let s = if x % 2 == 0 { 220 } else { 30 };
                let c = if (x + y) % 2 == 0 { 220 } else { 30 };
                stripes.set(x, y, [s, s, s, 255]);
                checks.set(x, y, [c, c, c, 255]);
            }
        }
        let secure_a = rasterize(RedactStyle::SecureBlur, 0.5, 11, 1.0, &stripes, Inset::default());
        let secure_b = rasterize(RedactStyle::SecureBlur, 0.5, 11, 1.0, &checks, Inset::default());
        let apart = mean_abs_diff(&secure_a, &secure_b);
        assert!(apart < 3.0, "the secure blurs of two fine patterns differ by {apart:.2}/255");
        // And it is not that the output is empty: it still carries the picture's tone.
        let tone = secure_a.at(w / 2, h / 2);
        assert!((100..=150).contains(&tone[0]), "the tone {tone:?} is not the patterns' mean");

        // The smooth blur at the same intensity keeps the two apart -- a small radius
        // leaves the stripes' phase in the result.
        let smooth_a = rasterize(RedactStyle::Blur, 0.0, 11, 1.0, &stripes, Inset::default());
        let smooth_b = rasterize(RedactStyle::Blur, 0.0, 11, 1.0, &checks, Inset::default());
        let smooth_apart = mean_abs_diff(&smooth_a, &smooth_b);
        assert!(
            smooth_apart > apart * 3.0,
            "smooth {smooth_apart:.2} should keep more than secure {apart:.2}"
        );
    }

    #[test]
    fn secure_blur_is_smooth_not_blocky() {
        let source = busy(120, 80);
        let out = rasterize(RedactStyle::SecureBlur, 0.5, 4, 1.0, &source, Inset::default());
        // Neighbouring pixels differ by little: the bilinear upscale ramps between cells.
        let mut worst = 0u8;
        for y in 0..80 {
            for x in 1..120 {
                worst = worst.max(out.at(x, y)[0].abs_diff(out.at(x - 1, y)[0]));
            }
        }
        assert!(worst < 24, "a step of {worst} between neighbours is a block edge");
    }

    #[test]
    fn smooth_blur_softens_an_edge_and_keeps_the_mean() {
        let (w, h) = (100usize, 40usize);
        let mut edge = Pixels::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = if x < w / 2 { 200 } else { 40 };
                edge.set(x, y, [v, v, v, 255]);
            }
        }
        let out = rasterize(RedactStyle::Blur, 0.5, 0, 1.0, &edge, Inset::default());
        let mid = out.at(w / 2, h / 2)[0];
        assert!((90..=150).contains(&mid), "the edge pixel {mid} is not between 40 and 200");
        assert!(out.at(2, h / 2)[0] > 190, "far from the edge the left side is unchanged");
        assert!(out.at(w - 3, h / 2)[0] < 50, "far from the edge the right side is unchanged");
        let red = |px: &Pixels| px.rgba.as_chunks::<4>().0.iter().map(|p| f64::from(p[0])).sum::<f64>();
        let (mean_before, mean_after) = (red(&edge), red(&out));
        assert!((mean_before - mean_after).abs() / mean_before < 0.02, "the blur changed the tone");
    }

    #[test]
    fn a_blur_across_transparency_fades_rather_than_darkens() {
        let (w, h) = (60usize, 20usize);
        let mut half = Pixels::new(w, h);
        for y in 0..h {
            for x in 0..w / 2 {
                half.set(x, y, [255, 255, 255, 255]);
            }
        }
        let out = rasterize(RedactStyle::Blur, 1.0, 0, 1.0, &half, Inset::default());
        let at_edge = out.at(w / 2 - 1, h / 2);
        // Alpha drops, colour stays white. Straight-alpha blurring would give grey here.
        assert!(at_edge[3] < 250 && at_edge[3] > 5, "alpha {} did not fade", at_edge[3]);
        assert!(at_edge[0] > 240, "the colour {at_edge:?} darkened across the transparent half");
    }

    #[test]
    fn black_out_is_black_and_opaque() {
        let source = busy(30, 20);
        let out = rasterize(RedactStyle::BlackOut, 0.3, 0, 1.0, &source, Inset::default());
        assert!(out.rgba.as_chunks::<4>().0.iter().all(|p| *p == [0, 0, 0, 255]));
    }

    #[test]
    fn the_margin_is_cut_away_and_only_the_blur_asks_for_one() {
        let source = busy(50, 40);
        let inset = Inset { left: 5, top: 4, right: 3, bottom: 2 };
        for style in RedactStyle::ALL {
            let out = rasterize(style, 0.5, 1, 1.0, &source, inset);
            assert_eq!((out.width, out.height), (42, 34), "{style:?}");
        }
        assert_eq!(margin(RedactStyle::Pixelate, 1.0, 2.0), 0);
        assert_eq!(margin(RedactStyle::SecureBlur, 1.0, 2.0), 0);
        assert_eq!(margin(RedactStyle::BlackOut, 1.0, 2.0), 0);
        assert!(margin(RedactStyle::Blur, 0.5, 1.0) > 0);
        assert_eq!(margin(RedactStyle::Blur, 0.5, 2.0), 2 * margin(RedactStyle::Blur, 0.5, 1.0));
    }

    /// The scale doubles the blocks in pixels, so the picture is the same at 2x. Checked
    /// on the grid rather than the pixels: the same seed at twice the scale puts every
    /// boundary at twice the position.
    #[test]
    fn the_block_grid_scales_with_the_export() {
        let mut one = SplitMix::new(5);
        let mut two = SplitMix::new(5);
        let at_1x = jittered_boundaries(100, block_size(0.5, 1.0), &mut one);
        let at_2x = jittered_boundaries(200, block_size(0.5, 2.0), &mut two);
        assert_eq!(at_1x.len(), at_2x.len(), "{at_1x:?} vs {at_2x:?}");
        for (a, b) in at_1x.iter().zip(&at_2x) {
            assert!((b.abs_diff(a * 2)) <= 1, "{at_1x:?} vs {at_2x:?}");
        }
    }

    #[test]
    fn crop_and_at_agree_about_where_pixels_are() {
        let source = busy(10, 8);
        let cropped = source.crop(Inset { left: 2, top: 1, right: 3, bottom: 2 });
        assert_eq!((cropped.width, cropped.height), (5, 5));
        assert_eq!(cropped.at(0, 0), source.at(2, 1));
        assert_eq!(cropped.at(4, 4), source.at(6, 5));
        assert_eq!(source.at(99, 99), [0; 4]);
        assert!(Pixels::from_rgba(3, 3, vec![0; 35]).is_none());
        assert!(Pixels::from_rgba(3, 3, vec![0; 36]).is_some());
    }

    #[test]
    fn empty_and_tiny_buffers_do_not_panic() {
        for (w, h) in [(0usize, 0usize), (1, 1), (0, 5), (5, 0), (2, 3)] {
            let source = Pixels::new(w, h);
            for style in RedactStyle::ALL {
                let out = rasterize(style, 1.0, 1, 1.0, &source, Inset::default());
                assert_eq!((out.width, out.height), (w, h));
            }
        }
    }

    /// `spec/05` §6's third budget: "redaction rasterization <= 50 ms for a 1000x600
    /// region". Asserted in a release build only -- a debug build measures the absence
    /// of optimisation -- and printed in both, so `cargo test -- --nocapture` shows the
    /// number.
    #[test]
    fn a_1000_by_600_region_rasterizes_within_the_budget() {
        let source = busy(1000 + 2 * 45, 600 + 2 * 45);
        let inset = Inset { left: 45, top: 45, right: 45, bottom: 45 };
        for style in [RedactStyle::Pixelate, RedactStyle::SecureBlur, RedactStyle::Blur] {
            let started = std::time::Instant::now();
            let out = rasterize(style, 1.0, 1, 2.0, &source, inset);
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            assert_eq!((out.width, out.height), (1000, 600));
            println!("{style:?}: 1000x600 in {ms:.1} ms (budget 50)");
            if !cfg!(debug_assertions) {
                assert!(ms <= 50.0, "{style:?} took {ms:.1} ms of a 50 ms budget");
            }
        }
    }
}
