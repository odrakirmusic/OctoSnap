// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * `spec/09` §3's motion table, the extension's rows, in one place (D129).
 *
 * Every animation the extension plays goes through `Clutter.Actor.ease`, and gnome-shell
 * wraps that: the duration passes through `adjustAnimationTime`, which is 0 while
 * `org.gnome.desktop.interface enable-animations` is off, and an eased property then lands
 * at once with `onComplete` still called. So reduced motion comes with every `ease` here
 * and needs no branch of its own. It has to be asked for by hand only where a duration is
 * *reported* rather than played -- the fly's `animation_ms`, which the app waits out
 * before its card -- and that is what [`played`] is for.
 *
 * `spec/09` §1 item 5 is the rule the easings follow: ease-out for what arrives, ease-in
 * for what leaves.
 */

import Clutter from 'gi://Clutter';
import { adjustAnimationTime } from 'resource:///org/gnome/shell/misc/animationUtils.js';

/** One row of the table: how long, and on which curve. */
export interface Motion {
    duration: number;
    mode: Clutter.AnimationMode;
}

/** "Overlay | open | 120–150 ms | ease-out-quad". */
export const OVERLAY_OPEN: Motion = { duration: 140, mode: Clutter.AnimationMode.EASE_OUT_QUAD };

/** "Overlay | close (cancel) | 100 ms | ease-in-quad". */
export const OVERLAY_CANCEL: Motion = { duration: 100, mode: Clutter.AnimationMode.EASE_IN_QUAD };

/**
 * The frozen frame after a capture, fading while the fly sets off (`spec/03` §7 step 5).
 * Not a row of its own: `spec/03` §10 gives it 150 ms, and it leaves, so it eases in.
 */
export const FROZEN_FADE: Motion = { duration: 150, mode: Clutter.AnimationMode.EASE_IN_QUAD };

/** "Overlay | window highlight move | 120 ms | ease-out-cubic". */
export const HIGHLIGHT_MOVE: Motion = { duration: 120, mode: Clutter.AnimationMode.EASE_OUT_CUBIC };

/** "Overlay | handles appear | 100 ms | ease-out-back (subtle) | scale 0.6→1". */
export const HANDLES_APPEAR: Motion = { duration: 100, mode: Clutter.AnimationMode.EASE_OUT_BACK };
export const HANDLES_FROM_SCALE = 0.6;

/** "Overlay | countdown number | 200 ms per tick | ease-out-cubic | scale 1.2→1". */
export const COUNTDOWN_TICK: Motion = { duration: 200, mode: Clutter.AnimationMode.EASE_OUT_CUBIC };
export const COUNTDOWN_FROM_SCALE = 1.2;

/**
 * `ms` as gnome-shell will play it: 0 with animations off, and scaled by the shell's
 * slow-down factor otherwise. For a duration that something else waits for, so that it
 * waits for the animation the user actually sees rather than the one in the table.
 */
export function played(ms: number): number {
    return adjustAnimationTime(ms);
}

/**
 * `Clutter.Actor.ease`'s parameters, with the keys gnome-shell actually reads.
 *
 * `@girs` types the eased properties as the camelCase JS accessors, and gnome-shell's
 * `_easeActor` drops every key `find_property` does not know, which is every camelCase
 * one: a `scaleX` would be ignored in silence (`docs/decisions.md` D19). The snake_case
 * keys it does read need this cast.
 */
export function easing(params: Record<string, unknown>): Parameters<Clutter.Actor['ease']>[0] {
    return params as unknown as Parameters<Clutter.Actor['ease']>[0];
}

/** Fades `actor` in from nothing, as an overlay opens. */
export function fadeIn(actor: Clutter.Actor, motion: Motion): void {
    actor.opacity = 0;
    actor.ease(easing({ opacity: 255, duration: motion.duration, mode: motion.mode }));
}

/**
 * Finishes an arrival still under way, so what is on screen is the whole of it: a capture
 * confirmed in an overlay's first frames must not fly from a half-faded frozen frame.
 */
export function settle(actor: Clutter.Actor): void {
    actor.remove_transition('opacity');
    actor.opacity = 255;
}

/**
 * Fades `actor` out and runs `drop` when the fade ends, however it ends.
 *
 * `reactive = false` first, and that is the load-bearing line: the modal grab has already
 * been popped by the time this runs, so a still-reactive full-screen actor would swallow
 * every click for the length of the fade. The user would see the desktop and find it
 * dead, which is worse than a hard cut.
 *
 * `remove_all_transitions` guards the case where the overlay is torn down twice in quick
 * succession -- a second fade on an actor already fading would otherwise restart it and
 * postpone the destroy. It also ends an opening fade still running, which the leaving one
 * then starts from where that had got to.
 *
 * `drop` runs exactly once. gnome-shell's `_easeActor` calls `onStopped(true)` and then
 * `onComplete` when a transition finishes, and only `onStopped(false)` when it is
 * interrupted -- so the guard below covers the interrupted case without double-firing the
 * finished one. An interrupted fade must still free the actors, or a display change
 * mid-fade leaves an overlay on screen with no grab and no way to close it.
 */
export function fadeOutAndDrop(actor: Clutter.Actor, motion: Motion, drop: () => void): void {
    actor.reactive = false;
    actor.remove_all_transitions();
    actor.ease(
        easing({
            opacity: 0,
            duration: motion.duration,
            mode: motion.mode,
            onComplete: drop,
            onStopped: (finished: boolean) => {
                if (!finished) drop();
            },
        }),
    );
}
