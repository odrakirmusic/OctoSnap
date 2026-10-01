// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Every trick a pet knows (`spec/14` §6), as generators the runner plays (`runner.ts`).
 *
 * Each one re-rolls its details every time: speed (0.7 to 1.4 times), height, how many
 * times, which way, and for a few whether it works at all -- the frog misses a fly now
 * and then. That is the difference between a pet and a loop. The dice are the pet's own
 * (`rng.ts`), so a test with a seed sees the same tricks every run.
 *
 * All distances are art pixels, `dx` across the screen and `dy` down it. No `gi://` imports.
 */

import type { EmoteName } from './art/emotes.js';
import { type PetSpec } from './art/index.js';
import { OCTOPUS_HUES } from './art/octopus.js';
import type { Face, Pose } from './art/pose.js';
import type { Rng } from './rng.js';
import { type Act, arc, fx, hold, move } from './runner.js';

/** What a trick may ask about the world. The engine answers; a test fakes it. */
export interface ActContext {
    readonly rng: Rng;
    readonly spec: PetSpec;
    /** Art pixels the pet can move to its left and to its right from where it stands. */
    room(): { left: number; right: number };
    /**
     * On the desk (`spec/14` §4), the art pixels it can move each way from where it
     * stands: to the desk's edges, and down to the floor's band. `null` off the desk.
     */
    desk(): { left: number; right: number; up: number; down: number } | null;
    /** On the desk, the art pixels from its feet down to the floor; `null` off it. */
    toFloor(): number | null;
    /**
     * On the floor with the pets allowed anywhere, the art pixels up to clear the floor's
     * band, onto the desk; `null` when it cannot climb from where it is.
     */
    climb(): number | null;
    /** Whether standing `dx` art pixels across and `dy` down from here would crowd another pet. */
    crowded(dx: number, dy?: number): boolean;
    /** Where the pet's fly is from its mouth, in art pixels, or `null` when it has none. */
    fly(): { dx: number; dy: number } | null;
}

export type ActFactory = (ctx: ActContext, arg?: number, arg2?: number) => Act;

// --- the moves every trick is made of ------------------------------------------------

/** A speed factor: the plan's 0.7 to 1.4 times. */
function tempo(ctx: ActContext): number {
    return ctx.rng.range(0.7, 1.4);
}

/** `dx` clamped to the room there is. */
function within(ctx: ActContext, dx: number): number {
    const { left, right } = ctx.room();
    return Math.max(-left, Math.min(right, dx));
}

/** Which way a pet going `dx` across and `dy` down faces, seen from above. */
export function facing(dx: number, dy: number): Face {
    if (Math.abs(dy) > Math.abs(dx)) return dy > 0 ? 'down' : 'up';
    return dx < 0 ? 'left' : 'right';
}

/**
 * Walks, hops or glides `dx` art pixels across and `dy` down, at `speed` times the pet's
 * own pace, looking the way it goes and, seen from above, facing it.
 */
export function* travel(ctx: ActContext, dx: number, speed = 1, dy = 0): Act {
    const dir = Math.sign(dx) || 1;
    // Straight up or down the desk it looks ahead rather than to a side.
    const look = dy === 0 || Math.abs(dx) * 2 > Math.abs(dy) ? dir : 0;
    const face = facing(dx, dy);
    const distance = Math.hypot(dx, dy);
    const ux = distance > 0 ? dx / distance : 0;
    const uy = distance > 0 ? dy / distance : 0;
    let left = distance;
    if (ctx.spec.gait === 'hop') {
        while (left > 1) {
            const d = Math.min(left, ctx.rng.range(5, 10));
            yield hold({ squash: 0.7, look, face }, 90 / speed);
            yield arc({ look, face }, 300 / speed, ctx.rng.range(2.3, 5), ux * d, uy * d);
            yield hold({ squash: 0.8, look, face }, 70 / speed);
            left -= d;
        }
        return;
    }
    const glide = ctx.spec.gait === 'glide';
    const stride = glide ? 3 : 2;
    const steps = [1, 0, 3, 0] as const;
    const bobs = glide ? [0, 1, 2, 1] : [1, 0, 1, 0];
    let i = 0;
    while (left > 0.5) {
        i = (i + 1) % 4;
        const d = Math.min(stride, left);
        const pose: Pose = glide ? { phase: i, look, squash: -0.2, face } : { step: steps[i] ?? 0, look, face };
        yield move(ux * d, pose, bobs[i], uy * d);
        yield hold(pose, (glide ? 120 : 140) / speed);
        left -= d;
    }
    yield move(0, { look, face }, 0);
}

