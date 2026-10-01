// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * An area capture's repaint, checked pixel by pixel against the stage D116 measured on a
 * nested shell. A repaint of a region takes a texture of the region's extent times the
 * scale, rounded to nearest, whose first pixel is the region's origin times the scale,
 * rounded down. Its viewport is the stage's length times the scale, truncated, so a logical
 * coordinate lands at that many pixels over the stage's length. A monitor's background
 * paints a pixel when its centre is inside both the monitor and the region: on a near edge
 * counts as inside, on a far edge as outside. Mutter multiplies in single precision
 * throughout. It is written out again here rather than borrowed from `repaint.ts`.
 */

import { describe, expect, it } from 'vitest';

import type { Rect, Size } from './place.js';
import { origin, pastPainted, repaintPlan } from './repaint.js';

const f32 = Math.fround;
const S43 = 1.3333333730697632;
const S53 = 1.6666666269302368;

/** The capture's own size: what `get_capture_final_size` says, `roundf` of the extent. */
function captureSize(rect: Rect, scale: number): Size {
    return {
        width: Math.round(f32(rect.width * scale)),
        height: Math.round(f32(rect.height * scale)),
    };
}

/** A desk: its monitors, the one at the scale under test, and the box around them. */
interface Desk {
    name: string;
    monitors: Rect[];
    panel: Rect;
    stage: Size;
}

function desk(name: string, monitors: Rect[], panel: number): Desk {
    const width = Math.max(...monitors.map(m => m.x + m.width));
    const height = Math.max(...monitors.map(m => m.y + m.height));
    return { name, monitors, panel: monitors[panel]!, stage: { width, height } };
}

/**
 * A 1920 × 1200 panel at `scale`, laid out five ways against a 2560 × 1440 screen at 1:
 * alone, below the screen at x 443 as on the owner's desk, at the origin with the screen
 * to its right, to the right of the screen, and below it flush with its right edge.
 */
function desks(scale: number): Desk[] {
    const w = Math.round(1920 / scale);
    const h = Math.round(1200 / scale);
    const screen = { x: 0, y: 0, width: 2560, height: 1440 };
    return [
        desk('alone', [{ x: 0, y: 0, width: w, height: h }], 0),
        desk('owner', [screen, { x: 443, y: 1440, width: w, height: h }], 1),
        desk('origin', [{ x: 0, y: 0, width: w, height: h }, { ...screen, x: w }], 0),
        desk('right', [screen, { x: 2560, y: 0, width: w, height: h }], 1),
        desk('below', [screen, { x: 2560 - w, y: 1440, width: w, height: h }], 1),
    ];
}

/** Stage pixels per logical pixel: the viewport's truncated length over the stage's. */
function perLogical(stageLength: number, scale: number): number {
    return Math.trunc(f32(stageLength * scale)) / stageLength;
}

/**
 * Whether a repaint clipped to `clip`, or to nothing, paints the stage pixel whose centre
 * is at `x`, `y`, given the stage pixels per logical pixel `across` and `down`.
 */
function paints(
    desk: Desk,
    across: number,
    down: number,
    clip: Rect | null,
    x: number,
    y: number,
): boolean {
    const inside = (r: Rect) =>
        x >= r.x * across && x < (r.x + r.width) * across &&
        y >= r.y * down && y < (r.y + r.height) * down;
    return (clip === null || inside(clip)) && desk.monitors.some(inside);
}

/** The stage pixel a capture of `rect` starts at, as `screenshot_area` takes it. */
function firstPixel(rect: Rect, scale: number): { x: number; y: number } {
    return { x: Math.floor(f32(rect.x * scale)), y: Math.floor(f32(rect.y * scale)) };
}

/**
 * The pixels of a capture of `rect` that a repaint clipped to `clip` leaves unpainted, as
 * `x,y` within the capture, on the outermost `rings` rings. Deeper than that, a region on a
 * monitor is always painted.
 */
