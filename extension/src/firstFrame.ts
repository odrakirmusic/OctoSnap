// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Waiting for a window's first frame before `PlaceWindow` places it (D34, D36, D49, D193).
 *
 * A window can be found before it can be placed. Mutter learns its GTK object path with the
 * surface's first commit, and GTK's first commits carry no picture: the role, then the
 * acknowledgement of the first configure. The first buffer comes with the first frame GTK
 * actually renders, and on 2026-10-10 that came 646 ms after the acknowledgement: GTK's
 * Vulkan renderer's first picture with its pipeline and shader caches cold, on battery.
 * Until then Mutter has no size for the window and has not placed it, and when the buffer
 * comes Mutter places the window itself. A window moved before then is moved again, to the
 * middle of the work area, four times out of four in a nested shell, and its frame rect
 * read back has no size. So nothing is placed before the first frame, however long it takes
 * to come.
 *
 * No `gi://` imports, so the sequencing can be tested under Node: `dbus.ts` answers these
 * questions with Mutter, `firstFrame.test.ts` with fakes.
 */

/**
 * How long a window may take to draw before it is given up on and not placed.
 *
 * Long, because waiting costs nothing: a window with no buffer shows nothing, so there is
 * nothing on the screen to put right, and a window placed late is placed exactly as one
 * placed early. What it must stay under is the app's patience for the answer
 * (`PLACE_TIMEOUT_MS` in `crates/shell/src/gnome.rs`, which `firstFrame.test.ts` reads).
 * The slowest first frame measured is 646 ms; a first card on battery took 2.8 s from the
 * capture to its placement (D166). A window that has not drawn in ten seconds is a client
 * that has hung, and it goes wherever Mutter puts it if it ever draws.
 */
export const FIRST_FRAME_WAIT_MS = 10_000;

/** What the wait asks of the compositor about one window. */
export interface FirstFrameWindow {
    /** Whether Mutter has the window's first buffer, which is when its frame rect has a size. */
    drawn(): boolean;
    /**
     * Whether Mutter still manages the window: `isAlive`, which asks the window and never
     * its actor, since the actor may be disposed (D49).
     */
    alive(): boolean;
    /**
     * Calls `then` when the window's actor has painted its first frame, from inside that
     * frame. Returns the disconnect, which may only be called while the window is alive.
     */
    onFirstFrame(then: () => void): () => void;
    /** Calls `then` when Mutter unmanages the window. Returns the disconnect. */
    onUnmanaged(then: () => void): () => void;
    /** Runs `then` before the next redraw, outside any frame (D36). */
    later(then: () => void): void;
    /** Calls `then` once, `ms` from now. Returns the removal, for a timer that has not fired. */
    timer(ms: number, then: () => void): () => void;
}

/** How a wait ends: exactly one of these, once. */
export interface FirstFrameEnds {
    /** The window has drawn: place it. Never called from inside a frame. */
    drawn(): void;
    /** The window closed before it drew, or between drawing and being placed (D49). */
    gone(): void;
    /**
     * The window had not drawn when the wait ran out. It is not placed: Mutter will place
     * it when it draws, and a move made now would be undone then.
     */
    late(): void;
    /** The extension was turned off first. */
    stopped(): void;
}

/**
 * Waits until `window` has drawn, has gone, has run out of `waitMs`, or the wait is
 * abandoned, and ends it with the matching one of `ends`.
 *
 * Returns how to abandon the wait, for `disable()`, or `null` when it ended at once.
 *
 * Releases what it connected exactly once, on whichever path ends it. The actor's handler
 * is released only while the window is alive: once Mutter has unmanaged the window its
 * actor may be disposed, and touching a disposed actor is the half of D49 that crashed the
 * shell. The timer is not removed after it has fired: removing a fired GLib source is a
 * `GLib-CRITICAL`, and the rule from D32 is that whichever side ends a one-shot source
 * releases its handle.
 */
export function waitForFirstFrame(
    window: FirstFrameWindow,
    ends: FirstFrameEnds,
    waitMs: number,
): (() => void) | null {
    if (!window.alive()) {
        // No actor is not "too early", it is "not composited": the window closed between
        // being found and being waited for.
        ends.gone();
        return null;
    }
    if (window.drawn()) {
        ends.drawn();
        return null;
    }

    let done = false;
    let timerFired = false;
    // Replaced below, before anything can call them.
    let stopFirstFrame = (): void => {};
    let stopUnmanaged = (): void => {};
    let stopTimer = (): void => {};

    const release = (): void => {
        if (window.alive()) stopFirstFrame();
        stopUnmanaged();
        if (!timerFired) stopTimer();
    };
    const end = (how: () => void): void => {
        if (done) return;
        done = true;
        release();
        how();
    };

    stopFirstFrame = window.onFirstFrame(() => {
        if (done) return;
        done = true;
        release();
        // `first-frame` is emitted while the frame is being painted, and placing restacks:
        // that mid-paint takes the shell down (D36). A later runs outside the paint, and is
        // a frame away, so whether the window is still there is asked again there (D49).
        window.later(() => (window.alive() ? ends.drawn() : ends.gone()));
    });
    // Without this a window closed before it drew would hold the wait, and its caller's
    // answer, until the timer ran out (D49 had the timer at 500 ms, so it did not show).
    stopUnmanaged = window.onUnmanaged(() => end(ends.gone));
    stopTimer = window.timer(waitMs, () => {
        timerFired = true;
        end(ends.late);
    });
    return () => end(ends.stopped);
}
