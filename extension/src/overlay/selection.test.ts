// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * These are the cases from `crates/core/src/selection.rs`'s test module, with the same
 * expected values. The point is not coverage for its own sake: the same arithmetic
 * exists in Rust and in TypeScript because the overlay runs in the shell while the
 * editor and history run in the app, and a divergence between them would produce a
 * capture whose pixels disagree with the label the user saw. A divergence should fail
 * here, not ship.
 */

import { describe, expect, it } from 'vitest';

import {
    clampToMonitor,
    cycle,
    dimPieces,
    dominantMonitor,
    dragRatio,
    handleCentre,
    HANDLES,
    hitTest,
    intersect,
    isClick,
    isEmpty,
    middleHalf,
    nudge,
    rect,
    rectFromDrag,
    resizeByHandle,
    resizeByKey,
    resolveRectScale,
    scaleLabel,
    setHeight,
    setWidth,
    STEP,
    STEP_FAST,
    toPhysical,
    bufferScale,
    imageScale,
    scalesUnder,
    type Modifiers,
    type ScaledMonitor,
} from './selection.js';

const NONE: Modifiers = { aspectLock: false, fromCentre: false };
const ALT: Modifiers = { aspectLock: false, fromCentre: true };
const SHIFT: Modifiers = { aspectLock: true, fromCentre: false };

describe('drag normalisation', () => {
    it('spans anchor to pointer when dragging down-right', () => {
        expect(rectFromDrag([100, 100], [300, 250], NONE)).toEqual(rect(100, 100, 200, 150));
    });

    // Getting this wrong gives negative sizes, the most common selection bug there is.
    it('gives the same rect in any direction', () => {
        const expected = rect(100, 100, 200, 150);
        expect(rectFromDrag([300, 250], [100, 100], NONE)).toEqual(expected);
        expect(rectFromDrag([100, 250], [300, 100], NONE)).toEqual(expected);
        expect(rectFromDrag([300, 100], [100, 250], NONE)).toEqual(expected);
    });

    it('treats a zero-length drag as empty, not negative', () => {
        const r = rectFromDrag([50, 50], [50, 50], NONE);
        expect(r).toEqual(rect(50, 50, 0, 0));
        expect(isEmpty(r)).toBe(true);
    });

    it('draws from the centre with Alt', () => {
        expect(rectFromDrag([200, 200], [300, 250], ALT)).toEqual(rect(100, 150, 200, 100));
    });

    it('is symmetric under Alt regardless of direction', () => {
        expect(rectFromDrag([200, 200], [300, 250], ALT)).toEqual(
            rectFromDrag([200, 200], [100, 150], ALT),
        );
    });

    it('locks the ratio by growing the smaller axis', () => {
        const wide = rectFromDrag([0, 0], [320, 10], SHIFT, 16 / 9);
        expect([wide.width, wide.height]).toEqual([320, 180]);
        const tall = rectFromDrag([0, 0], [10, 180], SHIFT, 16 / 9);
        expect([tall.width, tall.height]).toEqual([320, 180]);
    });

    it('does nothing with Shift and no ratio', () => {
        expect(rectFromDrag([0, 0], [200, 30], SHIFT, null)).toEqual(rect(0, 0, 200, 30));
    });

    it('keeps the dominant axis under a square lock', () => {
        const r = rectFromDrag([0, 0], [200, 40], SHIFT, 1);
        expect([r.width, r.height]).toEqual([200, 200]);
    });
});

