// SPDX-License-Identifier: GPL-3.0-or-later

/** Spud: a potato with a sprout on top, which grows, rolls and basks with butter. 32x32. */

import { Facing } from './above.js';
import { drawEyes } from './eyes.js';
import { type Grid, type Palette, type StampKey, Painter, rotateGrid } from './painter.js';
import type { Pose } from './pose.js';

export const POTATO_PALETTE: Palette = {
    skin: { h: '#f2cf94', b: '#d9a563', s: '#b37d41', o: '#6b4324' },
    spot: { b: '#a8723c' },
    leaf: { h: '#9ee37a', b: '#5fbd4f', s: '#3a8a3b', o: '#1f5528' },
    eye: { b: '#2b1a10' },
    eyeHi: { b: '#ffffff' },
    blush: { b: '#ff9f8a' },
    mouth: { b: '#6b4324' },
    feet: { h: '#c48e52', b: '#b07a42', s: '#8e5d2e', o: '#5a371c' },
    butter: { b: '#ffe27a', s: '#e8b93c' },
};

const LEAF: StampKey = { h: ['leaf', 'h'], b: ['leaf', 'b'], s: ['leaf', 's'], o: ['leaf', 'o'] };

/** The skin's eyes: darker dots, placed by hand so they sit where a potato's would. */
const SPOTS: readonly (readonly [number, number])[] = [
    [8, 21],
    [9, 26],
    [24, 14],
    [25, 21],
    [21, 11],
    [14, 28],
    [26, 17],
    [11, 12],
];

export function potato(pose: Pose = {}): Grid {
    const { squash: sq = 0, look = 0, mood = 'open', step = 0, sprout = 1, lookY = 0, butter = false, rot = 0 } = pose;
    const P = new Painter(32, 32);
    const lf = step === 1 ? -0.8 : 0;
    const rf = step === 3 ? -0.8 : 0;
    P.ellipse(12.2, 30.5 + lf, 2.4, 1.2, 'feet', { hi: 0.8, sh: 0.3 });
    P.ellipse(19.8, 30.5 + rf, 2.4, 1.2, 'feet', { hi: 0.8, sh: 0.3 });
    const cy = 20.0 + sq * 1.0;
    P.ellipse(16, cy, 10.7 + sq * 1.1, 10.1 - sq * 1.0, 'skin', { rot: -0.14 });
    P.ellipse(10.6, cy - 5.0 + sq * 0.6, 5.3, 4.7, 'skin');
    P.ellipse(22.0, cy + 4.9 - sq * 0.3, 5.0, 4.5, 'skin');
    P.reshade('skin', 15.4, cy - 0.4, 12.0, 11.4, 0.84, 0.3);
    P.outline();
    for (const [x, y] of SPOTS) {
        const yy = y + Math.round(sq);
        if (P.m(x, yy) === 'skin' && P.toneAt(x, yy) !== 'o') P.set(x, yy, 'spot', 'b', true);
    }
    const top = Math.round(cy - 10.1 + sq * 1.0);
    if (sprout === 1) P.stamp(13, top - 4, ['.hb..bh', 'hbbssbb', '.ss.s..', '...s...', '...s...'], LEAF);
    else if (sprout >= 2)
        P.stamp(
            11,
            top - 7,
            ['....hh....', '...hbbs...', '.hh.bs.hh.', 'hbbs.s.bbs', '.bss.sss..', '.....s....', '.....s....'],
            LEAF,
        );
    if (butter) P.stamp(13, top, ['.yyyy.', 'yyyyyy', '.oooo.'], { y: ['butter', 'b'], o: ['butter', 's'] });
    const ey = Math.round(cy - 2.6 + lookY);
    drawEyes(P, 12, 19, ey, 'mid', mood, look);
    P.stamp(10, ey + 3, ['bb'], { b: ['blush', 'b'] });
    P.stamp(20, ey + 3, ['bb'], { b: ['blush', 'b'] });
    if (mood === 'wide' || mood === 'dizzy') P.stamp(15, ey + 4, ['mm', 'mm'], { m: ['mouth', 'b'] });
    else P.stamp(14, ey + 4, ['m..m', '.mm.'], { m: ['mouth', 'b'] });
    const grid = P.toGrid(POTATO_PALETTE);
    return rot === 0 ? grid : rotateGrid(grid, rot);
}

/**
 * Spud from above (`above.ts`): a lumpy potato with its sprout's leaves spread on top, a
 * pat of butter there when it basks, and its face and feet at the front. Rolling turns
 * the whole of it a quarter at a time. 32x32, facing any of four ways.
 */
export function potatoTop(pose: Pose = {}): Grid {
    const { squash: sq = 0, mood = 'open', step = 0, sprout = 1, look = 0, lookY = 0, butter = false, rot = 0 } = pose;
    const face = pose.face ?? 'down';
    const P = new Painter(32, 32);
    // Facing away a row lower, so its bottom row is the one facing the screen: it turns
    // that way on the floor's line to climb onto the desk, and moves no pixel doing it.
    const F = new Facing(face, 16, face === 'up' ? 17 : 16);
    for (const side of [-1, 1]) {
        const ahead = step === (side < 0 ? 1 : 3) ? 0.9 : 0;
        F.ellipse(P, side * 4.2, 10.4 + ahead, 2.1, 1.5, 'feet', { hi: 0.8, sh: 0.3 });
    }
    F.ellipse(P, 0, -0.4, 9.6 + sq * 0.8, 10.4 - sq * 0.5, 'skin', { rot: -0.18 });
    F.ellipse(P, -4.8, -4.6, 4.6, 4.2, 'skin');
    F.ellipse(P, 5.0, 4.4, 4.4, 4.0, 'skin');
    P.reshade('skin', 15.4, 15.4, 12.6, 12.6, 0.84, 0.3);
    P.outline();
    for (const [u, v] of [[-6.5, 2.5], [6.8, -3.5], [-2.5, -7.5], [4.5, -7.0], [-7.5, -3.0], [2.0, 8.8]] as const) {
        const [x, y] = F.at(u, v);
        const [px, py] = [Math.floor(x), Math.floor(y)];
        if (P.m(px, py) === 'skin' && P.toneAt(px, py) !== 'o') P.set(px, py, 'spot', 'b', true);
    }
    // The sprout from above, out of the middle of the top: its leaves open away from the face.
    const leaf = (u: number, v: number, ru: number, rv: number, turn: number) =>
        F.ellipse(P, u, v, ru, rv, 'leaf', { rot: turn, hi: 0.8, sh: 0.3 });
    if (sprout >= 1) {
        leaf(-2.0, -3.8, 2.6, 1.3, 0.8);
        leaf(2.0, -3.8, 2.6, 1.3, -0.8);
    }
    if (sprout >= 2) {
        leaf(-3.6, -1.6, 2.2, 1.1, -0.3);
        leaf(3.6, -1.6, 2.2, 1.1, 0.3);
    }
    if (sprout >= 1) F.set(P, 0, -2.2, 'leaf', 's');
    if (butter) F.ellipse(P, 0, 1.2, 2.6, 2.0, 'butter', { shade: false });
    P.outline(['skin', 'spot', 'feet', 'butter']);
    if (face !== 'up') {
        F.eyes(P, 5.4, 5.2, 2, mood, look, lookY);
        for (const side of [-1, 1]) F.set(P, side * 5.0, 7.4, 'blush');
        for (let u = -1; u <= 0; u++) F.set(P, u + 0.5, 8.2, 'mouth');
    }
    const grid = P.toGrid(POTATO_PALETTE);
    return rot === 0 ? grid : rotateGrid(grid, rot);
}
