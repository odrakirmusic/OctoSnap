// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The pets, by the name the settings use (`pets`, `spec/08` §11), and what the engine has
 * to know about each one beyond its pixels: the five, and the pets of the packs the
 * extension has read (`spec/14` §14), which `setPackPets` hands in.
 */

import type { ActFactory } from '../acts.js';
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

/** The five, which come with OctoSnap. */
export type BuiltinKind = 'octopus' | 'penguin' | 'frog' | 'mushroom' | 'potato';

/** A pet's kind: one of the five, or a pack's id (`spec/14` §14). */
export type PetKind = string;

/** The settings' order, which is also the order they are offered in; the packs' pets come after. */
export const PET_KINDS: readonly BuiltinKind[] = ['octopus', 'penguin', 'frog', 'mushroom', 'potato'];

/** How a pet gets about: walking with a waddle, hopping, or gliding on its arms. */
export type Gait = 'waddle' | 'hop' | 'glide';

export interface PetSpec {
    kind: PetKind;
    /** Its name, as Settings and the menu say it. */
    name: string;
    /** What it is, for its card and its accessible name: "Omni the octopus". */
    species: string;
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
    /** Tricks only this pet knows, by name: a pack's acts. Looked up before `ACTS`. */
    acts?: Readonly<Record<string, ActFactory>>;
    /** The colours of the puff its `ink` vanishes in; absent is the octopus's ink. */
    ink?: readonly string[];
}

export const PETS: Record<BuiltinKind, PetSpec> = {
    octopus: {
        kind: 'octopus',
        species: 'octopus',
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
        species: 'penguin',
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
        species: 'frog',
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
        species: 'mushroom',
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
        species: 'potato',
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

/** The packs' pets, in id order: whatever `setPackPets` was given last. */
let packPets: readonly PetSpec[] = [];
const listeners = new Set<() => void>();

/** One of the five, by its kind. */
export function isBuiltinKind(name: string): name is BuiltinKind {
    return (PET_KINDS as readonly string[]).includes(name);
}

/** The pet of that kind, one of the five or a pack's, or `undefined` when there is none. */
export function petSpec(kind: PetKind): PetSpec | undefined {
    return isBuiltinKind(kind) ? PETS[kind] : packPets.find(p => p.kind === kind);
}

/** Every pet there is, in the order they are offered: the five, then the packs' by id. */
export function petKinds(): PetKind[] {
    return [...PET_KINDS, ...packPets.map(p => p.kind)];
}

/** Whether `name` is a pet there is, for reading the settings' list. */
export function isPetKind(name: string): boolean {
    return petSpec(name) !== undefined;
}

/**
 * The packs' pets from now on (`packs.ts`), in place of the ones before. A pack's id that is
 * one of the five is never taken; `pack.ts` refuses it before it gets here.
 */
export function setPackPets(specs: readonly PetSpec[]): void {
    packPets = specs.filter(s => !isBuiltinKind(s.kind)).sort((a, b) => (a.kind < b.kind ? -1 : a.kind > b.kind ? 1 : 0));
    for (const listener of [...listeners]) listener();
}

/** Calls `listener` whenever the packs' pets change; the function returned stops it. */
export function onPetsChanged(listener: () => void): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}

/**
 * The row under a pet's feet at rest, in art pixels from its top: what stands on the
 * floor, or seen from above (`view` `top`) the bottom of what sits on the desk. Not always
 * the canvas's bottom -- the octopus's arms end three rows short of it. Painted each time
 * it is asked, which the engine does once per pet and view.
 */
export function baselineOf(kind: PetKind | PetSpec, view: View = 'side'): number {
    const spec = typeof kind === 'string' ? petSpec(kind) : kind;
    if (spec === undefined) throw new Error(`no pet ${String(kind)}`);
    const grid = spec.draw(view === 'top' ? { view } : {});
    let row = grid.h;
    while (row > 0 && grid.px.slice((row - 1) * grid.w, row * grid.w).every(c => c === null)) row--;
    return row;
}