// D136: the toolbar's lock or ratio outlives a drag without Shift.
describe('the ratio a drag keeps', () => {
    const r = rect(0, 0, 400, 300);

    it('is the one the toolbar set for as long as it is set, Shift or not', () => {
        expect(dragRatio(false, true, 16 / 9, r)).toBe(16 / 9);
        expect(dragRatio(true, true, 16 / 9, r)).toBe(16 / 9);
    });

    it('is the one the selection had when Shift went down, kept while it is held', () => {
        const locked = dragRatio(true, false, null, r);
        expect(locked).toBe(4 / 3);
        expect(dragRatio(true, false, locked, rect(0, 0, 900, 100))).toBe(4 / 3);
    });

    it('goes when Shift comes up, and needs a selection with a height to start', () => {
        expect(dragRatio(false, false, 4 / 3, r)).toBeNull();
        expect(dragRatio(true, false, null, null)).toBeNull();
        expect(dragRatio(true, false, null, rect(0, 0, 40, 0))).toBeNull();
    });

    it('makes a plain drag free, or 16:9 when the toolbar says so', () => {
        const plain = rectFromDrag([0, 0], [300, 40], NONE, dragRatio(false, false, null, r));
        expect([plain.width, plain.height]).toEqual([300, 40]);
        const locked = rectFromDrag([0, 0], [320, 40], SHIFT, dragRatio(false, true, 16 / 9, r));
        expect([locked.width, locked.height]).toEqual([320, 180]);
    });
});

describe('hit testing', () => {
    const r = rect(100, 100, 200, 200);

    it('prefers handles over the interior', () => {
        expect(hitTest(r, [100, 100])).toEqual({ kind: 'handle', handle: 'top-left' });
        expect(hitTest(r, [108, 108])).toEqual({ kind: 'handle', handle: 'top-left' });
        expect(hitTest(r, [200, 200])).toEqual({ kind: 'inside' });
    });

    // Without the overhang, resizing a selection whose corner is at the screen edge is
    // impossible.
    it('lets the handle hit area overhang the selection', () => {
        expect(hitTest(r, [92, 92])).toEqual({ kind: 'handle', handle: 'top-left' });
        expect(hitTest(r, [89, 89])).toEqual({ kind: 'outside' });
    });

    it('reaches every handle at its own centre', () => {
        for (const handle of HANDLES)
            expect(hitTest(r, handleCentre(r, handle))).toEqual({ kind: 'handle', handle });
    });

    it('starts a new selection when there is none', () => {
        expect(hitTest(null, [10, 10])).toEqual({ kind: 'outside' });
    });

    it('starts a new selection on a press outside', () => {
        expect(hitTest(r, [500, 500])).toEqual({ kind: 'outside' });
    });
});

describe('handle resize', () => {
    const r = rect(100, 100, 200, 200);

    it('keeps the opposite corner fixed', () => {
        const out = resizeByHandle(r, 'top-left', [150, 120], NONE);
        expect(out).toEqual(rect(150, 120, 150, 180));
        expect([out.x + out.width, out.y + out.height]).toEqual([300, 300]);
    });

    it('moves only its own edge', () => {
        expect(resizeByHandle(r, 'right', [400, 999], NONE)).toEqual(rect(100, 100, 300, 200));
    });

    // A negative width would be mishandled by every downstream calculation.
    it('flips rather than going negative', () => {
        const out = resizeByHandle(r, 'left', [400, 100], NONE);
        expect(out.x).toBe(300);
        expect(out.width).toBe(100);
        expect(isEmpty(out)).toBe(false);
    });

    it('keeps the anchor corner under a ratio lock', () => {
        const out = resizeByHandle(r, 'bottom-right', [400, 210], SHIFT, 1);
        expect([out.x, out.y]).toEqual([100, 100]);
        expect(out.width).toBe(out.height);
    });
});

describe('clamping and monitor choice', () => {
    it('slides a selection back inside', () => {
        expect(clampToMonitor(rect(1500, 900, 200, 200), rect(0, 0, 1536, 960))).toEqual(
            rect(1336, 760, 200, 200),
        );
    });

    it('shrinks only what cannot fit', () => {
        const monitor = rect(0, 0, 1536, 960);
        expect(clampToMonitor(rect(-50, -50, 5000, 5000), monitor)).toEqual(monitor);
    });

    // Monitor offsets are in logical units, as M0's mixed-scale spike showed.
    it('respects a monitor offset', () => {
        expect(clampToMonitor(rect(1500, 100, 200, 200), rect(1536, 0, 1280, 720))).toEqual(
            rect(1536, 100, 200, 200),
        );
    });

    it('picks the monitor holding most of the selection', () => {
        const monitors = [rect(0, 0, 1536, 960), rect(1536, 0, 1280, 720)];
        expect(dominantMonitor(rect(1436, 100, 400, 200), monitors)).toBe(1);
        expect(dominantMonitor(rect(1200, 100, 400, 200), monitors)).toBe(0);
        expect(dominantMonitor(rect(10, 10, 100, 100), monitors)).toBe(0);
    });

    it('has no dominant monitor when the selection is on none', () => {
        expect(dominantMonitor(rect(9000, 9000, 10, 10), [rect(0, 0, 1536, 960)])).toBe(null);
    });
});

