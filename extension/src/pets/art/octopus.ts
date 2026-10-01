// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Omni: the octopus, OctoSnap's mascot (D141). Six arms are drawn -- the two behind the
 * body are implied -- and the two outer ones rise to hold a selection by its handles, as
 * on the app's icon. 32x32.
 */

import { Facing } from './above.js';
import { drawEyes } from './eyes.js';
import { type Grid, type Material, type Palette, Painter } from './painter.js';
import type { Pose } from './pose.js';

export const OCTOPUS_PALETTE: Palette = {
    skin: { h: '#dcb0fa', b: '#b67be6', s: '#8a4fc0', o: '#45206d' },
    spot: { b: '#9a5fcf' },
    eye: { b: '#1c1230' },
    eyeHi: { b: '#ffffff' },
    blush: { b: '#ff9fd0' },
    mouth: { b: '#45206d' },
};

/** The colours it changes through, like a real octopus's skin (`camo`). */
export const OCTOPUS_HUES: Record<string, { skin: Material; spot: Material }> = {
    teal: { skin: { h: '#9ff0e0', b: '#3fc7b0', s: '#23907f', o: '#0f4a43' }, spot: { b: '#2aa594' } },
    coral: { skin: { h: '#ffc3a8', b: '#ff8a65', s: '#d4603f', o: '#6e2a17' }, spot: { b: '#e86f4c' } },
    gold: { skin: { h: '#ffe9a0', b: '#f5c542', s: '#c99a1d', o: '#6b4e0a' }, spot: { b: '#dcae2c' } },
};

/** The arms' roots across the skirt, left to right. */
const ARM_ROOTS = [5.9, 10.2, 14.1, 17.9, 21.8, 26.1] as const;

/** Freckles on the mantle, above the eyes. */
const SPOTS: readonly (readonly [number, number])[] = [
    [8, 7],
    [9, 8],
    [9, 6],
    [23, 6],
    [24, 7],
    [22, 5],
    [12, 4],
    [20, 3],
];

export function octopus(pose: Pose = {}): Grid {
    const { squash: sq = 0, look = 0, mood = 'open', phase = 0, arms = 'down', lookY = 0, hue = '' } = pose;
    const P = new Painter(32, 32);
    const armTop = 18.6 + sq * 1.2;
    ARM_ROOTS.forEach((ax, i) => {
        const side = i < 3 ? -1 : 1;
        const outer = i === 0 || i === 5;
        const wiggle = Math.sin((phase * Math.PI) / 2 + i * 1.7) * 0.8;
        const up = arms === 'up' && outer;
        const len = (outer ? 7.2 : 8.4) - sq * 1.2;
        // Twenty-six discs along each arm, thinning to the tip. A fixed count rather than
        // a float step: `t += 0.04` drifts, and a drift is a pixel on some pose.
        for (let n = 0; n <= 25; n++) {
            const t = n / 25;
            let x: number;
            let y: number;
            let r: number;
            if (up) {
                x = ax + side * (1.0 + t * 3.0);
                y = armTop - 1 - t * 8.0;
                r = 2.3 - t * 0.8;
            } else {
                x = ax + side * (t * t * 1.6) + wiggle * t * t;
                y = armTop + t * len;
                r = 2.35 - t * 0.75;
            }
            P.ellipse(x, y, r, r, 'skin');
        }
        if (!up) {
            const tx = ax + side * 1.6 + wiggle;
            const ty = armTop + len;
            P.ellipse(tx + side * 1.3, ty - 0.2, 1.45, 1.35, 'skin');
        }
    });
    const cy = 12.4 + sq * 1.6;
    P.ellipse(16, cy, 11.1 + sq * 1.2, 10.3 - sq * 1.2, 'skin');
    P.ellipse(16, cy + 6.8, 10.4 + sq, 3.6, 'skin');
    P.reshade('skin', 16, cy + 1.2, 12.2, 15.5, 0.84, 0.36);
    for (const [x, y] of SPOTS) {
        const yy = Math.round(y + sq * 1.6);
        if (P.m(x, yy) === 'skin' && P.toneAt(x, yy) !== 's') P.set(x, yy, 'spot', 'b');
    }
    P.outline();
    const ey = Math.round(cy + 0.4 + lookY);
    drawEyes(P, 10, 21, ey, 'big', mood, look);
    P.stamp(7, ey + 4, ['bb'], { b: ['blush', 'b'] });
    P.stamp(23, ey + 4, ['bb'], { b: ['blush', 'b'] });
    if (mood === 'wide' || mood === 'dizzy') P.stamp(15, ey + 5, ['mm', 'mm'], { m: ['mouth', 'b'] });
    else P.stamp(14, ey + 5, ['m..m', '.mm.'], { m: ['mouth', 'b'] });
    const tint = OCTOPUS_HUES[hue];
    return P.toGrid(tint === undefined ? OCTOPUS_PALETTE : { ...OCTOPUS_PALETTE, skin: tint.skin, spot: tint.spot });
}

