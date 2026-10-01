// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * One pet on the desktop: where it is, what it is doing, and what it looks like this
 * frame (`spec/14`). The crew (`crew.ts`) owns the clock, the world and the input; this
 * is the part that is the same for every pet.
 *
 * **States.** On the ground it plays tricks, and blinks and breathes between them. Held,
 * it dangles from the pointer. In the air -- let go, thrown, or its pin gone from under
 * it -- it falls under `physics.ts`. With its menu open it looks at the menu. Asleep it
 * shows one frame and wants no clock at all until something wakes it. Hidden, for the
 * overview or a fullscreen window or a screen being shared, it is invisible and does
 * nothing, and picks up where it was when it comes back.
 *
 * **Two views** (`spec/14` §4). On the floor, on a pin and in the air a pet is seen from
 * the side. With the pets allowed anywhere, one let go of away from the floor and the pins
 * stays on the desk, seen from above and facing one of four ways; "the ground" is then the
 * desk, a throw slides it across the desk rather than letting it fall, and its tricks move
 * it up and down the desk as well as across. It changes view only on the floor's line,
 * where both views put its feet, or in the hand.
 *
 * **Where it is drawn.** Its body is in logical pixels -- the line its feet are on and its
 * left edge -- and its sprite is drawn from there: the view's `baseline` art pixels up from
 * the feet, a hop's height higher, both on the physical grid of the monitor it is on
 * (`frames.ts`).
 */

import Clutter from 'gi://Clutter';

import { info } from '../log.js';
import { PETS, type PetKind, type PetSpec, baselineOf } from './art/index.js';
import type { Grid } from './art/painter.js';
import { type Face, type Pose, type View, poseKey, quantizePose } from './art/pose.js';
import { ACTS, type ActContext } from './acts.js';
import { BLINK_MS, BREATH_MS, type Place, type Temperament, nextAct, nextBlink, pause } from './brain.js';
import { Effects } from './effects.js';
import { type PetSize, artPixel, snapToPhysical } from './frames.js';
import {
    BAND_CLEARANCE_ART,
    type Body,
    FLOOR_BAND_ART,
    HARD_DROP_ART,
    SLIDE_MIN_ART,
    type World,
    atBottom,
    deskOf,
    fall,
    floorOf,
    landsBelow,
    monitorAt,
    monitorOf,
    reaches,
    slide,
    wallsOf,
    wallsOn,
} from './physics.js';
import { Rng } from './rng.js';
import { type Fx, MOTION_FRAME_MS, Runner } from './runner.js';
import { type Frame, FrameCache, PetSprite } from './sprite.js';

export type PetState = 'ground' | 'air' | 'held' | 'menu' | 'asleep';

/** Why a pet is hidden; any one is enough. */
export type HideReason = 'overview' | 'modal' | 'sharing' | 'fullscreen' | 'recording' | 'scrolling' | 'capture';

/** A rectangle on the stage, logical. */
export interface Box {
    x: number;
    y: number;
    width: number;
    height: number;
}

/** What the crew lends a pet for each step. */
export interface Surroundings {
    world: World;
    size: PetSize;
    temperament: Temperament;
    /** A recording is on: quiet tricks only, and none that leave the ground. */
    quiet: boolean;
    /** A recorded area to stay out of. */
    avoid: Box | null;
    /**
     * Whether a pet whose left edge were at `x`, and its feet at `y` (where they are when
     * left out), would crowd another, or stand in the area to stay out of.
     */
    crowded(pet: Pet, x: number, y?: number): boolean;
    /**
     * True when a step's one piece of pose work may be done -- a new pose painted, or a
     * painted one made a texture -- and false once it has been, across the crew.
     */
    paint(): boolean;
}

/** The dangle while held: feet and arms swinging, every this many milliseconds. */
const DANGLE_MS = 110;

/** Tricks that stay on the ground and show nothing beside the pet, for `quiet`. */
const QUIET = ['look', 'sit', 'walk'];

/** Tricks that go up or down the desk, which walk across it instead while an area is kept clear. */
const ACROSS_THE_DESK = ['roam', 'climb', 'descend'];

/**
 * Tricks that take a pet somewhere by itself: what *Move on their own* off keeps it from,
 * as `brain.ts` does for the tricks it chooses, and what reduced motion stops.
 */
const ON_ITS_OWN = ['walk', 'roam', 'climb', 'descend', 'jump', 'ink', 'roll'];

/** The tricks that cross the floor's band on purpose, seen from above on it on the way. */
const ACROSS_THE_BAND = ['climb', 'descend'];