function unpainted(
    desk: Desk,
    rect: Rect,
    scale: number,
    clip: Rect | null,
    rings = 2,
): Set<string> {
    const first = firstPixel(rect, scale);
    const { width, height } = captureSize(rect, scale);
    const across = perLogical(desk.stage.width, scale);
    const down = perLogical(desk.stage.height, scale);
    const missed = new Set<string>();
    const look = (i: number, j: number) => {
        if (i < 0 || j < 0 || i >= width || j >= height) return;
        const x = first.x + i + 0.5;
        const y = first.y + j + 0.5;
        if (!paints(desk, across, down, clip, x, y)) missed.add(`${i},${j}`);
    };
    for (let ring = 0; ring < rings; ring++) {
        for (let i = ring; i < width - ring; i++) {
            look(i, ring);
            look(i, height - 1 - ring);
        }
        for (let j = ring; j < height - ring; j++) {
            look(ring, j);
            look(width - 1 - ring, j);
        }
    }
    return missed;
}

/** The far column and row a repaint of the region itself, `screenshot_area`'s, missed. */
function farMisses(desk: Desk, rect: Rect, scale: number): string[] {
    const { width, height } = captureSize(rect, scale);
    const missed = unpainted(desk, rect, scale, rect, 1);
    const out: string[] = [];
    if (missed.has(`${width - 1},${Math.floor(height / 2)}`)) out.push('last column');
    if (missed.has(`${Math.floor(width / 2)},${height - 1}`)) out.push('last row');
    return out;
}

/**
 * What is wrong with a capture of `rect` at `scale` on `desk`, if anything: whether it
 * keeps the pixels `screenshot_area` kept, and whether every one it misses is one its
 * fill makes up for. And how many it misses.
 */
function faults(desk: Desk, rect: Rect, scale: number): { out: string[]; missed: number } {
    const out: string[] = [];
    const size = captureSize(rect, scale);
    const { paint, keep } = repaintPlan(rect, scale, size);
    // The same pixels as `screenshot_area`: its first, and as many, all of them inside the
    // repaint's texture.
    const first = firstPixel(rect, scale);
    if (origin(paint.x, scale) + keep.x !== first.x || origin(paint.y, scale) + keep.y !== first.y)
        out.push('keeps from another pixel');
    if (keep.width !== size.width || keep.height !== size.height)
        out.push(`keeps ${keep.width} x ${keep.height}`);
    const texture = captureSize(paint, scale);
    if (keep.x < 1 || keep.y < 1 || keep.x + keep.width > texture.width ||
        keep.y + keep.height > texture.height)
        out.push(`keeps ${JSON.stringify(keep)} of ${JSON.stringify(texture)}`);

    // The repaint misses nothing the stage paints, and the stage misses only pixels on the
    // outermost ring, on a side [`pastPainted`] names: so each has a painted one inside it,
    // on the ring within, to take its colour from.
    const live = unpainted(desk, rect, scale, paint);
    const frozen = unpainted(desk, rect, scale, null);
    if ([...live].sort().join(' ') !== [...frozen].sort().join(' '))
        out.push('the repaint misses what the stage paints');
    const sides = pastPainted(rect, scale, desk.stage, desk.monitors);
    const right = size.width - 1;
    const bottom = size.height - 1;
    for (const key of frozen) {
        const [i, j] = key.split(',').map(Number) as [number, number];
        if (i !== 0 && j !== 0 && i !== right && j !== bottom)
            out.push(`misses ${key}, inside the outermost ring`);
        else if (!((i === 0 && sides.left) || (i === right && sides.right) ||
            (j === 0 && sides.top) || (j === bottom && sides.bottom)))
            out.push(`misses ${key} on a side not named`);
    }
    // A side named is missed all along, corners aside, or its fill in the frozen capture
    // would cover up pixels that were painted.
    const along = (named: boolean, keys: string[]) => {
        if (named && keys.some(key => !frozen.has(key))) out.push(`names ${JSON.stringify(sides)}`);
    };
    const down = Array.from({ length: Math.max(0, size.height - 2) }, (_, j) => j + 1);
    const across = Array.from({ length: Math.max(0, size.width - 2) }, (_, i) => i + 1);
    along(sides.left, down.map(j => `0,${j}`));
    along(sides.right, down.map(j => `${right},${j}`));
    along(sides.top, across.map(i => `${i},0`));
    along(sides.bottom, across.map(i => `${i},${bottom}`));
    if (Number.isInteger(scale) && frozen.size > 0)
        out.push(`misses ${frozen.size} at a whole-number scale`);
    return { out, missed: frozen.size };
}

