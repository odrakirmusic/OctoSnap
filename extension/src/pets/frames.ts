// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * From a painted grid to what the stage needs, and the one rule that keeps pixel art crisp
 * (`spec/14` §3). No `gi://` imports, so it is tested under Node.
 *
 * **One art pixel is a whole number of screen pixels, at every scale.** A pet is drawn
 * `artPixel()` physical pixels to the art pixel -- the size setting's base times the
 * monitor's scale, rounded -- and never an in-between: 3 at 100 %, 4 at 125 % and 133 %,
 * 5 at 150 % and 167 %, 6 at 200 % for Medium. The stage lays actors out in logical pixels,
 * so the pet's logical size is that divided by the scale, and its position is put on the
 * monitor's physical grid (`snapToPhysical`). The texture is the grid itself, one texel per
 * art pixel, scaled up with nearest-neighbour filtering: every screen pixel then shows
 * exactly one art pixel, and none is ever a blend of two.
 */

import type { Grid } from './art/painter.js';

/**
 * `pet-size` (`spec/08` §11): screen pixels per art pixel at 100 %. Tiny is one, the
 * fewest there can be, which puts Omni at 32 pixels at 100 % (D155).
 */
export const SIZE_BASES = { tiny: 1, small: 2, medium: 3, large: 4, huge: 5 } as const;

export type PetSize = keyof typeof SIZE_BASES;

export function isPetSize(name: string): name is PetSize {
    return Object.hasOwn(SIZE_BASES, name);
}

/**
 * Physical pixels per art pixel for a size on a monitor at `scale`. Rounded, and never
 * below one: Small at 100 % is 2, and at 125 % it is 3, not 2.5. Tiny is 1 up to 133 %
 * and 2 from 150 %.
 */
export function artPixel(size: PetSize, scale: number): number {
    return Math.max(1, Math.round(SIZE_BASES[size] * scale));
}

/**
 * Logical pixels per art pixel: `artPixel` over the stage's own scale for that monitor.
 * `viewScale` is the monitor's scale when the stage is laid out in logical pixels, which
 * is how GNOME runs a fractional scale, and 1 when it is laid out in physical ones.
 */
export function artPixelLogical(size: PetSize, scale: number, viewScale: number): number {
    return artPixel(size, scale) / (viewScale > 0 ? viewScale : 1);
}

/**
 * `value`, a logical coordinate on a monitor whose logical origin along this axis is
 * `origin`, moved to the nearest physical pixel boundary of that monitor.
 */
export function snapToPhysical(value: number, origin: number, viewScale: number): number {
    const s = viewScale > 0 ? viewScale : 1;
    return origin + Math.round((value - origin) * s) / s;
}

/** A colour as the painter writes it -- `#rrggbb` or `#rrggbbaa` -- as RGBA bytes. */
export function parseColour(colour: string): [number, number, number, number] {
    const hex = colour.startsWith('#') ? colour.slice(1) : colour;
    if (!/^[0-9a-fA-F]{6}([0-9a-fA-F]{2})?$/.test(hex)) throw new Error(`not a colour: ${colour}`);
    const n = (i: number) => parseInt(hex.slice(i, i + 2), 16);
    return [n(0), n(2), n(4), hex.length === 8 ? n(6) : 255];
}

/**
 * The grid as premultiplied RGBA, row by row with no padding: what
 * `St.ImageContent.set_bytes` takes as `RGBA_8888_PRE`. `mirror` flips it left to right.
 */
export function gridToRGBA(grid: Grid, mirror = false): Uint8Array {
    const out = new Uint8Array(grid.w * grid.h * 4);
    const colours = new Map<string, [number, number, number, number]>();
    for (let y = 0; y < grid.h; y++) {
        for (let x = 0; x < grid.w; x++) {
            const c = grid.px[y * grid.w + (mirror ? grid.w - 1 - x : x)];
            if (c === null || c === undefined) continue;
            let rgba = colours.get(c);
            if (rgba === undefined) {
                const [r, g, b, a] = parseColour(c);
                rgba = [Math.round((r * a) / 255), Math.round((g * a) / 255), Math.round((b * a) / 255), a];
                colours.set(c, rgba);
            }
            out.set(rgba, (y * grid.w + x) * 4);
        }
    }
    return out;
}

/** A rectangle of art pixels. */
export interface ArtRect {
    x: number;
    y: number;
    w: number;
    h: number;
}

/**
 * The drawn pixels as a few rectangles, for picking: a click on a pet counts only where it
 * is drawn, and passes through the empty space around it to the window underneath
 * (`spec/14` §5). Runs of drawn pixels in each row, merged down while the rows below have
 * the same run, which is most of a round body.
 */
export function opaqueRects(grid: Grid, mirror = false): ArtRect[] {
    const done: ArtRect[] = [];
    // Rectangles still growing, by their run's `x,w`.
    let open = new Map<string, ArtRect>();
    for (let y = 0; y < grid.h; y++) {
        const next = new Map<string, ArtRect>();
        let x = 0;
        while (x < grid.w) {
            const at = (i: number) => grid.px[y * grid.w + (mirror ? grid.w - 1 - i : i)];
            if (at(x) === null || at(x) === undefined) {
                x++;
                continue;
            }
            let end = x + 1;
            while (end < grid.w && at(end) !== null && at(end) !== undefined) end++;
            const key = `${x},${end - x}`;
            const growing = open.get(key);
            if (growing !== undefined) {
                growing.h++;
                open.delete(key);
                next.set(key, growing);
            } else {
                next.set(key, { x, y, w: end - x, h: 1 });
            }
            x = end;
        }
        for (const rect of open.values()) done.push(rect);
        open = next;
    }
    for (const rect of open.values()) done.push(rect);
    return done.sort((a, b) => a.y - b.y || a.x - b.x);
}

/** Whether the art pixel at `x, y` is drawn. */
export function isOpaque(grid: Grid, x: number, y: number): boolean {
    if (x < 0 || y < 0 || x >= grid.w || y >= grid.h) return false;
    const c = grid.px[Math.floor(y) * grid.w + Math.floor(x)];
    return c !== null && c !== undefined;
}
