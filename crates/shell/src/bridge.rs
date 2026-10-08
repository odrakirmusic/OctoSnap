// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::{Path, PathBuf};

use octosnap_core::qao::{Edge, Size};
use octosnap_core::request::CaptureRequest;
use octosnap_core::{Monitor, Rect, ScrollDirection};

use crate::error::BridgeError;

/// Which monitor a placement is measured against (`PlaceWindow`'s `monitor` option).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlacementMonitor {
    /// The monitor the pointer is on -- `spec/04` §10 item 1's rule, and the default for
    /// a card, because "Move to active screen" is on by default (`spec/08` §1).
    #[default]
    Pointer,
    Primary,
    /// Wherever Mutter mapped the window. Right for a surface that has already been
    /// placed once and is only being nudged.
    Window,
}

impl PlacementMonitor {
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Pointer => "pointer",
            Self::Primary => "primary",
            Self::Window => "window",
        }
    }
}

/// What the extension read off the clipboard for the app (`ReadClipboardImage`, D169).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardImage {
    /// The file the extension wrote beside the spool: the clipboard's pixels, or a copy of
    /// the image file it named. The reader's to remove.
    pub path: PathBuf,
    /// The copied file's own name when the clipboard held a file; `None` for pixels.
    pub name: Option<String>,
}

/// A sound the app asks the extension to play (`PlaySound`, D134).
///
/// Named for what happened, not for what it sounds like: the extension's `soundCues.ts`
/// holds the table, so the two halves cannot disagree about which file is the copy sound.
/// The shutter is not here. It is the extension's own, played at the instant the pixels
/// are read (D11), and the only way the app names one is Settings' preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// `spec/07` §2.1: recognised text is on the clipboard.
    TextCopied,
    /// Something the user asked to copy is on the clipboard.
    Copied,
    /// Something the user asked to pin is on the screen.
    Pinned,
    RecordStart,
    /// Played once the stream has ended (`spec/09` §4), so it is in no recording.
    RecordStop,
    /// Settings' preview of a `capture-sound` value, `none` excepted.
    Shutter(&'static str),
}

impl Cue {
    /// The name `PlaySound` takes.
    #[must_use]
    pub fn wire(self) -> String {
        match self {
            Self::TextCopied => "text-copied".to_owned(),
            Self::Copied => "copied".to_owned(),
            Self::Pinned => "pinned".to_owned(),
            Self::RecordStart => "record-start".to_owned(),
            Self::RecordStop => "record-stop".to_owned(),
            Self::Shutter(kind) => format!("shutter-{kind}"),
        }
    }
}

/// Where a window should go, in the vocabulary `PlaceWindow` understands.
///
/// Two shapes rather than one set of nullable fields: a window is either placed at a
/// point somebody computed or slotted into an edge-anchored stack, and the two never
/// share arguments. Making that an enum means a caller cannot ask for an offset *and* an
/// x, which the D-Bus dictionary would happily accept and silently resolve one way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// An explicit top-left in logical stage coordinates.
    At { x: i32, y: i32 },
    /// A slot in the edge-anchored stack (`spec/04` §2).
    Stacked {
        edge: Edge,
        /// Distance from the anchor to this card, in **card** space -- see `place.ts`.
        offset: i32,
        /// The transparent shadow band the window carries around its card, so the
        /// extension can put the *card* at the measured margin rather than the window.
        inset: i32,
        monitor: PlacementMonitor,
        /// The window's own size, because the compositor may not know it yet.
        ///
        /// A card asks to be placed as soon as it is drawable, and at that moment Mutter
        /// has not processed its first buffer, so `get_frame_rect()` answers `0x0`. The
        /// client made the window and knows how big it is; the compositor knows where it
        /// landed. Neither is asked for the other's half.
        size: Size,
    },
}

/// A pixel's colour, as `spec/10` §3.1's `PickColor` answers it: channels in `0..=1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickedColor {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