/** A hop on the spot or a little to one side, `times` times. */
function* hop(_ctx: ActContext, height: number, pose: Pose, dx = 0, k = 1): Act {
    yield hold({ ...pose, squash: 0.8 }, 110 * k);
    yield arc(pose, 340 * k, height, dx);
    yield hold({ ...pose, squash: 0.9 }, 100 * k);
}

function* emote(name: EmoteName): Act {
    yield fx({ kind: 'emote', name });
}

// --- the common tricks -----------------------------------------------------------------

/** Wanders a little way, somewhere not on top of another pet. */
export function* walk(ctx: ActContext): Act {
    // Somewhere no other pet is standing, or nowhere: a crowd thins from its edges.
    let dx = 0;
    for (let i = 0; i < 6; i++) {
        const tried = within(ctx, ctx.rng.range(13, 73) * ctx.rng.sign());
        if (ctx.crowded(tried)) continue;
        dx = tried;
        break;
    }
    yield* travel(ctx, dx, tempo(ctx));
    yield hold({}, ctx.rng.range(200, 600));
}

/**
 * On the desk: off across it in any direction, somewhere no other pet is, or nowhere. Its
 * room is the desk's edges and the floor's band, which it comes down into only to leave
 * the desk (`descend`).
 */
export function* roam(ctx: ActContext): Act {
    const desk = ctx.desk();
    if (desk === null) {
        yield* walk(ctx);
        return;
    }
    let to = { dx: 0, dy: 0 };
    for (let i = 0; i < 6; i++) {
        const angle = ctx.rng.range(0, Math.PI * 2);
        const distance = ctx.rng.range(13, 73);
        const dx = Math.max(-desk.left, Math.min(desk.right, Math.cos(angle) * distance));
        const dy = Math.max(-desk.up, Math.min(desk.down, Math.sin(angle) * distance));
        if (ctx.crowded(dx, dy)) continue;
        to = { dx, dy };
        break;
    }
    yield* travel(ctx, to.dx, tempo(ctx), to.dy);
    yield hold({}, ctx.rng.range(200, 600));
}

/**
 * Up off the floor onto the desk, with the pets allowed anywhere: a crouch, a hop up clear
 * of the floor's band seen from above, and off across the desk. The view changes on the
 * floor's line, where both views put the pet's feet, so nothing jumps.
 */
export function* climb(ctx: ActContext): Act {
    const up = ctx.climb();
    if (up === null) {
        yield* walk(ctx);
        return;
    }
    const k = tempo(ctx);
    yield hold({ squash: 0.8, lookY: -1 }, 150 / k);
    yield fx({ kind: 'view', view: 'top', face: 'up' });
    yield hold({ squash: 0.8, face: 'up' }, 60 / k);
    yield arc({ face: 'up' }, 380 / k, ctx.rng.range(3.5, 6), ctx.rng.range(-4, 4), -(up + ctx.rng.range(2, 12)));
    yield hold({ squash: 0.6, face: 'up' }, 110 / k);
    yield* roam(ctx);
}

/**
 * Down off the desk onto the floor: a walk down to within a hop of it, the hop, and seen
 * from the side again as it lands, on the floor's line.
 */
export function* descend(ctx: ActContext): Act {
    const first = ctx.toFloor();
    if (first === null) {
        yield* look(ctx);
        return;
    }
    const k = tempo(ctx);
    const hop = Math.min(first, ctx.rng.range(6, 9));
    if (first - hop > 0.5) yield* travel(ctx, within(ctx, ctx.rng.range(-12, 12)), k, first - hop);
    yield hold({ squash: 0.8, face: 'down' }, 120 / k);
    // Measured again: the walk down may have stopped short of where it was going.
    const rest = ctx.toFloor() ?? 0;
    yield arc({ face: 'down', mood: 'happy' }, 380 / k, ctx.rng.range(4, 7), 0, rest);
    yield fx({ kind: 'view', view: 'side' });
    yield* land(ctx, 0);
}

