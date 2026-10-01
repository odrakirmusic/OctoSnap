// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * `spec/14` §3's rule, over the whole ladder: one art pixel is a whole number of physical
 * pixels at every scale, and a pet placed anywhere starts on a physical pixel. No scale is
 * the default case (`spec/01` §1), so every rung is checked the same way.
 */

import { describe, expect, it } from 'vitest';

import { PETS, PET_KINDS } from './art/index.js';
import {
    SIZE_BASES,
    artPixel,
    artPixelLogical,
    gridToRGBA,
    isOpaque,
    isPetSize,
    opaqueRects,
    parseColour,
    snapToPhysical,
} from './frames.js';

/** GNOME's scales as Mutter hands them over: single precision, so 4/3 is not 4/3. */
const LADDER = [1, 1.25, 1.3333333730697632, 1.5, 1.6666666269302368, 2];

/** Physical pixels per art pixel, from the plan the owner approved. */
const MEDIUM = [3, 4, 4, 5, 5, 6];

describe('art pixels', () => {
    it('are the approved sizes at Medium on every rung', () => {
        expect(LADDER.map(s => artPixel('medium', s))).toEqual(MEDIUM);
    });

    it('come in five sizes, read from the settings by name alone', () => {
        expect(['tiny', 'small', 'medium', 'large', 'huge'].every(isPetSize)).toBe(true);
        // A stored value that is an object's own property name is still not a size.
        expect(['toString', 'constructor', '__proto__', 'Medium', ''].some(isPetSize)).toBe(false);
    });

    it('are whole physical pixels at every size and scale, and at least one', () => {
        for (const size of Object.keys(SIZE_BASES) as (keyof typeof SIZE_BASES)[]) {
            for (const scale of LADDER) {
                const k = artPixel(size, scale);
                expect(Number.isInteger(k)).toBe(true);
                expect(k).toBeGreaterThanOrEqual(1);
                // A pet is never much smaller on a sharper screen: three-quarters of its
                // size at 100 % at the least, which Tiny is at 133 %, as Mutter's single
                // precision has it a hair under.
                expect(k / scale).toBeGreaterThan(SIZE_BASES[size] * 0.75 - 1e-6);
            }
        }
    });

    it('come back to whole physical pixels from their logical size, as Mutter multiplies', () => {
        for (const scale of LADDER) {
            for (const kind of PET_KINDS) {
                const u = artPixelLogical('medium', scale, scale);
                const width = PETS[kind].width * u;
                // Mutter multiplies in single precision.
                const physical = Math.fround(Math.fround(width) * Math.fround(scale));
                expect(Math.abs(physical - Math.round(physical))).toBeLessThan(1e-3);
                expect(Math.round(physical)).toBe(PETS[kind].width * artPixel('medium', scale));
            }
        }
    });

    it('are physical pixels as they are when the stage is laid out in physical pixels', () => {
        expect(artPixelLogical('medium', 2, 1)).toBe(6);
    });
});

describe('snapping', () => {
    it('lands every position on a physical pixel of its monitor', () => {
        for (const scale of LADDER) {
            for (const origin of [0, 1536, 1920, 2560]) {
                for (let x = origin; x < origin + 50; x += 0.37) {
                    const snapped = snapToPhysical(x, origin, scale);
                    const physical = Math.fround(Math.fround(snapped - origin) * Math.fround(scale));
                    expect(Math.abs(physical - Math.round(physical))).toBeLessThan(1e-3);
                    // And never more than half a physical pixel away.
                    expect(Math.abs(snapped - x)).toBeLessThanOrEqual(0.5 / scale + 1e-9);
                }
            }
        }
    });

    it('leaves a position already on the grid where it is', () => {
        expect(snapToPhysical(100, 0, 1.25)).toBe(100);
        expect(snapToPhysical(0.8, 0, 1.25)).toBeCloseTo(0.8, 12);
    });
});

describe('textures', () => {
    it('are premultiplied RGBA, one texel per art pixel', () => {
        const grid = { w: 2, h: 1, px: ['#ff0000', '#00000080'] };
        expect([...gridToRGBA(grid)]).toEqual([255, 0, 0, 255, 0, 0, 0, 128]);
        expect([...gridToRGBA({ w: 1, h: 1, px: ['#ffffff80'] })]).toEqual([128, 128, 128, 128]);
        expect([...gridToRGBA({ w: 1, h: 1, px: [null] })]).toEqual([0, 0, 0, 0]);
    });

    it('mirror left to right when asked', () => {
        const grid = { w: 2, h: 1, px: ['#ff0000', null] };
        expect([...gridToRGBA(grid, true)]).toEqual([0, 0, 0, 0, 255, 0, 0, 255]);
    });

    it('refuse what is not a colour', () => {
        expect(() => parseColour('red')).toThrow();
        expect(parseColour('#0a0b0c')).toEqual([10, 11, 12, 255]);
    });

    it('are the size of every pet', () => {
        for (const kind of PET_KINDS) {
            const grid = PETS[kind].draw({});
            expect(gridToRGBA(grid).length).toBe(grid.w * grid.h * 4);
        }
    });
});

describe('picking', () => {
    it('covers exactly the drawn pixels, for every pet in every mood', () => {
        for (const kind of PET_KINDS) {
            for (const mood of ['open', 'happy', 'wide', 'sleep'] as const) {
                for (const mirror of [false, true]) {
                    const grid = PETS[kind].draw({ mood, squash: 0.4 });
                    const rects = opaqueRects(grid, mirror);
                    const covered = new Set<number>();
                    for (const r of rects) {
                        for (let y = r.y; y < r.y + r.h; y++) {
                            for (let x = r.x; x < r.x + r.w; x++) {
                                const i = y * grid.w + x;
                                expect(covered.has(i)).toBe(false);
                                covered.add(i);
                            }
                        }
                    }
                    for (let y = 0; y < grid.h; y++) {
                        for (let x = 0; x < grid.w; x++) {
                            const source = mirror ? grid.w - 1 - x : x;
                            const drawn = grid.px[y * grid.w + source] !== null;
                            expect(covered.has(y * grid.w + x)).toBe(drawn);
                        }
                    }
                    // Merged: far fewer rectangles than rows of runs.
                    expect(rects.length).toBeLessThan(grid.h * 3);
                }
            }
        }
    });

    it('answers for a single pixel', () => {
        const grid = { w: 2, h: 2, px: ['#000000', null, null, '#000000'] };
        expect(isOpaque(grid, 0, 0)).toBe(true);
        expect(isOpaque(grid, 1, 0)).toBe(false);
        expect(isOpaque(grid, 1.5, 1.9)).toBe(true);
        expect(isOpaque(grid, -1, 0)).toBe(false);
        expect(isOpaque(grid, 2, 0)).toBe(false);
    });
});