/// A recording's phase, as `spec/10` §3.1's `SetRecordingState` names it. M5 uses four of
/// the five: a GIF has no pause (D68), so `Paused` is M9's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    /// Nothing is recording; the indicator is off.
    Idle,
    /// The countdown before the first frame (`rec-countdown`).
    Countdown,
    /// Frames are being taken.
    Recording,
    /// Stop was pressed; the GIF is still being written.
    Processing,
}

impl RecordingState {
    /// The wire string `SetRecordingState` expects.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Countdown => "countdown",
            Self::Recording => "recording",
            Self::Processing => "processing",
        }
    }
}

/// What the extension reports about itself (`spec/10` §3.1 `Version`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellVersion {
    /// The extension's own release, e.g. `0.1.0`.
    pub version: String,
    /// The protocol it speaks. Compared against `protocol::PROTOCOL_VERSION`.
    pub protocol: u32,
}

/// The seam that lets the app run without the extension.
///
/// `spec/00` §4.3 is explicit that degraded mode must be *designed for* now and *built*
/// no earlier than M6, which is what this trait is: `GnomeExtensionBridge` today,
/// `NullBridge` in tests, `PortalBridge` later. It is intentionally not object-safe --
/// the app selects an implementation once at startup, so static dispatch is enough and
/// avoids boxing every call on the capture path.
#[allow(async_fn_in_trait)]
pub trait ShellBridge {
    async fn version(&self) -> Result<ShellVersion, BridgeError>;

    /// The extension's recent log lines, newest last (`GetLog`), for the report
    /// `spec/11` M7's crash-log export writes. [`BridgeError::Unsupported`] from an
    /// extension older than the method.
    async fn recent_log(&self) -> Result<Vec<String>, BridgeError>;

    /// Tells the extension where this app's capture spool is (`SetSpool`, D122), so a
    /// capture is written where this app can read it: inside the Flatpak that is the
    /// app's own `~/.var/app/<id>/cache`, which the extension cannot guess from the
    /// host's `$XDG_CACHE_HOME`. [`BridgeError::Unsupported`] from an older extension.
    async fn set_spool(&self, dir: &Path) -> Result<(), BridgeError>;

    /// Monitors in logical stage coordinates, each carrying its own scale
    /// (`spec/01` §1).
    async fn monitors(&self) -> Result<Vec<Monitor>, BridgeError>;

    /// Starts a capture and returns the handle it will be reported under
    /// (`spec/10` §3.1 `BeginCapture`).
    ///
    /// Returns as soon as the capture is *accepted*, not when it finishes: the result
    /// arrives later as `HandleCapture` on the app. So an `Ok` here means "the overlay is
    /// up", and a capture can still be cancelled or fail after this returns -- which is
    /// exactly why the handle is worth having.
    async fn begin_capture(&self, request: &CaptureRequest) -> Result<String, BridgeError>;

    /// Withdraws a capture in flight. Cancelling a handle that is not running is not an
    /// error (`spec/10` §3.1).
    async fn cancel_capture(&self, handle: &str) -> Result<(), BridgeError>;

    /// Puts a PNG on the clipboard (`spec/10` §3.1).
    ///
    /// Goes through the extension rather than `Gdk.Clipboard` because `spec/01` §2 row 15
    /// notes the latter only works once the app has had focus or input -- and the point of
    /// copy-on-capture is that it happens without the app ever taking focus.
    async fn set_clipboard_image(&self, path: &Path) -> Result<(), BridgeError>;

    /// The clipboard's image, read by the extension (D169): its pixels, or the image file
    /// it names, written where this app can read it. `None` when it holds no image.
    ///
    /// Through the extension for the reason copies go through it, the other way round:
    /// Mutter offers the selection to the focused client only, and Annotate the Clipboard's
    /// Image is asked for while another application has the keyboard. `Gdk.Clipboard` then
    /// reads nothing, or only the types the app was offered when one of its windows last
    /// had focus.
    /// An extension from before D169 answers [`BridgeError::Unsupported`].
    async fn read_clipboard_image(&self) -> Result<Option<ClipboardImage>, BridgeError>;

