// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The outline around a region being captured, checked against the captures themselves.
 *
 * The model of a capture here is the one D115 measured on a nested shell: a stroke fills
 * the pixels whose centres it covers; each capture repaints the stage through a viewport
 * whose origin and whose length are truncated to whole pixels, multiplying in single
 * precision; `GrabFrame` then takes the region's origin down and its extent to nearest, and
 * the stream is cropped the way `GifPlan::new` crops it. It is written out again here,
 * pixel by pixel, rather than borrowed from `outline.ts`, and the owner's own two cases
 * show it gets the old outline's leaks exactly right.
 */

import { describe, expect, it } from 'vitest';

import { outlineRect, type ScaledMonitor } from './outline.js';
import type { Rect, Size } from './place.js';

const STROKE = 3;
const f32 = Math.fround;
const roundAway = (v: number): number => Math.sign(v) * Math.round(Math.abs(v));

type Axis = 'x' | 'y';

/** One capture along one axis: pixels per logical pixel, and the pixels it takes. */
interface Capture {
    name: string;
    per: number;
    /** The pixel the capture's own buffer calls 0. */
    origin: number;
    first: number;
    end: number;
}

function along(rect: Rect, axis: Axis): [number, number] {
    return axis === 'x' ? [rect.x, rect.width] : [rect.y, rect.height];
}

function under(rect: Rect, monitors: readonly ScaledMonitor[]): ScaledMonitor[] {
    return monitors.filter(
        m =>
            m.rect.x < rect.x + rect.width &&
            rect.x < m.rect.x + m.rect.width &&
            m.rect.y < rect.y + rect.height &&
            rect.y < m.rect.y + m.rect.height,
    );
}

/** Pixels per logical pixel in a repaint at `scale`: the stage's length, truncated. */
function perLogical(stage: Size, axis: Axis, scale: number): number {
    const length = axis === 'x' ? stage.width : stage.height;
    return Math.trunc(f32(length * scale)) / length;
}

/** Every capture that can take `rect` along `axis`: `GrabFrame`, then each monitor's stream. */
function captures(
    rect: Rect,
    monitors: readonly ScaledMonitor[],
    stage: Size,
    axis: Axis,
): Capture[] {
    const [start, length] = along(rect, axis);
    const on = under(rect, monitors);
    const shot = Math.max(1, ...on.map(m => m.scale));
    const shotAt = Math.floor(f32(start * shot));
    const out: Capture[] = [{
        name: 'GrabFrame',
        per: perLogical(stage, axis, shot),
        origin: shotAt,
        first: shotAt,
        end: shotAt + Math.round(f32(length * shot)),
    }];
    for (const m of on) {
        const [mo] = along(m.rect, axis);
        const origin = Math.floor(f32(mo * m.scale));
        const first = origin + roundAway((start - mo) * m.scale);
        out.push({
            name: `stream of the monitor at ${m.rect.x},${m.rect.y}`,
            per: perLogical(stage, axis, m.scale),
            origin,
            first,
            end: first + roundAway(length * m.scale),
        });
    }
    return out;
}

/**
 * The pixels the outline's stroke fills along `axis`, as the capture `c` sees them: the
 * near stroke and the far one, each `[first, last]` inclusive.
 */
function stroked(outline: Rect, axis: Axis, c: Capture): [number, number][] {
    const [start, length] = along(outline, axis);
    const fill = (a: number, b: number): [number, number] => [
        Math.ceil(a * c.per - 0.5),
        Math.ceil(b * c.per - 0.5) - 1,
    ];
    return [fill(start, start + STROKE), fill(start + length - STROKE, start + length)];
}

