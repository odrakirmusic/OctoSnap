// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Where a pet can be, and how it falls (`spec/14` §4). No `gi://` imports.
 *
 * **The floor** is the bottom of the work area of the monitor under the pet: above a dock
 * or a bottom panel, and on each monitor its own. A pet on a pinned screenshot stands on
 * the pin's top edge instead, and moves with it (`spec/14` §4, "Pins").
 *
 * **Falling** is plain gravity from wherever the pet was let go, with the throw's speed:
 * off the walls of the stage with some of the speed lost, and down onto the first surface
 * under it. The octopus falls slowly, arms spread.
 *
 * **The desk.** With `pet-roam` at `anywhere` a pet let go of away from the floor and from
 * the pins stays where it was put, seen from above (`spec/14` §4): the screen is a desk it
 * sits on, and its monitor's work area the desk's edges. A throw there is a slide, slowed
 * by friction and off the edges, rather than a fall.
 *
 * Everything here is in logical pixels and seconds, on the stage's coordinates.
 */

import type { Rect } from '../place.js';

export interface WorldMonitor {
    /** The monitor's logical rect. */
    rect: Rect;
    /** Its work area: the rect less panels and a dock that holds space. */
    work: Rect;
    scale: number;
}

/** Something a pet can stand on besides the floor: a pin, by the app's id for it. */
export interface Platform {
    id: string;
    rect: Rect;
}

export interface World {
    monitors: readonly WorldMonitor[];
    platforms: readonly Platform[];
    /** The stage's logical size. */
    width: number;
    height: number;
}

/** A pet as physics sees it: its feet's position and its size, all logical. */
export interface Body {
    /** Left edge. */
    x: number;
    /** Where the feet are: the bottom edge. */
    y: number;
    width: number;
    height: number;
    vx: number;
    vy: number;
}

/** Pixels per second per second, for a pet `artPixel` logical pixels to the art pixel. */
export function gravity(artPixel: number, floats: boolean): number {
    return (floats ? 200 : 800) * artPixel;
}

/** The fastest a falling pet goes, in pixels per second. */
export function terminalSpeed(artPixel: number, floats: boolean): number {
    return (floats ? 60 : 900) * artPixel;
}

/** The fastest a throw sends a pet, in pixels per second, either way. */
export function throwCap(artPixel: number): number {
    return 600 * artPixel;
}

/** How much speed a wall hands back. */
const BOUNCE = 0.6;

/** The monitor under a point, or the nearest one when none is (a gap between monitors). */
export function monitorAt(world: World, x: number, y: number): WorldMonitor | null {
    let best: WorldMonitor | null = null;
    let bestDistance = Infinity;
    for (const m of world.monitors) {
        const dx = x < m.rect.x ? m.rect.x - x : x >= m.rect.x + m.rect.width ? x - (m.rect.x + m.rect.width - 1) : 0;
        const dy = y < m.rect.y ? m.rect.y - y : y >= m.rect.y + m.rect.height ? y - (m.rect.y + m.rect.height - 1) : 0;
        const distance = Math.hypot(dx, dy);
        if (distance === 0) return m;
        if (distance < bestDistance) {
            bestDistance = distance;
            best = m;
        }
    }
    return best;
}

/** The monitor a pet belongs to: the one under its middle. */
export function monitorOf(world: World, body: Body): WorldMonitor | null {
    return monitorAt(world, body.x + body.width / 2, body.y - body.height / 2);
}

/**
 * Where a pet saved as `along` its monitor's work area comes back: its middle's x, and a
 * height inside that monitor. The height is what tells stacked monitors apart, since one x
 * is on both; and the x is kept inside the monitor, so a pet at the right edge of the left
 * one of two side by side is not on the right one.
 */
export function spotOn(monitor: WorldMonitor, along: number): [number, number] {
    const { work } = monitor;
    const x = work.x + Math.max(0, Math.min(1, along)) * work.width;
    return [Math.min(x, work.x + work.width - 1), work.y + work.height / 2];
}

/** Which of `monitors` a pet is on, by its middle and its feet; -1 for none. */
export function monitorIndexOf(monitors: readonly WorldMonitor[], body: Body): number {
    const middle = body.x + body.width / 2;
    const feet = body.y - 1;
    const across = (m: WorldMonitor) => middle >= m.rect.x && middle < m.rect.x + m.rect.width;
    const index = monitors.findIndex(m => across(m) && feet >= m.rect.y && feet < m.rect.y + m.rect.height);
    return index >= 0 ? index : monitors.findIndex(across);
}

/** Where the floor is under a pet: the bottom of its monitor's work area. */
export function floorOf(world: World, body: Body): number {
    const m = monitorOf(world, body);
    return m === null ? world.height : m.work.y + m.work.height;
}

/** A pet's horizontal limits while it stands on its monitor's floor. */
export function wallsOf(world: World, body: Body): { left: number; right: number } {
    const m = monitorOf(world, body);
    if (m === null) return { left: 0, right: world.width - body.width };
    return { left: m.work.x, right: m.work.x + m.work.width - body.width };
}

