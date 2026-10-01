// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Falling, throwing and standing (`spec/14` §4), on the rigs the other tests use: one
 * panel under a shell bar, and the owner's two monitors at different scales and heights.
 */

import { describe, expect, it } from 'vitest';

import {
    type Body,
    FLOOR_BAND_ART,
    HARD_DROP_ART,
    PIN_BAND_ART,
    type World,
    atBottom,
    besideArea,
    besideAreaY,
    deskOf,
    fall,
    landsBelow,
    slide,
    floorOf,
    monitorAt,
    monitorIndexOf,
    reaches,
    settle,
    spotOn,
    throwVelocity,
    wallsOf,
} from './physics.js';

const ONE: World = {
    width: 1920,
    height: 1200,
    monitors: [{ rect: { x: 0, y: 0, width: 1920, height: 1200 }, work: { x: 0, y: 32, width: 1920, height: 1168 }, scale: 1 }],
    platforms: [],
};

/** A 2560x1440 at 1.25 beside a 1920x1200 at 1, bottoms not aligned. */
const TWO: World = {
    width: 2048 + 1920,
    height: 1200,
    monitors: [
        { rect: { x: 0, y: 0, width: 2048, height: 1152 }, work: { x: 0, y: 32, width: 2048, height: 1120 }, scale: 1.25 },
        { rect: { x: 2048, y: 0, width: 1920, height: 1200 }, work: { x: 2048, y: 0, width: 1920, height: 1200 }, scale: 1 },
    ],
    platforms: [],
};

/**
 * The developer's rig, and `pets-test.sh`'s second layout: a 1920x1200 at 2 on top, and a
 * 1920x1200 panel at 1.25 under it at x 443, with the shell's bar. From x 443 to 960, one x
 * is on both.
 */
const STACKED: World = {
    width: 443 + 1536,
    height: 600 + 960,
    monitors: [
        { rect: { x: 443, y: 600, width: 1536, height: 960 }, work: { x: 443, y: 632, width: 1536, height: 928 }, scale: 1.25 },
        { rect: { x: 0, y: 0, width: 960, height: 600 }, work: { x: 0, y: 0, width: 960, height: 600 }, scale: 2 },
    ],
    platforms: [],
};

function body(x: number, y: number, vx = 0, vy = 0): Body {
    return { x, y, width: 96, height: 96, vx, vy };
}

/** Drops a body until it lands, at 60 steps a second; returns the landing and the steps. */
function drop(world: World, b: Body, u = 3, floats = false) {
    const from = b.y;
    for (let i = 0; i < 600; i++) {
        const landed = fall(world, b, 1 / 60, u, floats, from);
        if (landed !== null) return { landed, steps: i + 1 };
    }
    throw new Error('never landed');
}

describe('the floor', () => {
    it('is the bottom of the work area under the pet', () => {
        expect(floorOf(ONE, body(500, 300))).toBe(1200);
        expect(floorOf(TWO, body(100, 300))).toBe(1152);
        expect(floorOf(TWO, body(3000, 300))).toBe(1200);
    });

    it('belongs to the monitor under the middle of the pet', () => {
        // Mostly on the right-hand monitor, a little over the edge.
        expect(floorOf(TWO, body(2048 - 30, 300))).toBe(1200);
        expect(floorOf(TWO, body(2048 - 70, 300))).toBe(1152);
    });

    it('holds a pet between its monitor\'s edges', () => {
        expect(wallsOf(TWO, body(100, 1152))).toEqual({ left: 0, right: 2048 - 96 });
        expect(wallsOf(TWO, body(3000, 1200))).toEqual({ left: 2048, right: 2048 + 1920 - 96 });
    });

    it('finds the nearest monitor for a point in no monitor', () => {
        // Below the shorter monitor, where the taller one's bottom strip is not.
        expect(monitorAt(TWO, 1000, 1180)?.scale).toBe(1.25);
        expect(monitorAt(TWO, 3000, 1180)?.scale).toBe(1);
    });

    it('settles a pet put down anywhere onto its floor, inside its walls', () => {
        const b = body(-50, 400);
        settle(ONE, b);
        expect(b).toMatchObject({ x: 0, y: 1200 });
    });
});