function clamp(value: number, low: number, high: number): number {
    return Math.max(low, Math.min(high, value));
}

export class Pet {
    readonly kind: PetKind;
    readonly spec: PetSpec;
    readonly rng: Rng;
    readonly sprite: PetSprite;
    readonly effects: Effects;
    /** Logical: the left edge, the feet's line, the size, the speed. */
    readonly body: Body = { x: 0, y: 0, width: 0, height: 0, vx: 0, vy: 0 };
    readonly #frames = new FrameCache(160);
    /** Art pixels from the sprite's top to under its feet, for each view (`baselineOf`). */
    readonly #baselines: Record<View, number>;
    /** Seen from the side, or from above on the desk. */
    #view: View = 'side';
    /** Which way it faces from above: the last way a pose said, and the screen at each new trick. */
    #face: Face = 'down';

    state: PetState = 'ground';
    /** The pin it stands on, or `null` for the floor. */
    platform: string | null = null;
    /** Where that pin's left edge was at the last step, so the pet rides along when it moves. */
    #platformX: number | null = null;
    readonly hidden = new Set<HideReason>();

    #runner: Runner | null = null;
    #act: string | null = null;
    #lastAct: string | null = null;
    /** Tricks to play next, before the brain chooses: a wake-up, then a reaction. */
    #queue: [string, number | undefined][] = [];
    #nextThink = 0;
    #nextBlink = 0;
    #blinkUntil = 0;
    #fallFrom = 0;
    #lastFallAt = 0;
    #unit = 3;
    #viewScale = 1;
    #origin = { x: 0, y: 0 };
    #shown: Frame | null = null;
    #shownKey = '';
    /** A new pose painted and waiting for its texture, which is the next step's work. */
    #pending: { key: string; grid: Grid } | null = null;
    #wanted: Pose = {};
    /** A pose held outside any trick: the menu's look, the sleep. */
    #still: Pose = {};

    constructor(kind: PetKind, seed: number, layers: { pets: Clutter.Actor; fx: Clutter.Actor; shadows: Clutter.Actor }) {
        this.kind = kind;
        this.spec = PETS[kind];
        this.rng = new Rng(seed);
        this.#baselines = { side: baselineOf(kind), top: baselineOf(kind, 'top') };
        this.sprite = new PetSprite(`octosnap-pet-${kind}`);
        layers.pets.add_child(this.sprite);
        this.effects = new Effects(layers.fx, layers.shadows, (seed ^ 0x9e3779b9) >>> 0, this.spec.mouth);
    }

    /** Logical pixels to the art pixel, on the pet's monitor. */
    get unit(): number {
        return this.#unit;
    }

    get visible(): boolean {
        return this.hidden.size === 0;
    }

    /** How it is seen: `top` on the desk, and while held over it; `side` everywhere else. */
    get view(): View {
        return this.#view;
    }

    get asleep(): boolean {
        return this.state === 'asleep' || this.#act === 'doze';
    }

    /** On the ground and doing nothing it was asked to: free to react to a capture. */
    get free(): boolean {
        return this.visible && (this.state === 'ground' || this.state === 'asleep');
    }

    /**
     * Free but for a capture's overlay, which it stepped aside for: what it hears then it
     * does when the overlay goes, so a screenshot still gets its squint.
     */
    get aside(): boolean {
        return this.hidden.size === 1 && this.hidden.has('capture') && (this.state === 'ground' || this.state === 'asleep');
    }

    /** On its way somewhere by itself, which a setting that keeps the pets still stops. */
    get movingOnItsOwn(): boolean {
        return this.#act !== null && ON_ITS_OWN.includes(this.#act);
    }

