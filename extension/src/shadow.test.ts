// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The pin shadow's pixels (D132): that the eight pieces, made once, are the shadow of any
 * pin they are put around, with nothing under the pin; and that the shadow is shaped like
 * one, falling away from the pin and heavier below it.
 */

import { describe, expect, it } from 'vitest';

import type { Rect } from './place.js';
import {
    PIN_SHADOW,
    type ShadowPiece,
    erfc,
    pieceRects,
    piecePixels,
    roundedRectDistance,
    shadowAlpha,
    shadowAt,
    shadowMargins,
    wholeShadow,
} from './shadow.js';

/** A piece's alpha at a device pixel, 0 to 255. */
function alphaAt(piece: ShadowPiece, column: number, row: number): number {
    return piece.pixels[(row * piece.width + column) * 4 + 3] ?? -1;
}

/** Every piece of a pin's shadow, the eight of them placed for this pin. */
function placed(width: number, height: number, radius: number, scale: number): ShadowPiece[] {
    const rects = pieceRects({ width, height, radius }, PIN_SHADOW);
    if (rects === null) throw new Error('expected a pin with straight edges');
    return piecePixels(radius, scale, PIN_SHADOW).map(piece => ({
        ...piece,
        rect: rects[piece.name as keyof typeof rects],
    }));
}

function overlaps(a: Rect, b: Rect): boolean {
    return a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
}

describe('the shadow', () => {
    it('reaches three deviations past the pin, and further below it than above', () => {
        const m = shadowMargins(PIN_SHADOW);
        expect(m.left).toBe(30);
        expect(m.right).toBe(30);
        expect(m.top).toBe(26);
        expect(m.bottom).toBe(34);
    });

    it('erfc is the complementary error function', () => {
        expect(erfc(0)).toBeCloseTo(1, 6);
        expect(erfc(1)).toBeCloseTo(0.157299207, 6);
        expect(erfc(-1)).toBeCloseTo(1.842700793, 6);
        expect(erfc(3)).toBeCloseTo(0.0000220905, 6);
    });

    it('measures a rounded rect the way a rounded rect is', () => {
        const rect = { x: 0, y: 0, width: 100, height: 50 };
        expect(roundedRectDistance(50, 25, rect, 8)).toBeCloseTo(-25, 6);
        expect(roundedRectDistance(-10, 25, rect, 8)).toBeCloseTo(10, 6);
        // Off a corner, it is the distance to the arc: from its centre, less the radius.
        expect(roundedRectDistance(-3, -3, rect, 8)).toBeCloseTo(Math.hypot(11, 11) - 8, 6);
        expect(roundedRectDistance(-3, -3, rect, 0)).toBeCloseTo(Math.hypot(3, 3), 6);
    });

    it('falls away from the pin, and is heavier under it than over it', () => {
        const [w, h] = [400, 300];
        let previous = 1;
        for (let d = 1; d <= 30; d++) {
            const here = shadowAt(-d, h / 2, w, h, 8, PIN_SHADOW);
            expect(here).toBeLessThan(previous);
            previous = here;
        }
        expect(shadowAt(w / 2, h + 3, w, h, 8, PIN_SHADOW)).toBeGreaterThan(
            shadowAt(w / 2, -3, w, h, 8, PIN_SHADOW),
        );
    });

    it('stops where the pieces do', () => {
        // Just past the margins, under half a step of an 8-bit alpha.
        const pin = { width: 640, height: 480, radius: 8 };
        const m = shadowMargins(PIN_SHADOW);
        expect(shadowAlpha(-m.left - 0.5, 240, pin, 1, PIN_SHADOW)).toBe(0);
        expect(shadowAlpha(320, -m.top - 0.5, pin, 1, PIN_SHADOW)).toBe(0);
        expect(shadowAlpha(320, 480 + m.bottom + 0.5, pin, 1, PIN_SHADOW)).toBe(0);
    });
});