describe('falling', () => {
    it('lands on the floor, stopped, having fallen the height it was dropped from', () => {
        const b = body(500, 400);
        const { landed } = drop(ONE, b);
        expect(landed).toEqual({ on: 'floor', drop: 800 });
        expect(b).toMatchObject({ y: 1200, vx: 0, vy: 0 });
    });

    it('takes about as long as gravity says', () => {
        // 800 px at 2400 px/s^2 is 0.82 s.
        const { steps } = drop(ONE, body(500, 400));
        expect(steps).toBeGreaterThan(44);
        expect(steps).toBeLessThan(56);
    });

    it('floats, for the octopus: much slower, and drifting to a stop sideways', () => {
        const b = body(500, 400, 600, 0);
        const { steps } = drop(ONE, b, 3, true);
        expect(steps).toBeGreaterThan(200);
        expect(b.x).toBeLessThan(500 + 600 * 2);
    });

    it('bounces off the walls with some of its speed', () => {
        const b = body(1800, 400, 3000, 0);
        fall(ONE, b, 1 / 20, 3, false, 400);
        expect(b.x).toBe(1920 - 96);
        expect(b.vx).toBeLessThan(0);
        expect(Math.abs(b.vx)).toBeLessThan(3000);
    });

    it('stops at the top of the stage when thrown up', () => {
        const b = body(500, 150, 0, -4000);
        fall(ONE, b, 1 / 20, 3, false, 150);
        expect(b.y - b.height).toBeGreaterThanOrEqual(0);
        expect(b.vy).toBeGreaterThanOrEqual(0);
    });

    it('lands on a pin when it comes down across its top edge', () => {
        const world: World = { ...ONE, platforms: [{ id: 'pin-1', rect: { x: 400, y: 700, width: 400, height: 300 } }] };
        const b = body(500, 300);
        const { landed } = drop(world, b);
        expect(landed.on).toBe('pin-1');
        expect(b.y).toBe(700);
    });

    it('falls past a pin it is not over', () => {
        const world: World = { ...ONE, platforms: [{ id: 'pin-1', rect: { x: 1000, y: 700, width: 400, height: 300 } }] };
        const { landed } = drop(world, body(500, 300));
        expect(landed.on).toBe('floor');
    });

    it('calls a long drop hard, in art pixels whatever the size', () => {
        const u = 3;
        const { landed } = drop(ONE, body(500, 1200 - (HARD_DROP_ART + 5) * u));
        expect(landed.drop / u).toBeGreaterThan(HARD_DROP_ART);
    });
});

describe('throwing', () => {
    it('leaves the hand at the pointer\'s speed over its last moments', () => {
        const trail: [number, number, number][] = [
            [0, 0, 0],
            [100, 10, 0],
            [140, 50, -20],
            [180, 90, -40],
        ];
        const { vx, vy } = throwVelocity(trail, 3);
        expect(vx).toBeCloseTo(1000, 0);
        expect(vy).toBeCloseTo(-500, 0);
    });

    it('is capped, and a pet let go of without moving just drops', () => {
        expect(throwVelocity([[0, 0, 0], [20, 900, 0]], 3).vx).toBe(1800);
        expect(throwVelocity([[0, 5, 5]], 3)).toEqual({ vx: 0, vy: 0 });
        expect(throwVelocity([], 3)).toEqual({ vx: 0, vy: 0 });
    });

    it('drops a pet held still before it was let go, however fast it moved first', () => {
        // A fling that stopped, a second's rest, and the release where it rested.
        const trail: [number, number, number][] = [
            [0, 0, 0],
            [40, 200, -300],
            [80, 400, -600],
            [1080, 400, -600],
        ];
        expect(throwVelocity(trail, 3)).toEqual({ vx: 0, vy: 0 });
    });
});

