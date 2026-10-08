// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The tricks, played on a fake clock. Every trick every pet knows has to finish, keep the
 * pet on the ground once it has, stay inside the room it was given, and come out the same
 * for the same seed -- and a different seed has to make it come out differently, which is
 * the whole point of re-rolling (`spec/14` §6).
 */

import { describe, expect, it } from 'vitest';

import { PETS, PET_KINDS, type PetKind } from './art/index.js';
import { type View, poseKey } from './art/pose.js';
import { ACTS, type ActContext, LICK_REACH, facing, lickReach } from './acts.js';
import { LIVELINESS, type Place, type Temperament, isLiveliness, nextAct, pause, startsTricks } from './brain.js';
import { BAND_CLEARANCE_ART, FLOOR_BAND_ART } from './physics.js';
import { Rng } from './rng.js';
import { type Fx, Runner, arc, fx, hold, move } from './runner.js';

interface Played {
    ms: number;
    dx: number;
    dy: number;
    maxY: number;
    fx: Fx[];
    poses: string[];
}

function context(kind: PetKind, seed: number, room = { left: 200, right: 200 }): ActContext {
    let fly: { dx: number; dy: number } | null = null;
    return {
        rng: new Rng(seed),
        spec: PETS[kind],
        room: () => room,
        desk: () => null,
        toFloor: () => null,
        climb: () => null,
        crowded: () => false,
        fly: () => fly,
        // A test hook: the frog's fly appears where the effect would put it.
        ...{ setFly: (f: typeof fly) => (fly = f) },
    };
}

/** Plays a whole trick at 16 ms a frame; throws if it has not finished in two minutes. */
function play(ctx: ActContext, name: string, arg?: number, arg2?: number): Played {
    const factory = ACTS[name];
    if (factory === undefined) throw new Error(`no trick ${name}`);
    const out: Played = { ms: 0, dx: 0, dy: 0, maxY: 0, fx: [], poses: [] };
    const runner = new Runner(factory(ctx, arg, arg2), 0);
    let now = 0;
    for (; !runner.done && now < 120_000; now += 16) {
        runner.update(now);
        out.dx += runner.takeDx();
        out.dy += runner.takeDy();
        out.maxY = Math.max(out.maxY, runner.y);
        const effects = runner.takeFx();
        out.fx.push(...effects);
        // The fly buzzes above and to the right of the mouth once it has been called.
        if (effects.some(e => e.kind === 'fly')) (ctx as unknown as { setFly(f: unknown): void }).setFly({ dx: 20, dy: -12 });
        const key = poseKey(runner.pose);
        if (out.poses[out.poses.length - 1] !== key) out.poses.push(key);
    }
    if (!runner.done) throw new Error(`${name} did not finish`);
    out.ms = now;
    out.fx.push(...runner.takeFx());
    out.dx += runner.takeDx();
    out.dy += runner.takeDy();
    expect(runner.y).toBe(0);
    return out;
}

/**
 * A desk 300 by 200 art pixels for a pet 32 high, as the engine keeps it (`pet.ts`): its
 * left edge from 0 to 300 and its feet from 32 to the floor at 200, moved up and down the
 * desk only from above, and a `view` the tricks change -- onto the desk only from the
 * floor, and onto the floor at once.
 */
interface Desk {
    x: number;
    y: number;
    view: View;
}

const DESK = { width: 300, floor: 200, top: 32 };

function deskContext(kind: PetKind, seed: number, at: Desk): ActContext {
    return {
        rng: new Rng(seed),
        spec: PETS[kind],
        room: () => ({ left: at.x, right: DESK.width - at.x }),
        desk: () =>
            at.view === 'top'
                ? { left: at.x, right: DESK.width - at.x, up: at.y - DESK.top, down: Math.max(0, DESK.floor - FLOOR_BAND_ART - BAND_CLEARANCE_ART - at.y) }
                : null,
        toFloor: () => (at.view === 'top' ? DESK.floor - at.y : null),
        climb: () => (at.view === 'side' && at.y === DESK.floor ? FLOOR_BAND_ART + 1 : null),
        crowded: () => false,
        fly: () => null,
    };
}