export function* jump(ctx: ActContext): Act {
    const k = ctx.rng.range(0.8, 1.3);
    const times = ctx.rng.chance(0.2) ? 2 : 1;
    for (let i = 0; i < times; i++) {
        yield hold({ squash: 0.9, mood: 'happy' }, 120 * k);
        const dx = within(ctx, ctx.rng.range(-3.3, 3.3));
        const height = ctx.rng.range(6.7, 14.7) * (i > 0 ? 1.25 : 1);
        yield arc({ mood: ctx.rng.chance(0.5) ? 'happy' : 'open' }, ctx.rng.range(430, 640) * k, height, dx);
        yield hold({ squash: 1 }, 110 * k);
    }
    yield hold({ squash: 0.3 }, 90);
}

/** A jump that stays put, for when wandering is off. */
export function* bounce(ctx: ActContext): Act {
    const k = ctx.rng.range(0.8, 1.3);
    yield* hop(ctx, ctx.rng.range(4, 9), { mood: 'happy' }, 0, k);
    yield hold({ squash: 0.3 }, 90);
}

export function* look(ctx: ActContext): Act {
    const n = ctx.rng.int(2, 4);
    for (let i = 0; i < n; i++)
        yield hold({ look: ctx.rng.pick([-1, 1, 0]), lookY: ctx.rng.chance(0.3) ? -1 : 0 }, ctx.rng.range(400, 900));
}

export function* sit(ctx: ActContext): Act {
    yield hold({ squash: 0.45, mood: ctx.rng.chance(0.5) ? 'happy' : 'open' }, ctx.rng.range(1200, 2600));
}

/** Petted: a heart and a happy hop. */
export function* heart(ctx: ActContext): Act {
    yield* emote('heart');
    yield* hop(ctx, ctx.rng.range(3.3, 6), { mood: 'happy' });
    yield hold({ mood: 'happy' }, 500);
}

/**
 * Down after a fall: a squash, and after a long drop (`arg` 1) a moment of seeing stars.
 * The octopus floats down and never lands hard.
 */
export function* land(ctx: ActContext, hard = 0): Act {
    if (hard > 0 && !ctx.spec.floats) {
        yield hold({ squash: 1, mood: 'squeeze' }, 220);
        yield* emote('star');
        for (let i = 0; i < 4; i++) yield hold({ squash: 0.2, mood: 'dizzy', look: i % 2 ? 1 : -1 }, 180);
        yield hold({ squash: 0.2, mood: 'wide' }, 300);
    } else {
        yield hold({ squash: 1, mood: 'happy' }, 140);
    }
    yield hold({ squash: 0.3 }, 120);
}

export function* wave(ctx: ActContext): Act {
    const n = ctx.rng.int(4, 7);
    const k = ctx.rng.range(0.8, 1.3);
    if (ctx.spec.kind === 'octopus') {
        for (let i = 0; i < n; i++) yield hold({ arms: i % 2 ? 'down' : 'up', mood: 'happy', phase: i % 4 }, 170 * k);
        return;
    }
    for (let i = 0; i < n; i++) yield hold({ arm: i % 2 ? 'wave' : 'wave2', mood: 'happy' }, 170 * k);
}

/**
 * Dozing off: eyes heavy, closed, one "z". The engine keeps the sleeping pose on screen
 * with no timer at all until something wakes it (`spec/14` §7's "redraw nothing while
 * asleep"), so this ends asleep rather than looping.
 */
export function* doze(_ctx: ActContext): Act {
    yield hold({ squash: 0.3, mood: 'half' }, 700);
    yield hold({ squash: 0.3, mood: 'blink' }, 400);
    yield* emote('zzz');
    yield hold({ squash: 0.4, mood: 'sleep' }, 0);
}

