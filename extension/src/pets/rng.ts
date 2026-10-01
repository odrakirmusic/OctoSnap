// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The pets' dice: a small seeded generator, so a test that gives a pet a seed sees the
 * same tricks in the same order every run, and a pet in the shell still never does the
 * same thing twice the same way (`spec/14` §6). Mulberry32: 32 bits of state, fast, and
 * more than random enough for when to blink.
 */
export class Rng {
    #state: number;

    constructor(seed: number) {
        this.#state = seed >>> 0;
    }

    /** A number in `[0, 1)`. */
    next(): number {
        this.#state = (this.#state + 0x6d2b79f5) >>> 0;
        let t = this.#state;
        t = Math.imul(t ^ (t >>> 15), t | 1);
        t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
        return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    }

    /** A number in `[low, high)`. */
    range(low: number, high: number): number {
        return low + this.next() * (high - low);
    }

    /** A whole number from `low` to `high`, both included. */
    int(low: number, high: number): number {
        return low + Math.floor(this.next() * (high - low + 1));
    }

    /** True with probability `p`. */
    chance(p: number): boolean {
        return this.next() < p;
    }

    pick<T>(items: readonly T[]): T {
        const item = items[Math.floor(this.next() * items.length)];
        if (item === undefined) throw new Error('picking from nothing');
        return item;
    }

    /** -1 or 1. */
    sign(): number {
        return this.next() < 0.5 ? -1 : 1;
    }
}