/** The limits on a platform: a pet stays on the pin it stands on. */
export function wallsOn(platform: Platform, body: Body): { left: number; right: number } {
    // Half of it may hang over an edge, which is how a pet sits on a ledge.
    const hang = body.width / 3;
    return { left: platform.rect.x - hang, right: platform.rect.x + platform.rect.width - body.width + hang };
}

/**
 * Whether a pet's feet are over a platform: the middle two thirds of the pet within the
 * pin's width.
 */
function over(platform: Platform, body: Body): boolean {
    const middle = body.x + body.width / 2;
    return middle >= platform.rect.x && middle <= platform.rect.x + platform.rect.width;
}

export interface Landing {
    /** Where it landed: the floor, or a platform by id. */
    on: 'floor' | string;
    /** The height fallen, in logical pixels, for how hard the landing was. */
    drop: number;
}

/**
 * One step of a fall, `dt` seconds long. Returns where it landed when it did, and leaves
 * the body's speed at zero; otherwise the body moved and is still falling. `from` is the
 * height the fall started at, for the size of the drop.
 */
export function fall(
    world: World,
    body: Body,
    dt: number,
    artPixel: number,
    floats: boolean,
    from: number,
): Landing | null {
    const g = gravity(artPixel, floats);
    const terminal = terminalSpeed(artPixel, floats);
    const before = body.y;
    body.vy = Math.min(terminal, body.vy + g * dt);
    if (floats) body.vx *= Math.pow(0.35, dt);
    body.x += body.vx * dt;
    body.y += body.vy * dt;

    // The stage's walls and ceiling.
    if (body.x < 0) {
        body.x = 0;
        body.vx = -body.vx * BOUNCE;
    } else if (body.x > world.width - body.width) {
        body.x = world.width - body.width;
        body.vx = -body.vx * BOUNCE;
    }
    if (body.y - body.height < 0 && body.vy < 0) {
        body.y = body.height;
        body.vy = 0;
    }

    if (body.vy < 0) return null;
    // Onto a pin: the feet crossed its top edge on the way down, over it.
    for (const p of world.platforms) {
        if (before <= p.rect.y && body.y >= p.rect.y && over(p, body)) {
            body.y = p.rect.y;
            body.vx = 0;
            body.vy = 0;
            return { on: p.id, drop: Math.max(0, p.rect.y - from) };
        }
    }
    const floor = floorOf(world, body);
    if (body.y >= floor) {
        body.y = floor;
        body.vx = 0;
        body.vy = 0;
        return { on: 'floor', drop: Math.max(0, floor - from) };
    }
    return null;
}

/**
 * The speed a pet leaves the hand with: the pointer's over its last `window` milliseconds,
 * capped. `trail` is `[time ms, x, y]`, oldest first.
 */
export function throwVelocity(
    trail: readonly (readonly [number, number, number])[],
    artPixel: number,
    window = 80,
): { vx: number; vy: number } {
    const last = trail[trail.length - 1];
    if (last === undefined) return { vx: 0, vy: 0 };
    let first = last;
    for (let i = trail.length - 1; i >= 0; i--) {
        const sample = trail[i];
        if (sample === undefined || last[0] - sample[0] > window) break;
        first = sample;
    }
    const dt = (last[0] - first[0]) / 1000;
    if (dt <= 0.008) return { vx: 0, vy: 0 };
    const cap = throwCap(artPixel);
    const clamp = (v: number) => Math.max(-cap, Math.min(cap, v));
    return { vx: clamp((last[1] - first[1]) / dt), vy: clamp((last[2] - first[2]) / dt) };
}

/** Where a pet let go of at `body` should stand if there is no fall to make: clamped to its floor. */
export function settle(world: World, body: Body): void {
    const { left, right } = wallsOf(world, body);
    body.x = Math.max(left, Math.min(right, body.x));
    body.y = floorOf(world, body);
}

/** A drop this many art pixels high or more is a hard landing: stars, and a wobble. */
export const HARD_DROP_ART = 40;

/**
 * Closer than this, in logical pixels, is touching, not overlapping. A pet walks in whole
 * art pixels, and at a fractional scale an art pixel is a fraction of a logical one, so
 * where a walk ends is a sum that can come out a hair past where the pet was sent.
 */
export const TOUCHING = 0.01;

/**
 * Whether `box` reaches into `rect`, or to within `margin` of its left and right sides and
 * `marginY` of its top and bottom.
 */
export function reaches(box: Rect, rect: Rect, margin: number, marginY = 0): boolean {
    return (
        box.x + box.width > rect.x - margin + TOUCHING &&
        box.x < rect.x + rect.width + margin - TOUCHING &&
        box.y + box.height > rect.y - marginY + TOUCHING &&
        box.y < rect.y + rect.height + marginY - TOUCHING
    );
}

/**
 * Where a pet can stand beside `area`, on its left (`side` -1) or its right (1): the
 * nearest place out from the area that keeps `gap` from it and from every box in `taken`,
 * or null when `work`, the work area, ends first. `pet` is the pet's box where it stands
 * now; only its size and its height on the stage count.
 */
