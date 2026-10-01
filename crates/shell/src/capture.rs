// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoding of the `HandleCapture` payload (`spec/10` §3.3).
//!
//! Only `path` is treated as required. Everything else has a defensible default, because
//! `spec/10` §8 makes the capture itself the thing that must survive: a capture that
//! arrives with an unrecognised mode or a missing animation duration is still a capture
//! the user took, and dropping it would be the worse failure.

use std::path::PathBuf;

use glib::prelude::ToVariant;
use octosnap_core::request::CaptureRequest;
use octosnap_core::{CaptureMode, CaptureResult, RequestedAction, Rect, SourceWindow};

use crate::error::BridgeError;
use crate::variant;


fn missing(field: &str) -> BridgeError {
    BridgeError::MalformedReply {
        method: "HandleCapture",
        detail: format!("required field '{field}' is absent or of the wrong type"),
    }
}

/// Decodes an `a{sv}` capture result.
pub fn from_variant(payload: &glib::Variant) -> Result<CaptureResult, BridgeError> {
    let dict = glib::VariantDict::new(Some(payload));

    let path = variant::string(&dict, "path").ok_or_else(|| missing("path"))?;
    let path = PathBuf::from(path);

    // The JSON twin sits beside the PNG by convention; derive it when absent rather than
    // failing, so a slightly older extension still works.
    let meta_path = variant::string(&dict, "meta_path")
        .map(PathBuf::from)
        .unwrap_or_else(|| path.with_extension("json"));

    let mode = variant::string(&dict, "mode")
        .and_then(|m| CaptureMode::from_wire(&m))
        .unwrap_or(CaptureMode::Area);

    let rect = variant::rect(&dict, "rect").unwrap_or_else(Rect::empty);

    // A scale of 0 would make every physical-pixel calculation collapse, so it is
    // treated as absent rather than trusted.
    let scale = variant::f64_(&dict, "scale").filter(|s| *s > 0.0).unwrap_or(1.0);

    let source_window = SourceWindow {
        app_id: variant::string(&dict, "app_id").unwrap_or_default(),
        app_name: variant::string(&dict, "app_name").unwrap_or_default(),
        title: variant::string(&dict, "window_title").unwrap_or_default(),
    };

    Ok(CaptureResult {
        path,
        meta_path,
        mode,
        rect,
        scale,
        display: variant::string(&dict, "display").unwrap_or_default(),
        cursor_rect: variant::rect(&dict, "cursor_rect"),
        source_window,
        window_alpha: variant::bool_(&dict, "window_alpha").unwrap_or(false),
        timestamp: variant::u64_(&dict, "timestamp").unwrap_or_default(),
        // Absent rather than defaulted: `0` would read as 1970 and make the elapsed time
        // fifty-six years, which is a worse answer than "not measured".
        confirmed_at: variant::u64_(&dict, "confirmed_at").filter(|t| *t > 0),
        animation_ms: variant::u32_(&dict, "animation_ms").unwrap_or(0),
        requested_action: variant::string(&dict, "requested_action")
            .and_then(|a| RequestedAction::from_wire(&a)),
        modifiers: variant::u32_(&dict, "modifiers").unwrap_or(0),
        // The extension only ever reports captures; files the user opens are the app's.
        external: false,
        // Echoed back from the request that started the capture: the shortcut that asked
        // for a single line asked before the pixels existed.
        linebreaks: variant::bool_(&dict, "linebreaks"),
        // A recording is made by the app, never decoded from an extension payload.
        duration_ms: None,
    })
}

/// Encodes a [`CaptureRequest`] as `BeginCapture`'s `(s, a{sv})` arguments
/// (`spec/10` §3.1).
///
/// Absent options are *omitted* rather than sent as a default. `spec/08` §5 makes
/// `freeze` and `cursor` user settings the extension owns, so sending `false` for an
/// option the caller did not mention would quietly override the user's preference on
/// every CLI invocation.
#[must_use]
pub fn request_to_variant(request: &CaptureRequest) -> glib::Variant {
    let options = glib::VariantDict::new(None);

    if let Some(rect) = request.rect {
        options.insert_value(
            "rect",
            &glib::Variant::from((rect.x, rect.y, rect.width, rect.height)),
        );
    }
    if let Some(display) = &request.display {
        options.insert("display", display.as_str());
    }
    if let Some(freeze) = request.freeze {
        options.insert("freeze", freeze);
    }
    if let Some(cursor) = request.cursor {
        options.insert("cursor", cursor);
    }
    if let Some(timer) = request.timer {
        options.insert("timer", timer);
    }
    if let Some(action) = request.action {
        options.insert("action", action.as_wire());
    }
    if let Some(start) = request.start {
        options.insert("start", start);
    }
    if let Some(linebreaks) = request.linebreaks {
        options.insert("linebreaks", linebreaks);
    }

    glib::Variant::tuple_from_iter([request.mode.as_wire().to_variant(), options.end()])
}

