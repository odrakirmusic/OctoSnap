// SPDX-License-Identifier: GPL-3.0-or-later

//! Colour, stroke weight and the per-tool style enumerations (`spec/05` §2, §5.1).
//!
//! All of it is data the object carries, not state the tool holds. `spec/05` §2 records
//! that "the last colour and size per tool persist", which is a settings question; what
//! an object was drawn *with* has to travel in the object or a reopened project would
//! render differently from the session that made it.

use serde::{Deserialize, Serialize};

/// `spec/05` §2: "Size is not a slider... six diagonal stroke previews", bound to the
/// digits 1-6. **[P→V]** against `editor-popover-stroke-width.png`.
pub const SIZE_LEVELS: u8 = 6;

/// Stroke width in image pixels for each of the six levels.
///
/// Chosen rather than measured: `spec/05` §2 verifies that there are six and that they
/// run thin to thick, and the screenshot shows previews rather than numbers. The
/// progression is roughly geometric so the top of the range is useful on a 5K capture
/// while the bottom is still a hairline at 1x.
const STROKE_TABLE: [f64; SIZE_LEVELS as usize] = [1.0, 2.0, 3.0, 5.0, 8.0, 13.0];

/// The stroke width for a level, clamped rather than panicking on an index.
///
/// Levels are 1-based because the user types them: `spec/05` §9 binds "sizes 1-6" to the
/// digit keys, and a table indexed from 0 would make `3` mean the fourth weight.
#[must_use]
pub fn stroke_width(level: u8) -> f64 {
    let index = level.clamp(1, SIZE_LEVELS) - 1;
    STROKE_TABLE[index as usize]
}

/// `spec/05` §4.2: "Head length ≈ `3.2w`, half-width ≈ `1.6w` for shaft width `w` [M]".
///
/// Proportional, **plus a floor**. Proportional alone was the first version and it is
/// what the user saw as "arrows don't look right": at level 1 the shaft is one pixel and
/// `3.2w` is a three-pixel head, which is no head at all, and at level 3 -- the default --
/// it was nine pixels on a three-pixel shaft. Every head needs a few pixels that do not
/// scale with the stroke just to read as a triangle; the ratio takes over from there. On
/// the reference canvas (`editor-arrow-styles-on-canvas.png`) the thinnest arrow still
/// carries a head about a dozen pixels long.
pub const ARROWHEAD_PER_STROKE: f64 = 3.2;
pub const ARROWHEAD_FLOOR: f64 = 6.0;

/// The length of an arrowhead for a shaft of `width`, tip to base.
#[must_use]
pub fn arrowhead_length(width: f64) -> f64 {
    ARROWHEAD_PER_STROKE * width + ARROWHEAD_FLOOR
}

/// How far the widest head style reaches sideways from the shaft's line.
///
/// Here rather than in the renderer because it is part of the object's *bounds*, which the
/// marquee and the selection chrome read and which cannot see a render node. The Standard
/// head is half as wide as it is long (§4.2's `1.6w` against `3.2w`); Fancy's swept head
/// spreads a little further, and the bounds have to hold the wider one.
#[must_use]
pub fn arrowhead_reach(width: f64) -> f64 {
    arrowhead_length(width) * 0.6
}

/// Straight premultiplied-free RGBA in the 0..=1 range, as `spec/05` §5.1 writes it:
/// `"color": [1, 0.23, 0.19, 1]`.
///
/// `f32` and not `u8` per channel because the editor's colour picker has an alpha slider
/// in percent (`spec/05` §2) and because every consumer -- `gsk` nodes, the PNG encoder,
/// the hex field -- wants a different integer width. One float source of truth converts
/// to all of them without a rounding chain.
/// Serialised as a four-element array, because `spec/05` §5.1 writes it as one:
/// `"color": [1, 0.23, 0.19, 1]`. A struct with named channels would read better in the
/// file and would not be what the spec says, and the project format is a contract --
/// `spec/05` §8's `.octosnap` is meant to be reopened by later versions.
///
/// `f64` and not `f32`, which is what this was until its own round-trip test objected:
/// `0.23f32` widens to `0.23000000417232513`, so a file written from `f32` channels does
/// not contain the number the spec quotes, and `spec/05` §11 item 4 wants `objects.json`
/// **byte-identical** after fifty undos. The precision is free -- the colour arrives from
/// a picker with 0-255 integer fields and a 0-100 alpha -- and the one consumer that
/// wants `f32` is `gsk`, at the far end of the render path, where the narrowing is exact.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(from = "[f64; 4]", into = "[f64; 4]")]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl From<[f64; 4]> for Rgba {
    fn from([r, g, b, a]: [f64; 4]) -> Self {
        Self { r, g, b, a }
    }
}

impl From<Rgba> for [f64; 4] {
    fn from(c: Rgba) -> Self {
        [c.r, c.g, c.b, c.a]
    }
}

impl Rgba {
    #[must_use]
    pub const fn new(r: f64, g: f64, b: f64, a: f64) -> Self {
        Self { r, g, b, a }
    }

