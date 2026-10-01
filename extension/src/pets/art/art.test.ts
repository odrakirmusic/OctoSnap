// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The pets' pixels. The art is code, so the thing to hold is that it is *the same* code
 * every time: a pose paints the same pixels on every run and on every machine, which is
 * what lets the extension paint at runtime and the build paint the sheets for Settings
 * without the two ever disagreeing (`spec/14` §2).
 */

import { createHash } from 'node:crypto';

import { describe, expect, it } from 'vitest';

import { EMOTE_NAMES, emote, fly, shadow } from './emotes.js';
import { PETS, PET_KINDS, baselineOf, isPetKind } from './index.js';
import type { Grid } from './painter.js';
import { mirrorGrid, rotateGrid } from './painter.js';
import { type Pose, poseKey, quantizePose } from './pose.js';

function hash(grid: Grid): string {
    return createHash('sha256').update(`${grid.w}x${grid.h}:${grid.px.map(c => c ?? '-').join(',')}`).digest('hex').slice(0, 16);
}

const MOODS = ['open', 'blink', 'half', 'closed', 'sleep', 'happy', 'squeeze', 'wide', 'dizzy'] as const;

describe('every pet', () => {
    for (const kind of PET_KINDS) {
        const spec = PETS[kind];

        it(`${kind} paints a grid of its own size in every mood`, () => {
            for (const mood of MOODS) {
                for (const squash of [-1, 0, 1]) {
                    const grid = spec.draw({ mood, squash });
                    expect(grid.w).toBe(spec.width);
                    expect(grid.h).toBe(spec.height);
                    expect(grid.px.length).toBe(spec.width * spec.height);
                    // Something is drawn, and not everything.
                    const drawn = grid.px.filter(c => c !== null).length;
                    expect(drawn).toBeGreaterThan(spec.width * spec.height * 0.25);
                    expect(drawn).toBeLessThan(spec.width * spec.height);
                }
            }
        });

        it(`${kind} paints the same pixels for the same pose, every time`, () => {
            const pose: Pose = { mood: 'happy', squash: 0.3, look: 1 };
            expect(hash(spec.draw(pose))).toBe(hash(spec.draw({ ...pose })));
        });

        it(`${kind} changes when it blinks, and blinks the same both times`, () => {
            const open = hash(spec.draw({}));
            const blink = hash(spec.draw({ mood: 'blink' }));
            expect(blink).not.toBe(open);
            expect(hash(spec.draw({ mood: 'blink' }))).toBe(blink);
        });

        it(`${kind} has its feet near the bottom, and the engine knows the row`, () => {
            // The feet are what the floor is measured to, so the row under them is found
            // from the pixels rather than assumed to be the canvas's last.
            const grid = spec.draw({});
            const row = baselineOf(kind);
            expect(row).toBeGreaterThan(grid.h - 5);
            expect(row).toBeLessThanOrEqual(grid.h);
            expect(grid.px.slice((row - 1) * grid.w, row * grid.w).some(c => c !== null)).toBe(true);
            expect(grid.px.slice(row * grid.w).every(c => c === null)).toBe(true);
        });
    }

    it('knows its own names', () => {
        expect(PET_KINDS).toEqual(['octopus', 'penguin', 'frog', 'mushroom', 'potato']);
        expect(isPetKind('frog')).toBe(true);
        expect(isPetKind('dragon')).toBe(false);
    });
});

const FACES = ['down', 'up', 'left', 'right'] as const;

describe('every pet from above', () => {
    for (const kind of PET_KINDS) {
        const spec = PETS[kind];

        it(`${kind} paints a grid of its own size facing every way, in every mood`, () => {
            for (const face of FACES) {
                for (const mood of MOODS) {
                    for (const squash of [-1, 0, 1]) {
                        const grid = spec.draw({ view: 'top', face, mood, squash });
                        expect(grid.w).toBe(spec.width);
                        expect(grid.h).toBe(spec.height);
                        const drawn = grid.px.filter(c => c !== null);
                        expect(drawn.length).toBeGreaterThan(spec.width * spec.height * 0.2);
                        expect(drawn.length).toBeLessThan(spec.width * spec.height);
                        // Opaque or clear, as from the side: no pixel a blend with the wallpaper.
                        expect(drawn.every(c => c?.length === 7)).toBe(true);
                    }
                }
            }
        });

        it(`${kind} paints the same pixels for the same pose, every time`, () => {
            const pose: Pose = { view: 'top', face: 'left', mood: 'happy', squash: 0.3, step: 1 };
            expect(hash(spec.draw(pose))).toBe(hash(spec.draw({ ...pose })));
        });

        it(`${kind} blinks where its eyes show, and ${kind === 'octopus' ? 'shows them every way' : 'shows none from behind'}`, () => {
            for (const face of FACES) {
                const open = hash(spec.draw({ view: 'top', face }));
                const blink = hash(spec.draw({ view: 'top', face, mood: 'blink' }));
                if (face === 'up' && kind !== 'octopus') expect(blink).toBe(open);
                else expect(blink).not.toBe(open);
            }
        });

        it(`${kind} ${kind === 'octopus' ? 'is round, the same every way' : 'looks different every way it faces'}`, () => {
            const faces = new Set(FACES.map(face => hash(spec.draw({ view: 'top', face }))));
            // The octopus from above is its mantle and its arms all round, with no front to it.
            expect(faces.size).toBe(kind === 'octopus' ? 1 : 4);
            expect(hash(spec.draw({ view: 'top' }))).not.toBe(hash(spec.draw({})));
        });

        it(`${kind} sits on the desk on a row the engine knows`, () => {
            const grid = spec.draw({ view: 'top' });
            const row = baselineOf(kind, 'top');
            expect(row).toBeGreaterThan(grid.h - 9);
            expect(grid.px.slice((row - 1) * grid.w, row * grid.w).some(c => c !== null)).toBe(true);
            expect(grid.px.slice(row * grid.w).every(c => c === null)).toBe(true);
        });

        it(`${kind} facing away sits on that row too, where it turns to climb onto the desk`, () => {
            // `climb` turns it from the side to facing away on the floor's line, and
            // `descend` turns it back facing the screen: both on the baseline, to the pixel.
            const grid = spec.draw({ view: 'top', face: 'up' });
            const row = baselineOf(kind, 'top');
            expect(grid.px.slice((row - 1) * grid.w, row * grid.w).some(c => c !== null)).toBe(true);
            expect(grid.px.slice(row * grid.w).every(c => c === null)).toBe(true);
        });
    }
});

