// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/05` §2's tool strip and §4's per-tool gestures, with no widget in sight.
//!
//! A tool is a function from a drag to an [`Object`], and that function is where every
//! rule in §4 lives: Shift snapping an arrow to 45°, Alt drawing a rectangle from its
//! centre, the eight-unit minimum that makes a stray click leave nothing behind, the
//! resampling that stops a freehand path storing one point per motion event. All of it is
//! arithmetic on `f64` image coordinates, so all of it is testable at full speed -- which
//! matters more than it looks, because the alternative is confirming "Shift makes a
//! square" by dragging one and squinting at it.
//!
//! What is deliberately *not* here: which object is selected, what the pointer is doing
//! right now, and anything about a `GtkGesture`. A [`Draft`] is handed points and answers
//! with geometry; the editor decides where the points came from.

use crate::geometry::{Bounds, Point};
use crate::object::{Geometry, Object, TextAlign};
use crate::style::{
    ArrowHead, ArrowStyle, CounterStyle, RedactStyle, SpotlightShape, Style, TextStyle,
};

/// `spec/05` §4.2: "Minimum length 8 units (otherwise cancelled)."
///
/// Applied to every dragged tool and not only the arrow, because the failure it prevents
/// is the same one everywhere: a click that moves three pixels while the button is down
/// is a click, and it should not leave a 3x1 rectangle behind for the user to hunt for.
/// Measured on the drag's diagonal rather than per axis, so a deliberately thin shape --
/// a 200x1 underline is a real thing to draw -- still counts.
pub const MIN_DRAG: f64 = 8.0;

/// `spec/05` §4.6: "input points are resampled to >= 2 units spacing".
pub const RESAMPLE_SPACING: f64 = 2.0;

/// `spec/05` §4.7: the highlighter's alpha. "colour alpha 55 %".
pub const HIGHLIGHTER_ALPHA: f64 = 0.55;

/// `spec/05` §4.7: "a flat wide stroke (`3 x size`)".
///
/// Here rather than in the renderer because it is part of the object's *bounds*, which
/// the hit test and the selection chrome both need and neither of which can see a
/// `gsk::Stroke`.
pub const HIGHLIGHTER_WIDTH_FACTOR: f64 = 3.0;

/// How wide a highlighter stroke is drawn: `spec/05` §4.7's `3 x size`, or the band it
/// snapped to.
///
/// One function, because three callers have to agree exactly -- the renderer, the hit
/// test and the selection chrome -- and the version where each multiplied by three itself
/// was the version where a snapped stroke's outline sat inside the wash.
#[must_use]
pub fn highlighter_width(stroke: f64, band: Option<f64>) -> f64 {
    band.filter(|b| *b > 0.0).unwrap_or(stroke * HIGHLIGHTER_WIDTH_FACTOR)
}

/// `spec/05` §4.4: "corner radius option `r = 2 x size` when enabled".
const RADIUS_PER_SIZE: f64 = 2.0;

/// How far a curved arrow bows, as a fraction of its length.
///
/// `spec/05` §4.2 says Curved and Double render as arcs but does not give a number, so
/// this is [M]. A fifth of the length reads as a deliberate curve at any size without the
/// head swinging away from the direction the user dragged.
const CURVE_BOW: f64 = 0.2;

/// One entry in `spec/05` §2's tool strip.
///
/// `Resize`, `Rotate`, `Background`, `Add image` and `Screenshot` are in §2's table but
/// not in the strip -- the table's own "Options row" column puts them in menus -- so they
/// are not here either.
// `Ord` so the editor can key its per-tool memory by one: `spec/05` §2 ends with "the
// last colour and size **per tool** persist".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum Tool {
    #[default]
    Select,
    Arrow,
    Line,
    Rect,
    FilledRect,
    Ellipse,
    Text,
    Pencil,
    Highlighter,
    Spotlight,
    Counter,
    Pixelate,
    Crop,
}

impl Tool {
    /// The strip's order, which is `spec/05` §2's table order.
    pub const ALL: [Self; 13] = [
        Self::Select,
        Self::Arrow,
        Self::Line,
        Self::Rect,
        Self::FilledRect,
        Self::Ellipse,
        Self::Text,
        Self::Pencil,
        Self::Highlighter,
        Self::Spotlight,
        Self::Counter,
        Self::Pixelate,
        Self::Crop,
    ];

    /// The strip's groups, in the strip's order (`spec/13` #5, Miller's law).
    ///
    /// Thirteen identical toggles in one run is a list nobody holds in mind; five short
    /// runs -- selection, shapes, text, marks, layers, mode -- is `spec/05` §1's own
    /// reading of the reference ("canvas actions in their own group, then the tools").
    /// Here rather than in the widget so the grouping is data the strip reads and a test
    /// can check covers [`Self::ALL`] exactly once, in order.
    pub const CHUNKS: [&'static [Self]; 6] = [
        &[Self::Select],
        &[Self::Arrow, Self::Line, Self::Rect, Self::FilledRect, Self::Ellipse],
        &[Self::Text],
        &[Self::Pencil, Self::Highlighter],
        &[Self::Spotlight, Self::Counter, Self::Pixelate],
        &[Self::Crop],
    ];

    /// `spec/05` §2's Key column.
    ///
    /// Returned as an accelerator string rather than a keyval so the editor can hand it
    /// straight to `ShortcutTrigger::parse_string`, and so `Shift+R` is expressible --
    /// it is the one entry that is not a bare letter.
    #[must_use]
    pub const fn accelerator(self) -> &'static str {
        match self {
            Self::Select => "v",
            Self::Arrow => "a",
            Self::Line => "l",
            Self::Rect => "r",
            Self::FilledRect => "<Shift>R",
            Self::Ellipse => "e",
            Self::Text => "t",
            Self::Pencil => "p",
            Self::Highlighter => "h",
            Self::Spotlight => "s",
            Self::Counter => "c",
            Self::Pixelate => "b",
            Self::Crop => "x",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Arrow => "Arrow",
            Self::Line => "Line",
            Self::Rect => "Rectangle",
            Self::FilledRect => "Filled rectangle",
            Self::Ellipse => "Ellipse",
            Self::Text => "Text",
            Self::Pencil => "Pencil",
            Self::Highlighter => "Highlighter",
            Self::Spotlight => "Spotlight",
            Self::Counter => "Counter",
            Self::Pixelate => "Pixelate",
            Self::Crop => "Crop",
        }
    }

    /// A symbolic icon name.
    ///
    /// Named here rather than in the widget because the strip is not the only thing that
    /// needs them -- §4.11's crop mode swaps the whole toolbar and still shows the tool
    /// it came from -- and because a missing icon should be one edit to fix, not a hunt.
    ///
    /// The `tool-*` names are the application's own, from `crates/app/resources`; the
    /// Pencil's is Adwaita's. The split is not a preference. **Adwaita ships no line, no
    /// rectangle, no circle, no crop frame and no marker** -- searching its symbolic set
    /// for those words finds text-direction arrows and microphones -- and the first
    /// version of this function named the ones it seemed obvious it *would* have. The
    /// editor then drew six broken-image glyphs, which is what a screenshot showed and no
    /// test could: an icon name is a string until a display resolves it. Adwaita's
    /// nearest for Spotlight and Pixelate were a sun and a crossed-out eye, which say
    /// "brightness" and "hide", so those are OctoSnap's own too (`spec/09` §4b).
    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Select => "tool-select-symbolic",
            Self::Arrow => "tool-arrow-symbolic",
            Self::Line => "tool-line-symbolic",
            Self::Rect => "tool-rect-symbolic",
            Self::FilledRect => "tool-rect-filled-symbolic",
            Self::Ellipse => "tool-ellipse-symbolic",
            Self::Text => "tool-text-symbolic",
            Self::Pencil => "document-edit-symbolic",
            Self::Highlighter => "tool-highlighter-symbolic",
            Self::Spotlight => "tool-spotlight-symbolic",
            Self::Counter => "tool-counter-symbolic",
            Self::Pixelate => "tool-pixelate-symbolic",
            Self::Crop => "tool-crop-symbolic",
        }
    }

    /// Whether a gesture with this tool produces a new object.
    ///
    /// Select edits what is already there and Crop changes the canvas, so both answer no
    /// and neither goes through [`Draft`].
    #[must_use]
    pub const fn creates(self) -> bool {
        !matches!(self, Self::Select | Self::Crop)
    }

    /// Whether one click is the whole gesture (`spec/05` §4.9, §4.5).
    ///
    /// The counter's badge and the text box are *placed*, not dragged out, so the minimum
    /// drag does not apply to them -- applying it would make the counter tool need a
    /// wiggle to work.
    #[must_use]
    pub const fn placed_by_click(self) -> bool {
        matches!(self, Self::Counter | Self::Text)
    }

    /// Whether the gesture accumulates a path rather than two corners.
    #[must_use]
    pub const fn is_freehand(self) -> bool {
        matches!(self, Self::Pencil | Self::Highlighter)
    }

    /// The tool that draws objects like this one, or `None` for the kinds no tool draws.
    ///
    /// `spec/05` §2's options row is per *tool*, and once a selected object is restyled
    /// from that row (D56) the row has to know whose controls to show for it. Two kinds
    /// split on a flag: a filled rectangle and a highlighter stroke are the same geometry
    /// as their hollow and pencil siblings, with different rows.
    #[must_use]
    pub const fn for_object(object: &Object) -> Option<Self> {
        Some(match &object.geometry {
            Geometry::Arrow { .. } => Self::Arrow,
            Geometry::Line { .. } => Self::Line,
            Geometry::Rect { filled: true, .. } => Self::FilledRect,
            Geometry::Rect { .. } => Self::Rect,
            Geometry::Ellipse { .. } => Self::Ellipse,
            Geometry::Text { .. } => Self::Text,
            Geometry::Path { highlighter: true, .. } => Self::Highlighter,
            Geometry::Path { .. } => Self::Pencil,
            Geometry::Spotlight { .. } => Self::Spotlight,
            Geometry::Counter { .. } => Self::Counter,
            Geometry::Redact { .. } => Self::Pixelate,
            Geometry::Image { .. } | Geometry::Background { .. } | Geometry::Crop { .. } => {
                return None;
            }
        })
    }
}

