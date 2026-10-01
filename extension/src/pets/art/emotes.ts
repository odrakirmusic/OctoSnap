// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The small pieces that are not pets: the emotes that float up from a pet's head, the fly
 * the frog catches, and the shadow each pet stands on. Drawn in the same art pixels as the
 * pets, so at any size they are exactly as crisp.
 */

import { type Grid, gridFromRows } from './painter.js';

export type EmoteName = 'heart' | 'bang' | 'what' | 'note' | 'zzz' | 'spark' | 'text' | 'star';

const EMOTES: Record<EmoteName, { rows: string[]; colours: Record<string, string> }> = {
    heart: {
        colours: { r: '#ff4d6d', h: '#ffb3c1', o: '#8a1030' },
        rows: ['.oo.oo.', 'orhorro', 'orrrrro', '.orrro.', '..oro..', '...o...'],
    },
    bang: {
        colours: { y: '#ffd43b', o: '#5c4200' },
        rows: ['.ooo.', '.oyo.', '.oyo.', '.oyo.', '.ooo.', '.oyo.', '.ooo.'],
    },
    what: {
        colours: { w: '#ffffff', o: '#3d3846' },
        rows: ['.ooo.', 'owwwo', 'ooowo', '.owwo', '.ooo.', '.owo.', '.ooo.'],
    },
    note: {
        colours: { k: '#3d3846' },
        rows: ['...kkk', '...k.k', '...k.k', '.kkk.k', 'kkkk.k', '.kk.kk', '....kk'],
    },
    zzz: { colours: { k: '#6c7a96' }, rows: ['kkkk', '..k.', '.k..', 'kkkk'] },
    spark: {
        colours: { y: '#ffe066', w: '#ffffff' },
        rows: ['...y...', '...y...', '..ywy..', 'yywwwyy', '..ywy..', '...y...', '...y...'],
    },
    text: {
        colours: { w: '#ffffff', o: '#3d3846', l: '#9a9996' },
        rows: ['oooooo.', 'owwwwoo', 'owllwwo', 'owwwwwo', 'owlllwo', 'owwwwwo', 'owllwwo', 'ooooooo'],
    },
    star: { colours: { y: '#ffd43b', o: '#8f6a00' }, rows: ['.o.', 'oyo', '.o.'] },
};

export const EMOTE_NAMES = Object.keys(EMOTES) as EmoteName[];

export function emote(name: EmoteName): Grid {
    const e = EMOTES[name];
    return gridFromRows(e.rows, e.colours);
}

/** The frog's fly, wings up or down. */
export function fly(wingsUp: boolean): Grid {
    const colours = { b: '#2b2a33', w: '#cfe3ff' };
    return gridFromRows(wingsUp ? ['w..w', '.bb.'] : ['....', 'wbbw'], colours);
}

/**
 * The shadow a pet stands on: a flat oval `width` art pixels across, which narrows as the
 * pet rises (`spec/14` §4). Semi-transparent black, so it reads on a light desktop and a
 * dark one.
 */
export function shadow(width: number): Grid {
    const w = Math.max(4, Math.round(width));
    const rows = [
        `..${'s'.repeat(Math.max(0, w - 4))}..`,
        's'.repeat(w),
        `..${'s'.repeat(Math.max(0, w - 4))}..`,
    ];
    return gridFromRows(rows, { s: '#0000002e' });
}
