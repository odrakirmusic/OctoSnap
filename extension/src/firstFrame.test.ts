// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * `waitForFirstFrame`, the wait `PlaceWindow` makes before it places a window (D193).
 *
 * The fault these exist for: a window whose first frame came later than the wait's 500 ms
 * was moved before Mutter had drawn it, and answered with the frame rect it read back,
 * which had no size. Mutter then placed the window itself when its buffer came, in the
 * middle of the work area, and the app stacked against a card at the bottom-left corner
 * that was not there. Every case below ends in exactly one answer, and only `drawn` places.
 *
 * The fake is strict where the shell is fragile: touching the actor's handler once the
 * window is gone throws (D49), and so does removing a timer that has fired (D32).
 */

import { readFileSync } from 'node:fs';

import { describe, expect, it } from 'vitest';

import {
    FIRST_FRAME_WAIT_MS,
    type FirstFrameEnds,
    type FirstFrameWindow,
    waitForFirstFrame,
} from './firstFrame.js';

const WAIT_MS = FIRST_FRAME_WAIT_MS;

describe('FIRST_FRAME_WAIT_MS', () => {
    it('outlasts a slow first frame, which the old 500 ms did not', () => {
        // 2026-10-10's first card drew 646 ms after it was asked about; D166's first card
        // on battery took 2.8 s from the capture to its placement.
        expect(FIRST_FRAME_WAIT_MS).toBeGreaterThan(2_800);
    });

    it('ends before the app stops waiting for the answer', () => {
        // The app's half, read from its source: an answer that comes after the app has
        // given up is an answer nobody reads, and the placement it reports is lost.
        const gnome = readFileSync(new URL('../../crates/shell/src/gnome.rs', import.meta.url), 'utf8');
        const found = /const PLACE_TIMEOUT_MS: i32 = ([0-9_]+);/.exec(gnome);
        expect(found, 'PLACE_TIMEOUT_MS in crates/shell/src/gnome.rs').not.toBeNull();
        const appWaits = Number((found?.[1] ?? '0').replaceAll('_', ''));
        // With a second to spare for the later, the placement and the reply.
        expect(FIRST_FRAME_WAIT_MS + 1_000).toBeLessThanOrEqual(appWaits);
    });
});

/** A window under test, with the compositor's side driven by hand. */
function compositor(start: { drawn?: boolean; alive?: boolean } = {}) {
    const state = {
        drawn: start.drawn ?? false,
        alive: start.alive ?? true,
        firstFrame: null as (() => void) | null,
        unmanaged: null as (() => void) | null,
        timer: null as { ms: number; then: () => void } | null,
        timerFired: false,
        laters: [] as (() => void)[],
    };
    const window: FirstFrameWindow = {
        drawn: () => state.drawn,
        alive: () => state.alive,
        onFirstFrame: then => {
            state.firstFrame = then;
            return () => {
                if (!state.alive) throw new Error('disconnected from the actor of a window that is gone');
                state.firstFrame = null;
            };
        },
        onUnmanaged: then => {
            state.unmanaged = then;
            return () => {
                state.unmanaged = null;
            };
        },
        later: then => {
            state.laters.push(then);
        },
        timer: (ms, then) => {
            state.timer = { ms, then };
            return () => {
                if (state.timerFired) throw new Error('removed a timer that had fired');
                state.timer = null;
            };
        },
    };
    const answers: string[] = [];
    const ends: FirstFrameEnds = {
        drawn: () => answers.push('drawn'),
        gone: () => answers.push('gone'),
        late: () => answers.push('late'),
        stopped: () => answers.push('stopped'),
    };
    return {
        state,
        window,
        ends,
        answers,
        /** Mutter has processed the first buffer and painted the window once. */
        paint(): void {
            state.drawn = true;
            state.firstFrame?.();
        },
        /** The next redraw: the laters run. */
        redraw(): void {
            const laters = state.laters.splice(0);
            for (const later of laters) later();
        },
        /** The wait's time is up. */
        timeUp(): void {
            const timer = state.timer;
            if (timer === null) return;
            state.timerFired = true;
            state.timer = null;
            timer.then();
        },
        /** Mutter unmanages the window: its actor goes, then the window says so. */
        unmanage(): void {
            state.alive = false;
            state.unmanaged?.();
        },
        /** Nothing is left connected or pending. */
        released(): boolean {
            return state.firstFrame === null && state.unmanaged === null && state.timer === null;
        },
    };
}

describe('waitForFirstFrame', () => {
    it('places a window that has already drawn at once, and connects nothing', () => {
        const c = compositor({ drawn: true });
        expect(waitForFirstFrame(c.window, c.ends, WAIT_MS)).toBeNull();
        expect(c.answers).toEqual(['drawn']);
        expect(c.released()).toBe(true);
    });

    it('answers gone for a window that closed before the wait began', () => {
        const c = compositor({ alive: false });
        expect(waitForFirstFrame(c.window, c.ends, WAIT_MS)).toBeNull();
        expect(c.answers).toEqual(['gone']);
    });

    it('places a window when it first draws, outside that frame', () => {
        const c = compositor();
        waitForFirstFrame(c.window, c.ends, WAIT_MS);
        expect(c.state.timer?.ms).toBe(WAIT_MS);
        c.paint();
        // Not from inside the frame that painted it (D36).
        expect(c.answers).toEqual([]);
        expect(c.released()).toBe(true);
        c.redraw();
        expect(c.answers).toEqual(['drawn']);
    });

    it('never places a window that has not drawn when the wait runs out', () => {
        const c = compositor();
        waitForFirstFrame(c.window, c.ends, WAIT_MS);
        c.timeUp();
        expect(c.answers).toEqual(['late']);
        expect(c.released()).toBe(true);
        // Drawing afterwards changes nothing: the wait is over and it was not placed.
        c.paint();
        c.redraw();
        expect(c.answers).toEqual(['late']);
    });

    it('answers gone as soon as the window is unmanaged, not when the wait runs out', () => {
        const c = compositor();
        waitForFirstFrame(c.window, c.ends, WAIT_MS);
        // The actor's handler is left alone: the fake throws if it is touched now.
        c.unmanage();
        expect(c.answers).toEqual(['gone']);
        expect(c.state.unmanaged).toBeNull();
        expect(c.state.timer).toBeNull();
        c.timeUp();
        expect(c.answers).toEqual(['gone']);
    });

    it('answers gone for a window that drew and closed before the later ran', () => {
        const c = compositor();
        waitForFirstFrame(c.window, c.ends, WAIT_MS);
        c.paint();
        c.unmanage();
        c.redraw();
        expect(c.answers).toEqual(['gone']);
    });

    it('answers stopped once when abandoned, and nothing after', () => {
        const c = compositor();
        const abandon = waitForFirstFrame(c.window, c.ends, WAIT_MS);
        expect(abandon).not.toBeNull();
        abandon?.();
        abandon?.();
        expect(c.answers).toEqual(['stopped']);
        expect(c.released()).toBe(true);
        c.paint();
        c.redraw();
        c.timeUp();
        expect(c.answers).toEqual(['stopped']);
    });

    it('lets an abandon that comes after the first frame change nothing', () => {
        // `disable()` between the first frame and its later: the placement still answers.
        const c = compositor();
        const abandon = waitForFirstFrame(c.window, c.ends, WAIT_MS);
        c.paint();
        abandon?.();
        c.redraw();
        expect(c.answers).toEqual(['drawn']);
    });
});