/** Everything but a desk's panel at 1.25, which is what the owner's frames came from. */
const OWNER = desks(1.25)[1]!;
const ORIGIN = desks(1.25)[2]!;

describe('repaintPlan', () => {
    it('reproduces the unpainted edges the nested shell showed, and paints them now', () => {
        const cases: [Rect, Desk, string[]][] = [
            [{ x: 568, y: 1605, width: 230, height: 265 }, OWNER, ['last column']],
            [{ x: 1823, y: 2240, width: 152, height: 154 }, OWNER, ['last row']],
            [{ x: 1811, y: 2268, width: 161, height: 122 }, OWNER, ['last row']],
            [{ x: 569, y: 1605, width: 230, height: 265 }, OWNER, []],
            [{ x: 568, y: 1605, width: 231, height: 265 }, OWNER, []],
            [{ x: 300, y: 400, width: 230, height: 122 }, ORIGIN, ['last column', 'last row']],
            [{ x: 301, y: 400, width: 230, height: 122 }, ORIGIN, ['last row']],
        ];
        for (const [rect, where, measured] of cases) {
            // `screenshot_area` repainted the region itself. Its near edges came back painted
            // even where a centre sat outside them, so only the far ones are compared.
            expect(farMisses(where, rect, 1.25)).toEqual(measured);
            const { paint } = repaintPlan(rect, 1.25, captureSize(rect, 1.25));
            expect(unpainted(where, rect, 1.25, paint).size).toBe(0);
        }
    });

    it('keeps the pixels screenshot_area kept, at every scale, and misses only the edge', () => {
        // Mutter's ladder in its own float spellings, and the odd ones a panel's size makes.
        const scales = [
            1, 1.25, S43, 1.5, S53, 1.75, 2, 2.25, 2.5, 2.6666667461395264, 3, 3.5, 4,
            f32(1920 / 1096), f32(1920 / 1488), 1.6, 2.4,
        ];
        let seed = 116;
        const next = (n: number): number => {
            seed = (seed * 48271) % 2147483647;
            return seed % n;
        };
        const failures: string[] = [];
        let checked = 0;
        let past = 0;
        for (const scale of scales) {
            for (const where of desks(scale)) {
                const panel = where.panel;
                for (let i = 0; i < 120; i++) {
                    const width = 1 + next(Math.floor(panel.width / 2));
                    const height = 1 + next(Math.floor(panel.height / 2));
                    // A quarter of the regions against the panel's far edges, a quarter
                    // against its near ones, as a drag clamped to the panel ends up.
                    const room = { x: panel.width - width, y: panel.height - height };
                    const x = i % 4 === 1 ? room.x : i % 4 === 3 ? 0 : next(room.x + 1);
                    const y = i % 4 === 2 ? room.y : i % 4 === 3 ? 0 : next(room.y + 1);
                    const rect = { x: panel.x + x, y: panel.y + y, width, height };
                    const at = `${JSON.stringify(rect)} on ${where.name} at ${scale}`;
                    const { out, missed } = faults(where, rect, scale);
                    failures.push(...out.map(fault => `${at}: ${fault}`));
                    if (missed > 0) past++;
                    checked++;
                }
            }
        }
        expect(failures.slice(0, 5)).toEqual([]);
        expect(checked).toBe(scales.length * 5 * 120);
        // The sweep does reach pixels the stage has nothing for, or it proves nothing about
        // them.
        expect(past).toBeGreaterThan(100);
        // About a second alone, and several with the other files running beside it.
    }, 30_000);

    it('grows the repaint by a logical pixel on every side, the stage or not', () => {
        const corner = repaintPlan({ x: 0, y: 0, width: 230, height: 122 }, 1.25,
            { width: 288, height: 153 });
        expect(corner.paint).toEqual({ x: -1, y: -1, width: 232, height: 124 });
        // -1 x 1.25 is -1.25, which the viewport truncates to -1, so the capture starts one
        // pixel in.
        expect(corner.keep).toEqual({ x: 1, y: 1, width: 288, height: 153 });
        const inside = repaintPlan({ x: 568, y: 1605, width: 230, height: 265 }, 1.25,
            { width: 288, height: 331 });
        // 567 x 1.25 is 708.75, so the repaint starts at 708 and the capture at 710.
        expect(inside.paint).toEqual({ x: 567, y: 1604, width: 232, height: 267 });
        expect(inside.keep).toEqual({ x: 2, y: 1, width: 288, height: 331 });
    });
});