/** Every capture pixel the stroke fills, as `capture: pixel` strings; none is the goal. */
function leaks(
    outline: Rect,
    rect: Rect,
    monitors: readonly ScaledMonitor[],
    stage: Size,
): string[] {
    const found: string[] = [];
    for (const axis of ['x', 'y'] as const) {
        for (const c of captures(rect, monitors, stage, axis)) {
            for (const [a, b] of stroked(outline, axis, c)) {
                for (let k = Math.max(a, c.first); k <= Math.min(b, c.end - 1); k++)
                    found.push(`${c.name} ${axis === 'x' ? 'column' : 'row'} ${k - c.origin}`);
            }
        }
    }
    return found;
}

/** The capture pixels `outlineRect`'s own outline fills: none, if it is right. */
function leaksNow(rect: Rect, monitors: readonly ScaledMonitor[], stage: Size): string[] {
    return leaks(outlineRect(rect, STROKE, monitors, stage), rect, monitors, stage);
}

/** The pre-D115 outline: the rectangle grown by the stroke in logical pixels. */
function logicalOutset(rect: Rect): Rect {
    return {
        x: rect.x - STROKE,
        y: rect.y - STROKE,
        width: rect.width + 2 * STROKE,
        height: rect.height + 2 * STROKE,
    };
}

/** The capture of a monitor's stream, of all those `captures` lists. */
function stream(all: Capture[]): Capture {
    const found = all.find(c => c.name.startsWith('stream'));
    if (found === undefined) throw new Error('no stream takes this region');
    return found;
}

/** A 1920x1200 panel's logical size at `scale`, as Mutter lays it out. */
function panel(x: number, y: number, scale: number): ScaledMonitor {
    const size = { width: Math.round(1920 / scale), height: Math.round(1200 / scale) };
    return { rect: { x, y, ...size }, scale };
}

const S43 = 1.3333333730697632;
const S53 = 1.6666666269302368;

