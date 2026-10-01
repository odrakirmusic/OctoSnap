// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Pengu: the penguin that used to be the app's icon (D121), retired to a pet in a red
 * scarf when the app became OctoSnap (D141). 32x32.
 */

import { Facing } from './above.js';
import { drawEyes } from './eyes.js';
import { type Grid, type Palette, type StampKey, Painter, inEllipse } from './painter.js';
import type { Pose } from './pose.js';

export const PENGUIN_PALETTE: Palette = {
    body: { h: '#565d8c', b: '#373c5c', s: '#262a42', o: '#15172a' },
    white: { h: '#ffffff', b: '#f5f5f1', s: '#dcdce4', o: '#9fa2b8' },
    beak: { h: '#ffcf70', b: '#ffa53a', s: '#e5791f', o: '#8f4510' },
    feet: { h: '#ffc163', b: '#f7953a', s: '#d8701e', o: '#7f3d0e' },
    scarf: { h: '#ff7a73', b: '#e8423f', s: '#b52631', o: '#66121b' },
    eye: { b: '#15172a' },
    eyeHi: { b: '#ffffff' },
    blush: { b: '#ffa2b8' },
};

const SCARF: StampKey = { h: ['scarf', 'h'], b: ['scarf', 'b'], s: ['scarf', 's'], o: ['scarf', 'o'] };
const BEAK: StampKey = { h: ['beak', 'h'], b: ['beak', 'b'], s: ['beak', 's'] };

export function penguin(pose: Pose = {}): Grid {
    const { squash: sq = 0, look = 0, mood = 'open', step = 0, arm = 'down', lookY = 0 } = pose;
    const P = new Painter(32, 32);
    const top = 5.4 + sq * 1.4;
    const lf = step === 1 ? -0.9 : 0;
    const rf = step === 3 ? -0.9 : 0;
    P.ellipse(12.4 - sq * 0.4, 30.5 + lf, 2.9 + sq * 0.3, 1.25, 'feet', { hi: 0.7, sh: 0.2 });
    P.ellipse(19.6 + sq * 0.4, 30.5 + rf, 2.9 + sq * 0.3, 1.25, 'feet', { hi: 0.7, sh: 0.2 });
    const flipper = (s: number) => {
        const up = arm === 'up' || arm === 'flap' || ((arm === 'wave' || arm === 'wave2') && s > 0);
        let cx = 16 + s * (9.7 + sq * 0.9);
        let cy = 20.3;
        let rot = s * -0.28;
        if (up) {
            cx = 16 + s * (10.8 + sq * 0.5);
            cy = arm === 'wave2' ? 13.0 : 15.2;
            rot = s * (arm === 'wave2' ? -1.1 : -0.8);
        }
        P.ellipse(cx, cy, 2.1, 5.2, 'body', { rot, hi: 0.8, sh: 0.3 });
    };
    flipper(-1);
    flipper(1);
    const headCy = top + 7.2;
    P.ellipse(16, headCy, 8.4 + sq * 0.6, 7.5 - sq * 0.4, 'body', { hi: 0.8, sh: 0.3 });
    P.ellipse(16, 21.2 + sq * 0.2, 10.1 + sq * 1.1, 9.3 - sq * 0.7, 'body', { hi: 0.9, sh: 0.3 });
    const faceY = headCy + 0.7;
    P.ellipse(12.7 - sq * 0.2, faceY, 4.1, 3.7, 'white', { hi: 0.95, sh: 0.2 });
    P.ellipse(19.3 + sq * 0.2, faceY, 4.1, 3.7, 'white', { hi: 0.95, sh: 0.2 });
    P.ellipse(16, faceY + 2.7, 6.0 + sq * 0.4, 2.9, 'white', { hi: 0.95, sh: 0.2 });
    P.ellipse(16, 22.8 + sq * 0.3, 7.0 + sq * 0.9, 7.2 - sq * 0.6, 'white', { hi: 0.95, sh: 0.33 });
    const sy = Math.round(faceY + 4.6);
    for (let y = sy; y <= sy + 1; y++) {
        for (let x = 0; x < 32; x++) {
            const torso = inEllipse(x, y, 16, 21.2 + sq * 0.2, 9.0 + sq * 1.1, 9.3);
            if (torso && (P.m(x, y) === 'body' || P.m(x, y) === 'white')) P.set(x, y, 'scarf', y === sy ? 'b' : 's');
        }
    }
    P.stamp(20, sy + 1, ['.bs', 'hbs', 'bbs', 'bs.', 'b.s'], SCARF, false);
    P.outline();
    P.seam('white', 'scarf', 's');
    const ey = Math.round(faceY - 1.6 + lookY);
    drawEyes(P, 11, 20, ey, 'mid', mood, look);
    const by = Math.round(faceY + 1.4);
    if (mood === 'wide' || mood === 'dizzy') P.stamp(15, by, ['hb', 'sb', 'ss'], BEAK);
    else P.stamp(15, by, ['hb', 'ss'], BEAK);
    P.stamp(9, by + 1, ['bb'], { b: ['blush', 'b'] });
    P.stamp(21, by + 1, ['bb'], { b: ['blush', 'b'] });
    return P.toGrid(PENGUIN_PALETTE);
}