describe('pastPainted', () => {
    it('finds the pixel past the stage that no repaint reaches', () => {
        // The panel at 5/3 to the right of the screen: 3712 x 5/3 is 6186.67, so the
        // viewport ends at 6186. A region from 2562 to the stage's edge starts at 4270 and
        // is 1917 pixels wide, so its last pixel is 6186: past the viewport.
        const where = desks(S53)[3]!;
        expect(where.stage).toEqual({ width: 3712, height: 1440 });
        const rect = { x: 2562, y: 100, width: 1150, height: 200 };
        expect(captureSize(rect, S53)).toEqual({ width: 1917, height: 333 });
        expect(pastPainted(rect, S53, where.stage, where.monitors))
            .toEqual({ left: false, right: true, top: false, bottom: false });
        // One logical pixel to the left, and the last pixel is 6185: inside.
        const left = pastPainted({ ...rect, x: 2561 }, S53, where.stage, where.monitors);
        expect(left.right).toBe(false);
    });

    it('finds the pixel past a monitor with nothing beyond it', () => {
        // The owner's desk at 1.5: the panel is 1280 wide at 443, so it runs from 664.5 to
        // 2584.5. A region from an even coordinate to its right edge rounds up to 2585 and
        // ends on the pixel whose centre is its edge; from an odd one it ends a pixel short.
        const where = desks(1.5)[1]!;
        expect(where.panel).toEqual({ x: 443, y: 1440, width: 1280, height: 800 });
        const sides = (rect: Rect) => pastPainted(rect, 1.5, where.stage, where.monitors);
        const even = { x: 1000, y: 1500, width: 723, height: 300 };
        expect(sides(even).right).toBe(true);
        expect(sides({ ...even, x: 1001, width: 722 }).right).toBe(false);
        // Nothing lies under the panel either, but 1440 and 2240 are whole pixels at 1.5.
        expect(sides({ ...even, y: 1940 }).bottom).toBe(false);
    });

    it('finds nothing past where a monitor meets another', () => {
        // The panel at the origin with the screen to its right: its right edge is the
        // screen's left, so the pixel past it is the screen's.
        for (const scale of [1.25, S43, 1.5, S53]) {
            const where = desks(scale)[2]!;
            const panel = where.panel;
            for (let x = panel.width - 40; x < panel.width - 30; x++) {
                const rect = { x, y: 10, width: panel.width - x, height: 100 };
                expect(pastPainted(rect, scale, where.stage, where.monitors).right).toBe(false);
            }
        }
    });

    it('rounds as Mutter does, in single precision', () => {
        // 576 x 5/3 is 959.99998 as a double and 960 as a float: the repaint starts at 960.
        expect(origin(576, S53)).toBe(960);
        expect(Math.floor(576 * S53)).toBe(959);
        // A region past the stage's near edge starts where the viewport does: rounded up.
        expect(origin(-3, 1.25)).toBe(-3);
        expect(origin(-1, 1.25)).toBe(-1);
        expect(origin(0, S53)).toBe(0);
    });
});