    /// From `RRGGBB` or `RRGGBBAA`, with or without a leading `#`.
    ///
    /// `spec/05` §2 specifies a "**Hex** field written without a leading `#`", and saved
    /// colours persist to a `gsettings` `as` of hex strings -- so both spellings arrive
    /// and neither is an error.
    #[must_use]
    pub fn from_hex(text: &str) -> Option<Self> {
        let hex = text.strip_prefix('#').unwrap_or(text);
        let byte = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
        let channel = |at: usize| byte(at).map(|v| f64::from(v) / 255.0);
        match hex.len() {
            6 => Some(Self::new(channel(0)?, channel(2)?, channel(4)?, 1.0)),
            8 => Some(Self::new(channel(0)?, channel(2)?, channel(4)?, channel(6)?)),
            _ => None,
        }
    }

    /// `RRGGBB`, or `RRGGBBAA` when the colour is not fully opaque.
    ///
    /// Asymmetric on purpose: the field in `spec/05` §2 is six characters wide for the
    /// colours a user types, and eight only where the alpha would otherwise be lost.
    #[must_use]
    pub fn to_hex(self) -> String {
        let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let (r, g, b, a) = (byte(self.r), byte(self.g), byte(self.b), byte(self.a));
        if a == 255 {
            format!("{r:02X}{g:02X}{b:02X}")
        } else {
            format!("{r:02X}{g:02X}{b:02X}{a:02X}")
        }
    }

    /// From `spec/05` §2's HSV square, hue slider and alpha slider.
    ///
    /// `hue` in degrees 0..360, `saturation` and `value` in 0..1. Wrapped rather than
    /// clamped, so a hue slider that runs past 360 comes back to red instead of sticking.
    #[must_use]
    pub fn from_hsv(hue: f64, saturation: f64, value: f64, alpha: f64) -> Self {
        let hue = hue.rem_euclid(360.0);
        let saturation = saturation.clamp(0.0, 1.0);
        let value = value.clamp(0.0, 1.0);
        let chroma = value * saturation;
        let sector = hue / 60.0;
        let x = chroma * (1.0 - ((sector % 2.0) - 1.0).abs());
        let (r, g, b) = match sector as u32 {
            0 => (chroma, x, 0.0),
            1 => (x, chroma, 0.0),
            2 => (0.0, chroma, x),
            3 => (0.0, x, chroma),
            4 => (x, 0.0, chroma),
            _ => (chroma, 0.0, x),
        };
        let base = value - chroma;
        Self::new(r + base, g + base, b + base, alpha.clamp(0.0, 1.0))
    }

    /// Hue in degrees, saturation and value in 0..1.
    ///
    /// **Hue is undefined for a grey**, and this answers 0 for one. A picker must not use
    /// that answer as the hue: dragging the value slider down to black and back up would
    /// come back red, whatever colour you started from. Keep the hue the user chose
    /// alongside the colour and only read this when the colour arrives from elsewhere.
    #[must_use]
    pub fn to_hsv(self) -> (f64, f64, f64) {
        let max = self.r.max(self.g).max(self.b);
        let min = self.r.min(self.g).min(self.b);
        let chroma = max - min;
        let hue = if chroma <= f64::EPSILON {
            0.0
        } else if max == self.r {
            60.0 * (((self.g - self.b) / chroma) % 6.0)
        } else if max == self.g {
            60.0 * ((self.b - self.r) / chroma + 2.0)
        } else {
            60.0 * ((self.r - self.g) / chroma + 4.0)
        };
        let saturation = if max <= f64::EPSILON { 0.0 } else { chroma / max };
        (hue.rem_euclid(360.0), saturation, max)
    }

    /// Black or white, whichever is legible on this colour.
    ///
    /// `spec/05` §4.5: box styles "pick the box colour from the text colour and
    /// **auto-contrast** the text (white on dark, black on light)". So the plate is the
    /// user's colour and the glyphs are this, which is what lets one colour choice produce
    /// a readable label whatever it is.
    ///
    /// Relative luminance rather than a mean of the channels: green at full strength is
    /// far brighter than blue at full strength, and averaging them puts the threshold in
    /// the wrong place for both. The 0.55 crossover is above the usual 0.5 because a
    /// mid-tone plate reads better with white on it than with black.
    #[must_use]
    pub fn contrasting(self) -> Self {
        let luminance = 0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b;
        if luminance > 0.55 {
            Self::new(0.0, 0.0, 0.0, 1.0)
        } else {
            Self::new(1.0, 1.0, 1.0, 1.0)
        }
    }

    /// The same colour at full opacity, for the far end of §2's alpha slider.
    #[must_use]
    pub const fn opaque(self) -> Self {
        Self { a: 1.0, ..self }
    }

    /// The eight-bit channels `spec/05` §2's **R G B** fields show.
    #[must_use]
    pub fn to_rgb8(self) -> (u8, u8, u8) {
        (to_u8(self.r), to_u8(self.g), to_u8(self.b))
    }

    /// From eight-bit channels, keeping the alpha.
    #[must_use]
    pub fn with_rgb8(self, r: u8, g: u8, b: u8) -> Self {
        Self::new(
            f64::from(r) / 255.0,
            f64::from(g) / 255.0,
            f64::from(b) / 255.0,
            self.a,
        )
    }

