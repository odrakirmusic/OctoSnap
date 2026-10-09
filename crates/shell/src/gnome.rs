// SPDX-License-Identifier: GPL-3.0-or-later

//! `ShellBridge` over the real extension.
//!
//! Uses gio's D-Bus rather than `zbus`, which `spec/10` §5 suggested. The reason is
//! structural, not taste: the app's own `App1` interface has to be exported on the
//! connection `GApplication` already owns, or callers addressing the well-known name
//! reach GApplication's connection and find no object there. zbus speaks the wire
//! protocol itself and cannot adopt an existing `GDBusConnection`, so using it would mean
//! a second connection plus a second async runtime inside a GTK main loop. gio is
//! already in-process and integrates with `glib::MainContext`. Recorded as D7 in
//! `docs/decisions.md`; zbus remains a good fit for the M5 recorder, which runs off the
//! UI thread anyway.

use std::path::{Path, PathBuf};

use octosnap_core::protocol::{SHELL_BUS_NAME, SHELL_INTERFACE, SHELL_OBJECT_PATH};
use octosnap_core::request::CaptureRequest;
use octosnap_core::{Monitor, Rect, ScrollDirection};

use crate::bridge::{
    ClipboardImage, Cue, PickedColor, Placement, RecordingState, ShellBridge, ShellVersion,
};
use crate::capture;
use crate::display_config;
use crate::error::BridgeError;
use crate::variant;

/// Short on purpose. The extension runs on the compositor's main loop; if it has not
/// answered in two seconds the desktop has a bigger problem than OctoSnap, and the app
/// should get on with degraded mode rather than hang.
const CALL_TIMEOUT_MS: i32 = 2_000;

/// How long `PickColor` may wait for the user to click: five minutes, after which a
/// picker left open counts as abandoned.
const PICK_TIMEOUT_MS: i32 = 300_000;

/// How long `ReadClipboardImage` may take (D169). The application that copied serves its
/// type when it is read, and a GTK app encodes a picture's PNG only then, so a large one
/// takes longer than any other method's answer; a client that never answers at all is
/// given up on here rather than left to hold the action.
const CLIPBOARD_READ_TIMEOUT_MS: i32 = 30_000;

/// How long one scroll-assist frame may take.
///
/// A grab is the compositor repainting the selection, measured as `screenshot_area` in
/// `docs/spikes/17` at 6.6 ms plus about 81 ms per megapixel (the same repaint since D116):
/// a 3840x2160 selection is 8.3 megapixels, so two thirds of a second on an idle machine
/// and more on a busy one. Ten seconds is generous enough that a slow frame
/// is never mistaken for a dead compositor, and short enough that a dead one is noticed.
const GRAB_TIMEOUT_MS: i32 = 10_000;

#[derive(Debug, Clone)]
pub struct GnomeExtensionBridge {
    connection: gio::DBusConnection,
}

impl GnomeExtensionBridge {
    /// Opens its own session-bus connection. Prefer [`Self::from_connection`] inside the
    /// app so the bridge shares `GApplication`'s connection.
    pub async fn connect() -> Result<Self, BridgeError> {
        let connection = gio::bus_get_future(gio::BusType::Session).await?;
        Ok(Self { connection })
    }

    #[must_use]
    pub fn from_connection(connection: gio::DBusConnection) -> Self {
        Self { connection }
    }

    async fn call(
        &self,
        method: &'static str,
        params: Option<glib::Variant>,
        reply_type: &str,
    ) -> Result<glib::Variant, BridgeError> {
        self.call_with_timeout(method, params, reply_type, CALL_TIMEOUT_MS).await
    }

    /// [`Self::call`] with the caller's own patience. Every method but one answers in
    /// milliseconds; `PickColor` answers when a person has clicked.
    async fn call_with_timeout(
        &self,
        method: &'static str,
        params: Option<glib::Variant>,
        reply_type: &str,
        timeout_ms: i32,
    ) -> Result<glib::Variant, BridgeError> {
        let ty = glib::VariantTy::new(reply_type).map_err(|e| BridgeError::MalformedReply {
            method,
            detail: format!("invalid reply signature {reply_type}: {e}"),
        })?;

        // DO_NOT_AUTO_START has no effect here because an extension cannot be
        // bus-activated at all: if it is disabled, the name simply has no owner. That is
        // exactly the condition we translate into Unavailable.
        self.connection
            .call_future(
                Some(SHELL_BUS_NAME),
                SHELL_OBJECT_PATH,
                SHELL_INTERFACE,
                method,
                params.as_ref(),
                Some(ty),
                gio::DBusCallFlags::NONE,
                timeout_ms,
            )
            .await
            .map_err(|e| {
                if is_method_missing(&e) {
                    BridgeError::Unsupported { method }
                } else if is_extension_absent(&e) {
                    BridgeError::Unavailable(e.message().to_owned())
                } else {
                    BridgeError::Dbus(e)
                }
            })
    }
}