/// `spec/05` §4.4: "corner radius option `r = 2 x size` when enabled".
fn corner_radius(rounded: bool, style: &Style) -> f64 {
    if rounded { RADIUS_PER_SIZE * style.stroke_width() } else { 0.0 }
}

/// One control in `spec/05` §2's options row.
///
/// §2's table is a per-tool list of controls, so this is that table as data and the widget
/// builds whatever the list says. The alternative -- a widget that matches on the tool and
/// appends controls -- puts the table in a `match` arm where it cannot be checked against
/// the spec, and §2's rows are [V], verified against screenshots, so getting one wrong is
/// getting a verified fact wrong.
///
/// The order within a tool's list is the order §2 writes it, which is also left-to-right
/// in the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Control {
    /// The palette button. §2: "a menu button showing the current swatch plus a chevron".
    Color,
    /// The same button, with the alpha slider reachable: §2 gives the filled rectangle
    /// "colour (**with opacity**)", and §4.4 explains why it is called out -- "palette
    /// colours default to 100 %; useful at 30-50 %".
    ColorWithAlpha,
    /// Levels 1-6. §2: not a slider -- "a menu button that opens a vertical popover of six
    /// diagonal stroke previews".
    Size,
    Shadow,
    ArrowStyle,
    /// The tip's shape ([`ArrowHead`]). Not in §2's table; asked for on hardware.
    ArrowHead,
    TextStyle,
    /// §4.5's thirteen presets over a continuous value ([`crate::style::FONT_SIZES`]).
    FontSize,
    /// How a text's lines line up with each other ([`TextAlign`]). The model had it from
    /// the start and nothing could set it until D167.
    TextAlign,
    /// §2's "corner radius toggle", stored as the radius itself.
    CornerRadius,
    /// §4.6's "smoothing on/off".
    Smoothing,
    /// §4.7's "smart mode on/off". The behaviour is `ANN-24`, Tier 2, and arrives in M6;
    /// the control is in §2's row now.
    SmartMode,
    SpotlightShape,
    /// §4.8's opacity, and the one real slider in the editor: "This is the one place a real
    /// slider appears; colour and size both use popovers instead. Follow that split:
    /// sliders for continuous perceptual values, popovers for discrete choices."
    SpotlightOpacity,
    /// §4.9's menu: numbering system, starting number, and size.
    CounterSettings,
    RedactStyle,
    /// §4.10's intensity, "a slider in the toolbar, alongside the style menu".
    RedactIntensity,
    CropAspect,
    CropSnapping,
    /// §4.11's "expand-canvas colour", for the area a crop adds beyond the image (D52).
    CropExpandColor,
}

impl Tool {
    /// `spec/05` §2's Options row column, verbatim.
    #[must_use]
    pub const fn controls(self) -> &'static [Control] {
        match self {
            // §2: "Select | V (Esc also deselects) | —"
            Self::Select => &[],
            Self::Arrow => &[
                Control::Color,
                Control::Size,
                Control::ArrowStyle,
                Control::ArrowHead,
                Control::Shadow,
            ],
            Self::Line => &[Control::Color, Control::Size, Control::Shadow],
            Self::Rect => {
                &[Control::Color, Control::Size, Control::CornerRadius, Control::Shadow]
            }
            // No size and no shadow: a fill has no stroke to widen, and §2's row for this
            // tool lists neither.
            Self::FilledRect => &[Control::ColorWithAlpha, Control::CornerRadius],
            Self::Ellipse => &[Control::Color, Control::Size, Control::Shadow],
            Self::Text => {
                &[Control::Color, Control::FontSize, Control::TextStyle, Control::TextAlign]
            }
            Self::Pencil => &[Control::Color, Control::Size, Control::Smoothing],
            Self::Highlighter => &[Control::Color, Control::Size, Control::SmartMode],
            // No colour: the spotlight paints a scrim, not a stroke.
            Self::Spotlight => &[Control::SpotlightShape, Control::SpotlightOpacity],
            Self::Counter => &[Control::Color, Control::CounterSettings],
            Self::Pixelate => &[Control::RedactStyle, Control::RedactIntensity],
            Self::Crop => {
                &[Control::CropAspect, Control::CropSnapping, Control::CropExpandColor]
            }
        }
    }
}