/** Plays a trick on `DESK`, moving `at` as the engine would; returns the views it was seen in, in order. */
function playOnDesk(ctx: ActContext, at: Desk, name: string): { views: View[]; faces: string[] } {
    const runner = new Runner(ACTS[name]!(ctx), 0);
    const views: View[] = [at.view];
    const faces: string[] = [];
    for (let now = 0; !runner.done; now += 16) {
        if (now > 120_000) throw new Error(`${name} did not finish`);
        runner.update(now);
        const dx = runner.takeDx();
        const dy = runner.takeDy();
        at.x = Math.max(0, Math.min(DESK.width, at.x + dx));
        if (at.view === 'top') at.y = Math.max(DESK.top, Math.min(DESK.floor, at.y + dy));
        for (const effect of runner.takeFx()) {
            if (effect.kind !== 'view' || effect.view === at.view) continue;
            if (effect.view === 'top' && at.y !== DESK.floor) continue;
            at.view = effect.view;
            if (at.view === 'side') at.y = DESK.floor;
            views.push(at.view);
        }
        if (runner.pose.face !== undefined && faces[faces.length - 1] !== runner.pose.face) faces.push(runner.pose.face);
    }
    return { views, faces };
}

const COMMON = ['walk', 'jump', 'bounce', 'look', 'sit', 'heart', 'land', 'wave', 'wake', 'squint', 'watch', 'tick', 'leave', 'clap', 'read', 'run', 'love', 'shrug'];

describe('every trick, for every pet', () => {
    for (const kind of PET_KINDS) {
        const names = [...COMMON, ...new Set(PETS[kind].specials)];
        for (const name of names) {
            it(`${kind} ${name} finishes, on the ground, inside its room`, () => {
                const room = { left: 40, right: 60 };
                const result = play(context(kind, 7, room), name, name === 'leave' ? 50 : name === 'land' ? 1 : undefined);
                expect(result.ms).toBeLessThan(20_000);
                expect(result.dx).toBeGreaterThanOrEqual(-room.left - 1e-9);
                expect(result.dx).toBeLessThanOrEqual(room.right + 1e-9);
                // Every pose it showed is one the pet can paint.
                for (const key of result.poses) expect(typeof key).toBe('string');
            });
        }
    }

    for (const kind of PET_KINDS) {
        for (const name of ['roam', 'climb', 'descend', 'leave', 'ink', 'jump', 'roll']) {
            if ((name === 'ink' && kind !== 'octopus') || (name === 'roll' && kind !== 'potato')) continue;
            it(`${kind} ${name} finishes on the desk, inside it`, () => {
                for (let seed = 0; seed < 12; seed++) {
                    const at: Desk = { x: 150, y: 110, view: 'top' };
                    const ctx = deskContext(kind, seed, at);
                    const factory = ACTS[name]!;
                    const runner = new Runner(factory(ctx, name === 'leave' ? -40 : undefined, name === 'leave' ? 30 : undefined), 0);
                    for (let now = 0; !runner.done; now += 16) {
                        if (now > 60_000) throw new Error(`${kind} ${name} did not finish`);
                        runner.update(now);
                        at.x += runner.takeDx();
                        at.y += runner.takeDy();
                        runner.takeFx();
                    }
                    expect(runner.y).toBe(0);
                    expect(at.x).toBeGreaterThanOrEqual(-1e-9);
                    expect(at.x).toBeLessThanOrEqual(DESK.width + 1e-9);
                    expect(at.y).toBeGreaterThanOrEqual(DESK.top - 1e-9);
                    expect(at.y).toBeLessThanOrEqual(DESK.floor + 1e-9);
                }
            });
        }
    }

    it('dozing ends asleep, and stays so with no timer', () => {
        const ctx = context('penguin', 1);
        const runner = new Runner(ACTS.doze!(ctx), 0);
        for (let now = 0; now < 5000 && !runner.done; now += 16) runner.update(now);
        expect(runner.done).toBe(true);
        expect(runner.pose.mood).toBe('sleep');
        expect(runner.nextAt(6000)).toBe(Infinity);
    });
});

