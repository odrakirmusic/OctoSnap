// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * `spec/04` §2's placement, which is a set of measurements off a screenshot rather than a
 * design. Worth testing because every number in it is one somebody read off a picture.
 */

import { describe, expect, it } from 'vitest';

import { QAO_GAP, QAO_MARGIN, placeAtEdge, stackFits, stackOffset } from './place.js';

/** The rig: a 1920x1200 panel under a 32 px shell bar. */
const WORK = { x: 0, y: 32, width: 1920, height: 1168 };

/** A 16:10 card at the default size, per `spec/04` §2's ~207x125 measurement. */
const CARD = { width: 207, height: 129 };

describe('placeAtEdge', () => {
    it('hugs the left edge and the bottom for the first card', () => {
        const at = placeAtEdge(WORK, CARD, 'left', 0);
        expect(at.x).toBe(QAO_MARGIN);
        expect(at.y + CARD.height).toBe(WORK.y + WORK.height - QAO_MARGIN);
    });

    it('hugs the right edge when asked, at the same height', () => {
        const left = placeAtEdge(WORK, CARD, 'left', 0);
        const right = placeAtEdge(WORK, CARD, 'right', 0);
        expect(right.x).toBe(WORK.width - QAO_MARGIN - CARD.width);
        expect(right.y).toBe(left.y);
    });

    it('grows upward, not downward', () => {
        const first = placeAtEdge(WORK, CARD, 'left', 0);
        const second = placeAtEdge(WORK, CARD, 'left', stackOffset([CARD.height]));
        expect(second.y).toBeLessThan(first.y);
        // And it clears the card below it by exactly one gap.
        expect(first.y - (second.y + CARD.height)).toBe(QAO_GAP);
    });

    it('respects a work area that does not start at the origin', () => {
        // The second monitor on the two-display rig, with the panel on the other one.
        const area = { x: 1920, y: 0, width: 2560, height: 1440 };
        const at = placeAtEdge(area, CARD, 'left', 0);
        expect(at.x).toBe(1920 + QAO_MARGIN);
        expect(at.y + CARD.height).toBe(1440 - QAO_MARGIN);
    });

    it('clears the panel rather than being clamped onto it', () => {
        // The whole reason placement uses the work area: `move_frame` silently turns
        // (0, 0) into (0, 32). Nothing here should ever ask for a y above the work area.
        const tall = { width: 207, height: 1160 };
        const at = placeAtEdge(WORK, tall, 'left', 0);
        expect(at.y).toBeGreaterThanOrEqual(WORK.y);
    });

    it('keeps a card too tall for the work area inside it anyway', () => {
        const huge = { width: 207, height: 2000 };
        const at = placeAtEdge(WORK, huge, 'left', 0);
        expect(at.y).toBe(WORK.y);
        expect(at.x).toBeGreaterThanOrEqual(WORK.x);
    });

    it('keeps a card wider than the work area inside it', () => {
        const wide = { width: 3000, height: 129 };
        const at = placeAtEdge(WORK, wide, 'right', 0);
        expect(at.x).toBe(WORK.x);
    });

    it('clamps a stack that has grown past the top instead of leaving the screen', () => {
        const at = placeAtEdge(WORK, CARD, 'left', 100_000);
        expect(at.y).toBe(WORK.y);
    });
});

