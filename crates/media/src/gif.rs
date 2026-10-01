// SPDX-License-Identifier: GPL-3.0-or-later
//! The plan for one GIF recording, computed before anything touches the bus.
//!
//! `spec/06` §4.1 and `docs/spikes/05`: the stream is the **monitor** the selection sits
//! on (`RecordMonitor`, physical pixels), never `RecordArea`, which streams at logical
//! resolution and throws a third of the pixels away on a 1.25 panel. The selection is
//! therefore a *crop* of that stream, in physical pixels, and the GIF's size follows from
//! the crop and `gif-max-width`. Everything the graph is asked for is decided here, from
//! the selection, the monitor and the settings, and tested against the same rounding the
//! compositor uses (D23).

use octosnap_core::geometry::Rect;
use serde::{Deserialize, Serialize};

/// The most a GIF can play at, in frames a second.
///
/// A GIF stores each frame's delay in hundredths of a second, and browsers treat a delay
/// of one hundredth as a tenth, so gifski writes nothing shorter than two: `write_frames`
/// clamps every delay to `2..=30000`. Asking it for sixty frames a second does not drop
/// frames -- "it's too late to drop frames now", its own comment says -- it *stretches
/// time*: a ten-second recording of six hundred frames plays for twelve seconds. So the
/// graph is never asked for more than this, whatever the setting or the request said
/// (D101).
pub const GIF_FPS_MAX: u32 = 50;

/// What "the screen's rate" becomes when the bridge could not learn the monitor's. Thirty
/// rather than the default fifteen, because the user asked for the screen and a screen
/// is at least sixty -- and thirty is what a sixty-hertz screen gives ([`GifSettings::fps_for`]).
/// The toolbar's row says the same thirty when the extension cannot see the rate (D117).
pub const SCREEN_RATE_FALLBACK: u32 = 30;

/// `spec/08` §5's GIF settings, plus the one recording setting a GIF reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GifSettings {
    /// Frames per second: `gif-fps`, one of 10, 15, 24, 30, 50 -- or **0**, which is the
    /// recorded monitor's own rate, resolved by [`Self::fps_for`] (D101).
    pub fps: u32,
    /// The widest the GIF may be, in pixels: `gif-max-width`. Zero means no cap.
    pub max_width: u32,
    /// gifski's 1–100: `gif-quality`.
    pub quality: u8,
    /// Whether the compositor draws the cursor into the stream: `rec-cursor`.
    pub cursor: bool,
}

impl Default for GifSettings {
    /// `spec/08` §5's defaults: 15 fps, 1280 px, quality 80, cursor shown (D69).
    fn default() -> Self {
        Self {
            fps: 15,
            max_width: 1280,
            quality: 80,
            cursor: true,
        }
    }
}

impl GifSettings {
    /// The rate the graph is asked for, given the monitor it will record.
    ///
    /// `fps` as set, never above [`GIF_FPS_MAX`] for the reason given on it. Zero is the
    /// monitor's own rate, and a screen faster than the ceiling is divided by the fewest
    /// whole times that bring it under: thirty at sixty hertz, forty at 120, 48 at 144.
    ///
    /// Divided, not capped. Fifty frames a second of a sixty-hertz screen is five frames of
    /// every six refreshes, so every fifth frame skips one: whatever moves on the screen
    /// moves twice as far ten times a second, and a real recording of a page scrolled by
    /// hand came out "choppy at some point" (2026-09-24). A fifty-frame GIF plays unevenly
    /// on a sixty-hertz screen too, since two hundredths is not a whole number of
    /// refreshes; thirty's three, three and four hundredths are two refreshes each. Rounded
    /// to the frame, because a 59.94 Hz panel is a sixty-hertz panel to a frame counter. A
    /// monitor whose rate the bridge could not learn gets [`SCREEN_RATE_FALLBACK`].
    ///
    /// The extension works the same number out, step for step, so that the toolbar's
    /// recording row can say it before anything is recorded: `recordedFps` in
    /// `extension/src/recordChoices.ts` (D117). Both sides test the same rates for the same
    /// answers, so a change here is a change there.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn fps_for(&self, source: &StreamSource) -> u32 {
        if self.fps != 0 {
            return self.fps.min(GIF_FPS_MAX);
        }
        let Some(hz) = source.refresh.filter(|hz| hz.is_finite() && *hz >= 1.0) else {
            return SCREEN_RATE_FALLBACK;
        };
        let every = (hz / f64::from(GIF_FPS_MAX)).ceil().max(1.0);
        ((hz / every).round() as u32).clamp(1, GIF_FPS_MAX)
    }
}

