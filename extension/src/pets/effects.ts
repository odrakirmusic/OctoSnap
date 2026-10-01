// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * What a pet shows beside itself (`spec/14` §6): a heart or a note floating up from its
 * head, a burst of specks, the frog's fly and tongue, and the shadow every pet stands on.
 * The octopus's selection frame and its shutter's flash were here too, until a white box
 * round the octopus read as a fault of the drawing and not as a trick (D154).
 *
 * All of it is pixel art in the pet's own art pixels, and all of it moves in whole
 * physical pixels: the effects are moved by `update(now)` from the crew's clock rather
 * than eased by Clutter, whose in-between positions would put a crisp picture a fraction
 * of a pixel off the grid on every frame and make it shimmer. None of it is reactive.
 */

import Clutter from 'gi://Clutter';
import Cogl from 'gi://Cogl';

import { type EmoteName, emote as emoteGrid, fly as flyGrid, shadow as shadowGrid } from './art/emotes.js';
import { snapToPhysical } from './frames.js';
import { Rng } from './rng.js';
import { FrameCache } from './sprite.js';

/** Where a pet is, for placing what it shows: its sprite's top-left and its pixel grid. */
export interface Anchor {
    /** The sprite's top-left, logical. */
    x: number;
    y: number;
    /** Logical pixels per art pixel. */
    unit: number;
    /** The sprite's size in art pixels. */
    width: number;
    height: number;
    /** The monitor's logical origin and the stage's scale on it, for snapping. */
    originX: number;
    originY: number;
    viewScale: number;
    /** Where the floor is, logical. */
    floor: number;
}

interface Timed {
    start: number;
    duration: number;
}

interface Floating extends Timed {
    actor: Clutter.Actor;
    /** Art pixels over the head, where it starts, and how far it rises. */
    from: { x: number; y: number };
    rise: number;
    width: number;
    height: number;
}

interface Speck {
    actor: Clutter.Actor;
    x: number;
    y: number;
    vx: number;
    vy: number;
    start: number;
    life: number;
}

/** The frog's fly, in art pixels from the frog's mouth. */
interface FlyState {
    actor: Clutter.Actor;
    x: number;
    y: number;
    t0: number;
    mode: 'buzz' | 'held' | 'eaten' | 'away';
    /** Which way it leaves. */
    away: number;
    wings: boolean;
}

interface TongueState extends Timed {
    back: boolean;
    hit: boolean;
    /** Where it reaches instead of the fly, from the mouth, in art pixels. */
    to: { dx: number; dy: number } | null;
    segments: Clutter.Actor[];
}

const TONGUE = new Cogl.Color({ red: 0xff, green: 0x7f, blue: 0xa0, alpha: 0xff });
const TONGUE_TIP = new Cogl.Color({ red: 0xf0, green: 0x58, blue: 0x7e, alpha: 0xff });

function colourOf(hex: string): Cogl.Color {
    const n = (i: number) => parseInt(hex.slice(i, i + 2), 16);
    return new Cogl.Color({ red: n(1), green: n(3), blue: n(5), alpha: hex.length > 7 ? n(7) : 0xff });
}

/** One pet's effects. The crew owns one per pet and destroys it with the pet. */
export class Effects {
    readonly #fxLayer: Clutter.Actor;
    readonly #shadowLayer: Clutter.Actor;
    readonly #frames: FrameCache;
    readonly #rng: Rng;
    #floating: Floating[] = [];
    #specks: Speck[] = [];
    #fly: FlyState | null = null;
    #tongue: TongueState | null = null;
    #shadow: Clutter.Actor;
    #shadowKey = '';
    /**
     * Plain squares -- specks, the selection frame and its flash, the tongue -- put back
     * here when their effect ends and taken again by the next, rather than destroyed and
     * made anew. Making an actor costs a tenth of a millisecond or more inside the shell,
     * and a tongue wants forty of them in one step (D144).
     */
    #spare: Clutter.Actor[] = [];
    /** Where the tongue and the fly are measured from, in art pixels on the sprite. */
    readonly #mouth: { x: number; y: number };
    /**
     * Where the fly buzzes: over the frog's head (-1) seen from the side, and in front of
     * it (1) seen from above, facing the screen, where the tongue reaches down to it.
     */
    #front: -1 | 1 = -1;

