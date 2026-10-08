// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * What a pet does next when nothing is asking it to do anything (`spec/14` §6).
 *
 * A weighted draw over its tricks, with three rules on top. **Never the same trick twice
 * in a row**: a pet that jumps and then jumps again looks like a loop, which is the one
 * thing a pet must not look like. **The settings shape the draw**: with moving on their
 * own off nothing that walks is drawn, with the pets allowed anywhere a pet on the floor
 * now and then climbs onto the desk and one on the desk roams it and now and then comes
 * down, and Liveliness stretches or shrinks the pauses between tricks and shifts the mix.
 * **Silent starts nothing**: a pet breathes, blinks and answers what is done to it, and
 * chooses no trick of its own (D166). **Reduced motion keeps it still**: GNOME's "Reduce
 * animation" leaves a pet that blinks and looks around, and nothing else (`spec/14` §9).
 * No `gi://` imports.
 */

import type { PetSpec } from './art/index.js';
import type { Rng } from './rng.js';

/** `pet-activity` (`spec/08` §11), quietest first. Silent and Zen since 2026-10-07 (D166). */
export const LIVELINESS = ['silent', 'zen', 'calm', 'normal', 'lively'] as const;

export type Liveliness = (typeof LIVELINESS)[number];

export function isLiveliness(name: string): name is Liveliness {
    return (LIVELINESS as readonly string[]).includes(name);
}

/** `pet-roam` (`spec/14` §4): along the bottom only, or anywhere on the screen. */
export type Roam = 'floor' | 'anywhere';

export function isRoam(name: string): name is Roam {
    return name === 'floor' || name === 'anywhere';
}

/** Where a pet is, for what it may do next: on the floor, on a pin, or on the desk. */
export type Place = 'floor' | 'pin' | 'desk';

export interface Temperament {
    liveliness: Liveliness;
    /** `pet-wander`, "Move on their own": false keeps a pet where it was put. */
    wander: boolean;
    roam: Roam;
    /** GNOME's reduced motion. */
    reduced: boolean;
}

/**
 * How much longer or shorter the pauses between tricks are. Silent's never end: it starts
 * nothing by itself. Zen's are five times Normal's, 4.5 to 17 seconds.
 */
const PAUSE: Record<Liveliness, number> = { silent: Infinity, zen: 5, calm: 1.8, normal: 1, lively: 0.55 };

/** Whether a pet chooses tricks of its own at all: under Silent it never does. */
export function startsTricks(temperament: Temperament): boolean {
    return temperament.liveliness !== 'silent';
}

/**
 * The prototype's mix, which the owner saw and approved (2026-09-26). With the pets allowed
 * anywhere (2026-09-27) the walk is shared out: on the floor a walk or a climb onto the
 * desk, on the desk a roam across it or a hop down to the floor. More climb than come down,
 * so about three pets in five are on the desk at a time, and none stays on either for good.
 */
const WEIGHTS = { walk: 34, jump: 14, special: 30, look: 14, sit: 8, climb: 10, descend: 6 } as const;

type Mix = Record<'walk' | 'jump' | 'special' | 'look' | 'sit', number>;

/**
 * How Liveliness shifts the mix: a lively pet does more of its own tricks, a calm one sits
 * more. Zen mostly sits and looks about, now and then wanders, and only rarely does a trick
 * of its own. With its longer pauses that is about five things a minute against Calm's ten,
 * and one of its own tricks every three minutes or so against Calm's two or three a minute,
 * by tricks' own lengths (D166). Silent draws nothing, so its mix is never read.
 */
const MIX: Record<Liveliness, Mix> = {
    silent: { walk: 0, jump: 0, special: 0, look: 1, sit: 0 },
    zen: { walk: 0.6, jump: 0.4, special: 0.15, look: 1.5, sit: 3 },
    calm: { walk: 1, jump: 1, special: 0.7, look: 1, sit: 1.6 },
    normal: { walk: 1, jump: 1, special: 1, look: 1, sit: 1 },
    lively: { walk: 1, jump: 1, special: 1.3, look: 1, sit: 1 },
};

/**
 * The next trick, by the name `ACTS` knows it, for a pet at `where`. `last` is the trick
 * before, which this one is never the same as when there is anything else to draw.
 */
export function nextAct(rng: Rng, spec: PetSpec, temperament: Temperament, last: string | null, where: Place = 'floor'): string {
    if (temperament.reduced || !startsTricks(temperament)) return 'look';
    for (let attempt = 0; attempt < 4; attempt++) {
        const name = draw(rng, spec, temperament, where);
        if (name !== last) return name;
    }
    // Four draws that all came up the same: take anything else there is.
    return last === 'look' ? 'sit' : 'look';
}

/** How the walk's weight is shared out at `where`: `[name, weight]`. */
function walks(temperament: Temperament, where: Place): [string, number][] {
    if (!temperament.wander) return [['walk', 0]];
    const k = MIX[temperament.liveliness].walk;
    if (where === 'desk') return [['roam', (WEIGHTS.walk - WEIGHTS.descend) * k], ['descend', WEIGHTS.descend * k]];
    if (where === 'floor' && temperament.roam === 'anywhere')
        return [['walk', (WEIGHTS.walk - WEIGHTS.climb) * k], ['climb', WEIGHTS.climb * k]];
    return [['walk', WEIGHTS.walk * k]];
}

function draw(rng: Rng, spec: PetSpec, temperament: Temperament, where: Place): string {
    const mix = MIX[temperament.liveliness];
    const weights: [string, number][] = [
        ...walks(temperament, where),
        [temperament.wander ? 'jump' : 'bounce', WEIGHTS.jump * mix.jump],
        ['special', WEIGHTS.special * mix.special],
        ['look', WEIGHTS.look * mix.look],
        ['sit', WEIGHTS.sit * mix.sit],
    ];
    const total = weights.reduce((sum, [, w]) => sum + w, 0);
    let roll = rng.next() * total;
    let chosen = 'look';
    for (const [name, w] of weights) {
        if (roll < w) {
            chosen = name;
            break;
        }
        roll -= w;
    }
    if (chosen !== 'special') return chosen;
    // Tricks that go somewhere stay home with moving on their own off.
    const specials = spec.specials.filter(s => temperament.wander || (s !== 'ink' && s !== 'roll'));
    return specials.length > 0 ? rng.pick(specials) : 'look';
}

/** How long to wait, in milliseconds, before the next trick: for ever, under Silent. */
export function pause(rng: Rng, temperament: Temperament): number {
    if (!startsTricks(temperament)) return Infinity;
    if (temperament.reduced) return rng.range(2500, 6000);
    return rng.range(900, 3400) * PAUSE[temperament.liveliness];
}

/** How long until the next blink, and whether it is a double one. */
export function nextBlink(rng: Rng): { in: number; double: boolean } {
    const double = rng.chance(0.18);
    return { in: double ? 230 : rng.range(1600, 5200), double };
}

/** How long a blink holds its eyes shut. */
export const BLINK_MS = 110;

/**
 * How long a breath takes, in milliseconds: the idle pose squashes by one step and back
 * over this, so a pet at rest is not a still picture.
 */
export const BREATH_MS = 2600;
