// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The example pet pack, `data/pets/packs/mochi` (`spec/14` §14): Mochi the cat, painted by
 * the painter the five are painted with, cut into the frames a pack has, and written as a
 * pack anyone can copy and draw over. And the test that it still is one.
 *
 * Mochi has a frame for every mood, both steps of a walk, a raised paw, a look each way,
 * three facings seen from above -- left is right mirrored, as a pack may leave it -- and two
 * named frames for an act of its own, `groom`. It has no squashed frame: the engine squashes
 * it (`squashRows`).
 *
 * `npx vitest run src/pets/pack-example.test.ts` checks the files; with
 * `UPDATE_PET_PACKS=1` it writes them. The check compares pixels rather than PNG bytes, as
 * `sheets.test.ts` does.
 */

import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { nodeInflate, rgbaPng, sheetOf } from '../../testkit/png.js';
import { Facing } from './art/above.js';
import { drawEyes } from './art/eyes.js';
import { type Grid, type Palette, type StampKey, Painter, mirrorRows } from './art/painter.js';
import type { Face, Pose } from './art/pose.js';
import { decodePng } from './png.js';
import { readPack } from './pack.js';
import { packParts, previewOf } from './preview.js';

const OUT = join(dirname(fileURLToPath(import.meta.url)), '../../../data/pets/packs/mochi');
const UPDATE = process.env.UPDATE_PET_PACKS === '1';

const PALETTE: Palette = {
    fur: { h: '#ffc98a', b: '#f4a04c', s: '#cf7529', o: '#6e3712' },
    stripe: { b: '#d97c2c' },
    white: { h: '#ffffff', b: '#fbf3e8', s: '#e8d6c0', o: '#97806a' },
    ear: { b: '#ffa3b1' },
    nose: { b: '#ff7486' },
    mouth: { b: '#6e3712' },
    whisker: { b: '#fff7ec' },
    eye: { b: '#2a1a10' },
    eyeHi: { b: '#ffffff' },
    blush: { b: '#ffb0a0' },
    tongue: { b: '#ff8fa0' },
};

const EAR: StampKey = { f: ['fur', 'b'], h: ['fur', 'h'], s: ['fur', 's'], p: ['ear', 'b'] };
const LEFT_EAR = ['h....', 'hh...', 'hph..', 'hpph.', 'fppff'];

/** What Mochi's side view is asked for: a pose, and a paw at its mouth for grooming. */
interface CatPose extends Pose {
    groom?: 0 | 1;
}

/** Mochi from the front, as the five are seen from the side. 32 × 32. */
function cat(pose: CatPose = {}): Grid {
    const { mood = 'open', look = 0, step = 0, arm, groom } = pose;
    const P = new Painter(32, 32);
    // The tail, curled round behind it on the left.
    P.ellipse(7.2, 25.6, 1.8, 4.8, 'fur', { rot: 0.95, hi: 0.8, sh: 0.3 });
    P.ellipse(4.6, 22.0, 1.6, 1.8, 'fur', { hi: 0.8, sh: 0.3 });
    // A raised paw beside its head, on the right.
    const paw = groom !== undefined ? null : arm === 'wave2' ? { x: 27.0, y: 8.6, rot: 0.7 } : arm !== undefined ? { x: 26.4, y: 10.8, rot: 0.3 } : null;
    if (paw !== null) P.ellipse(paw.x, paw.y, 1.9, 3.6, 'fur', { rot: paw.rot, hi: 0.8, sh: 0.3 });
    P.ellipse(16, 22.6, 8.4, 7.4, 'fur', { hi: 0.88, sh: 0.3 });
    P.ellipse(16, 24.4, 4.8, 5.4, 'white', { hi: 0.95, sh: 0.25, only: ['fur'] });
    // Its front paws, one lifted a step at a time.
    const lf = step === 1 ? -1 : 0;
    const rf = step === 3 ? -1 : 0;
    P.ellipse(12.6, 29.9 + lf, 2.5, 1.5, 'white', { hi: 0.95, sh: 0.25 });
    P.ellipse(19.4, 29.9 + rf, 2.5, 1.5, 'white', { hi: 0.95, sh: 0.25 });
    P.ellipse(16, 12.6, 8.9, 7.2, 'fur', { hi: 0.86, sh: 0.3 });
    P.stamp(7, 3, LEFT_EAR, EAR, false);
    P.stamp(20, 3, mirrorRows(LEFT_EAR), EAR, false);
    P.ellipse(16, 16.0, 3.9, 2.5, 'white', { hi: 0.95, sh: 0.25 });
    if (groom !== undefined) {
        // A paw at its mouth, licked.
        P.ellipse(12.6 + groom, 17.4 - groom, 2.2, 2.6, 'white', { rot: -0.4, hi: 0.95, sh: 0.25 });
    }
    if (paw !== null) P.ellipse(paw.x + paw.rot * 1.6, paw.y - 2.4, 1.5, 1.3, 'white', { rot: paw.rot, hi: 0.95, sh: 0.25 });
    P.outline();
    P.seam('white', 'fur', 's');
    // Tabby stripes on the forehead.
    for (const [x, y0, y1] of [
        [14, 6, 8],
        [16, 6, 9],
        [18, 6, 8],
    ] as const)
        for (let y = y0; y <= y1; y++) P.set(x, y, 'stripe', 'b', true);
    drawEyes(P, 10, 21, 10, 'mid', groom !== undefined ? 'closed' : mood, look);
    P.stamp(15, 14, ['nn'], { n: ['nose', 'b'] });
    if (groom !== undefined) P.stamp(14, 16, ['t'], { t: ['tongue', 'b'] });
    else if (mood === 'wide' || mood === 'dizzy') P.stamp(15, 16, ['mm', 'mm'], { m: ['mouth', 'b'] });
    else P.stamp(14, 15, ['m..m', '.mm.'], { m: ['mouth', 'b'] });
    for (const y of [14, 16]) {
        for (const x of [4, 5, 6]) P.set(x, y + (x === 4 ? (y === 14 ? -1 : 1) : 0), 'whisker', 'b', true);
        for (const x of [25, 26, 27]) P.set(x, y + (x === 27 ? (y === 14 ? -1 : 1) : 0), 'whisker', 'b', true);
    }
    P.stamp(9, 14, ['bb'], { b: ['blush', 'b'] });
    P.stamp(21, 14, ['bb'], { b: ['blush', 'b'] });
    return P.toGrid(PALETTE);
}

