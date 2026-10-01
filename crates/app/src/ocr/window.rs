// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/07` §2.1's result window.
//!
//! > … a notification "Text copied" with the first line as preview and a **Show** action
//! > opening a small window with the text (editable, Copy, Search).
//!
//! **Search is find-in-text** (`docs/decisions.md` D84). The word is not qualified in
//! §2.1 and could as easily have meant "look this up on the web", which is what a phone's
//! text selection offers -- but `spec/00` sells the app as "fast and private by design"
//! and §2.2 ends with "all on-device; no network", so a button that sent the capture's
//! text to a search engine would be the one place the app went online behind the user's
//! back. A find bar is also what every GNOME window full of text already has.
//!
//! **Editable, so the window has to keep up with itself.** The links are a property of
//! the *text*, not of the read: fixing a misrecognised full stop in a URL has to make the
//! URL work, and deleting a line has to take its link with it. So they are found again on
//! every change, which costs a linear scan of a few kilobytes.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use octosnap_ocr::{Breaks, Read};
use tracing::{info, warn};

/// Small, as §2.1 asks: wide enough for a line of prose without wrapping every line, and
/// short enough to read as a result rather than as a document.
const WIDTH: i32 = 520;
const HEIGHT: i32 = 440;

/// What find-in-text looks for. Case-insensitive because the user is looking for a word
/// they saw, not for a spelling; `TEXT_ONLY` so a search crosses the paragraph joins
/// rather than stopping at the invisible boundaries between them.
const SEARCH: gtk::TextSearchFlags =
    gtk::TextSearchFlags::CASE_INSENSITIVE.union(gtk::TextSearchFlags::TEXT_ONLY);

thread_local! {
    /// The one open result window.
    ///
    /// Reused rather than opened again, because the notification that leads here has a
    /// single id (`spec/09` §1's rule against saying a thing twice) and so there is only
    /// ever one read to show. Pressing **Show** on two captures in a row must not leave
    /// two windows behind, one of which is stale.
    static OPEN: RefCell<Option<Rc<Results>>> = const { RefCell::new(None) };
}

/// One open result window.
pub struct Results {
    window: adw::ApplicationWindow,
    view: gtk::TextView,
    buffer: gtk::TextBuffer,
    title: adw::WindowTitle,
    toasts: adw::ToastOverlay,
    search: gtk::SearchBar,
    entry: gtk::SearchEntry,
    /// "3 of 12", beside the find entry.
    tally: gtk::Label,
    /// §2.1's "URLs offered to open", for a QR code whose whole payload is one.
    open: gtk::Button,
    target: RefCell<Option<String>>,
    /// The tag a link wears, and the two a search match wears.
    link: gtk::TextTag,
    found: gtk::TextTag,
    /// Where the links are *now*, in character offsets, with what each one opens.
    links: RefCell<Vec<(Range<i32>, String)>>,
    detect: Cell<bool>,
    /// What the subtitle says about the read, and the text the app last put on the
    /// clipboard from it: the read's own, until Copy puts the edits there.
    said: RefCell<String>,
    copied: RefCell<String>,
    codes: Cell<bool>,
    /// Whether the pointer is currently showing the hand, so motion does not reset the
    /// cursor on every pixel it crosses.
    hand: Cell<bool>,
    /// The style manager's handlers that repaint the tags. The manager outlives every
    /// window, so a closed one takes them off it.
    styled: RefCell<Vec<glib::SignalHandlerId>>,
}

/// Shows a read, in the window if one is already open and in a new one if not.
pub fn show(app: &adw::Application, read: &Read, breaks: Breaks, detect_links: bool) {
    let results = OPEN.with(|open| open.borrow().clone()).unwrap_or_else(|| {
        let built = Results::new(app);
        OPEN.with(|open| *open.borrow_mut() = Some(Rc::clone(&built)));
        built
    });
    results.fill(read, breaks, detect_links);
    // present() is right here and not in the QAO: this is a window the user asked for by
    // pressing a button, which is the one case `spec/01` §5's "never focus-steal" allows.
    results.window.present();
}