export function* wake(ctx: ActContext): Act {
    yield hold({ squash: 0.4, mood: 'blink' }, 200);
    yield hold({ squash: -0.6, mood: 'wide' }, 260);
    yield hold({ mood: 'happy' }, ctx.rng.range(300, 600));
}

// --- the pets' own tricks --------------------------------------------------------------

export function* camo(ctx: ActContext): Act {
    const hues = Object.keys(OCTOPUS_HUES);
    // A shuffle from the pet's own dice, so a seeded test sees one order.
    for (let i = hues.length - 1; i > 0; i--) {
        const j = ctx.rng.int(0, i);
        [hues[i], hues[j]] = [hues[j] ?? '', hues[i] ?? ''];
    }
    for (const hue of hues) yield hold({ hue, mood: 'happy' }, ctx.rng.range(260, 520));
    yield hold({ mood: 'happy' }, 400);
}

/** A puff of ink, and the octopus is somewhere else. */
export function* ink(ctx: ActContext): Act {
    yield hold({ squash: 0.8, mood: 'squeeze' }, 160);
    yield fx({ kind: 'burst', colours: ['#3b2358', '#4a2d70', '#2a1840'], count: 22, up: false, at: { x: 16, y: 18 } });
    yield fx({ kind: 'vanish' });
    yield hold({}, 420);
    // On the desk it can come back up or down it as well as across.
    const desk = ctx.desk();
    let dx = 0;
    let dy = 0;
    for (let i = 0; i < 4; i++) {
        const tried = within(ctx, ctx.rng.range(20, 47) * ctx.rng.sign());
        const triedY = desk === null ? 0 : Math.max(-desk.up, Math.min(desk.down, ctx.rng.range(-30, 30)));
        if (ctx.crowded(tried, triedY)) continue;
        dx = tried;
        dy = triedY;
        break;
    }
    yield move(dx, {}, undefined, dy);
    yield fx({ kind: 'appear' });
    yield* emote('spark');
    yield hold({ mood: 'happy', arms: 'up' }, 600);
}

/** The penguin flaps; a third of the time it actually takes off. */
export function* flap(ctx: ActContext): Act {
    const k = ctx.rng.range(0.75, 1.3);
    const n = ctx.rng.int(8, 14);
    const lift = ctx.rng.chance(0.35);
    let y = 0;
    for (let i = 0; i < n; i++) {
        const pose: Pose = { arm: i % 2 ? 'down' : 'flap', mood: lift && i > n / 2 ? 'happy' : 'open', lookY: -1 };
        y = lift ? Math.min(20, i * ctx.rng.range(1.3, 2.3)) : i % 2 ? 0 : 0.7;
        yield move(0, pose, y);
        yield hold(pose, 85 * k);
    }
    if (lift) {
        yield* emote('spark');
        let arm: 'down' | 'flap' = 'down';
        while (y > 0) {
            arm = arm === 'flap' ? 'down' : 'flap';
            y = Math.max(0, y - 0.5);
            const pose: Pose = { arm, mood: 'happy' };
            yield move(0, pose, y);
            yield hold(pose, 70);
        }
        yield* land(ctx, 0);
    } else {
        yield move(0, {}, 0);
        yield hold({ mood: 'closed', squash: 0.4 }, 600);
    }
}