/** Mochi from above: its back with its stripes, its head ahead with its ears, its tail behind. 32 × 32. */
function catTop(face: Face): Grid {
    const P = new Painter(32, 32);
    const F = new Facing(face, 16, 15.5);
    // The tail out behind, with a curl at its end.
    F.ellipse(P, 2.0, -10.4, 1.4, 2.8, 'fur', { rot: 0.6, hi: 0.8, sh: 0.3 });
    F.ellipse(P, 3.8, -12.6, 1.5, 1.5, 'fur', { hi: 0.8, sh: 0.3 });
    // Its paws peek out at the front and the back.
    for (const side of [-1, 1]) {
        F.ellipse(P, side * 4.4, 4.6, 1.6, 1.6, 'white', { hi: 0.95, sh: 0.25 });
        F.ellipse(P, side * 4.6, -6.8, 1.6, 1.6, 'white', { hi: 0.95, sh: 0.25 });
    }
    F.ellipse(P, 0, -1.6, 6.4, 7.8, 'fur', { hi: 0.86, sh: 0.3 });
    F.ellipse(P, 0, 6.6, 5.4, 4.8, 'fur', { hi: 0.86, sh: 0.3 });
    for (const side of [-1, 1]) F.ellipse(P, side * 3.6, 10.2, 1.6, 2.2, 'fur', { rot: side * 0.5, hi: 0.8, sh: 0.3 });
    if (face !== 'up') F.ellipse(P, 0, 9.6, 2.4, 1.6, 'white', { hi: 0.95, sh: 0.25 });
    P.outline();
    P.seam('white', 'fur', 's');
    // Stripes across its back.
    for (const v of [-4.6, -2.2, 0.2]) for (const u of [-2, -1, 0, 1, 2]) F.set(P, u, v, 'stripe');
    for (const side of [-1, 1]) F.set(P, side * 3.6, 10.6, 'ear');
    if (face !== 'up') {
        F.eyes(P, 7.4, 4.6, 2, 'open');
        F.set(P, 0, 10.4, 'nose');
    }
    return P.toGrid(PALETTE);
}

/** Each frame: what it shows, as `pet.json` says it, and how it is painted. */
const FRAMES: { says: Record<string, unknown>; paint: () => Grid }[] = [
    { says: {}, paint: () => cat() },
    { says: { mood: 'blink' }, paint: () => cat({ mood: 'blink' }) },
    { says: { mood: 'closed' }, paint: () => cat({ mood: 'closed' }) },
    { says: { mood: 'happy' }, paint: () => cat({ mood: 'happy' }) },
    { says: { mood: 'wide' }, paint: () => cat({ mood: 'wide' }) },
    { says: { mood: 'squeeze' }, paint: () => cat({ mood: 'squeeze' }) },
    { says: { mood: 'dizzy' }, paint: () => cat({ mood: 'dizzy' }) },
    { says: { step: 1 }, paint: () => cat({ step: 1 }) },
    { says: { step: 3 }, paint: () => cat({ step: 3 }) },
    { says: { mood: 'happy', step: 1 }, paint: () => cat({ mood: 'happy', step: 1 }) },
    { says: { mood: 'happy', step: 3 }, paint: () => cat({ mood: 'happy', step: 3 }) },
    { says: { arm: 'up' }, paint: () => cat({ arm: 'up', mood: 'happy' }) },
    { says: { arm: 'wave2' }, paint: () => cat({ arm: 'wave2', mood: 'happy' }) },
    { says: { look: -1 }, paint: () => cat({ look: -1 }) },
    { says: { look: 1 }, paint: () => cat({ look: 1 }) },
    { says: { view: 'top' }, paint: () => catTop('down') },
    { says: { view: 'top', face: 'up' }, paint: () => catTop('up') },
    { says: { view: 'top', face: 'right' }, paint: () => catTop('right') },
    { says: { name: 'groom-1' }, paint: () => cat({ groom: 0 }) },
    { says: { name: 'groom-2' }, paint: () => cat({ groom: 1 }) },
];