describe('re-rolling', () => {
    it('the same seed plays the same trick the same way', () => {
        for (const kind of PET_KINDS) {
            for (const name of ['walk', 'jump', ...PETS[kind].specials]) {
                const a = play(context(kind, 42), name);
                const b = play(context(kind, 42), name);
                expect(b).toEqual(a);
            }
        }
    });

    it('another seed plays it differently', () => {
        const differs = (name: string, kind: PetKind) => {
            const a = play(context(kind, 1), name);
            const b = play(context(kind, 2), name);
            return a.ms !== b.ms || a.dx !== b.dx || a.maxY !== b.maxY || a.poses.join() !== b.poses.join();
        };
        expect(differs('walk', 'penguin')).toBe(true);
        expect(differs('jump', 'frog')).toBe(true);
        expect(differs('camo', 'octopus')).toBe(true);
    });

    it('never walks or inks onto another pet, and stays put when it would have to', () => {
        for (const kind of PET_KINDS) {
            for (let seed = 0; seed < 20; seed++) {
                const crowd = { ...context(kind, seed), crowded: () => true };
                expect(play(crowd, 'walk').dx).toBe(0);
                if (kind === 'octopus') expect(play(crowd, 'ink').dx).toBe(0);
                // Crowded on the right only: it goes left, or nowhere.
                const right = { ...context(kind, seed), crowded: (dx: number) => dx > 0 };
                expect(play(right, 'walk').dx).toBeLessThanOrEqual(0);
            }
        }
    });

    it('the frog usually catches the fly, and sometimes misses', () => {
        let caught = 0;
        for (let seed = 0; seed < 60; seed++) {
            const result = play(context('frog', seed), 'fly');
            if (result.fx.some(e => e.kind === 'fly-eaten')) caught++;
            // It always lets go of the fly at the end, caught or not.
            expect(result.fx[result.fx.length - 1]?.kind).toBe('fly-free');
        }
        expect(caught).toBeGreaterThan(30);
        expect(caught).toBeLessThan(60);
    });
});

describe('the desk', () => {
    it('roams it every way, never into the floor\'s band, facing the way it goes', () => {
        const ways = new Set<string>();
        for (const kind of PET_KINDS) {
            for (let seed = 0; seed < 30; seed++) {
                const at: Desk = { x: 150, y: 110, view: 'top' };
                const { views, faces } = playOnDesk(deskContext(kind, seed, at), at, 'roam');
                expect(views).toEqual(['top']);
                expect(at.y).toBeLessThanOrEqual(DESK.floor - FLOOR_BAND_ART - BAND_CLEARANCE_ART + 1e-9);
                expect(at.y).toBeGreaterThanOrEqual(DESK.top - 1e-9);
                const way = facing(at.x - 150, at.y - 110);
                if (Math.hypot(at.x - 150, at.y - 110) > 1) {
                    expect(faces).toEqual([way]);
                    ways.add(way);
                }
            }
        }
        expect([...ways].sort()).toEqual(['down', 'left', 'right', 'up']);
    });

    it('goes all the way it is sent, across and down, walking or hopping', () => {
        for (const kind of PET_KINDS) {
            const result = play(context(kind, 4), 'leave', -30, 40);
            expect(result.dx).toBeCloseTo(-30, 6);
            expect(result.dy).toBeCloseTo(40, 6);
        }
        // Along the floor, only across.
        expect(play(context('penguin', 4), 'walk').dy).toBe(0);
    });

    it('is climbed onto from the floor, seen from above from the crouch on, clear of the band', () => {
        for (const kind of PET_KINDS) {
            for (let seed = 0; seed < 20; seed++) {
                const at: Desk = { x: 150, y: DESK.floor, view: 'side' };
                const { views } = playOnDesk(deskContext(kind, seed, at), at, 'climb');
                expect(views).toEqual(['side', 'top']);
                // Up clear of the band, and wandering on across the desk no nearer the floor than its edge.
                expect(at.y).toBeLessThanOrEqual(DESK.floor - FLOOR_BAND_ART - BAND_CLEARANCE_ART + 1e-9);
            }
        }
    });

    it('is not climbed onto from a pin, or with the pets kept to the bottom: a walk instead', () => {
        const at: Desk = { x: 150, y: 120, view: 'side' };
        const { views } = playOnDesk(deskContext('frog', 3, at), at, 'climb');
        expect(views).toEqual(['side']);
        expect(at.y).toBe(120);
    });

    it('is come down from onto the floor, seen from the side as it lands there', () => {
        for (const kind of PET_KINDS) {
            for (let seed = 0; seed < 20; seed++) {
                const at: Desk = { x: 150, y: 60 + seed * 6, view: 'top' };
                const { views, faces } = playOnDesk(deskContext(kind, seed, at), at, 'descend');
                expect(views).toEqual(['top', 'side']);
                expect(at.y).toBe(DESK.floor);
                expect(faces[faces.length - 1]).toBe('down');
            }
        }
    });

    it('faces the way the most of its going is', () => {
        expect(facing(10, 3)).toBe('right');
        expect(facing(-10, 3)).toBe('left');
        expect(facing(3, 10)).toBe('down');
        expect(facing(3, -10)).toBe('up');
        expect(facing(0, 0)).toBe('right');
    });
});

