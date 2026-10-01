// SPDX-License-Identifier: GPL-3.0-or-later
//! The GIF editor: `spec/04` §1's Trim on a recording card, at GIF size (M5).
//!
//! A preview that plays, a filmstrip to trim on, and three ways out. Deliberately not
//! `spec/06` §5's video editor -- audio, camera, Convert to GIF -- which is M9's and needs
//! a video. What a GIF needs is to be looked at, shortened and made smaller, and this is
//! that: **Space** plays and pauses the kept range on a loop, **←/→** step a frame,
//! **I** and **O** put the start and end marks at the playhead (the editing vocabulary
//! every cutter knows), the filmstrip's two handles do the same with the mouse, and Copy,
//! Save and Save as… write the result. Escape, Ctrl+W and × close to a card holding the
//! trim; Final Close leaves none (D108).
//!
//! **Frames come from `octosnap_media::gif_edit`, composed one at a time and kept as
//! textures in a bounded cache.** A frame-difference GIF cannot be read backwards -- frame
//! *n* only exists as the sum of every frame before it -- so a naive step back re-composes
//! from zero, which on a 150-frame recording is a fifth of a second of nothing happening.
//! The cache turns every frame the editor has already shown into a texture it can put back
//! instantly, which is what makes stepping, scrubbing and looping feel like a video player
//! rather than a slideshow. Playback is scheduled on a **deadline** rather than "this
//! frame's delay from now", so the time spent decoding does not accumulate into a GIF that
//! plays slower than it was recorded.
//!
//! Outputs are rendered on the way out, like the annotation editor's: the kept range is
//! re-encoded through gifski into `<stem>-trim.gif` beside the original in the spool -- the
//! counterpart of the canvas's `-annotated.png`, a derived file with no twin that the
//! history janitor clears after a day -- and that file goes through the capture flow, so
//! the filename template and `last saved` hold and the original is never touched. A whole
//! range at the source's own settings *is* the original, so a look without an edit costs no
//! re-encode and loses no quality.
//!
//! **A recording is read from its reel** (D113): the frames it was taken as, losslessly, at
//! the rate it was taken, each one a PNG of its own. Its GIF has not been written when the
//! editor opens -- that is what lets it open the moment Stop is pressed -- and is written
//! once, at the options the reel says, when an output needs the original; a trim is written
//! from the reel itself, never from a GIF of it, so a recording is quantised once. The
//! options open at what the reel says -- the quality the recording was taken at -- and a
//! close on a trim cuts a reel of its own for the card rather than waiting for gifski.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::gdk;
use octosnap_core::CaptureResult;
use octosnap_media::encoder::Progress;
use octosnap_media::gif_edit::{self, Frames, Timeline, TrimOptions};
use octosnap_media::reel;
use octosnap_media::text;
use tracing::{debug, info, warn};

use crate::editor::actions::{Closed, Done, Outcome, Returned};
use crate::recording::render;

/// The tallest the filmstrip's thumbnails are made, in pixels.
const STRIP_HEIGHT: i32 = 48;
/// How many thumbnails the strip holds, at the fewest and the most. The actual number is
/// the strip's width divided by a thumbnail's, so each one is shown at the GIF's own
/// aspect instead of being cropped to a sliver -- a filmstrip of twenty-four vertical
/// slices of a 16:10 recording tells you nothing about any of them.
const STRIP_THUMBS: std::ops::RangeInclusive<usize> = 4..=40;
/// How close to a handle a press counts as grabbing it rather than scrubbing, in pixels.
const HANDLE_GRAB: f64 = 14.0;
/// How often the export bar reads the encoder's counter. Fast enough to look continuous,
/// slow enough that a short render does not rewrite its label forty times a frame.
const PROGRESS_TICK: Duration = Duration::from_millis(80);
/// How many bytes of composed frames the editor keeps as textures. A 10 s 800x450
/// recording is 216 MB of frames, so a cap is not optional; this holds every frame of the
/// GIFs `spec/06` §9 item 6 budgets for, and the most recent ones of anything longer.
const CACHE_BYTES: usize = 192 * 1024 * 1024;

/// What the editor asks of the flow: the three routes an annotated image takes, and
/// where it goes when the window closes (D108).
pub struct GifEditorActions {
    pub copy: Box<dyn Fn(CaptureResult, Done)>,
    pub save: Box<dyn Fn(CaptureResult, Done)>,
    pub save_as_path: Box<dyn Fn(CaptureResult, PathBuf, Done)>,
    pub closed: Box<dyn Fn(Closed)>,
}

impl std::fmt::Debug for GifEditorActions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GifEditorActions").finish_non_exhaustive()
    }
}

/// The marks: frames `start..end` play and are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Marks {
    start: usize,
    end: usize,
}

/// What a press on the filmstrip is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grab {
    Start,
    End,
    Scrub,
}

/// The composed frames the editor has shown, as textures, oldest dropped first.
///
/// Keyed by frame index, so a frame is composed at most once however often it is shown.
#[derive(Debug, Default)]
struct FrameCache {
    textures: HashMap<usize, gdk::MemoryTexture>,
    order: VecDeque<usize>,
    bytes: usize,
    each: usize,
}

impl FrameCache {
    fn new(width: u32, height: u32) -> Self {
        Self { each: (width as usize * height as usize * 4).max(1), ..Self::default() }
    }

    fn get(&self, index: usize) -> Option<gdk::MemoryTexture> {
        self.textures.get(&index).cloned()
    }

    fn put(&mut self, index: usize, texture: gdk::MemoryTexture) {
        if self.textures.insert(index, texture).is_none() {
            self.order.push_back(index);
            self.bytes += self.each;
        }
        while self.bytes > CACHE_BYTES && self.order.len() > 1 {
            let Some(oldest) = self.order.pop_front() else { break };
            if self.textures.remove(&oldest).is_some() {
                self.bytes = self.bytes.saturating_sub(self.each);
            }
        }
    }
}

pub struct GifEditor {
    window: adw::ApplicationWindow,
    capture: CaptureResult,
    /// What the frames are read from: the recording's reel when it has one, lossless and
    /// at the rate it was taken, else its GIF (D113).
    source: PathBuf,
    /// The options the GIF is written with when nobody changes them: the reel's own, or
    /// for a GIF the defaults. Writing the whole range with these is "unchanged".
    initial: TrimOptions,
    timeline: Timeline,
    frames: RefCell<Frames>,
    cache: RefCell<FrameCache>,
    marks: Cell<Marks>,
    /// The frame the canvas shows.
    shown: Cell<usize>,
    playing: Cell<bool>,
    /// Bumped whenever playback starts or stops, so a tick from an earlier run does
    /// nothing -- the same reason the cards never keep a `SourceId` (D32).
    run: Cell<u64>,
    /// True while a render is on its thread; the outputs are disabled meanwhile.
    rendering: Cell<bool>,
    /// What the press on the filmstrip is moving, while one is down.
    grab: Cell<Option<Grab>>,
    /// How many thumbnails the strip currently holds, and which filling is the live one:
    /// a resize that changes the count leaves an older worker's frames on the way.
    thumbs_shown: Cell<usize>,
    thumbs_run: Cell<u64>,
    /// How the kept range is written out: `spec/08` §5's three GIF settings, here so a
    /// recording taken at the wrong ones can still be fixed.
    options: Cell<TrimOptions>,
    picture: gtk::Picture,
    strip: gtk::DrawingArea,
    thumbs: gtk::Box,
    play_button: gtk::Button,
    time_label: gtk::Label,
    frame_label: gtk::Label,
    range_label: gtk::Label,
    export_button: gtk::MenuButton,
    /// The export's own progress, shown while gifski writes and hidden the rest of the
    /// time. Slid in rather than always present, so a window that is not exporting has no
    /// empty bar in it.
    progress: gtk::ProgressBar,
    progress_row: gtk::Revealer,
    /// Which render the ticker is following, so one that has finished cannot keep the bar
    /// moving for the next (D32's reason for never keeping a `SourceId`).
    render_run: Cell<u64>,
    toasts: adw::ToastOverlay,
    outputs: RefCell<Vec<gtk::Button>>,
    actions: GifEditorActions,
    /// Final Close rather than ×: no card, and the history keeps the GIF (D108).
    final_close: Cell<bool>,
    /// Whether [`GifEditor::hand_back`] has handed the GIF back: the window may go.
    handed_back: Cell<bool>,
    /// True while the trim a close keeps is rendering, which the close waits for.
    keeping: Cell<bool>,
    /// A close asked for while an output was rendering, carried out when it is done.
    pending_close: Cell<bool>,
    /// The last trim an output wrote, and the range and settings it was written at.
    rendered: RefCell<Option<(Marks, TrimOptions, CaptureResult)>>,
}