describe('keyboard', () => {
    const r = rect(100, 100, 200, 200);

    it('moves with arrows and resizes with Ctrl-arrows', () => {
        expect(nudge(r, 'right', STEP)).toEqual(rect(101, 100, 200, 200));
        expect(nudge(r, 'up', STEP_FAST)).toEqual(rect(100, 90, 200, 200));
        expect(resizeByKey(r, 'right', STEP)).toEqual(rect(100, 100, 201, 200));
        expect(resizeByKey(r, 'down', STEP_FAST)).toEqual(rect(100, 100, 200, 210));
    });

    // 1x1 is legal per spec/03 §5.1; 0x0 is not.
    it('stops shrinking at one pixel', () => {
        const out = resizeByKey(rect(100, 100, 1, 1), 'left', STEP_FAST);
        expect([out.width, out.height]).toEqual([1, 1]);
    });
});

describe('exact sizes', () => {
    it('anchors a typed size at the top-left', () => {
        const r = rect(100, 100, 200, 200);
        expect(setWidth(r, 640)).toEqual(rect(100, 100, 640, 200));
        expect(setHeight(r, 480)).toEqual(rect(100, 100, 200, 480));
    });

    it('drives the other field from a locked ratio', () => {
        const r = rect(0, 0, 100, 100);
        const byWidth = setWidth(r, 1920, 16 / 9);
        expect([byWidth.width, byWidth.height]).toEqual([1920, 1080]);
        const byHeight = setHeight(r, 1080, 16 / 9);
        expect([byHeight.width, byHeight.height]).toEqual([1920, 1080]);
    });

    it('turns a typed zero into one', () => {
        const r = rect(0, 0, 100, 100);
        expect(setWidth(r, 0).width).toBe(1);
        expect(setHeight(r, -5).height).toBe(1);
    });
});

describe('click versus drag', () => {
    it('treats a short movement as a click', () => {
        expect(isClick([100, 100], [102, 101])).toBe(true);
        expect(isClick([100, 100], [104, 100])).toBe(false);
        expect(isClick([100, 100], [100, 105])).toBe(false);
    });
});

describe('logical to physical', () => {
    // The target machine runs at 1.25, so fractional scale is the common case.
    it('matches the Rust conversion at the target scale', () => {
        expect(toPhysical(rect(0, 0, 1536, 960), 1.25)).toEqual([1920, 1200]);
        expect(toPhysical(rect(0, 0, 512, 384), 1.25)).toEqual([640, 480]);
        expect(toPhysical(rect(0, 0, 512, 384), 2)).toEqual([1024, 768]);
        expect(toPhysical(rect(0, 0, 1280, 800), 1.5)).toEqual([1920, 1200]);

        // The exact float32 approximations Mutter reports, and the sizes `screenshot_area`
        // was measured to produce for them. Rounding outward described a 1920 px panel as
        // 1921 px and predicted 334 where the compositor produced 333. D23.
        expect(toPhysical(rect(0, 0, 1440, 900), 1.3333333730697632)).toEqual([1920, 1200]);
        expect(toPhysical(rect(0, 0, 1152, 720), 1.6666666269302368)).toEqual([1920, 1200]);
        expect(toPhysical(rect(0, 0, 200, 150), 1.3333333730697632)).toEqual([267, 200]);
        expect(toPhysical(rect(0, 0, 200, 150), 1.6666666269302368)).toEqual([333, 250]);
        expect(toPhysical(rect(0, 0, 201, 149), 1.25)).toEqual([251, 186]);
    });

    // Rounding outward: a rect between pixels must grow, or a strip is silently lost.
    it('rounds to nearest, matching the compositor rather than covering the edge', () => {
        // This asserted [127, 127] until 2026-09-08, on the reasoning that a selection
        // should never lose an edge pixel. The reasoning was sound and the rule was still
        // wrong: `screenshot_area` rounds, so covering the edge here only made the twin
        // claim a size the PNG did not have. Measured, then changed. D23.
        expect(toPhysical(rect(0, 0, 101, 101), 1.25)).toEqual([126, 126]);
    });
});