describe('a capture flying to its card', () => {
    // A frog 3 logical pixels to the art pixel, standing with its sprite's corner at 100, 400.
    const frog = { x: 100, y: 400 };
    const mouth = PETS.frog.mouth;
    const mouthX = frog.x + mouth.x * 3;
    const mouthY = frog.y + mouth.y * 3;
    const card = (x: number, y = mouthY - 50) => ({ x, y, width: 120, height: 80 });

    it('is within the frog\'s reach beside it, level with its mouth, and near', () => {
        expect(lickReach(frog, 3, mouth, card(mouthX + 30))).toBe(10);
        expect(lickReach(frog, 3, mouth, card(mouthX - 30 - 120))).toBe(-10);
        expect(lickReach(frog, 3, mouth, card(mouthX + LICK_REACH * 3))).toBe(LICK_REACH);
    });

    it('is not, far off, above or below its mouth, or right on top of it', () => {
        expect(lickReach(frog, 3, mouth, card(mouthX + LICK_REACH * 3 + 3))).toBeNull();
        expect(lickReach(frog, 3, mouth, card(mouthX + 30, mouthY + 1))).toBeNull();
        expect(lickReach(frog, 3, mouth, card(mouthX + 30, mouthY - 81))).toBeNull();
        expect(lickReach(frog, 3, mouth, card(mouthX - 60))).toBeNull();
    });

    it('the pets follow it with their eyes, towards where it lands', () => {
        const right = play(context('penguin', 3), 'watch', 80);
        const left = play(context('penguin', 3), 'watch', -80);
        const looks = (poses: string[], look: string) => poses.some(key => key.split('|').includes(look));
        expect(looks(right.poses, 'look=1')).toBe(true);
        expect(looks(left.poses, 'look=-1')).toBe(true);
        expect(right.fx).toContainEqual({ kind: 'emote', name: 'spark' });
    });

    it('the frog licks the card where it lands, and gets its tongue back', () => {
        const result = play(context('frog', 5), 'lick', -20);
        const tongues = result.fx.filter(e => e.kind === 'tongue');
        expect(tongues.map(e => e.kind === 'tongue' && [e.back, e.to])).toEqual([
            [false, { dx: -20, dy: 0 }],
            [true, { dx: -20, dy: 0 }],
        ]);
        expect(result.ms).toBeLessThan(3000);
    });
});

