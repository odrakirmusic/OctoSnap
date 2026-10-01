// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §4.13's background, as arithmetic: what the parameters mean, what canvas
//! they imply, and what auto-balance trims.
//!
//! The parameter names are appendix A.7's, read out of a real preset file -- `background`,
//! `padding`, `inset`, `cornerRadius`, `ratio`, `alignment`, `autoBalance`,
//! `shadowIntensity` -- because a preset the user saved has to survive being reopened and
//! there is no point inventing a second vocabulary for the same eight numbers.
//!
//! **Padding changes the canvas; inset changes the picture** (`docs/decisions.md` D87).
//! The panel has two spacing sliders and the difference between them is not obvious from
//! a screenshot: padding is the margin the canvas grows *around* the image, so more of it
//! makes a larger export at the image's own resolution; inset shrinks the image *inside*
//! the space that margin left, so more of it shows more background at the same export
//! size. Two sliders, two axes: how big the picture is printed, and how much of the
//! print is picture.
//!
//! Nothing here draws. [`layout`] answers with two rectangles -- the canvas and the
//! image's place in it -- and the canvas widget and the export walk the same answer,
//! which is what `spec/05` §6's "preview == export by construction" means for this tool.

use serde::{Deserialize, Serialize};

use crate::geometry::Bounds;
use crate::redact::Pixels;
use crate::style::Rgba;

/// How many gradients ship with the app. Appendix A.7's `bg1…bg20`, and `spec/05` §4.13's
/// "4 × 5 grid of twenty gradient swatches [V]".
pub const GRADIENTS: u8 = 20;

/// `spec/05` §4.13's defaults, from the preset JSON in appendix A.7.
pub const PADDING: f64 = 100.0;
pub const INSET: f64 = 0.0;
pub const CORNER_RADIUS: f64 = 0.08;
pub const SHADOW_INTENSITY: f64 = 28.0;
/// The 3 × 3 grid's centre.
pub const CENTRE: u8 = 4;

/// The widest a corner radius may be, as a fraction of the image's shorter side. Half is
/// a stadium; anything past it is the same shape with a different number in front of it.
pub const MAX_CORNER_RADIUS: f64 = 0.5;

/// The most the sliders offer. Padding in image pixels, the other two on their own scales.
pub const MAX_PADDING: f64 = 400.0;
pub const MAX_INSET: f64 = 400.0;
pub const MAX_SHADOW: f64 = 100.0;

/// How close to the corner colour a pixel has to be to count as border, in `spec/05`
/// §4.13's words: "colour within 2 % of the corner colour".
const UNIFORM: f64 = 0.02;

/// What is behind the image.
///
/// `spec/05` §4.13 groups the sources rather than pooling them -- gradients, wallpapers,
/// blurred, plain colour -- and so does this, because the panel's grid headings and this
/// enum have to agree about what a swatch *is*.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Background {
    /// §4.13's "wide **None** button … selected when there is none".
    #[default]
    None,
    /// One of the twenty shipped gradients, 1-based as appendix A.7 names them.
    Gradient { id: u8 },
    /// §4.13's "Plain color".
    Color { color: Rgba },
    /// The user's own image. A name inside the project's `assets/` when the project has
    /// been saved, exactly as an image object's `file` is: `spec/05` §8 makes a project
    /// self-contained, and a background that pointed into the user's pictures folder
    /// would break the first time the file moved.
    Image { file: String },
    /// §4.13's "Blurred": three swatches "that derive a background from the screenshot
    /// itself". The number is which of the three, so a reopened project blurs the same
    /// amount rather than whatever the current build's middle setting happens to be.
    Blurred { strength: Blur },
}

/// The three blurred swatches, weakest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Blur {
    Soft,
    #[default]
    Medium,
    Strong,
}

impl Blur {
    pub const ALL: [Self; 3] = [Self::Soft, Self::Medium, Self::Strong];

    /// The blur radius, as a fraction of the image's shorter side. Relative, so the same
    /// swatch looks the same on a 400 px crop and on a 5K capture.
    #[must_use]
    pub const fn radius(self) -> f64 {
        match self {
            Self::Soft => 0.03,
            Self::Medium => 0.06,
            Self::Strong => 0.12,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Soft => "Soft blur",
            Self::Medium => "Blur",
            Self::Strong => "Strong blur",
        }
    }
}

impl Background {
    /// Whether there is anything to draw behind the image.
    #[must_use]
    pub const fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// The asset name of a shipped gradient, `bg1`…`bg20`.
    #[must_use]
    pub fn asset(&self) -> Option<String> {
        match self {
            Self::Gradient { id } => Some(format!("bg{id}")),
            _ => None,
        }
    }
}

/// `spec/05` §4.13's "Ratio dropdown defaulting to **Auto**", as a shape rather than a
/// label: `None` is Auto, which keeps whatever the padded image already is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ratio {
    pub width: u32,
    pub height: u32,
}

impl Ratio {
    /// The menu, in the order §4.11's crop menu lists the same numbers -- one vocabulary
    /// for aspect in the editor, not two.
    pub const ALL: [Self; 6] = [
        Self { width: 1, height: 1 },
        Self { width: 4, height: 3 },
        Self { width: 3, height: 2 },
        Self { width: 16, height: 9 },
        Self { width: 16, height: 10 },
        Self { width: 9, height: 16 },
    ];

    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Width over height, or `None` for a ratio with a zero in it.
    #[must_use]
    pub fn value(self) -> Option<f64> {
        (self.width > 0 && self.height > 0)
            .then(|| f64::from(self.width) / f64::from(self.height))
    }

    #[must_use]
    pub fn label(self) -> String {
        format!("{}:{}", self.width, self.height)
    }
}

/// `spec/05` §4.13's whole parameter set, and appendix A.7's field names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundParams {
    /// What is behind the image.
    #[serde(default)]
    pub background: Background,
    /// The margin the canvas grows around the image, in image pixels.
    pub padding: f64,
    /// How much the image is shrunk inside that margin, in image pixels a side.
    pub inset: f64,
    /// A fraction of the image's shorter side, 0 to 0.5. Relative, so the same preset
    /// rounds a thumbnail and a 5K capture by the same visual amount (appendix A.7).
    pub corner_radius: f64,
    /// `null` is §4.13's **Auto**.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio: Option<Ratio>,
    /// A 3 × 3 index, 0 top-left to 8 bottom-right, 4 centre.
    pub alignment: u8,
    /// §4.13's checkbox beside Inset: trim the image's uniform border and centre what is
    /// left of it.
    pub auto_balance: bool,
    /// 0 to 100.
    pub shadow_intensity: f64,
}

impl Default for BackgroundParams {
    /// Appendix A.7's preset, field for field.
    fn default() -> Self {
        Self {
            background: Background::None,
            padding: PADDING,
            inset: INSET,
            corner_radius: CORNER_RADIUS,
            ratio: None,
            alignment: CENTRE,
            auto_balance: false,
            shadow_intensity: SHADOW_INTENSITY,
        }
    }
}