export function besideArea(area: Rect, side: -1 | 1, pet: Rect, gap: number, taken: readonly Rect[], work: Rect): number | null {
    let x = side < 0 ? area.x - gap - pet.width : area.x + area.width + gap;
    // Each box can stand in the way once, since the place only moves further out.
    for (let i = 0; i <= taken.length; i++) {
        if (x < work.x - TOUCHING || x + pet.width > work.x + work.width + TOUCHING) return null;
        const there = { ...pet, x };
        const inTheWay = taken.find(box => reaches(there, box, gap));
        if (inTheWay === undefined) return x;
        x = side < 0 ? inTheWay.x - gap - pet.width : inTheWay.x + inTheWay.width + gap;
    }
    return null;
}

/**
 * Where a pet on the desk can stand beside `area`, above it (`side` -1) or below it (1):
 * `besideArea` turned on its side. It returns the top of the pet's box, the nearest place
 * out from the area that keeps `gap` from it and from every box in `taken`, or null when
 * `work` ends first.
 */
export function besideAreaY(area: Rect, side: -1 | 1, pet: Rect, gap: number, taken: readonly Rect[], work: Rect): number | null {
    let y = side < 0 ? area.y - gap - pet.height : area.y + area.height + gap;
    for (let i = 0; i <= taken.length; i++) {
        if (y < work.y - TOUCHING || y + pet.height > work.y + work.height + TOUCHING) return null;
        const there = { ...pet, y };
        const inTheWay = taken.find(box => reaches(there, box, 0, gap));
        if (inTheWay === undefined) return y;
        y = side < 0 ? inTheWay.y - gap - pet.height : inTheWay.y + inTheWay.height + gap;
    }
    return null;
}

// --- the desk --------------------------------------------------------------------------

/**
 * How near the floor, in art pixels, is at the bottom (`spec/14` §4). A pet let go of this
 * near it lands on the floor, seen from the side, and one on the desk wanders no nearer:
 * it comes down to the floor only on purpose (`descend`).
 */
export const FLOOR_BAND_ART = 10;

/**
 * How far above the floor's band, in art pixels, a pet on the desk goes on its own: the
 * band's own line counts as the floor (`atBottom`), so a pet that stopped on it would be
 * on the floor when it next came out.
 */
export const BAND_CLEARANCE_ART = 0.5;

/** How far above a pin's top edge, in art pixels, a pet let go of still lands on the pin. */
export const PIN_BAND_ART = 12;

/** Below this speed, in art pixels a second, a pet let go of on the desk is put down rather than slid. */
export const SLIDE_MIN_ART = 60;

/** How fast a slide slows, in art pixels a second, every second. */
const FRICTION_ART = 900;

/** Limits for a pet's left edge and its feet. */
export interface Bounds {
    left: number;
    right: number;
    top: number;
    bottom: number;
}

/**
 * Where a pet on the desk can be: all of its monitor's work area, down to the floor. Its
 * feet are its bottom edge, so they are at most on the floor, and at least its height
 * below the work area's top.
 */
export function deskOf(world: World, body: Body): Bounds {
    const m = monitorOf(world, body);
    const work = m?.work ?? { x: 0, y: 0, width: world.width, height: world.height };
    return {
        left: work.x,
        right: work.x + work.width - body.width,
        top: work.y + body.height,
        bottom: work.y + work.height,
    };
}

/** Whether a pet's feet are within `FLOOR_BAND_ART` of its floor, or under it. */
export function atBottom(world: World, body: Body, artPixel: number): boolean {
    return body.y >= floorOf(world, body) - FLOOR_BAND_ART * artPixel;
}

/**
 * Whether a pet let go of at `body` comes down onto something rather than staying on the
 * desk: near the floor, or with its feet just above a pin's top edge and over the pin.
 */
export function landsBelow(world: World, body: Body, artPixel: number): boolean {
    if (atBottom(world, body, artPixel)) return true;
    const reach = PIN_BAND_ART * artPixel;
    return world.platforms.some(p => body.y <= p.rect.y && body.y >= p.rect.y - reach && over(p, body));
}

/**
 * One step of a slide across the desk, `dt` seconds long: slowed by friction, and off the
 * desk's edges with some of the speed lost. Returns true once it has stopped, with the
 * speed at zero.
 */
export function slide(body: Body, dt: number, artPixel: number, bounds: Bounds): boolean {
    const speed = Math.hypot(body.vx, body.vy);
    const slow = FRICTION_ART * artPixel * dt;
    if (speed <= slow) {
        body.vx = 0;
        body.vy = 0;
        return true;
    }
    const k = (speed - slow) / speed;
    body.vx *= k;
    body.vy *= k;
    body.x += body.vx * dt;
    body.y += body.vy * dt;
    if (body.x < bounds.left || body.x > bounds.right) {
        body.x = Math.max(bounds.left, Math.min(bounds.right, body.x));
        body.vx = -body.vx * BOUNCE;
    }
    if (body.y < bounds.top || body.y > bounds.bottom) {
        body.y = Math.max(bounds.top, Math.min(bounds.bottom, body.y));
        body.vy = -body.vy * BOUNCE;
    }
    return false;
}
