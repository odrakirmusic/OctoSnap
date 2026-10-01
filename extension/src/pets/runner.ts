// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * How a pet plays a trick (`spec/14` §6). A trick -- an "act" -- is a generator that
 * yields steps: hold this pose for so long, hop this high and this far, move a pixel,
 * float a heart. The runner plays those steps against the clock the engine gives it. No
 * `gi://` imports and no timers of its own: the engine calls `update(now)` and asks
 * `nextAt(now)` when to call again, so a test can play a whole trick in a loop over a
 * fake clock, and the shell wakes only when a frame actually changes.
 *
 * Distances are in art pixels, so a trick is the same trick at every size and scale; the
 * engine turns them into logical pixels. `dx` is across the screen and `dy` down it: a pet
 * on the floor or on a pin moves only across, and one on the desk either way (`spec/14` §4).
 */

import type { EmoteName } from './art/emotes.js';
import type { Face, Pose, View } from './art/pose.js';

/** Something a trick asks the engine to show beside the pet. */
export type Fx =
    | { kind: 'emote'; name: EmoteName }
    /** Specks thrown out from `at` (art pixels on the pet), upward or all round. */
    | { kind: 'burst'; colours: readonly string[]; count: number; up: boolean; at: { x: number; y: number } }
    | { kind: 'fly' }
    | { kind: 'fly-hold' }
    | { kind: 'fly-free' }
    | { kind: 'fly-eaten' }
    | { kind: 'fly-flee' }
    /**
     * The tongue, out to the fly (or where it was) over `ms`, or back when `back`. `to`
     * aims it somewhere else, in art pixels from the mouth: a capture's card landing.
     */
    | { kind: 'tongue'; ms: number; hit: boolean; back: boolean; to?: { dx: number; dy: number } }
    /** Gone, as in a puff of ink; `appear` brings it back. */
    | { kind: 'vanish' }
    | { kind: 'appear' }
    /**
     * Seen from the other side: up onto the desk (`top`) or down onto the floor (`side`),
     * facing `face`. The engine does it only where the pet is: onto the desk from the
     * floor with the pets allowed anywhere, and onto the floor with its feet on it.
     */
    | { kind: 'view'; view: View; face?: Face };

export type Step =
    | { do: 'hold'; pose: Pose; ms: number }
    /**
     * A hop: up to `height` and down again over `ms`, travelling `dx` and `dy`. The pose is
     * stretched on the way up and eased on the way down.
     */
    | { do: 'arc'; pose: Pose; ms: number; height: number; dx: number; dy: number }
    /** An instant move of `dx` and `dy`, a new pose and a bob off the ground: a walk's step. */
    | { do: 'move'; dx: number; dy: number; pose?: Pose; bob?: number }
    | { do: 'fx'; fx: Fx };

export type Act = Generator<Step, void, void>;

export const hold = (pose: Pose, ms: number): Step => ({ do: 'hold', pose, ms });
export const arc = (pose: Pose, ms: number, height: number, dx = 0, dy = 0): Step => ({ do: 'arc', pose, ms, height, dx, dy });
export const move = (dx: number, pose?: Pose, bob?: number, dy = 0): Step => {
    const step: Step = { do: 'move', dx, dy };
    if (pose !== undefined) step.pose = pose;
    if (bob !== undefined) step.bob = bob;
    return step;
};
export const fx = (effect: Fx): Step => ({ do: 'fx', fx: effect });

/** How often a hop is redrawn: the plan's "move at up to 30 fps". */
export const MOTION_FRAME_MS = 33;

/**
 * Behind by more than this and the runner starts the next step from now rather than from
 * when it was due. A suspended laptop or a shell that stalled for a second must not play
 * the rest of a trick in one frame.
 */
const CATCH_UP_MS = 250;

/** Steps started in one update, at most. A trick that yields only instant steps forever is a bug, not a hang. */
const STEPS_PER_UPDATE = 64;

export class Runner {
    readonly #act: Act;
    #step: Step | null = null;
    #start = 0;
    #end = 0;
    /** How far through the current hop the pet had got, for its share of `dx`. */
    #arcDone = 0;
    #dx = 0;
    #dy = 0;
    #fx: Fx[] = [];
    #done = false;
    /** Steps left to start in this update (`STEPS_PER_UPDATE`). */
    #budget = STEPS_PER_UPDATE;
    /** The pose to show now. */
    pose: Pose = {};
    /** Art pixels above the ground. */
    y = 0;