impl BackgroundParams {
    /// A flat fill and nothing else: no padding, no rounding, no shadow.
    ///
    /// `spec/05` §11 item 7's "crop beyond the image expands the canvas with the detected
    /// colour" makes one of these, and it must not carry [`Self::default`]'s hundred
    /// pixels of padding with it -- a crop that silently added a margin would be the
    /// opposite of what dragging a crop handle asks for.
    #[must_use]
    pub fn plain(color: Rgba) -> Self {
        Self {
            background: Background::Color { color },
            padding: 0.0,
            inset: 0.0,
            corner_radius: 0.0,
            ratio: None,
            alignment: CENTRE,
            auto_balance: false,
            shadow_intensity: 0.0,
        }
    }

    /// Whether this is one of [`Self::plain`]'s, or nothing at all.
    ///
    /// What the crop asks before it recolours a background it did not make: a gradient
    /// the user chose already covers the expanded canvas, and replacing it with the
    /// median of the border pixels would be an edit nobody asked for.
    #[must_use]
    pub const fn is_plain(&self) -> bool {
        matches!(self.background, Background::None | Background::Color { .. })
            && self.padding <= 0.0
            && self.inset <= 0.0
    }

    /// The same parameters with a background chosen, which is what a swatch click does.
    #[must_use]
    pub fn with(&self, background: Background) -> Self {
        Self { background, ..self.clone() }
    }

    /// Every value inside the range its slider offers.
    ///
    /// `spec/05` §4.13 says "values can be typed manually [D 4.6]", and a typed value is
    /// the one that arrives out of range -- as is a preset written by a future build with
    /// wider sliders. Clamped rather than rejected: a preset that opened to an error
    /// message would be worse than one that opened to its nearest legal shape.
    #[must_use]
    pub fn clamped(&self) -> Self {
        Self {
            background: self.background.clone(),
            padding: clamp(self.padding, 0.0, MAX_PADDING),
            inset: clamp(self.inset, 0.0, MAX_INSET),
            corner_radius: clamp(self.corner_radius, 0.0, MAX_CORNER_RADIUS),
            ratio: self.ratio.filter(|r| r.value().is_some()),
            alignment: self.alignment.min(8),
            auto_balance: self.auto_balance,
            shadow_intensity: clamp(self.shadow_intensity, 0.0, MAX_SHADOW),
        }
    }

    /// Whether these parameters would change anything at all.
    ///
    /// A `None` background with no padding is not a background object, and the editor
    /// removes rather than keeps one: `spec/05` §5.3 draws the object beneath the base
    /// image, and an object that draws nothing is a row in `objects.json` that the undo
    /// history has to carry for no reason.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.background.is_none() && self.padding <= 0.0 && self.inset <= 0.0
    }

    /// The horizontal and vertical thirds the alignment index names, each 0.0, 0.5 or 1.0.
    #[must_use]
    pub fn anchor(&self) -> (f64, f64) {
        let index = self.alignment.min(8);
        let third = |n: u8| f64::from(n) / 2.0;
        (third(index % 3), third(index / 3))
    }
}

/// One of the twenty shipped backgrounds, as the stops that draw it.
///
/// **Data, not a bitmap** (`docs/decisions.md` D88). `spec/05` §4.13 asks for twenty
/// *original* backgrounds, "gradients/mesh/blur-style", and the shortest road to
/// originality is to define them rather than to draw them somewhere and check in the
/// pixels: a gradient written as its stops cannot have come from anywhere else, weighs a
/// few hundred bytes for all twenty, is sharp at a 44 px swatch and at a 5K export alike,
/// and makes the swatch and the canvas draw literally the same thing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gradient {
    /// 1-based, matching appendix A.7's `bg1`…`bg20`.
    pub id: u8,
    pub name: &'static str,
    /// Degrees clockwise from straight up, the way a CSS `linear-gradient` angle reads.
    pub angle: f64,
    /// Offset along that axis, and the colour there, as `0xRRGGBB`.
    pub stops: &'static [(f64, u32)],
    /// Soft radial washes over the base. This is what makes a mesh gradient a mesh: the
    /// base carries the two-colour sweep and the blobs put the light somewhere.
    pub blobs: &'static [Blob],
}

/// A soft radial wash, in fractions of the canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blob {
    pub x: f64,
    pub y: f64,
    /// As a fraction of the canvas's longer side.
    pub radius: f64,
    pub color: u32,
    /// How strong it is at its centre; it fades to nothing at `radius`.
    pub alpha: f64,
}

const fn blob(x: f64, y: f64, radius: f64, color: u32, alpha: f64) -> Blob {
    Blob { x, y, radius, color, alpha }
}