describe('out of a recorded area', () => {
    // At 125 %, where an art pixel is 3.2 logical pixels: the area `pets-test.sh` records,
    // and the penguin, 32 art pixels wide, on the floor under it.
    const area = { x: 384, y: 480, width: 768, height: 480 };
    const work = { x: 0, y: 0, width: 1536, height: 960 };
    const unit = 3.2;
    const penguin = { x: 409.6, y: 960 - 32 * unit, width: 32 * unit, height: 32 * unit };

    it('counts a pet that walked to where it was sent as out, not a hair inside', () => {
        const target = besideArea(area, -1, penguin, 2 * unit, [], work);
        expect(target).toBeCloseTo(275.2, 9);
        // Its walk there: 21 strides of 2 art pixels, which do not sum to the target exactly.
        let x = penguin.x;
        for (let i = 0; i < 21; i++) x -= 2 * unit;
        expect(x).not.toBe(target);
        expect(reaches({ ...penguin, x }, area, 2 * unit)).toBe(false);
        expect(reaches({ ...penguin, x: x + 0.5 }, area, 2 * unit)).toBe(true);
        expect(reaches(penguin, area, 2 * unit)).toBe(true);
    });

    it('lines the pets that go up side by side, each clear of the one before', () => {
        const frog = { ...penguin, x: 633.6, width: 36 * unit };
        const first = besideArea(area, -1, penguin, 2 * unit, [], work) as number;
        const second = besideArea(area, -1, frog, 2 * unit, [{ ...penguin, x: first }], work) as number;
        expect(second).toBeCloseTo(first - 2 * unit - frog.width, 9);
        expect(reaches({ ...frog, x: second }, { ...penguin, x: first }, 2 * unit)).toBe(false);
    });

    it('stands clear of a pet already beside the area, or says there is no room', () => {
        const potato = { ...penguin, x: area.x + area.width + 10, width: 30 * unit };
        const past = besideArea(area, 1, penguin, 2 * unit, [potato], work) as number;
        expect(past).toBeCloseTo(potato.x + potato.width + 2 * unit, 9);
        // With one pet gone to the left already, the octopus near the screen's edge leaves
        // no room for another: the next place, at 166.4, would stand on it.
        const octopus = { ...penguin, x: 40, width: 40 * unit };
        expect(besideArea(area, -1, penguin, 2 * unit, [{ ...penguin, x: 275.2 }, octopus], work)).toBeNull();
        expect(besideArea(area, -1, penguin, 2 * unit, [{ ...penguin, x: 275.2 }], work)).toBeCloseTo(166.4, 9);
    });

    it('minds only the pets at its own height', () => {
        const onAPin = { ...penguin, x: 270, y: 100 };
        expect(besideArea(area, -1, penguin, 2 * unit, [onAPin], work)).toBeCloseTo(275.2, 9);
    });

    it('has no room beside an area as wide as the work area', () => {
        const wide = { ...area, x: 0, width: 1536 };
        expect(besideArea(wide, -1, penguin, 2 * unit, [], work)).toBeNull();
        expect(besideArea(wide, 1, penguin, 2 * unit, [], work)).toBeNull();
    });
});

describe('where a pet comes back', () => {
    it('comes back on the monitor it was saved on, where monitors are stacked', () => {
        // The octopus, a fifth of the way along the lower panel: an x the upper one has too.
        const [x, y] = spotOn(STACKED.monitors[0]!, 0.2);
        expect(x).toBeCloseTo(443 + 0.2 * 1536, 9);
        expect(monitorAt(STACKED, x, y)).toBe(STACKED.monitors[0]);
        expect(monitorAt(STACKED, x, 300)).toBe(STACKED.monitors[1]);
    });

    it('stays on its own monitor at the far edge, not on the one beside it', () => {
        const [x, y] = spotOn(TWO.monitors[0]!, 1);
        expect(monitorAt(TWO, x, y)).toBe(TWO.monitors[0]);
    });

    it('is saved as on the monitor under its feet, not the first one its middle is over', () => {
        const onPanel = { ...body(700, 1560), width: 128, height: 128 };
        const onTop = { ...body(700, 600), width: 128, height: 128 };
        expect(monitorIndexOf(STACKED.monitors, onPanel)).toBe(0);
        expect(monitorIndexOf(STACKED.monitors, onTop)).toBe(1);
        // Off every monitor, over the panel: the one its middle is over.
        expect(monitorIndexOf(STACKED.monitors, { ...onTop, x: 1500, y: 300 })).toBe(0);
        expect(monitorIndexOf(STACKED.monitors, { ...onTop, x: 3000 })).toBe(-1);
    });
});