/**
 * Omni from above (`above.ts`): the round mantle, and eight arms spread about it, curling
 * at their tips. Its face is always to the front: an octopus goes arms-first any way it
 * likes, so it glides with its eyes on where it is going rather than turning round. 32x32.
 */
export function octopusTop(pose: Pose = {}): Grid {
    const { squash: sq = 0, look = 0, mood = 'open', phase = 0, arms = 'down', lookY = 0, hue = '' } = pose;
    const P = new Painter(32, 32);
    const cx = 16;
    const cy = 15.5;
    // Squashed flat is a landing, spread wider; stretched is a hop, drawn in.
    const reach = 1 + sq * 0.07;
    for (let i = 0; i < 8; i++) {
        // Half a step round, so no arm runs straight under the face.
        let angle = ((i + 0.5) * Math.PI) / 4;
        // The two arms either side of the face wave when they are up.
        if (arms === 'up' && (i === 1 || i === 2)) angle += (i === 1 ? -1 : 1) * 0.45;
        const wiggle = Math.sin((phase * Math.PI) / 2 + i * 1.7);
        // Stubby and round, as its arms are from the side, all sweeping the same way.
        for (let n = 0; n <= 24; n++) {
            const t = n / 24;
            const r = (5.4 + t * 7.4) * reach;
            const bend = angle + t * t * 0.5 + wiggle * t * 0.18;
            const w = 2.45 - t * 0.9;
            P.ellipse(cx + Math.cos(bend) * r, cy + Math.sin(bend) * r * 0.92, w, w, 'skin');
        }
    }
    P.ellipse(cx, cy - 0.6, 7.9 + sq * 0.7, 7.5 + sq * 0.4, 'skin');
    P.reshade('skin', cx, cy - 0.8, 12.5, 12.5, 0.78, 0.38);
    for (const [x, y] of [[12, 10], [13, 9], [19, 9]] as const) {
        if (P.m(x, y) === 'skin' && P.toneAt(x, y) !== 's') P.set(x, y, 'spot', 'b');
    }
    P.outline();
    const face = new Facing('down', cx, cy);
    face.eyes(P, 0.9, 6, 3, mood, look, lookY);
    const ey = Math.round(cy + 0.9 + lookY);
    P.stamp(10, ey + 2, ['bb'], { b: ['blush', 'b'] });
    P.stamp(20, ey + 2, ['bb'], { b: ['blush', 'b'] });
    if (mood === 'wide' || mood === 'dizzy') P.stamp(15, ey + 3, ['mm'], { m: ['mouth', 'b'] });
    else P.stamp(14, ey + 3, ['m..m', '.mm.'], { m: ['mouth', 'b'] });
    const tint = OCTOPUS_HUES[hue];
    return P.toGrid(tint === undefined ? OCTOPUS_PALETTE : { ...OCTOPUS_PALETTE, skin: tint.skin, spot: tint.spot });
}