/// The twenty, in the order the 4 × 5 grid shows them.
///
/// Four rows of five, and the rows are a progression rather than a shuffle: warm, cool,
/// deep, then neutral. A grid a person scans for "something blue" is a grid where the
/// blues are together.
pub const GRADIENT_SET: [Gradient; GRADIENTS as usize] = [
    // --- warm -----------------------------------------------------------------------
    Gradient {
        id: 1,
        name: "Sunrise",
        angle: 160.0,
        stops: &[(0.0, 0xFF_9A_5A), (1.0, 0xFF_4D_6E)],
        blobs: &[blob(0.18, 0.16, 0.7, 0xFF_D6_8A, 0.55)],
    },
    Gradient {
        id: 2,
        name: "Ember",
        angle: 145.0,
        stops: &[(0.0, 0xF4_5B_2A), (0.55, 0xC9_2A_4E), (1.0, 0x6B_16_4B)],
        blobs: &[blob(0.8, 0.22, 0.6, 0xFF_B0_5E, 0.35)],
    },
    Gradient {
        id: 3,
        name: "Peach",
        angle: 200.0,
        stops: &[(0.0, 0xFF_C7_A1), (1.0, 0xF2_8A_8A)],
        blobs: &[blob(0.3, 0.8, 0.75, 0xFF_E8_C8, 0.45)],
    },
    Gradient {
        id: 4,
        name: "Marmalade",
        angle: 135.0,
        stops: &[(0.0, 0xFF_B3_3C), (1.0, 0xE8_52_2B)],
        blobs: &[blob(0.75, 0.75, 0.65, 0xFF_E0_7A, 0.4)],
    },
    Gradient {
        id: 5,
        name: "Blossom",
        angle: 170.0,
        stops: &[(0.0, 0xFF_B8_D9), (1.0, 0xB0_6A_C9)],
        blobs: &[blob(0.15, 0.85, 0.7, 0xFF_E3_F0, 0.5)],
    },
    // --- cool -----------------------------------------------------------------------
    Gradient {
        id: 6,
        name: "Lagoon",
        angle: 155.0,
        stops: &[(0.0, 0x3E_C9_D6), (1.0, 0x2A_6D_C9)],
        blobs: &[blob(0.82, 0.2, 0.62, 0x9B_F2_E4, 0.4)],
    },
    Gradient {
        id: 7,
        name: "Meadow",
        angle: 150.0,
        stops: &[(0.0, 0x8A_D6_6A), (1.0, 0x1E_9E_87)],
        blobs: &[blob(0.2, 0.25, 0.7, 0xD8_F5_A6, 0.45)],
    },
    Gradient {
        id: 8,
        name: "Sky",
        angle: 180.0,
        stops: &[(0.0, 0xA8_D8_FF), (1.0, 0x4E_7B_E2)],
        blobs: &[blob(0.5, 0.1, 0.8, 0xFF_FF_FF, 0.35)],
    },
    Gradient {
        id: 9,
        name: "Iris",
        angle: 140.0,
        stops: &[(0.0, 0x7B_6B_E8), (1.0, 0x3B_2E_8C)],
        blobs: &[blob(0.78, 0.78, 0.7, 0xB5_A6_FF, 0.4)],
    },
    Gradient {
        id: 10,
        name: "Mint",
        angle: 190.0,
        stops: &[(0.0, 0xD3_F5_E4), (1.0, 0x6F_C2_B0)],
        blobs: &[blob(0.25, 0.7, 0.7, 0xFF_FF_FF, 0.4)],
    },
    // --- deep -----------------------------------------------------------------------
    Gradient {
        id: 11,
        name: "Midnight",
        angle: 160.0,
        stops: &[(0.0, 0x1B_23_40), (1.0, 0x0A_0D_1A)],
        blobs: &[blob(0.2, 0.15, 0.7, 0x3D_5A_A8, 0.45)],
    },
    Gradient {
        id: 12,
        name: "Aurora",
        angle: 145.0,
        stops: &[(0.0, 0x0E_2A_3A), (1.0, 0x07_12_1C)],
        blobs: &[
            blob(0.25, 0.75, 0.65, 0x2E_D9_A0, 0.45),
            blob(0.75, 0.3, 0.6, 0x5B_6B_E8, 0.35),
        ],
    },
    Gradient {
        id: 13,
        name: "Plum",
        angle: 150.0,
        stops: &[(0.0, 0x4A_1F_54), (1.0, 0x17_0A_23)],
        blobs: &[blob(0.7, 0.2, 0.65, 0xB0_4A_9E, 0.4)],
    },
    Gradient {
        id: 14,
        name: "Espresso",
        angle: 165.0,
        stops: &[(0.0, 0x3A_2A_22), (1.0, 0x15_0F_0C)],
        blobs: &[blob(0.3, 0.25, 0.7, 0x8A_63_45, 0.35)],
    },
    Gradient {
        id: 15,
        name: "Teal Night",
        angle: 155.0,
        stops: &[(0.0, 0x10_3A_3A), (1.0, 0x05_16_1A)],
        blobs: &[blob(0.75, 0.7, 0.65, 0x2A_9E_9E, 0.4)],
    },
    // --- neutral --------------------------------------------------------------------
    Gradient {
        id: 16,
        name: "Paper",
        angle: 180.0,
        stops: &[(0.0, 0xFA_F7_F2), (1.0, 0xE6_DF_D4)],
        blobs: &[],
    },
    Gradient {
        id: 17,
        name: "Fog",
        angle: 175.0,
        stops: &[(0.0, 0xE9_ED_F2), (1.0, 0xC2_CB_D6)],
        blobs: &[blob(0.5, 0.2, 0.8, 0xFF_FF_FF, 0.5)],
    },
    Gradient {
        id: 18,
        name: "Slate",
        angle: 160.0,
        stops: &[(0.0, 0x6B_74_82), (1.0, 0x33_3A_45)],
        blobs: &[blob(0.25, 0.2, 0.7, 0x9A_A5_B5, 0.35)],
    },
    Gradient {
        id: 19,
        name: "Graphite",
        angle: 170.0,
        stops: &[(0.0, 0x2E_31_36), (1.0, 0x16_18_1B)],
        blobs: &[blob(0.7, 0.25, 0.6, 0x55_5B_66, 0.35)],
    },
    Gradient {
        id: 20,
        name: "Sand",
        angle: 185.0,
        stops: &[(0.0, 0xEF_DE_C2), (1.0, 0xC9_A9_7A)],
        blobs: &[blob(0.3, 0.75, 0.7, 0xFF_F4_DE, 0.45)],
    },
];

/// The shipped gradient with that id, 1-based.
#[must_use]
pub fn gradient(id: u8) -> Option<&'static Gradient> {
    GRADIENT_SET.iter().find(|g| g.id == id)
}

/// A packed `0xRRGGBB` as the editor's own colour, opaque.
#[must_use]
pub fn rgb(packed: u32) -> Rgba {
    let channel = |shift: u32| f64::from((packed >> shift) & 0xFF) / 255.0;
    Rgba::new(channel(16), channel(8), channel(0), 1.0)
}

impl Blob {
    /// The wash's colour at its centre, with its own alpha.
    #[must_use]
    pub fn rgba(&self) -> Rgba {
        Rgba { a: self.alpha, ..rgb(self.color) }
    }
}

impl Gradient {
    /// The stops as colours, in order.
    #[must_use]
    pub fn colors(&self) -> Vec<(f64, Rgba)> {
        self.stops.iter().map(|(at, packed)| (*at, rgb(*packed))).collect()
    }

    /// The gradient's axis as a unit vector, from `angle`.
    ///
    /// Degrees clockwise from straight up, the way a CSS `linear-gradient` angle reads --
    /// so 180° is top to bottom, which is what most of these are.
    #[must_use]
    pub fn axis(&self) -> (f64, f64) {
        let radians = self.angle.to_radians();
        (radians.sin(), -radians.cos())
    }
}

/// One saved parameter set: `spec/05` §4.13's "**Presets**: save the parameter set with a
/// name; apply with one click; delete".
///
/// Appendix A.7's shape without its `deletable` flag. That flag exists in CleanShot to
/// mark presets the application itself shipped, and OctoSnap ships none -- §4.13's twenty
/// gradients are *backgrounds*, offered in the grid, and a preset is a thing the user
/// made. A field that is `true` in every record is not a field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    /// Stable across renames, because the "Default Preset" setting stores one of these
    /// and a rename must not quietly clear it.
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub params: BackgroundParams,
}

impl Preset {
    /// A new preset, with an id of its own.
    ///
    /// The same ULID an object gets, so the list sorts by when it was made without
    /// carrying a timestamp beside it.
    #[must_use]
    pub fn new(name: impl Into<String>, params: BackgroundParams) -> Self {
        Self {
            id: crate::object::ObjectId::new().to_string(),
            name: name.into(),
            params,
        }
    }

    /// Reads one back, or `None` when the text is not a preset this build understands.
    ///
    /// Total, because these live in a `gsettings` list that a future build may have
    /// written: one unreadable entry must cost the user that preset, not all of them.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        serde_json::from_str::<Self>(text).ok().map(|preset| Self {
            params: preset.params.clamped(),
            ..preset
        })
    }

    /// Writes one out, or `None` if it somehow will not serialise.
    #[must_use]
    pub fn to_json(&self) -> Option<String> {
        serde_json::to_string(self).ok()
    }
}