    #[must_use]
    pub const fn is_transparent(self) -> bool {
        self.a <= 0.0
    }
}

/// `spec/05` §2's ten-swatch palette, in the order the popover lists it.
///
/// **[P→V]** 2026-09-03: "the guessed ten-colour palette turned out to be exactly right",
/// verified against `editor-popover-color-palette.png`. Fixed, per §2 -- the user's own
/// colours go in `my-colors` and never displace these.
///
/// The hex values are OctoSnap's own. `spec/00` §6 forbids copying the reference app's
/// assets, and a palette is an asset; what was verified is the *set of hues* and their
/// order, so these are picked to match those hues at a consistent lightness.
/// `spec/05` §4.5's thirteen text size presets, in points.
///
/// A ladder over a continuous value, which §4.5 is explicit about: "Dragging a text
/// object's corner handle sets any value in between, which is why the stored preference is
/// a float such as 37.744. Implement the menu as presets over a continuous underlying
/// property, **not as an enum**."
pub const FONT_SIZES: [f64; 13] =
    [10.0, 13.0, 16.0, 20.0, 24.0, 30.0, 36.0, 48.0, 72.0, 96.0, 144.0, 216.0, 288.0];

/// `spec/05` §4.9's six counter sizes, in points.
pub const COUNTER_SIZES: [f64; 6] = [11.0, 14.0, 20.0, 24.0, 31.0, 45.0];

pub const PALETTE: [(&str, &str); 10] = [
    ("Black", "1B1B1F"),
    ("Red", "E01B24"),
    ("Orange", "FF7800"),
    ("Yellow", "F5C211"),
    ("Green", "2EC27E"),
    ("Turquoise", "1DC8CD"),
    ("Blue", "3584E4"),
    ("Purple", "9141AC"),
    ("Pink", "F06BA8"),
    ("White", "FFFFFF"),
];

/// `spec/05` §2's My Colors list after adding `hex`, newest first.
///
/// Pure, and here rather than in the widget, because the rules are easy to get subtly
/// wrong and impossible to see: a colour already saved must **move** to the front rather
/// than appear twice, the list is capped at `capacity`, and the cap must drop the oldest
/// rather than refuse the newest -- a picker that silently ignores "+ Add to My Colors"
/// once ten slots are full looks broken.
#[must_use]
pub fn remember_color(saved: &[String], hex: &str, capacity: usize) -> Vec<String> {
    let mut next = Vec::with_capacity(saved.len() + 1);
    next.push(hex.to_owned());
    for existing in saved {
        if !existing.eq_ignore_ascii_case(hex) {
            next.push(existing.clone());
        }
    }
    next.truncate(capacity);
    next
}

/// A channel as the eight-bit value a hex string or an R/G/B field shows.
///
/// Rounded rather than truncated: 0.5 is 128, not 127, and a round trip through the hex
/// field should not walk a colour downwards one step at a time.
fn to_u8(channel: f64) -> u8 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let scaled = (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
    scaled
}

/// What every drawn object carries (`spec/05` §5.1's common fields).
///
/// A struct here and **flattened** into the object on the way out, because §5.1 puts
/// `color`, `size` and `shadow` at the top level beside `id` and `z`. Grouping them in
/// Rust and flattening in the file gets both: one thing to pass to a tool, and a file
/// that reads as the spec writes it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub color: Rgba,
    /// 1-6, `spec/05` §2.
    pub size: u8,
    /// `spec/05` §2's per-tool shadow toggle, and §5.1's `"shadow": true`.
    pub shadow: bool,
}

impl Style {
    #[must_use]
    pub const fn new(color: Rgba, size: u8, shadow: bool) -> Self {
        Self { color, size, shadow }
    }

    #[must_use]
    pub fn stroke_width(&self) -> f64 {
        stroke_width(self.size)
    }
}

impl Default for Style {
    fn default() -> Self {
        // Red, weight 3, with a shadow: the first three swatches of `spec/05` §2's palette
        // are black, red, orange, and red is the one that reads on both a dark terminal
        // and a light document. Level 3 is the middle of the six.
        Self::new(Rgba::new(0.878, 0.106, 0.141, 1.0), 3, true)
    }
}

/// What the tip of an arrow looks like.
///
/// Not in `spec/05` -- CleanShot has one head shape -- and asked for by the user on
/// 2026-09-11: "give different options like 5 most common". These are the five every
/// diagramming tool offers, from draw.io's `classic`/`open`/`block`/`oval`/`diamond`
/// downwards, so the names will already mean something. Independent of [`ArrowStyle`]:
/// the style decides the shaft and how many ends carry a head, the head decides the tip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArrowHead {
    /// The filled triangle, `spec/05` §4.2's own.
    #[default]
    Triangle,
    /// Two strokes from the tip -- a chevron. The shaft runs to the tip.
    Open,
    /// The barbed head with a notched back that the reference's Fancy style wears.
    Swept,
    Diamond,
    /// A filled disc whose far edge is the tip.
    Dot,
}