describe('the pieces', () => {
    it('tile the shadow around the pin without overlapping it or each other', () => {
        const [w, h] = [640, 480];
        const rects = pieceRects({ width: w, height: h, radius: 8 }, PIN_SHADOW);
        expect(rects).not.toBeNull();
        const all = Object.values(rects ?? {});
        expect(all).toHaveLength(8);
        for (let i = 0; i < all.length; i++)
            for (let j = i + 1; j < all.length; j++)
                expect(overlaps(all[i] as Rect, all[j] as Rect)).toBe(false);
        // The strips keep off the pin: the top one ends at its edge, the left one too.
        expect((rects?.top.y ?? 0) + (rects?.top.height ?? 0)).toBe(0);
        expect((rects?.left.x ?? 0) + (rects?.left.width ?? 0)).toBe(0);
    });

    it("are this pin's shadow, though made once for every pin", () => {
        // The pieces come from the smallest pin with straight edges. Placed around a real
        // one, every pixel is that pin's shadow where the pixel lands, at every scale the
        // ladder has, and a strip is right at each end and in the middle of its run.
        for (const scale of [1, 1.25, 4 / 3, 1.5, 5 / 3, 2]) {
            for (const radius of [8, 0]) {
                const pin = { width: 640, height: 480, radius };
                let worst = 0;
                for (const piece of placed(pin.width, pin.height, radius, scale)) {
                    const r = piece.rect;
                    const along =
                        piece.name === 'top' || piece.name === 'bottom'
                            ? [0.5, r.width / 2, r.width - 0.5].map(x => ({ x: r.x + x, y: null }))
                            : piece.name === 'left' || piece.name === 'right'
                              ? [0.5, r.height / 2, r.height - 0.5].map(y => ({ x: null, y: r.y + y }))
                              : [{ x: null, y: null }];
                    for (const at of along) {
                        for (let row = 0; row < piece.height; row++) {
                            for (let column = 0; column < piece.width; column++) {
                                const x = at.x ?? r.x + ((column + 0.5) * r.width) / piece.width;
                                const y = at.y ?? r.y + ((row + 0.5) * r.height) / piece.height;
                                const expected = shadowAlpha(x, y, pin, scale, PIN_SHADOW);
                                worst = Math.max(worst, Math.abs(alphaAt(piece, column, row) - expected));
                            }
                        }
                    }
                }
                expect(worst, `scale ${scale}, radius ${radius}`).toBeLessThanOrEqual(1);
            }
        }
    });

    it('leave nothing under the pin, so a translucent pin is not darkened', () => {
        const pin = { width: 60, height: 40, radius: 8 };
        const whole = wholeShadow(pin, 1, PIN_SHADOW);
        const m = shadowMargins(PIN_SHADOW);
        const rect = { x: 0, y: 0, width: pin.width, height: pin.height };
        let inside = 0;
        for (let y = 0; y < pin.height; y++) {
            for (let x = 0; x < pin.width; x++) {
                // A pixel wholly inside the rounded pin, clear of its antialiased edge.
                if (roundedRectDistance(x + 0.5, y + 0.5, rect, pin.radius) > -0.5) continue;
                inside++;
                expect(alphaAt(whole, x + m.left, y + m.top)).toBe(0);
            }
        }
        expect(inside).toBeGreaterThan(2000);
    });

    it('fill the corner the pin rounds off', () => {
        const whole = wholeShadow({ width: 60, height: 40, radius: 8 }, 1, PIN_SHADOW);
        const m = shadowMargins(PIN_SHADOW);
        // The pixel in the pin's rect at its very corner is outside the rounded pin.
        expect(alphaAt(whole, m.left, m.top + 39)).toBeGreaterThan(0);
        const square = wholeShadow({ width: 60, height: 40, radius: 0 }, 1, PIN_SHADOW);
        expect(alphaAt(square, m.left, m.top + 39)).toBe(0);
    });

    it('are about a hundred kilobytes at scale 2, whatever the pin', () => {
        const bytes = piecePixels(8, 2, PIN_SHADOW).reduce((sum, p) => sum + p.pixels.length, 0);
        expect(bytes).toBeLessThan(128 * 1024);
    });

    it('give way to one piece for a pin too small to have straight edges', () => {
        expect(pieceRects({ width: 12, height: 200, radius: 8 }, PIN_SHADOW)).toBeNull();
        expect(pieceRects({ width: 200, height: 20, radius: 8 }, PIN_SHADOW)).toBeNull();
        expect(pieceRects({ width: 20, height: 24, radius: 8 }, PIN_SHADOW)).not.toBeNull();
    });
});