/// The monitor whose stream is recorded: its connector, its logical rect on the stage
/// and its scale, exactly as `GetMonitors` reports them -- and its refresh rate, which
/// the bridge adds from `DisplayConfig` when it can (D101).
#[derive(Debug, Clone, PartialEq)]
pub struct StreamSource {
    pub connector: String,
    pub rect: Rect,
    pub scale: f64,
    /// Hertz, or `None` when not known. Only [`GifSettings::fps_for`] reads it.
    pub refresh: Option<f64>,
}

/// A size in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// `videocrop`'s four margins, in the stream's physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Crop {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

/// Why a plan could not be made. Each is a message for the user, not a panic.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("the selection has no area")]
    EmptySelection,
    #[error("the selection lies outside the monitor being recorded")]
    OutsideMonitor,
    #[error("a frame rate of {0} is not a frame rate")]
    BadFrameRate(u32),
    #[error("a GIF quality of {0} is outside 1–100")]
    BadQuality(u8),
}

/// Everything the graph and the encoder are asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GifPlan {
    /// The monitor to `RecordMonitor`.
    pub connector: String,
    pub fps: u32,
    /// `pipewiresrc keepalive-time`, one frame interval rounded up: the stream is
    /// damage-driven and a still screen otherwise records nothing (`docs/spikes/11`).
    pub keepalive_ms: u32,
    /// The stream's own size, the monitor in physical pixels.
    pub stream: Size,
    pub crop: Crop,
    /// The cropped region before scaling.
    pub region: Size,
    /// The GIF's size after `gif-max-width`.
    pub output: Size,
    pub quality: u8,
    /// Mutter's `cursor-mode`: 0 hidden, 1 embedded (2, metadata, is M9's Studio mode).
    pub cursor_mode: u32,
}

impl GifPlan {
    /// Plans a recording of `selection` (logical stage coordinates) on `source`.
    pub fn new(
        selection: Rect,
        source: &StreamSource,
        settings: GifSettings,
    ) -> Result<Self, PlanError> {
        // Zero means the screen's rate and is resolved here; what cannot be resolved is
        // the fallback, so the only way to a bad rate now is a request that clamps to
        // nothing, which `fps_for` never produces. The check stays as the guard it was.
        let fps = settings.fps_for(source);
        if fps == 0 {
            return Err(PlanError::BadFrameRate(fps));
        }
        if !(1..=100).contains(&settings.quality) {
            return Err(PlanError::BadQuality(settings.quality));
        }
        if selection.is_empty() {
            return Err(PlanError::EmptySelection);
        }
        let (stream_w, stream_h) = source.rect.to_physical(source.scale);
        let stream = Size {
            width: stream_w,
            height: stream_h,
        };

        // The offset rounds the way the size does (`Rect::to_physical`, D23): to nearest,
        // because the compositor does and because Mutter's float32 scales carry an error
        // that ceiling turns into whole pixels. The extension keeps the recording frame and
        // the scroll outline clear of exactly this crop (`extension/src/outline.ts`, D115),
        // so a change here is a change there.
        let left = round_physical(selection.x - source.rect.x, source.scale);
        let top = round_physical(selection.y - source.rect.y, source.scale);
        let (sel_w, sel_h) = selection.to_physical(source.scale);
        // Clamped to the stream, so a selection dragged past the monitor's edge records
        // what is on the monitor rather than asking `videocrop` for pixels it has not got.
        let left = left.clamp(0, i64::from(stream_w));
        let top = top.clamp(0, i64::from(stream_h));
        let width = (i64::from(sel_w)).min(i64::from(stream_w) - left);
        let height = (i64::from(sel_h)).min(i64::from(stream_h) - top);
        if width <= 0 || height <= 0 {
            return Err(PlanError::OutsideMonitor);
        }
        let region = Size {
            width: width as u32,
            height: height as u32,
        };
        let crop = Crop {
            left: left as u32,
            top: top as u32,
            right: stream_w - left as u32 - region.width,
            bottom: stream_h - top as u32 - region.height,
        };
        let output = capped(region, settings.max_width);
        Ok(Self {
            connector: source.connector.clone(),
            fps,
            keepalive_ms: keepalive_ms(fps),
            stream,
            crop,
            region,
            output,
            quality: settings.quality,
            cursor_mode: u32::from(settings.cursor),
        })
    }