impl ArrowHead {
    pub const ALL: [Self; 5] = [Self::Triangle, Self::Open, Self::Swept, Self::Diamond, Self::Dot];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Triangle => "Triangle",
            Self::Open => "Open",
            Self::Swept => "Swept",
            Self::Diamond => "Diamond",
            Self::Dot => "Dot",
        }
    }

    /// How far back from the tip the shaft may run before it shows past the head, as a
    /// share of the head's length. The renderer stops the shaft here.
    #[must_use]
    pub const fn shaft_depth(self) -> f64 {
        match self {
            // An open head has nothing to hide behind: the shaft *is* the apex.
            Self::Open => 0.0,
            // Solid from the notch to the tip; the shaft enters the notch.
            Self::Swept => 0.7,
            // The disc's centre; its far edge is the tip.
            Self::Dot => 0.32,
            Self::Triangle | Self::Diamond => 1.0,
        }
    }
}

/// `spec/05` §4.2's four arrow styles [V] `annotateArrowStyle*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArrowStyle {
    #[default]
    Standard,
    /// One control point, so the shaft bows (`spec/05` §5.1's `ctrl?`).
    Curved,
    DoubleHeaded,
    /// Tapered: the shaft narrows from head to tail.
    Fancy,
}

impl ArrowStyle {
    /// `spec/05` §4.2's menu order, which is **not** the declaration order: "The menu
    /// lists four rows … **Standard, Fancy, Curved, Double**, in that order."
    ///
    /// Kept apart deliberately. The declaration order is whatever the file grew into and
    /// changing it would change nothing; the menu order is verified against a screenshot
    /// and a widget that iterates the wrong one is wrong in a way only a person notices.
    pub const ALL: [Self; 4] = [Self::Standard, Self::Fancy, Self::Curved, Self::DoubleHeaded];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Fancy => "Fancy",
            Self::Curved => "Curved",
            Self::DoubleHeaded => "Double",
        }
    }

    /// A picture of the style, for its menu (`spec/09` §4b): "Fancy" and "Curved" are
    /// words for shapes, and a shape is quicker seen.
    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Standard => "arrow-standard-symbolic",
            Self::Fancy => "arrow-fancy-symbolic",
            Self::Curved => "arrow-curved-symbolic",
            Self::DoubleHeaded => "arrow-double-symbolic",
        }
    }
}

/// `spec/05` §4.5's seven text styles [V] `annotateTextStyle*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextStyle {
    #[default]
    Standard,
    Box,
    Monospaced,
    MonospacedBox,
    Outline,
    Rounded,
    RoundedBox,
}

impl TextStyle {
    /// `spec/05` §4.5's menu order: "**Standard, Rounded, Outlined, Mono, Box, Mono Box,
    /// Rounded Box**".
    pub const ALL: [Self; 7] = [
        Self::Standard,
        Self::Rounded,
        Self::Outline,
        Self::Monospaced,
        Self::Box,
        Self::MonospacedBox,
        Self::RoundedBox,
    ];

    /// The label the menu shows, which `spec/05` §4.5 warns is not the asset name: "the
    /// UI wording differs from the asset names: **'Outlined' not Outline, 'Mono' not
    /// Monospaced**". The variants keep the asset names; this is what a person reads.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Rounded => "Rounded",
            Self::Outline => "Outlined",
            Self::Monospaced => "Mono",
            Self::Box => "Box",
            Self::MonospacedBox => "Mono Box",
            Self::RoundedBox => "Rounded Box",
        }
    }

    /// Whether the style draws a filled plate behind the glyphs.
    ///
    /// Asked as a question rather than matched at each call site because three of the
    /// seven styles answer yes and the renderer, the hit test and the export all need the
    /// same answer -- a plate is part of the object's bounds.
    #[must_use]
    pub const fn has_plate(self) -> bool {
        matches!(self, Self::Box | Self::MonospacedBox | Self::RoundedBox)
    }

    #[must_use]
    pub const fn is_monospaced(self) -> bool {
        matches!(self, Self::Monospaced | Self::MonospacedBox)
    }

    /// Whether the glyphs are stroked rather than filled (`spec/05` §4.5's "Outlined").
    #[must_use]
    pub const fn is_outlined(self) -> bool {
        matches!(self, Self::Outline)
    }

    /// `spec/05` §4.5: "the box has **padding 0.4em**".
    ///
    /// In ems, so the plate scales with the text rather than with the zoom -- a 288 pt
    /// label and a 10 pt one both get a plate in proportion to their glyphs, which is what
    /// makes the style look like one style at every size.
    #[must_use]
    pub const fn plate_padding_em(self) -> f64 {
        if self.has_plate() { 0.4 } else { 0.0 }
    }

    /// `spec/05` §4.5: "radius `0.35em` (Rounded Box) or `0.15em` (Box)".
    ///
    /// Mono Box is not named there and takes Box's radius: it is the same plate with a
    /// different typeface, and the one style whose name says "Rounded" is the one that
    /// gets the rounder corner.
    #[must_use]
    pub const fn plate_radius_em(self) -> f64 {
        match self {
            Self::RoundedBox => 0.35,
            Self::Box | Self::MonospacedBox => 0.15,
            _ => 0.0,
        }
    }
}