/**
 * Pengu from above (`above.ts`): its black back, the head with its white eye patches and
 * the beak ahead of it, the red scarf round its neck with an end trailing, and its
 * flippers out to the sides, spread wide when it flaps. 32x32, facing any of four ways.
 */
export function penguinTop(pose: Pose = {}): Grid {
    const { squash: sq = 0, mood = 'open', step = 0, arm = 'down', look = 0, lookY = 0 } = pose;
    const face = pose.face ?? 'down';
    const P = new Painter(32, 32);
    const F = new Facing(face, 16, 15.5);
    // A waddle rocks the body a little to each side.
    const rock = step === 1 ? -0.12 : step === 3 ? 0.12 : 0;
    const spread = arm === 'up' || arm === 'flap' ? 1 : 0;
    for (const side of [-1, 1]) {
        const wave = (arm === 'wave' || arm === 'wave2') && side > 0 ? (arm === 'wave2' ? 1.2 : 0.8) : 0;
        F.ellipse(P, side * (8.1 + spread * 1.3), -1.0, 1.9, 5.6, 'body', { rot: side * (0.3 + spread * 0.75 + wave), hi: 0.8, sh: 0.3 });
    }
    // The feet peek out behind, one further back at each step.
    for (const side of [-1, 1]) {
        const back = step === (side < 0 ? 1 : 3) ? 1 : 0;
        F.ellipse(P, side * 3.0, -10.8 - back, 2.2, 1.5, 'feet', { hi: 0.7, sh: 0.2 });
    }
    // The white of its front shows at the flanks, below the black of its back.
    F.ellipse(P, 0, -1.2, 8.3 + sq * 0.6, 9.4 - sq * 0.4, 'white', { rot: rock, hi: 0.95, sh: 0.3 });
    F.ellipse(P, 0, -1.8, 7.2 + sq * 0.6, 9.2 - sq * 0.4, 'body', { rot: rock, hi: 0.86, sh: 0.3 });
    F.ellipse(P, 0, -11.2, 2.0, 1.5, 'body', { hi: 0.8, sh: 0.3 });
    // The scarf round the neck, and its end over one shoulder.
    F.ellipse(P, 0, 2.6, 6.9, 2.1, 'scarf', { hi: 0.9, sh: 0.3 });
    F.ellipse(P, 4.6, -2.6, 1.5, 3.4, 'scarf', { rot: -0.35, hi: 0.9, sh: 0.3 });
    F.ellipse(P, 0, 6.0, 6.4, 5.6, 'body', { hi: 0.8, sh: 0.3 });
    if (face !== 'up') for (const side of [-1, 1]) F.ellipse(P, side * 2.7, 7.4, 2.5, 2.3, 'white', { hi: 0.95, sh: 0.2 });
    F.ellipse(P, 0, 11.0, 1.6, 2.0, 'beak', { hi: 0.8, sh: 0.25 });
    P.outline();
    P.seam('body', 'scarf', 's');
    P.seam('white', 'body', 's');
    if (face !== 'up') {
        F.eyes(P, 7.4, 5.4, 2, mood, look, lookY);
        for (const side of [-1, 1]) F.set(P, side * 4.6, 8.8, 'blush');
    }
    return P.toGrid(PENGUIN_PALETTE);
}
