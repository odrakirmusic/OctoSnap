// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The pet packs as `GetPetPacks` hands them to the app (`spec/14` §14, `spec/10` §3.1): JSON
 * with the user's folder, each pack's pet as Settings plays it, and each pack that could not
 * be read with its problem.
 *
 * A pet goes as its preview (`preview.ts`) already played and painted, so Settings shows
 * what the desktop does and has nothing of a pack to read itself. Its frames are a palette
 * and, for each frame, two hex digits a pixel, rows top to bottom: `00` clear, and `n` the
 * palette's `n`th colour from 1. No `gi://` imports.
 */

import { type PetSpec, baselineOf } from './art/index.js';
import type { Pack } from './pack.js';
import { packParts, previewOf } from './preview.js';

/** What the packs are now: the user's folder, each pet read, and each pack left out. */
export interface PackListing {
    folder: string;
    pets: { pack: Pack; from: string }[];
    problems: { id: string; from: string; problem: string }[];
}

/** A pack's pet on the wire. */
export interface WirePet {
    id: string;
    name: string;
    species: string;
    from: string;
    width: number;
    height: number;
    baseline: number;
    palette: string[];
    frames: string[];
    loop: [number, number, number][];
}

/** Each pet's preview, made once: playing it is a few milliseconds a pet. */
const made = new WeakMap<PetSpec, Omit<WirePet, 'from'>>();

function wirePet(spec: PetSpec): Omit<WirePet, 'from'> {
    const cached = made.get(spec);
    if (cached !== undefined) return cached;
    const { frames, loop } = previewOf(spec, packParts(spec));
    const palette: string[] = [];
    const index = new Map<string, number>();
    const encoded = frames.map(grid => {
        let out = '';
        for (const colour of grid.px) {
            if (colour === null) {
                out += '00';
                continue;
            }
            let n = index.get(colour);
            if (n === undefined) {
                palette.push(colour);
                n = palette.length;
                index.set(colour, n);
            }
            out += n.toString(16).padStart(2, '0');
        }
        return out;
    });
    const pet = {
        id: spec.kind,
        name: spec.name,
        species: spec.species,
        width: spec.width,
        height: spec.height,
        baseline: baselineOf(spec),
        palette,
        frames: encoded,
        loop,
    };
    made.set(spec, pet);
    return pet;
}

/** `listing` as `GetPetPacks`' JSON. */
export function wirePacks(listing: PackListing): string {
    return JSON.stringify({
        folder: listing.folder,
        pets: listing.pets.map(({ pack, from }) => ({ ...wirePet(pack.spec), from })),
        problems: listing.problems,
    });
}