impl Results {
    fn new(app: &adw::Application) -> Rc<Self> {
        install_css();

        let buffer = gtk::TextBuffer::new(None);
        let view = gtk::TextView::with_buffer(&buffer);
        view.set_editable(true);
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        view.set_top_margin(12);
        view.set_bottom_margin(12);
        view.set_left_margin(12);
        view.set_right_margin(12);
        view.set_vexpand(true);
        view.add_css_class("octosnap-text");

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_child(Some(&view));

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&scroller));

        let title = adw::WindowTitle::new("Recognized Text", "");
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&title));

        let find = gtk::ToggleButton::new();
        find.set_icon_name("system-search-symbolic");
        find.set_tooltip_text(Some("Find in text (Ctrl+F)"));
        header.pack_start(&find);

        let open = gtk::Button::with_label("Open");
        open.set_tooltip_text(Some("Open the address this code carries"));
        open.set_visible(false);
        header.pack_start(&open);

        let copy = gtk::Button::with_label("Copy");
        copy.add_css_class("suggested-action");
        copy.set_tooltip_text(Some("Copy all of the text (Ctrl+Shift+C)"));
        header.pack_end(&copy);

        let entry = gtk::SearchEntry::new();
        entry.set_hexpand(true);
        entry.set_placeholder_text(Some("Find in text"));
        let tally = gtk::Label::new(None);
        tally.add_css_class("dim-label");
        tally.add_css_class("numeric");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.append(&entry);
        row.append(&tally);
        let search = gtk::SearchBar::new();
        search.set_child(Some(&row));
        // Not `set_key_capture_widget`: type-to-search is right for a list and wrong for
        // an editable buffer, where every keystroke belongs to the text.
        search.connect_search_mode_enabled_notify({
            let find = find.clone();
            move |bar| find.set_active(bar.is_search_mode())
        });

        let view_stack = adw::ToolbarView::new();
        view_stack.add_top_bar(&header);
        view_stack.add_top_bar(&search);
        view_stack.set_content(Some(&toasts));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Recognized Text")
            .default_width(WIDTH)
            .default_height(HEIGHT)
            .content(&view_stack)
            .build();
        window.set_size_request(360, 240);

        let link = buffer.create_tag(Some("link"), &[]).unwrap_or_else(|| gtk::TextTag::new(None));
        let found =
            buffer.create_tag(Some("found"), &[]).unwrap_or_else(|| gtk::TextTag::new(None));
        paint(&link, &found);

        let results = Rc::new(Self {
            window: window.clone(),
            view: view.clone(),
            buffer: buffer.clone(),
            title,
            toasts,
            search: search.clone(),
            entry: entry.clone(),
            tally,
            open: open.clone(),
            target: RefCell::new(None),
            link: link.clone(),
            found: found.clone(),
            links: RefCell::new(Vec::new()),
            detect: Cell::new(true),
            said: RefCell::new(String::new()),
            copied: RefCell::new(String::new()),
            codes: Cell::new(false),
            hand: Cell::new(false),
            styled: RefCell::new(Vec::new()),
        });

        // The accent colour is a setting and the theme is a setting; a window that is
        // open when either changes has to repaint its own tags (`spec/09` §2).
        let style = adw::StyleManager::default();
        *results.styled.borrow_mut() = vec![
            style.connect_accent_color_notify({
                let (link, found) = (link.clone(), found.clone());
                move |_| paint(&link, &found)
            }),
            style.connect_dark_notify({
                let (link, found) = (link.clone(), found.clone());
                move |_| paint(&link, &found)
            }),
        ];

        wire(&results, &find, &copy, &open);
        results
    }

    /// Puts a read in the window, replacing whatever was there.
    fn fill(self: &Rc<Self>, read: &Read, breaks: Breaks, detect_links: bool) {
        let text = read.text(breaks);
        self.detect.set(detect_links);
        *self.target.borrow_mut() = match read {
            Read::Codes(codes) => codes.iter().find_map(octosnap_ocr::Code::target),
            Read::Text(_) => None,
        };
        self.open.set_visible(self.target.borrow().is_some());
        self.codes.set(matches!(read, Read::Codes(_)));
        *self.said.borrow_mut() = describe(read, &text);
        // Before the text goes in, so the change it makes is not taken for an edit. The
        // read put this text on the clipboard; a Show from a notification shows it again.
        *self.copied.borrow_mut() = text.clone();
        self.buffer.set_text(&text);
        self.buffer.place_cursor(&self.buffer.start_iter());
        self.retag();
        self.count();
    }

    /// Finds the links again over whatever the buffer now holds.
    ///
    /// `octosnap_ocr::links` answers in byte ranges into the string and a `TextBuffer`
    /// counts characters, so each range is converted as it is applied rather than
    /// afterwards -- a buffer whose offsets were bytes would put the tag in the wrong
    /// place on the first accented word.
    fn retag(&self) {
        let (start, end) = self.buffer.bounds();
        self.buffer.remove_tag(&self.link, &start, &end);
        let mut links = self.links.borrow_mut();
        links.clear();
        if !self.detect.get() {
            return;
        }
        let text = self.buffer.text(&start, &end, false);
        for link in octosnap_ocr::links(&text) {
            let Some(before) = text.get(..link.range.start) else { continue };
            let Some(inside) = text.get(link.range.clone()) else { continue };
            let from = i32::try_from(before.chars().count()).unwrap_or(i32::MAX);
            let to = from.saturating_add(i32::try_from(inside.chars().count()).unwrap_or(0));
            let (a, b) = (self.buffer.iter_at_offset(from), self.buffer.iter_at_offset(to));
            self.buffer.apply_tag(&self.link, &a, &b);
            links.push((from..to, link.target));
        }
    }

    /// The link under a point in the view, if there is one.
    fn at(&self, x: f64, y: f64) -> Option<String> {
        #[allow(clippy::cast_possible_truncation)]
        let (bx, by) = self.view.window_to_buffer_coords(
            gtk::TextWindowType::Widget,
            x as i32,
            y as i32,
        );
        let iter = self.view.iter_at_location(bx, by)?;
        let offset = iter.offset();
        self.links
            .borrow()
            .iter()
            .find(|(range, _)| range.contains(&offset))
            .map(|(_, target)| target.clone())
    }

    /// Opens one, which is the only thing in this window that leaves the machine -- and
    /// only ever because the user clicked the link.
    fn launch(&self, target: &str) {
        let launcher = gtk::UriLauncher::new(target);
        let target = target.to_owned();
        launcher.launch(Some(&self.window), gio::Cancellable::NONE, move |result| match result {
            Ok(()) => info!(target, "opened a recognised link"),
            Err(e) => warn!("could not open {target}: {e}"),
        });
    }

    /// The subtitle: what the read was, or that the text has changed since the app last
    /// put it on the clipboard. The window is editable and the clipboard is not, so
    /// without this nothing says that closing it now leaves the edits behind.
    fn mark(&self) {
        if *self.copied.borrow() == self.text() {
            self.title.set_subtitle(&self.said.borrow());
        } else {
            self.title.set_subtitle("Edited, not copied");
        }
    }

    /// The whole text, edits included: what Copy copies.
    fn text(&self) -> String {
        let (start, end) = self.buffer.bounds();
        self.buffer.text(&start, &end, false).into()
    }

    /// Copies through GTK rather than through the extension.
    ///
    /// The post-capture copy cannot: it happens with no window of the app's on screen,
    /// and a Wayland clipboard offer needs a surface and the serial of an input event
    /// (`docs/decisions.md` D12). Here there is a window, the user just clicked a button
    /// in it, and the serial is the click's.
    fn copy(&self) {
        let text = self.text();
        crate::editor::tools::forget_copied_objects("the app copied text");
        self.window.clipboard().set_text(&text);
        self.toasts.add_toast(adw::Toast::new("Copied"));
        crate::flow::play_cue(octosnap_shell::Cue::Copied);
        info!(characters = text.len(), "copied the recognised text");
        // A code's description stays true through an edit; a count of lines may not.
        if !self.codes.get() {
            *self.said.borrow_mut() = lines(&text);
        }
        *self.copied.borrow_mut() = text;
        self.mark();
    }

    /// Paints every match, and says how many there are.
    fn count(&self) {
        let (start, end) = self.buffer.bounds();
        self.buffer.remove_tag(&self.found, &start, &end);
        let needle = self.entry.text();
        if needle.is_empty() {
            self.tally.set_label("");
            return;
        }
        let here = self.buffer.iter_at_mark(&self.buffer.selection_bound()).offset();
        let (mut hits, mut index) = (0_usize, 0_usize);
        let mut at = start;
        while let Some((from, to)) = at.forward_search(&needle, SEARCH, None) {
            self.buffer.apply_tag(&self.found, &from, &to);
            hits += 1;
            if from.offset() == here {
                index = hits;
            }
            at = to;
        }
        self.tally.set_label(&match (hits, index) {
            (0, _) => "No matches".to_owned(),
            (hits, 0) => format!("{hits} matches"),
            (hits, index) => format!("{index} of {hits}"),
        });
    }

    /// Selects the next match, or the previous one, wrapping at the end.
    fn jump(&self, forward: bool) {
        let needle = self.entry.text();
        if needle.is_empty() {
            return;
        }
        // From the far end of the selection, so the match the user is standing on is not
        // the one that is found again. Wrapping rather than stopping: a find bar that
        // goes quiet at the last match looks broken.
        let found = if forward {
            let from = self.buffer.iter_at_mark(&self.buffer.get_insert());
            from.forward_search(&needle, SEARCH, None)
                .or_else(|| self.buffer.start_iter().forward_search(&needle, SEARCH, None))
        } else {
            let from = self.buffer.iter_at_mark(&self.buffer.selection_bound());
            from.backward_search(&needle, SEARCH, None)
                .or_else(|| self.buffer.end_iter().backward_search(&needle, SEARCH, None))
        };
        let Some((from, to)) = found else {
            self.count();
            return;
        };
        // Insert at the end of the match, so the next forward search starts past it.
        self.buffer.select_range(&to, &from);
        self.view.scroll_to_iter(&mut from.clone(), 0.1, false, 0.0, 0.0);
        self.count();
    }

    /// Opens or closes the find bar. Closing puts the focus back in the text.
    fn find(&self, on: bool) {
        self.search.set_search_mode(on);
        if on {
            self.entry.grab_focus();
        } else {
            self.entry.set_text("");
            self.count();
            self.view.grab_focus();
        }
    }
}

