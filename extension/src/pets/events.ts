// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * What the capture tool tells the pets (`spec/14` §8). The flows call `tellPets` at the
 * moments a pet reacts to -- a screenshot taken, a countdown's tick, a recording about to
 * start -- and the pets' crew listens while pets are on. With no pets there is no
 * listener and a call costs one empty loop, so the flows never have to ask whether pets
 * are on. No `gi://` imports.
 *
 * Module state, as D135 allows it: one set of listeners, emptied by `forgetPetEvents` in
 * `disable()`.
 */

import { error } from '../log.js';
import type { Rect } from '../place.js';

export type PetEvent =
    /** Pixels were read: the flash. `rect` is what was captured. */
    | { kind: 'captured'; rect: Rect; mode: string }
    /** The capture set off from `from` to land as its card at `to`. */
    | { kind: 'flew'; from: Rect; to: Rect }
    /** A second of the self-timer or the recording countdown went by. */
    | { kind: 'tick'; remaining: number }
    /** A recording of `rect` will start after `countdownMs`: get out of it. */
    | { kind: 'recording-soon'; rect: Rect; countdownMs: number }
    /** A recording of `rect` is on. */
    | { kind: 'recording'; rect: Rect }
    | { kind: 'recording-stopped' }
    | { kind: 'reading' }
    /** A scrolling capture's session started over `rect`, or ended. */
    | { kind: 'scrolling'; active: boolean; rect: Rect | null }
    | { kind: 'copied' }
    /** A capture overlay -- a selection or the window picker -- opened, or went. */
    | { kind: 'overlay'; open: boolean }
    | { kind: 'failed' };

type Listener = (event: PetEvent) => void;

const listeners = new Set<Listener>();

/** Tells every listening crew. A listener that throws does not stop the capture that told it. */
export function tellPets(event: PetEvent): void {
    for (const listener of listeners) {
        try {
            listener(event);
        } catch (e) {
            // The pets are the cherry on top: nothing they do may fail a capture.
            error(`a pet could not react to ${event.kind}`, e);
        }
    }
}

/** Listens until the returned function is called. */
export function listenToPets(listener: Listener): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}

/** For `disable()`, which leaves nothing in module scope (D135). */
export function forgetPetEvents(): void {
    listeners.clear();
}