describe('placeAtEdge with a shadow band', () => {
    /** `spec/04` §9's transparent band, which is `SHADOW_MARGIN` on the app side. */
    const INSET = 16;
    /** The window is the card plus a band on every side. */
    const WINDOW = { width: CARD.width + 2 * INSET, height: CARD.height + 2 * INSET };

    it('puts the card, not the window, at the measured margin', () => {
        const at = placeAtEdge(WORK, WINDOW, 'left', 0, INSET);
        // What the user sees is the card: it must be exactly where the un-shadowed card
        // was, or the measurement in `spec/04` §2 has been quietly changed.
        expect(at.x + INSET).toBe(WORK.x + QAO_MARGIN);
        expect(at.y + INSET + CARD.height).toBe(WORK.y + WORK.height - QAO_MARGIN);

        const plain = placeAtEdge(WORK, CARD, 'left', 0);
        expect(at.x + INSET).toBe(plain.x);
        expect(at.y + INSET).toBe(plain.y);
    });

    it('does the same against the right edge', () => {
        const at = placeAtEdge(WORK, WINDOW, 'right', 0, INSET);
        expect(at.x + INSET + CARD.width).toBe(WORK.x + WORK.width - QAO_MARGIN);
    });

    it('keeps the gap between cards at 8, not at 8 plus two bands', () => {
        const first = placeAtEdge(WORK, WINDOW, 'left', 0, INSET);
        const second = placeAtEdge(WORK, WINDOW, 'left', stackOffset([CARD.height]), INSET);

        const firstCardTop = first.y + INSET;
        const secondCardBottom = second.y + INSET + CARD.height;
        expect(firstCardTop - secondCardBottom).toBe(QAO_GAP);

        // And the windows themselves overlap, which is the thing that buys that gap.
        expect(second.y + WINDOW.height).toBeGreaterThan(first.y);
    });

    it('never places a window outside the work area, so Mutter never clamps it', () => {
        const at = placeAtEdge(WORK, WINDOW, 'left', 0, INSET);
        expect(at.x).toBeGreaterThanOrEqual(WORK.x);
        expect(at.y + WINDOW.height).toBeLessThanOrEqual(WORK.y + WORK.height);
    });

    it('caps a band wider than the margin instead of going off-screen', () => {
        // A caller asking for a 40 px band would otherwise put the window 24 px outside
        // the work area, where the clamp would move the card.
        const wide = { width: CARD.width + 80, height: CARD.height + 80 };
        const at = placeAtEdge(WORK, wide, 'left', 0, 40);
        expect(at.x).toBe(WORK.x);
        expect(at.y + wide.height).toBe(WORK.y + WORK.height);
    });

    it('is unchanged when there is no band, which is every other role', () => {
        expect(placeAtEdge(WORK, CARD, 'left', 0, 0)).toEqual(placeAtEdge(WORK, CARD, 'left', 0));
    });
});

describe('stackOffset', () => {
    it('is zero for the first card', () => {
        expect(stackOffset([])).toBe(0);
    });

    it('sums the cards below plus a gap each', () => {
        expect(stackOffset([129])).toBe(129 + QAO_GAP);
        expect(stackOffset([129, 100])).toBe(129 + QAO_GAP + 100 + QAO_GAP);
    });

    it('handles cards of different heights, which is why it takes heights', () => {
        // `spec/04` §2: a card takes its size from the capture, so an index could not
        // locate the third card in a stack of three different shapes.
        const heights = [129, 207, 60];
        expect(stackOffset(heights)).toBe(129 + 207 + 60 + 3 * QAO_GAP);
    });
});

describe('stackFits', () => {
    it('is true for nothing and for the six cards spec/04 §2 saw at once', () => {
        expect(stackFits(WORK, [])).toBe(true);
        expect(stackFits(WORK, Array.from({ length: 6 }, () => CARD.height))).toBe(true);
    });

    it('is false once the stack would be clamped on top of itself', () => {
        expect(stackFits(WORK, Array.from({ length: 20 }, () => CARD.height))).toBe(false);
    });

    it('counts gaps between cards, not after the last one', () => {
        // The budget is the work area minus a margin at each end. One card can use all
        // of it; two cards must also pay for the single gap between them, and only that
        // one -- there is no gap after the last card.
        const budget = WORK.height - 2 * QAO_MARGIN;
        expect(stackFits(WORK, [budget])).toBe(true);
        expect(stackFits(WORK, [budget + 1])).toBe(false);
        expect(stackFits(WORK, [budget - QAO_GAP - 1, 1])).toBe(true);
        expect(stackFits(WORK, [budget - QAO_GAP, 1])).toBe(false);
    });
});