    constructor(fxLayer: Clutter.Actor, shadowLayer: Clutter.Actor, seed: number, mouth: { x: number; y: number }) {
        this.#fxLayer = fxLayer;
        this.#shadowLayer = shadowLayer;
        this.#mouth = mouth;
        this.#frames = new FrameCache(48);
        this.#rng = new Rng(seed);
        this.#shadow = this.#pixelActor('octosnap-pet-shadow');
        this.#shadowLayer.add_child(this.#shadow);
    }

    /** Whether anything is still moving, so the crew keeps its clock at 30 frames a second. */
    get busy(): boolean {
        return (
            this.#floating.length > 0 ||
            this.#specks.length > 0 ||
            this.#fly !== null ||
            this.#tongue !== null
        );
    }

    /** Seen from the side (-1) or from above (1): where a fly buzzes from now on. */
    setFront(front: -1 | 1): void {
        this.#front = front;
    }

    /** The fly, from the mouth, in art pixels; `null` with no fly out. */
    flyFromMouth(): { dx: number; dy: number } | null {
        const f = this.#fly;
        if (f === null || f.mode === 'away' || f.mode === 'eaten') return null;
        return { dx: f.x, dy: f.y };
    }

    emote(name: EmoteName, now: number): void {
        const grid = emoteGrid(name);
        const frame = this.#frames.get(`emote:${name}`, () => grid);
        const actor = this.#pixelActor('octosnap-pet-emote');
        actor.set_content(frame.content);
        this.#fxLayer.add_child(actor);
        // Newer emotes stack above older ones still in the air.
        const lift = this.#floating.length * 3;
        this.#floating.push({
            actor,
            from: { x: -grid.w / 2, y: 4 + lift },
            rise: 6,
            width: grid.w,
            height: grid.h,
            start: now,
            duration: 1250,
        });
    }

    burst(colours: readonly string[], count: number, up: boolean, at: { x: number; y: number }, now: number): void {
        for (let i = 0; i < count; i++) {
            const angle = up ? this.#rng.range(-Math.PI * 0.9, -Math.PI * 0.1) : this.#rng.range(0, Math.PI * 2);
            // Art pixels a second.
            const speed = this.#rng.range(10, 40);
            const actor = this.#square('octosnap-pet-speck', colourOf(this.#rng.pick(colours)));
            this.#specks.push({
                actor,
                x: at.x,
                y: at.y,
                vx: Math.cos(angle) * speed,
                vy: Math.sin(angle) * speed - (up ? 7 : 0),
                start: now,
                life: this.#rng.range(700, 1300),
            });
        }
    }

    fly(now: number): void {
        if (this.#fly !== null) return;
        const actor = this.#pixelActor('octosnap-pet-fly');
        this.#fxLayer.add_child(actor);
        const from = this.#rng.sign();
        this.#fly = { actor, x: from * 60, y: this.#front * 30, t0: now, mode: 'buzz', away: from, wings: false };
    }

    flyHold(): void {
        if (this.#fly?.mode === 'buzz') this.#fly.mode = 'held';
    }

    flyEaten(): void {
        if (this.#fly !== null && this.#fly.mode !== 'away') this.#fly.mode = 'eaten';
    }

    /** The fly gets away: `flee` after a miss, `free` when the hunt was cut short. */
    flyAway(): void {
        const f = this.#fly;
        if (f === null) return;
        if (f.mode === 'eaten') {
            this.#drop(f.actor);
            this.#fly = null;
            return;
        }
        f.mode = 'away';
        f.away = f.x >= 0 ? 1 : -1;
    }

    tongue(ms: number, hit: boolean, back: boolean, now: number, to: { dx: number; dy: number } | null = null): void {
        this.#dropTongue();
        const segments: Clutter.Actor[] = [];
        for (let i = 0; i < 40; i++) segments.push(this.#square('octosnap-pet-tongue', i === 0 ? TONGUE_TIP : TONGUE));
        this.#tongue = { start: now, duration: Math.max(1, ms), back, hit, to, segments };
    }

    /** Moves everything to where it is at `now`, for a pet at `at`; `hidden` hides it all. */
    update(now: number, at: Anchor, air: number, hidden: boolean): void {
        const snap = (x: number, y: number, actor: Clutter.Actor) =>
            actor.set_position(snapToPhysical(x, at.originX, at.viewScale), snapToPhysical(y, at.originY, at.viewScale));
        const u = at.unit;
        const headX = at.x + (at.width / 2) * u;
        const headY = at.y;

        this.#updateShadow(at, air, hidden);

        this.#floating = this.#floating.filter(f => {
            const t = (now - f.start) / f.duration;
            if (t >= 1) {
                this.#drop(f.actor);
                return false;
            }
            // Up in whole art pixels, and fading over the last third.
            const rise = Math.round(f.rise * t);
            f.actor.visible = !hidden;
            f.actor.opacity = Math.round(255 * Math.min(1, (1 - t) * 3));
            f.actor.set_size(f.width * u, f.height * u);
            snap(headX + f.from.x * u, headY - (f.from.y + rise + f.height) * u, f.actor);
            return true;
        });

        this.#specks = this.#specks.filter(s => {
            const t = now - s.start;
            if (t >= s.life) {
                this.#spareSquare(s.actor);
                return false;
            }
            const secs = t / 1000;
            const x = s.x + s.vx * secs;
            const y = s.y + s.vy * secs + 20 * secs * secs;
            s.actor.visible = !hidden;
            s.actor.opacity = Math.round(255 * (1 - t / s.life));
            s.actor.set_size(u, u);
            snap(at.x + Math.round(x) * u, at.y + Math.round(y) * u, s.actor);
            return true;
        });

        this.#updateFly(now, at, hidden, snap);
        this.#updateTongue(now, at, hidden, snap);
    }

    /** Everything out of sight at once, for a pet that is hiding; the next update shows it again. */
    hideAll(): void {
        this.#shadow.visible = false;
        for (const f of this.#floating) f.actor.visible = false;
        for (const s of this.#specks) s.actor.visible = false;
        if (this.#fly !== null) this.#fly.actor.visible = false;
        for (const t of this.#tongue?.segments ?? []) t.visible = false;
    }

    destroy(): void {
        for (const f of this.#floating) f.actor.destroy();
        for (const s of this.#specks) this.#spareSquare(s.actor);
        this.#floating = [];
        this.#specks = [];
        this.#fly?.actor.destroy();
        this.#fly = null;
        this.#dropTongue();
        // The layer is the crew's, so the spares go by hand.
        for (const actor of this.#spare) actor.destroy();
        this.#spare = [];
        this.#shadow.destroy();
        this.#frames.clear();
    }

    #pixelActor(name: string): Clutter.Actor {
        const actor = new Clutter.Actor({ name, reactive: false });
        actor.set_content_scaling_filters(Clutter.ScalingFilter.NEAREST, Clutter.ScalingFilter.NEAREST);
        return actor;
    }

    #drop(actor: Clutter.Actor): void {
        actor.destroy();
    }

    /**
     * A plain square of `colour`, a spare when there is one, above every other effect as a
     * new one would be, and out of sight until the next `update` puts it in its place.
     */
    #square(name: string, colour: Cogl.Color): Clutter.Actor {
        let actor = this.#spare.pop();
        if (actor === undefined) {
            actor = new Clutter.Actor({ reactive: false });
            this.#fxLayer.add_child(actor);
        } else {
            this.#fxLayer.set_child_above_sibling(actor, null);
        }
        actor.name = name;
        actor.background_color = colour;
        actor.opacity = 255;
        actor.visible = false;
        return actor;
    }

    /** A square back among the spares, out of sight; past a hundred, destroyed. */
    #spareSquare(actor: Clutter.Actor): void {
        if (this.#spare.length >= 100) {
            actor.destroy();
            return;
        }
        actor.visible = false;
        this.#spare.push(actor);
    }

    #updateShadow(at: Anchor, air: number, hidden: boolean): void {
        // Narrower the higher the pet is, and gone well above the floor.
        const width = Math.max(6, Math.round(at.width * 0.62) - Math.round(air / 3));
        const key = `shadow:${width}`;
        const frame = this.#frames.get(key, () => shadowGrid(width));
        if (key !== this.#shadowKey) {
            this.#shadow.set_content(frame.content);
            this.#shadowKey = key;
        }
        const u = at.unit;
        this.#shadow.set_size(frame.width * u, frame.height * u);
        this.#shadow.visible = !hidden && air < 60;
        this.#shadow.opacity = Math.round(255 * Math.max(0.3, 1 - air / 60));
        this.#shadow.set_position(
            snapToPhysical(at.x + ((at.width - frame.width) / 2) * u, at.originX, at.viewScale),
            snapToPhysical(at.floor - 2 * u, at.originY, at.viewScale),
        );
    }

    #updateFly(now: number, at: Anchor, hidden: boolean, snap: (x: number, y: number, a: Clutter.Actor) => void): void {
        const f = this.#fly;
        if (f === null) return;
        const t = now - f.t0;
        const u = at.unit;
        switch (f.mode) {
            case 'buzz': {
                // Circling over the frog's head, or before it seen from above, closing in
                // over a second.
                const tx = Math.sin(t / 380) * 16;
                const ty = this.#front < 0 ? -26 + Math.cos(t / 260) * 6 : 18 + Math.cos(t / 260) * 4;
                const k = Math.min(1, t / 900);
                f.x += (tx - f.x) * 0.12 * (k + 0.2) + Math.sin(t / 90) * 0.2;
                f.y += (ty - f.y) * 0.12 * (k + 0.2) + Math.cos(t / 70) * 0.2;
                break;
            }
            case 'held':
                break;
            case 'eaten':
                // Pulled back along the tongue to the mouth.
                f.x *= 0.6;
                f.y *= 0.6;
                break;
            case 'away':
                f.x += f.away * 2.2;
                f.y -= 1.1;
                if (Math.abs(f.x) > 200 || f.y < -200) {
                    this.#drop(f.actor);
                    this.#fly = null;
                    return;
                }
                break;
        }
        const wings = Math.floor(t / 60) % 2 === 0;
        if (wings !== f.wings || f.actor.get_content() === null) {
            f.wings = wings;
            f.actor.set_content(this.#frames.get(`fly:${wings}`, () => flyGrid(wings)).content);
        }
        f.actor.set_size(4 * u, 2 * u);
        f.actor.visible = !hidden && !(f.mode === 'eaten' && Math.hypot(f.x, f.y) < 2);
        const mouthX = at.x + this.#mouth.x * u;
        const mouthY = at.y + this.#mouth.y * u;
        snap(mouthX + Math.round(f.x) * u - 2 * u, mouthY + Math.round(f.y) * u, f.actor);
    }

    #updateTongue(now: number, at: Anchor, hidden: boolean, snap: (x: number, y: number, a: Clutter.Actor) => void): void {
        const tongue = this.#tongue;
        if (tongue === null) return;
        const t = Math.min(1, (now - tongue.start) / tongue.duration);
        if (t >= 1 && tongue.back) {
            this.#dropTongue();
            return;
        }
        const u = at.unit;
        const target = tongue.to ?? this.flyFromMouth() ?? { dx: this.#fly?.x ?? 14, dy: this.#fly?.y ?? this.#front * 20 };
        // A miss reaches where the fly was, a little short and to the side.
        const tx = tongue.hit ? target.dx : target.dx + 6;
        const ty = target.dy;
        const reach = tongue.back ? 1 - t : t;
        const length = Math.hypot(tx, ty);
        const n = Math.max(1, Math.round((length / 2) * reach));
        const mouth = this.#mouth;
        for (const [i, segment] of tongue.segments.entries()) {
            if (i > n || hidden) {
                segment.visible = false;
                continue;
            }
            // The tip is segment 0, at the far end.
            const along = ((n - i) / n) * reach;
            const x = Math.round(tx * along);
            const y = Math.round(ty * along);
            segment.visible = true;
            segment.set_size((i === 0 ? 3 : 2) * u, (i === 0 ? 3 : 2) * u);
            snap(at.x + (mouth.x + x - 1) * u, at.y + (mouth.y + y - 1) * u, segment);
        }
        if (t >= 1 && !tongue.back) {
            // Out: stays until the trick asks for it back.
            tongue.start = now - tongue.duration;
        }
    }

    #dropTongue(): void {
        for (const s of this.#tongue?.segments ?? []) this.#spareSquare(s);
        this.#tongue = null;
    }
}