/// `spec/05` §4.8's spotlight shapes [V] `annotateLastHighlightShape`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpotlightShape {
    #[default]
    Rectangle,
    Rounded,
    Ellipse,
}

impl SpotlightShape {
    /// `spec/05` §4.8: "Drag a rect, rounded rect or ellipse".
    pub const ALL: [Self; 3] = [Self::Rectangle, Self::Rounded, Self::Ellipse];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rectangle => "Rectangle",
            Self::Rounded => "Rounded rectangle",
            Self::Ellipse => "Ellipse",
        }
    }
}

/// `spec/05` §4.9's four **numbering systems**.
///
/// Not Filled/Outline, which is what the first version of this enum had. §4.9 corrects
/// exactly that reading: "**[P→V]** There is no Filled/Outline choice. Instead the menu
/// offers **four numbering systems** with the active one check-marked". The badge is
/// always filled -- "Click places a filled circular badge … drawn in the current colour
/// with a white glyph" -- so what looked like a rendering choice is a *formatting* one,
/// and the render node that branched on it was drawing a variant the UI would never offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CounterStyle {
    /// `1, 2, 3, 4 …`
    #[default]
    Arabic,
    /// `A, B, C, D …`
    UpperAlpha,
    /// `I, II, III, IV …`
    UpperRoman,
    /// `a, b, c, d …`
    LowerAlpha,
}

impl CounterStyle {
    pub const ALL: [Self; 4] = [Self::Arabic, Self::UpperAlpha, Self::UpperRoman, Self::LowerAlpha];

    /// The menu row's own label, which `spec/05` §4.9 writes as a sample of the sequence.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Arabic => "1, 2, 3, 4 …",
            Self::UpperAlpha => "A, B, C, D …",
            Self::UpperRoman => "I, II, III, IV …",
            Self::LowerAlpha => "a, b, c, d …",
        }
    }

    /// The glyph a badge carrying `number` shows.
    ///
    /// Falls back to the digits for anything the system cannot express. `spec/05` §2
    /// allows a starting number of 0 and nothing bounds it above, so `A` has to answer
    /// something for 0 and for 1 000 000 -- and a badge showing the number it was given is
    /// a better answer than a blank one or a panic.
    #[must_use]
    pub fn format(self, number: u32) -> String {
        match self {
            Self::Arabic => number.to_string(),
            Self::UpperAlpha => alpha(number, false).unwrap_or_else(|| number.to_string()),
            Self::LowerAlpha => alpha(number, true).unwrap_or_else(|| number.to_string()),
            Self::UpperRoman => roman(number).unwrap_or_else(|| number.to_string()),
        }
    }
}

/// Bijective base-26: 1 -> A, 26 -> Z, 27 -> AA. `None` for 0, which has no letter.
fn alpha(number: u32, lower: bool) -> Option<String> {
    if number == 0 {
        return None;
    }
    let base = if lower { b'a' } else { b'A' };
    let mut digits = Vec::new();
    let mut n = number;
    while n > 0 {
        let rem = (n - 1) % 26;
        digits.push(base + u8::try_from(rem).unwrap_or(0));
        n = (n - 1) / 26;
    }
    digits.reverse();
    String::from_utf8(digits).ok()
}