/// Where everything goes, for one image under one set of parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// What gets exported, at the origin. `spec/05` §5.3's canvas.
    pub canvas: Bounds,
    /// Where the base image sits inside it.
    pub image: Bounds,
    /// The corner radius in canvas units, from the relative one.
    pub radius: f64,
    /// How much the image was shrunk to get there: 1.0 unless an inset is set.
    ///
    /// Carried rather than recovered from `image.width / base.width`, because the
    /// renderer needs it as the scale factor of a transform and a divide by a
    /// zero-width base is not a case worth having in three files.
    pub scale: f64,
}

/// `spec/05` §4.13's geometry: the canvas the parameters imply and the image's place in it.
///
/// Three steps, in this order, and the order is what makes the two sliders different:
///
/// 1. **Padding** decides the canvas: a margin on all four sides of the image, so more of
///    it is a larger export at the picture's own resolution.
/// 2. **Ratio** grows that canvas again on whichever axis is short, never cropping -- a
///    background tool that cut the image off would be the one thing this tool must not
///    do -- and `alignment` then says where in the room that made the picture sits.
/// 3. **Inset** shrinks the picture inside the room padding left it, without touching the
///    canvas: the export stays the size padding chose and more background shows.
///
/// The inset is a **fit**, not four subtractions. Taking `inset` off each edge of a
/// rectangle is only a uniform scale when the rectangle is square, and at the maximum it
/// would turn a 1920 × 1080 capture into 1120 × 280 -- every circle an ellipse and every
/// annotation sheared with it. So the picture is fitted into the deflated box the way
/// every other "contain" fit works, which loses exactly `inset` per side on the short
/// axis and keeps the aspect on the long one.
///
/// With no ratio there is no extra room and every alignment gives the same answer, which
/// is correct: the 3 × 3 grid is for the case where the canvas is bigger than the image
/// needs, and `spec/05` §4.13 puts it beside the Ratio dropdown for exactly that reason.
/// The padding is a margin on all four sides rather than a budget the alignment spends,
/// so `alignment = 0` with no ratio does not push the picture into the top-left corner
/// and leave a double margin on the other two edges.
#[must_use]
pub fn layout(image: Bounds, params: &BackgroundParams) -> Layout {
    let params = params.clamped();
    let image = image.normalised();

    // 1. The room the padding asks for. This is the smallest canvas there can be, and
    //    the picture sits in the middle of it whatever the alignment says -- padding is a
    //    margin on all four sides, not a budget the alignment gets to spend.
    let (padded_width, padded_height) =
        (image.width + params.padding * 2.0, image.height + params.padding * 2.0);
    let (mut canvas_width, mut canvas_height) = (padded_width, padded_height);

    // 2. The ratio, by growing the short axis.
    if let Some(wanted) = params.ratio.and_then(Ratio::value) {
        let have = canvas_width / canvas_height.max(f64::EPSILON);
        if have < wanted {
            canvas_width = canvas_height * wanted;
        } else {
            canvas_height = canvas_width / wanted;
        }
    }

    // 3. The inset, as a contain fit into the padded box deflated by it. Never to
    //    nothing: `MAX_INSET` on a small capture would otherwise ask for a negative box.
    let shortest = image.width.min(image.height);
    let inset = params.inset.min((shortest - 1.0).max(0.0) / 2.0);
    let scale = if shortest > 0.0 { 1.0 - inset * 2.0 / shortest } else { 1.0 };
    let (width, height) = (image.width * scale, image.height * scale);

    // 4. The alignment spends what step 2 added plus what step 3 gave back, which is why
    //    the 3 × 3 grid says nothing at all until a ratio or an inset has made room.
    let (ax, ay) = params.anchor();
    let x = params.padding + (canvas_width - padded_width + image.width - width) * ax;
    let y = params.padding + (canvas_height - padded_height + image.height - height) * ay;

    Layout {
        canvas: Bounds::new(0.0, 0.0, canvas_width, canvas_height),
        image: Bounds::new(x, y, width, height),
        radius: params.corner_radius * width.min(height),
        scale,
    }
}

/// `spec/05` §4.13's Auto Balance, first half: the part of the image that is not border.
///
/// > trims the image's uniform border (colour within 2 % of the corner colour), then
/// > centres the *content* rather than the bitmap so the visual weight is balanced.
///
/// The corner colour rather than an average, because that is what a uniform border *is*
/// -- and the four corners are checked rather than one, so a screenshot with a coloured
/// title bar at the top and white below it is not trimmed on the strength of whichever
/// corner happened to be asked.
///
/// Answers in the image's own coordinates. `None` when there is no border to speak of,
/// which is the common case and the one where the checkbox should do nothing at all.
#[must_use]
pub fn content(pixels: &Pixels) -> Option<Bounds> {
    if pixels.width == 0 || pixels.height == 0 {
        return None;
    }
    let (width, height) = (pixels.width, pixels.height);
    // The corners agree or there is no uniform border. A single corner would trim a page
    // with a white margin on the left and a photograph on the right down to the photograph
    // and call it balance.
    let corner = at(pixels, 0, 0);
    for (x, y) in [(width - 1, 0), (0, height - 1), (width - 1, height - 1)] {
        if !alike(corner, at(pixels, x, y)) {
            return None;
        }
    }

    let row_is_border = |y: usize| (0..width).all(|x| alike(corner, at(pixels, x, y)));
    let column_is_border = |x: usize| (0..height).all(|y| alike(corner, at(pixels, x, y)));

    let top = (0..height).find(|y| !row_is_border(*y))?;
    let bottom = (0..height).rev().find(|y| !row_is_border(*y))?;
    let left = (0..width).find(|x| !column_is_border(*x))?;
    let right = (0..width).rev().find(|x| !column_is_border(*x))?;

    let trimmed = Bounds::new(
        left as f64,
        top as f64,
        (right - left + 1) as f64,
        (bottom - top + 1) as f64,
    );
    // Nothing was trimmed: say so, rather than answering with the whole image and making
    // every caller compare it against what it already had.
    (trimmed.width < width as f64 || trimmed.height < height as f64).then_some(trimmed)
}

/// How opaque a pixel has to be to be part of the window rather than its shadow.
///
/// A compositor's drop shadow is a soft alpha ramp and a window is not, so the two are
/// separated by almost the whole range and the exact number does not matter much. Just
/// under full, rather than full, because the outermost row of a window's own edge is
/// antialiased against what is behind it.
const OPAQUE: u8 = 250;

/// How much of each axis the body has to occupy before a trim is believed.
///
/// A window a user made translucent on purpose -- a terminal at 80 % -- has no fully
/// opaque body at all, and the answer to "where does the shadow end" is then whatever
/// solid thing happens to be drawn inside it. Refusing to trim is the right answer there:
/// the setting is about the compositor's shadow, and if it cannot be found, it stays.
const BODY_SHARE: f64 = 0.5;