/// Distinguishes "the extension is not there" from "the extension failed".
///
/// The former is a normal state on a fresh install and must lead to the setup page, not
/// an error dialog (`spec/10` §2). The error name is checked as well as the mapped kind,
/// because whether glib surfaces a remote error as `G_DBUS_ERROR` or as a generic
/// `G_IO_ERROR` depends on how the peer reported it.
/// The name is owned and the object is there, but this member is not.
///
/// Checked before "absent", because `UnknownMethod` means the opposite of absent: the
/// extension answered.
fn is_method_missing(e: &glib::Error) -> bool {
    if e.kind::<gio::DBusError>() == Some(gio::DBusError::UnknownMethod) {
        return true;
    }
    e.message().contains("No such method")
}

fn is_extension_absent(e: &glib::Error) -> bool {
    if let Some(kind) = e.kind::<gio::DBusError>()
        && matches!(
            kind,
            gio::DBusError::ServiceUnknown
                | gio::DBusError::NameHasNoOwner
                | gio::DBusError::UnknownObject
                | gio::DBusError::UnknownInterface
        )
    {
        return true;
    }
    let message = e.message();
    message.contains("ServiceUnknown")
        || message.contains("NameHasNoOwner")
        || message.contains("was not provided by any .service files")
}

fn monitor_from_dict(dict_variant: &glib::Variant) -> Option<Monitor> {
    let dict = glib::VariantDict::new(Some(dict_variant));
    Some(Monitor {
        index: variant::i32_(&dict, "index")?,
        connector: variant::string(&dict, "connector").unwrap_or_default(),
        geometry: variant::rect_from_parts(&dict, "x", "y", "width", "height")?,
        work_area: variant::rect(&dict, "work-area")?,
        scale: variant::f64_(&dict, "scale")?,
        geometry_scale: variant::i32_(&dict, "geometry-scale")?,
        primary: variant::bool_(&dict, "primary").unwrap_or(false),
        current: variant::bool_(&dict, "current").unwrap_or(false),
        refresh: None,
    })
}

impl ShellBridge for GnomeExtensionBridge {
    async fn version(&self) -> Result<ShellVersion, BridgeError> {
        let reply = self.call("Version", None, "(su)").await?;
        let (version, protocol) =
            reply
                .get::<(String, u32)>()
                .ok_or_else(|| BridgeError::MalformedReply {
                    method: "Version",
                    detail: format!("expected (su), got {}", reply.type_()),
                })?;
        Ok(ShellVersion { version, protocol })
    }

    async fn set_spool(&self, dir: &Path) -> Result<(), BridgeError> {
        let params = glib::Variant::tuple_from_iter([glib::Variant::from(dir.to_string_lossy().as_ref())]);
        self.call("SetSpool", Some(params), "()").await?;
        Ok(())
    }

    async fn recent_log(&self) -> Result<Vec<String>, BridgeError> {
        let reply = self.call("GetLog", None, "(as)").await?;
        reply.get::<(Vec<String>,)>().map(|(lines,)| lines).ok_or_else(|| {
            BridgeError::MalformedReply {
                method: "GetLog",
                detail: format!("expected (as), got {}", reply.type_()),
            }
        })
    }

    async fn begin_capture(&self, request: &CaptureRequest) -> Result<String, BridgeError> {
        let params = capture::request_to_variant(request);
        let reply = self.call("BeginCapture", Some(params), "(s)").await?;
        reply
            .child_value(0)
            .get::<String>()
            .ok_or_else(|| BridgeError::MalformedReply {
                method: "BeginCapture",
                detail: "handle is not a string".to_owned(),
            })
    }

    async fn cancel_capture(&self, handle: &str) -> Result<(), BridgeError> {
        let params = glib::Variant::from((handle.to_owned(),));
        self.call("CancelCapture", Some(params), "()").await?;
        Ok(())
    }

    async fn set_clipboard_image(&self, path: &Path) -> Result<(), BridgeError> {
        let params = glib::Variant::from((path.to_string_lossy().into_owned(),));
        self.call("SetClipboardImage", Some(params), "()").await?;
        Ok(())
    }