describe('intersect', () => {
    // The two-monitor rig this was found on: 2560x1440 at the origin, 1920x1200 beside it.
    const LEFT = rect(0, 0, 2560, 1440);
    const RIGHT = rect(2560, 0, 1920, 1200);

    it('is the selection itself when it sits inside one monitor', () => {
        const sel = rect(2700, 200, 500, 400);
        expect(intersect(sel, RIGHT)).toEqual(sel);
    });

    it('is null for the monitor the selection never touches', () => {
        // This is the bug: tiling the dim around this rect gave the left monitor a
        // 2700 px wide dim piece on a 2560 px screen, overhanging its neighbour.
        expect(intersect(rect(2700, 200, 500, 400), LEFT)).toBeNull();
    });

    it('splits a selection that spans two monitors at each boundary', () => {
        const sel = rect(2400, 100, 400, 300);
        expect(intersect(sel, LEFT)).toEqual(rect(2400, 100, 160, 300));
        expect(intersect(sel, RIGHT)).toEqual(rect(2560, 100, 240, 300));
    });

    it('treats a shared edge as no overlap rather than a zero-width rect', () => {
        expect(intersect(rect(2560, 0, 100, 100), LEFT)).toBeNull();
    });

    it('clips a selection that overhangs the far edge', () => {
        expect(intersect(rect(4400, 0, 400, 100), RIGHT)).toEqual(rect(4400, 0, 80, 100));
    });
});

