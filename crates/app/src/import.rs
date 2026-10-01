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
use tracing::{info, warn};

/// A picture file as a capture. Any format GDK reads; the spool copy is a PNG -- except a
/// GIF (M5), which is copied as it is: a GIF is its frames, and flattening it to the first
/// one would lose exactly what the GIF editor is for.
pub fn import_file(path: &Path) -> Option<CaptureResult> {
    if octosnap_core::history::is_gif(path) {
        return import_gif(path);
    }
    let texture = match gdk::Texture::from_filename(path) {
        Ok(texture) => texture,
        Err(e) => {
            warn!(path = %path.display(), "cannot open the file as an image: {e}");
            return None;
        }
    };
    let title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    import_texture(&texture, &title)
}

/// The clipboard's image as a capture, or `None` when it holds none.
pub async fn import_clipboard() -> Option<CaptureResult> {
    let display = gdk::Display::default()?;
    match display.clipboard().read_texture_future().await {
        Ok(Some(texture)) => import_texture(&texture, "Clipboard"),
        Ok(None) => {
            info!("the clipboard holds no image");
            None
        }
        Err(e) => {
            warn!("could not read an image from the clipboard: {e}");
            None
        }
    }
}

/// A GIF file as a capture: the bytes copied into the spool under a fresh id, its size and
/// length read from its frames. `mode: Record`, so the flow names a save `.gif` and the
/// card is a GIF card -- the badge, Trim, no Pin; `external` still says where it came from,
/// and the history files it as a `File` like any opened picture.
fn import_gif(path: &Path) -> Option<CaptureResult> {
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
    let spool = crate::history::History::default_spool();
    if let Err(e) = std::fs::create_dir_all(&spool) {
        warn!(spool = %spool.display(), "could not create the spool: {e}");
        return None;
    }
    let id = octosnap_core::capture::fresh_id();
    let target = spool.join(format!("{id}.gif"));
    if let Err(e) = std::fs::copy(path, &target) {
        warn!(path = %path.display(), "could not copy the GIF into the spool: {e}");
        return None;
    }
    let title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
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
        source_window: SourceWindow { app_id: String::new(), app_name: String::new(), title: title.clone() },
        window_alpha: false,
        timestamp: glib::real_time().unsigned_abs(),
        confirmed_at: None,
        animation_ms: 0,
        requested_action: None,
        modifiers: 0,
        external: true,
        linebreaks: None,
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

fn import_texture(texture: &gdk::Texture, title: &str) -> Option<CaptureResult> {
    let spool = crate::history::History::default_spool();
    if let Err(e) = std::fs::create_dir_all(&spool) {
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