    async fn read_clipboard_image(&self) -> Result<Option<ClipboardImage>, BridgeError> {
        let reply = self
            .call_with_timeout("ReadClipboardImage", None, "(ss)", CLIPBOARD_READ_TIMEOUT_MS)
            .await?;
        let (path, name) = reply.get::<(String, String)>().ok_or_else(|| BridgeError::MalformedReply {
            method: "ReadClipboardImage",
            detail: "the reply is not a path and a name".to_owned(),
        })?;
        if path.is_empty() {
            return Ok(None);
        }
        Ok(Some(ClipboardImage { path: PathBuf::from(path), name: (!name.is_empty()).then_some(name) }))
    }

    async fn set_clipboard_text(&self, text: &str) -> Result<(), BridgeError> {
        let params = glib::Variant::from((text.to_owned(),));
        self.call("SetClipboardText", Some(params), "()").await?;
        Ok(())
    }

    async fn play_sound(&self, cue: Cue) -> Result<(), BridgeError> {
        let params = glib::Variant::from((cue.wire(),));
        self.call("PlaySound", Some(params), "()").await?;
        Ok(())
    }

    async fn place_window(
        &self,
        object_path: &str,
        role: &str,
        placement: &Placement,
        animate_ms: u32,
    ) -> Result<Rect, BridgeError> {
        let options = glib::VariantDict::new(None);
        options.insert("animate_ms", animate_ms);
        match *placement {
            Placement::At { x, y } => {
                options.insert("x", x);
                options.insert("y", y);
            }
            Placement::Stacked { edge, offset, inset, monitor, size } => {
                options.insert("edge", edge.as_wire());
                options.insert("offset", offset);
                options.insert("inset", inset);
                options.insert("monitor", monitor.as_wire());
                options.insert("width", size.width);
                options.insert("height", size.height);
            }
        }

        let params = glib::Variant::tuple_from_iter([
            glib::Variant::from(object_path),
            glib::Variant::from(role),
            options.end(),
        ]);
        let reply = self.call("PlaceWindow", Some(params), "((iiii))").await?;
        let (x, y, width, height) = reply
            .child_value(0)
            .get::<(i32, i32, i32, i32)>()
            .ok_or_else(|| BridgeError::MalformedReply {
                method: "PlaceWindow",
                detail: "the landed rect is not (iiii)".to_owned(),
            })?;
        Ok(Rect::new(x, y, width, height))
    }

    async fn move_window_by(
        &self,
        object_path: &str,
        dx: i32,
        dy: i32,
    ) -> Result<Rect, BridgeError> {
        let params = glib::Variant::from((object_path.to_owned(), dx, dy));
        let reply = self.call("MoveWindowBy", Some(params), "((iiii))").await?;
        let (x, y, width, height) = reply
            .child_value(0)
            .get::<(i32, i32, i32, i32)>()
            .ok_or_else(|| BridgeError::MalformedReply {
                method: "MoveWindowBy",
                detail: "the landed rect is not (iiii)".to_owned(),
            })?;
        Ok(Rect::new(x, y, width, height))
    }

    async fn set_window_shadow(
        &self,
        object_path: &str,
        radius: i32,
        opacity: f64,
    ) -> Result<(), BridgeError> {
        let look = glib::VariantDict::new(None);
        look.insert("radius", radius);
        look.insert("opacity", opacity);
        let params = glib::Variant::tuple_from_iter([glib::Variant::from(object_path), look.end()]);
        self.call("SetWindowShadow", Some(params), "()").await?;
        Ok(())
    }

    async fn focus_window(&self, object_path: &str, focus: bool) -> Result<bool, BridgeError> {
        let params = glib::Variant::from((object_path.to_owned(), focus));
        let reply = self.call("FocusWindow", Some(params), "(b)").await?;
        Ok(reply.child_value(0).get::<bool>().unwrap_or(false))
    }

    async fn set_recording_state(
        &self,
        state: RecordingState,
        elapsed_ms: u32,
    ) -> Result<(), BridgeError> {
        let params = glib::Variant::from((state.as_wire(), elapsed_ms));
        self.call("SetRecordingState", Some(params), "()").await?;
        Ok(())
    }

    async fn set_update_offered(&self, offered: bool) -> Result<(), BridgeError> {
        self.call("SetUpdateOffered", Some(glib::Variant::from((offered,))), "()").await?;
        Ok(())
    }

    async fn show_recording_frame(&self, rect: Rect, visible: bool) -> Result<(), BridgeError> {
        let params = glib::Variant::tuple_from_iter([
            glib::Variant::from((rect.x, rect.y, rect.width, rect.height)),
            glib::Variant::from(visible),
        ]);
        self.call("ShowRecordingFrame", Some(params), "()").await?;
        Ok(())
    }