impl Control {
    /// Writes this **one** control's value from `settings` onto `object`. Answers whether
    /// anything changed.
    ///
    /// One control and not the whole of `settings`, because the row sits over a
    /// *selection* and a selection can be mixed: recolouring a red level-2 arrow and a
    /// blue level-5 one together must leave both weights alone. The row derives its
    /// settings from the first selected object, so writing all of them back would make
    /// the second arrow a copy of the first without anyone having asked. The first
    /// version of the row did exactly that -- `now.style = settings.style` -- and a
    /// colour change on a selected box quietly reset its stroke to the tool's.
    ///
    /// Controls a kind does not have are ignored, so applying a font size to an ellipse
    /// answers `false` rather than doing something inventive.
    pub fn apply(self, settings: &ToolSettings, object: &mut Object) -> bool {
        let before = object.clone();
        match self {
            Self::Color | Self::ColorWithAlpha => {
                object.style.color = settings.style.color;
                // `spec/05` §4.7: a highlighter keeps its wash whatever the palette says.
                if let Geometry::Path { highlighter: true, .. } = object.geometry {
                    object.style.color.a = HIGHLIGHTER_ALPHA;
                }
            }
            Self::Size => {
                object.style.size = settings.style.size;
                // A snapped highlighter's width is the line it covers, not `3 x size`
                // (`spec/05` §4.7) -- so the size control would do nothing to one, which
                // is worse than a control that undoes the snap. Asking for a weight is
                // asking for that weight.
                if let Geometry::Path { highlighter: true, band, .. } = &mut object.geometry {
                    *band = None;
                }
                // §4.4's radius is `2 x size`, so a rounded corner follows the stroke.
                let rounded = corner_radius(true, &object.style);
                if let Geometry::Rect { radius, .. } = &mut object.geometry
                    && *radius > 0.0
                {
                    *radius = rounded;
                }
            }
            Self::Shadow => object.style.shadow = settings.style.shadow,
            Self::ArrowHead => {
                if let Geometry::Arrow { head, .. } = &mut object.geometry {
                    *head = settings.head;
                }
            }
            Self::ArrowStyle => {
                if let Geometry::Arrow { start, end, ctrl, style, .. } = &mut object.geometry {
                    *style = settings.arrow;
                    // A straight style has no control point. A curved one that had none
                    // gets the default bow; one that had one keeps the curve the user
                    // shaped, because switching Curved to Double is a change of heads and
                    // not a request to flatten the arc.
                    *ctrl = match (*ctrl, bow(*start, *end, settings.arrow)) {
                        (_, None) => None,
                        (Some(kept), Some(_)) => Some(kept),
                        (None, Some(default)) => Some(default),
                    };
                }
            }
            Self::TextStyle => {
                if let Geometry::Text { style, .. } = &mut object.geometry {
                    *style = settings.text;
                }
            }
            Self::FontSize => {
                if let Geometry::Text { font_size, .. } = &mut object.geometry {
                    *font_size = settings.font_size;
                }
            }
            Self::TextAlign => {
                if let Geometry::Text { align, .. } = &mut object.geometry {
                    *align = settings.align;
                }
            }
            Self::CornerRadius => {
                let radius = corner_radius(settings.rounded_corners, &object.style);
                if let Geometry::Rect { radius: r, .. } = &mut object.geometry {
                    *r = radius;
                }
            }
            Self::Smoothing => {
                if let Geometry::Path { smoothing, .. } = &mut object.geometry {
                    *smoothing = settings.smoothing;
                }
            }
            Self::SpotlightShape => {
                if let Geometry::Spotlight { shape, .. } = &mut object.geometry {
                    *shape = settings.spotlight;
                }
            }
            Self::SpotlightOpacity => {
                if let Geometry::Spotlight { opacity, .. } = &mut object.geometry {
                    *opacity = settings.spotlight_opacity;
                }
            }
            Self::CounterSettings => {
                if let Geometry::Counter { style, radius, .. } = &mut object.geometry {
                    *style = settings.counter;
                    *radius = settings.counter_radius;
                }
            }
            Self::RedactStyle => {
                if let Geometry::Redact { style, .. } = &mut object.geometry {
                    *style = settings.redact;
                }
            }
            Self::RedactIntensity => {
                if let Geometry::Redact { intensity, .. } = &mut object.geometry {
                    *intensity = settings.redact_intensity;
                }
            }
            // §4.7's smart mode is a gesture-time behaviour (`ANN-24`), and the crop
            // controls act on the canvas rect, never on an object.
            Self::SmartMode | Self::CropAspect | Self::CropSnapping | Self::CropExpandColor => {}
        }
        *object != before
    }
}

/// What `spec/05` §2's options row edits, as one value a tool can be handed.
///
/// One set rather than one per tool, for now. §2 ends with "the last colour and size per
/// tool persist (`markupLastColor`, `markupLastWidth`)", which makes this a map keyed by
/// tool -- but that is a *persistence* rule and it belongs with the options row and the
/// GSettings keys behind it, which is the next task on M3's list. Splitting it later is
/// one field becoming a lookup; guessing the shape now would be a lookup nobody reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolSettings {
    pub style: Style,
    pub arrow: ArrowStyle,
    /// The arrow tip's shape, chosen independently of the style.
    pub head: ArrowHead,
    pub text: TextStyle,
    pub font_size: f64,
    /// How a new text's lines line up, and what the row sets on a selected one.
    pub align: TextAlign,
    pub spotlight: SpotlightShape,
    pub spotlight_opacity: f64,
    pub counter: CounterStyle,
    /// `spec/05` §4.9's Size submenu: 11, 14, 20, 24, 31, 45 pt. Stored as the badge's
    /// radius, which is what the geometry holds.
    pub counter_radius: f64,
    pub redact: RedactStyle,
    pub redact_intensity: f64,
    /// `spec/05` §4.6's "smoothing on/off".
    pub smoothing: bool,
    /// `spec/05` §2's "corner radius toggle" for the rectangle tools.
    pub rounded_corners: bool,
    /// `spec/05` §4.7's smart highlighter: whether a stroke snaps to the line of text
    /// under it. The snap itself is `crate::highlight`, asked by the editor because it is
    /// the only side that has the base image's pixels.
    pub smart_highlighter: bool,
}

impl ToolSettings {
    /// What `spec/05` §2's row shows for an object that already exists: the object's own
    /// values wherever its kind has them, and `base` -- the tool's memory -- for the rest.
    ///
    /// The inverse of [`Control::apply`], and the two are tested against each other: a
    /// value read off an object and written straight back must change nothing.
    #[must_use]
    pub fn from_object(object: &Object, base: Self) -> Self {
        let mut settings = Self { style: object.style, ..base };
        match &object.geometry {
            Geometry::Arrow { style, head, .. } => {
                settings.arrow = *style;
                settings.head = *head;
            }
            Geometry::Rect { radius, .. } => settings.rounded_corners = *radius > 0.0,
            Geometry::Text { style, font_size, align, .. } => {
                settings.text = *style;
                settings.font_size = *font_size;
                settings.align = *align;
            }
            Geometry::Path { smoothing, .. } => settings.smoothing = *smoothing,
            Geometry::Spotlight { shape, opacity, .. } => {
                settings.spotlight = *shape;
                settings.spotlight_opacity = *opacity;
            }
            Geometry::Counter { style, radius, .. } => {
                settings.counter = *style;
                settings.counter_radius = *radius;
            }
            Geometry::Redact { style, intensity, .. } => {
                settings.redact = *style;
                settings.redact_intensity = *intensity;
            }
            Geometry::Line { .. }
            | Geometry::Ellipse { .. }
            | Geometry::Image { .. }
            | Geometry::Background { .. }
            | Geometry::Crop { .. } => {}
        }
        settings
    }
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            style: Style::default(),
            arrow: ArrowStyle::default(),
            head: ArrowHead::default(),
            text: TextStyle::default(),
            // The middle of §4.5's thirteen presets.
            font_size: 24.0,
            align: TextAlign::Start,
            spotlight: SpotlightShape::default(),
            spotlight_opacity: 0.5,
            counter: CounterStyle::default(),
            // The third of §4.9's six sizes, as a radius.
            counter_radius: 20.0,
            // `spec/05` §4.10: "Blur (secure) ... the style selected by default".
            redact: RedactStyle::SecureBlur,
            redact_intensity: 0.5,
            smoothing: true,
            rounded_corners: false,
            // On, because §4.7 is what the highlighter is *for* -- a marker that follows
            // the line rather than the hand -- and because it degrades to the freehand
            // stroke wherever there is no line to find. Ctrl turns it off for one stroke.
            smart_highlighter: true,
        }
    }
}

/// Which modifiers were down, at the moment they mattered.
///
/// Read at each update rather than at the gesture's start: `spec/05` §4.4 lets Shift make
/// a square, and a user who starts dragging and *then* presses Shift expects the shape to
/// square up under them. A snapshot taken at drag-begin cannot do that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

impl Modifiers {
    #[must_use]
    pub const fn none() -> Self {
        Self { shift: false, alt: false, ctrl: false }
    }

    #[must_use]
    pub const fn shift() -> Self {
        Self { shift: true, alt: false, ctrl: false }
    }

    #[must_use]
    pub const fn alt() -> Self {
        Self { shift: false, alt: true, ctrl: false }
    }
}

