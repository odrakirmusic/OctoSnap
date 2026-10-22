// SPDX-License-Identifier: GPL-3.0-or-later

import { describe, expect, it } from 'vitest';

import { SETTLE_BACK_MS, SETTLE_MIN_MOVE_PX, SETTLE_PX, settleFor } from './settle.js';

describe('settleFor (D201)', () => {
    it('takes a card collapsing down a card-height 4 px past its place, then back', () => {
        const settle = settleFor(0, 158, 200);
        expect(settle).not.toBeNull();
        expect(settle?.pastX).toBeCloseTo(0);
        expect(settle?.pastY).toBeCloseTo(SETTLE_PX);
        expect(settle?.outMs).toBe(200);
        expect(settle?.backMs).toBe(SETTLE_BACK_MS);
    });

    it('goes past in the direction of the move, by the same distance however long it is', () => {
        const up = settleFor(0, -400, 200);
        expect(up?.pastY).toBeCloseTo(-SETTLE_PX);
        const diagonal = settleFor(30, 40, 200);
        expect(Math.hypot(diagonal?.pastX ?? 0, diagonal?.pastY ?? 0)).toBeCloseTo(SETTLE_PX);
        expect(diagonal?.pastX).toBeCloseTo(SETTLE_PX * 0.6);
        expect(diagonal?.pastY).toBeCloseTo(SETTLE_PX * 0.8);
    });

    it('sends a nudge straight there', () => {
        expect(settleFor(0, SETTLE_MIN_MOVE_PX - 1, 200)).toBeNull();
        expect(settleFor(0, 0, 200)).toBeNull();
        expect(settleFor(0, SETTLE_MIN_MOVE_PX, 200)).not.toBeNull();
    });

    it('does nothing extra when there is no time to spend, as with animations off', () => {
        expect(settleFor(0, 158, 0)).toBeNull();
        expect(settleFor(0, 158, -5)).toBeNull();
        expect(settleFor(0, 158, Number.NaN)).toBeNull();
    });
});