    async fn show_scroll_frame(&self, object_path: &str, rect: Rect) -> Result<(), BridgeError> {
        let params = glib::Variant::tuple_from_iter([
            glib::Variant::from(object_path),
            glib::Variant::from((rect.x, rect.y, rect.width, rect.height)),
        ]);
        self.call("ShowScrollFrame", Some(params), "()").await?;
        Ok(())
    }

    async fn start_scroll_assist(&self, rect: Rect, direction: ScrollDirection) -> Result<String, BridgeError> {
        // The step stays on the wire, as nothing: it was how far `ScrollStep` scrolled,
        // which went with auto-scroll (D153), and a shell from before that still asks for
        // three arguments.
        let params = glib::Variant::tuple_from_iter([
            glib::Variant::from((rect.x, rect.y, rect.width, rect.height)),
            glib::Variant::from(direction.as_wire()),
            glib::Variant::from(0i32),
        ]);
        let reply = self.call("StartScrollAssist", Some(params), "(s)").await?;
        reply.get::<(String,)>().map(|(handle,)| handle).ok_or_else(|| {
            BridgeError::MalformedReply {
                method: "StartScrollAssist",
                detail: format!("expected (s), got {}", reply.type_()),
            }
        })
    }

    async fn grab_frame(&self, handle: &str) -> Result<std::path::PathBuf, BridgeError> {
        let params = glib::Variant::tuple_from_iter([glib::Variant::from(handle)]);
        // A grab is a repaint on the compositor's own thread -- 6.6 ms plus about 80 ms a
        // megapixel (`docs/spikes/17`), so a full-height selection on a 4K screen is
        // already past the default two seconds by itself on a busy machine.
        let reply = self.call_with_timeout("GrabFrame", Some(params), "(s)", GRAB_TIMEOUT_MS).await?;
        reply.get::<(String,)>().map(|(path,)| std::path::PathBuf::from(path)).ok_or_else(|| {
            BridgeError::MalformedReply {
                method: "GrabFrame",
                detail: format!("expected (s), got {}", reply.type_()),
            }
        })
    }

    async fn end_scroll_assist(&self, handle: &str) -> Result<(), BridgeError> {
        let params = glib::Variant::tuple_from_iter([glib::Variant::from(handle)]);
        self.call("EndScrollAssist", Some(params), "()").await?;
        Ok(())
    }

    async fn pick_color(&self) -> Result<Option<PickedColor>, BridgeError> {
        // The reply arrives when the user clicks, so the two-second timeout every other
        // call lives under is a person's patience here, not a bus's.
        let reply = self.call_with_timeout("PickColor", None, "(bddd)", PICK_TIMEOUT_MS).await?;
        let (ok, r, g, b) = reply.get::<(bool, f64, f64, f64)>().ok_or_else(|| {
            BridgeError::MalformedReply {
                method: "PickColor",
                detail: format!("expected (bddd), got {}", reply.type_()),
            }
        })?;
        Ok(ok.then_some(PickedColor { r, g, b }))
    }

    async fn desktop_icons(&self, what: &str) -> Result<(bool, bool), BridgeError> {
        let reply = self
            .call(
                "ToggleDesktopIcons",
                Some(glib::Variant::tuple_from_iter([glib::Variant::from(what)])),
                "(bb)",
            )
            .await?;
        reply.get::<(bool, bool)>().ok_or_else(|| BridgeError::MalformedReply {
            method: "ToggleDesktopIcons",
            detail: format!("expected (bb), got {}", reply.type_()),
        })
    }

    async fn monitors(&self) -> Result<Vec<Monitor>, BridgeError> {
        let reply = self.call("GetMonitors", None, "(aa{sv})").await?;
        let array = reply.child_value(0);

        let mut monitors = Vec::with_capacity(array.n_children());
        for index in 0..array.n_children() {
            let dict = array.child_value(index);
            monitors.push(monitor_from_dict(&dict).ok_or_else(|| {
                BridgeError::MalformedReply {
                    method: "GetMonitors",
                    detail: format!("monitor {index} is missing a required key"),
                }
            })?);
        }

        // The extension cannot report connectors (see display_config), so fill them in
        // from Mutter. A failure here is not fatal: a monitor with an empty connector is
        // still usable for everything except naming it, so log nothing and carry on
        // rather than losing the layout.
        if let Ok(logical) = display_config::logical_monitors(&self.connection).await {
            for monitor in &mut monitors {
                let (x, y) = (monitor.geometry.x, monitor.geometry.y);
                if let Some(connector) = display_config::connector_at(&logical, x, y) {
                    monitor.connector = connector.to_owned();
                }
                // And the refresh rate, from the same reply and by the same key (D101).
                monitor.refresh = display_config::refresh_at(&logical, x, y);
            }
        }

        Ok(monitors)
    }
}