/// `None` outside 1..=3999, which is what the standard numerals cover.
fn roman(number: u32) -> Option<String> {
    if !(1..=3999).contains(&number) {
        return None;
    }
    const PAIRS: [(u32, &str); 13] = [
        (1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"),
        (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I"),
    ];
    let mut out = String::new();
    let mut n = number;
    for (value, glyph) in PAIRS {
        while n >= value {
            out.push_str(glyph);
            n -= value;
        }
    }
    Some(out)
}

/// `spec/05` §4.10's redaction styles [V] `annotatePixelateStyle*`.
///
/// `SecureBlur` is a separate variant from `Blur` and not an intensity of it, because
/// `spec/05` §11 item 6 makes them different *guarantees*: a smooth blur only has to look
/// blurred, a secure one must not be "reversible by upscaling". They cannot share a
/// rasterizer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RedactStyle {
    #[default]
    Pixelate,
    Blur,
    SecureBlur,
    BlackOut,
}

impl RedactStyle {
    /// `spec/05` §4.10's menu order and exact labels, which the spec asks to be careful
    /// about: "Note the exact labels and the order, which puts the irreversible option
    /// first". The default is `SecureBlur` -- §4.10 marks "the style selected by default"
    /// with a [V] -- so the first row and the default row are deliberately not the same.
    pub const ALL: [Self; 4] = [Self::Pixelate, Self::SecureBlur, Self::Blur, Self::BlackOut];

    /// The labels matter more than usual here. §4.10: "Calling the weak one 'smooth' and
    /// the strong one 'secure' tells the user what they are choosing without a warning
    /// dialog."
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pixelate => "Pixelate",
            Self::SecureBlur => "Blur (secure)",
            Self::Blur => "Blur (smooth)",
            Self::BlackOut => "Black Out",
        }
    }

    /// A picture of the style, for its menu. The secure blur's has a lock on it, which
    /// is the difference §4.10 wants the labels to make.
    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Pixelate => "tool-pixelate-symbolic",
            Self::SecureBlur => "redact-secure-blur-symbolic",
            Self::Blur => "redact-blur-symbolic",
            Self::BlackOut => "redact-blackout-symbolic",
        }
    }

    /// Whether the drawn preview is allowed to stand in for the exported result.
    ///
    /// `spec/05` §6 previews redactions on the GPU during a drag and rasterizes the exact
    /// region on gesture end. For a black rectangle those are the same thing; for
    /// anything that samples the image below it they are not, and the difference is the
    /// whole point of the second pass -- including the randomisation §11 item 6 asks to
    /// be able to see.
    #[must_use]
    pub const fn preview_is_exact(self) -> bool {
        matches!(self, Self::BlackOut)
    }
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn stroke_levels_are_one_based_and_clamped() {
        assert_eq!(stroke_width(1), STROKE_TABLE[0]);
        assert_eq!(stroke_width(6), STROKE_TABLE[5]);
        // The digits the user can type are 1-6; 0 and 9 are not errors worth a panic.
        assert_eq!(stroke_width(0), STROKE_TABLE[0]);
        assert_eq!(stroke_width(99), STROKE_TABLE[5]);
    }

    /// The floor is the point: a level-1 arrow used to get a three-pixel head.
    #[test]
    fn an_arrowhead_has_a_floor_and_then_grows_with_the_stroke() {
        assert!(arrowhead_length(stroke_width(1)) >= 9.0);
        assert!(arrowhead_length(stroke_width(6)) > arrowhead_length(stroke_width(1)));
        assert!(arrowhead_reach(3.0) > arrowhead_length(3.0) / 2.0);
    }

    #[test]
    fn stroke_levels_run_thin_to_thick() {
        for pair in STROKE_TABLE.windows(2) {
            assert!(pair[1] > pair[0], "the popover draws them thinnest at the top");
        }
    }

    /// Both spellings arrive: the picker's field has no `#`, saved colours may.
    #[test]
    fn hex_round_trips_both_ways() {
        let red = Rgba::from_hex("E01B24").expect("six digits");
        assert_eq!(red.to_hex(), "E01B24");
        assert_eq!(Rgba::from_hex("#E01B24"), Some(red));
        let half = Rgba::from_hex("E01B2480").expect("eight digits");
        assert_eq!(half.to_hex(), "E01B2480");
        assert!((half.a - 128.0 / 255.0).abs() < 1e-12);
    }

    #[test]
    fn a_hex_string_that_is_not_one_is_none_rather_than_black() {
        assert_eq!(Rgba::from_hex(""), None);
        assert_eq!(Rgba::from_hex("E01B2"), None);
        assert_eq!(Rgba::from_hex("gggggg"), None);
        // Length is checked before the digits, so a 6-character non-hex string still
        // fails rather than reading as zero.
        assert_eq!(Rgba::from_hex("#ZZZZZZ"), None);
    }

    #[test]
    fn opaque_colours_do_not_carry_an_alpha_pair() {
        assert_eq!(Rgba::new(0.0, 0.0, 0.0, 1.0).to_hex().len(), 6);
        assert_eq!(Rgba::new(0.0, 0.0, 0.0, 0.5).to_hex().len(), 8);
    }

    /// The palette is `spec/05` §2's verified order, and its length is what the popover's
    /// single column was measured to hold.
    #[test]
    fn the_palette_is_ten_named_colours_in_the_verified_order() {
        assert_eq!(PALETTE.len(), 10);
        assert_eq!(PALETTE[0].0, "Black");
        assert_eq!(PALETTE[9].0, "White");
        for (name, hex) in PALETTE {
            assert!(Rgba::from_hex(hex).is_some(), "{name} is not a hex colour");
        }
    }

    #[test]
    fn three_of_the_seven_text_styles_draw_a_plate() {
        let plated = [TextStyle::Box, TextStyle::MonospacedBox, TextStyle::RoundedBox];
        for style in plated {
            assert!(style.has_plate());
        }
        for style in [TextStyle::Standard, TextStyle::Monospaced, TextStyle::Outline,
                      TextStyle::Rounded] {
            assert!(!style.has_plate());
        }
    }

    /// `spec/05` §11 item 6 is why this is a question the style answers: only a black
    /// rectangle can be exported straight from its preview.
    #[test]
    fn only_black_out_needs_no_second_pass() {
        assert!(RedactStyle::BlackOut.preview_is_exact());
        for style in [RedactStyle::Pixelate, RedactStyle::Blur, RedactStyle::SecureBlur] {
            assert!(!style.preview_is_exact());
        }
    }

    #[test]
    fn styles_serialise_as_kebab_case_for_objects_json() {
        assert_eq!(serde_json::to_string(&ArrowStyle::DoubleHeaded).unwrap(), "\"double-headed\"");
        assert_eq!(serde_json::to_string(&TextStyle::MonospacedBox).unwrap(), "\"monospaced-box\"");
        assert_eq!(serde_json::to_string(&RedactStyle::SecureBlur).unwrap(), "\"secure-blur\"");
    }
}

#[cfg(test)]
mod counter_style_tests {
    use super::*;