/// Reads back what [`request_to_variant`] wrote, which is how the app's `capture` action
/// receives a request from the CLI and the URL handler (`spec/10` §3.2).
///
/// Total by design: an unknown mode or a mistyped option yields the nearest sane request
/// rather than an error, because the two halves and the CLI are versioned separately.
/// The one thing that can fail is the mode, since there is no defensible default for
/// "capture something".
pub fn request_from_variant(payload: &glib::Variant) -> Result<CaptureRequest, BridgeError> {
    // Read the children rather than `get::<(String, glib::Variant)>()`: glib maps a
    // `glib::Variant` field to the signature `v`, so that destructuring rejects the
    // `a{sv}` this contract actually uses -- and does it with the baffling message
    // "expected (sa{sv}), got (sa{sv})".
    if payload.type_().as_str() != "(sa{sv})" {
        return Err(BridgeError::MalformedReply {
            method: "capture",
            detail: format!("expected (sa{{sv}}), got {}", payload.type_()),
        });
    }

    let mode = payload.child_value(0).get::<String>().ok_or_else(|| {
        BridgeError::MalformedReply {
            method: "capture",
            detail: "mode is not a string".to_owned(),
        }
    })?;
    let mode = CaptureMode::from_wire(&mode).ok_or_else(|| BridgeError::MalformedReply {
        method: "capture",
        detail: format!("unknown capture mode '{mode}'"),
    })?;

    let dict = glib::VariantDict::new(Some(&payload.child_value(1)));
    Ok(CaptureRequest {
        mode,
        // A rect with no area is treated as absent: it would reach the extension as
        // "capture nothing", and falling back to the overlay is the useful reading.
        rect: variant::rect(&dict, "rect").filter(|r| !r.is_empty()),
        display: variant::string(&dict, "display").filter(|d| !d.is_empty()),
        freeze: variant::bool_(&dict, "freeze"),
        cursor: variant::bool_(&dict, "cursor"),
        timer: variant::i32_(&dict, "timer"),
        action: variant::string(&dict, "action").and_then(|a| RequestedAction::from_wire(&a)),
        start: variant::bool_(&dict, "start"),
        linebreaks: variant::bool_(&dict, "linebreaks"),
    })
}

/// The interface XML the app exports. Kept beside the decoder so the two cannot drift.
pub const APP1_INTERFACE_XML: &str = r#"
<node>
  <interface name="io.github.odrakirmusic.OctoSnap.App1">
    <method name="Ping">
      <arg type="u" direction="out" name="protocol"/>
    </method>
    <method name="HandleCapture">
      <arg type="a{sv}" direction="in" name="result"/>
    </method>
  </interface>