/**
 * The prototype the owner approved on 2026-09-26, fixed. A change to any of these is a
 * change to how a pet looks, which should be on purpose: update the hash in the same
 * commit as the art, and say so.
 */
describe('the approved look', () => {
    const golden: Record<string, string> = {
        octopus: '5837d9035ecff7fd',
        penguin: '142af89339067334',
        frog: 'cb60c691166d78f0',
        mushroom: '8ca0be0fb1c98436',
        potato: '01984c5aa2852dc4',
    };
    for (const kind of PET_KINDS) {
        it(`${kind} at rest is the approved pixels`, () => {
            expect(hash(PETS[kind].draw({}))).toBe(golden[kind]);
        });
    }

    // From above, facing the screen: drawn on 2026-09-27 for the pets allowed anywhere.
    const above: Record<string, string> = {
        octopus: '555377bf67b5da36',
        penguin: '6bc3a6f1306d1c7f',
        frog: '93d0f10abf8dae1b',
        mushroom: '1555ec82add88c68',
        potato: '09aec379d626c71a',
    };
    for (const kind of PET_KINDS) {
        it(`${kind} at rest from above is the drawn pixels`, () => {
            expect(hash(PETS[kind].draw({ view: 'top' }))).toBe(above[kind]);
        });
    }
});

describe('poses', () => {
    it('are keyed the same however they were written', () => {
        expect(poseKey({ mood: 'happy', squash: 0.8 })).toBe(poseKey({ squash: 0.8, mood: 'happy' }));
        expect(poseKey({})).toBe('');
        // Defaults are left out, so a pose that says the default is the plain pose.
        expect(poseKey({ mood: 'open', squash: 0, look: 0 })).toBe('');
        expect(poseKey({ squash: -0 })).toBe('');
    });

    it('are quantized to the steps the art has', () => {
        expect(quantizePose({ squash: 0.83 }).squash).toBe(0.8);
        expect(quantizePose({ squash: 7 }).squash).toBe(1);
        expect(quantizePose({ look: -0.7 }).look).toBe(-1);
        expect(quantizePose({ phase: 5 }).phase).toBe(1);
        expect(quantizePose({ rot: -1 }).rot).toBe(3);
        expect(quantizePose({ step: 2 }).step).toBe(0);
        expect(poseKey({ squash: 0.83 })).toBe(poseKey({ squash: 0.79 }));
    });

    it('paint the same as their quantized selves', () => {
        for (const kind of PET_KINDS) {
            const pose: Pose = { squash: 0.34, look: 0.6, mood: 'wide' };
            // The engine paints the quantized pose; this is what makes the cache honest.
            expect(poseKey(quantizePose(pose))).toBe(poseKey(pose));
            expect(PETS[kind].draw(quantizePose(pose)).w).toBe(PETS[kind].width);
        }
    });
});

describe('the other pieces', () => {
    it('every emote is a small drawn grid', () => {
        for (const name of EMOTE_NAMES) {
            const grid = emote(name);
            expect(grid.w).toBeGreaterThan(2);
            expect(grid.w).toBeLessThanOrEqual(8);
            expect(grid.px.some(c => c !== null)).toBe(true);
        }
    });

    it('the fly flaps', () => {
        expect(hash(fly(true))).not.toBe(hash(fly(false)));
    });

    it('the shadow is as wide as asked, and see-through', () => {
        const grid = shadow(20);
        expect(grid.w).toBe(20);
        expect(grid.px.filter(c => c !== null).every(c => c?.length === 9)).toBe(true);
    });

    it('a grid turned four times, or mirrored twice, is itself', () => {
        const grid = PETS.potato.draw({ mood: 'happy' });
        expect(hash(rotateGrid(grid, 4))).toBe(hash(grid));
        expect(hash(rotateGrid(rotateGrid(grid, 1), 3))).toBe(hash(grid));
        expect(hash(mirrorGrid(mirrorGrid(grid)))).toBe(hash(grid));
        expect(hash(rotateGrid(grid, 1))).not.toBe(hash(grid));
    });
});