    #[test]
    fn the_four_systems_are_the_ones_the_spec_verified() {
        // §4.9's menu, in its order, with its own labels.
        let labels: Vec<&str> = CounterStyle::ALL.iter().map(|s| s.label()).collect();
        assert_eq!(labels, ["1, 2, 3, 4 …", "A, B, C, D …", "I, II, III, IV …", "a, b, c, d …"]);
        assert_eq!(CounterStyle::default(), CounterStyle::Arabic);
    }

    #[test]
    fn letters_carry_past_z() {
        assert_eq!(CounterStyle::UpperAlpha.format(1), "A");
        assert_eq!(CounterStyle::UpperAlpha.format(26), "Z");
        assert_eq!(CounterStyle::UpperAlpha.format(27), "AA");
        assert_eq!(CounterStyle::UpperAlpha.format(52), "AZ");
        assert_eq!(CounterStyle::UpperAlpha.format(53), "BA");
        assert_eq!(CounterStyle::LowerAlpha.format(28), "ab");
    }

    #[test]
    fn roman_numerals_are_the_usual_ones() {
        for (n, expected) in [(1, "I"), (4, "IV"), (9, "IX"), (14, "XIV"), (40, "XL"), (1987, "MCMLXXXVII"), (3999, "MMMCMXCIX")] {
            assert_eq!(CounterStyle::UpperRoman.format(n), expected, "{n}");
        }
    }

    #[test]
    fn a_number_the_system_cannot_write_falls_back_to_digits() {
        // `spec/05` §2 allows a starting number of 0, and nothing bounds it above. A
        // blank badge would be worse than a numeric one.
        assert_eq!(CounterStyle::UpperAlpha.format(0), "0");
        assert_eq!(CounterStyle::UpperRoman.format(0), "0");
        assert_eq!(CounterStyle::UpperRoman.format(4000), "4000");
        assert_eq!(CounterStyle::Arabic.format(0), "0");
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "a failing assertion should panic")]
mod hsv_tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn the_six_corners_of_the_hue_wheel_round_trip() {
        for (hue, expected) in [
            (0.0, (1.0, 0.0, 0.0)),
            (60.0, (1.0, 1.0, 0.0)),
            (120.0, (0.0, 1.0, 0.0)),
            (180.0, (0.0, 1.0, 1.0)),
            (240.0, (0.0, 0.0, 1.0)),
            (300.0, (1.0, 0.0, 1.0)),
        ] {
            let color = Rgba::from_hsv(hue, 1.0, 1.0, 1.0);
            assert!(
                close(color.r, expected.0) && close(color.g, expected.1) && close(color.b, expected.2),
                "hue {hue} gave {color:?}, wanted {expected:?}"
            );
            let (back, saturation, value) = color.to_hsv();
            assert!(close(back, hue), "hue {hue} came back as {back}");
            assert!(close(saturation, 1.0) && close(value, 1.0));
        }
    }

    #[test]
    fn a_grey_has_no_hue_and_says_so() {
        // The trap this documents: a picker that re-reads the hue from the colour turns
        // every grey into red the moment the value slider passes through it.
        let (hue, saturation, value) = Rgba::new(0.5, 0.5, 0.5, 1.0).to_hsv();
        assert!(close(hue, 0.0), "an undefined hue should answer 0, got {hue}");
        assert!(close(saturation, 0.0), "a grey has no saturation");
        assert!(close(value, 0.5));
        // And black, where value is zero too.
        let (_, saturation, value) = Rgba::new(0.0, 0.0, 0.0, 1.0).to_hsv();
        assert!(close(saturation, 0.0) && close(value, 0.0));
    }

    #[test]
    fn the_hue_wraps_rather_than_sticking() {
        // A slider dragged past the end comes back to red instead of clamping there.
        assert_eq!(Rgba::from_hsv(360.0, 1.0, 1.0, 1.0), Rgba::from_hsv(0.0, 1.0, 1.0, 1.0));
        assert_eq!(Rgba::from_hsv(-60.0, 1.0, 1.0, 1.0), Rgba::from_hsv(300.0, 1.0, 1.0, 1.0));
    }

    #[test]
    fn alpha_survives_the_conversion() {
        let color = Rgba::from_hsv(200.0, 0.5, 0.8, 0.42);
        assert!(close(color.a, 0.42));
    }

    #[test]
    fn eight_bit_channels_round_trip_through_the_fields() {
        // `spec/05` §2's R G B fields are 0-255 and must not walk a colour downwards.
        for hex in ["1B1B1F", "E01B24", "FF7800", "3584E4", "FFFFFF", "000000"] {
            let color = Rgba::from_hex(hex).expect("a palette hex parses");
            let (r, g, b) = color.to_rgb8();
            let back = color.with_rgb8(r, g, b);
            assert_eq!(back.to_hex(), hex, "{hex} did not survive");
        }
        // 0.5 rounds up, not down.
        assert_eq!(Rgba::new(0.5, 0.5, 0.5, 1.0).to_rgb8(), (128, 128, 128));
    }

    #[test]
    fn every_palette_entry_survives_a_trip_through_hsv() {
        for (name, hex) in PALETTE {
            let color = Rgba::from_hex(hex).expect("a palette hex parses");
            let (hue, saturation, value) = color.to_hsv();
            let back = Rgba::from_hsv(hue, saturation, value, color.a);
            assert_eq!(back.to_hex(), hex, "{name} came back as {}", back.to_hex());
        }
    }
}