</node>"#;

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn full() -> glib::Variant {
        let d = glib::VariantDict::new(None);
        d.insert("path", "/spool/01J.png");
        d.insert("meta_path", "/spool/01J.json");
        d.insert("mode", "area");
        d.insert_value("rect", &glib::Variant::from((10i32, 20i32, 512i32, 384i32)));
        d.insert("scale", 1.25f64);
        d.insert("display", "eDP-1");
        d.insert("window_alpha", false);
        d.insert("timestamp", 1_757_251_200_000_000u64);
        d.insert("animation_ms", 220u32);
        d.insert("requested_action", "copy");
        d.insert("modifiers", 0u32);
        d.end()
    }

    #[test]
    fn decodes_a_complete_payload() {
        let c = from_variant(&full()).expect("decode");
        assert_eq!(c.mode, CaptureMode::Area);
        assert_eq!(c.rect, Rect::new(10, 20, 512, 384));
        assert_eq!(c.scale, 1.25);
        assert_eq!(c.display, "eDP-1");
        assert_eq!(c.requested_action, Some(RequestedAction::Copy));
        // The whole point of carrying rect + scale: the PNG's real size is derivable.
        assert_eq!(c.rect.to_physical(c.scale), (640, 480));
    }

    #[test]
    fn path_is_the_only_required_field() {
        let d = glib::VariantDict::new(None);
        d.insert("path", "/spool/x.png");
        let c = from_variant(&d.end()).expect("decode minimal");
        assert_eq!(c.meta_path, PathBuf::from("/spool/x.json"));
        assert_eq!(c.mode, CaptureMode::Area);
        assert_eq!(c.scale, 1.0);
        assert!(c.rect.is_empty());
    }

    #[test]
    fn a_payload_without_a_path_is_rejected() {
        let d = glib::VariantDict::new(None);
        d.insert("mode", "area");
        assert!(from_variant(&d.end()).is_err());
    }

    /// A zero scale would make every physical-size calculation collapse to nothing.
    #[test]
    fn a_nonsensical_scale_falls_back_to_one() {
        let d = glib::VariantDict::new(None);
        d.insert("path", "/spool/x.png");
        d.insert("scale", 0.0f64);
        assert_eq!(from_variant(&d.end()).expect("decode").scale, 1.0);
    }

    /// An unknown mode from a newer extension must not discard the capture.
    #[test]
    fn an_unknown_mode_degrades_to_area() {
        let d = glib::VariantDict::new(None);
        d.insert("path", "/spool/x.png");
        d.insert("mode", "holographic");
        assert_eq!(from_variant(&d.end()).expect("decode").mode, CaptureMode::Area);
    }

    #[test]
    fn a_request_round_trips_through_the_wire() {
        let request = CaptureRequest::new(CaptureMode::Fullscreen)
            .with_rect(Some(Rect::new(5, 6, 700, 500)))
            .with_display("DP-2")
            .with_freeze(Some(true))
            .with_cursor(Some(false))
            .with_timer(Some(5))
            .with_action(Some(RequestedAction::Upload));

        let encoded = request_to_variant(&request);
        assert_eq!(encoded.type_().as_str(), "(sa{sv})");
        assert_eq!(request_from_variant(&encoded).expect("decode"), request);
    }

    /// The distinction the type exists for: an option the caller never mentioned must
    /// not arrive as `false` and override the user's setting (`spec/08` §5).
    #[test]
    fn absent_options_are_omitted_not_defaulted() {
        let encoded = request_to_variant(&CaptureRequest::new(CaptureMode::Area));
        let dict = glib::VariantDict::new(Some(&encoded.child_value(1)));
        assert!(dict.lookup_value("freeze", None).is_none());
        assert!(dict.lookup_value("cursor", None).is_none());
        assert!(dict.lookup_value("rect", None).is_none());

        let decoded = request_from_variant(&encoded).expect("decode");
        assert_eq!(decoded, CaptureRequest::new(CaptureMode::Area));
    }

    #[test]
    fn an_unknown_mode_is_the_one_thing_a_request_cannot_survive() {
        let payload = glib::Variant::tuple_from_iter([
            "holographic".to_variant(),
            glib::VariantDict::new(None).end(),
        ]);
        assert!(request_from_variant(&payload).is_err());
    }

    /// A caller that passes `--rect 0,0,0,0` should get the overlay, not a capture of
    /// nothing.
    #[test]
    fn an_empty_rect_falls_back_to_the_overlay() {
        let options = glib::VariantDict::new(None);
        options.insert_value("rect", &glib::Variant::from((0i32, 0i32, 0i32, 0i32)));
        let payload =
            glib::Variant::tuple_from_iter(["area".to_variant(), options.end()]);
        assert_eq!(request_from_variant(&payload).expect("decode").rect, None);
    }

    /// An option sent with the wrong type is treated as absent rather than taking the
    /// capture down, because the CLI and the extension ship separately.
    #[test]
    fn a_mistyped_option_is_ignored() {
        let options = glib::VariantDict::new(None);
        options.insert("freeze", "yes please");
        options.insert("timer", 5i32);
        let payload =
            glib::Variant::tuple_from_iter(["self-timer".to_variant(), options.end()]);
        let decoded = request_from_variant(&payload).expect("decode");
        assert_eq!(decoded.freeze, None);
        assert_eq!(decoded.timer, Some(5));
    }
}
