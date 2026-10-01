// SPDX-License-Identifier: GPL-3.0-or-later

//! When a read is slow enough to be worth saying so (`docs/decisions.md` D96).
//!
//! `spec/07` §2.1's text capture skips the after-capture plan wholesale -- nothing is
//! saved, carded or pinned -- so between the selection and the clipboard there is nothing
//! on screen at all. That is right for a strip of a page, which reads in half a second
//! and would only get a flicker, and wrong for a dense region at three seconds or a whole
//! screen at eight.
//!
//! So the indicator is *scheduled* rather than shown. A read that lands first leaves no
//! trace of it; one that does not gets it while the read is still going, and then keeps
//! it long enough to be read itself.
//!
//! No `SourceId` is kept here and none is cancelled, for D32's reason: `SourceId::remove`
//! panics on a source that has already fired, from inside a GLib callback, and that
//! aborts rather than unwinds. The timer always fires, and the *guard* decides whether
//! anything is still waiting for it -- by being alive or not.
//!
//! Both waits are spawned futures rather than `timeout_add_local_once`, because that one
//! attaches to the process's **default** main context: it is `g_timeout_add` underneath,
//! and the context is not a parameter. The read is on whatever context is thread-default
//! where it started, so its indicator has to be too -- and asking for the default from a
//! thread that does not own it is a panic, not a mistimed pill.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Takes the indicator off the screen again.
pub type Hide = Box<dyn FnOnce()>;

/// How long a read may take before it is worth saying so.
///
/// Measured for `spec/10` §7: a 1500x110 strip reads in 0.53 s, a paragraph of prose in
/// 1.2 s, a dense 1000x600 region in 3.1 s and a whole 1920x1080 screen in 7.7 s. The
/// threshold sits in the gap between the first two, so the reads that are over before the
/// eye has settled show nothing at all and every longer one says what it is doing.
const DELAY: Duration = Duration::from_millis(600);

/// The least time the indicator stays once it has appeared.
///
/// Without it a read landing at 610 ms would map a window and unmap it ten milliseconds
/// later, which is a flicker with no information in it. Nothing downstream waits for the
/// difference: the sound, the clipboard and the notification have already gone out.
const LINGER: Duration = Duration::from_millis(400);

/// What the timer finds when it fires: the indicator, if it is already up.
type Up = Rc<RefCell<Option<(Instant, Hide)>>>;

/// One read in flight, and the indicator that may or may not belong to it.
///
/// Dropping it means the read has landed -- or has been abandoned, which wants the same
/// thing to happen to the indicator.
pub struct Reading {
    up: Up,
}

impl std::fmt::Debug for Reading {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shown = self.up.borrow().is_some();
        f.debug_struct("Reading").field("shown", &shown).finish()
    }
}

impl Reading {
    /// Arms the indicator for a read that is starting now.
    ///
    /// `show` runs only if [`DELAY`] passes with the read still going, and answers `None`
    /// when there turned out to be nothing to put on screen.
    pub fn start(show: impl FnOnce() -> Option<Hide> + 'static) -> Self {
        let up: Up = Rc::new(RefCell::new(None));
        let waiting = Rc::downgrade(&up);
        glib::spawn_future_local(async move {
            glib::timeout_future(DELAY).await;
            // Gone means the read landed inside the threshold, and `show` is dropped
            // here without ever having been called.
            let Some(up) = waiting.upgrade() else { return };
            let raised = show().map(|hide| (Instant::now(), hide));
            *up.borrow_mut() = raised;
        });
        Self { up }
    }
}

impl Drop for Reading {
    fn drop(&mut self) {
        let Some((since, hide)) = self.up.borrow_mut().take() else { return };
        let rest = LINGER.saturating_sub(since.elapsed());
        if rest.is_zero() {
            hide();
        } else {
            glib::spawn_future_local(async move {
                glib::timeout_future(rest).await;
                hide();
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// The tests are about timers, so they need a main loop to run them on.
    fn on_a_loop<F: std::future::Future<Output = ()>>(future: F) {
        glib::MainContext::new().block_on(future);
    }

    /// Counts what the indicator was asked to do, without any of GTK in the way.
    #[derive(Default)]
    struct Fake {
        shown: Cell<u32>,
        hidden: Cell<u32>,
    }

    impl Fake {
        fn shower(self: &Rc<Self>) -> impl FnOnce() -> Option<Hide> + 'static {
            let fake = Rc::clone(self);
            move || {
                fake.shown.set(fake.shown.get() + 1);
                let hiding = Rc::clone(&fake);
                Some(Box::new(move || hiding.hidden.set(hiding.hidden.get() + 1)))
            }
        }
    }

    #[test]
    fn a_read_that_lands_before_the_delay_shows_nothing() {
        on_a_loop(async {
            let fake = Rc::new(Fake::default());
            let reading = Reading::start(fake.shower());
            glib::timeout_future(DELAY / 3).await;
            drop(reading);
            // Well past when the timer would have fired: it did, and found nobody.
            glib::timeout_future(DELAY * 2).await;
            assert_eq!(fake.shown.get(), 0, "a quick read put something on screen");
            assert_eq!(fake.hidden.get(), 0);
        });
    }

    #[test]
    fn a_read_that_outstays_the_delay_is_given_something_to_look_at() {
        on_a_loop(async {
            let fake = Rc::new(Fake::default());
            let reading = Reading::start(fake.shower());
            glib::timeout_future(DELAY + DELAY / 3).await;
            assert_eq!(fake.shown.get(), 1, "a slow read was left with nothing on screen");
            assert_eq!(fake.hidden.get(), 0, "the indicator went away while the read was going");
            drop(reading);
            glib::timeout_future(LINGER * 2).await;
            assert_eq!(fake.hidden.get(), 1, "the indicator outlived the read");
        });
    }

    #[test]
    fn an_indicator_that_has_just_appeared_is_not_taken_away_at_once() {
        on_a_loop(async {
            let fake = Rc::new(Fake::default());
            let reading = Reading::start(fake.shower());
            glib::timeout_future(DELAY + DELAY / 20).await;
            assert_eq!(fake.shown.get(), 1);
            // The read lands a moment after the indicator did. Taking it down now would
            // be a flicker, so it is held -- but only the indicator waits.
            drop(reading);
            glib::timeout_future(LINGER / 2).await;
            assert_eq!(fake.hidden.get(), 0, "the indicator blinked rather than spoke");
            glib::timeout_future(LINGER).await;
            assert_eq!(fake.hidden.get(), 1, "the indicator stayed up for good");
        });
    }

    #[test]
    fn a_shower_with_nothing_to_show_leaves_the_guard_harmless() {
        on_a_loop(async {
            let reading = Reading::start(|| None);
            glib::timeout_future(DELAY + DELAY / 3).await;
            // Dropping a guard whose indicator never appeared must not schedule a hide
            // for an indicator that is not there.
            drop(reading);
            glib::timeout_future(LINGER).await;
        });
    }
}
