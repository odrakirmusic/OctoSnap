// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The pets, by the name the settings use (`pets`, `spec/08` §11), and what the engine has
 * to know about each one beyond its pixels.
 */

import { frog, FROG_MOUTH, frogTop } from './frog.js';
import { mushroom, mushroomTop } from './mushroom.js';
import { octopus, octopusTop } from './octopus.js';
import type { Grid } from './painter.js';
import { penguin, penguinTop } from './penguin.js';
import type { Pose, View } from './pose.js';
import { potato, potatoTop } from './potato.js';

/** One painter for both views: from above when the pose says so (`above.ts`). */
function views(side: (pose: Pose) => Grid, top: (pose: Pose) => Grid): (pose: Pose) => Grid {
    return (pose: Pose = {}) => (pose.view === 'top' ? top(pose) : side(pose));
}

export type PetKind = 'octopus' | 'penguin' | 'frog' | 'mushroom' | 'potato';

/** The settings' order, which is also the order they are offered in. */
export const PET_KINDS: readonly PetKind[] = ['octopus', 'penguin', 'frog', 'mushroom', 'potato'];

/** How a pet gets about: walking with a waddle, hopping, or gliding on its arms. */
export type Gait = 'waddle' | 'hop' | 'glide';

export interface PetSpec {
    kind: PetKind;
    /** Its name, as Settings and the menu say it. */
    name: string;
    /** Canvas size in art pixels. */
    width: number;
    height: number;
    gait: Gait;
    /** Falls slowly when let go, arms spread. */
    floats: boolean;
    /** Its own tricks, by the name `acts.ts` knows them; repeats weight the draw. */
    specials: readonly string[];
    /** Where its mouth is, in art pixels, for a tongue or a sneeze. */
    mouth: { x: number; y: number };
    /** The row its eyes are on, for looking at something. */
    eyeRow: number;
    draw(pose: Pose): Grid;
}

export const PETS: Record<PetKind, PetSpec> = {
    octopus: {
        kind: 'octopus',
        name: 'Omni',
        width: 32,
        height: 32,
        gait: 'glide',
        floats: true,
        specials: ['camo', 'ink', 'wave'],
        mouth: { x: 16, y: 18 },
        eyeRow: 13,
        draw: views(octopus, octopusTop),
    },
    penguin: {
        kind: 'penguin',
        name: 'Pengu',
        width: 32,
        height: 32,
        gait: 'waddle',
        floats: false,
        specials: ['flap', 'wave', 'flap'],
        mouth: { x: 16, y: 15 },
        eyeRow: 12,
        draw: views(penguin, penguinTop),
    },
    frog: {
        kind: 'frog',
        name: 'Hops',
        width: 36,
        height: 32,
        gait: 'hop',
        floats: false,
        specials: ['fly', 'fly', 'croak', 'blep', 'wiggle'],
        mouth: FROG_MOUTH,
        eyeRow: 14,
        draw: views(frog, frogTop),
    },
    mushroom: {
        kind: 'mushroom',
        name: 'Morel',
        width: 32,
        height: 32,
        gait: 'waddle',
        floats: false,
        specials: ['spores', 'tip', 'glow'],
        mouth: { x: 16, y: 24 },
        eyeRow: 20,
        draw: views(mushroom, mushroomTop),
    },
    potato: {
        kind: 'potato',
        name: 'Spud',
        width: 32,
        height: 32,
        gait: 'waddle',
        floats: false,
        specials: ['sprout', 'roll', 'bask'],
        mouth: { x: 16, y: 22 },
        eyeRow: 17,
        draw: views(potato, potatoTop),
    },
};

/**
 * The row under a pet's feet at rest, in art pixels from its top: what stands on the
 * floor, or seen from above (`view` `top`) the bottom of what sits on the desk. Not always
 * the canvas's bottom -- the octopus's arms end three rows short of it. Painted each time
 * it is asked, which the engine does once per pet and view.
 */
export function baselineOf(kind: PetKind, view: View = 'side'): number {
    const grid = PETS[kind].draw(view === 'top' ? { view } : {});
    let row = grid.h;
    while (row > 0 && grid.px.slice((row - 1) * grid.w, row * grid.w).every(c => c === null)) row--;
    return row;
}

/** Whether `name` is one of the pets, for reading the settings' list. */
export function isPetKind(name: string): name is PetKind {
    return (PET_KINDS as readonly string[]).includes(name);
}
