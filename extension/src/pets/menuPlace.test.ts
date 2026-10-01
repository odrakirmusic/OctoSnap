// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The menu's four placements (`spec/14` §10), at the middle of a screen and at each of its
 * edges: the menu is never off the work area and never over the pet it belongs to.
 */

import { describe, expect, it } from 'vitest';

import { MENU_GAP, MENU_MARGIN, placePill, placeRing } from './menuPlace.js';

const AREA = { x: 0, y: 32, width: 1920, height: 1168 };
const MENU = { width: 560, height: 64 };
const PET = { width: 96, height: 96 };

function at(x: number, y: number) {
    return { x, y, ...PET };
}

function inside(r: { x: number; y: number; width: number; height: number }) {
    return (
        r.x >= AREA.x + MENU_MARGIN &&
        r.y >= AREA.y + MENU_MARGIN &&
        r.x + r.width <= AREA.x + AREA.width - MENU_MARGIN &&
        r.y + r.height <= AREA.y + AREA.height - MENU_MARGIN
    );
}

function overlaps(a: { x: number; y: number; width: number; height: number }, b: typeof a) {
    return a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
}

describe('the pill', () => {
    it('opens above on Auto, centred on the pet', () => {
        const pet = at(900, 1000);
        const place = placePill(pet, MENU, AREA, 'auto');
        expect(place.side).toBe('above');
        expect(place.y).toBe(1000 - MENU_GAP - MENU.height);
        expect(place.x + MENU.width / 2).toBe(948);
    });

    it('opens below on Auto when there is no room above', () => {
        const place = placePill(at(900, 40), MENU, AREA, 'auto');
        expect(place.side).toBe('below');
        expect(place.y).toBe(40 + 96 + MENU_GAP);
    });

    it('opens below when asked, and above when below does not fit', () => {
        expect(placePill(at(900, 500), MENU, AREA, 'below').side).toBe('below');
        expect(placePill(at(900, 1200 - 96), MENU, AREA, 'below').side).toBe('above');
        expect(placePill(at(900, 40), MENU, AREA, 'above').side).toBe('below');
    });

    it('stays on the screen at the left and right edges', () => {
        for (const x of [0, 1920 - 96]) {
            const place = placePill(at(x, 1000), MENU, AREA, 'auto');
            expect(inside({ ...place, ...MENU })).toBe(true);
        }
    });

    it('never covers its pet, wherever the pet is', () => {
        for (const x of [0, 300, 912, 1824]) {
            for (const y of [40, 500, 1104]) {
                for (const placement of ['auto', 'above', 'below'] as const) {
                    const pet = at(x, y);
                    const place = placePill(pet, MENU, AREA, placement);
                    expect(inside({ ...place, ...MENU })).toBe(true);
                    expect(overlaps({ ...place, ...MENU }, pet)).toBe(false);
                }
            }
        }
    });
});

describe('the ring', () => {
    const CAPTION = { width: 140, height: 24 };

    it('is a full ring in the middle of the screen, starting at the top', () => {
        const pet = at(900, 500);
        const ring = placeRing(pet, 7, 40, AREA, CAPTION);
        expect(ring.centres.length).toBe(7);
        const top = ring.centres[0]!;
        expect(top.x).toBe(948);
        expect(top.y).toBeLessThan(500);
        // Evenly round: every button the same distance from the pet's middle.
        const r = ring.centres.map(c => Math.hypot(c.x - 948, c.y - 548));
        for (const d of r) expect(Math.abs(d - r[0]!)).toBeLessThan(1.5);
    });

    it('turns into an arc at an edge, and every button stays on the screen', () => {
        for (const [x, y] of [
            [0, 500],
            [1824, 500],
            [900, 1104],
            [900, 32],
            [0, 1104],
            [1824, 32],
        ] as const) {
            const ring = placeRing(at(x, y), 7, 40, AREA, CAPTION);
            for (const c of ring.centres) {
                expect(c.x - 20).toBeGreaterThanOrEqual(AREA.x);
                expect(c.x + 20).toBeLessThanOrEqual(AREA.x + AREA.width);
                expect(c.y - 20).toBeGreaterThanOrEqual(AREA.y);
                expect(c.y + 20).toBeLessThanOrEqual(AREA.y + AREA.height);
            }
        }
    });

    it('keeps its round buttons clear of the pet and of each other, at the edges too', () => {
        for (const [x, y] of [
            [900, 500],
            [0, 500],
            [1824, 1104],
            [0, 32],
        ] as const) {
            const pet = at(x, y);
            const ring = placeRing(pet, 7, 40, AREA, CAPTION);
            for (const [i, c] of ring.centres.entries()) {
                // The nearest point of the pet's box to the button's centre.
                const nx = Math.max(pet.x, Math.min(pet.x + pet.width, c.x));
                const ny = Math.max(pet.y, Math.min(pet.y + pet.height, c.y));
                expect(Math.hypot(c.x - nx, c.y - ny)).toBeGreaterThanOrEqual(20);
                for (const d of ring.centres.slice(i + 1)) expect(Math.hypot(c.x - d.x, c.y - d.y)).toBeGreaterThanOrEqual(40);
            }
        }
    });

    it('puts its caption on the screen, below the ring or above it at the bottom', () => {
        const low = placeRing(at(900, 1104), 7, 40, AREA, CAPTION);
        expect(low.caption.y + CAPTION.height).toBeLessThanOrEqual(AREA.y + AREA.height);
        const high = placeRing(at(900, 400), 7, 40, AREA, CAPTION);
        expect(high.caption.y).toBeGreaterThan(400 + 96);
    });
});