    /// True when the GIF is the whole monitor, which is when the controls have nowhere
    /// outside the rectangle to be (`spec/06` §3, `spec/01` row 36).
    #[must_use]
    pub fn is_whole_monitor(&self) -> bool {
        self.region == self.stream
    }
}

/// A logical offset in physical pixels, rounded to nearest (D23).
fn round_physical(logical: i32, scale: f64) -> i64 {
    (f64::from(logical) * scale).round() as i64
}

/// One frame interval in milliseconds, rounded up: 34 at 30 fps, 67 at 15, 100 at 10.
#[must_use]
pub fn keepalive_ms(fps: u32) -> u32 {
    if fps == 0 {
        return 0;
    }
    (1000.0 / f64::from(fps)).ceil() as u32
}

/// The size under `gif-max-width`, aspect kept; zero means no cap.
#[must_use]
pub fn capped(region: Size, max_width: u32) -> Size {
    if max_width == 0 || region.width <= max_width || region.width == 0 {
        return region;
    }
    let height =
        (f64::from(region.height) * f64::from(max_width) / f64::from(region.width)).round();
    Size {
        width: max_width,
        height: (height as u32).max(1),
    }
}

/// How many frames a recording of `duration_ms` should hold at `fps`, to the nearest
/// frame -- what a harness compares an encoded file's frame count against.
#[must_use]
pub fn expected_frames(duration_ms: u64, fps: u32) -> u64 {
    (duration_ms as f64 * f64::from(fps) / 1000.0).round() as u64
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn monitor(scale: f64) -> StreamSource {
        let logical_w = (1920.0 / scale).round() as i32;
        let logical_h = (1200.0 / scale).round() as i32;
        StreamSource {
            connector: "Meta-0".into(),
            rect: Rect::new(0, 0, logical_w, logical_h),
            scale,
            refresh: None,
        }
    }

    /// D101, D111: zero is the screen's own rate, divided by the fewest whole times that
    /// bring it under the GIF's ceiling -- so every frame is the same number of refreshes
    /// after the last -- and nothing is ever above that ceiling, a request for sixty
    /// included.
    #[test]
    fn the_screen_rate_is_a_whole_fraction_of_the_monitors_under_the_ceiling() {
        let screen = GifSettings { fps: 0, ..GifSettings::default() };
        let at = |hz: f64| screen.fps_for(&StreamSource { refresh: Some(hz), ..monitor(1.0) });
        assert_eq!(at(59.94), 30, "a sixty-hertz panel is halved, not capped at fifty");
        assert_eq!(at(60.026), 30, "the target machine's own panel");
        assert_eq!(at(120.0), 40);
        assert_eq!(at(143.98), 48);
        assert_eq!(at(240.0), 48);
        assert_eq!(at(100.0), 50);
        assert_eq!(at(50.0), 50, "a screen at the ceiling is its own rate");
        let slow = StreamSource { refresh: Some(30.0), ..monitor(1.0) };
        assert_eq!(screen.fps_for(&slow), 30);
        let sixty_asked = GifSettings { fps: 60, ..GifSettings::default() };
        assert_eq!(sixty_asked.fps_for(&slow), GIF_FPS_MAX, "asking for sixty gets fifty");
        assert_eq!(GifSettings::default().fps_for(&slow), 15, "a set rate is not the screen's");
    }

    /// The rates `extension/src/recordChoices.test.ts` pins for the toolbar's "Screen rate
    /// · 30 fps", with the same answers (D117). The row says the number before a recording
    /// is asked for and this is what the recording is made at, so the two must agree:
    /// change one of these and change that file with it.
    #[test]
    fn the_screen_rates_the_toolbar_says() {
        let screen = GifSettings { fps: 0, ..GifSettings::default() };
        let at = |hz: f64| screen.fps_for(&StreamSource { refresh: Some(hz), ..monitor(1.0) });
        // The owner's desk on 2026-09-25, as `DisplayConfig` reported it: the panel, DP-6.
        let (panel, big) = (60.025901794433594, 119.99758911132812);
        for (hz, fps) in [
            (60.0, 30),
            (59.94, 30),
            (75.0, 38),
            (120.0, 40),
            (144.0, 48),
            (165.0, 41),
            (panel, 30),
            (big, 40),
        ] {
            assert_eq!(at(hz), fps, "{hz} Hz");
        }
        assert_eq!(screen.fps_for(&monitor(1.0)), SCREEN_RATE_FALLBACK, "a rate not known");
    }

    /// A monitor whose rate the bridge could not learn is still a plan, at the fallback.
    #[test]
    fn an_unknown_screen_rate_falls_back_rather_than_failing() {
        let screen = GifSettings { fps: 0, ..GifSettings::default() };
        assert_eq!(screen.fps_for(&monitor(1.0)), SCREEN_RATE_FALLBACK);
        let plan = GifPlan::new(Rect::new(0, 0, 64, 40), &monitor(1.0), screen).expect("a plan");
        assert_eq!(plan.fps, SCREEN_RATE_FALLBACK);
    }

    /// The spike's own numbers: at 1.25, a 512x320 logical selection is 640x400 physical
    /// and the stream is the 1920x1200 panel, not the 1536x960 stage.
    #[test]
    fn crops_the_physical_stream_at_one_point_two_five() {
        let plan = GifPlan::new(
            Rect::new(512, 240, 512, 320),
            &monitor(1.25),
            GifSettings::default(),
        )
        .expect("plan");
        assert_eq!(
            plan.stream,
            Size {
                width: 1920,
                height: 1200
            }
        );
        assert_eq!(
            plan.region,
            Size {
                width: 640,
                height: 400
            }
        );
        assert_eq!(
            plan.crop,
            Crop {
                left: 640,
                top: 300,
                right: 640,
                bottom: 500
            }
        );
        assert_eq!(
            plan.output,
            Size {
                width: 640,
                height: 400
            }
        );
        assert_eq!(plan.connector, "Meta-0");
        assert!(!plan.is_whole_monitor());
    }

    /// Every scale the ladder has, the crop margins always add back up to the stream.
    #[test]
    fn margins_add_up_to_the_stream_at_every_scale() {
        for scale in [1.0, 1.25, 1.3333333730697632, 1.5, 1.6666666269302368, 2.0] {
            let source = monitor(scale);
            let plan = GifPlan::new(Rect::new(37, 53, 200, 120), &source, GifSettings::default())
                .expect("plan");
            assert_eq!(
                plan.crop.left + plan.region.width + plan.crop.right,
                plan.stream.width,
                "{scale}"
            );
            assert_eq!(
                plan.crop.top + plan.region.height + plan.crop.bottom,
                plan.stream.height,
                "{scale}"
            );
        }
    }

    /// D23's case: 200 logical at 1.6666666269302368 is 333 physical, not 334.
    #[test]
    fn rounds_to_nearest_like_the_compositor() {
        let plan = GifPlan::new(
            Rect::new(0, 0, 200, 120),
            &monitor(1.6666666269302368),
            GifSettings::default(),
        )
        .expect("plan");
        assert_eq!(plan.region.width, 333);
    }

    /// The two crops of 2026-09-24 on the laptop panel at 1.25, a scrolling capture's live
    /// view and a GIF, which the extension keeps its outlines clear of
    /// (`extension/src/outline.ts`, D115). Its tests expect these same crops.
    #[test]
    fn crops_the_panel_where_the_outlines_expect() {
        let panel = StreamSource {
            connector: "eDP-1".into(),
            rect: Rect::new(443, 1440, 1536, 960),
            scale: 1.25,
            refresh: None,
        };
        let whole = GifSettings { max_width: 0, ..GifSettings::default() };
        let scroll = GifPlan::new(Rect::new(847, 1870, 1081, 328), &panel, whole).expect("plan");
        assert_eq!((scroll.crop.left, scroll.crop.top), (505, 538));
        assert_eq!(
            scroll.region,
            Size {
                width: 1351,
                height: 410
            }
        );
        let gif = GifPlan::new(Rect::new(783, 1555, 1090, 669), &panel, whole).expect("plan");
        assert_eq!((gif.crop.left, gif.crop.top), (425, 144));
        assert_eq!(
            gif.region,
            Size {
                width: 1363,
                height: 836
            }
        );
    }

    #[test]
    fn a_selection_off_the_monitor_is_clamped_to_it() {
        let source = monitor(1.0);
        let plan = GifPlan::new(
            Rect::new(1800, 1100, 400, 300),
            &source,
            GifSettings::default(),
        )
        .expect("plan");
        assert_eq!(
            plan.region,
            Size {
                width: 120,
                height: 100
            }
        );
        assert_eq!(plan.crop.right, 0);
        assert_eq!(plan.crop.bottom, 0);
        assert_eq!(
            GifPlan::new(Rect::new(2000, 0, 10, 10), &source, GifSettings::default()),
            Err(PlanError::OutsideMonitor)
        );
    }

    #[test]
    fn the_whole_monitor_is_recognised() {
        let source = monitor(2.0);
        let plan = GifPlan::new(source.rect, &source, GifSettings::default()).expect("plan");
        assert!(plan.is_whole_monitor());
        assert_eq!(
            plan.crop,
            Crop {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0
            }
        );
        // 3840 physical pixels wide capped to 1280: the height follows.
        assert_eq!(
            plan.output,
            Size {
                width: 1280,
                height: 800
            }
        );
    }

    #[test]
    fn the_width_cap_keeps_the_aspect_and_zero_means_none() {
        assert_eq!(
            capped(
                Size {
                    width: 1600,
                    height: 900
                },
                800
            ),
            Size {
                width: 800,
                height: 450
            }
        );
        assert_eq!(
            capped(
                Size {
                    width: 640,
                    height: 400
                },
                800
            ),
            Size {
                width: 640,
                height: 400
            }
        );
        assert_eq!(
            capped(
                Size {
                    width: 1600,
                    height: 900
                },
                0
            ),
            Size {
                width: 1600,
                height: 900
            }
        );
        assert_eq!(
            capped(
                Size {
                    width: 3000,
                    height: 1
                },
                800
            ),
            Size {
                width: 800,
                height: 1
            }
        );
    }

    /// The spike's 34 ms at 30 fps, and one interval rounded up at the other rates.
    #[test]
    fn keepalive_is_one_frame_interval_rounded_up() {
        assert_eq!(keepalive_ms(30), 34);
        assert_eq!(keepalive_ms(24), 42);
        assert_eq!(keepalive_ms(15), 67);
        assert_eq!(keepalive_ms(10), 100);
        assert_eq!(keepalive_ms(0), 0);
    }

    /// A frame rate of zero used to be the bad setting here; since D101 it is "the
    /// screen's rate" and makes a plan (`an_unknown_screen_rate_falls_back_rather_than_failing`),
    /// so the quality and the selection are what is left to refuse before the bus is touched.
    #[test]
    fn settings_are_checked_before_the_bus_is_touched() {
        let source = monitor(1.0);
        let bad_quality = GifSettings {
            quality: 0,
            ..GifSettings::default()
        };
        assert_eq!(
            GifPlan::new(Rect::new(0, 0, 10, 10), &source, bad_quality),
            Err(PlanError::BadQuality(0))
        );
        assert_eq!(
            GifPlan::new(Rect::new(0, 0, 0, 10), &source, GifSettings::default()),
            Err(PlanError::EmptySelection)
        );
    }

    #[test]
    fn cursor_setting_becomes_mutters_mode() {
        let source = monitor(1.0);
        let shown =
            GifPlan::new(Rect::new(0, 0, 10, 10), &source, GifSettings::default()).expect("plan");
        assert_eq!(shown.cursor_mode, 1);
        let hidden = GifSettings {
            cursor: false,
            ..GifSettings::default()
        };
        assert_eq!(
            GifPlan::new(Rect::new(0, 0, 10, 10), &source, hidden)
                .expect("plan")
                .cursor_mode,
            0
        );
    }

    #[test]
    fn expected_frames_rounds_to_the_nearest_frame() {
        assert_eq!(expected_frames(10_000, 15), 150);
        assert_eq!(expected_frames(10_040, 15), 151);
        assert_eq!(expected_frames(0, 30), 0);
    }
}