    /// Puts text on the clipboard (`spec/07` §2.1's "recognized text is copied").
    ///
    /// Through the extension for the same reason the image is: on Wayland the selection
    /// belongs to whoever has a surface and a recent input serial, and a text capture
    /// copies without the app ever taking focus.
    async fn set_clipboard_text(&self, text: &str) -> Result<(), BridgeError>;

    /// One of the app's sounds (D134), played by the extension.
    ///
    /// Not the shutter and not on the shutter's path: each of these happens a moment
    /// after a capture, or with none, so it is a second sound at a second moment. The
    /// caller decides whether sounds are on; `spec/08` §1's `ui-sounds` is the app's key,
    /// and a switch read in two places is a switch that disagrees with itself.
    async fn play_sound(&self, cue: Cue) -> Result<(), BridgeError>;

    /// Puts one of the app's own windows where it belongs (`spec/10` §3.1 `PlaceWindow`).
    ///
    /// Returns the rect the window **landed** in, which is not always the one requested:
    /// `move_frame` is clamped silently (`docs/spikes/01-02`). A caller that stacks
    /// windows has to stack against reality, so the answer is the rect and not a unit.
    ///
    /// `animate_ms` asks the compositor to slide the window's *actor* there rather than
    /// have it appear at the new position (`spec/04` §5's 200 ms stack shift). Zero for a
    /// window that has never been placed, which has nowhere to slide from.
    async fn place_window(
        &self,
        object_path: &str,
        role: &str,
        placement: &Placement,
        animate_ms: u32,
    ) -> Result<Rect, BridgeError>;

    /// Nudges a window by a delta and reports where it landed (`spec/07` §3.2).
    ///
    /// Relative, not absolute: the user can drag a pin between two arrow-key presses, so
    /// the caller cannot know where it currently is, and an absolute move would teleport
    /// it back to wherever the caller last remembered.
    async fn move_window_by(
        &self,
        object_path: &str,
        dx: i32,
        dy: i32,
    ) -> Result<Rect, BridgeError>;

    /// `spec/07` §3.1's pin shadow, which the compositor draws outside the window, since a
    /// pin's window is the capture's rect to the pixel (D66, D132). `radius` is the pin's
    /// corner radius in logical pixels and `opacity` its own, from 0 to 1; an opacity of 0
    /// takes the shadow away. A window shown again is a new window, and needs asking again.
    async fn set_window_shadow(
        &self,
        object_path: &str,
        radius: i32,
        opacity: f64,
    ) -> Result<(), BridgeError>;

    /// `spec/10` §3.1's `PickColor`: the colour under the pointer when the user next
    /// clicks, anywhere on screen -- `spec/05` §2's pipette "via the extension, from
    /// anywhere on screen". Only the compositor can read a pixel on Wayland, and the
    /// shell's own picker (the `color-pick` loupe cursor, Escape to cancel) is the UI.
    /// `None` when the user backed out rather than clicking.
    async fn pick_color(&self) -> Result<Option<PickedColor>, BridgeError>;

    /// `DSK-01` (`spec/07` §5): `toggle`, `hide` or `show` the desktop icons, which on
    /// Ubuntu are another extension's windows and so the shell's to hide. Answers
    /// `(hidden, available)`: the state afterwards, and whether the session has desktop
    /// icons at all -- vanilla GNOME has none, and "hidden" would be the wrong word for
    /// nothing.
    async fn desktop_icons(&self, what: &str) -> Result<(bool, bool), BridgeError>;

    /// Lends a window the keyboard, or gives it back (`spec/04` §3).
    ///
    /// `focus = false` restores what had it before rather than merely unfocusing, which
    /// is why this is one call with a flag instead of two: only the compositor knows what
    /// the previous window was, so only the compositor can put it back.
    async fn focus_window(&self, object_path: &str, focus: bool) -> Result<bool, BridgeError>;