describe('the brain', () => {
    const normal: Temperament = { liveliness: 'normal', wander: true, roam: 'floor', reduced: false };
    const anywhere: Temperament = { ...normal, roam: 'anywhere' };

    const drawn = (temperament: Temperament, where: Place, n = 600) => {
        const rng = new Rng(21);
        const seen = new Map<string, number>();
        let last: string | null = null;
        for (let i = 0; i < n; i++) {
            last = nextAct(rng, PETS.penguin, temperament, last, where);
            seen.set(last, (seen.get(last) ?? 0) + 1);
        }
        return seen;
    };

    it('keeps the pets to the floor and the pins unless they may be anywhere', () => {
        for (const where of ['floor', 'pin'] as const) {
            const seen = drawn(normal, where);
            for (const name of ['climb', 'roam', 'descend']) expect(seen.has(name)).toBe(false);
            expect(seen.has('walk')).toBe(true);
        }
    });

    it('with the pets anywhere, climbs from the floor now and then, and never from a pin', () => {
        const floor = drawn(anywhere, 'floor');
        expect(floor.get('climb') ?? 0).toBeGreaterThan(20);
        expect(floor.get('walk') ?? 0).toBeGreaterThan(floor.get('climb') ?? 0);
        expect(floor.has('roam') || floor.has('descend')).toBe(false);
        const pin = drawn(anywhere, 'pin');
        expect(pin.has('climb')).toBe(false);
        expect(pin.has('walk')).toBe(true);
    });

    it('roams the desk, comes down from it less often than it climbs up, and never walks it', () => {
        const desk = drawn(anywhere, 'desk');
        expect(desk.get('roam') ?? 0).toBeGreaterThan(desk.get('descend') ?? 0);
        expect(desk.get('descend') ?? 0).toBeGreaterThan(10);
        expect(desk.get('descend') ?? 0).toBeLessThan(drawn(anywhere, 'floor').get('climb') ?? 0);
        expect(desk.has('walk') || desk.has('climb')).toBe(false);
    });

    it('with moving on their own off, goes nowhere by itself on the desk either', () => {
        for (const where of ['floor', 'pin', 'desk'] as const) {
            const seen = drawn({ ...anywhere, wander: false }, where);
            for (const name of ['walk', 'roam', 'climb', 'descend', 'jump', 'ink', 'roll']) expect(seen.has(name)).toBe(false);
        }
    });

    it('never picks the same trick twice in a row', () => {
        for (const kind of PET_KINDS) {
            const rng = new Rng(3);
            let last: string | null = null;
            for (let i = 0; i < 400; i++) {
                const name = nextAct(rng, PETS[kind], normal, last);
                expect(name).not.toBe(last);
                expect(ACTS[name] ?? PETS[kind].specials.includes(name)).toBeTruthy();
                last = name;
            }
        }
    });

    it('draws every kind of trick over a while', () => {
        const rng = new Rng(9);
        const seen = new Set<string>();
        let last: string | null = null;
        for (let i = 0; i < 500; i++) {
            last = nextAct(rng, PETS.frog, normal, last);
            seen.add(last);
        }
        for (const name of ['walk', 'jump', 'look', 'sit', 'fly', 'croak', 'blep', 'wiggle']) expect(seen).toContain(name);
    });

    it('keeps a pet at home with wandering off', () => {
        const rng = new Rng(5);
        let last: string | null = null;
        for (let i = 0; i < 400; i++) {
            last = nextAct(rng, PETS.octopus, { ...normal, wander: false }, last);
            expect(['walk', 'jump', 'ink']).not.toContain(last);
        }
    });

    it('only looks about under reduced motion, and waits longer', () => {
        const rng = new Rng(5);
        for (let i = 0; i < 50; i++) expect(nextAct(rng, PETS.potato, { ...normal, reduced: true }, null)).toBe('look');
        const calm = pause(new Rng(1), { ...normal, reduced: true });
        expect(calm).toBeGreaterThanOrEqual(2500);
    });

    it('waits longer when calm and less when lively', () => {
        const mean = (liveliness: Temperament['liveliness']) => {
            const rng = new Rng(11);
            let sum = 0;
            for (let i = 0; i < 200; i++) sum += pause(rng, { ...normal, liveliness });
            return sum / 200;
        };
        expect(mean('zen')).toBeGreaterThan(mean('calm') * 2.5);
        expect(mean('calm')).toBeGreaterThan(mean('normal'));
        expect(mean('normal')).toBeGreaterThan(mean('lively'));
    });

    it('starts nothing by itself when Silent, whatever else is set', () => {
        for (const temperament of [normal, anywhere, { ...anywhere, wander: false }, { ...normal, reduced: true }]) {
            const silent: Temperament = { ...temperament, liveliness: 'silent' };
            expect(startsTricks(silent)).toBe(false);
            expect(pause(new Rng(4), silent)).toBe(Infinity);
        }
        for (const liveliness of LIVELINESS.filter(l => l !== 'silent')) expect(startsTricks({ ...normal, liveliness })).toBe(true);
    });

    it('when Zen, does a trick of its own far more rarely than when Calm, and sits more', () => {
        const share = (liveliness: Temperament['liveliness'], names: readonly string[]) => {
            const rng = new Rng(17);
            let last: string | null = null;
            let hits = 0;
            for (let i = 0; i < 2000; i++) {
                last = nextAct(rng, PETS.octopus, { ...normal, liveliness }, last);
                if (names.includes(last)) hits++;
            }
            return hits / 2000;
        };
        const own = PETS.octopus.specials;
        expect(share('zen', own)).toBeLessThan(share('calm', own) / 2.5);
        expect(share('zen', ['sit'])).toBeGreaterThan(share('calm', ['sit']) * 1.5);
        expect(share('zen', own)).toBeGreaterThan(0);
    });

    it('knows every liveliness by name, and nothing else', () => {
        expect([...LIVELINESS]).toEqual(['silent', 'zen', 'calm', 'normal', 'lively']);
        expect(LIVELINESS.every(isLiveliness)).toBe(true);
        expect(['Silent', 'toString', '', 'busy'].some(isLiveliness)).toBe(false);
    });
});