describe('dimPieces', () => {
    // The two-monitor rig the reported bug was found on.
    const LEFT = rect(0, 0, 2560, 1440);
    const RIGHT = rect(2560, 0, 1920, 1200);

    /** Area of the overlap of two rects, 0 when they do not overlap. */
    const overlap = (a: Rect, b: Rect): number => {
        const w = Math.min(a.x + a.width, b.x + b.width) - Math.max(a.x, b.x);
        const h = Math.min(a.y + a.height, b.y + b.height) - Math.max(a.y, b.y);
        return w <= 0 || h <= 0 ? 0 : w * h;
    };

    /**
     * The whole contract in one assertion, checked by area rather than by rasterising:
     * inside the monitor, mutually disjoint, and covering everything the selection does
     * not. Any one of those failing is a visible hole or a double-dimmed strip.
     */
    const expectExactCover = (monitor: Rect, selection: Rect | null) => {
        const pieces = dimPieces(monitor, selection);
        const local = rect(0, 0, monitor.width, monitor.height);

        for (const piece of pieces) {
            expect(piece.width).toBeGreaterThanOrEqual(0);
            expect(piece.height).toBeGreaterThanOrEqual(0);
            if (piece.width === 0 || piece.height === 0) continue;
            // Inside the monitor: a piece reaching past the edge dims a neighbour.
            expect(piece.x).toBeGreaterThanOrEqual(0);
            expect(piece.y).toBeGreaterThanOrEqual(0);
            expect(piece.x + piece.width).toBeLessThanOrEqual(monitor.width);
            expect(piece.y + piece.height).toBeLessThanOrEqual(monitor.height);
            expect(overlap(piece, local)).toBe(piece.width * piece.height);
        }

        for (let i = 0; i < pieces.length; i++)
            for (let j = i + 1; j < pieces.length; j++)
                expect(overlap(pieces[i]!, pieces[j]!)).toBe(0);

        const part = selection === null ? null : intersect(selection, monitor);
        const hole = part === null ? 0 : part.width * part.height;
        const covered = pieces.reduce((sum, p) => sum + p.width * p.height, 0);
        expect(covered).toBe(monitor.width * monitor.height - hole);
    };

    it('covers the whole monitor when there is no selection', () => {
        expectExactCover(LEFT, null);
        expect(dimPieces(LEFT, null)[0]).toEqual(rect(0, 0, 2560, 1440));
    });

    it('covers the whole monitor the selection does not touch', () => {
        // The reported bug: this monitor used to get a 2700 px wide piece for a 2560 px
        // screen, overhanging its neighbour and double-dimming a strip of it.
        expectExactCover(LEFT, rect(2700, 200, 500, 400));
        expect(dimPieces(LEFT, rect(2700, 200, 500, 400))[0]).toEqual(rect(0, 0, 2560, 1440));
    });

    it('tiles around a selection on its own monitor', () => {
        const sel = rect(2700, 200, 500, 400);
        expectExactCover(RIGHT, sel);
        // Monitor-local, and these are the numbers measured in the nested shell.
        expect(dimPieces(RIGHT, sel)).toEqual([
            rect(0, 0, 1920, 200),
            rect(0, 600, 1920, 600),
            rect(0, 200, 140, 400),
            rect(640, 200, 1280, 400),
        ]);
    });

    it('tiles around each monitor\u2019s own share of a spanning selection', () => {
        const sel = rect(2400, 100, 400, 300);
        expectExactCover(LEFT, sel);
        expectExactCover(RIGHT, sel);
    });

    it('holds for a selection flush against each edge', () => {
        for (const sel of [
            rect(2560, 0, 400, 300),          // top-left corner of RIGHT
            rect(4080, 900, 400, 300),        // bottom-right corner of RIGHT
            rect(2560, 0, 1920, 1200),        // the whole monitor
            rect(2000, 0, 3000, 1400),        // larger than either monitor
        ]) {
            expectExactCover(RIGHT, sel);
            expectExactCover(LEFT, sel);
        }
    });

    it('holds for a one-pixel selection', () => {
        expectExactCover(RIGHT, rect(3000, 500, 1, 1));
    });
});

describe('scaleLabel', () => {
    it('reads like the Displays panel, not like a float', () => {
        // The value Mutter reports for 4/3 is a float32 approximation, and printing it
        // raw made the user-facing refusal message unreadable.
        expect(scaleLabel(1.3333333730697632)).toBe('133%');
        expect(scaleLabel(1.6666666269302368)).toBe('167%');
        expect(scaleLabel(1.25)).toBe('125%');
        expect(scaleLabel(1)).toBe('100%');
        expect(scaleLabel(2)).toBe('200%');
    });
});