describe('outlineRect', () => {
    it('is the rectangle grown by the stroke wherever logical and physical pixels line up', () => {
        const rect = { x: 443, y: 1467, width: 641, height: 333 };
        for (const scale of [1, 2, 3]) {
            const screen = { rect: { x: 0, y: 0, width: 3000, height: 3000 }, scale };
            expect(outlineRect(rect, STROKE, [screen], { width: 3000, height: 3000 })).toEqual(
                logicalOutset(rect),
            );
        }
        // At 1.25 a logical pixel is a whole number of physical ones every fourth pixel.
        const quarter = { x: 404, y: 432, width: 1080, height: 328 };
        const screen = [panel(0, 0, 1.25)];
        expect(outlineRect(quarter, STROKE, screen, { width: 1536, height: 960 })).toEqual(
            logicalOutset(quarter),
        );
    });

    it('keeps the blue outline out of the owner\'s scrolling frames (1.25, 2026-09-24)', () => {
        // The laptop panel at 1.25 under a 2560x1440 at 1, and the selection its kept frames
        // were taken of: 1081x328 logical, 1351x410 in every frame.
        const monitors = [
            { rect: { x: 0, y: 0, width: 2560, height: 1440 }, scale: 1 },
            { rect: { x: 443, y: 1440, width: 1536, height: 960 }, scale: 1.25 },
        ];
        const stage = { width: 2560, height: 2400 };
        const rect = { x: 847, y: 1870, width: 1081, height: 328 };

        // `GifPlan::new`'s crop of the panel's stream, as the app logged it.
        const x = stream(captures(rect, monitors, stage, 'x'));
        const y = stream(captures(rect, monitors, stage, 'y'));
        expect([x.first - x.origin, y.first - y.origin, x.end - x.first, y.end - y.first]).toEqual(
            [505, 538, 1351, 410],
        );

        // What the kept frames showed, the stream's first column and its last row, and
        // GrabFrame's first column, which the nested shell showed for the same selection.
        // Nothing else.
        expect(leaks(logicalOutset(rect), rect, monitors, stage).sort()).toEqual([
            'GrabFrame column 0',
            'stream of the monitor at 443,1440 column 505',
            'stream of the monitor at 443,1440 row 947',
        ]);

        const outline = outlineRect(rect, STROKE, monitors, stage);
        expect(leaks(outline, rect, monitors, stage)).toEqual([]);
        // And still flush: the stroke fills the column and the row just outside the crop.
        const [nearX] = stroked(outline, 'x', x);
        const [, farY] = stroked(outline, 'y', y);
        expect(nearX![1] - x.origin).toBe(504);
        expect(farY![0] - y.origin).toBe(948);
    });

    it('keeps the red frame out of the owner\'s GIF (1.25, 2026-09-24)', () => {
        // 1090x669 logical on the same panel, recorded as 1363x836: red all down column 0.
        const monitors = [
            { rect: { x: 0, y: 0, width: 2560, height: 1440 }, scale: 1 },
            { rect: { x: 443, y: 1440, width: 1536, height: 960 }, scale: 1.25 },
        ];
        const stage = { width: 2560, height: 2400 };
        const rect = { x: 783, y: 1555, width: 1090, height: 669 };
        const streamOnly = (found: string[]) => found.filter(l => l.startsWith('stream'));

        expect(streamOnly(leaks(logicalOutset(rect), rect, monitors, stage))).toEqual([
            'stream of the monitor at 443,1440 column 425',
        ]);
        expect(leaksNow(rect, monitors, stage)).toEqual([]);
    });

    it('stays out of every capture at every scale, and meets the outermost', () => {
        // Mutter's ladder in its own float spellings, and two it makes when a panel's size
        // does not divide: 1920 over 1096 and over 1488.
        const scales = [
            1, 1.25, S43, 1.5, S53, 1.75, 2, 2.25, 2.5, 2.6666667461395264, 3, 3.5, 4,
            f32(1920 / 1096), f32(1920 / 1488),
        ];
        // MINSTD: small enough a multiplier that every product is exact in a double.
        let seed = 113;
        const next = (n: number): number => {
            seed = (seed * 48271) % 2147483647;
            return seed % n;
        };
        const failures: string[] = [];
        let checked = 0;
        for (const scale of scales) {
            const alone = panel(0, 0, scale);
            const { width: w, height: h } = alone.rect;
            const layouts: { monitors: ScaledMonitor[]; stage: Size; on: ScaledMonitor }[] = [
                { monitors: [alone], stage: { width: w, height: h }, on: alone },
            ];
            const below = panel(443, 1440, scale);
            layouts.push({
                monitors: [{ rect: { x: 0, y: 0, width: 2560, height: 1440 }, scale: 1 }, below],
                stage: { width: Math.max(2560, 443 + w), height: 1440 + h },
                on: below,
            });
            const right = panel(2560, 0, scale);
            layouts.push({
                monitors: [{ rect: { x: 0, y: 0, width: 2560, height: 1440 }, scale: 1 }, right],
                stage: { width: 2560 + w, height: Math.max(1440, h) },
                on: right,
            });
            const beside = panel(1536, 0, scale);
            layouts.push({
                monitors: [panel(0, 0, 1.25), beside],
                stage: { width: 1536 + w, height: Math.max(960, h) },
                on: beside,
            });
            for (const { monitors, stage, on } of layouts) {
                for (let i = 0; i < 150; i++) {
                    const width = 1 + next(Math.floor(on.rect.width / 2));
                    const height = 1 + next(Math.floor(on.rect.height / 2));
                    const rect = {
                        x: on.rect.x + next(on.rect.width - width + 1),
                        y: on.rect.y + next(on.rect.height - height + 1),
                        width,
                        height,
                    };
                    const outline = outlineRect(rect, STROKE, monitors, stage);
                    const where = (): string => `${JSON.stringify(rect)} at ${scale}`;
                    const found = leaks(outline, rect, monitors, stage);
                    if (found.length > 0) failures.push(`${where()} fills ${found[0]}`);
                    // Meets the outermost capture: nothing sits between the stroke and it.
                    for (const axis of ['x', 'y'] as const) {
                        const all = captures(rect, [on], stage, axis);
                        const [near, far] = stroked(outline, axis, all[0]!);
                        if (near![1] + 1 !== Math.min(...all.map(c => c.first)))
                            failures.push(`${where()} leaves a gap before it along ${axis}`);
                        if (far![0] !== Math.max(...all.map(c => c.end)))
                            failures.push(`${where()} leaves a gap after it along ${axis}`);
                    }
                    checked++;
                }
            }
        }
        expect(failures.slice(0, 5)).toEqual([]);
        expect(checked).toBe(scales.length * 4 * 150);
    });

    it('multiplies in single precision, as Mutter does', () => {
        // A 5/3 panel under a 1440-high screen: 1440 x 5/3 is 2399.99994 as a double and
        // 2400 as a float, so the stream starts a row lower than a double says, and an
        // outline worked out in doubles filled the crop's last row.
        const monitors = [
            { rect: { x: 0, y: 0, width: 2560, height: 1440 }, scale: 1 },
            panel(443, 1440, S53),
        ];
        const stage = { width: 2560, height: 1440 + 720 };
        const rect = { x: 500, y: 1540, width: 150, height: 100 };
        expect(Math.floor(1440 * S53)).toBe(2399);
        const y = stream(captures(rect, monitors, stage, 'y'));
        expect(y.origin).toBe(2400);
        expect([y.first - y.origin, y.end - y.origin]).toEqual([167, 334]);
        // The stroke starts on the row after the crop's last, as the float has it.
        const [, far] = stroked(outlineRect(rect, STROKE, monitors, stage), 'y', y);
        expect(far![0] - y.origin).toBe(334);
        expect(leaksNow(rect, monitors, stage)).toEqual([]);
    });

    it('allows for the stage length the viewport truncates', () => {
        // A 5/3 panel right of a 2560 screen: the stage is 3712 wide, 6186.67 pixels at
        // 5/3, and the repaint draws it 6186 -- two thirds of a pixel short at the far end.
        const monitors = [
            { rect: { x: 0, y: 0, width: 2560, height: 1440 }, scale: 1 },
            panel(2560, 0, S53),
        ];
        const stage = { width: 3712, height: 1440 };
        expect(Math.trunc(f32(stage.width * S53))).toBe(6186);
        const rect = { x: 2634, y: 100, width: 151, height: 100 };
        const outline = outlineRect(rect, STROKE, monitors, stage);
        expect(leaks(outline, rect, monitors, stage)).toEqual([]);
        // The same pixels turned back into logical ones as though the repaint were exactly
        // 5/3 across: the stroke then fills the last column GrabFrame and the stream take.
        const [shot, own] = captures(rect, monitors, stage, 'x');
        const start = Math.min(shot!.first, own!.first) / S53;
        const end = Math.max(shot!.end, own!.end) / S53;
        const naive = { ...outline, x: start - STROKE, width: end - start + 2 * STROKE };
        expect(leaks(naive, rect, monitors, stage)).not.toEqual([]);
    });

    it('keeps clear of both monitors a region spans', () => {
        // Two 1.25 panels side by side and a region across the seam: GrabFrame takes it
        // whole, and each monitor's stream would take its own part.
        const monitors = [panel(0, 0, 1.25), panel(1536, 0, 1.25)];
        const stage = { width: 3072, height: 960 };
        const rect = { x: 1403, y: 301, width: 263, height: 197 };
        expect(leaks(logicalOutset(rect), rect, monitors, stage)).not.toEqual([]);
        expect(leaksNow(rect, monitors, stage)).toEqual([]);
    });

    it('keeps a region no monitor shows to its own edges', () => {
        const rect = { x: 5000, y: 5000, width: 10, height: 10 };
        const screen = [panel(0, 0, 1.25)];
        expect(outlineRect(rect, STROKE, screen, { width: 1536, height: 960 })).toEqual(
            logicalOutset(rect),
        );
    });
});