/// A gesture in progress: the object that does not exist yet.
///
/// Kept as the raw points plus the settings, and asked for its [`Geometry`] on demand,
/// rather than mutating an `Object` as the pointer moves. Two reasons, and the second is
/// the one that matters. A drag can cross the minimum length in both directions -- out
/// past eight units and back to three -- so "has this become an object yet" has to be a
/// question about the current points and not a latch. And the answer is needed twice per
/// frame by two different callers, the preview and the commit, which must agree exactly:
/// `spec/05` §6's "preview == export by construction" starts here, at the point where the
/// shape the user is watching and the shape that gets stored are the same function of the
/// same numbers.
#[derive(Debug, Clone)]
pub struct Draft {
    tool: Tool,
    settings: ToolSettings,
    start: Point,
    current: Point,
    /// Freehand only, and already resampled: a motion event that lands within
    /// [`RESAMPLE_SPACING`] of the last kept point is dropped as it arrives rather than
    /// stored and thinned later, so a slow careful stroke does not accumulate ten
    /// thousand points before anyone looks at it.
    points: Vec<Point>,
    modifiers: Modifiers,
}

impl Draft {
    /// Starts a gesture, or answers `None` for a tool that does not create objects.
    #[must_use]
    pub fn begin(tool: Tool, settings: ToolSettings, at: Point, modifiers: Modifiers) -> Option<Self> {
        if !tool.creates() {
            return None;
        }
        Some(Self {
            tool,
            settings,
            start: at,
            current: at,
            points: if tool.is_freehand() { vec![at] } else { Vec::new() },
            modifiers,
        })
    }

    #[must_use]
    pub const fn tool(&self) -> Tool {
        self.tool
    }

    /// Where the gesture started.
    ///
    /// The editor needs it to turn a `GtkGestureDrag` offset back into a point: the
    /// gesture reports how far the pointer has moved, not where it is, and applying the
    /// delta to the draft's own origin is what keeps the two from drifting apart.
    #[must_use]
    pub const fn start(&self) -> Point {
        self.start
    }

    /// What was held the last time the gesture was fed a point.
    ///
    /// `spec/05` §4.7's "Ctrl held disables snapping" is asked of a *drag in progress*, by
    /// the preview as well as by the commit, and the two have to agree -- so the answer
    /// comes from the draft rather than from whatever the keyboard says at the moment each
    /// one happens to look.
    #[must_use]
    pub const fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// Feeds the gesture a new pointer position.
    pub fn extend(&mut self, to: Point, modifiers: Modifiers) {
        self.current = to;
        self.modifiers = modifiers;
        if !self.tool.is_freehand() {
            return;
        }
        // `spec/05` §4.6's Shift: "constrains to a straight line". A constrained stroke
        // keeps its first point and follows the pointer with a second, so releasing Shift
        // mid-stroke resumes freehand from where the line ended rather than from the
        // origin.
        if modifiers.shift {
            self.points.truncate(1);
            self.points.push(to);
            return;
        }
        let far_enough = self
            .points
            .last()
            .is_none_or(|last| last.distance_to(to) >= RESAMPLE_SPACING);
        if far_enough {
            self.points.push(to);
        }
    }

    /// How far the gesture has travelled, on the diagonal.
    #[must_use]
    pub fn length(&self) -> f64 {
        self.start.distance_to(self.current)
    }

    /// Whether this gesture has become something worth keeping.
    #[must_use]
    pub fn is_committable(&self) -> bool {
        if self.tool.placed_by_click() {
            return true;
        }
        if self.tool.is_freehand() {
            // A path needs two distinct points to be a stroke; the minimum drag would
            // reject a deliberate dot, which a pencil is allowed to draw.
            return self.points.len() >= 2;
        }
        self.length() >= MIN_DRAG
    }

    /// The geometry this gesture describes right now, or `None` if it is still too small.
    #[must_use]
    pub fn geometry(&self, next_number: u32) -> Option<Geometry> {
        if !self.is_committable() {
            return None;
        }
        let end = self.constrained_end();
        let bounds = self.dragged_bounds();
        Some(match self.tool {
            Tool::Arrow => Geometry::Arrow {
                start: self.start,
                end,
                ctrl: bow(self.start, end, self.settings.arrow),
                style: self.settings.arrow,
                head: self.settings.head,
            },
            Tool::Line => Geometry::Line { start: self.start, end },
            // The radius follows the toggle and nothing else. The filled rectangle used to
            // be rounded unconditionally, which made §2's "corner radius" control on its
            // row a switch wired to nothing.
            Tool::Rect | Tool::FilledRect => Geometry::Rect {
                bounds,
                filled: matches!(self.tool, Tool::FilledRect),
                radius: corner_radius(self.settings.rounded_corners, &self.settings.style),
            },
            Tool::Ellipse => Geometry::Ellipse { bounds },
            // A click places an auto-width box at the pointer (`spec/05` §4.5); a *drag*
            // draws out a box of a chosen width, which wraps. The click is what §4.5
            // describes and the drag is what every editor with a text tool does with one,
            // and the difference is `MIN_DRAG`, the same threshold as every other tool.
            Tool::Text => {
                let (pos, width) = self.text_box();
                Geometry::Text {
                    pos,
                    text: String::new(),
                    style: self.settings.text,
                    font_size: self.settings.font_size,
                    align: self.settings.align,
                    width,
                }
            }
            Tool::Pencil => Geometry::Path {
                points: self.points.clone(),
                smoothing: self.settings.smoothing,
                highlighter: false,
                band: None,
            },
            // The freehand stroke, always. `spec/05` §4.7's snap needs the base image's
            // pixels, which this crate does not have and should not: the editor asks
            // `highlight::snap` and replaces the geometry before it commits.
            Tool::Highlighter => Geometry::Path {
                points: self.points.clone(),
                smoothing: self.settings.smoothing,
                highlighter: true,
                band: None,
            },
            Tool::Spotlight => Geometry::Spotlight {
                shape: self.settings.spotlight,
                bounds,
                opacity: self.settings.spotlight_opacity,
            },
            Tool::Counter => Geometry::Counter {
                center: self.start,
                number: next_number,
                style: self.settings.counter,
                radius: self.settings.counter_radius,
            },
            Tool::Pixelate => Geometry::Redact {
                bounds,
                style: self.settings.redact,
                intensity: self.settings.redact_intensity,
                // Not random. `spec/05` §11 item 6 asks that the same region pixelated
                // twice produce *different* blocks, which needs the seed to differ per
                // object -- and §5.1 stores it so a reopened project re-rasterises to the
                // same pixels. Deriving it from the gesture gives both: two drags are
                // never bit-identical, and the value is fixed the moment it is stored.
                seed: seed_from(self.start, self.current),
            },
            Tool::Select | Tool::Crop => return None,
        })
    }

    /// The object this gesture would commit, at `z` and stamped `created`.
    #[must_use]
    pub fn object(&self, z: i32, created: u64, next_number: u32) -> Option<Object> {
        let geometry = self.geometry(next_number)?;
        let mut style = self.settings.style;
        // `spec/05` §4.7: the highlighter's colour is used at 55 % whatever the palette
        // says. Applied to the stored object rather than at render time, because an
        // exported highlighter that reads as opaque would be a preview/export difference
        // -- and because a user who later recolours the stroke should keep the wash.
        if matches!(self.tool, Tool::Highlighter) {
            style.color.a = HIGHLIGHTER_ALPHA;
        }
        Some(Object::new(z, created, style, geometry))
    }

    /// Where a text gesture puts its box, and how wide: the press point and no width for
    /// a click, the dragged rect's corner and width for a drag.
    #[must_use]
    pub fn text_box(&self) -> (Point, Option<f64>) {
        if self.length() < MIN_DRAG {
            return (self.start, None);
        }
        let bounds = self.dragged_bounds().normalised();
        (Point::new(bounds.x, bounds.y), Some(bounds.width.max(MIN_DRAG)))
    }

    /// The end point after `spec/05` §4.2's Shift snapping.
    fn constrained_end(&self) -> Point {
        if self.modifiers.shift { snap_to_45(self.start, self.current) } else { self.current }
    }