describe('resolveRectScale', () => {
    // The real rig this was found on: a 1.25 laptop panel below a 1.0 external screen,
    // with the panel's origin nowhere near zero.
    const RIG: ScaledMonitor[] = [
        { rect: rect(0, 0, 2560, 1440), scale: 1 },        // DP-6
        { rect: rect(443, 1440, 1536, 960), scale: 1.25 }, // eDP-1
    ];

    it('takes the scale of the monitor holding the rect, not the first one', () => {
        expect(resolveRectScale(rect(200, 200, 400, 300), RIG)).toEqual({
            ok: true, scale: 1, monitor: 0,
        });
        expect(resolveRectScale(rect(600, 1600, 400, 300), RIG)).toEqual({
            ok: true, scale: 1.25, monitor: 1,
        });
    });

    /**
     * `spec/01` §1. Measured: a 600x400 logical rect across this boundary produced a
     * 750x500 PNG while the twin said scale 1, because `screenshot_area` picks its own
     * scale across the span.
     */
    it('refuses a rect spanning two differently-scaled monitors', () => {
        const result = resolveRectScale(rect(300, 1300, 600, 400), RIG);
        expect(result.ok).toBe(false);
        if (!result.ok) expect(result.reason).toContain('different scales');
    });

    /** No ambiguity at equal scale, so a shot across two matched displays is allowed. */
    it('allows a rect spanning two monitors of the same scale', () => {
        const matched: ScaledMonitor[] = [
            { rect: rect(0, 0, 1920, 1080), scale: 1 },
            { rect: rect(1920, 0, 1920, 1080), scale: 1 },
        ];
        // 120 px of the 300 fall left of the seam and 180 right of it, so the dominant
        // monitor is the right-hand one. The scale is what matters here; the index is
        // only there to name a display in the twin.
        expect(resolveRectScale(rect(1800, 100, 300, 200), matched)).toEqual({
            ok: true, scale: 1, monitor: 1,
        });
    });

    it('names the monitor holding most of the rect when scales agree', () => {
        const matched: ScaledMonitor[] = [
            { rect: rect(0, 0, 1920, 1080), scale: 2 },
            { rect: rect(1920, 0, 1920, 1080), scale: 2 },
        ];
        // 250 px of 300 fall on the right-hand monitor.
        expect(resolveRectScale(rect(1870, 100, 300, 200), matched).ok).toBe(true);
        const r = resolveRectScale(rect(1870, 100, 300, 200), matched);
        if (r.ok) expect(r.monitor).toBe(1);
    });

    /**
     * A `previous-area` rect can outlive the layout that produced it: unplug the monitor
     * it was on and it points at nothing. Refusing beats capturing black pixels.
     */
    it('refuses a rect that is off every monitor', () => {
        const result = resolveRectScale(rect(9000, 9000, 100, 100), RIG);
        expect(result.ok).toBe(false);
        if (!result.ok) expect(result.reason).toContain('not on any monitor');
    });

    it('treats a rect touching only a corner as being on that monitor', () => {
        const result = resolveRectScale(rect(2500, 100, 200, 200), RIG);
        expect(result).toEqual({ ok: true, scale: 1, monitor: 0 });
    });
});

describe('scalesUnder', () => {
    const RIG: ScaledMonitor[] = [
        { rect: rect(0, 0, 2560, 1440), scale: 1 },        // DP-6
        { rect: rect(443, 1440, 1536, 960), scale: 1.25 }, // eDP-1
    ];

    it('is the one scale of the one monitor a window is on', () => {
        expect(scalesUnder(rect(100, 100, 800, 600), RIG)).toEqual([1]);
        expect(scalesUnder(rect(600, 1600, 800, 600), RIG)).toEqual([1.25]);
    });

    it('is both, largest first, for a window across the two', () => {
        expect(scalesUnder(rect(600, 1000, 800, 600), RIG)).toEqual([1.25, 1]);
    });

    it('is none for a window on no monitor', () => {
        expect(scalesUnder(rect(3000, 3000, 100, 100), RIG)).toEqual([]);
    });
});

