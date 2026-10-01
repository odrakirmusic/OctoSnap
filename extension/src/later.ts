// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Waiting for the compositor to paint.
 *
 * `spec/03` §3 requires the overlay to hide itself for one frame before a grab, so its
 * chrome is never in the shot. That is only meaningful if the code can *wait* for the
 * frame, which is what this does.
 *
 * It waits on a timer, for two frames' time. Mutter's laters run before a redraw and never
 * after one: `Meta.LaterType` has no `AFTER_REDRAW`. This module used to look for one
 * first, and on every GNOME Shell it has run on the lookup found nothing and the timer did
 * the waiting. What is new is that each wait is a source `disable()` can remove, which the
 * review at extensions.gnome.org requires of every one (D135).
 */

import GLib from 'gi://GLib';

/** Two frames at 60 Hz, enough for hidden actors to be off screen. */
const WAIT_MS = 34;

/** The waits still running. */
const pending = new Set<number>();

export function waitForRedraw(): Promise<void> {
    return new Promise(resolve => {
        const id = GLib.timeout_add(GLib.PRIORITY_DEFAULT, WAIT_MS, () => {
            pending.delete(id);
            resolve();
            return GLib.SOURCE_REMOVE;
        });
        pending.add(id);
    });
}

/**
 * For `disable()`: every wait's source removed, and its promise never settled. A capture
 * waiting here has hidden its chrome and is about to read the screen, and the extension
 * turning off, often because the screen is locking, is no moment to read it. With the
 * source gone nothing holds the promise, so the capture is dropped with it.
 */
export function forgetRedrawWaits(): void {
    for (const id of pending) GLib.source_remove(id);
    pending.clear();
}