    /// `spec/10` §3.1's `SetRecordingState`: the extension drives its panel indicator from
    /// this -- red with the timer while `Recording`, a spinner while `Processing`, gone at
    /// `Idle`. `elapsed_ms` is the recording's own clock, which the app ticks.
    async fn set_recording_state(
        &self,
        state: RecordingState,
        elapsed_ms: u32,
    ) -> Result<(), BridgeError>;

    /// `spec/10` §3.1's `ShowRecordingFrame`: the red frame the extension draws *outside*
    /// `rect`, so the recorded pixels never carry it (`spec/06` §4.4). `visible = false`
    /// takes it away, which Stop and Trash both do.
    async fn show_recording_frame(&self, rect: Rect, visible: bool) -> Result<(), BridgeError>;

    /// The blue outline round a scrolling capture's selection, from the moment its pill is
    /// placed rather than from Start (D152). Drawn *outside* `rect`, as the scroll assist's
    /// own (D78, D115), and kept for as long as the window at `object_path` lives, so there
    /// is no call that takes it away: the pill going is the capture over, however it ended.
    async fn show_scroll_frame(&self, object_path: &str, rect: Rect) -> Result<(), BridgeError>;

    /// `spec/07` §1.3's `StartScrollAssist(rect, direction, step_px) -> handle`.
    ///
    /// The extension creates and *holds* a virtual pointer device, parks the real pointer
    /// in the middle of `rect` -- so the wheel scrolls the page, and not the pill Start was
    /// pressed on -- and answers with a handle. The pointer goes back at the end.
    ///
    /// Only the app may call this, and the extension checks that rather than trusting it
    /// (`spec/10` §7): a method that injects input into whatever has focus is not one to
    /// leave open to anything on the session bus.
    ///
    /// Nothing scrolls the page but the user. `ScrollStep`, which did, went with
    /// auto-scroll (D153), and the step a session was started with went with it.
    async fn start_scroll_assist(&self, rect: Rect, direction: ScrollDirection) -> Result<String, BridgeError>;

    /// `spec/07` §1.3's `GrabFrame(handle) -> path`: one PNG of the selection, written
    /// where the app can read it. The app deletes the frames when it ends the session.
    async fn grab_frame(&self, handle: &str) -> Result<std::path::PathBuf, BridgeError>;

    /// `spec/07` §1.3's `EndScrollAssist(handle)`: drop the device, put the pointer back,
    /// take the frames with it. Safe to call twice, and the app does -- from the end of
    /// the loop and from every failure path, which is what makes the device impossible to
    /// leak.
    async fn end_scroll_assist(&self, handle: &str) -> Result<(), BridgeError>;
}

#[cfg(test)]
mod tests {
    use super::Cue;

    /// The extension's table of cues, which is where each name turns into a sound.
    const CUES: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../extension/src/soundCues.ts"));

    fn listed(name: &str) -> bool {
        CUES.contains(&format!("'{name}':")) || CUES.contains(&format!("\n    {name}:"))
    }

    /// A name only one half knows is a sound that never plays: `PlaySound` refuses it,
    /// and the app only logs the refusal.
    #[test]
    fn every_cue_is_one_the_extension_plays() {
        for cue in [Cue::TextCopied, Cue::Copied, Cue::Pinned, Cue::RecordStart, Cue::RecordStop] {
            assert!(listed(&cue.wire()), "{} is not in soundCues.ts", cue.wire());
        }
        // The previews are `shutter-` and one of `SHUTTERS`' keys.
        for kind in ["classic", "pop", "subtle", "8bit", "soft", "system"] {
            assert_eq!(Cue::Shutter(kind).wire(), format!("shutter-{kind}"));
            assert!(listed(kind), "the {kind} shutter is not in soundCues.ts");
        }
    }
}