    /// The bounds of an area drag, after §4.4's Shift and Alt.
    fn dragged_bounds(&self) -> Bounds {
        let end = if self.modifiers.shift {
            square_corner(self.start, self.current)
        } else {
            self.current
        };
        if self.modifiers.alt {
            // "Alt draws from the centre": the anchor becomes the middle, so the drag
            // extends the shape in both directions at once.
            let (dx, dy) = (end.x - self.start.x, end.y - self.start.y);
            Bounds::from_corners(
                Point::new(self.start.x - dx, self.start.y - dy),
                end,
            )
        } else {
            Bounds::from_corners(self.start, end)
        }
    }
}

/// `spec/05` §4.2: "Shift snaps the angle to 45° steps."
///
/// The snapped point keeps the drag's *length*, so the arrowhead stays under the pointer
/// rather than jumping to a projection of it -- a projection shortens the arrow whenever
/// the pointer is off-axis, which reads as the tool fighting the drag.
#[must_use]
pub fn snap_to_45(start: Point, end: Point) -> Point {
    let (dx, dy) = (end.x - start.x, end.y - start.y);
    let length = (dx * dx + dy * dy).sqrt();
    if length == 0.0 {
        return end;
    }
    let step = std::f64::consts::FRAC_PI_4;
    let angle = (dy.atan2(dx) / step).round() * step;
    Point::new(start.x + length * angle.cos(), start.y + length * angle.sin())
}

/// `spec/05` §4.4: "Shift makes a square/circle."
///
/// The larger of the two extents wins, so the shape always contains the drag rather than
/// shrinking to its smaller axis -- the same reason [`snap_to_45`] keeps the length.
#[must_use]
pub fn square_corner(start: Point, end: Point) -> Point {
    let (dx, dy) = (end.x - start.x, end.y - start.y);
    let side = dx.abs().max(dy.abs());
    Point::new(
        start.x + side * if dx < 0.0 { -1.0 } else { 1.0 },
        start.y + side * if dy < 0.0 { -1.0 } else { 1.0 },
    )
}

/// The control point for `spec/05` §4.2's two curved styles, or `None` for the straight ones.
///
/// Both Curved *and* Double get one: §4.2 corrects itself on exactly this point --
/// "**Double**: **also curved** ... The spec previously described this as a straight shaft
/// with two heads, which is wrong."
#[must_use]
pub fn bow(start: Point, end: Point, style: ArrowStyle) -> Option<Point> {
    if !matches!(style, ArrowStyle::Curved | ArrowStyle::DoubleHeaded) {
        return None;
    }
    let (dx, dy) = (end.x - start.x, end.y - start.y);
    let mid = Point::new((start.x + end.x) / 2.0, (start.y + end.y) / 2.0);
    // Perpendicular to the shaft, scaled by the length -- so the bow is proportional and
    // a long arrow does not look straight.
    Some(Point::new(mid.x - dy * CURVE_BOW, mid.y + dx * CURVE_BOW))
}

/// A per-object redaction seed derived from the gesture that made it.
fn seed_from(start: Point, end: Point) -> u64 {
    // The bit patterns of the four coordinates, mixed. Not cryptographic and not meant to
    // be: it has to differ between two drags over the same region, which sub-pixel
    // coordinates guarantee on their own.
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for value in [start.x, start.y, end.x, end.y] {
        hash ^= value.to_bits();
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "a failing assertion should panic")]
mod tests {
    use super::*;
    use crate::object::ObjectKind;

    fn drag(tool: Tool, from: (f64, f64), to: (f64, f64), modifiers: Modifiers) -> Option<Object> {
        let mut draft = Draft::begin(
            tool,
            ToolSettings::default(),
            Point::new(from.0, from.1),
            Modifiers::none(),
        )?;
        draft.extend(Point::new(to.0, to.1), modifiers);
        draft.object(0, 0, 1)
    }

    #[test]
    fn every_tool_has_a_distinct_key_and_icon() {
        let flattened: Vec<Tool> = Tool::CHUNKS.iter().flat_map(|c| c.iter().copied()).collect();
        assert_eq!(flattened, Tool::ALL.to_vec(), "the chunks are not ALL, in order, once each");
        let mut keys: Vec<_> = Tool::ALL.iter().map(|t| t.accelerator()).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "two tools share an accelerator");