#[cfg(test)]
mod my_colors_tests {
    use super::*;

    #[test]
    fn the_newest_colour_goes_first() {
        let saved = vec!["AAAAAA".to_owned(), "BBBBBB".to_owned()];
        assert_eq!(remember_color(&saved, "CCCCCC", 10), ["CCCCCC", "AAAAAA", "BBBBBB"]);
    }

    #[test]
    fn a_colour_already_saved_moves_rather_than_duplicating() {
        let saved = vec!["AAAAAA".to_owned(), "BBBBBB".to_owned(), "CCCCCC".to_owned()];
        assert_eq!(remember_color(&saved, "CCCCCC", 10), ["CCCCCC", "AAAAAA", "BBBBBB"]);
        // Case is a hex-string detail, not a different colour.
        assert_eq!(remember_color(&saved, "bbbbbb", 10), ["bbbbbb", "AAAAAA", "CCCCCC"]);
    }

    #[test]
    fn a_full_list_drops_the_oldest_and_keeps_the_newest() {
        // The other way round -- refusing the new one -- makes the button look broken.
        let saved: Vec<String> = (0..10).map(|i| format!("00000{i}")).collect();
        let next = remember_color(&saved, "FFFFFF", 10);
        assert_eq!(next.len(), 10);
        assert_eq!(next[0], "FFFFFF");
        assert!(!next.contains(&"000009".to_owned()), "the oldest survived: {next:?}");
    }

    #[test]
    fn every_palette_colour_can_be_saved() {
        let mut saved = Vec::new();
        for (_, hex) in PALETTE {
            saved = remember_color(&saved, hex, 10);
        }
        assert_eq!(saved.len(), PALETTE.len().min(10));
        // Newest first means the list is the palette reversed.
        assert_eq!(saved[0], PALETTE[PALETTE.len() - 1].1);
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "a failing assertion should panic")]
mod text_style_tests {
    use super::*;

    #[test]
    fn auto_contrast_picks_the_legible_one() {
        // `spec/05` §4.5: "white on dark, black on light".
        for hex in ["1B1B1F", "E01B24", "3584E4", "9141AC"] {
            let plate = Rgba::from_hex(hex).expect("a palette hex parses");
            assert_eq!(plate.contrasting().to_hex(), "FFFFFF", "{hex} wanted white text");
        }
        for hex in ["FFFFFF", "F5C211", "2EC27E", "1DC8CD"] {
            let plate = Rgba::from_hex(hex).expect("a palette hex parses");
            assert_eq!(plate.contrasting().to_hex(), "000000", "{hex} wanted black text");
        }
    }

    #[test]
    fn luminance_and_not_a_mean_of_the_channels() {
        // Pure green and pure blue have the same channel mean and are nothing alike:
        // green needs black text, blue needs white. A mean would give both the same.
        let green = Rgba::new(0.0, 1.0, 0.0, 1.0);
        let blue = Rgba::new(0.0, 0.0, 1.0, 1.0);
        assert_eq!(green.contrasting().to_hex(), "000000");
        assert_eq!(blue.contrasting().to_hex(), "FFFFFF");
    }

    #[test]
    fn only_the_three_box_styles_have_a_plate() {
        let plated: Vec<&str> =
            TextStyle::ALL.into_iter().filter(|s| s.has_plate()).map(TextStyle::label).collect();
        assert_eq!(plated, ["Box", "Mono Box", "Rounded Box"]);
        for style in TextStyle::ALL {
            // Padding and radius are both zero exactly when there is no plate, so a
            // renderer can ask either question.
            assert_eq!(
                style.plate_padding_em() > 0.0,
                style.has_plate(),
                "{:?} disagrees about its plate",
                style
            );
        }
    }

    #[test]
    fn the_rounded_box_is_the_round_one() {
        // §4.5 gives 0.35em to Rounded Box and 0.15em to Box, and does not name Mono Box.
        assert!((TextStyle::RoundedBox.plate_radius_em() - 0.35).abs() < 1e-9);
        assert!((TextStyle::Box.plate_radius_em() - 0.15).abs() < 1e-9);
        assert!(
            (TextStyle::MonospacedBox.plate_radius_em() - TextStyle::Box.plate_radius_em()).abs()
                < 1e-9,
            "Mono Box is a Box with a different typeface"
        );
    }

    #[test]
    fn the_monospaced_and_outlined_styles_are_the_ones_named_so() {
        let mono: Vec<&str> = TextStyle::ALL
            .into_iter()
            .filter(|s| s.is_monospaced())
            .map(TextStyle::label)
            .collect();
        assert_eq!(mono, ["Mono", "Mono Box"]);
        let outlined: Vec<&str> = TextStyle::ALL
            .into_iter()
            .filter(|s| s.is_outlined())
            .map(TextStyle::label)
            .collect();
        assert_eq!(outlined, ["Outlined"]);
    }
}