/** Eight frames to a row of the sheet. */
const ACROSS = 8;

const MANIFEST = {
    format: 1,
    name: 'Mochi',
    species: 'cat',
    size: [32, 32],
    gait: 'waddle',
    tricks: ['groom', 'wave', 'wiggle'],
    frames: FRAMES.map((f, cell) => ({ cell, ...f.says })),
    acts: {
        groom: {
            times: [2, 4],
            steps: [
                { frame: 'groom-1', ms: 260 },
                { frame: 'groom-2', ms: 220 },
            ],
        },
    },
};

/** `pet.json` as written: one frame and one step to a line, readable and still JSON. */
function manifestText(): string {
    return `${JSON.stringify(MANIFEST, null, 2)
        .replace(/\{\s+"cell": (\d+)((?:,\s+"\w+": [^,\n]+)*)\s+\}/g, (_, cell: string, rest: string) => `{ "cell": ${cell}${rest.replace(/\s+/g, ' ')} }`)
        .replace(/\{\s+"frame": ("[^"]+"),\s+"ms": (\d+)\s+\}/g, '{ "frame": $1, "ms": $2 }')
        .replace(/\[\s+(\d+),\s+(\d+)\s+\]/g, '[$1, $2]')
        .replace(/\[\s+("[a-z]+"),\s+("[a-z]+"),\s+("[a-z]+")\s+\]/g, '[$1, $2, $3]')}\n`;
}

describe('the example pet pack', () => {
    const grids = FRAMES.map(f => f.paint());
    const sheet = sheetOf(grids, ACROSS);

    if (UPDATE) {
        it('is written', () => {
            mkdirSync(OUT, { recursive: true });
            writeFileSync(join(OUT, 'sheet.png'), rgbaPng(sheet.width, sheet.height, sheet.rgba));
            writeFileSync(join(OUT, 'pet.json'), manifestText());
        });
        return;
    }

    it('has the pixels its painter paints, and its pet.json', () => {
        const path = join(OUT, 'sheet.png');
        expect(existsSync(path), `${path}: write the pack with UPDATE_PET_PACKS=1`).toBe(true);
        const got = decodePng(readFileSync(path), nodeInflate);
        expect([got.width, got.height]).toEqual([sheet.width, sheet.height]);
        expect(Buffer.from(got.rgba).equals(Buffer.from(sheet.rgba)), 'sheet.png is out of date').toBe(true);
        expect(readFileSync(join(OUT, 'pet.json'), 'utf8')).toBe(manifestText());
        expect(JSON.parse(manifestText())).toEqual(MANIFEST);
    });

    it('reads as a pack, as the extension reads it', () => {
        const result = readPack('mochi', readFileSync(join(OUT, 'pet.json'), 'utf8'), readFileSync(join(OUT, 'sheet.png')), nodeInflate);
        if (!result.ok) throw new Error(result.problem);
        const { pack } = result;
        expect(pack.spec).toMatchObject({ kind: 'mochi', name: 'Mochi', species: 'cat', width: 32, height: 32 });
        expect(pack.frames).toHaveLength(FRAMES.length);
        expect(pack.blended).toBe(0);
        // Facing left, it is facing right mirrored.
        const right = pack.spec.draw({ view: 'top', face: 'right' });
        const left = pack.spec.draw({ view: 'top', face: 'left' });
        expect(left.px[5 * 32 + 3]).toBe(right.px[5 * 32 + 28]);
        // It grooms, and its preview plays it.
        const preview = previewOf(pack.spec, packParts(pack.spec));
        expect(packParts(pack.spec)).toContain('groom');
        expect(preview.frames.some(g => g === pack.frames[18]?.grid)).toBe(true);
        const ms = preview.loop.reduce((sum, [, t]) => sum + t, 0);
        expect(ms).toBeGreaterThan(10_000);
        expect(ms).toBeLessThan(40_000);
    });
});