/** The frog and a fly: watch it, shoot, and usually catch it. */
export function* catchFly(ctx: ActContext): Act {
    yield fx({ kind: 'fly' });
    try {
        const watch = ctx.rng.range(1400, 2800);
        for (let t = 0; t < watch; t += 60) {
            const at = ctx.fly();
            const pose: Pose =
                at === null
                    ? {}
                    : {
                          look: Math.abs(at.dx) < 7 ? 0 : Math.sign(at.dx),
                          lookY: at.dy < -8 ? -1 : at.dy > 3 ? 1 : 0,
                      };
            yield hold(pose, 60);
        }
        yield fx({ kind: 'fly-hold' });
        const hit = ctx.rng.chance(0.75);
        const k = ctx.rng.range(0.7, 1.4);
        const toward = Math.sign(ctx.fly()?.dx ?? 1) || 1;
        yield fx({ kind: 'tongue', ms: 70 / k, hit, back: false });
        yield hold({ mouth: 'open', look: toward }, 70 / k);
        if (hit) {
            yield fx({ kind: 'fly-eaten' });
            yield fx({ kind: 'tongue', ms: 110 / k, hit: true, back: true });
            yield hold({ mouth: 'open', look: toward }, 110 / k);
            yield hold({ squash: 0.7, mood: 'closed' }, 260);
            yield* emote(ctx.rng.chance(0.5) ? 'heart' : 'note');
            yield hold({ mood: 'happy' }, 700);
        } else {
            yield fx({ kind: 'fly-flee' });
            yield fx({ kind: 'tongue', ms: 110 / k, hit: false, back: true });
            yield hold({ mouth: 'open', look: toward }, 110 / k);
            yield* emote('what');
            yield hold({ mood: 'wide', squash: 0.2 }, 700);
        }
    } finally {
        // Stopped part-way -- picked up, or a capture -- the fly goes on its way.
        yield fx({ kind: 'fly-free' });
    }
}

export function* croak(ctx: ActContext): Act {
    yield* emote('note');
    const n = ctx.rng.int(2, 3);
    for (let i = 0; i < n; i++) {
        yield hold({ throat: 1, mood: 'closed', mouth: 'o' }, ctx.rng.range(260, 420));
        yield hold({ throat: 0, mood: 'closed' }, 180);
    }
}

export function* blep(ctx: ActContext): Act {
    const k = ctx.rng.range(0.8, 1.3);
    yield hold({ mood: 'happy', mouthStyle: 'blep' }, ctx.rng.range(700, 1300) * k);
    if (ctx.rng.chance(0.5)) yield hold({ mouthStyle: 'blep', look: ctx.rng.sign() }, ctx.rng.range(400, 800) * k);
    yield hold({ mood: 'blink', mouthStyle: 'blep' }, 110);
    yield hold({ mood: 'happy' }, 300 * k);
}

export function* wiggle(ctx: ActContext): Act {
    const k = ctx.rng.range(0.75, 1.3);
    const n = ctx.rng.int(4, 8);
    if (ctx.rng.chance(0.6)) yield* emote('note');
    for (let i = 0; i < n; i++) {
        const pose: Pose = { mood: 'happy', look: i % 2 ? 1 : -1, squash: i % 2 ? 0.25 : -0.2, step: i % 2 ? 1 : 3 };
        yield move(0, pose, i % 2 ? 0 : 1);
        yield hold(pose, 150 * k);
    }
    yield move(0, { mood: 'happy', squash: 0.3 }, 0);
    yield hold({ mood: 'happy', squash: 0.3 }, 300);
}

export function* spores(ctx: ActContext): Act {
    const big = ctx.rng.chance(0.2);
    yield hold({ squash: 0.9, mood: 'closed' }, 180);
    yield fx({
        kind: 'burst',
        colours: ['#fff6c9', '#ffe7a3', '#fffdf2'],
        count: big ? 26 : 12,
        up: true,
        at: { x: 16, y: 8 },
    });
    if (big) {
        // Sneezed itself into a hop.
        yield arc({ mood: 'squeeze' }, 300, 4.7);
        yield* emote('bang');
    }
    yield hold({ squash: -0.3, mood: 'happy' }, 600);
}

export function* tip(ctx: ActContext): Act {
    for (const c of [1, 2, 3]) yield hold({ capLift: c, mood: 'happy', arm: 'up' }, 90);
    yield hold({ capLift: 3, mood: 'happy', arm: 'up' }, ctx.rng.range(400, 800));
    for (const c of [2, 1, 0]) yield hold({ capLift: c, mood: 'happy' }, 90);
}

export function* glow(_ctx: ActContext): Act {
    yield* emote('spark');
    for (let i = 0; i < 6; i++) yield hold({ glow: i % 2 === 0 || i > 2, mood: 'happy' }, i < 3 ? 140 : 700);
}