impl std::fmt::Debug for GifEditor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GifEditor")
            .field("file", &self.capture.path)
            .field("frames", &self.timeline.len())
            .field("marks", &self.marks.get())
            .finish_non_exhaustive()
    }
}

impl GifEditor {
    /// Opens the GIF, playing. `Err` names why it could not be read; the caller tells the
    /// user, and nothing has been shown.
    pub fn open(
        app: &adw::Application,
        capture: &CaptureResult,
        actions: GifEditorActions,
    ) -> Result<Rc<Self>, String> {
        install_gif_editor_css();
        let source = reel::source_of(&capture.path);
        let timeline = gif_edit::scan(&source).map_err(|e| e.to_string())?;
        if timeline.is_empty() {
            return Err("the GIF has no frames".to_owned());
        }
        let frames = gif_edit::Frames::open(&source).map_err(|e| e.to_string())?;
        // A reel says what its GIF is written at: the quality the recording asked for, and
        // after a trim the rate and size it was left at. A GIF says none of that, and gets
        // the settings' quality -- 80, where a recording taken at 100 used to open at 80
        // too (2026-09-24).
        let initial = match reel::Reel::open(&source) {
            Ok(reel) => reel.recorded().gif,
            Err(_) => TrimOptions {
                quality: crate::settings::Settings::load().gif_settings().quality,
                fps: None,
                scale_percent: None,
            },
        };
        let count = timeline.len();
        let (w, h) = (timeline.width, timeline.height);

        // --- the preview, on a surface of its own so a GIF with transparency reads as
        // --- transparent rather than as a hole in the window.
        let picture = gtk::Picture::new();
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        let canvas = gtk::Box::new(gtk::Orientation::Vertical, 0);
        canvas.add_css_class("octosnap-gif-canvas");
        canvas.append(&picture);
        canvas.set_hexpand(true);
        canvas.set_vexpand(true);

        // --- transport: the three buttons a player has, linked, then the clock ---------
        let step_back = icon_button("media-skip-backward-symbolic", "Previous frame (←)");
        let play_button = icon_button("media-playback-pause-symbolic", "Pause (Space)");
        let step_forward = icon_button("media-skip-forward-symbolic", "Next frame (→)");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        buttons.add_css_class("linked");
        for button in [&step_back, &play_button, &step_forward] {
            buttons.append(button);
        }

        let time_label = gtk::Label::new(None);
        time_label.add_css_class("numeric");
        let frame_label = gtk::Label::new(None);
        frame_label.add_css_class("numeric");
        frame_label.add_css_class("dim-label");
        let clock = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        clock.append(&time_label);
        clock.append(&frame_label);

        let transport = gtk::CenterBox::new();
        transport.set_start_widget(Some(&buttons));
        transport.set_end_widget(Some(&clock));

        // --- the filmstrip: thumbnails under a layer that draws the trim ---------------
        let thumbs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        thumbs.set_homogeneous(true);
        thumbs.add_css_class("octosnap-gif-thumbs");
        let strip = gtk::DrawingArea::new();
        strip.set_content_height(STRIP_HEIGHT);
        strip.set_hexpand(true);
        let film = gtk::Overlay::new();
        film.set_child(Some(&thumbs));
        film.add_overlay(&strip);
        film.set_size_request(-1, STRIP_HEIGHT);
        film.add_css_class("octosnap-gif-film");

        // --- what is kept, and the two keyboard ways to say so ------------------------
        let range_label = gtk::Label::new(None);
        range_label.add_css_class("numeric");
        range_label.set_xalign(0.0);
        let set_start = gtk::Button::with_label("Set start");
        set_start.set_tooltip_text(Some("Start the GIF at this frame (I)"));
        let set_end = gtk::Button::with_label("Set end");
        set_end.set_tooltip_text(Some("End the GIF after this frame (O)"));
        let reset = gtk::Button::with_label("Reset");
        reset.set_tooltip_text(Some("The whole GIF again"));
        let marks_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for button in [&set_start, &set_end, &reset] {
            marks_buttons.append(button);
        }
        let marks_row = gtk::CenterBox::new();
        marks_row.set_start_widget(Some(&range_label));
        marks_row.set_end_widget(Some(&marks_buttons));

        let controls = gtk::Box::new(gtk::Orientation::Vertical, 10);
        controls.set_margin_top(12);
        controls.set_margin_bottom(6);
        controls.set_margin_start(12);
        controls.set_margin_end(12);
        controls.append(&transport);
        controls.append(&film);
        controls.append(&marks_row);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.append(&canvas);
        body.append(&controls);

        // Toasts float over the preview, not over the bars, for the editor's reason.
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&body));

        let title = adw::WindowTitle::new(
            "GIF",
            &format!("{w} × {h} · {} · {count} frames", text::duration_label(timeline.duration_ms())),
        );
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&title));

        let export_button = gtk::MenuButton::builder()
            .tooltip_text("How the GIF is written out")
            .build();

        let view = adw::ToolbarView::new();
        view.set_bottom_bar_style(adw::ToolbarStyle::Raised);
        view.add_top_bar(&header);
        view.set_content(Some(&toasts));

        // The GIF at 1:1 where that fits, with room for the controls; never a window
        // wider than a laptop.
        let default_width = (w as i32 + 32).clamp(620, 1280);
        let default_height = (h as i32 + 300).clamp(520, 940);
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("GIF")
            .default_width(default_width)
            .default_height(default_height)
            .content(&view)
            .build();
        window.set_size_request(560, 460);

        let editor = Rc::new(Self {
            window,
            capture: capture.clone(),
            source,
            initial,
            cache: RefCell::new(FrameCache::new(timeline.width, timeline.height)),
            timeline,
            frames: RefCell::new(frames),
            marks: Cell::new(Marks { start: 0, end: count }),
            shown: Cell::new(0),
            playing: Cell::new(false),
            run: Cell::new(0),
            rendering: Cell::new(false),
            grab: Cell::new(None),
            thumbs_shown: Cell::new(0),
            thumbs_run: Cell::new(0),
            options: Cell::new(initial),
            picture,
            strip,
            thumbs,
            play_button,
            time_label,
            frame_label,
            range_label,
            export_button,
            progress: gtk::ProgressBar::builder()
                .show_text(true)
                .text("Rendering\u{2026}")
                .build(),
            progress_row: gtk::Revealer::builder()
                .transition_type(gtk::RevealerTransitionType::SlideDown)
                .transition_duration(120)
                .reveal_child(false)
                .build(),
            render_run: Cell::new(0),
            toasts,
            outputs: RefCell::new(Vec::new()),
            actions,
            final_close: Cell::new(false),
            handed_back: Cell::new(false),
            keeping: Cell::new(false),
            pending_close: Cell::new(false),
            rendered: RefCell::new(None),
        });

        view.add_bottom_bar(&editor.build_bottom_bar());
        editor.wire(&step_back, &step_forward, &set_start, &set_end, &reset);
        editor.wire_strip();
        editor.wire_keys();
        editor.refresh_export();

        // The first frame before the window shows, so it never opens blank.
        editor.seek_to(0);
        editor.refresh_marks();
        editor.window.present();
        editor.play();
        info!(
            file = %capture.path.display(),
            from = %editor.source.display(),
            frames = count,
            fps = editor.source_fps(),
            quality = editor.initial.quality,
            window = editor.object_path().unwrap_or_default(),
            "GIF editor opened"
        );
        Ok(editor)
    }

    #[must_use]
    pub fn window(&self) -> &adw::ApplicationWindow {
        &self.window
    }

    /// The window's D-Bus object path, logged at open for the same reason the canvas
    /// editor logs its own: a harness reads the window's geometry from the compositor
    /// with `MoveWindowBy(path, 0, 0)` instead of guessing from a screenshot.
    #[must_use]
    pub fn object_path(&self) -> Option<String> {
        let app = self.window.application()?;
        let base = app.dbus_object_path()?;
        Some(format!("{base}/window/{}", self.window.id()))
    }

    // --- building ---------------------------------------------------------------

    /// The export settings on the left and the ways out on the right, wrapping onto a
    /// second line in a narrow window the way the annotation editor's bar does (D108).
    fn build_bottom_bar(self: &Rc<Self>) -> gtk::Widget {
        let bar = adw::WrapBox::new();
        bar.set_child_spacing(8);
        bar.set_line_spacing(6);
        bar.set_justify(adw::JustifyMode::Spread);
        bar.set_justify_last_line(true);
        bar.set_margin_start(12);
        bar.set_margin_end(12);
        bar.set_margin_top(6);
        bar.set_margin_bottom(6);
        self.export_button.set_popover(Some(&self.build_export_popover()));
        bar.append(&self.export_button);

        let end = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        end.set_halign(gtk::Align::End);
        let final_close = gtk::Button::with_label("Final Close");
        final_close.set_tooltip_text(Some(
            "Close without leaving a preview; the GIF goes to the history (Ctrl+Shift+W)",
        ));
        {
            let editor = Rc::downgrade(self);
            final_close.connect_clicked(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.close(true);
                }
            });
        }
        let save_as = gtk::Button::with_label("Save as…");
        save_as.set_tooltip_text(Some("Choose where to put it (Ctrl+Shift+S)"));
        {
            let editor = Rc::downgrade(self);
            save_as.connect_clicked(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.save_as();
                }
            });
        }
        let copy = gtk::Button::with_label("Copy");
        copy.set_tooltip_text(Some("Copy the GIF (Ctrl+C)"));
        {
            let editor = Rc::downgrade(self);
            copy.connect_clicked(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.copy();
                }
            });
        }
        let save = gtk::Button::with_label("Save");
        save.add_css_class("suggested-action");
        save.set_tooltip_text(Some("Save to the recording folder (Ctrl+S)"));
        {
            let editor = Rc::downgrade(self);
            save.connect_clicked(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.save();
                }
            });
        }
        // Apart from the outputs: it leaves the editor, they do not (`spec/13` #4).
        final_close.set_margin_end(12);
        for button in [&final_close, &save_as, &copy, &save] {
            end.append(button);
        }
        *self.outputs.borrow_mut() = vec![save_as, copy, save];
        bar.append(&end);

        // Above the buttons, across the window: an export bar, where the eye already is
        // when the three buttons it belongs to have just gone grey.
        self.progress.set_margin_start(12);
        self.progress.set_margin_end(12);
        self.progress.set_margin_top(8);
        self.progress_row.set_child(Some(&self.progress));
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&self.progress_row);
        column.append(&bar);
        column.upcast()
    }

    /// `spec/08` §5's three GIF settings, on the GIF itself.
    ///
    /// The Recording page decides what a *new* recording is taken at; these decide what
    /// this one is written out as, which is the only half that can still be changed. A
    /// frame rate and a size can always come down -- and between them they decide whether
    /// a GIF is twelve megabytes or two -- so they are here, where the user is looking at
    /// the GIF that is too big, rather than only in a dialog two clicks away.
    fn build_export_popover(self: &Rc<Self>) -> gtk::Popover {
        let popover = gtk::Popover::new();
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        list.add_css_class("boxed-list");
        list.set_size_request(320, -1);

        let source_fps = self.source_fps();
        let choices = fps_choices(source_fps);
        let fps_labels: Vec<String> = std::iter::once(format!("Unchanged ({source_fps} fps)"))
            .chain(choices.iter().map(|fps| format!("{fps} fps")))
            .collect();
        let fps = adw::ComboRow::builder()
            .title("Frame rate")
            .subtitle("Fewer frames a second is a smaller file")
            .model(&gtk::StringList::new(
                &fps_labels.iter().map(String::as_str).collect::<Vec<_>>(),
            ))
            .build();
        // Where the options already are: a reel cut on a close keeps the rate it was left at.
        let at = |choice: Option<usize>| choice.map_or(0, |i| u32::try_from(i + 1).unwrap_or(0));
        let options = self.options.get();
        fps.set_selected(at(options.fps.and_then(|f| choices.iter().position(|&c| c == f))));
        {
            let editor = Rc::downgrade(self);
            fps.connect_selected_notify(move |row| {
                let Some(editor) = editor.upgrade() else { return };
                let mut options = editor.options.get();
                options.fps = choices.get(row.selected().saturating_sub(1) as usize).copied();
                editor.options.set(options);
                editor.refresh_export();
            });
        }
        list.append(&fps);

        let (w, h) = (self.timeline.width, self.timeline.height);
        let size_labels: Vec<String> = std::iter::once(format!("Unchanged ({w} × {h})"))
            .chain(SCALE_CHOICES.iter().map(|percent| {
                let (sw, sh) = scaled(w, h, *percent);
                format!("{percent}% ({sw} × {sh})")
            }))
            .collect();
        let size = adw::ComboRow::builder()
            .title("Size")
            .subtitle("A GIF can be made smaller, never sharper")
            .model(&gtk::StringList::new(
                &size_labels.iter().map(String::as_str).collect::<Vec<_>>(),
            ))
            .build();
        let scale = options.scale_percent;
        size.set_selected(at(scale.and_then(|p| SCALE_CHOICES.iter().position(|&c| c == p))));
        {
            let editor = Rc::downgrade(self);
            size.connect_selected_notify(move |row| {
                let Some(editor) = editor.upgrade() else { return };
                let mut options = editor.options.get();
                options.scale_percent =
                    SCALE_CHOICES.get(row.selected().saturating_sub(1) as usize).copied();
                editor.options.set(options);
                editor.refresh_export();
            });
        }
        list.append(&size);

        let quality = adw::SpinRow::builder()
            .title("Quality")
            .subtitle("gifski's palette effort, 1 to 100")
            .adjustment(&gtk::Adjustment::new(
                f64::from(self.options.get().quality),
                1.0,
                100.0,
                1.0,
                10.0,
                0.0,
            ))
            .build();
        {
            let editor = Rc::downgrade(self);
            quality.connect_value_notify(move |row| {
                let Some(editor) = editor.upgrade() else { return };
                let mut options = editor.options.get();
                options.quality = (row.value().round() as i64).clamp(1, 100) as u8;
                editor.options.set(options);
                editor.refresh_export();
            });
        }
        list.append(&quality);

        let box_ = gtk::Box::new(gtk::Orientation::Vertical, 8);
        box_.set_margin_top(8);
        box_.set_margin_bottom(8);
        box_.set_margin_start(8);
        box_.set_margin_end(8);
        box_.append(&list);
        let note = gtk::Label::new(Some(
            "Defaults for new recordings live in Settings → Recording.",
        ));
        note.add_css_class("dim-label");
        note.add_css_class("caption");
        note.set_wrap(true);
        note.set_xalign(0.0);
        box_.append(&note);
        popover.set_child(Some(&box_));
        popover
    }

    /// How many thumbnails a strip `width` wide should hold: as many as fit at the GIF's
    /// own aspect, never more than it has frames.
    fn wanted_thumbs(&self, width: f64) -> usize {
        let (w, h) = (f64::from(self.timeline.width.max(1)), f64::from(self.timeline.height.max(1)));
        let each = (f64::from(STRIP_HEIGHT) * w / h).max(8.0);
        let fits = (width / each).round().max(1.0) as usize;
        fits.clamp(*STRIP_THUMBS.start(), *STRIP_THUMBS.end()).min(self.timeline.len().max(1))
    }

    /// Lays `count` empty places out and starts the worker that fills them.
    ///
    /// On a worker because composing a 150-frame GIF takes a fifth of a second, and the
    /// window is already up and playing by then: the strip fills in a moment later rather
    /// than the editor opening a moment late.
    fn fill_thumbs(self: &Rc<Self>, count: usize) {
        self.thumbs_shown.set(count);
        let run = self.thumbs_run.get() + 1;
        self.thumbs_run.set(run);
        while let Some(child) = self.thumbs.first_child() {
            self.thumbs.remove(&child);
        }
        for _ in 0..count {
            let picture = gtk::Picture::new();
            picture.set_can_shrink(true);
            picture.set_content_fit(gtk::ContentFit::Cover);
            picture.set_hexpand(true);
            self.thumbs.append(&picture);
        }

        let frames = self.timeline.len();
        // The frames the editor plays: a recording's reel until its GIF is written (D113).
        let path = self.source.clone();
        let (w, h) = (self.timeline.width.max(1), self.timeline.height.max(1));
        let height = u32::try_from(STRIP_HEIGHT).unwrap_or(48);
        let width = ((u64::from(w) * u64::from(height) / u64::from(h)) as u32).clamp(8, 480);
        let editor = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let decoded = gio::spawn_blocking(move || {
                thumbnails(&path, frames, count, width, height)
            })
            .await;
            let Some(editor) = editor.upgrade() else { return };
            // A resize asked for a different number while this one was decoding: its
            // frames are the wrong ones for the places now in the strip.
            if editor.thumbs_run.get() != run {
                return;
            }
            let Ok(thumbs) = decoded else {
                warn!("the filmstrip's thumbnails could not be decoded");
                return;
            };
            let mut child = editor.thumbs.first_child();
            for bytes in &thumbs {
                let Some(picture) = child.and_then(|c| c.downcast::<gtk::Picture>().ok()) else {
                    break;
                };
                let texture = gdk::MemoryTexture::new(
                    width as i32,
                    height as i32,
                    gdk::MemoryFormat::R8g8b8a8,
                    &glib::Bytes::from(bytes),
                    width as usize * 4,
                );
                picture.set_paintable(Some(&texture));
                child = picture.next_sibling();
            }
            debug!(thumbs = thumbs.len(), "the filmstrip is drawn");
        });
    }

    fn wire(
        self: &Rc<Self>,
        step_back: &gtk::Button,
        step_forward: &gtk::Button,
        set_start: &gtk::Button,
        set_end: &gtk::Button,
        reset: &gtk::Button,
    ) {
        let on = |button: &gtk::Button, f: fn(&Rc<Self>)| {
            let editor = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(editor) = editor.upgrade() {
                    f(&editor);
                }
            });
        };
        on(step_back, |e| e.step(-1));
        on(step_forward, |e| e.step(1));
        on(set_start, Self::mark_start);
        on(set_end, Self::mark_end);
        on(reset, Self::reset_marks);
        on(&self.play_button, Self::toggle_play);

        {
            // What the window leaves behind (D108). A trim is rendered first, and gifski
            // takes seconds over it, so the first request is refused while it runs and
            // the window closes itself once the render is in. Closing stops the ticker;
            // `hold_until_closed` releases the editor after -- its handler only runs once
            // this one lets the close through.
            let editor = Rc::downgrade(self);
            self.window.connect_close_request(move |_| {
                let Some(editor) = editor.upgrade() else { return glib::Propagation::Proceed };
                if editor.hand_back() {
                    editor.pause();
                    glib::Propagation::Proceed
                } else {
                    glib::Propagation::Stop
                }
            });
        }
    }

    /// The filmstrip draws the trim, and takes it: a press on a handle moves that mark,
    /// a press anywhere else scrubs.
    fn wire_strip(self: &Rc<Self>) {
        {
            let editor = Rc::downgrade(self);
            self.strip.set_draw_func(move |area, cr, width, height| {
                if let Some(editor) = editor.upgrade() {
                    editor.draw_strip(area, cr, f64::from(width), f64::from(height));
                }
            });
        }
        let drag = gtk::GestureDrag::new();
        {
            let editor = Rc::downgrade(self);
            drag.connect_drag_begin(move |gesture, x, _| {
                let Some(editor) = editor.upgrade() else { return };
                let width = f64::from(editor.strip.width()).max(1.0);
                let marks = editor.marks.get();
                let start_x = editor.x_of(marks.start, width);
                let end_x = editor.x_of(marks.end, width);
                let grab = if (x - start_x).abs() <= HANDLE_GRAB {
                    Grab::Start
                } else if (x - end_x).abs() <= HANDLE_GRAB {
                    Grab::End
                } else {
                    Grab::Scrub
                };
                editor.grab.set(Some(grab));
                gesture.set_state(gtk::EventSequenceState::Claimed);
                editor.drag_to(x, width);
            });
        }
        {
            let editor = Rc::downgrade(self);
            drag.connect_drag_update(move |gesture, dx, _| {
                let Some(editor) = editor.upgrade() else { return };
                let Some((start, _)) = gesture.start_point() else { return };
                let width = f64::from(editor.strip.width()).max(1.0);
                editor.drag_to(start + dx, width);
            });
        }
        {
            let editor = Rc::downgrade(self);
            drag.connect_drag_end(move |_, _, _| {
                if let Some(editor) = editor.upgrade() {
                    editor.grab.set(None);
                }
            });
        }
        self.strip.add_controller(drag);
    }

    /// The keys, on the window in the capture phase so Space plays even when a button
    /// has the focus -- a focused Save that took Space as a click would write a file.
    ///
    /// Except in the Quality field: its arrows, Home and End move its cursor, and Ctrl+C
    /// copies what is selected in it. Only the window's own Save, Save As and Close reach
    /// past it, and Esc leaves the field before a second one closes the window.
    fn wire_keys(self: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let editor = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(editor) = editor.upgrade() else { return glib::Propagation::Proceed };
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            let typing = gtk::prelude::RootExt::focus(&editor.window).is_some_and(|f| f.is::<gtk::Text>());
            if typing {
                match key {
                    gdk::Key::s | gdk::Key::S if ctrl && shift => editor.save_as(),
                    gdk::Key::s | gdk::Key::S if ctrl => editor.save(),
                    gdk::Key::w | gdk::Key::W if ctrl => editor.close(shift),
                    gdk::Key::Escape => gtk::prelude::GtkWindowExt::set_focus(&editor.window, None::<&gtk::Widget>),
                    _ => return glib::Propagation::Proceed,
                }
                return glib::Propagation::Stop;
            }
            match key {
                gdk::Key::space => editor.toggle_play(),
                gdk::Key::Left => editor.step(-1),
                gdk::Key::Right => editor.step(1),
                gdk::Key::Home => editor.jump(editor.marks.get().start),
                gdk::Key::End => editor.jump(editor.marks.get().end.saturating_sub(1)),
                gdk::Key::i | gdk::Key::I if !ctrl => editor.mark_start(),
                gdk::Key::o | gdk::Key::O if !ctrl => editor.mark_end(),
                gdk::Key::s | gdk::Key::S if ctrl && shift => editor.save_as(),
                gdk::Key::s | gdk::Key::S if ctrl => editor.save(),
                gdk::Key::c | gdk::Key::C if ctrl => editor.copy(),
                gdk::Key::w | gdk::Key::W if ctrl => editor.close(shift),
                gdk::Key::Escape => editor.close(false),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.window.add_controller(keys);
    }

    // --- the filmstrip ------------------------------------------------------------

    /// Where frame `index` sits along a strip `width` wide.
    fn x_of(&self, index: usize, width: f64) -> f64 {
        let count = self.timeline.len().max(1) as f64;
        (index as f64 / count) * width
    }

    /// The frame under `x`.
    fn frame_at(&self, x: f64, width: f64) -> usize {
        let count = self.timeline.len().max(1);
        let index = (x / width.max(1.0) * count as f64).floor();
        (index.max(0.0) as usize).min(count - 1)
    }

    fn draw_strip(self: &Rc<Self>, area: &gtk::DrawingArea, cr: &gtk::cairo::Context, width: f64, height: f64) {
        // The strip's width is not known until it is drawn, and it changes with the
        // window. Refilling from an idle rather than from here: a draw callback must not
        // add and remove the widgets underneath it.
        let wanted = self.wanted_thumbs(width);
        if wanted != self.thumbs_shown.get() {
            self.thumbs_shown.set(wanted);
            let editor = Rc::downgrade(self);
            glib::idle_add_local_once(move || {
                if let Some(editor) = editor.upgrade() {
                    editor.fill_thumbs(wanted);
                }
            });
        }
        let marks = self.marks.get();
        let (start, end) = (self.x_of(marks.start, width), self.x_of(marks.end, width));

        // What is thrown away is dimmed, so the kept range is what the eye lands on.
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.62);
        cr.rectangle(0.0, 0.0, start, height);
        cr.rectangle(end, 0.0, width - end, height);
        let _ = cr.fill();

        let accent = adw::StyleManager::default().accent_color_rgba();
        let (r, g, b) = (
            f64::from(accent.red()),
            f64::from(accent.green()),
            f64::from(accent.blue()),
        );
        // The kept range, ringed; the two handles are the ring's ends, made grabbable.
        cr.set_line_width(2.0);
        cr.set_source_rgba(r, g, b, 1.0);
        cr.rectangle(start + 1.0, 1.0, (end - start - 2.0).max(1.0), height - 2.0);
        let _ = cr.stroke();
        for (x, side) in [(start, 1.0), (end, -1.0)] {
            cr.set_source_rgba(r, g, b, 1.0);
            cr.rectangle(x - (1.0 - side) * 5.0, 0.0, 5.0, height);
            let _ = cr.fill();
            // A grip line, so a handle looks like something to take hold of.
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.85);
            cr.rectangle(x - (1.0 - side) * 5.0 + 2.0, height / 2.0 - 6.0, 1.0, 12.0);
            let _ = cr.fill();
        }

        // The playhead, with a head: a bare line is lost against a busy thumbnail.
        let at = self.x_of(self.shown.get(), width) + self.x_of(1, width) / 2.0;
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
        cr.rectangle(at - 1.0, 0.0, 2.0, height);
        let _ = cr.fill();
        cr.move_to(at - 5.0, 0.0);
        cr.line_to(at + 5.0, 0.0);
        cr.line_to(at, 6.0);
        cr.close_path();
        let _ = cr.fill();
        let _ = area;
    }

    /// A press or a drag at `x`: the grabbed handle follows it, or the playhead does.
    fn drag_to(&self, x: f64, width: f64) {
        let frame = self.frame_at(x, width);
        match self.grab.get() {
            Some(Grab::Start) => {
                let mut marks = self.marks.get();
                marks.start = frame.min(marks.end.saturating_sub(1));
                self.marks.set(marks);
                self.pause();
                self.seek_to(marks.start);
                self.refresh_marks();
            }
            Some(Grab::End) => {
                let mut marks = self.marks.get();
                // `end` is exclusive, so the frame under the pointer is kept.
                marks.end = (frame + 1).max(marks.start + 1).min(self.timeline.len());
                self.marks.set(marks);
                self.pause();
                self.seek_to(marks.end - 1);
                self.refresh_marks();
            }
            _ => {
                // Scrubbing is looking, so it pauses.
                self.pause();
                self.seek_to(frame);
            }
        }
    }

    // --- looking ------------------------------------------------------------------

    /// Shows frame `index`: from the cache if it has been composed before, otherwise the
    /// reader composes it and the cache keeps it.
    fn seek_to(&self, index: usize) {
        let index = index.min(self.timeline.len().saturating_sub(1));
        if let Some(texture) = self.cache.borrow().get(index) {
            self.picture.set_paintable(Some(&texture));
            self.after_seek(index);
            return;
        }
        let texture = {
            let mut frames = self.frames.borrow_mut();
            match frames.seek(index) {
                Ok(Some(_)) => {
                    let (w, h) = (frames.width(), frames.height());
                    let bytes = glib::Bytes::from(frames.bytes());
                    Some(gdk::MemoryTexture::new(
                        w as i32,
                        h as i32,
                        gdk::MemoryFormat::R8g8b8a8,
                        &bytes,
                        w as usize * 4,
                    ))
                }
                Ok(None) => None,
                Err(e) => {
                    warn!(file = %self.source.display(), "could not read frame {index}: {e}");
                    None
                }
            }
        };
        let Some(texture) = texture else { return };
        self.picture.set_paintable(Some(&texture));
        self.cache.borrow_mut().put(index, texture);
        self.after_seek(index);
    }

    /// The labels and the strip, after the canvas changed.
    fn after_seek(&self, index: usize) {
        self.shown.set(index);
        let start_ms = self.timeline.frames.get(index).map_or(0, |f| f.start_ms);
        self.time_label.set_text(&format!(
            "{} / {}",
            tenths(start_ms),
            tenths(self.timeline.duration_ms())
        ));
        self.frame_label.set_text(&format!("frame {} / {}", index + 1, self.timeline.len()));
        self.strip.queue_draw();
    }

    fn step(&self, by: i64) {
        self.pause();
        let next = (self.shown.get() as i64 + by).clamp(0, self.timeline.len() as i64 - 1);
        self.seek_to(next as usize);
    }

    fn jump(&self, index: usize) {
        self.pause();
        self.seek_to(index);
    }

    fn toggle_play(self: &Rc<Self>) {
        if self.playing.get() {
            self.pause();
        } else {
            self.play();
        }
    }

    fn play(self: &Rc<Self>) {
        let marks = self.marks.get();
        // Outside the marks, playing starts from the start mark: that is the GIF that
        // will be written, and what the loop is for.
        if self.shown.get() < marks.start || self.shown.get() >= marks.end {
            self.seek_to(marks.start);
        }
        self.playing.set(true);
        self.play_button.set_icon_name("media-playback-pause-symbolic");
        self.play_button.set_tooltip_text(Some("Pause (Space)"));
        let run = self.run.get() + 1;
        self.run.set(run);
        let deadline = Instant::now() + Duration::from_millis(self.delay_of(self.shown.get()));
        self.schedule_tick(run, deadline);
    }

    fn pause(&self) {
        if !self.playing.replace(false) {
            return;
        }
        self.run.set(self.run.get() + 1);
        self.play_button.set_icon_name("media-playback-start-symbolic");
        self.play_button.set_tooltip_text(Some("Play (Space)"));
    }

    fn delay_of(&self, index: usize) -> u64 {
        self.timeline.frames.get(index).map_or(100, |f| f.delay_ms.max(20))
    }

    /// The next frame at its **deadline**, not "its delay from now".
    ///
    /// Composing a frame that is not in the cache yet takes a millisecond or two, and an
    /// editor that added that to every delay played a 15 fps recording at 14 -- slowly and
    /// visibly wrongly on a loop. Counting from the deadline spends the decode out of the
    /// frame's own time instead. A run that has fallen more than a frame behind (a stalled
    /// decode, a laptop waking up) starts counting again from now rather than racing to
    /// catch up.
    fn schedule_tick(self: &Rc<Self>, run: u64, deadline: Instant) {
        let now = Instant::now();
        let wait = deadline.saturating_duration_since(now);
        let editor = Rc::downgrade(self);
        glib::timeout_add_local_once(wait, move || {
            let Some(editor) = editor.upgrade() else { return };
            if !editor.playing.get() || editor.run.get() != run {
                return;
            }
            let marks = editor.marks.get();
            let mut next = editor.shown.get() + 1;
            if next >= marks.end {
                next = marks.start;
            }
            editor.seek_to(next);
            let next_deadline = deadline + Duration::from_millis(editor.delay_of(next));
            let floor = Instant::now() + Duration::from_millis(2);
            editor.schedule_tick(run, next_deadline.max(floor));
        });
    }

    // --- marking -----------------------------------------------------------------

    fn mark_start(self: &Rc<Self>) {
        let mut marks = self.marks.get();
        marks.start = self.shown.get();
        if marks.end <= marks.start {
            marks.end = (marks.start + 1).min(self.timeline.len());
        }
        self.marks.set(marks);
        self.refresh_marks();
    }

    fn mark_end(self: &Rc<Self>) {
        let mut marks = self.marks.get();
        marks.end = self.shown.get() + 1;
        if marks.start >= marks.end {
            marks.start = marks.end - 1;
        }
        self.marks.set(marks);
        self.refresh_marks();
    }

    fn reset_marks(self: &Rc<Self>) {
        self.marks.set(Marks { start: 0, end: self.timeline.len() });
        self.refresh_marks();
    }

    fn is_whole(&self) -> bool {
        self.marks.get() == Marks { start: 0, end: self.timeline.len() }
    }

    fn refresh_marks(&self) {
        let marks = self.marks.get();
        let length = self.timeline.duration_of(marks.start..marks.end);
        let frames = marks.end - marks.start;
        let start = self.timeline.frames.get(marks.start).map_or(0, |f| f.start_ms);
        self.range_label.set_text(&if self.is_whole() {
            format!("Whole GIF · {} · {frames} frames", text::duration_label(length))
        } else {
            format!(
                "{} → {} · {} · {frames} frames",
                tenths(start),
                tenths(start + length),
                text::duration_label(length)
            )
        });
        self.strip.queue_draw();
        debug!(start = marks.start, end = marks.end, "marks");
    }

    /// The source's own frame rate, as the label that says what "unchanged" means
    /// ([`Timeline::rate`]): the rate a reel was taken at, or the rate a GIF's delays keep
    /// while something moves in it.
    fn source_fps(&self) -> u32 {
        self.timeline.rate()
    }

    fn refresh_export(&self) {
        let options = self.options.get();
        let fps = match options.fps {
            Some(fps) => format!("{fps} fps"),
            None => format!("{} fps", self.source_fps()),
        };
        let size = match options.scale_percent.filter(|&p| p < 100) {
            Some(percent) => {
                let (w, h) = scaled(self.timeline.width, self.timeline.height, percent);
                format!("{w} × {h}")
            }
            None => format!("{} × {}", self.timeline.width, self.timeline.height),
        };
        self.export_button.set_label(&format!("{fps} · {size} · quality {}", options.quality));
    }

    // --- the ways out ------------------------------------------------------------

    /// The GIF as marked and as the export settings ask for: the original when nothing
    /// was changed, otherwise `-trim.gif` beside it in the spool, encoded on its own
    /// thread. `Err` is a sentence for the user.
    ///
    /// An original that is still frames is written first, once, at its own settings --
    /// the same render a card's Save would have started, and joined if it has (D113).
    async fn render(self: &Rc<Self>, progress: &Progress) -> Result<CaptureResult, String> {
        if self.unchanged() {
            if let Some(writing) = render::start(&self.capture.path) {
                self.follow(writing);
            }
            render::ensure(&self.capture.path).await?;
            return Ok(self.capture.clone());
        }
        let (marks, options) = (self.marks.get(), self.options.get());
        let capture = self.render_to(trimmed_path(&self.capture.path), progress).await?;
        *self.rendered.borrow_mut() = Some((marks, options, capture.clone()));
        Ok(capture)
    }

    /// The trim an output already wrote of exactly this range at these settings, linked
    /// to `out` rather than encoded again: Copy then × is the usual way out, and gifski
    /// takes seconds over a trim.
    fn reuse_render(&self, out: &std::path::Path) -> Option<CaptureResult> {
        let (marks, options, capture) = self.rendered.borrow().clone()?;
        if marks != self.marks.get() || options != self.options.get() || !capture.path.is_file()
        {
            return None;
        }
        if std::fs::hard_link(&capture.path, out).is_err() {
            std::fs::copy(&capture.path, out).ok()?;
        }
        debug!(from = %capture.path.display(), "the close keeps the trim an output wrote");
        let meta_path = out.with_extension("json");
        Some(CaptureResult { meta_path, path: out.to_path_buf(), ..capture })
    }

    /// Whether what would be written is the original: the whole range, at the options it
    /// is written with anyway. The quality counts -- a GIF asked for at another quality is
    /// encoded again, not handed back as it was.
    fn unchanged(&self) -> bool {
        self.is_whole() && self.options.get() == self.initial
    }

    /// The kept range, encoded to `out` on its own thread, described as the capture.
    async fn render_to(
        self: &Rc<Self>,
        out: PathBuf,
        progress: &Progress,
    ) -> Result<CaptureResult, String> {
        let options = self.options.get();
        let marks = self.marks.get();
        let source = self.source.clone();
        let range = marks.start..marks.end;
        info!(
            file = %source.display(),
            start = marks.start,
            end = marks.end,
            fps = options.fps,
            scale = options.scale_percent,
            quality = options.quality,
            "rendering the GIF"
        );

        let watched = progress.clone();
        let written = gio::spawn_blocking(move || {
            gif_edit::trim_reporting(&source, range, options, &out, &watched)
        })
        .await
        .map_err(|_| "the encoder thread panicked".to_owned())?
        .map_err(|e| e.to_string())?;

        // No twin here, on purpose: a twin makes a spool file a capture the janitor files
        // into the history, and an output's trim is derived from one already there. The
        // trim a close keeps writes its own (`hand_back`).
        let mut capture = self.capture.clone();
        capture.meta_path = written.path.with_extension("json");
        capture.path = written.path;
        capture.duration_ms = Some(u64::try_from(written.duration.as_millis()).unwrap_or(u64::MAX));
        info!(
            path = %capture.path.display(),
            frames = written.frames,
            bytes = written.bytes,
            "GIF rendered"
        );
        Ok(capture)
    }

    fn set_rendering(&self, on: bool) {
        self.rendering.set(on);
        for button in self.outputs.borrow().iter() {
            button.set_sensitive(!on);
        }
        self.export_button.set_sensitive(!on);
        if !on {
            // The run number moves on whether the render succeeded or failed, which is
            // what stops the ticker either way.
            self.render_run.set(self.render_run.get() + 1);
            self.progress_row.set_reveal_child(false);
            if self.pending_close.replace(false) {
                // A turn later, so the output's own reply has landed first.
                let window = self.window.downgrade();
                glib::idle_add_local_once(move || {
                    if let Some(window) = window.upgrade() {
                        window.close();
                    }
                });
            }
        }
    }

    /// × or Ctrl+W (`final_close` false), or Final Close and Ctrl+Shift+W (true).
    fn close(self: &Rc<Self>, final_close: bool) {
        self.final_close.set(final_close);
        self.window.close();
    }

    /// What the window leaves behind (D108), from its close-request. True when the window
    /// may go now; false while the trim it keeps is rendering, after which it closes
    /// itself.
    ///
    /// An untouched GIF goes back as the capture it was. A trimmed one goes back as the
    /// trim, rendered into the spool as a capture of its own -- `<id>.gif` with a twin,
    /// not the `-trim.gif` an output writes beside the original -- because from here on
    /// it is one: a card shows it, and the history files it when the card goes.
    fn hand_back(self: &Rc<Self>) -> bool {
        if self.handed_back.get() {
            return true;
        }
        if self.rendering.get() {
            if !self.keeping.get() {
                self.pending_close.set(true);
                self.toast("Closing once the render is done\u{2026}");
            }
            return false;
        }
        if self.unchanged() {
            self.handed_back.set(true);
            let returned = Returned::Unchanged;
            (self.actions.closed)(self.closed_as(returned));
            return true;
        }
        let spool = crate::history::History::default_spool();
        if let Err(e) = std::fs::create_dir_all(&spool) {
            warn!(spool = %spool.display(), "could not create the spool: {e}");
            self.toast("The trim could not be kept: the spool is not writable");
            return false;
        }
        let out = spool.join(format!("{}.gif", octosnap_core::capture::fresh_id()));
        // A reel is cut rather than written: a close that waited for gifski over a
        // full-size recording would wait a minute (D113). The cut is linked frames and a
        // `recording.json` with the settings the editor was left at, and its GIF is
        // written when something asks for it -- unless an output already wrote this very
        // trim, which is then the cut's GIF from the start.
        if reel::is_reel(&self.source) {
            match self.cut(&out) {
                Ok(capture) => {
                    let capture = self.reuse_render(&out).unwrap_or(capture);
                    self.keep(capture);
                    return true;
                }
                Err(e) => {
                    warn!("the trim a close keeps could not be cut: {e}");
                    self.toast(&format!("The trim could not be kept: {e}"));
                    return false;
                }
            }
        }
        if let Some(capture) = self.reuse_render(&out) {
            self.keep(capture);
            return true;
        }
        self.keeping.set(true);
        self.set_rendering(true);
        let progress = Progress::new();
        self.follow(progress.clone());
        let editor = Rc::clone(self);
        glib::spawn_future_local(async move {
            let rendered = editor.render_to(out, &progress).await;
            editor.keeping.set(false);
            editor.set_rendering(false);
            match rendered {
                Ok(capture) => {
                    editor.keep(capture);
                    editor.window.close();
                }
                Err(e) => {
                    warn!("the trim a close keeps failed: {e}");
                    editor.toast(&format!("The trim could not be kept: {e}"));
                }
            }
        });
        false
    }

    /// The kept range of the reel, cut to a reel of its own beside `out` and described as
    /// the capture `out` will be once its GIF is written.
    fn cut(&self, out: &std::path::Path) -> Result<CaptureResult, String> {
        let from = reel::Reel::open(&self.source).map_err(|e| e.to_string())?;
        let marks = self.marks.get();
        let options = self.options.get();
        let cut = reel::cut(&from, marks.start..marks.end, options, &reel::beside(out))
            .map_err(|e| e.to_string())?;
        info!(
            from = %self.source.display(),
            to = %cut.dir().display(),
            frames = cut.len(),
            "the trim a close keeps is a reel of its own"
        );
        let mut capture = self.capture.clone();
        capture.path = out.to_path_buf();
        capture.meta_path = out.with_extension("json");
        capture.duration_ms = Some(cut.timeline().duration_ms());
        Ok(capture)
    }

    /// Hands the trim back as a capture of its own: its twin written, and nothing about
    /// it that would make a card treat it as just taken.
    fn keep(&self, mut capture: CaptureResult) {
        capture.confirmed_at = None;
        capture.animation_ms = 0;
        capture.requested_action = None;
        crate::import::write_twin(&capture);
        self.handed_back.set(true);
        (self.actions.closed)(self.closed_as(Returned::Rendered(Box::new(capture))));
    }

    /// `returned`, as the way out that was taken.
    fn closed_as(&self, returned: Returned) -> Closed {
        info!(final_close = self.final_close.get(), "GIF editor closing");
        if self.final_close.get() { Closed::Final(returned) } else { Closed::Preview(returned) }
    }

    /// Reveals the export bar and follows `progress` until this render is over.
    ///
    /// Pulsing until gifski has written its first frame, a fraction after. gifski buffers
    /// before it writes anything -- it has to quantise a frame before it can diff the next
    /// -- so a bar that sat at zero through that would look stuck at exactly the moment it
    /// exists to say "this is working".
    fn follow(self: &Rc<Self>, progress: Progress) {
        let run = self.render_run.get() + 1;
        self.render_run.set(run);
        self.progress.set_fraction(0.0);
        self.progress.set_text(Some("Rendering\u{2026}"));
        self.progress_row.set_reveal_child(true);

        let editor = Rc::downgrade(self);
        glib::timeout_add_local(PROGRESS_TICK, move || {
            let Some(editor) = editor.upgrade() else { return glib::ControlFlow::Break };
            if editor.render_run.get() != run {
                return glib::ControlFlow::Break;
            }
            match progress.frames() {
                (written, total) if total > 0 && written > 0 => {
                    editor.progress.set_fraction(f64::from(written) / f64::from(total));
                    editor
                        .progress
                        .set_text(Some(&format!("Rendering\u{2026} {written} of {total} frames")));
                }
                _ => editor.progress.pulse(),
            }
            glib::ControlFlow::Continue
        });
    }

    /// Runs one output: render, then hand the result to the flow, then report.
    fn output(
        self: &Rc<Self>,
        what: &'static str,
        act: impl FnOnce(&GifEditorActions, CaptureResult, Done) + 'static,
        after: impl Fn(&Rc<Self>, Outcome) + 'static,
    ) {
        if self.rendering.replace(true) {
            return;
        }
        self.set_rendering(true);
        let progress = Progress::new();
        if !self.unchanged() {
            // A 10 s GIF re-encodes in a few seconds -- gifski quantises every frame to
            // its own palette and diffs it against the last -- and the three buttons are
            // grey for all of it. The bar is what says the window has not hung.
            self.follow(progress.clone());
        }
        let editor = Rc::clone(self);
        glib::spawn_future_local(async move {
            match editor.render(&progress).await {
                Ok(capture) => {
                    let weak = Rc::downgrade(&editor);
                    let done: Done = Box::new(move |outcome| {
                        if let Some(editor) = weak.upgrade() {
                            editor.set_rendering(false);
                            after(&editor, outcome);
                        }
                    });
                    act(&editor.actions, capture, done);
                }
                Err(e) => {
                    warn!("{what} failed: {e}");
                    editor.set_rendering(false);
                    editor.toast(&format!("{what} failed: {e}"));
                }
            }
        });
    }

    fn copy(self: &Rc<Self>) {
        self.output(
            "Copy",
            |actions, capture, done| (actions.copy)(capture, done),
            // And stays open, like the annotation editor's Copy (D108).
            |editor, outcome| {
                if outcome.is_ok() {
                    editor.toast("Copied");
                }
            },
        );
    }

    fn save(self: &Rc<Self>) {
        self.output(
            "Save",
            |actions, capture, done| (actions.save)(capture, done),
            |editor, outcome| match outcome {
                Ok(path) => editor.toast_saved(path.as_deref()),
                Err(e) => editor.toast(&format!("Save failed: {e}")),
            },
        );
    }

    fn save_as(self: &Rc<Self>) {
        if self.rendering.get() {
            return;
        }
        let editor = Rc::clone(self);
        glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder()
                .title("Save GIF")
                .initial_name(file_name(&trimmed_path(&editor.capture.path)))
                .modal(true)
                .build();
            let Ok(file) = dialog.save_future(Some(&editor.window)).await else {
                info!("the GIF save chooser was dismissed");
                return;
            };
            let Some(path) = file.path() else { return };
            editor.output(
                "Save as",
                move |actions, capture, done| (actions.save_as_path)(capture, path, done),
                |editor, outcome| match outcome {
                    Ok(path) => editor.toast_saved(path.as_deref()),
                    Err(e) => editor.toast(&format!("Save failed: {e}")),
                },
            );
        });
    }

    fn toast(&self, title: &str) {
        let toast = adw::Toast::new(title);
        toast.set_timeout(2);
        info!(title, "toast");
        self.toasts.add_toast(toast);
    }

    /// "Saved …", with *Show in Files* for that file, as the annotation editor's is. This
    /// one names the path rather than asking for the last save: a card's Save can land
    /// between the two.
    fn toast_saved(&self, path: Option<&std::path::Path>) {
        let Some(path) = path else {
            self.toast("Saved");
            return;
        };
        let title = format!("Saved {}", file_name(path));
        let toast = adw::Toast::new(&title);
        toast.set_timeout(crate::editor::actions::toast_seconds(true));
        toast.set_button_label(Some("Show in Files"));
        toast.set_action_name(Some("app.reveal-file"));
        toast.set_action_target_value(Some(&path.display().to_string().to_variant()));
        info!(title, button = "Show in Files", "toast");
        self.toasts.add_toast(toast);
    }
}