    constructor(act: Act, now: number) {
        this.#act = act;
        this.#end = now;
        this.update(now);
    }

    get done(): boolean {
        return this.#done;
    }

    /**
     * Plays everything due by `now`, or up to a change of view: what moves after one moves
     * the pet in the new view, which it takes on with this update's effects, so the rest
     * waits for the next update.
     */
    update(now: number): void {
        this.#budget = STEPS_PER_UPDATE;
        while (!this.#done && now >= this.#end) {
            this.#finish();
            const from = now - this.#end > CATCH_UP_MS ? now : this.#end;
            this.#next(from);
            if (this.#fx.some(fx => fx.kind === 'view')) return;
        }
        const step = this.#step;
        if (!this.#done && step !== null && step.do === 'arc') this.#playArc(step, now);
    }

    /** When the pet next looks different: the end of a hold, or the next frame of a hop. */
    nextAt(now: number): number {
        if (this.#done) return Infinity;
        if (this.#step?.do === 'arc') return Math.min(this.#end, now + MOTION_FRAME_MS);
        return this.#end;
    }

    /** The art pixels moved across since the last call. */
    takeDx(): number {
        const dx = this.#dx;
        this.#dx = 0;
        return dx;
    }

    /** The art pixels moved down since the last call. */
    takeDy(): number {
        const dy = this.#dy;
        this.#dy = 0;
        return dy;
    }

    /** The effects asked for since the last call. */
    takeFx(): Fx[] {
        const out = this.#fx;
        this.#fx = [];
        return out;
    }

    /**
     * Stops the trick where it is. Its `finally` blocks run, and what they ask for is kept
     * for `takeFx` -- a frog picked up mid-hunt lets its fly go.
     */
    stop(): void {
        if (this.#done) return;
        this.#done = true;
        this.#step = null;
        this.y = 0;
        let result = this.#act.return();
        for (let n = 0; result.done !== true && n < STEPS_PER_UPDATE; n++) {
            if (result.value.do === 'fx') this.#fx.push(result.value.fx);
            result = this.#act.next();
        }
    }

    #finish(): void {
        const step = this.#step;
        if (step?.do === 'arc') {
            this.#dx += step.dx * (1 - this.#arcDone);
            this.#dy += step.dy * (1 - this.#arcDone);
            this.#arcDone = 1;
            this.y = 0;
        }
        this.#step = null;
    }

    #next(from: number): void {
        for (;;) {
            // Every step counts, instant or not: a trick that yields only moves forever
            // must stop here rather than hang the shell's main loop.
            if (--this.#budget < 0) {
                this.stop();
                throw new Error('a trick yielded too many steps without waiting');
            }
            const result = this.#act.next();
            if (result.done === true) {
                this.#done = true;
                this.#step = null;
                return;
            }
            const step = result.value;
            switch (step.do) {
                case 'move':
                    this.#dx += step.dx;
                    this.#dy += step.dy;
                    if (step.pose !== undefined) this.pose = step.pose;
                    this.y = step.bob ?? 0;
                    continue;
                case 'fx':
                    this.#fx.push(step.fx);
                    continue;
                case 'hold':
                    this.pose = step.pose;
                    this.#begin(step, from, step.ms);
                    return;
                case 'arc':
                    this.#arcDone = 0;
                    this.#begin(step, from, step.ms);
                    this.#playArc(step, from);
                    return;
            }
        }
    }

    #begin(step: Step, from: number, ms: number): void {
        this.#step = step;
        this.#start = from;
        this.#end = from + Math.max(0, ms);
    }

    #playArc(step: Extract<Step, { do: 'arc' }>, now: number): void {
        const t = step.ms > 0 ? Math.min(1, Math.max(0, (now - this.#start) / step.ms)) : 1;
        this.y = 4 * step.height * t * (1 - t);
        this.#dx += step.dx * (t - this.#arcDone);
        this.#dy += step.dy * (t - this.#arcDone);
        this.#arcDone = t;
        this.pose = { ...step.pose, squash: t < 0.45 ? -0.75 : -0.25 };
    }
}