export function* sprout(ctx: ActContext): Act {
    yield hold({ squash: 0.5, mood: 'closed' }, 300);
    yield hold({ sprout: 2, mood: 'happy', squash: -0.3 }, ctx.rng.range(900, 1600));
    yield* emote('spark');
    yield hold({ sprout: 2, mood: 'happy' }, 500);
}

/** The potato rolls away from the nearer edge, and is dizzy after. */
export function* roll(ctx: ActContext): Act {
    const { left, right } = ctx.room();
    const dir = left < right ? 1 : -1;
    const n = ctx.rng.int(4, 8);
    for (let i = 1; i <= n; i++) {
        const quarter = dir > 0 ? i % 4 : (4 - (i % 4)) % 4;
        const d = within(ctx, dir * 9);
        yield move(d, { rot: quarter, mood: 'squeeze' }, 0);
        yield hold({ rot: quarter, mood: 'squeeze' }, ctx.rng.range(110, 170));
    }
    yield hold({ mood: 'wide', squash: 0.6 }, 250);
    for (let i = 0; i < 4; i++) yield hold({ look: i % 2 ? 1 : -1, mood: 'dizzy' }, 140);
    yield hold({ mood: 'happy' }, 400);
}

export function* bask(ctx: ActContext): Act {
    yield hold({ squash: 0.4, mood: 'closed' }, 500);
    yield hold({ squash: 0.4, mood: 'closed', butter: true }, ctx.rng.range(1600, 2600));
    yield* emote('heart');
    yield hold({ squash: 0.4, mood: 'happy', butter: true }, 600);
}

// --- reactions to the capture tool (`spec/14` §8) --------------------------------------

/** A screenshot: squint at the flash, then a sparkle and a little hop. */
export function* squint(ctx: ActContext): Act {
    yield hold({ mood: 'squeeze', squash: 0.6 }, ctx.rng.range(260, 420));
    yield hold({ look: -1, lookY: 1 }, 400);
    yield* emote('spark');
    yield* hop(ctx, ctx.rng.range(3.3, 7.3), { mood: 'happy' });
    yield hold({ mood: 'happy' }, 400);
}

/**
 * A capture flying to its card, which lands `dx` art pixels away across: a squint at the
 * flash, eyes on it all the way down, and a sparkle when it lands.
 */
export function* watch(ctx: ActContext, dx = 0): Act {
    const look = Math.abs(dx) < 8 ? 0 : Math.sign(dx);
    yield hold({ mood: 'squeeze', squash: 0.5 }, ctx.rng.range(140, 240));
    yield hold({ mood: 'wide', lookY: -1 }, ctx.rng.range(100, 160));
    yield hold({ mood: 'wide', look, lookY: -1 }, 180);
    yield hold({ mood: 'wide', look, lookY: 1 }, 260);
    yield* emote('spark');
    yield* hop(ctx, ctx.rng.range(3.3, 5.3), { mood: 'happy', look });
    yield hold({ mood: 'happy', look }, 400);
}

/** How far the frog's tongue reaches for a card, in art pixels. */
export const LICK_REACH = 48;

/**
 * How far across from the frog's mouth a card lands, in art pixels, when its tongue can
 * reach it: beside the frog, level with its mouth, and near. `null` when it cannot. The
 * boxes are logical pixels; `unit` is the pet's art pixel in them.
 */
export function lickReach(
    pet: { x: number; y: number },
    unit: number,
    mouth: { x: number; y: number },
    card: { x: number; y: number; width: number; height: number },
): number | null {
    const x = pet.x + mouth.x * unit;
    const y = pet.y + mouth.y * unit;
    if (unit <= 0 || y < card.y || y > card.y + card.height) return null;
    const across = x < card.x ? card.x - x : x > card.x + card.width ? card.x + card.width - x : 0;
    const art = across / unit;
    return Math.abs(art) >= 2 && Math.abs(art) <= LICK_REACH ? art : null;
}

/**
 * The frog, and a card landing `dx` art pixels across from its mouth: it follows the
 * capture down, and licks the card as it lands.
 */