describe('the desk', () => {
    // A pet 3 logical pixels to the art pixel, 96 by 96.
    const unit = 3;

    it('is the whole work area, the feet no lower than the floor', () => {
        expect(deskOf(ONE, body(500, 600))).toEqual({ left: 0, right: 1920 - 96, top: 32 + 96, bottom: 1200 });
        // On the right monitor of two, its own work area.
        expect(deskOf(TWO, body(2500, 600))).toEqual({ left: 2048, right: 2048 + 1920 - 96, top: 96, bottom: 1200 });
    });

    it('ends at the floor\'s band, where a pet is at the bottom', () => {
        expect(atBottom(ONE, body(500, 1200), unit)).toBe(true);
        expect(atBottom(ONE, body(500, 1200 - FLOOR_BAND_ART * unit), unit)).toBe(true);
        expect(atBottom(ONE, body(500, 1200 - FLOOR_BAND_ART * unit - 1), unit)).toBe(false);
    });

    it('keeps a pet let go of on it, but not near the floor or just over a pin', () => {
        const pin = { id: '7', rect: { x: 400, y: 700, width: 600, height: 300 } };
        const world: World = { ...ONE, platforms: [pin] };
        expect(landsBelow(world, body(1200, 500), unit)).toBe(false);
        expect(landsBelow(world, body(1200, 1195), unit)).toBe(true);
        // Its feet a little above the pin's top edge, and over it: onto the pin.
        expect(landsBelow(world, body(600, 700 - PIN_BAND_ART * unit), unit)).toBe(true);
        expect(landsBelow(world, body(600, 700 - PIN_BAND_ART * unit - 1), unit)).toBe(false);
        // On the pin's face, below its top edge: it stays there, on the desk.
        expect(landsBelow(world, body(600, 800), unit)).toBe(false);
        // Beside the pin, not over it.
        expect(landsBelow(world, body(1100, 690), unit)).toBe(false);
    });

    it('slides a thrown pet to a stop, the harder the throw the further', () => {
        const bounds = deskOf(ONE, body(900, 600));
        const slid = (vx: number) => {
            const b = body(900, 600, vx, 0);
            let steps = 0;
            while (!slide(b, 1 / 60, unit, bounds)) steps++;
            expect(b.vx).toBe(0);
            expect(b.vy).toBe(0);
            expect(steps).toBeLessThan(120);
            return b.x - 900;
        };
        expect(slid(300)).toBeGreaterThan(0);
        expect(slid(900)).toBeGreaterThan(slid(300) * 4);
        expect(slid(-900)).toBeLessThan(0);
    });

    it('slides off the desk\'s edges with some of its speed, and never past them', () => {
        const bounds = deskOf(ONE, body(40, 300));
        const b = body(40, 300, -1500, -1500);
        let turnedX = false;
        let turnedY = false;
        while (!slide(b, 1 / 60, unit, bounds)) {
            expect(b.x).toBeGreaterThanOrEqual(bounds.left);
            expect(b.y).toBeGreaterThanOrEqual(bounds.top);
            if (b.vx > 0) turnedX = true;
            if (b.vy > 0) turnedY = true;
        }
        expect(turnedX && turnedY).toBe(true);
    });

    it('has a way out up or down past a recorded area, clear of the pets there', () => {
        const area = { x: 400, y: 300, width: 800, height: 400 };
        const pet = { x: 600, y: 400, width: 96, height: 96 };
        const work = { x: 0, y: 32, width: 1920, height: 1168 };
        expect(besideAreaY(area, -1, pet, 6, [], work)).toBe(300 - 6 - 96);
        expect(besideAreaY(area, 1, pet, 6, [], work)).toBe(700 + 6);
        // A pet already under the area, where this one would go: below it instead.
        const under = { x: 620, y: 706, width: 96, height: 96 };
        expect(besideAreaY(area, 1, pet, 6, [under], work)).toBe(706 + 96 + 6);
        // One beside that place, not under it, is not in the way.
        const aside = { x: 800, y: 706, width: 96, height: 96 };
        expect(besideAreaY(area, 1, pet, 6, [aside], work)).toBe(700 + 6);
        // No room above an area at the top of the work area.
        expect(besideAreaY({ ...area, y: 40 }, -1, pet, 6, [], work)).toBeNull();
    });
});