/// `spec/08` §2's "Capture window shadow" turned off: the window without it.
///
/// > Capture window shadow \[on\]
///
/// `Shell.Screenshot.screenshot_window` writes the compositor's shadow into the PNG's
/// alpha -- twenty-five pixels of it on the machines `extension/src/capture.ts` measured
/// -- so there is nothing for this setting to *add*, and turning it off can only take
/// away what is already in the file. What it takes away is everything outside the
/// window's own body, and the body is what is opaque.
///
/// Answers in the buffer's own pixel coordinates. `None` when there is nothing to trim:
/// a capture with no transparent margin, one the compositor drew no shadow around, or a
/// window too translucent to have a body at all.
#[must_use]
pub fn window_body(pixels: &Pixels) -> Option<Bounds> {
    if pixels.width == 0 || pixels.height == 0 {
        return None;
    }
    let (width, height) = (pixels.width, pixels.height);
    let opaque = |x: usize, y: usize| at(pixels, x, y)[3] >= OPAQUE;
    let row_has_body = |y: usize| (0..width).any(|x| opaque(x, y));
    let column_has_body = |x: usize| (0..height).any(|y| opaque(x, y));

    let top = (0..height).find(|y| row_has_body(*y))?;
    let bottom = (0..height).rev().find(|y| row_has_body(*y))?;
    let left = (0..width).find(|x| column_has_body(*x))?;
    let right = (0..width).rev().find(|x| column_has_body(*x))?;

    let (body_width, body_height) = ((right - left + 1) as f64, (bottom - top + 1) as f64);
    // Not a window body: see [`BODY_SHARE`].
    if body_width < width as f64 * BODY_SHARE || body_height < height as f64 * BODY_SHARE {
        return None;
    }
    // Nothing to trim, which is most captures: say so rather than answering with the
    // whole image and making the caller compare it against what it already had.
    (body_width < width as f64 || body_height < height as f64)
        .then(|| Bounds::new(left as f64, top as f64, body_width, body_height))
}

/// `spec/05` §4.13's Auto Balance, second half: the margins that centre the *content*.
///
/// The returned rectangle is the image's, moved so that `content` ends up in the middle
/// of it. A screenshot with a 40 px chrome at the top and nothing at the bottom is drawn
/// 20 px higher than its bitmap would put it, which is what "the visual weight is
/// balanced" means when it is written down.
#[must_use]
pub fn balanced(image: Bounds, content: Bounds) -> Bounds {
    let image = image.normalised();
    // How far the content's centre is from the image's, in the image's own units.
    let dx = (content.x + content.width / 2.0) - image.width / 2.0;
    let dy = (content.y + content.height / 2.0) - image.height / 2.0;
    Bounds::new(image.x - dx, image.y - dy, image.width, image.height)
}

/// `spec/05` §4.13's Auto Balance as the editor stores it: a shift of the *source* rect.
///
/// [`balanced`] answers where the picture should be *drawn*, and the document has nowhere
/// to put that: `Scene::placement` maps the source rect onto wherever [`layout`] placed
/// it, so the only way to move the picture without a pixel buffer at render time is to
/// move the rect that gets mapped. Shifting the source the other way does exactly that,
/// and it means auto-balance costs the renderer nothing and survives a reopened project.
///
/// It answers a **vector**, not a rectangle, because the shift is relative and the stored
/// source already carries it whenever the parameter is on: a caller that adds it to a
/// source it has already added it to moves the picture twice, and a panel that recomputes
/// its parameters on every slider does that once per frame. So this is added when the
/// checkbox goes on and subtracted when it goes off, and nothing else ever touches it.
///
/// The opposite sign is the whole content of this function, and the reason it is here
/// rather than inlined in the panel: a sign error would centre the content *further* off
/// and look like a plausible amount of balance.
#[must_use]
pub fn balance_shift(source: Bounds, content: Bounds) -> (f64, f64) {
    let moved = balanced(source, content);
    (source.x - moved.x, source.y - moved.y)
}

fn at(pixels: &Pixels, x: usize, y: usize) -> [u8; 4] {
    pixels.at(x, y)
}

/// Within `spec/05` §4.13's "2 % of the corner colour", on every channel including alpha.
fn alike(a: [u8; 4], b: [u8; 4]) -> bool {
    let tolerance = UNIFORM * 255.0;
    (0..4).all(|i| f64::from(a[i].abs_diff(b[i])) <= tolerance)
}