describe('the runner', () => {
    it('holds each pose for as long as asked', () => {
        function* act() {
            yield hold({ mood: 'happy' }, 100);
            yield hold({ mood: 'wide' }, 50);
        }
        const runner = new Runner(act(), 1000);
        expect(runner.pose.mood).toBe('happy');
        expect(runner.nextAt(1000)).toBe(1100);
        runner.update(1099);
        expect(runner.pose.mood).toBe('happy');
        runner.update(1100);
        expect(runner.pose.mood).toBe('wide');
        runner.update(1150);
        expect(runner.done).toBe(true);
    });

    it('hops in a parabola, and travels its distance exactly', () => {
        function* act() {
            yield arc({}, 400, 10, 8, -6);
        }
        const runner = new Runner(act(), 0);
        let dx = 0;
        let dy = 0;
        runner.update(200);
        dx += runner.takeDx();
        dy += runner.takeDy();
        expect(runner.y).toBeCloseTo(10, 6);
        expect(dx).toBeCloseTo(4, 6);
        expect(dy).toBeCloseTo(-3, 6);
        // Redrawn at 30 fps while in the air.
        expect(runner.nextAt(200)).toBe(233);
        runner.update(400);
        dx += runner.takeDx();
        dy += runner.takeDy();
        expect(runner.y).toBe(0);
        expect(dx).toBeCloseTo(8, 9);
        expect(dy).toBeCloseTo(-6, 9);
        expect(runner.done).toBe(true);
    });

    it('does instant steps at once, in order', () => {
        function* act() {
            yield move(2, { step: 1 }, 1);
            yield fx({ kind: 'emote', name: 'heart' });
            yield hold({}, 10);
        }
        const runner = new Runner(act(), 0);
        expect(runner.takeDx()).toBe(2);
        expect(runner.takeDy()).toBe(0);
        expect(runner.takeFx()).toEqual([{ kind: 'emote', name: 'heart' }]);
        expect(runner.y).toBe(1);
    });

    it('does not race through a trick after a stall', () => {
        function* act() {
            for (let i = 0; i < 10; i++) yield hold({ look: i % 2 }, 100);
        }
        const runner = new Runner(act(), 0);
        // A second's stall: the rest of the trick restarts from now, not all at once.
        runner.update(1000);
        expect(runner.done).toBe(false);
        expect(runner.nextAt(1000)).toBe(1100);
    });

    it('ends an update at a change of view, and moves the pet in the next', () => {
        // `climb`'s shape: a crouch, the view, a moment, and the hop up. An update late
        // enough to be in the hop runs only to the view, or the hop's first rise would be
        // taken in the view before it, where a pet moves only across.
        function* act() {
            yield hold({}, 100);
            yield fx({ kind: 'view', view: 'top', face: 'up' });
            yield hold({}, 20);
            yield arc({}, 200, 5, 0, -10);
        }
        const runner = new Runner(act(), 0);
        runner.update(200);
        expect(runner.takeFx()).toEqual([{ kind: 'view', view: 'top', face: 'up' }]);
        expect(runner.takeDy()).toBe(0);
        runner.update(200);
        expect(runner.takeFx()).toEqual([]);
        expect(runner.takeDy()).toBeCloseTo(-4, 9);
        expect(runner.nextAt(200)).toBe(233);
    });

    it('lets a trick tidy up when it is stopped', () => {
        function* act() {
            try {
                yield hold({}, 1000);
            } finally {
                yield fx({ kind: 'fly-free' });
            }
        }
        const runner = new Runner(act(), 0);
        runner.stop();
        expect(runner.done).toBe(true);
        expect(runner.takeFx()).toEqual([{ kind: 'fly-free' }]);
    });

    it('refuses a trick that never waits', () => {
        function* act() {
            for (;;) yield move(0);
        }
        expect(() => new Runner(act(), 0)).toThrow();
    });
});
