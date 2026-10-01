// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Where a pet's menu opens (`spec/14` §10, `pet-menu`). No `gi://` imports.
 *
 * Three placements are a pill, the All-In-One toolbar's own row of modes: **Auto** puts it
 * above the pet and below when there is no room above; **Above** and **Below** ask for a
 * side and take the other one only when the asked side does not fit. **Around** is a ring
 * of round buttons about the pet, which turns into an arc -- the half facing away from the
 * nearest edge -- when a whole ring would leave the screen. Every placement is kept inside
 * the work area of the pet's monitor.
 */

import type { Rect, Size } from '../place.js';

export type MenuPlacement = 'auto' | 'above' | 'below' | 'around';

export function isMenuPlacement(name: string): name is MenuPlacement {
    return name === 'auto' || name === 'above' || name === 'below' || name === 'around';
}

/** Between the pet and its menu, and between the menu and the screen's edge. */
export const MENU_GAP = 8;
export const MENU_MARGIN = 4;

export interface PillPlace {
    x: number;
    y: number;
    side: 'above' | 'below';
}

/** The pill's top-left corner for a pet at `pet`, inside `area`. */
export function placePill(pet: Rect, menu: Size, area: Rect, placement: Exclude<MenuPlacement, 'around'>): PillPlace {
    const aboveY = pet.y - MENU_GAP - menu.height;
    const belowY = pet.y + pet.height + MENU_GAP;
    const roomAbove = aboveY >= area.y + MENU_MARGIN;
    const roomBelow = belowY + menu.height <= area.y + area.height - MENU_MARGIN;
    let side: 'above' | 'below' = placement === 'below' ? 'below' : 'above';
    if (side === 'above' && !roomAbove && roomBelow) side = 'below';
    if (side === 'below' && !roomBelow && roomAbove) side = 'above';
    let y = side === 'above' ? aboveY : belowY;
    // Neither side fits -- a pet held up by a small screen: over the pet, inside the area.
    y = Math.max(area.y + MENU_MARGIN, Math.min(area.y + area.height - MENU_MARGIN - menu.height, y));
    const middle = pet.x + pet.width / 2;
    const x = Math.max(
        area.x + MENU_MARGIN,
        Math.min(area.x + area.width - MENU_MARGIN - menu.width, middle - menu.width / 2),
    );
    return { x: Math.round(x), y: Math.round(y), side };
}

export interface RingPlace {
    /** Each button's centre, in the order given. */
    centres: { x: number; y: number }[];
    /** Where the caption naming the hovered button goes: its top-left corner. */
    caption: { x: number; y: number };
}

/**
 * `count` round buttons `button` across, about the pet. The first is at the top and they
 * go clockwise. A full ring when it fits; otherwise the widest arc that does, facing away
 * from the edges in the way.
 */
export function placeRing(pet: Rect, count: number, button: number, area: Rect, caption: Size): RingPlace {
    const cx = pet.x + pet.width / 2;
    const cy = pet.y + pet.height / 2;
    // Clear of the pet's corners, which are the nearest it comes to a round button.
    const clear = Math.hypot(pet.width / 2, pet.height / 2) + button / 2 + MENU_GAP / 2;
    // Neighbours this far apart, centre to centre, so no two buttons touch.
    const spacing = button + 6;
    const fits = (a: number, radius: number) => {
        const x = cx + Math.cos(a) * radius;
        const y = cy + Math.sin(a) * radius;
        const half = button / 2 + MENU_MARGIN;
        return (
            x - half >= area.x &&
            x + half <= area.x + area.width &&
            y - half >= area.y &&
            y + half <= area.y + area.height
        );
    };
    // A full ring, then half rings facing up, left, right and down, then quarter rings
    // for the corners, then wider arcs. Each at the radius its spacing needs.
    const tries: [number, number, boolean][] = [
        [-Math.PI / 2, 2 * Math.PI, true],
        [-Math.PI, Math.PI, false],
        [Math.PI / 2, Math.PI, false],
        [-Math.PI / 2, Math.PI, false],
        [0, Math.PI, false],
        [-Math.PI / 2, Math.PI / 2, false],
        [-Math.PI, Math.PI / 2, false],
        [0, Math.PI / 2, false],
        [Math.PI / 2, Math.PI / 2, false],
        [-Math.PI * 1.1, Math.PI * 1.2, false],
        [-Math.PI * 0.1, Math.PI * 1.2, false],
    ];
    let chosen: number[] | null = null;
    let radius = clear;
    for (const [start, span, full] of tries) {
        const gaps = full ? count : Math.max(1, count - 1);
        const r = Math.max(clear, (spacing * gaps) / span);
        const as = Array.from({ length: count }, (_, i) => start + (span * i) / gaps);
        if (as.every(a => fits(a, r))) {
            chosen = as;
            radius = r;
            break;
        }
    }
    if (chosen === null) {
        // Nothing fits -- a screen smaller than the ring: a row across the area instead.
        const y = Math.max(area.y + MENU_MARGIN + button / 2, pet.y - MENU_GAP - button / 2);
        const row = Array.from({ length: count }, (_, i) => ({
            x: Math.round(area.x + MENU_MARGIN + button / 2 + i * spacing),
            y: Math.round(y),
        }));
        return { centres: row, caption: { x: Math.round(area.x + MENU_MARGIN), y: Math.round(y + button / 2 + MENU_GAP) } };
    }
    const centres = chosen.map(a => ({
        x: Math.round(cx + Math.cos(a) * radius),
        y: Math.round(cy + Math.sin(a) * radius),
    }));
    // The caption under the ring, or over it when the ring is near the bottom.
    const lowest = Math.max(...centres.map(c => c.y));
    const highest = Math.min(...centres.map(c => c.y));
    let captionY = Math.max(lowest, cy + radius) + button / 2 + MENU_GAP;
    if (captionY + caption.height > area.y + area.height - MENU_MARGIN)
        captionY = Math.min(highest, cy - radius) - button / 2 - MENU_GAP - caption.height;
    const captionX = Math.max(
        area.x + MENU_MARGIN,
        Math.min(area.x + area.width - MENU_MARGIN - caption.width, cx - caption.width / 2),
    );
    return { centres, caption: { x: Math.round(captionX), y: Math.round(captionY) } };
}