/// The frame rates a trim offers besides the GIF's own: the Recording page's
/// (`prefs::GIF_FPS_VALUES`, without its "match the screen"), and 5, not worth recording at
/// but worth thinning a reel to. Only those below the GIF's own rate, since a trim can take
/// frames out and never put them in: a rate above it would repeat frames and grow the file.
fn fps_choices(source_fps: u32) -> Vec<u32> {
    std::iter::once(5)
        .chain(crate::prefs::GIF_FPS_VALUES.iter().filter_map(|&fps| u32::try_from(fps).ok()))
        .filter(|&fps| fps > 0 && fps < source_fps)
        .collect()
}
/// The sizes it offers, as a share of the GIF's own.
const SCALE_CHOICES: [u8; 3] = [75, 50, 33];

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.set_tooltip_text(Some(tooltip));
    button
}

/// A size scaled by a percentage, never below one pixel.
fn scaled(width: u32, height: u32, percent: u8) -> (u32, u32) {
    let factor = f64::from(percent.clamp(10, 100)) / 100.0;
    (
        ((f64::from(width) * factor).round() as u32).max(1),
        ((f64::from(height) * factor).round() as u32).max(1),
    )
}

/// `count` thumbnails spread evenly over a GIF's `frames`, each `width` x `height` RGBA.
///
/// One pass over the GIF, taking the frames the strip wants as they come: a
/// frame-difference GIF has to be composed in order anyway, so asking for them in order
/// costs one decode rather than `count` of them.
fn thumbnails(
    path: &std::path::Path,
    frames: usize,
    count: usize,
    width: u32,
    height: u32,
) -> Vec<Vec<u8>> {
    let Ok(mut reader) = Frames::open(path) else { return Vec::new() };
    let (source_w, source_h) = (reader.width() as usize, reader.height() as usize);
    let wanted: Vec<usize> =
        (0..count).map(|i| i * frames.max(1) / count.max(1)).collect();
    let mut out = Vec::with_capacity(count);
    let mut next = 0;
    while next < wanted.len() {
        match reader.seek(wanted[next]) {
            Ok(Some(_)) => {}
            _ => break,
        }
        out.push(box_scale(
            reader.bytes(),
            source_w,
            source_h,
            width as usize,
            height as usize,
        ));
        next += 1;
    }
    out
}

