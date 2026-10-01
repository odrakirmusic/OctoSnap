// SPDX-License-Identifier: GPL-3.0-or-later

/** Morel: a red-capped mushroom that tips its cap, puffs spores and glows. 32x32. */

import { Facing } from './above.js';
import { drawEyes } from './eyes.js';
import { type Grid, type Palette, Painter } from './painter.js';
import type { Pose } from './pose.js';

export const MUSHROOM_PALETTE: Palette = {
    cap: { h: '#ff8a78', b: '#ea4a3f', s: '#b92c30', o: '#681419' },
    spot: { h: '#ffffff', b: '#fff6ea', s: '#f0dccd', o: '#fff6ea' },
    gill: { b: '#e7d3b0', s: '#cdb58c', o: '#8b7050' },
    stem: { h: '#fffcf3', b: '#f4e8cf', s: '#dcc7a2', o: '#8b7050' },
    eye: { b: '#2a1d18' },
    eyeHi: { b: '#ffffff' },
    blush: { b: '#ffab9e' },
    mouth: { b: '#7a4a38' },
};

/** The cap when it glows (`glow`): the same shading in a cool blue. */
const GLOW_CAP = { h: '#b8f2ff', b: '#62d6f0', s: '#2fa3c9', o: '#16526b' };

export function mushroom(pose: Pose = {}): Grid {
    const { squash: sq = 0, look = 0, mood = 'open', step = 0, capLift = 0, arm = 'down', lookY = 0, glow = false } = pose;
    const P = new Painter(32, 32);
    P.ellipse(16, 23.9 + sq * 0.5, 7.6 + sq * 1.1, 7.7 - sq * 0.6, 'stem', { hi: 0.9, sh: 0.28, clip: (_x, y) => y <= 31 });
    const lf = step === 1 ? -0.8 : 0;
    const rf = step === 3 ? -0.8 : 0;
    P.ellipse(12.2, 30.6 + lf, 2.3, 1.2, 'stem', { hi: 0.8, sh: 0.3 });
    P.ellipse(19.8, 30.6 + rf, 2.3, 1.2, 'stem', { hi: 0.8, sh: 0.3 });
    const leftUp = arm === 'up';
    const rightUp = arm === 'up' || arm === 'wave';
    P.ellipse(8.7 - sq * 0.8, leftUp ? 20.8 : 24.4, 1.5, 2.5, 'stem', { rot: leftUp ? 0.6 : -0.35 });
    P.ellipse(23.3 + sq * 0.8, rightUp ? 20.8 : 24.4, 1.5, 2.5, 'stem', { rot: rightUp ? -0.6 : 0.35 });
    P.reshade('stem', 16, 23.6 + sq * 0.5, 9.5, 8.6, 0.9, 0.3);
    const cy0 = 13.2 + sq * 1.5 - capLift;
    const rim = Math.round(cy0 + 2.4);
    for (let x = 6; x <= 25; x++) P.set(x, rim + 1, 'gill', Math.abs(x - 15.5) > 6 ? 's' : 'b');
    P.ellipse(16, cy0, 13.4 + sq * 1.0, 9.8 - sq * 0.8, 'cap', { hi: 0.8, sh: 0.33, clip: (_x, y) => y <= rim });
    const spot = (x: number, y: number, r: number) =>
        P.ellipse(x, y - capLift + sq * 1.2, r, r, 'spot', { hi: 0.7, sh: 0.2, only: ['cap'] });
    spot(9.4, 9.6, 2.3);
    spot(17.2, 6.4, 2.5);
    spot(24.0, 10.6, 2.0);
    spot(13.6, 12.9, 1.3);
    spot(28.1, 13.4, 1.05);
    spot(4.2, 13.2, 1.0);
    P.outline(['spot']);
    const ey = Math.round(20.6 + sq * 0.6 + lookY);
    drawEyes(P, 12, 19, ey, 'mid', mood, look);
    P.stamp(10, ey + 3, ['bb'], { b: ['blush', 'b'] });
    P.stamp(20, ey + 3, ['bb'], { b: ['blush', 'b'] });
    if (mood === 'wide' || mood === 'dizzy') P.stamp(15, ey + 3, ['mm', 'mm'], { m: ['mouth', 'b'] });
    else P.stamp(14, ey + 3, ['m..m', '.mm.'], { m: ['mouth', 'b'] });
    return P.toGrid(glow ? { ...MUSHROOM_PALETTE, cap: GLOW_CAP } : MUSHROOM_PALETTE);
}

/**
 * Morel from above (`above.ts`): its spotted cap, round from above, and under the cap's
 * front edge the top of its stem with its face peeking out and two feet. Tipping its cap
 * lifts it back to show more of the face. 32x32, facing any of four ways.
 */
export function mushroomTop(pose: Pose = {}): Grid {
    const { squash: sq = 0, mood = 'open', step = 0, capLift = 0, look = 0, lookY = 0, glow = false } = pose;
    const face = pose.face ?? 'down';
    const P = new Painter(32, 32);
    const F = new Facing(face, 16, 16);
    for (const side of [-1, 1]) {
        const ahead = step === (side < 0 ? 1 : 3) ? 0.9 : 0;
        F.ellipse(P, side * 3.4, 11.4 + ahead, 1.9, 1.5, 'stem', { hi: 0.8, sh: 0.3 });
    }
    F.ellipse(P, 0, 7.6 + capLift * 0.4, 6.4 + sq * 0.6, 4.2, 'stem', { hi: 0.9, sh: 0.28 });
    const back = -2.4 - capLift * 0.9;
    F.ellipse(P, 0, back, 10.8 + sq * 0.8, 10.2 + sq * 0.5, 'cap', { hi: 0.8, sh: 0.33 });
    const spot = (u: number, v: number, r: number) => {
        const [x, y] = F.at(u, v + back + 2.4);
        P.ellipse(x, y, r, r, 'spot', { hi: 0.7, sh: 0.2, only: ['cap'] });
    };
    spot(-4.6, -5.6, 2.2);
    spot(3.8, -6.4, 2.4);
    spot(0.2, -1.6, 1.7);
    spot(6.8, -0.8, 1.5);
    spot(-6.9, 0.6, 1.3);
    spot(-2.6, 3.6, 1.1);
    spot(3.4, 3.2, 1.0);
    P.outline(['spot']);
    // The cap's rim over the stem is a line of shadow on the stem.
    P.seam('stem', 'cap', 's');
    if (face !== 'up') {
        F.eyes(P, 8.4 + capLift * 0.4, 4.4, 2, mood, look, lookY);
        for (const side of [-1, 1]) F.set(P, side * 4.4, 9.6 + capLift * 0.4, 'blush');
    }
    return P.toGrid(glow ? { ...MUSHROOM_PALETTE, cap: GLOW_CAP } : MUSHROOM_PALETTE);
}
