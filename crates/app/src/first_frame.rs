// SPDX-License-Identifier: GPL-3.0-or-later

//! A window's first frame, as something to wait for: the first moment the compositor has a
//! window there to place (D176).
//!
//! `PlaceWindow` finds a window by the GTK object path its surface reports, and the
//! compositor learns the path with the surface's first commit. A call that overtakes the
//! commit is answered "no window at <path>; no window on the stage reports a GTK object
//! path", and the window stays wherever GTK mapped it. The card, the pin and the history
//! strip wait for their first frame before they ask, for that reason and for the two
//! `qao::card`'s `wait_for_surface` gives. The three pills (the recorder's, the scrolling
//! capture's and the reading indicator) asked at once. In a container's shell, which paints
//! in software, the recorder's lost the race every time (`docs/spikes/19`), so the pills
//! wait too.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use gtk::prelude::*;

/// How long to wait for a frame that does not come: a window that never paints is placed
/// as it was before, rather than never.
const AT_MOST: Duration = Duration::from_secs(2);

#[derive(Debug, Default)]
struct State {
    painted: bool,
    wakers: Vec<Waker>,
}

impl State {
    fn done(&mut self) {
        self.painted = true;
        for waker in self.wakers.drain(..) {
            waker.wake();
        }
    }
}

/// Resolves once a window has painted and committed its first frame. Cloned, it resolves
/// for each clone.
#[derive(Debug, Clone)]
pub struct FirstFrame(Rc<RefCell<State>>);

impl FirstFrame {
    /// Starts watching `window`, which must have been shown already. Called before the main
    /// loop runs again after that, or the first frame can go by unseen.
    pub fn of(window: &impl IsA<gtk::Native>) -> Self {
        let state = Rc::new(RefCell::new(State::default()));
        let Some(surface) = window.surface() else {
            // Nothing will paint, so there is nothing to wait for.
            state.borrow_mut().painted = true;
            return Self(state);
        };
        let clock = surface.frame_clock();
        let handler = Rc::new(RefCell::new(None));
        {
            let once = Rc::clone(&handler);
            let state = Rc::downgrade(&state);
            *handler.borrow_mut() = Some(clock.connect_after_paint(move |clock| {
                // After every frame from now on, so it lets go of the clock after the first.
                if let Some(id) = once.borrow_mut().take() {
                    clock.disconnect(id);
                }
                if let Some(state) = state.upgrade() {
                    state.borrow_mut().done();
                }
            }));
        }
        let late = Rc::downgrade(&state);
        glib::timeout_add_local_once(AT_MOST, move || {
            if let Some(state) = late.upgrade() {
                let mut state = state.borrow_mut();
                if !state.painted {
                    tracing::warn!("a window had not painted after {AT_MOST:?}; placing it anyway");
                    state.done();
                }
            }
        });
        Self(state)
    }
}

impl Future for FirstFrame {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut state = self.0.borrow_mut();
        if state.painted {
            return Poll::Ready(());
        }
        state.wakers.push(cx.waker().clone());
        Poll::Pending
    }
}