        let mut icons: Vec<_> = Tool::ALL.iter().map(|t| t.icon()).collect();
        icons.sort_unstable();
        let before = icons.len();
        icons.dedup();
        assert_eq!(icons.len(), before, "two tools share an icon");
    }

    #[test]
    fn a_click_that_wobbles_leaves_nothing_behind() {
        // Three units of travel is a click, not a drag: `spec/05` §4.2's minimum.
        for tool in [Tool::Arrow, Tool::Line, Tool::Rect, Tool::Ellipse, Tool::Spotlight] {
            assert!(
                drag(tool, (100.0, 100.0), (102.0, 102.0), Modifiers::none()).is_none(),
                "{tool:?} made an object out of a click"
            );
        }
        // ... but the two placed tools are all click, and must still work.
        assert!(drag(Tool::Counter, (100.0, 100.0), (100.0, 100.0), Modifiers::none()).is_some());
    }

    /// A click places an auto-width text box; a drag draws one out with a width.
    #[test]
    fn a_text_click_has_no_width_and_a_text_drag_has_the_dragged_one() {
        let click = Draft::begin(Tool::Text, ToolSettings::default(), Point::new(10.0, 20.0), Modifiers::none())
            .expect("creates");
        assert_eq!(click.text_box(), (Point::new(10.0, 20.0), None));
        let mut dragged = click.clone();
        dragged.extend(Point::new(210.0, 60.0), Modifiers::none());
        assert_eq!(dragged.text_box(), (Point::new(10.0, 20.0), Some(200.0)));
        // Dragged up and to the left: the box's corner is where the pointer ended.
        let mut backwards = click;
        backwards.extend(Point::new(-90.0, 0.0), Modifiers::none());
        assert_eq!(backwards.text_box(), (Point::new(-90.0, 0.0), Some(100.0)));
    }

    /// §2's "corner radius" control on the filled rectangle's row used to be wired to
    /// nothing: the fill was rounded whatever the toggle said.
    #[test]
    fn a_filled_rectangle_is_rounded_only_when_asked() {
        let square = drag(Tool::FilledRect, (0.0, 0.0), (100.0, 100.0), Modifiers::none())
            .expect("committable");
        let Geometry::Rect { radius, filled: true, .. } = square.geometry else {
            panic!("not a filled rect")
        };
        assert_eq!(radius, 0.0);

        let settings = ToolSettings { rounded_corners: true, ..ToolSettings::default() };
        let mut draft = Draft::begin(Tool::FilledRect, settings, Point::new(0.0, 0.0), Modifiers::none())
            .expect("creates");
        draft.extend(Point::new(100.0, 100.0), Modifiers::none());
        let rounded = draft.object(0, 0, 1).expect("committable");
        let Geometry::Rect { radius, .. } = rounded.geometry else { panic!("not a rect") };
        assert_eq!(radius, RADIUS_PER_SIZE * settings.style.stroke_width());
    }

    /// D56: every drawn kind names the tool whose row edits it; the three kinds no tool
    /// draws name none.
    #[test]
    fn every_drawn_kind_has_a_tool_and_the_canvas_kinds_have_none() {
        for tool in Tool::ALL {
            if !tool.creates() || tool == Tool::Text {
                continue;
            }
            let object = if tool.placed_by_click() {
                drag(tool, (50.0, 50.0), (50.0, 50.0), Modifiers::none())
            } else {
                drag(tool, (0.0, 0.0), (100.0, 100.0), Modifiers::none())
            }
            .expect("committable");
            assert_eq!(Tool::for_object(&object), Some(tool), "{tool:?}");
        }
        let background = Object::new(0, 0, Style::default(), Geometry::Background {
            params: crate::background::BackgroundParams::default(),
            source: None,
        });
        assert_eq!(Tool::for_object(&background), None);
    }

    /// The row reads an object's own values and writes one control back without touching
    /// the rest -- the failure the first row had was writing the *tool's* size along
    /// with the colour.
    #[test]
    fn one_control_changes_one_thing_and_reading_then_writing_changes_nothing() {
        let mut arrow = drag(Tool::Arrow, (0.0, 0.0), (100.0, 0.0), Modifiers::none())
            .expect("committable");
        arrow.style.size = 5;
        arrow.style.color = crate::style::Rgba::new(0.0, 0.0, 1.0, 1.0);

        let read = ToolSettings::from_object(&arrow, ToolSettings::default());
        assert_eq!(read.style.size, 5);
        assert_eq!(read.arrow, ArrowStyle::Standard);
        // The font size is not an arrow's to have, so the base's is kept.
        assert_eq!(read.font_size, ToolSettings::default().font_size);

        let mut copy = arrow.clone();
        for control in Tool::Arrow.controls() {
            assert!(!control.apply(&read, &mut copy), "{control:?} changed a value it had just read");
        }

        let mut recoloured = ToolSettings::default();
        recoloured.style.color = crate::style::Rgba::new(1.0, 0.0, 0.0, 1.0);
        recoloured.style.size = 1;
        assert!(Control::Color.apply(&recoloured, &mut copy));
        assert_eq!(copy.style.color, recoloured.style.color);
        assert_eq!(copy.style.size, 5, "a colour change is not a size change");
    }

    /// A text's alignment is the row's to set (D167): a new text takes the tool's, the row
    /// reads a selected text's back, and changing it leaves the style and the size alone.
    #[test]
    fn a_text_is_drawn_with_the_tools_alignment_and_the_row_changes_only_that() {
        assert!(Tool::Text.controls().contains(&Control::TextAlign));
        let settings = ToolSettings { align: TextAlign::Center, ..ToolSettings::default() };
        let mut draft = Draft::begin(Tool::Text, settings, Point::new(0.0, 0.0), Modifiers::none())
            .expect("creates");
        draft.extend(Point::new(200.0, 40.0), Modifiers::none());
        let mut text = draft.object(0, 0, 1).expect("committable");
        assert!(matches!(text.geometry, Geometry::Text { align: TextAlign::Center, .. }));

        let read = ToolSettings::from_object(&text, ToolSettings::default());
        assert_eq!(read.align, TextAlign::Center);
        for control in Tool::Text.controls() {
            let changed = control.apply(&read, &mut text);
            assert!(!changed, "{control:?} changed a value it had just read");
        }

        let right = ToolSettings { align: TextAlign::End, ..read };
        assert!(Control::TextAlign.apply(&right, &mut text));
        let Geometry::Text { align, style, font_size, .. } = text.geometry else { panic!("text") };
        assert_eq!(align, TextAlign::End);
        assert_eq!((style, font_size), (read.text, read.font_size));

        // Nothing else has an alignment to set.
        let mut arrow = drag(Tool::Arrow, (0.0, 0.0), (100.0, 0.0), Modifiers::none())
            .expect("committable");
        assert!(!Control::TextAlign.apply(&right, &mut arrow));
    }

    /// The head is its own control: changing it leaves the style, and a document from
    /// before there was a head field reads as the triangle it was drawn with.
    #[test]
    fn the_arrowhead_is_a_separate_choice_with_the_triangle_as_its_default() {
        let mut arrow = drag(Tool::Arrow, (0.0, 0.0), (100.0, 0.0), Modifiers::none())
            .expect("committable");
        assert!(matches!(arrow.geometry, Geometry::Arrow { head: ArrowHead::Triangle, .. }));
        let settings = ToolSettings { head: ArrowHead::Dot, ..ToolSettings::default() };
        assert!(Control::ArrowHead.apply(&settings, &mut arrow));
        let Geometry::Arrow { head, style, .. } = arrow.geometry else { panic!("arrow") };
        assert_eq!(head, ArrowHead::Dot);
        assert_eq!(style, ArrowStyle::Standard);
        assert_eq!(ToolSettings::from_object(&arrow, ToolSettings::default()).head, ArrowHead::Dot);

        let legacy: Object = serde_json::from_str(
            r#"{"id":"01J80000000000000000000000","z":0,"created":0,"color":[1,0,0,1],"size":3,
                "shadow":false,"type":"arrow","geometry":{"start":{"x":0,"y":0},
                "end":{"x":10,"y":0},"style":"standard"}}"#,
        )
        .expect("a pre-head document still reads");
        assert!(matches!(legacy.geometry, Geometry::Arrow { head: ArrowHead::Triangle, .. }));
        assert!(Tool::Arrow.controls().contains(&Control::ArrowHead));
    }

    #[test]
    fn switching_arrow_styles_adds_drops_and_keeps_the_control_point() {
        let mut arrow = drag(Tool::Arrow, (0.0, 0.0), (100.0, 0.0), Modifiers::none())
            .expect("committable");
        let mut settings = ToolSettings { arrow: ArrowStyle::Curved, ..ToolSettings::default() };
        assert!(Control::ArrowStyle.apply(&settings, &mut arrow));
        let Geometry::Arrow { ctrl: Some(bowed), .. } = arrow.geometry else {
            panic!("a curved arrow has a control point")
        };

        // The user shapes the curve, then asks for two heads: the shape stays.
        if let Geometry::Arrow { ctrl, .. } = &mut arrow.geometry {
            *ctrl = Some(Point::new(50.0, -80.0));
        }
        settings.arrow = ArrowStyle::DoubleHeaded;
        assert!(Control::ArrowStyle.apply(&settings, &mut arrow));
        let Geometry::Arrow { ctrl: Some(kept), .. } = arrow.geometry else { panic!("kept") };
        assert_eq!(kept, Point::new(50.0, -80.0));
        assert_ne!(kept, bowed);

        settings.arrow = ArrowStyle::Standard;
        assert!(Control::ArrowStyle.apply(&settings, &mut arrow));
        assert!(matches!(arrow.geometry, Geometry::Arrow { ctrl: None, .. }));
    }

    #[test]
    fn a_rounded_corner_follows_the_stroke_it_is_measured_from() {
        let mut settings = ToolSettings { rounded_corners: true, ..ToolSettings::default() };
        let mut draft = Draft::begin(Tool::Rect, settings, Point::new(0.0, 0.0), Modifiers::none())
            .expect("creates");
        draft.extend(Point::new(100.0, 100.0), Modifiers::none());
        let mut rect = draft.object(0, 0, 1).expect("committable");

        settings.style.size = 6;
        assert!(Control::Size.apply(&settings, &mut rect));
        let Geometry::Rect { radius, .. } = rect.geometry else { panic!("rect") };
        assert_eq!(radius, RADIUS_PER_SIZE * crate::style::stroke_width(6));
        // And a font size means nothing to it.
        assert!(!Control::FontSize.apply(&settings, &mut rect));
    }

    #[test]
    fn shift_snaps_an_arrow_to_45_and_keeps_its_length() {
        let object = drag(Tool::Arrow, (0.0, 0.0), (100.0, 10.0), Modifiers::shift())
            .expect("a 100-unit drag is committable");
        let Geometry::Arrow { start, end, .. } = object.geometry else {
            panic!("not an arrow")
        };
        // Snapped to horizontal, and the length is the drag's, not its projection.
        let dragged = Point::new(0.0, 0.0).distance_to(Point::new(100.0, 10.0));
        assert!((end.y - start.y).abs() < 1e-9, "not snapped to the axis: {end:?}");
        assert!(
            (start.distance_to(end) - dragged).abs() < 1e-9,
            "snapping changed the length: {} vs {dragged}",
            start.distance_to(end)
        );
    }

    #[test]
    fn shift_makes_a_square_that_contains_the_drag() {
        let object = drag(Tool::Rect, (10.0, 10.0), (110.0, 40.0), Modifiers::shift())
            .expect("committable");
        let Geometry::Rect { bounds, .. } = object.geometry else { panic!("not a rect") };
        assert!((bounds.width - bounds.height).abs() < 1e-9, "not square: {bounds:?}");
        // The larger extent won, so the drag is inside the square rather than clipped by it.
        assert!(bounds.width >= 100.0, "the square shrank to the small axis: {bounds:?}");
    }

    #[test]
    fn a_square_dragged_up_and_left_stays_square() {
        // The sign handling is the part that is easy to get wrong, and a drag towards the
        // origin is the case that exposes it.
        let object = drag(Tool::Ellipse, (200.0, 200.0), (150.0, 20.0), Modifiers::shift())
            .expect("committable");
        let Geometry::Ellipse { bounds } = object.geometry else { panic!("not an ellipse") };
        assert!((bounds.width - bounds.height).abs() < 1e-9, "not a circle: {bounds:?}");
        assert!(bounds.x < 200.0 && bounds.y < 200.0, "grew the wrong way: {bounds:?}");
        assert!((bounds.x + bounds.width - 200.0).abs() < 1e-9, "anchor moved: {bounds:?}");
    }

    #[test]
    fn alt_draws_from_the_centre() {
        let object =
            drag(Tool::Rect, (100.0, 100.0), (150.0, 130.0), Modifiers::alt()).expect("committable");
        let Geometry::Rect { bounds, .. } = object.geometry else { panic!("not a rect") };
        let centre = bounds.center();
        assert!(
            (centre.x - 100.0).abs() < 1e-9 && (centre.y - 100.0).abs() < 1e-9,
            "the anchor was not the centre: {centre:?}"
        );
        assert!((bounds.width - 100.0).abs() < 1e-9, "half the width: {bounds:?}");
    }

    #[test]
    fn both_curved_styles_get_a_control_point() {
        // §4.2 corrects itself here: Double is an arc too. A straight Double shaft was
        // exactly the reading the spec calls wrong, and the render node had it.
        for style in [ArrowStyle::Curved, ArrowStyle::DoubleHeaded] {
            let ctrl = bow(Point::new(0.0, 0.0), Point::new(100.0, 0.0), style);
            let ctrl = ctrl.unwrap_or_else(|| panic!("{style:?} has no bow"));
            assert!(ctrl.y.abs() > 1.0, "{style:?} bows by nothing: {ctrl:?}");
        }
        for style in [ArrowStyle::Standard, ArrowStyle::Fancy] {
            assert!(
                bow(Point::new(0.0, 0.0), Point::new(100.0, 0.0), style).is_none(),
                "{style:?} should be straight"
            );
        }
    }

    #[test]
    fn a_freehand_stroke_is_resampled_as_it_arrives() {
        let mut draft = Draft::begin(
            Tool::Pencil,
            ToolSettings::default(),
            Point::new(0.0, 0.0),
            Modifiers::none(),
        )
        .expect("the pencil creates");
        // A hundred events along one unit of travel each: half are within the spacing.
        for i in 1..=100 {
            draft.extend(Point::new(f64::from(i), 0.0), Modifiers::none());
        }
        let object = draft.object(0, 0, 1).expect("committable");
        let Geometry::Path { points, .. } = object.geometry else { panic!("not a path") };
        assert!(points.len() < 60, "not resampled: {} points", points.len());
        assert!(points.len() > 40, "over-thinned: {} points", points.len());
        for pair in points.windows(2) {
            assert!(
                pair[0].distance_to(pair[1]) >= RESAMPLE_SPACING - 1e-9,
                "points closer than the spacing survived"
            );
        }
    }

    #[test]
    fn shift_turns_the_pencil_into_a_ruler_and_lets_go_again() {
        let mut draft = Draft::begin(
            Tool::Pencil,
            ToolSettings::default(),
            Point::new(0.0, 0.0),
            Modifiers::none(),
        )
        .expect("the pencil creates");
        for i in 1..=20 {
            draft.extend(Point::new(f64::from(i) * 3.0, f64::from(i)), Modifiers::none());
        }
        draft.extend(Point::new(100.0, 100.0), Modifiers::shift());
        let Some(Geometry::Path { points, .. }) = draft.geometry(1) else { panic!("not a path") };
        assert_eq!(points.len(), 2, "a constrained stroke is one line: {points:?}");

        // Releasing Shift resumes from the line's end, not from the origin.
        draft.extend(Point::new(120.0, 100.0), Modifiers::none());
        let Some(Geometry::Path { points, .. }) = draft.geometry(1) else { panic!("not a path") };
        assert_eq!(points.len(), 3, "freehand did not resume: {points:?}");
        assert_eq!(points[1], Point::new(100.0, 100.0), "the line's end was lost");
    }

    #[test]
    fn the_highlighter_stores_its_wash_rather_than_relying_on_the_renderer() {
        let object = {
            let mut draft = Draft::begin(
                Tool::Highlighter,
                ToolSettings::default(),
                Point::new(0.0, 0.0),
                Modifiers::none(),
            )
            .expect("creates");
            draft.extend(Point::new(80.0, 0.0), Modifiers::none());
            draft.object(0, 0, 1).expect("committable")
        };
        assert!(
            (object.style.color.a - HIGHLIGHTER_ALPHA).abs() < 1e-9,
            "alpha is {}",
            object.style.color.a
        );
        let Geometry::Path { highlighter, .. } = object.geometry else { panic!("not a path") };
        assert!(highlighter, "not marked as a highlighter");

        // The pencil is left alone, which is the other half of the claim.
        let pencil = drag(Tool::Pencil, (0.0, 0.0), (80.0, 0.0), Modifiers::none());
        let pencil = pencil.expect("committable");
        assert!((pencil.style.color.a - 1.0).abs() < 1e-9, "the pencil got washed out");
    }

    #[test]
    fn two_pixelations_of_the_same_region_get_different_seeds() {
        // `spec/05` §11 item 6, as far as this crate can state it.
        let a = drag(Tool::Pixelate, (10.0, 10.0), (110.0, 60.0), Modifiers::none())
            .expect("committable");
        let b = drag(Tool::Pixelate, (10.0, 10.0), (110.0, 60.25), Modifiers::none())
            .expect("committable");
        let (Geometry::Redact { seed: sa, .. }, Geometry::Redact { seed: sb, .. }) =
            (a.geometry, b.geometry)
        else {
            panic!("not redactions")
        };
        assert_ne!(sa, sb, "the same seed twice");
    }

    #[test]
    fn the_pixelate_default_is_the_irreversible_one() {
        // §4.10 puts the irreversible option first "and makes it the default", which is a
        // safety property rather than a preference.
        let object = drag(Tool::Pixelate, (0.0, 0.0), (100.0, 50.0), Modifiers::none())
            .expect("committable");
        let Geometry::Redact { style, .. } = object.geometry else { panic!("not a redaction") };
        assert_eq!(style, RedactStyle::SecureBlur);
        assert!(!style.preview_is_exact(), "a secure blur cannot be previewed exactly");
    }

    #[test]
    fn select_and_crop_do_not_draft() {
        for tool in [Tool::Select, Tool::Crop] {
            assert!(
                Draft::begin(tool, ToolSettings::default(), Point::new(0.0, 0.0), Modifiers::none())
                    .is_none(),
                "{tool:?} should not draft"
            );
        }
    }

    #[test]
    fn a_counter_takes_the_number_it_is_given() {
        let mut draft = Draft::begin(
            Tool::Counter,
            ToolSettings::default(),
            Point::new(50.0, 50.0),
            Modifiers::none(),
        )
        .expect("creates");
        let object = draft.object(0, 0, 7).expect("a click is enough");
        let Geometry::Counter { number, center, .. } = object.geometry else {
            panic!("not a counter")
        };
        assert_eq!(number, 7);
        assert_eq!(center, Point::new(50.0, 50.0));
        // And it does not move if the pointer drifts, because it is placed by its click.
        draft.extend(Point::new(60.0, 60.0), Modifiers::none());
        let Some(Geometry::Counter { center, .. }) = draft.geometry(7) else { panic!() };
        assert_eq!(center, Point::new(50.0, 50.0), "a placed badge followed the pointer");
    }

    #[test]
    fn a_filled_rectangle_is_filled_and_a_plain_one_is_not() {
        let filled = drag(Tool::FilledRect, (0.0, 0.0), (100.0, 50.0), Modifiers::none())
            .expect("committable");
        let plain =
            drag(Tool::Rect, (0.0, 0.0), (100.0, 50.0), Modifiers::none()).expect("committable");
        assert_eq!(plain.kind(), ObjectKind::Rect);
        let (Geometry::Rect { filled: f, .. }, Geometry::Rect { filled: p, radius, .. }) =
            (filled.geometry, plain.geometry)
        else {
            panic!("not rects")
        };
        assert!(f, "the filled tool made a stroke");
        assert!(!p, "the plain tool made a fill");
        assert!(radius.abs() < 1e-9, "corners are square unless the option is on");
    }

    #[test]
    fn every_creating_tool_produces_the_kind_it_is_named_after() {
        let expected = [
            (Tool::Arrow, ObjectKind::Arrow),
            (Tool::Line, ObjectKind::Line),
            (Tool::Rect, ObjectKind::Rect),
            (Tool::FilledRect, ObjectKind::Rect),
            (Tool::Ellipse, ObjectKind::Ellipse),
            (Tool::Text, ObjectKind::Text),
            (Tool::Pencil, ObjectKind::Path),
            (Tool::Highlighter, ObjectKind::Path),
            (Tool::Spotlight, ObjectKind::Spotlight),
            (Tool::Counter, ObjectKind::Counter),
            (Tool::Pixelate, ObjectKind::Redact),
        ];
        for (tool, kind) in expected {
            let object = drag(tool, (20.0, 20.0), (140.0, 90.0), Modifiers::none())
                .unwrap_or_else(|| panic!("{tool:?} produced nothing"));
            assert_eq!(object.kind(), kind, "{tool:?}");
        }
        // And the list above is every tool that creates, so a new one cannot be added
        // without being covered here.
        assert_eq!(
            expected.len(),
            Tool::ALL.iter().filter(|t| t.creates()).count(),
            "a creating tool is missing from this test"
        );
    }
}