/// A box downscale of RGBA bytes: every output pixel is the average of the source box it
/// covers. No ringing on the hard edges screen content is made of, and no seam.
fn box_scale(rgba: &[u8], width: usize, height: usize, to_w: usize, to_h: usize) -> Vec<u8> {
    let mut out = vec![0u8; to_w * to_h * 4];
    for y in 0..to_h {
        let y0 = y * height / to_h.max(1);
        let y1 = (((y + 1) * height).div_ceil(to_h.max(1))).max(y0 + 1).min(height);
        for x in 0..to_w {
            let x0 = x * width / to_w.max(1);
            let x1 = (((x + 1) * width).div_ceil(to_w.max(1))).max(x0 + 1).min(width);
            let (mut sums, mut n) = ([0u32; 4], 0u32);
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let at = (sy * width + sx) * 4;
                    let Some(px) = rgba.get(at..at + 4) else { continue };
                    for (sum, value) in sums.iter_mut().zip(px) {
                        *sum += u32::from(*value);
                    }
                    n += 1;
                }
            }
            let n = n.max(1);
            let at = (y * to_w + x) * 4;
            for (channel, sum) in sums.iter().enumerate() {
                if let Some(slot) = out.get_mut(at + channel) {
                    *slot = (sum / n) as u8;
                }
            }
        }
    }
    out
}