/// Everything that needs the `Rc` to exist before it can be connected.
fn wire(results: &Rc<Results>, find: &gtk::ToggleButton, copy: &gtk::Button, open: &gtk::Button) {
    let weak = Rc::downgrade(results);
    copy.connect_clicked(move |_| {
        if let Some(results) = weak.upgrade() {
            results.copy();
        }
    });

    let weak = Rc::downgrade(results);
    open.connect_clicked(move |_| {
        let Some(results) = weak.upgrade() else { return };
        let target = results.target.borrow().clone();
        if let Some(target) = target {
            results.launch(&target);
        }
    });

    let weak = Rc::downgrade(results);
    find.connect_toggled(move |button| {
        if let Some(results) = weak.upgrade() {
            results.find(button.is_active());
        }
    });

    let weak = Rc::downgrade(results);
    results.entry.connect_search_changed(move |_| {
        if let Some(results) = weak.upgrade() {
            results.jump(true);
        }
    });

    let weak = Rc::downgrade(results);
    results.entry.connect_activate(move |_| {
        if let Some(results) = weak.upgrade() {
            results.jump(true);
        }
    });

    let weak = Rc::downgrade(results);
    results.entry.connect_stop_search(move |_| {
        if let Some(results) = weak.upgrade() {
            results.find(false);
        }
    });

    // The text is editable, so the links have to be found again after every edit. Tagging
    // does not change the text, so this cannot re-enter.
    let weak = Rc::downgrade(results);
    results.buffer.connect_changed(move |_| {
        if let Some(results) = weak.upgrade() {
            results.retag();
            results.count();
            results.mark();
        }
    });

    // A plain click opens a link, and a drag does not -- which is what leaves a way to
    // select one and edit it. `released` rather than `pressed`, so the gesture has had
    // its chance to claim the sequence for a selection first.
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_PRIMARY);
    let weak = Rc::downgrade(results);
    click.connect_released(move |gesture, _presses, x, y| {
        let Some(results) = weak.upgrade() else { return };
        if results.buffer.has_selection() {
            return;
        }
        if let Some(target) = results.at(x, y) {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            results.launch(&target);
        }
    });
    results.view.add_controller(click);

    // The hand is the only thing that says a word in an editable buffer is also a link.
    let motion = gtk::EventControllerMotion::new();
    let weak = Rc::downgrade(results);
    motion.connect_motion(move |_, x, y| {
        let Some(results) = weak.upgrade() else { return };
        let over = results.at(x, y).is_some();
        if results.hand.replace(over) != over {
            results.view.set_cursor_from_name(Some(if over { "pointer" } else { "text" }));
        }
    });
    results.view.add_controller(motion);

    // Ctrl+F and Ctrl+Shift+C. Ctrl+C is left alone: it is the text view's own, and a
    // window whose Ctrl+C ignored the selection would be the surprising one.
    let keys = gtk::EventControllerKey::new();
    let weak = Rc::downgrade(results);
    keys.connect_key_pressed(move |_, key, _code, state| {
        let Some(results) = weak.upgrade() else { return glib::Propagation::Proceed };
        let control = state.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        match key {
            gdk::Key::f if control => results.find(true),
            gdk::Key::C | gdk::Key::c if control && shift => results.copy(),
            gdk::Key::Return if results.search.is_search_mode() && shift => results.jump(false),
            gdk::Key::Escape if results.search.is_search_mode() => results.find(false),
            gdk::Key::Escape => results.window.close(),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    results.window.add_controller(keys);

    // Closed means gone: the next Show builds a fresh window rather than presenting a
    // destroyed one. On `close-request` and not `destroy`: GTK 4 emits `destroy` as the
    // window is disposed, and `OPEN` holding the window kept that from happening. So the
    // second Show presented a destroyed window, and at exit `OPEN`'s own destructor set
    // off the handler, whose `OPEN` was then gone, and the service aborted (D139).
    let weak = Rc::downgrade(results);
    results.window.connect_close_request(move |_| {
        let Some(results) = weak.upgrade() else { return glib::Propagation::Proceed };
        let style = adw::StyleManager::default();
        for handler in results.styled.take() {
            style.disconnect(handler);
        }
        OPEN.with(|open| {
            let mut open = open.borrow_mut();
            if open.as_ref().is_some_and(|o| Rc::ptr_eq(o, &results)) {
                *open = None;
            }
        });
        info!("text window closed");
        glib::Propagation::Proceed
    });
}

/// What the subtitle says about a read.
fn describe(read: &Read, text: &str) -> String {
    match read {
        Read::Codes(codes) => match codes.len() {
            1 => "QR code".to_owned(),
            n => format!("{n} QR codes"),
        },
        Read::Text(_) => lines(text),
    }
}

/// How many lines a text runs to, or that there is none.
fn lines(text: &str) -> String {
    match text.lines().count() {
        0 => "No text found".to_owned(),
        1 => "1 line".to_owned(),
        n => format!("{n} lines"),
    }
}

/// Colours the two tags from the current theme.
///
/// The accent colour rather than a fixed blue, because `spec/09` §2 has the app follow
/// the system accent everywhere else and a link is exactly the kind of thing the setting
/// exists for.
fn paint(link: &gtk::TextTag, found: &gtk::TextTag) {
    let accent = adw::StyleManager::default().accent_color_rgba();
    link.set_foreground_rgba(Some(&accent));
    link.set_underline(gtk::pango::Underline::Single);
    // A wash rather than the accent itself: the match has to stay readable, and the text
    // it covers keeps its own colour.
    let mut wash = accent;
    wash.set_alpha(0.3);
    found.set_background_rgba(Some(&wash));
}

/// The window's own styling, once per process (the editor's reason: a provider added to
/// a display is never taken away again).
fn install_css() {
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
.octosnap-text {
    /* A shade larger than the interface font: this window exists to be read, and the
       text in it came from a screenshot the user could already see. */
    font-size: 1.05em;
    line-height: 1.35;
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
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use octosnap_ocr::{Bounds, Code, Line, Paragraph, Shaped};

    fn shaped(lines: &[&str]) -> Read {
        let paragraph = Paragraph {
            lines: lines
                .iter()
                .enumerate()
                .map(|(row, text)| Line {
                    text: (*text).to_owned(),
                    bounds: Bounds::new(0, row as i32 * 20, 100, 14),
                })
                .collect(),
        };
        Read::Text(Shaped { paragraphs: vec![paragraph] })
    }

    #[test]
    fn a_read_says_how_much_of_it_there_is() {
        let read = shaped(&["one", "two", "three"]);
        let text = read.text(Breaks::Keep);
        assert_eq!(describe(&read, &text), "3 lines");
    }

    /// The two shortcuts produce two different strings, and the subtitle counts what the
    /// window is actually showing rather than what the page had on it.
    #[test]
    fn joining_the_lines_makes_it_one_line() {
        let read = shaped(&["one", "two", "three"]);
        let text = read.text(Breaks::Join);
        assert_eq!(describe(&read, &text), "1 line");
    }

    #[test]
    fn a_capture_with_nothing_in_it_says_so() {
        let read = Read::Text(Shaped { paragraphs: Vec::new() });
        assert_eq!(describe(&read, ""), "No text found");
    }

    #[test]
    fn a_code_is_described_as_a_code_not_as_a_line() {
        let read = Read::Codes(vec![Code {
            text: "https://example.com/".to_owned(),
            bounds: Bounds::new(0, 0, 40, 40),
        }]);
        let text = read.text(Breaks::Keep);
        assert_eq!(describe(&read, &text), "QR code");
    }
}