#[cfg(test)]
mod options_row_tests {
    use super::*;
    use crate::style::{COUNTER_SIZES, FONT_SIZES, PALETTE, SIZE_LEVELS};

    /// `spec/05` §2's table, transcribed independently of `Tool::controls` so the two have
    /// to agree. Copying the implementation into a test proves nothing; this is the row
    /// read off the spec.
    #[test]
    fn every_tools_row_is_the_one_the_spec_lists() {
        use Control as C;
        let table: [(Tool, &[Control]); 13] = [
            (Tool::Select, &[]),
            // §2's four, plus the arrowhead menu the user asked for (D57).
            (Tool::Arrow, &[C::Color, C::Size, C::ArrowStyle, C::ArrowHead, C::Shadow]),
            (Tool::Line, &[C::Color, C::Size, C::Shadow]),
            (Tool::Rect, &[C::Color, C::Size, C::CornerRadius, C::Shadow]),
            (Tool::FilledRect, &[C::ColorWithAlpha, C::CornerRadius]),
            (Tool::Ellipse, &[C::Color, C::Size, C::Shadow]),
            // §2's three, plus the alignment the model always had (D167).
            (Tool::Text, &[C::Color, C::FontSize, C::TextStyle, C::TextAlign]),
            (Tool::Pencil, &[C::Color, C::Size, C::Smoothing]),
            (Tool::Highlighter, &[C::Color, C::Size, C::SmartMode]),
            (Tool::Spotlight, &[C::SpotlightShape, C::SpotlightOpacity]),
            (Tool::Counter, &[C::Color, C::CounterSettings]),
            (Tool::Pixelate, &[C::RedactStyle, C::RedactIntensity]),
            (Tool::Crop, &[C::CropAspect, C::CropSnapping, C::CropExpandColor]),
        ];
        for (tool, expected) in table {
            assert_eq!(tool.controls(), expected, "{tool:?}");
        }
        assert_eq!(table.len(), Tool::ALL.len(), "a tool is missing from the table");
    }