    /** What it is doing, for the log: its state, how it is seen, and its trick if it has one. */
    get doing(): string {
        return `${this.state}, seen from ${this.#view === 'top' ? 'above' : 'the side'}${this.#act === null ? '' : `, ${this.#act}`}`;
    }

    /** The sprite's box, logical, where it was last drawn. */
    get box(): Box {
        const [x, y] = this.sprite.get_position();
        const [width, height] = this.sprite.get_size();
        return { x, y, width, height };
    }

    /**
     * Puts the pet with its middle at `middle`, on the floor of the monitor under `middle, y`,
     * sized for that monitor. Where monitors are stacked one x is on both, and `y` says
     * which (`spotOn`).
     */
    place(world: World, size: PetSize, middle: number, y: number): void {
        const monitor = monitorAt(world, middle, y);
        this.#view = 'side';
        this.#measure(world, size, middle, y);
        this.body.x = middle - this.body.width / 2;
        this.body.y = monitor === null ? world.height : monitor.work.y + monitor.work.height;
        const walls = wallsOf(world, this.body);
        this.body.x = Math.max(walls.left, Math.min(walls.right, this.body.x));
        this.body.y = floorOf(world, this.body);
        this.state = 'ground';
        this.platform = null;
    }

    /**
     * Puts the pet on the desk, seen from above, with its middle at `middle` and its feet at
     * `y`, inside its monitor's work area; at the bottom, it goes on the floor instead.
     */
    placeOnDesk(world: World, size: PetSize, middle: number, y: number): void {
        this.#view = 'top';
        this.#face = 'down';
        this.#measure(world, size, middle, y - 1);
        this.body.x = middle - this.body.width / 2;
        this.body.y = y;
        this.#keepOnDesk(world);
        this.state = 'ground';
        this.platform = null;
        this.#platformX = null;
        if (atBottom(world, this.body, this.#unit)) this.#toSide(world, size);
    }

    /** Sized again for a size or a monitor that changed, with its feet where they were. */
    resize(world: World, size: PetSize): void {
        const middle = this.body.x + this.body.width / 2;
        this.#measure(world, size, middle, this.body.y - this.body.height / 2);
        this.body.x = middle - this.body.width / 2;
        if (this.state !== 'ground' || this.platform !== null) return;
        if (this.#view === 'top') {
            this.#keepOnDesk(world);
            // Grown into the floor's band: on the floor.
            if (this.#onTheBand(world)) this.#toSide(world, size);
        } else {
            const walls = wallsOf(world, this.body);
            this.body.x = Math.max(walls.left, Math.min(walls.right, this.body.x));
            this.body.y = floorOf(world, this.body);
        }
    }

    /**
     * Plays a trick now, stopping whatever it was doing. On the desk it turns to the screen
     * first; a trick that goes somewhere turns it the way it goes.
     */
    play(name: string, now: number, s: Surroundings, arg?: number, arg2?: number): void {
        const factory = ACTS[name];
        if (factory === undefined || !this.visible) return;
        this.#stopTrick();
        if (this.state === 'asleep') this.state = 'ground';
        this.#act = name;
        this.#face = 'down';
        this.#runner = new Runner(factory(this.#context(s), arg, arg2), now);
        this.#takeFx(now, s);
    }

    /**
     * Plays `name` after what it is doing, or now when it is doing nothing. A trick already
     * waiting is not queued twice -- three ticks of a countdown are one look at it -- and
     * no more than three wait.
     */
    queue(name: string, now: number, s: Surroundings, arg?: number): void {
        if (this.visible && this.#runner === null && this.state === 'ground') this.play(name, now, s, arg);
        else if (this.#act !== name && !this.#queue.some(([queued]) => queued === name) && this.#queue.length < 3)
            this.#queue.push([name, arg]);
    }

    /**
     * Puts its left edge at `x` at once, and on the desk its feet at `y`, out of a
     * recording's way; off a pin, it falls.
     */
    relocate(x: number, y?: number): void {
        this.interrupt();
        this.body.x = x;
        // Thrown, it goes on down from where it is put, and on the desk it stops there.
        this.body.vx = 0;
        if (this.#view === 'top') {
            if (y !== undefined) this.body.y = y;
            this.body.vy = 0;
            if (this.state === 'air') this.state = 'ground';
        }
        if (this.platform !== null) this.drop(0, 0);
    }

    /** Stops the trick and forgets what was queued: picked up, or a menu. */
    interrupt(): void {
        this.#stopTrick();
        this.#queue = [];
    }

    lift(): void {
        this.interrupt();
        this.state = 'held';
        this.platform = null;
        this.#platformX = null;
    }

    /**
     * Let go of from the hand, at the pointer's speed. With the pets allowed anywhere and
     * nothing just under it -- the floor, a pin's top edge -- it stays on the desk, seen from
     * above: slid, when thrown, and put down when not. Anything else falls (`drop`).
     */
    letGo(vx: number, vy: number, now: number, s: Surroundings): void {
        if (!this.#landsOnDesk(s)) {
            this.drop(vx, vy);
            return;
        }
        this.interrupt();
        this.#view = 'top';
        this.#face = 'down';
        this.platform = null;
        this.#platformX = null;
        this.#lastFallAt = 0;
        if (Math.hypot(vx, vy) >= SLIDE_MIN_ART * this.#unit) {
            this.state = 'air';
            this.body.vx = vx;
            this.body.vy = vy;
            return;
        }
        this.body.vx = 0;
        this.body.vy = 0;
        this.state = 'ground';
        this.#keepOnDesk(s.world);
        info(`${this.spec.name} put down on the desk at ${Math.round(this.body.x)},${Math.round(this.body.y)}, ${this.body.width.toFixed(1)} wide`);
        this.play('land', now, s, 0);
    }

    /** Off the desk, for the pets kept to the bottom again: seen from the side, it falls. */
    leaveDesk(): void {
        if (this.#view !== 'top') return;
        this.#view = 'side';
        if (this.state === 'ground' || this.state === 'asleep' || this.state === 'air') this.drop(0, 0);
    }

    /** Let go of, at the pointer's speed: seen from the side, it falls from where it is. */
    drop(vx: number, vy: number): void {
        this.interrupt();
        this.#view = 'side';
        this.state = 'air';
        this.platform = null;
        this.#platformX = null;
        this.body.vx = vx;
        this.body.vy = vy;
        this.#fallFrom = this.body.y;
        this.#lastFallAt = 0;
    }

    /** Held still for a menu, looking at it. */
    hold(pose: Pose): void {
        this.interrupt();
        if (this.state === 'ground' || this.state === 'asleep') this.state = 'menu';
        this.#still = pose;
    }

    release(now: number, s: Surroundings): void {
        if (this.state !== 'menu') return;
        this.state = 'ground';
        this.#nextThink = now + pause(this.rng, s.temperament) * 0.5;
    }

    /** Dozes off, and then needs no clock until `wake`. */
    sleep(now: number, s: Surroundings): void {
        if (this.state === 'ground' && !this.asleep) this.play('doze', now, s);
    }

    wake(now: number, s: Surroundings): void {
        if (this.asleep) {
            this.state = 'ground';
            this.play('wake', now, s);
        }
    }

    hide(reason: HideReason): void {
        const was = this.visible;
        this.hidden.add(reason);
        if (!was) return;
        this.interrupt();
        this.effects.flyAway();
        this.effects.hideAll();
        this.#fade(0);
    }

    show(reason: HideReason, now: number, s: Surroundings): void {
        if (!this.hidden.delete(reason) || !this.visible) return;
        this.#fade(255);
        this.#nextThink = now + pause(this.rng, s.temperament);
    }

    /**
     * Moves the pet on to `now`. Returns when it next needs a step: `Infinity` for a pet
     * that will look the same until something happens to it.
     */
    step(now: number, s: Surroundings): number {
        if (!this.visible) return Infinity;
        // Held, it is seen as it will land: from above over the desk.
        if (this.state === 'held') this.#view = this.#landsOnDesk(s) ? 'top' : 'side';
        this.#measure(s.world, s.size, this.body.x + this.body.width / 2, this.body.y - this.body.height / 2);
        let next: number;
        switch (this.state) {
            case 'held':
                this.#wanted = this.#dangle(now);
                next = now + DANGLE_MS - (now % DANGLE_MS);
                break;
            case 'air':
                next = this.#fall(now, s);
                break;
            case 'menu':
            case 'asleep':
                this.#wanted = this.#still;
                next = Infinity;
                break;
            case 'ground':
                next = this.#ground(now, s);
                break;
        }
        this.#render(now, s);
        if (this.effects.busy) next = Math.min(next, now + MOTION_FRAME_MS);
        // A pose there was no time to paint: another step soon, to paint it.
        if (this.#shownKey !== poseKey(this.#pose())) next = Math.min(next, now + 16);
        return next;
    }

    destroy(): void {
        this.#stopTrick();
        this.effects.destroy();
        this.sprite.destroy();
        this.#frames.clear();
    }

    // --- the parts of a step -----------------------------------------------------------

    #ground(now: number, s: Surroundings): number {
        const platform = this.platform === null ? null : (s.world.platforms.find(p => p.id === this.platform) ?? null);
        if (this.platform !== null && platform === null) {
            // Its pin is gone: it falls from where it stood.
            this.drop(0, 0);
            return now;
        }
        if (this.#view === 'top') {
            // On the desk: where it was put, inside its monitor's work area, for as long as
            // the pets may be anywhere.
            if (s.temperament.roam !== 'anywhere') {
                this.leaveDesk();
                return now;
            }
            this.#platformX = null;
            this.#keepOnDesk(s.world);
            // On the floor's band and not crossing it on purpose -- a descent broken off
            // there, a work area grown from below -- it is on the floor, seen from the side.
            if (this.#onTheBand(s.world)) {
                this.#toSide(s.world, s.size);
                info(`${this.spec.name} came down to the floor at ${Math.round(this.body.x)},${Math.round(this.body.y)}`);
            }
        } else if (platform !== null) {
            if (this.#platformX !== null) this.body.x += platform.rect.x - this.#platformX;
            this.#platformX = platform.rect.x;
            this.body.y = platform.rect.y;
        } else {
            this.#platformX = null;
            const floor = floorOf(s.world, this.body);
            // The floor moved down from under it: a dock hidden, a monitor rearranged.
            if (this.body.y < floor - 0.5) {
                this.drop(0, 0);
                return now;
            }
            this.body.y = floor;
        }

        if (this.#runner === null) {
            const queued = this.#queue.shift();
            if (queued !== undefined) this.play(queued[0], now, s, queued[1]);
            else if (this.#nextThink === 0) this.#nextThink = now + pause(this.rng, s.temperament);
            else if (now >= this.#nextThink) this.#think(now, s);
        }

        let next = Infinity;
        const runner = this.#runner;
        if (runner !== null) {
            runner.update(now);
            const dx = runner.takeDx() * this.#unit;
            const dy = runner.takeDy() * this.#unit;
            // A trick chosen before an area was kept clear stops at its edge (`#clear`).
            if (this.#wouldEnter(s.avoid, dx, this.#view === 'top' ? dy : 0)) {
                this.#stopTrick();
                this.#nextThink = now + pause(this.rng, s.temperament) * 0.5;
                return now + 1;
            }
            if (this.#view === 'top') {
                if (dx !== 0 || dy !== 0) {
                    const desk = deskOf(s.world, this.body);
                    this.body.x = clamp(this.body.x + dx, desk.left, desk.right);
                    this.body.y = clamp(this.body.y + dy, desk.top, desk.bottom);
                }
            } else if (dx !== 0) {
                const walls = platform !== null ? wallsOn(platform, this.body) : wallsOf(s.world, this.body);
                this.body.x = Math.max(walls.left, Math.min(walls.right, this.body.x + dx));
            }
            if (runner.pose.face !== undefined) this.#face = runner.pose.face;
            this.#takeFx(now, s);
            this.#wanted = runner.pose;
            if (runner.done) {
                const act = this.#act;
                this.#runner = null;
                this.#act = null;
                if (act === 'doze') {
                    this.state = 'asleep';
                    this.#still = runner.pose;
                    return Infinity;
                }
                this.#lastAct = act;
                this.#nextThink = now + pause(this.rng, s.temperament);
                next = this.#queue.length > 0 ? now : this.#nextThink;
            } else {
                next = runner.nextAt(now);
            }
        } else {
            this.#wanted = this.#breathe(now);
            const quarter = BREATH_MS / 4;
            next = Math.min(this.#nextThink, now + quarter - (now % quarter));
        }

        // Blinks, over whatever it is doing, when its eyes are open.
        if (this.#nextBlink === 0) this.#nextBlink = now + nextBlink(this.rng).in;
        if (now >= this.#nextBlink) {
            this.#blinkUntil = now + BLINK_MS;
            this.#nextBlink = now + nextBlink(this.rng).in;
        }
        if (now < this.#blinkUntil && (this.#wanted.mood ?? 'open') === 'open')
            this.#wanted = { ...this.#wanted, mood: 'blink' };
        next = Math.min(next, now < this.#blinkUntil ? this.#blinkUntil : this.#nextBlink);
        return Math.max(now + 1, next);
    }

    #think(now: number, s: Surroundings): void {
        const where: Place = this.#view === 'top' ? 'desk' : this.platform !== null ? 'pin' : 'floor';
        let name = nextAct(this.rng, this.spec, s.temperament, this.#lastAct, where);
        // A way up or down the desk could cross an area kept clear; across it, the walk keeps out.
        if (s.avoid !== null && ACROSS_THE_DESK.includes(name)) name = 'walk';
        if (s.quiet && !QUIET.includes(name)) name = 'look';
        this.play(name, now, s);
    }

    #fall(now: number, s: Surroundings): number {
        const dt = this.#lastFallAt === 0 ? 1 / 60 : Math.min(0.05, Math.max(0, now - this.#lastFallAt) / 1000);
        this.#lastFallAt = now;
        if (this.#view === 'top') return this.#slide(now, dt, s);
        const landed = fall(s.world, this.body, dt, this.#unit, this.spec.floats, this.#fallFrom);
        if (this.spec.floats) {
            this.#wanted = { phase: Math.floor(now / 150) % 4, mood: 'happy', squash: -0.3, arms: 'up' };
        } else {
            const swing = Math.floor(now / 90) % 2 === 0;
            this.#wanted = { mood: 'wide', squash: -0.6, arm: 'up', arms: 'up', step: swing ? 1 : 3 };
        }
        if (landed === null) return now + MOTION_FRAME_MS;
        info(`${this.spec.name} landed on ${landed.on === 'floor' ? 'the floor' : `pin ${landed.on}`} at ${Math.round(this.body.x)},${Math.round(this.body.y)}, ${this.body.width.toFixed(1)} wide`);
        this.state = 'ground';
        this.platform = landed.on === 'floor' ? null : landed.on;
        this.play('land', now, s, landed.drop / this.#unit >= HARD_DROP_ART ? 1 : 0);
        return now + 1;
    }

    /** Thrown on the desk: sliding, wide-eyed and paddling, until friction stops it. */
    #slide(now: number, dt: number, s: Surroundings): number {
        if (s.temperament.roam !== 'anywhere') {
            this.leaveDesk();
            return now;
        }
        const stopped = slide(this.body, dt, this.#unit, deskOf(s.world, this.body));
        const swing = Math.floor(now / 90) % 2 === 0;
        this.#wanted = { mood: 'wide', squash: -0.3, arm: 'up', arms: 'up', step: swing ? 1 : 3, phase: swing ? 1 : 3 };
        if (!stopped) return now + MOTION_FRAME_MS;
        this.state = 'ground';
        // Slid to the bottom: on the floor, seen from the side.
        if (atBottom(s.world, this.body, this.#unit)) this.#toSide(s.world, s.size);
        info(`${this.spec.name} slid to a stop ${this.#view === 'top' ? 'on the desk' : 'on the floor'} at ${Math.round(this.body.x)},${Math.round(this.body.y)}, ${this.body.width.toFixed(1)} wide`);
        this.play('land', now, s, 0);
        return now + 1;
    }

    /** Whether, let go of now, it would stay on the desk rather than come down onto something. */
    #landsOnDesk(s: Surroundings): boolean {
        return s.temperament.roam === 'anywhere' && !landsBelow(s.world, this.body, this.#unit);
    }

    /** Seen from above on the floor's band, and not on its way across it. */
    #onTheBand(world: World): boolean {
        return (
            this.#view === 'top' &&
            !(this.#act !== null && ACROSS_THE_BAND.includes(this.#act)) &&
            atBottom(world, this.body, this.#unit)
        );
    }

    /**
     * Whether moving `dx, dy` would take it into `avoid`, an area kept clear, when it is not
     * in it yet: in it as `#clear` counts it, within two art pixels to either side.
     */
    #wouldEnter(avoid: Box | null, dx: number, dy: number): boolean {
        if (avoid === null || (dx === 0 && dy === 0)) return false;
        const margin = this.#unit * 2;
        const box = { x: this.body.x, y: this.body.y - this.body.height, width: this.body.width, height: this.body.height };
        return !reaches(box, avoid, margin) && reaches({ ...box, x: box.x + dx, y: box.y + dy }, avoid, margin);
    }

    /** Inside its monitor's work area, feet no lower than the floor. */
    #keepOnDesk(world: World): void {
        const desk = deskOf(world, this.body);
        this.body.x = clamp(this.body.x, desk.left, desk.right);
        this.body.y = clamp(this.body.y, desk.top, desk.bottom);
    }

    /** Seen from the side again, on the floor under it, inside its walls. */
    #toSide(world: World, size: PetSize): void {
        this.#view = 'side';
        this.#measure(world, size, this.body.x + this.body.width / 2, this.body.y - 1);
        const walls = wallsOf(world, this.body);
        this.body.x = clamp(this.body.x, walls.left, walls.right);
        this.body.y = floorOf(world, this.body);
    }

    /**
     * A trick's `view`: onto the desk only from the floor, with the pets allowed anywhere;
     * onto the floor from wherever it is on the desk, which a trick asks for at the floor's line.
     */
    #see(view: View, face: Face, s: Surroundings): void {
        if (view === this.#view || this.state !== 'ground') return;
        if (view === 'side') {
            this.#toSide(s.world, s.size);
            info(`${this.spec.name} came down to the floor at ${Math.round(this.body.x)},${Math.round(this.body.y)}`);
            return;
        }
        if (s.temperament.roam !== 'anywhere' || this.platform !== null) return;
        this.#view = 'top';
        this.#face = face;
        this.#measure(s.world, s.size, this.body.x + this.body.width / 2, this.body.y - 1);
        info(`${this.spec.name} climbs onto the desk at ${Math.round(this.body.x)},${Math.round(this.body.y)}`);
    }

    #dangle(now: number): Pose {
        const wig = Math.floor(now / DANGLE_MS) % 2 === 1;
        return { mood: 'wide', step: wig ? 1 : 3, squash: -0.5, arm: wig ? 'up' : 'down', arms: 'down', phase: wig ? 2 : 0 };
    }

    /** At rest: a squash step in and out over each breath. */
    #breathe(now: number): Pose {
        const quarter = Math.floor((now % BREATH_MS) / (BREATH_MS / 4));
        return quarter === 1 ? { squash: 0.1 } : quarter === 3 ? { squash: -0.1 } : {};
    }

    #takeFx(now: number, s: Surroundings): void {
        const runner = this.#runner;
        if (runner === null) return;
        for (const effect of runner.takeFx()) this.#effect(effect, now, s);
    }

    #effect(effect: Fx, now: number, s: Surroundings | null): void {
        switch (effect.kind) {
            case 'view':
                if (s !== null) this.#see(effect.view, effect.face ?? this.#face, s);
                break;
            case 'emote':
                this.effects.emote(effect.name, now);
                break;
            case 'burst':
                this.effects.burst(effect.colours, effect.count, effect.up, effect.at, now);
                break;
            case 'fly':
                this.effects.fly(now);
                break;
            case 'fly-hold':
                this.effects.flyHold();
                break;
            case 'fly-eaten':
                this.effects.flyEaten();
                break;
            case 'fly-flee':
            case 'fly-free':
                this.effects.flyAway();
                break;
            case 'tongue':
                this.effects.tongue(effect.ms, effect.hit, effect.back, now, effect.to ?? null);
                break;
            case 'vanish':
                this.sprite.opacity = 0;
                break;
            case 'appear':
                this.sprite.opacity = 255;
                break;
        }
    }

    #stopTrick(): void {
        const runner = this.#runner;
        if (runner === null) return;
        runner.stop();
        for (const effect of runner.takeFx()) this.#effect(effect, 0, null);
        this.#runner = null;
        this.#act = null;
        // A trick stopped mid-vanish must not leave the pet invisible.
        if (this.visible) this.sprite.opacity = 255;
    }

    #fade(opacity: number): void {
        this.sprite.remove_all_transitions();
        this.sprite.ease({ opacity, duration: 150, mode: Clutter.AnimationMode.EASE_OUT_QUAD });
    }

    #context(s: Surroundings): ActContext {
        return {
            rng: this.rng,
            spec: this.spec,
            room: () => this.#room(s),
            desk: () => this.#deskRoom(s),
            toFloor: () => (this.#view === 'top' ? Math.max(0, (floorOf(s.world, this.body) - this.body.y) / this.#unit) : null),
            climb: () => this.#climbRoom(s),
            crowded: (dx, dy = 0) => s.crowded(this, this.body.x + dx * this.#unit, this.body.y + dy * this.#unit),
            fly: () => this.effects.flyFromMouth(),
        };
    }

    /** Art pixels it may move either way: to its walls, and never into a recorded area. */
    #room(s: Surroundings): { left: number; right: number } {
        const platform = this.platform === null ? null : (s.world.platforms.find(p => p.id === this.platform) ?? null);
        const walls =
            this.#view === 'top' ? deskOf(s.world, this.body) : platform !== null ? wallsOn(platform, this.body) : wallsOf(s.world, this.body);
        let left = this.body.x - walls.left;
        let right = walls.right - this.body.x;
        const avoid = s.avoid;
        if (avoid !== null && this.body.y > avoid.y && this.body.y - this.body.height < avoid.y + avoid.height) {
            const margin = this.#unit * 2;
            if (this.body.x + this.body.width <= avoid.x)
                right = Math.min(right, avoid.x - margin - (this.body.x + this.body.width));
            else if (this.body.x >= avoid.x + avoid.width)
                left = Math.min(left, this.body.x - (avoid.x + avoid.width + margin));
        }
        return { left: Math.max(0, left / this.#unit), right: Math.max(0, right / this.#unit) };
    }

    /**
     * On the desk, art pixels it may move each way: to the desk's edges, down to the floor's
     * band and no further (`descend` goes on to the floor), and never into a recorded area.
     */
    #deskRoom(s: Surroundings): { left: number; right: number; up: number; down: number } | null {
        if (this.#view !== 'top') return null;
        const { left, right } = this.#room(s);
        const desk = deskOf(s.world, this.body);
        let up = (this.body.y - desk.top) / this.#unit;
        let down = (desk.bottom - this.body.y) / this.#unit - FLOOR_BAND_ART - BAND_CLEARANCE_ART;
        const avoid = s.avoid;
        if (avoid !== null && this.body.x + this.body.width > avoid.x && this.body.x < avoid.x + avoid.width) {
            const margin = 2;
            const top = this.body.y - this.body.height;
            if (this.body.y <= avoid.y) down = Math.min(down, (avoid.y - this.body.y) / this.#unit - margin);
            else if (top >= avoid.y + avoid.height) up = Math.min(up, (top - (avoid.y + avoid.height)) / this.#unit - margin);
        }
        return { left, right, up: Math.max(0, up), down: Math.max(0, down) };
    }

    /**
     * On the floor, with the pets allowed anywhere, the art pixels up that clear the floor's
     * band; `null` on a pin, with the pets kept to the bottom, or with no desk above to speak of.
     */
    #climbRoom(s: Surroundings): number | null {
        if (this.#view !== 'side' || this.platform !== null || s.temperament.roam !== 'anywhere') return null;
        const up = (this.body.y - deskOf(s.world, this.body).top) / this.#unit;
        return up >= FLOOR_BAND_ART * 3 ? FLOOR_BAND_ART + 1 : null;
    }

    /** The art pixel's size and the grid to snap to, for the monitor under `middle, y`. */
    #measure(world: World, size: PetSize, middle: number, y: number): void {
        const probe: Body = { ...this.body, x: middle - this.body.width / 2, y: y + this.body.height / 2 };
        const monitor = monitorOf(world, probe) ?? world.monitors[0] ?? null;
        const scale = monitor?.scale ?? 1;
        // GNOME lays the stage out in logical pixels for a fractional scale, so the
        // stage's own scale on a monitor is the monitor's (`frames.ts`).
        this.#viewScale = scale;
        this.#unit = artPixel(size, scale) / scale;
        this.#origin = { x: monitor?.rect.x ?? 0, y: monitor?.rect.y ?? 0 };
        this.body.width = this.spec.width * this.#unit;
        this.body.height = this.#baselines[this.#view] * this.#unit;
    }

    /**
     * The pose to paint: what it wants, seen from above facing its way on the desk, and
     * from the side with nothing of above about it, so that one frame has one key.
     */
    #pose(): Pose {
        if (this.#view === 'top') return { ...this.#wanted, view: 'top', face: this.#face };
        if (this.#wanted.face === undefined && this.#wanted.view === undefined) return this.#wanted;
        const side: Pose = { ...this.#wanted };
        delete side.face;
        delete side.view;
        return side;
    }

    #render(now: number, s: Surroundings): void {
        const pose = quantizePose(this.#pose());
        const key = poseKey(pose);
        let frame = this.#frames.peek(key);
        // A new pose is two pieces of work, and a step across the crew does one (`paint`):
        // its art painted into a grid, and in a later step the grid made a texture. Each
        // is a millisecond or so inside the shell, where the code runs colder than in a
        // benchmark, and the two with the rest of a step went past `spec/10` §7's 2 ms in
        // one step in ten (`pets-test.sh`, D144). A pet's first pose waits its turn too:
        // five of them in one step was the slowest step of all.
        if (frame === undefined && s.paint()) {
            const pending = this.#pending;
            if (pending?.key === key) {
                this.#pending = null;
                frame = this.#frames.get(key, () => pending.grid);
            } else {
                this.#pending = { key, grid: this.spec.draw(pose) };
            }
        }
        if (frame !== undefined) {
            this.#shown = frame;
            this.#shownKey = key;
        }
        const shown = this.#shown;
        if (shown === null) return;
        this.sprite.showFrame(shown, this.#unit);
        const air = this.#runner?.y ?? 0;
        const baseline = this.#baselines[this.#view];
        const left = snapToPhysical(this.body.x, this.#origin.x, this.#viewScale);
        const top = snapToPhysical(this.body.y - (baseline + air) * this.#unit, this.#origin.y, this.#viewScale);
        this.sprite.set_position(left, top);
        const platform = this.platform === null ? null : s.world.platforms.find(p => p.id === this.platform);
        // On the desk, and held or slid over it, its shadow is under its feet.
        const floor =
            this.#view === 'top'
                ? this.body.y
                : (platform?.rect.y ?? (this.state === 'ground' ? this.body.y : floorOf(s.world, this.body)));
        const height = (floor - this.body.y) / this.#unit + air;
        this.effects.setFront(this.#view === 'top' ? 1 : -1);
        this.effects.update(now, {
            x: left,
            y: top,
            unit: this.#unit,
            width: this.spec.width,
            height: baseline,
            originX: this.#origin.x,
            originY: this.#origin.y,
            viewScale: this.#viewScale,
            floor,
        }, Math.max(0, height), false);
    }
}
