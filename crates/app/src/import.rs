// SPDX-License-Identifier: GPL-3.0-or-later

//! Files the user opens with the app, and the clipboard's image (`HIS-04`, `SYS-03`,
//! `spec/07` §3.1's "file" and "clipboard" sources for a pin).
//!
//! Each becomes a capture: copied into the spool under a fresh id with a JSON twin, exactly
//! as the extension writes one, so a card, a pin, the editor and the history treat it as
//! they treat everything else. `CaptureResult::external` is what remembers that it was
//! not a capture -- the history files it as a `File`, and nothing ever moves or trashes
//! the user's original.

use std::path::Path;

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use octosnap_core::{CaptureMode, CaptureResult, Rect, SourceWindow};
use octosnap_shell::{BridgeError, ShellBridge};
use tracing::{info, warn};

/// A picture file as a capture. Any format GDK reads; the spool copy is a PNG -- except a
/// GIF (M5), which is copied as it is: a GIF is its frames, and flattening it to the first
/// one would lose exactly what the GIF editor is for.
pub fn import_file(path: &Path) -> Option<CaptureResult> {
    import_named(path, &file_title(path), &crate::history::History::default_spool())
}

/// What a file is called in the history: its name.
fn file_title(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// [`import_file`] under a title of the caller's, into `spool`.
fn import_named(path: &Path, title: &str, spool: &Path) -> Option<CaptureResult> {
    if octosnap_core::history::is_gif(path) {
        return import_gif(path, title, spool);
    }
    let texture = match gdk::Texture::from_filename(path) {
        Ok(texture) => texture,
        Err(e) => {
            warn!(path = %path.display(), "cannot open the file as an image: {e}");
            return None;
        }
    };
    import_texture(&texture, title, spool)
}

/// The clipboard's image as a capture, or `None` when it holds none.
///
/// Pixels, or else a file: OctoSnap copies a GIF as its file (D165), and Files copies any
/// file that way. A GIF stays one ([`import_file`]), so it opens in the GIF editor rather
/// than as its first frame.
///
/// **Read by the shell** (D169). Mutter offers the selection to the focused client only,
/// and this runs from a shortcut, `octosnap open-from-clipboard` or a link, while another
/// application has the keyboard: read through GDK, an app that had never had a focused
/// window got nothing, and one that had kept the types it was offered then, so a file
/// copied after a picture read as nothing. GDK is the reader only when there is no shell
/// to ask, or one from before D169 (its update waits for a logout, D161).
pub async fn import_clipboard<B: ShellBridge>(bridge: Option<&B>) -> Option<CaptureResult> {
    import_clipboard_into(bridge, &crate::history::History::default_spool()).await
}

async fn import_clipboard_into<B: ShellBridge>(bridge: Option<&B>, spool: &Path) -> Option<CaptureResult> {
    if let Some(bridge) = bridge {
        match through_shell(bridge, spool).await {
            Ok(capture) => return capture,
            Err(e) if gdk_reads_instead(&e) => info!("reading the clipboard through GDK: {e}"),
            Err(e) => {
                warn!("the shell could not read the clipboard: {e}");
                return None;
            }
        }
    }
    through_gdk(spool).await
}

/// Whether GDK reads the clipboard after the shell answered `e`: only when the shell has no
/// such method or is not there. A shell that failed to read it is not second-guessed by a
/// reader that may hold an older selection.
const fn gdk_reads_instead(e: &BridgeError) -> bool {
    e.is_version_mismatch() || e.is_extension_missing()
}

/// The shell's read (`ReadClipboardImage`): a file it wrote beside the spool, imported and
/// then removed, since the spool has its copy and the clipboard's content is not the
/// cache's to keep.
async fn through_shell<B: ShellBridge>(bridge: &B, spool: &Path) -> Result<Option<CaptureResult>, BridgeError> {
    let Some(image) = bridge.read_clipboard_image().await? else {
        info!("the clipboard holds no image");
        return Ok(None);
    };
    let capture = import_named(&image.path, image.name.as_deref().unwrap_or("Clipboard"), spool);
    if let Err(e) = std::fs::remove_file(&image.path) {
        warn!(path = %image.path.display(), "could not remove the clipboard's read: {e}");
    }
    Ok(capture)
}

/// GDK's read, which sees the selection only while one of the app's windows has the
/// keyboard.
async fn through_gdk(spool: &Path) -> Option<CaptureResult> {
    let clipboard = gdk::Display::default()?.clipboard();
    // Another application offers MIME types, not GTypes; the union says which of them GTK
    // can read as what.
    let readable = clipboard.formats().union_deserialize_types();
    if readable.contains_type(gdk::Texture::static_type()) {
        match clipboard.read_texture_future().await {
            Ok(Some(texture)) => return import_texture(&texture, "Clipboard", spool),
            Ok(None) => {}
            Err(e) => warn!("could not read an image from the clipboard: {e}"),
        }
    }
    if let Some(path) = clipboard_file(&clipboard).await {
        return import_named(&path, &file_title(&path), spool);
    }
    info!("the clipboard holds no image");
    None
}

/// The first file on the clipboard, when it holds a list of them: a GIF OctoSnap copied
/// (D165), or whatever was copied in Files.
pub async fn clipboard_file(clipboard: &gdk::Clipboard) -> Option<std::path::PathBuf> {
    if !clipboard.formats().union_deserialize_types().contains_type(gdk::FileList::static_type()) {
        return None;
    }
    match clipboard.read_value_future(gdk::FileList::static_type(), glib::Priority::DEFAULT).await {
        Ok(value) => value.get::<gdk::FileList>().ok()?.files().into_iter().next()?.path(),
        Err(e) => {
            warn!("could not read a file from the clipboard: {e}");
            None
        }
    }
}

/// A GIF file as a capture: the bytes copied into the spool under a fresh id, its size and
/// length read from its frames. `mode: Record`, so the flow names a save `.gif` and the
/// card is a GIF card -- the badge, Trim, no Pin; `external` still says where it came from,
/// and the history files it as a `File` like any opened picture.
fn import_gif(path: &Path, title: &str, spool: &Path) -> Option<CaptureResult> {
    let timeline = match octosnap_media::gif_edit::scan(path) {
        Ok(timeline) if !timeline.is_empty() => timeline,
        Ok(_) => {
            warn!(path = %path.display(), "the GIF has no frames");
            return None;
        }
        Err(e) => {
            warn!(path = %path.display(), "cannot open the file as a GIF: {e}");
            return None;
        }
    };
    if let Err(e) = std::fs::create_dir_all(spool) {
        warn!(spool = %spool.display(), "could not create the spool: {e}");
        return None;
    }
    let id = octosnap_core::capture::fresh_id();
    let target = spool.join(format!("{id}.gif"));
    if let Err(e) = std::fs::copy(path, &target) {
        warn!(path = %path.display(), "could not copy the GIF into the spool: {e}");
        return None;
    }
    let capture = CaptureResult {
        meta_path: target.with_extension("json"),
        path: target,
        mode: CaptureMode::Record,
        rect: Rect::new(
            0,
            0,
            i32::try_from(timeline.width).unwrap_or(i32::MAX),
            i32::try_from(timeline.height).unwrap_or(i32::MAX),
        ),
        scale: 1.0,
        display: String::new(),
        cursor_rect: None,
        source_window: SourceWindow { app_id: String::new(), app_name: String::new(), title: title.to_owned() },
        window_alpha: false,
        timestamp: glib::real_time().unsigned_abs(),
        confirmed_at: None,
        animation_ms: 0,
        requested_action: None,
        modifiers: 0,
        external: true,
        linebreaks: None,
        project: None,
        duration_ms: Some(timeline.duration_ms()),
    };
    write_twin(&capture);
    info!(
        path = %capture.path.display(),
        width = timeline.width,
        height = timeline.height,
        frames = timeline.len(),
        title,
        "imported a GIF into the spool"
    );
    Some(capture)
}

fn import_texture(texture: &gdk::Texture, title: &str, spool: &Path) -> Option<CaptureResult> {
    if let Err(e) = std::fs::create_dir_all(spool) {
        warn!(spool = %spool.display(), "could not create the spool: {e}");
        return None;
    }
    let id = octosnap_core::capture::fresh_id();
    let path = spool.join(format!("{id}.png"));
    if !crate::editor::Editor::encode_png(texture, &path) {
        return None;
    }
    // Logical is physical for a file: it was never on a screen, so its pixels are its size.
    let capture = CaptureResult {
        meta_path: path.with_extension("json"),
        path,
        mode: CaptureMode::Area,
        rect: Rect::new(0, 0, texture.width(), texture.height()),
        scale: 1.0,
        display: String::new(),
        cursor_rect: None,
        source_window: SourceWindow { app_id: String::new(), app_name: String::new(), title: title.to_owned() },
        window_alpha: false,
        timestamp: glib::real_time().unsigned_abs(),
        confirmed_at: None,
        animation_ms: 0,
        requested_action: None,
        modifiers: 0,
        external: true,
        linebreaks: None,
        project: None,
        duration_ms: None,
    };
    write_twin(&capture);
    info!(
        path = %capture.path.display(),
        width = texture.width(),
        height = texture.height(),
        title,
        "imported into the spool"
    );
    Some(capture)
}

/// The JSON twin beside a spool file, exactly as the extension writes one.
pub(crate) fn write_twin(capture: &CaptureResult) {
    match serde_json::to_vec_pretty(capture) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(&capture.meta_path, bytes) {
                warn!(path = %capture.meta_path.display(), "could not write the twin: {e}");
            }
        }
        Err(e) => warn!("could not serialise the twin: {e}"),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use octosnap_shell::{ClipboardImage, NullBridge};

    use super::*;

    /// A fresh context per test, as `flow.rs`'s tests take one: the default context is one
    /// thread's at a time, and cargo runs these in parallel.
    fn run<F: std::future::Future>(future: F) -> F::Output {
        glib::MainContext::new().block_on(future)
    }

    /// Three frames, 48x32, looping: `clipboard-test.sh`'s GIF.
    const GIF: &str = "R0lGODlhMAAgAIEAANwoKP///wAAAAAAACH/C05FVFNDQVBFMi4wAwEAAAAh+QQAHgAAACwAAAAAMAAgAAAIZwABCBxIsKDBgwgTKlzIsKHDhxAjSpxIsaLFixgzatyoMIDHjyADcEwYsuRIhCVDnjyYEuRKgy0/viwY0+NMgjVF3hSYcyfPmj4B9PQ5dGfRm0dnJn25dGXTk0+DSp1KtarVq1iDBgQAIfkEAR4AAgAsAAAAADAAIACBKLQ8////AAAAAAAACGcAAQgcSLCgwYMIEypcyLChw4cQI0qcSLGixYsYM2rciDGAx48gA3BMGLLkSIQlQ548mBLkSoMtP74sGNPjTII1Rd4UmHMnz5o+AfT0OXRn0ZtHZyZ9uXRl05NPg0qdSrWq1atYLwYEACH5BAEeAAIALAAAAAAwACAAgShQ3P///wAAAAAAAAhnAAEIHEiwoMGDCBMqXMiwocOHECNKnEixosWLGDNq3MhRYYCPIEMG6IhQpEmSB02KRGlQZUiWBV2ChElQ5keaA22OxAlAJ8+eNn/65DkUZ1GaR2EmZbkUZVOST39KnUq1qtWrWBMGBAA7";

    /// D169: a GIF file the shell read off the clipboard -- one OctoSnap copied (D165), or
    /// one copied in Files -- comes in as a GIF, under the file's own name, and the shell's
    /// copy goes once the spool has its own.
    ///
    /// GDK is never reached here: no test initialises it, and its first call would panic.
    #[test]
    fn a_gif_the_shell_read_is_imported_under_its_name_and_the_read_removed() {
        let temp = tempfile::tempdir().expect("temp dir");
        let spool = temp.path().join("spool");
        let read = temp.path().join("clipboard-read").join("01M4D4XD98A5CG004H15FTMR0G.gif");
        std::fs::create_dir_all(read.parent().expect("parent")).expect("read dir");
        std::fs::write(&read, glib::base64_decode(GIF)).expect("write the read");
        let bridge = NullBridge::default();
        bridge.set_clipboard_image(Some(ClipboardImage { path: read.clone(), name: Some("Shot 1.gif".to_owned()) }));

        let capture = run(import_clipboard_into(Some(&bridge), &spool)).expect("a capture");

        assert_eq!(bridge.clipboard_reads(), 1);
        assert_eq!(capture.mode, CaptureMode::Record, "a GIF stays a GIF");
        assert_eq!((capture.rect.width, capture.rect.height), (48, 32));
        assert_eq!(capture.source_window.title, "Shot 1.gif");
        assert!(capture.external);
        assert!(capture.path.starts_with(&spool) && capture.path.exists(), "{}", capture.path.display());
        assert_eq!(std::fs::read(&capture.path).expect("spool copy"), glib::base64_decode(GIF));
        assert!(!read.exists(), "the shell's read is removed once the spool has its copy");
    }

    /// The shell said the clipboard holds no image: that is the answer, and GDK, which may
    /// hold an older selection, is not asked instead.
    #[test]
    fn no_image_from_the_shell_is_no_image() {
        let temp = tempfile::tempdir().expect("temp dir");
        let bridge = NullBridge::default();

        assert!(run(import_clipboard_into(Some(&bridge), temp.path())).is_none());
        assert_eq!(bridge.clipboard_reads(), 1);
    }

    /// GDK reads instead only for a shell without the method -- the extension from before
    /// D169 that runs until the logout after an update (D161) -- or no shell at all.
    #[test]
    fn gdk_reads_only_when_the_shell_cannot_be_asked() {
        assert!(gdk_reads_instead(&BridgeError::Unsupported { method: "ReadClipboardImage" }));
        assert!(gdk_reads_instead(&BridgeError::Unavailable("not enabled".to_owned())));
        assert!(!gdk_reads_instead(&BridgeError::MalformedReply {
            method: "ReadClipboardImage",
            detail: "not (ss)".to_owned(),
        }));
        let refused = glib::Error::new(gio::DBusError::Failed, "the clipboard is the app's to read");
        assert!(!gdk_reads_instead(&BridgeError::Dbus(refused)));
    }
}