describe('imageScale', () => {
    const frame = { width: 508, height: 349 };

    it('is the monitor\'s scale for a window drawn at it', () => {
        expect(imageScale(frame, { width: 635, height: 436 }, [1.25])).toBe(1.25);
        expect(imageScale(frame, { width: 677, height: 465 }, [1.3333333730697632]))
            .toBe(1.3333333730697632);
        expect(imageScale(frame, { width: 1016, height: 698 }, [2])).toBe(2);
    });

    /** D138, measured: the nested shell's GTK draws a 2x buffer for a 125 % monitor. */
    it('is the whole number above it for a window that cannot draw at a fraction', () => {
        expect(imageScale(frame, { width: 1016, height: 698 }, [1.25])).toBe(2);
    });

    it('is the larger scale for a window across two monitors', () => {
        expect(imageScale(frame, { width: 1016, height: 698 }, [2, 1])).toBe(2);
    });

    /** `captureWindowToFile`'s measurement: a 25 px shadow round a 360x240 window. */
    it('takes an even margin round the window as a margin, not as a scale', () => {
        expect(imageScale({ width: 360, height: 240 }, { width: 410, height: 290 }, [1])).toBe(1);
        expect(imageScale({ width: 360, height: 240 }, { width: 820, height: 580 }, [2])).toBe(2);
    });

    it('is 1 for a buffer at 1 on a finer monitor', () => {
        expect(imageScale(frame, { width: 508, height: 349 }, [2])).toBe(1);
    });

    it('is null for a file that is no scale of the window', () => {
        expect(imageScale(frame, { width: 777, height: 123 }, [1, 2])).toBeNull();
    });

    /**
     * D138, measured in the matrix: Mutter reports the frame, 480x320, and GTK 4's buffer
     * round it has its own shadow, 14 at each side, 12 above and 17 below. Half a logical
     * pixel uneven, which a physical pixel of slack took at 1x and 2x and not at 3x.
     */
    it('takes a client shadow a pixel deeper than it is wide as a margin', () => {
        const drawn = { width: 480, height: 320 };
        expect(imageScale(drawn, { width: 508, height: 349 }, [1])).toBe(1);
        expect(imageScale(drawn, { width: 1016, height: 698 }, [1.25])).toBe(2);
        expect(imageScale(drawn, { width: 1524, height: 1047 }, [2.5])).toBe(3);
        expect(imageScale(drawn, { width: 1524, height: 1047 }, [2.6666667461395264])).toBe(3);
    });
});

describe('bufferScale', () => {
    const buffer = { width: 508, height: 349 };

    it('is the scale at which the file is the buffer exactly', () => {
        expect(bufferScale(buffer, { width: 508, height: 349 }, [1])).toBe(1);
        expect(bufferScale(buffer, { width: 635, height: 436 }, [1.25])).toBe(1.25);
        expect(bufferScale(buffer, { width: 1016, height: 698 }, [1.25])).toBe(2);
        expect(bufferScale(buffer, { width: 1016, height: 698 }, [2, 1])).toBe(2);
        expect(bufferScale(buffer, { width: 508, height: 349 }, [2])).toBe(1);
    });

    /** D138: the nested shell's GTK draws a 3x buffer for a 250 % and an 8/3 monitor. */
    it('is the whole number above a scale over 2 for a window that cannot draw at it', () => {
        expect(bufferScale(buffer, { width: 1524, height: 1047 }, [2.5])).toBe(3);
        expect(bufferScale(buffer, { width: 1524, height: 1047 }, [2.6666667461395264])).toBe(3);
    });

    it('is null for an image with a margin round the buffer', () => {
        expect(bufferScale({ width: 360, height: 240 }, { width: 410, height: 290 }, [1])).toBeNull();
    });
});

describe('the keyboard with no pointer (D137)', () => {
    const MODES = ['area', 'fullscreen', 'window'] as const;

    it('steps forward and back through a list', () => {
        expect(cycle(MODES, 'area', 1)).toBe('fullscreen');
        expect(cycle(MODES, 'window', -1)).toBe('fullscreen');
    });

    it('wraps at both ends, as Tab does', () => {
        expect(cycle(MODES, 'window', 1)).toBe('area');
        expect(cycle(MODES, 'area', -1)).toBe('window');
    });

    it('starts at the first item going forward and the last going back', () => {
        expect(cycle(MODES, null, 1)).toBe('area');
        expect(cycle(MODES, null, -1)).toBe('window');
    });

    it('starts over when the current item has left the list', () => {
        // A window closed while the picker was open is no longer a candidate.
        expect(cycle(['a', 'b'], 'gone', 1)).toBe('a');
    });

    it('has nothing to offer from an empty list', () => {
        expect(cycle([], null, 1)).toBeNull();
    });

    it('starts a selection in the middle half of the monitor', () => {
        expect(middleHalf(rect(0, 0, 1920, 1200))).toEqual(rect(480, 300, 960, 600));
    });

    it('keeps the middle half on a monitor away from the origin', () => {
        expect(middleHalf(rect(1920, -300, 1280, 1024))).toEqual(rect(2240, -44, 640, 512));
    });
});
