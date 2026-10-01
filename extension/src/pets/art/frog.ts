// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Hops: the frog, drawn the second time to be rounder, wider and softer -- a mochi of a
 * body on a 36x32 canvas, bead eyes on top, a tiny mouth and blush under the eyes (the
 * owner's "more kawaii, a bit wider", 2026-09-26).
 */

import { Facing } from './above.js';
import { type Grid, type Palette, type StampKey, Painter } from './painter.js';
import type { Pose } from './pose.js';

const GREEN = { h: '#c9f2ae', b: '#94da80', s: '#67b965', o: '#2c5f39' };

export const FROG_PALETTE: Palette = {
    skin: GREEN,
    hand: GREEN,
    belly: { h: '#fffdf0', b: '#f7f3cf', s: '#e6dfaa', o: '#2c5f39' },
    white: { b: '#ffffff' },
    eye: { b: '#1f1b27' },
    eyeHi: { b: '#ffffff' },
    mouth: { b: '#2c5f39' },
    inside: { b: '#b03d5a' },
    tongue: { b: '#ff7fa0' },
    blush: { b: '#ff9fb4', h: '#ffc6d2' },
    shine: { b: '#f4fdec' },
};

/** Where the tongue leaves the mouth, in art pixels from the top left, for the fly catch. */
export const FROG_MOUTH = { x: 18, y: 21 } as const;

const FACE: StampKey = { e: ['eye', 'b'], w: ['eyeHi', 'b'], o: ['mouth', 'b'] };
const MOUTH: StampKey = { o: ['mouth', 'b'], i: ['inside', 'b'], t: ['tongue', 'b'] };
const BLUSH: StampKey = { b: ['blush', 'b'], h: ['blush', 'h'] };

export function frog(pose: Pose = {}): Grid {
    const { squash: sq = 0, look = 0, mood = 'open', step = 0, mouth = 'smile', throat = 0, lookY = 0 } = pose;
    const mouthStyle = pose.mouthStyle ?? (mood === 'happy' ? 'w' : 'u');
    const P = new Painter(36, 32);
    const cx = 18;
    const bodyCy = 22.8 + sq * 1.0;
    // The haunches, then the body over them, then the eye bumps; shaded as one below.
    P.ellipse(5.3 - sq * 0.9, 26.9 + sq * 0.4, 3.9, 4.0 - sq * 0.3, 'skin');
    P.ellipse(30.7 + sq * 0.9, 26.9 + sq * 0.4, 3.9, 4.0 - sq * 0.3, 'skin');
    P.ellipse(cx, bodyCy, 15.0 + sq * 1.4, 8.7 - sq * 1.0, 'skin');
    const eyeCy = 14.9 + sq * 1.6;
    const bump = 4.9 - Math.max(0, sq) * 0.2;
    P.ellipse(11.0 - sq * 0.7, eyeCy, bump, bump, 'skin');
    P.ellipse(25.0 + sq * 0.7, eyeCy, bump, bump, 'skin');
    P.reshade('skin', cx - 1, bodyCy - 2.5, 17.5, 12.5, 0.84, 0.3);
    P.ellipse(cx, 26.9 + sq * 0.5, 9.2 + sq * 1.0, 4.7 - sq * 0.3, 'belly', { hi: 0.93, sh: 0.3 });
    if (throat > 0)
        P.ellipse(cx, 23.6 + sq, 3.2 + throat * 1.6, 1.8 + throat * 1.4, 'belly', { hi: 0.6, sh: 0.05 });
    const lf = step === 1 ? -0.9 : 0;
    const rf = step === 3 ? -0.9 : 0;
    P.ellipse(12.6, 30.7 + lf, 3.0, 1.3, 'skin', { hi: 0.7, sh: 0.2 });
    P.ellipse(23.4, 30.7 + rf, 3.0, 1.3, 'skin', { hi: 0.7, sh: 0.2 });
    P.outline();

    P.stamp(7, Math.round(eyeCy) - 3, ['..s', '.s.'], { s: ['shine', 'b'] });

    const ex = [Math.round(11.0 - sq * 0.7), Math.round(25.0 + sq * 0.7)];
    const dy = Math.max(-1, Math.min(1, Math.round(lookY)));
    const dx = Math.max(-1, Math.min(1, Math.round(look)));
    const ey = Math.round(eyeCy) - 2 + dy;
    ex.forEach((c, k) => {
        const x0 = (c ?? 0) - 2;
        switch (mood) {
            case 'blink':
                P.stamp(x0, ey + 2, ['.ooo.'], FACE);
                return;
            case 'closed':
            case 'sleep':
                P.stamp(x0, ey + 1, ['o...o', '.ooo.'], FACE);
                return;
            case 'happy':
                P.stamp(x0, ey + 1, ['.ooo.', 'o...o'], FACE);
                return;
            case 'squeeze':
                P.stamp(x0, ey, k === 0 ? ['oo...', '..oo.', 'oo...'] : ['...oo', '.oo..', '...oo'], FACE);
                return;
            case 'dizzy':
                P.stamp(x0 + 1, ey, ['e.e', '.e.', 'e.e'], FACE);
                return;
            case 'half':
                P.stamp(x0 + dx, ey + 1, ['eeeee', 'eeewe', '.eee.'], FACE);
                return;
            case 'wide':
                P.stamp(x0 + dx, ey - 2, ['.eee.', 'ewwee', 'ewwee', 'eeeee', 'eeeee', 'eeeee', '.eee.'], FACE);
                return;
            default:
                P.stamp(x0 + dx, ey - 1, ['.eee.', 'ewwee', 'ewwee', 'eeeee', 'eeewe', '.eee.'], FACE);
        }
    });

    const by = ey + 5;
    const left = ex[0] ?? 9;
    const right = ex[1] ?? 25;
    P.stamp(left - 4, by, ['.bb.', 'bbhb'], BLUSH);
    P.stamp(right + 1, by, ['.bb.', 'bhbb'], BLUSH);

    const my = Math.round(bodyCy - 3.4);
    if (mouth === 'open') P.stamp(15, my, ['.oooo.', 'oiiiio', 'oittio', '.oooo.'], MOUTH);
    else if (mouth === 'o') P.stamp(16, my, ['.oo.', 'oiio', '.oo.'], MOUTH);
    else if (mouthStyle === 'w') P.stamp(15, my, ['o.oo.o', '.o..o.'], MOUTH);
    else if (mouthStyle === 'blep') P.stamp(16, my, ['o..o', '.oo.', '.tt.'], MOUTH);
    else P.stamp(16, my, ['o..o', '.oo.'], MOUTH);
    return P.toGrid(FROG_PALETTE);
}

/**
 * Hops from above (`above.ts`): flat on the desk with its legs folded, the eye bumps on the
 * head and the eyes on top of them, the way a frog is seen from a lily pad. A hop stretches
 * its back legs out behind it; a crouch tucks them in. 36x32, facing any of four ways.
 */
export function frogTop(pose: Pose = {}): Grid {
    const { squash: sq = 0, mood = 'open', step = 0, mouth = 'smile', throat = 0, look = 0 } = pose;
    const face = pose.face ?? 'down';
    const mouthStyle = pose.mouthStyle ?? (mood === 'happy' ? 'w' : 'u');
    const P = new Painter(36, 32);
    // Facing away a row higher, so its bottom row is the one facing the screen: it turns
    // that way on the floor's line to climb onto the desk, and moves no pixel doing it.
    const F = new Facing(face, 18, face === 'up' ? 14.5 : 15.5);
    // Stretched out in a hop, tucked in a crouch; one foot a little further with each step.
    const stretch = Math.max(0, -sq);
    const tuck = Math.max(0, sq);
    for (const side of [-1, 1]) {
        const further = step === (side < 0 ? 1 : 3) ? 1 : 0;
        // The feet at the four corners, splayed out; in a hop the back ones trail behind.
        F.ellipse(P, side * (9.0 - stretch * 1.6), -6.4 - stretch * 3.0 - further * 0.8, 2.4, 1.4, 'hand', { rot: side * -0.45, hi: 0.75, sh: 0.25 });
        F.ellipse(P, side * 8.7, 3.4 + further * 0.6, 1.8, 1.25, 'hand', { rot: side * 0.5, hi: 0.75, sh: 0.25 });
    }
    // A round mochi of a back, and the eye bumps big at the front.
    F.ellipse(P, 0, -1.6, 8.9 + tuck * 0.9, 9.2 - tuck * 0.5 + stretch * 0.6, 'skin');
    for (const side of [-1, 1]) F.ellipse(P, side * 5.2, 5.6, 4.0, 3.8, 'skin');
    // The croak's sac swells out under the chin, which from above is ahead of the mouth.
    if (throat > 0) F.ellipse(P, 0, 8.2 + throat, 2.4 + throat * 1.6, 1.4 + throat * 1.3, 'belly', { hi: 0.6, sh: 0.05 });
    P.reshade('skin', 18, 15, 12.5, 12.5, 0.8, 0.32);
    P.outline();
    for (const [u, v] of [[-3.0, -4.4], [2.6, -6.2], [0.4, -2.0], [-4.6, -7.2], [5.0, -2.8]] as const) F.set(P, u, v, 'skin', 's', true);
    if (face === 'up') {
        // From behind, the tops of the eye bumps catch the light, and nothing of the face.
        for (const side of [-1, 1]) F.set(P, side * 5.2 - 0.5, 4.8, 'shine', 'b');
        return P.toGrid(FROG_PALETTE);
    }
    // The side's own big eyes, on top of the bumps: the same stamps, lit from the top left.
    const dx = face === 'down' ? Math.max(-1, Math.min(1, Math.round(look))) : 0;
    for (const side of [-1, 1]) {
        const [x, y] = F.at(side * 5.2, 5.6);
        const x0 = Math.round(x) - 2 + dx;
        const y0 = Math.round(y) - 3;
        switch (mood) {
            case 'blink':
            case 'closed':
            case 'sleep':
            case 'half':
                P.stamp(x0, y0 + 2, ['o...o', '.ooo.'], FACE);
                break;
            case 'happy':
                P.stamp(x0, y0 + 2, ['.ooo.', 'o...o'], FACE);
                break;
            case 'squeeze':
                P.stamp(x0, y0 + 1, side < 0 ? ['oo...', '..oo.', 'oo...'] : ['...oo', '.oo..', '...oo'], FACE);
                break;
            case 'dizzy':
                P.stamp(x0 + 1, y0 + 1, ['e.e', '.e.', 'e.e'], FACE);
                break;
            default:
                P.stamp(x0, y0, ['.eee.', 'ewwee', 'ewwee', 'eeeee', 'eeewe', '.eee.'], FACE);
        }
    }
    if (!F.upright) return P.toGrid(FROG_PALETTE);
    // Facing down, the face between and under the eyes: blush, and the mouth.
    const [mx, my] = F.at(0, 4.8);
    const bx = Math.round(mx);
    const by = Math.round(my);
    P.stamp(bx - 10, by - 2, ['bb'], BLUSH);
    P.stamp(bx + 8, by - 2, ['bb'], BLUSH);
    if (mouth === 'open') P.stamp(bx - 2, by - 1, ['.oo.', 'oito', '.tt.'], MOUTH);
    else if (mouth === 'o') P.stamp(bx - 1, by - 1, ['oo', 'oo'], MOUTH);
    else if (mouthStyle === 'w') P.stamp(bx - 3, by - 1, ['o.oo.o', '.o..o.'], MOUTH);
    else if (mouthStyle === 'blep') P.stamp(bx - 2, by - 1, ['o..o', '.oo.', '.tt.'], MOUTH);
    else P.stamp(bx - 2, by - 1, ['o..o', '.oo.'], MOUTH);
    return P.toGrid(FROG_PALETTE);
}
