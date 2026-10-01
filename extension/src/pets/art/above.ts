// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Painting a pet seen from above (`spec/14` §2 and §4): the screen read as a desk it sits
 * on, the owner's idea of 2026-09-27 for a pet put down anywhere but the floor.
 *
 * A pet from above faces one of four ways. Each is painted, never a turned picture: the
 * shapes are placed in the pet's own frame -- `u` across it, to the canvas's right when it
 * faces down, and `v` forwards, the way it faces -- and `Facing` puts them on the canvas.
 * The light stays where it is for every facing, above and to the left, so a frog facing
 * left is lit as the frog facing right is, where a turned picture would carry its
 * highlight round with it.
 *
 * Eyes from above are beads, the same for every facing: a round dark eye with its
 * catchlight to the top left, where the light is. No `gi://` imports.
 */

import type { EllipseOptions, Painter, Tone } from './painter.js';
import type { Face, Mood } from './pose.js';

/** The way a pet faces as a unit vector on the canvas. */
const FRONT: Record<Face, readonly [number, number]> = {
    down: [0, 1],
    up: [0, -1],
    right: [1, 0],
    left: [-1, 0],
};

export class Facing {
    readonly face: Face;
    readonly cx: number;
    readonly cy: number;
    /** Forwards, on the canvas. */
    readonly fx: number;
    readonly fy: number;
    /** Across, on the canvas: to the canvas's right when the pet faces down. */
    readonly ax: number;
    readonly ay: number;
    /** What the painter's `rot` adds for a shape placed in the pet's frame. */
    readonly turn: number;

    /** Centred on `cx, cy`, facing `face`. */
    constructor(face: Face, cx: number, cy: number) {
        this.face = face;
        this.cx = cx;
        this.cy = cy;
        const [fx, fy] = FRONT[face];
        this.fx = fx;
        this.fy = fy;
        this.ax = fy;
        this.ay = -fx;
        this.turn = Math.atan2(this.ay, this.ax);
    }

    /** Facing down or up, so that across the pet is across the canvas. */
    get upright(): boolean {
        return this.fx === 0;
    }

    /** The canvas point `u` across and `v` forwards of the centre. */
    at(u: number, v: number): [number, number] {
        return [this.cx + u * this.ax + v * this.fx, this.cy + u * this.ay + v * this.fy];
    }

    /** An ellipse `ru` across and `rv` along the pet, turned `rot` more in its own frame. */
    ellipse(P: Painter, u: number, v: number, ru: number, rv: number, mat: string, options: EllipseOptions = {}): void {
        const [x, y] = this.at(u, v);
        P.ellipse(x, y, ru, rv, mat, { ...options, rot: (options.rot ?? 0) + this.turn });
    }

    /** The art pixel whose centre is nearest the point `u` across and `v` forwards. */
    set(P: Painter, u: number, v: number, mat: string, tone: Tone = 'b', lock = true): void {
        const [x, y] = this.at(u, v);
        P.set(Math.floor(x), Math.floor(y), mat, tone, lock);
    }

    /**
     * Two bead eyes `apart` across from each other, their middle `v` forwards: `size` art
     * pixels square, moved a pixel by `look` and `lookY` when the pet faces down.
     */
    eyes(P: Painter, v: number, apart: number, size: 2 | 3, mood: Mood, look = 0, lookY = 0): void {
        const dx = this.face === 'down' ? Math.max(-1, Math.min(1, Math.round(look))) : 0;
        const dy = this.face === 'down' ? Math.max(-1, Math.min(1, Math.round(lookY))) : 0;
        for (const side of [-1, 1]) {
            const [x, y] = this.at((side * apart) / 2, v);
            beadEye(P, Math.round(x - size / 2) + dx, Math.round(y - size / 2) + dy, size, mood, this.upright);
        }
    }
}

/**
 * A bead eye whose top-left pixel is at `x, y`: `size` 2 or 3 art pixels, dark with a
 * catchlight, or shut as the mood has it. A shut eye is a line across the pet: along the
 * canvas's rows when the pet faces down or up, down its columns when it faces a side.
 */
export function beadEye(P: Painter, x: number, y: number, size: 2 | 3, mood: Mood, rows: boolean): void {
    const set = (dx: number, dy: number, mat: string) => P.set(x + dx, y + dy, mat, 'b', true);
    if (mood === 'blink' || mood === 'closed' || mood === 'sleep' || mood === 'half' || mood === 'squeeze') {
        for (let i = 0; i < size; i++) set(rows ? i : size - 1, rows ? size - 1 : i, 'eye');
        return;
    }
    if (mood === 'happy') {
        // An arc: the ends a row lower than the middle, seen from the front.
        if (rows) {
            set(0, size - 1, 'eye');
            set(size - 1, size - 1, 'eye');
            for (let i = 1; i < size - 1; i++) set(i, size - 2, 'eye');
            if (size === 2) set(0, 0, 'eye');
        } else {
            for (let i = 0; i < size; i++) set(size - 1, i, 'eye');
        }
        return;
    }
    if (mood === 'dizzy') {
        set(0, 0, 'eye');
        set(size - 1, size - 1, 'eye');
        set(size - 1, 0, 'eye');
        set(0, size - 1, 'eye');
        return;
    }
    for (let dy = 0; dy < size; dy++) for (let dx = 0; dx < size; dx++) set(dx, dy, 'eye');
    set(0, 0, 'eyeHi');
    if (mood === 'wide' && size === 3) set(1, 0, 'eyeHi');
}
