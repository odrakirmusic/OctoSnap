// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * What a pet does next when nothing is asking it to do anything (`spec/14` §6).
 *
 * A weighted draw over its tricks, with three rules on top. **Never the same trick twice
 * in a row**: a pet that jumps and then jumps again looks like a loop, which is the one
 * thing a pet must not look like. **The settings shape the draw**: with moving on their
 * own off nothing that walks is drawn, with the pets allowed anywhere a pet on the floor
 * now and then climbs onto the desk and one on the desk roams it and now and then comes
 * down, and Liveliness stretches or shrinks the pauses between tricks. **Reduced motion
 * keeps it still**: GNOME's "Reduce animation" leaves a pet that blinks and looks around,
 * and nothing else (`spec/14` §9). No `gi://` imports.
 */

import type { PetSpec } from './art/index.js';
import type { Rng } from './rng.js';

/** `pet-activity` (`spec/08` §11). */
export type Liveliness = 'calm' | 'normal' | 'lively';

export function isLiveliness(name: string): name is Liveliness {
    return name === 'calm' || name === 'normal' || name === 'lively';
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

/** How much longer or shorter the pauses between tricks are. */
const PAUSE: Record<Liveliness, number> = { calm: 1.8, normal: 1, lively: 0.55 };

/**
 * The prototype's mix, which the owner saw and approved (2026-09-26). With the pets allowed
 * anywhere (2026-09-27) the walk is shared out: on the floor a walk or a climb onto the
 * desk, on the desk a roam across it or a hop down to the floor. More climb than come down,
 * so about three pets in five are on the desk at a time, and none stays on either for good.
 */
const WEIGHTS = { walk: 34, jump: 14, special: 30, look: 14, sit: 8, climb: 10, descend: 6 } as const;

/**
 * The next trick, by the name `ACTS` knows it, for a pet at `where`. `last` is the trick
 * before, which this one is never the same as when there is anything else to draw.
 */
export function nextAct(rng: Rng, spec: PetSpec, temperament: Temperament, last: string | null, where: Place = 'floor'): string {
    if (temperament.reduced) return 'look';
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
    if (where === 'desk') return [['roam', WEIGHTS.walk - WEIGHTS.descend], ['descend', WEIGHTS.descend]];
    if (where === 'floor' && temperament.roam === 'anywhere')
        return [['walk', WEIGHTS.walk - WEIGHTS.climb], ['climb', WEIGHTS.climb]];
    return [['walk', WEIGHTS.walk]];
}

function draw(rng: Rng, spec: PetSpec, temperament: Temperament, where: Place): string {
    const weights: [string, number][] = [
        ...walks(temperament, where),
        [temperament.wander ? 'jump' : 'bounce', WEIGHTS.jump],
        ['special', WEIGHTS.special * (temperament.liveliness === 'lively' ? 1.3 : temperament.liveliness === 'calm' ? 0.7 : 1)],
        ['look', WEIGHTS.look],
        ['sit', WEIGHTS.sit * (temperament.liveliness === 'calm' ? 1.6 : 1)],
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

/** How long to wait, in milliseconds, before the next trick. */
export function pause(rng: Rng, temperament: Temperament): number {
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