fn clamp(value: f64, low: f64, high: f64) -> f64 {
    if value.is_nan() { low } else { value.clamp(low, high) }
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn image() -> Bounds {
        Bounds::new(0.0, 0.0, 800.0, 600.0)
    }

    /// Appendix A.7's preset, read back field for field.
    #[test]
    fn the_defaults_are_the_presets_defaults() {
        let params = BackgroundParams::default();
        assert_eq!(params.padding, 100.0);
        assert_eq!(params.inset, 0.0);
        assert_eq!(params.corner_radius, 0.08);
        assert_eq!(params.ratio, None);
        assert_eq!(params.alignment, 4);
        assert!(!params.auto_balance);
        assert_eq!(params.shadow_intensity, 28.0);
        assert_eq!(params.background, Background::None);
    }

    /// `spec/05` §11's acceptance list wants a preset to round-trip, and appendix A.7's
    /// names are the ones it round-trips through.
    #[test]
    fn a_preset_round_trips_through_its_documented_names() {
        let params = BackgroundParams {
            background: Background::Gradient { id: 18 },
            padding: 120.0,
            inset: 8.0,
            corner_radius: 0.12,
            ratio: Some(Ratio::new(16, 9)),
            alignment: 0,
            auto_balance: true,
            shadow_intensity: 40.0,
        };
        let json = serde_json::to_string(&params).expect("serialises");
        let names = [
            "padding", "inset", "cornerRadius", "ratio", "alignment", "autoBalance",
            "shadowIntensity",
        ];
        for key in names {
            assert!(json.contains(key), "{key} is missing from {json}");
        }
        let back: BackgroundParams = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back, params);
        // And byte-identical the second time round, which is what §11 item 4 asks of
        // everything in `objects.json`.
        assert_eq!(serde_json::to_string(&back).expect("again"), json);
    }

    /// Auto is `null`, and a preset written with Auto must not grow a `"ratio": null` key
    /// that a byte-identical comparison would trip over.
    #[test]
    fn auto_is_absent_rather_than_null() {
        let json = serde_json::to_string(&BackgroundParams::default()).expect("serialises");
        assert!(!json.contains("ratio"), "{json}");
    }

    #[test]
    fn padding_grows_the_canvas_around_the_image() {
        let params = BackgroundParams { padding: 50.0, ..BackgroundParams::default() };
        let out = layout(image(), &params);
        assert_eq!(out.canvas, Bounds::new(0.0, 0.0, 900.0, 700.0));
        assert_eq!(out.image, Bounds::new(50.0, 50.0, 800.0, 600.0));
    }

    /// D87: padding makes a bigger export at the same picture size; inset makes a smaller
    /// picture in the same export. The two sliders are not one slider twice.
    #[test]
    fn inset_shrinks_the_picture_rather_than_the_canvas() {
        let padded = layout(image(), &BackgroundParams {
            padding: 50.0,
            ..BackgroundParams::default()
        });
        let inset = layout(image(), &BackgroundParams {
            padding: 50.0,
            inset: 30.0,
            ..BackgroundParams::default()
        });
        // The export is the one padding chose, whatever the inset does inside it.
        assert_eq!(inset.canvas, padded.canvas);
        // 800 x 600 losing 30 a side on the short axis is a tenth off both.
        assert!((inset.scale - 0.9).abs() < 1e-9, "{}", inset.scale);
        assert!((inset.image.width - 720.0).abs() < 1e-9, "{:?}", inset.image);
        assert!((inset.image.height - 540.0).abs() < 1e-9, "{:?}", inset.image);
        // Centred in the room padding left it, so the 30 on the short axis is real and
        // the long axis gets the 40 that keeps the aspect.
        assert!((inset.image.x - 90.0).abs() < 1e-9, "{:?}", inset.image);
        assert!((inset.image.y - 80.0).abs() < 1e-9, "{:?}", inset.image);
        assert!((padded.scale - 1.0).abs() < 1e-9);
    }

    /// The picture keeps its shape at every inset, which four subtractions would not.
    #[test]
    fn an_inset_never_changes_the_pictures_aspect() {
        let aspect = image().width / image().height;
        for inset in [0.0, 1.0, 17.5, 100.0, 299.0, MAX_INSET] {
            let out = layout(image(), &BackgroundParams {
                inset,
                ..BackgroundParams::default()
            });
            assert!(out.image.width > 0.0 && out.image.height > 0.0, "{out:?} at {inset}");
            let had = out.image.width / out.image.height;
            assert!((had - aspect).abs() < 1e-9, "{had} != {aspect} at {inset}");
        }
    }

    /// A background tool that cropped the picture would be the one thing it must not do.
    #[test]
    fn a_ratio_grows_the_canvas_and_never_crops() {
        let params = BackgroundParams {
            padding: 0.0,
            ratio: Some(Ratio::new(1, 1)),
            ..BackgroundParams::default()
        };
        let out = layout(image(), &params);
        assert_eq!(out.canvas.width, 800.0);
        assert_eq!(out.canvas.height, 800.0);
        assert_eq!(out.image.width, 800.0);
        assert_eq!(out.image.height, 600.0);
    }

    #[test]
    fn a_wide_ratio_grows_the_other_way() {
        let params = BackgroundParams {
            padding: 0.0,
            ratio: Some(Ratio::new(16, 9)),
            ..BackgroundParams::default()
        };
        let out = layout(image(), &params);
        assert_eq!(out.canvas.height, 600.0);
        assert!((out.canvas.width - 600.0 * 16.0 / 9.0).abs() < 1e-9, "{out:?}");
    }

    /// The 3 × 3 grid only has anything to say once a ratio has made room, which is why
    /// `spec/05` §4.13 puts it beside the Ratio dropdown.
    #[test]
    fn alignment_places_the_image_in_the_room_the_ratio_made() {
        let params = |alignment| BackgroundParams {
            padding: 0.0,
            ratio: Some(Ratio::new(1, 1)),
            alignment,
            ..BackgroundParams::default()
        };
        assert_eq!(layout(image(), &params(0)).image.y, 0.0);
        assert_eq!(layout(image(), &params(4)).image.y, 100.0);
        assert_eq!(layout(image(), &params(8)).image.y, 200.0);
        // Nothing to move horizontally: the ratio grew the height, not the width.
        for alignment in [0, 4, 8] {
            assert_eq!(layout(image(), &params(alignment)).image.x, 0.0);
        }
    }

    #[test]
    fn with_no_ratio_every_alignment_is_the_same() {
        let out: Vec<_> = (0..9)
            .map(|alignment| {
                layout(image(), &BackgroundParams {
                    alignment,
                    ..BackgroundParams::default()
                })
                .image
            })
            .collect();
        assert!(out.windows(2).all(|pair| pair[0] == pair[1]), "{out:?}");
    }

    /// Relative, so one preset looks the same on a thumbnail and on a 5K capture.
    #[test]
    fn the_corner_radius_is_a_fraction_of_the_shorter_side() {
        let params = BackgroundParams { corner_radius: 0.1, ..BackgroundParams::default() };
        assert_eq!(layout(image(), &params).radius, 60.0);
        assert_eq!(layout(Bounds::new(0.0, 0.0, 200.0, 150.0), &params).radius, 15.0);
    }

    /// A typed value, or a preset from a build with wider sliders.
    #[test]
    fn values_out_of_range_are_clamped_rather_than_refused() {
        let wild = BackgroundParams {
            padding: -40.0,
            inset: 9_000.0,
            corner_radius: 3.0,
            alignment: 42,
            shadow_intensity: 400.0,
            ratio: Some(Ratio::new(0, 9)),
            ..BackgroundParams::default()
        };
        let sane = wild.clamped();
        assert_eq!(sane.padding, 0.0);
        assert_eq!(sane.inset, MAX_INSET);
        assert_eq!(sane.corner_radius, MAX_CORNER_RADIUS);
        assert_eq!(sane.alignment, 8);
        assert_eq!(sane.shadow_intensity, MAX_SHADOW);
        assert_eq!(sane.ratio, None);
    }

    /// An inset past half the image would turn the picture inside out.
    #[test]
    fn an_inset_larger_than_the_image_still_leaves_a_picture() {
        let out = layout(image(), &BackgroundParams {
            inset: MAX_INSET,
            ..BackgroundParams::default()
        });
        assert!(out.image.width > 0.0 && out.image.height > 0.0, "{out:?}");
    }

    /// The shift has to move the *content* to the middle, not away from it.
    #[test]
    fn rebalancing_puts_the_content_where_the_bitmaps_centre_was() {
        let source = Bounds::new(0.0, 0.0, 800.0, 600.0);
        // A 160 px margin down the left and 40 px along the top, and none opposite: the
        // content's centre is 80 to the right of the bitmap's and 20 below it.
        let content = Bounds::new(160.0, 40.0, 640.0, 560.0);
        let (dx, dy) = balance_shift(source, content);
        let shifted = Bounds::new(source.x + dx, source.y + dy, source.width, source.height);
        // The picture has to move left and up by that much, which the source does by
        // moving right and down.
        assert!((shifted.x - 80.0).abs() < 1e-9, "{shifted:?}");
        assert!((shifted.y - 20.0).abs() < 1e-9, "{shifted:?}");
        assert_eq!((shifted.width, shifted.height), (800.0, 600.0));

        // And the layout then draws the content in the middle of the padded canvas.
        let params = BackgroundParams { padding: 50.0, ..BackgroundParams::default() };
        let out = layout(shifted, &params);
        let content_centre_x = out.image.x + (content.x + content.width / 2.0 - shifted.x);
        assert!(
            (content_centre_x - out.canvas.width / 2.0).abs() < 1e-9,
            "{content_centre_x} is not the middle of {:?}",
            out.canvas
        );
    }

    /// A capture with no uniform border is not trimmed, so the checkbox does nothing --
    /// which is the common case and the one where doing something would be wrong.
    #[test]
    fn a_capture_with_no_border_is_left_where_it_is() {
        let mut rgba = Vec::new();
        for y in 0..20_u8 {
            for x in 0..20_u8 {
                rgba.extend_from_slice(&[x * 12, y * 12, 90, 255]);
            }
        }
        let pixels = Pixels { width: 20, height: 20, rgba };
        assert_eq!(content(&pixels), None);
    }

    /// `spec/05` §5.3 draws the object below the base image; one that draws nothing is a
    /// row in `objects.json` the undo history carries for no reason.
    #[test]
    fn a_background_that_draws_nothing_says_so() {
        assert!(BackgroundParams { padding: 0.0, ..BackgroundParams::default() }.is_empty());
        assert!(!BackgroundParams::default().is_empty());
        assert!(
            !BackgroundParams {
                padding: 0.0,
                background: Background::Color { color: Rgba::new(1.0, 0.0, 0.0, 1.0) },
                ..BackgroundParams::default()
            }
            .is_empty()
        );
    }

    /// A bordered image, with a smaller block of content off-centre inside it.
    fn bordered(border: [u8; 4], ink: [u8; 4], content: Bounds) -> Pixels {
        let (width, height) = (40_usize, 30_usize);
        let mut pixels = Pixels::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let inside = (x as f64) >= content.x
                    && (x as f64) < content.x + content.width
                    && (y as f64) >= content.y
                    && (y as f64) < content.y + content.height;
                let colour = if inside { ink } else { border };
                let at = (y * width + x) * 4;
                pixels.rgba[at..at + 4].copy_from_slice(&colour);
            }
        }
        pixels
    }

    #[test]
    fn a_uniform_border_is_found_and_trimmed() {
        let white = [255, 255, 255, 255];
        let black = [0, 0, 0, 255];
        let inner = Bounds::new(6.0, 4.0, 20.0, 10.0);
        let found = content(&bordered(white, black, inner)).expect("a border");
        assert_eq!(found, inner);
    }

    /// "Within 2 % of the corner colour": a border that is not quite flat is still border.
    #[test]
    fn a_border_within_two_percent_still_counts() {
        let white = [255, 255, 255, 255];
        let nearly = [251, 253, 255, 255];
        let inner = Bounds::new(6.0, 4.0, 20.0, 10.0);
        let mut pixels = bordered(white, [0, 0, 0, 255], inner);
        // Dapple the border with the near-white, which is inside the tolerance.
        for x in 0..pixels.width {
            let at = x * 4;
            pixels.rgba[at..at + 4].copy_from_slice(&nearly);
        }
        assert_eq!(content(&pixels).expect("still a border"), inner);
    }

    /// The common case, and the one where the checkbox has to do nothing at all.
    #[test]
    fn an_image_with_no_border_is_left_alone() {
        let full = Bounds::new(0.0, 0.0, 40.0, 30.0);
        assert_eq!(content(&bordered([255; 4], [0, 0, 0, 255], full)), None);
    }

    /// One corner is not evidence of a uniform border: a page with a white margin on the
    /// left and a photograph on the right would be trimmed down to the photograph.
    #[test]
    fn corners_that_disagree_are_not_a_border() {
        let mut pixels = bordered([255; 4], [0, 0, 0, 255], Bounds::new(6.0, 4.0, 20.0, 10.0));
        let last = (pixels.width - 1) * 4;
        pixels.rgba[last..last + 4].copy_from_slice(&[12, 34, 56, 255]);
        assert_eq!(content(&pixels), None);
    }

    /// A window in the middle of a soft shadow, which is what `screenshot_window` writes.
    fn windowed(width: usize, height: usize, margin: usize) -> Pixels {
        let mut pixels = Pixels::new(width, height);
        for y in 0..height {
            for x in 0..width {
                // How far inside the margin this pixel is, as a shadow ramp: nothing at
                // the very edge, and never opaque.
                let depth = x.min(y).min(width - 1 - x).min(height - 1 - y);
                let inside = depth >= margin;
                let alpha = if inside {
                    255
                } else {
                    u8::try_from(depth * 200 / margin.max(1)).unwrap_or(200)
                };
                let i = (y * width + x) * 4;
                pixels.rgba[i] = 40;
                pixels.rgba[i + 1] = 44;
                pixels.rgba[i + 2] = 52;
                pixels.rgba[i + 3] = alpha;
            }
        }
        pixels
    }

    /// `spec/08` §2's shadow switch, which can only ever subtract.
    #[test]
    fn the_window_body_is_what_the_compositor_did_not_blur() {
        let body = window_body(&windowed(410, 290, 25)).expect("no body found");
        assert_eq!((body.x, body.y), (25.0, 25.0));
        assert_eq!((body.width, body.height), (360.0, 240.0));
    }

    #[test]
    fn a_capture_with_no_shadow_has_nothing_to_trim() {
        assert!(window_body(&windowed(360, 240, 0)).is_none());
    }

    /// A terminal at 80 %: there is no opaque body, so the shadow cannot be found and the
    /// honest answer is to leave the capture alone.
    #[test]
    fn a_translucent_window_is_not_trimmed_to_whatever_is_solid_inside_it() {
        let mut pixels = windowed(410, 290, 25);
        for y in 25..265 {
            for x in 25..385 {
                // Translucent everywhere except a small solid button.
                let solid = (180..220).contains(&x) && (120..140).contains(&y);
                pixels.rgba[(y * 410 + x) * 4 + 3] = if solid { 255 } else { 204 };
            }
        }
        assert!(window_body(&pixels).is_none());
    }

    /// The bug this shape of API exists to make impossible.
    ///
    /// The panel recomputes its parameters on every change, so whatever it asks for is
    /// asked for again on the next slider, the next checkbox and the next sync. A shift
    /// that came back as a *rectangle* was added to a source that already carried it, and
    /// fifty-five of those in a row put a 1200 × 700 screenshot 7095 units off its own
    /// canvas -- reported from the editor as "the screenshot disappeared".
    #[test]
    fn the_shift_is_the_same_whatever_it_has_already_been_applied_to() {
        let source = Bounds::new(0.0, 0.0, 1200.0, 700.0);
        let content = Bounds::new(61.0, 64.0, 820.0, 400.0);
        let (dx, dy) = balance_shift(source, content);
        assert!((dx - -129.0).abs() < 1e-9 && (dy - -86.0).abs() < 1e-9, "{dx}, {dy}");
        // Applied, and asked again from where it landed: the same answer, so adding it
        // once and taking it away once is the identity however often the panel asks.
        let shifted = Bounds::new(source.x + dx, source.y + dy, source.width, source.height);
        assert_eq!(balance_shift(shifted, content), (dx, dy));
        let back = Bounds::new(shifted.x - dx, shifted.y - dy, source.width, source.height);
        assert_eq!((back.x, back.y), (source.x, source.y));
    }

    /// `spec/05` §4.13: "centres the *content* rather than the bitmap".
    #[test]
    fn balancing_moves_the_image_so_the_content_is_central() {
        let image = Bounds::new(0.0, 0.0, 100.0, 100.0);
        // Content sitting in the top half: its centre is 25 px above the image's.
        let inner = Bounds::new(20.0, 10.0, 60.0, 40.0);
        let moved = balanced(image, inner);
        assert_eq!(moved.y, 20.0);
        assert_eq!(moved.x, 0.0);
        assert_eq!(moved.width, image.width);
    }

    #[test]
    fn balancing_an_already_central_image_moves_nothing() {
        let image = Bounds::new(7.0, 9.0, 100.0, 100.0);
        let inner = Bounds::new(20.0, 20.0, 60.0, 60.0);
        assert_eq!(balanced(image, inner), image);
    }

    /// `spec/05` §4.13's presets, through the list they are stored in.
    #[test]
    fn a_preset_survives_being_written_and_read() {
        let preset = Preset::new("Dark 16:9", BackgroundParams {
            background: Background::Color { color: Rgba::new(0.1, 0.1, 0.12, 1.0) },
            ratio: Some(Ratio::new(16, 9)),
            padding: 160.0,
            ..BackgroundParams::default()
        });
        let text = preset.to_json().expect("serialises");
        let back = Preset::parse(&text).expect("parses");
        assert_eq!(back, preset);
        assert_eq!(back.name, "Dark 16:9");
        // The params are flattened, as appendix A.7 has them: one object, not two.
        assert!(text.contains("\"padding\":160"), "{text}");
    }

    /// One unreadable entry costs the user that preset, not the whole list.
    #[test]
    fn a_preset_this_build_cannot_read_is_skipped() {
        assert_eq!(Preset::parse("not json at all"), None);
        assert_eq!(Preset::parse("{}"), None);
    }

    /// A preset written by a build with wider sliders opens to its nearest legal shape.
    #[test]
    fn a_preset_from_the_future_is_clamped_on_the_way_in() {
        let wild = Preset { id: "x".to_owned(), name: "Wild".to_owned(), params: BackgroundParams {
            padding: 4_000.0,
            corner_radius: 9.0,
            ..BackgroundParams::default()
        }};
        let text = wild.to_json().expect("serialises");
        let back = Preset::parse(&text).expect("parses");
        assert_eq!(back.params.padding, MAX_PADDING);
        assert_eq!(back.params.corner_radius, MAX_CORNER_RADIUS);
    }

    /// Renaming must not clear the "Default Preset" setting, which stores an id.
    #[test]
    fn two_presets_made_together_do_not_share_an_id() {
        let one = Preset::new("A", BackgroundParams::default());
        let two = Preset::new("B", BackgroundParams::default());
        assert_ne!(one.id, two.id);
        assert!(!one.id.is_empty());
    }

    #[test]
    fn a_gradient_names_the_asset_it_ships_as() {
        assert_eq!(Background::Gradient { id: 18 }.asset().as_deref(), Some("bg18"));
        assert_eq!(Background::None.asset(), None);
    }

    /// `spec/05` §4.13's "4 × 5 grid of twenty gradient swatches", and appendix A.7's
    /// `bg1…bg20`: the ids are 1 to 20, once each, in order.
    #[test]
    fn the_twenty_gradients_are_numbered_one_to_twenty() {
        assert_eq!(GRADIENT_SET.len(), GRADIENTS as usize);
        for (index, g) in GRADIENT_SET.iter().enumerate() {
            assert_eq!(usize::from(g.id), index + 1, "{}", g.name);
            assert!(!g.name.is_empty());
            assert!(gradient(g.id).is_some());
        }
        assert!(gradient(0).is_none());
        assert!(gradient(GRADIENTS + 1).is_none());
    }

    /// A swatch the user cannot tell from the one beside it is a swatch that is not
    /// there. Every pair of first stops differs by more than a rounding error.
    #[test]
    fn no_two_gradients_are_the_same() {
        for (i, a) in GRADIENT_SET.iter().enumerate() {
            for b in GRADIENT_SET.iter().skip(i + 1) {
                assert_ne!(
                    (a.stops, a.angle.to_bits(), a.blobs.len()),
                    (b.stops, b.angle.to_bits(), b.blobs.len()),
                    "{} and {} are the same swatch",
                    a.name,
                    b.name
                );
            }
        }
    }

    /// Every stop is somewhere on the axis, and they run in order: a gradient whose
    /// second stop came before its first would draw as a hard edge.
    #[test]
    fn every_gradient_has_stops_in_order_from_end_to_end() {
        for g in &GRADIENT_SET {
            assert!(g.stops.len() >= 2, "{}", g.name);
            assert_eq!(g.stops.first().map(|s| s.0), Some(0.0), "{}", g.name);
            assert_eq!(g.stops.last().map(|s| s.0), Some(1.0), "{}", g.name);
            assert!(
                g.stops.windows(2).all(|pair| pair[0].0 < pair[1].0),
                "{} has stops out of order",
                g.name
            );
            for blob in g.blobs {
                assert!((0.0..=1.0).contains(&blob.x), "{}", g.name);
                assert!((0.0..=1.0).contains(&blob.y), "{}", g.name);
                assert!(blob.radius > 0.0 && blob.alpha > 0.0, "{}", g.name);
            }
        }
    }

    #[test]
    fn a_packed_colour_unpacks_to_its_channels() {
        let white = rgb(0xFF_FF_FF);
        assert_eq!((white.r, white.g, white.b, white.a), (1.0, 1.0, 1.0, 1.0));
        let orange = rgb(0xFF_80_00);
        assert_eq!(orange.r, 1.0);
        assert!((orange.g - 128.0 / 255.0).abs() < 1e-9);
        assert_eq!(orange.b, 0.0);
    }

    /// 180° is top to bottom, the way a CSS angle reads.
    #[test]
    fn the_axis_points_the_way_the_angle_says() {
        let down = Gradient { angle: 180.0, ..GRADIENT_SET[0] };
        let (x, y) = down.axis();
        assert!(x.abs() < 1e-9, "{x}");
        assert!((y - 1.0).abs() < 1e-9, "{y}");
        let right = Gradient { angle: 90.0, ..GRADIENT_SET[0] };
        let (x, y) = right.axis();
        assert!((x - 1.0).abs() < 1e-9, "{x}");
        assert!(y.abs() < 1e-9, "{y}");
    }

    #[test]
    fn the_blurred_swatches_get_stronger_in_order() {
        let radii: Vec<f64> = Blur::ALL.iter().map(|b| b.radius()).collect();
        assert!(radii.windows(2).all(|pair| pair[0] < pair[1]), "{radii:?}");
    }
}