export function* lick(ctx: ActContext, dx = 12): Act {
    const look = Math.sign(dx) || 1;
    const to = { dx, dy: 0 };
    yield hold({ mood: 'squeeze', squash: 0.5 }, ctx.rng.range(140, 240));
    yield hold({ mood: 'wide', look, lookY: -1 }, 200);
    yield hold({ mood: 'wide', look }, 160);
    const k = ctx.rng.range(0.8, 1.3);
    yield fx({ kind: 'tongue', ms: 60 / k, hit: true, back: false, to });
    yield hold({ mouth: 'open', look }, 60 / k);
    yield hold({ mouth: 'open', look }, ctx.rng.range(90, 180));
    yield fx({ kind: 'tongue', ms: 110 / k, hit: true, back: true, to });
    yield hold({ mouth: 'open', look }, 110 / k);
    yield hold({ squash: 0.6, mood: 'closed' }, 220);
    yield* emote('heart');
    yield hold({ mood: 'happy', look }, 600);
}

/** One tick of the self-timer: a small hop, eyes on the count. */
export function* tick(_ctx: ActContext): Act {
    yield hold({ squash: 0.7, mood: 'wide', lookY: -1 }, 90);
    yield arc({ mood: 'wide', lookY: -1 }, 260, 2.7);
    yield hold({ squash: 0.6 }, 80);
    yield hold({ lookY: -1 }, 300);
}

/**
 * Out of a recorded area: a startle, then off at a run to `dx` art pixels across and `dy`
 * down. Nothing here decides where -- the engine worked out the nearest way out
 * (`crew.ts`), which on the desk can be up or down it.
 */
export function* leave(ctx: ActContext, dx = 0, dy = 0): Act {
    yield* emote('bang');
    yield hold({ mood: 'wide' }, 150);
    yield* travel(ctx, dx, 2.6, dy);
    yield hold({ look: -Math.sign(dx) || 1 }, 600);
}

/** A recording stopped: two hops, arms up, and a note. */
export function* clap(ctx: ActContext): Act {
    for (let i = 0; i < 2; i++) yield* hop(ctx, 4, { mood: 'happy', arm: 'up', arms: 'up' }, 0, 0.9);
    yield* emote('note');
    yield hold({ mood: 'happy' }, 500);
}

/** Text recognition: reading a tiny page. */
export function* read(ctx: ActContext): Act {
    yield hold({}, ctx.rng.range(0, 250));
    yield* emote('text');
    yield hold({ lookY: 1, look: 0 }, 900);
    yield hold({ mood: 'happy' }, 500);
}

/** A scrolling capture: running on the spot while the page goes by. */
export function* run(_ctx: ActContext, ms = 1600): Act {
    const steps = [1, 0, 3, 0] as const;
    for (let t = 0, i = 0; t < ms; t += 110, i++) {
        const pose: Pose = { step: steps[i % 4] ?? 0, squash: -0.2, mood: 'wide', lookY: 1, phase: i % 4 };
        yield move(0, pose, i % 2);
        yield hold(pose, 110);
    }
    yield move(0, {}, 0);
    yield hold({ mood: 'happy' }, 300);
}

/** Copied: a heart. */
export function* love(ctx: ActContext): Act {
    yield hold({}, ctx.rng.range(0, 200));
    yield* emote('heart');
    yield hold({ mood: 'happy', squash: 0.3 }, 700);
}

/** A capture that failed: a question mark and a shrug. */
export function* shrug(ctx: ActContext): Act {
    yield* emote('what');
    yield hold({ mood: 'wide', arm: 'up', arms: 'up', squash: 0.2 }, 500);
    yield hold({ mood: 'open', look: ctx.rng.sign() }, 400);
    yield hold({ mood: 'open' }, 200);
}

/** Every trick by name: the brain draws from these, and reactions ask for them. */
export const ACTS: Record<string, ActFactory> = {
    walk,
    roam,
    climb,
    descend,
    jump,
    bounce,
    look,
    sit,
    heart,
    land,
    wave,
    doze,
    wake,
    camo,
    ink,
    flap,
    fly: catchFly,
    croak,
    blep,
    wiggle,
    spores,
    tip,
    glow,
    sprout,
    roll,
    bask,
    squint,
    watch,
    lick,
    tick,
    leave,
    clap,
    read,
    run,
    love,
    shrug,
};