    #[test]
    fn only_the_filled_rectangle_offers_opacity() {
        // §2 writes "colour (with opacity)" for exactly one row, and §4.4 says why it is
        // called out: palette colours are 100 % and a fill is "useful at 30-50 %".
        let with: Vec<Tool> = Tool::ALL
            .into_iter()
            .filter(|t| t.controls().contains(&Control::ColorWithAlpha))
            .collect();
        assert_eq!(with, [Tool::FilledRect]);
    }

    #[test]
    fn the_only_sliders_are_the_two_continuous_values() {
        // §4.8: "This is the one place a real slider appears; colour and size both use
        // popovers instead. Follow that split: sliders for continuous perceptual values,
        // popovers for discrete choices." §4.10's intensity is the other continuous one.
        let sliders: Vec<Control> = Tool::ALL
            .into_iter()
            .flat_map(|t| t.controls().iter().copied())
            .filter(|c| matches!(c, Control::SpotlightOpacity | Control::RedactIntensity))
            .collect();
        assert_eq!(sliders.len(), 2, "a third slider appeared: {sliders:?}");
    }

    #[test]
    fn select_is_the_only_tool_with_no_options() {
        let bare: Vec<Tool> =
            Tool::ALL.into_iter().filter(|t| t.controls().is_empty()).collect();
        assert_eq!(bare, [Tool::Select]);
    }

    #[test]
    fn the_menus_are_in_the_order_the_screenshots_verified() {
        // Each of these orders is [P→V] in the spec and differs from the declaration
        // order of its enum, which is the whole reason `ALL` exists separately.
        assert_eq!(
            ArrowStyle::ALL.map(ArrowStyle::label),
            ["Standard", "Fancy", "Curved", "Double"]
        );
        assert_eq!(
            TextStyle::ALL.map(TextStyle::label),
            ["Standard", "Rounded", "Outlined", "Mono", "Box", "Mono Box", "Rounded Box"]
        );
        assert_eq!(
            RedactStyle::ALL.map(RedactStyle::label),
            ["Pixelate", "Blur (secure)", "Blur (smooth)", "Black Out"]
        );
        // And the first row is deliberately not the default one: §4.10 marks
        // "Blur (secure) … the style selected by default" with a [V].
        assert_eq!(RedactStyle::ALL[0], RedactStyle::Pixelate);
        assert_eq!(ToolSettings::default().redact, RedactStyle::SecureBlur);
    }

    #[test]
    fn the_ladders_are_the_lengths_the_spec_gives() {
        assert_eq!(PALETTE.len(), 10, "§2's ten-colour palette");
        assert_eq!(PALETTE[0].0, "Black");
        assert_eq!(PALETTE[9].0, "White");
        assert_eq!(SIZE_LEVELS, 6, "§2's six stroke previews, bound to digits 1-6");
        assert_eq!(FONT_SIZES.len(), 13, "§4.5's thirteen presets");
        assert_eq!(FONT_SIZES[0], 10.0);
        assert_eq!(FONT_SIZES[12], 288.0);
        assert_eq!(COUNTER_SIZES.len(), 6, "§4.9's six sizes");
        // The default font size is one of the presets rather than a number between them.
        assert!(FONT_SIZES.contains(&ToolSettings::default().font_size));
    }

    #[test]
    fn every_control_is_reachable_from_some_tool() {
        // A control nothing shows is a control nobody can set, and the widget would have
        // no arm for it -- which is a silent hole rather than a compile error.
        use Control as C;
        let all = [
            C::Color, C::ColorWithAlpha, C::Size, C::Shadow, C::ArrowStyle, C::TextStyle,
            C::FontSize, C::CornerRadius, C::Smoothing, C::SmartMode, C::SpotlightShape,
            C::SpotlightOpacity, C::CounterSettings, C::RedactStyle, C::RedactIntensity,
            C::CropAspect, C::CropSnapping, C::CropExpandColor,
        ];
        for control in all {
            assert!(
                Tool::ALL.iter().any(|t| t.controls().contains(&control)),
                "{control:?} is shown by no tool"
            );
        }
    }
}
