// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Where the fly animation aims. Worth testing for the same reason `accel.test.ts` is:
 * a mistake here does not throw, it just files the capture somewhere odd.
 */

import { describe, expect, it } from 'vitest';

import { CARD_MAX, CARD_SHORT, EDGE_MARGIN, cardSize, cardSlot, flyLanding, flyScale } from './slot.js';

/** The rig this project is developed on: a 1920x1200 panel under a 32 px shell bar. */
const WORK_AREA = { x: 0, y: 32, width: 1920, height: 1168 };

describe('cardSize', () => {
    it('is the measured card, whatever the capture looked like', () => {
        // `docs/decisions.md` D43: a card is a handle to a capture, not a preview of its
        // geometry, so a stack of them is a list rather than a ragged column.
        expect(cardSize()).toEqual({ width: CARD_MAX, height: CARD_SHORT });
    });
});

describe('cardSlot', () => {
    it('anchors to the left edge and the bottom of the work area', () => {
        const slot = cardSlot(WORK_AREA);
        expect(slot.x).toBe(EDGE_MARGIN);
        expect(slot.y + slot.height).toBe(WORK_AREA.y + WORK_AREA.height - EDGE_MARGIN);
        expect(slot.width).toBe(CARD_MAX);
        expect(slot.height).toBe(CARD_SHORT);
    });

    it('respects a work area that does not start at the origin', () => {
        // The second monitor on the two-display rig: 2560x1440 at (0,0) with the panel
        // on the other one, so this work area's origin is the monitor's own.
        const area = { x: 1920, y: 0, width: 2560, height: 1440 };
        const slot = cardSlot(area);
        expect(slot.x).toBe(1920 + EDGE_MARGIN);
        expect(slot.y + slot.height).toBe(1440 - EDGE_MARGIN);
    });

    it('clamps a card that cannot fit the work area', () => {
        const tiny = { x: 0, y: 0, width: 120, height: 90 };
        const slot = cardSlot(tiny);
        expect(slot.width).toBeLessThanOrEqual(tiny.width - 2 * EDGE_MARGIN);
        expect(slot.height).toBeLessThanOrEqual(tiny.height - 2 * EDGE_MARGIN);
        expect(slot.y).toBeGreaterThanOrEqual(tiny.y);
    });
});

describe('flyLanding', () => {
    it('centres the capture in the card it flies to', () => {
        const source = { x: 0, y: 0, width: 400, height: 1200 };
        const slot = cardSlot(WORK_AREA);
        const landing = flyLanding(source, slot);
        const flownW = source.width * landing.scale;
        const flownH = source.height * landing.scale;
        // Within a pixel on each axis: the landing point is a whole number and the ideal
        // centre is not, so the two margins can differ by the rounding and no more.
        expect(Math.abs((landing.x - slot.x) - (slot.x + slot.width - (landing.x + flownW))))
            .toBeLessThanOrEqual(1);
        expect(Math.abs((landing.y - slot.y) - (slot.y + slot.height - (landing.y + flownH))))
            .toBeLessThanOrEqual(1);
    });

    it('lands a capture of the card\'s own shape exactly on it', () => {
        const source = { x: 0, y: 0, width: CARD_MAX * 4, height: CARD_SHORT * 4 };
        const slot = cardSlot(WORK_AREA);
        const landing = flyLanding(source, slot);
        expect(landing.x).toBe(slot.x);
        expect(landing.y).toBe(slot.y);
        expect(source.width * landing.scale).toBeCloseTo(slot.width, 5);
    });
});

describe('flyScale', () => {
    it('shrinks a full-HD capture into its card without overflowing it', () => {
        // Bounded by the card's *height* now, not its longest side: the card is a fixed
        // 207x125 box and a 16:10 capture is relatively taller than it. Asserting the
        // property rather than the ratio, because the ratio is the thing under test.
        const source = { x: 0, y: 0, width: 1600, height: 1000 };
        const slot = cardSlot(WORK_AREA);
        const k = flyScale(source, slot);
        expect(source.width * k).toBeLessThanOrEqual(slot.width);
        expect(source.height * k).toBeLessThanOrEqual(slot.height);
        // And it touches one edge, so it is as large as it can be.
        const touches =
            Math.abs(source.width * k - slot.width) < 1 || Math.abs(source.height * k - slot.height) < 1;
        expect(touches).toBe(true);
    });

    it('is 1 for a capture already smaller than a card', () => {
        const source = { x: 0, y: 0, width: 40, height: 30 };
        expect(flyScale(source, cardSlot(WORK_AREA))).toBe(1);
    });

    it('takes the smaller factor when the clamp changed the shape', () => {
        const source = { x: 0, y: 0, width: 1600, height: 1000 };
        const squashed = { x: 0, y: 0, width: 207, height: 40 };
        expect(flyScale(source, squashed)).toBeCloseTo(40 / 1000, 5);
    });

    it('does not divide by zero', () => {
        expect(flyScale({ x: 0, y: 0, width: 0, height: 0 }, WORK_AREA)).toBe(1);
    });
});