/// `m:ss.t`, for a playhead: the badge's `10s` is too coarse to place a mark by.
fn tenths(ms: u64) -> String {
    let tenths = ms / 100;
    let seconds = tenths / 10;
    format!("{}:{:02}.{}", seconds / 60, seconds % 60, tenths % 10)
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "the GIF".to_owned())
}

/// `<stem>-trim.gif` beside the original, or `-trim-2`, `-trim-3`… when that exists: a
/// second trim of the same GIF must not overwrite the first.
fn trimmed_path(source: &std::path::Path) -> PathBuf {
    let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "gif".to_owned());
    let first = source.with_file_name(format!("{stem}-trim.gif"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| source.with_file_name(format!("{stem}-trim-{n}.gif")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// The editor's own styling, once per process (the annotation editor's reason: a provider
/// added to the display is never taken away).
fn install_gif_editor_css() {
    thread_local! {
        static INSTALLED: Cell<bool> = const { Cell::new(false) };
    }
    if INSTALLED.replace(true) {
        return;
    }
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        "
.octosnap-gif-canvas {
    /* The desk the GIF lies on. Darker than the window, so a pale recording has an edge
       and a recording with transparency reads as transparent rather than as a hole. */
    background-color: shade(@window_bg_color, 0.72);
}
.octosnap-gif-film {
    border-radius: 6px;
    background-color: shade(@window_bg_color, 0.6);
}
.octosnap-gif-thumbs {
    border-radius: 6px;
}
",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// D133: the Recording page's ladder, cut at the GIF's own rate, with 5 below it.
    #[test]
    fn a_trim_offers_only_rates_below_the_gifs_own() {
        assert_eq!(fps_choices(15), vec![5, 10]);
        assert_eq!(fps_choices(30), vec![5, 10, 15, 24]);
        assert_eq!(fps_choices(50), vec![5, 10, 15, 24, 30]);
        assert_eq!(fps_choices(60), vec![5, 10, 15, 24, 30, 50]);
        assert!(fps_choices(5).is_empty(), "nothing below the lowest");
    }

    #[test]
    fn tenths_reads_like_a_playhead() {
        assert_eq!(tenths(0), "0:00.0");
        assert_eq!(tenths(3_200), "0:03.2");
        assert_eq!(tenths(65_040), "1:05.0");
        assert_eq!(tenths(600_900), "10:00.9");
    }

    #[test]
    fn a_trim_lands_beside_the_original_and_never_on_an_earlier_trim() {
        let dir = std::env::temp_dir().join(format!("octosnap-gif-editor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("01ABC.gif");
        assert_eq!(trimmed_path(&source), dir.join("01ABC-trim.gif"));
        std::fs::write(dir.join("01ABC-trim.gif"), b"x").unwrap();
        assert_eq!(trimmed_path(&source), dir.join("01ABC-trim-2.gif"));
        std::fs::write(dir.join("01ABC-trim-2.gif"), b"x").unwrap();
        assert_eq!(trimmed_path(&source), dir.join("01ABC-trim-3.gif"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_scale_choice_is_a_size_the_encoder_can_use() {
        assert_eq!(scaled(800, 450, 100), (800, 450));
        assert_eq!(scaled(800, 450, 50), (400, 225));
        assert_eq!(scaled(801, 451, 33), (264, 149));
        // Never zero, whatever the percentage and however small the GIF.
        assert_eq!(scaled(2, 1, 10), (1, 1));
    }

    #[test]
    fn a_box_scale_averages_and_keeps_the_size_it_was_asked_for() {
        // Two columns, black and white: halving gives one mid-grey pixel.
        let rgba = vec![
            0, 0, 0, 255, 255, 255, 255, 255, //
            0, 0, 0, 255, 255, 255, 255, 255,
        ];
        let out = box_scale(&rgba, 2, 2, 1, 1);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0], 127);
        assert_eq!(out[3], 255, "alpha is averaged too");
        assert_eq!(box_scale(&rgba, 2, 2, 2, 2).len(), 16);
    }
}
