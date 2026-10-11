// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Pet packs (`spec/14` §14, D196): `pet.json` and `sheet.png` read into a pet, every refusal
 * a pack can earn, the frame each pose shows, the squash, a pack's own acts, the registry the
 * engine looks pets up in, and the JSON `GetPetPacks` hands the app.
 */

import { afterEach, describe, expect, it } from 'vitest';

import { gridOf, nodeInflate, rgbaPng, sheetOf } from '../../testkit/png.js';
import type { ActContext } from './acts.js';
import { PETS, PET_KINDS, baselineOf, isPetKind, onPetsChanged, petKinds, petSpec, setPackPets } from './art/index.js';
import type { Grid } from './art/painter.js';
import type { Pose } from './art/pose.js';
import { type Temperament, nextAct } from './brain.js';
import { type Pack, type PackFrame, pickFrame, readPack, squashRows } from './pack.js';
import { wirePacks } from './packWire.js';
import { packParts, previewOf } from './preview.js';
import { Rng } from './rng.js';
import { Runner } from './runner.js';

const C = { a: '#112233', b: '#445566', c: '#778899', d: '#aabbcc', e: '#ddeeff', f: '#ff0000' };

/** A 16 × 16 frame: a body, and a mark `tag` wide on its top row to tell frames apart. */
function frame(tag: number, body = 'a'): Grid {
    const rows: string[] = [];
    for (let y = 0; y < 16; y++) {
        if (y === 0) rows.push('f'.repeat(tag + 1).padEnd(16, '.'));
        else if (y < 3) rows.push('................');
        else rows.push(`..${body.repeat(12)}..`);
    }
    return gridOf(rows, C);
}

interface Built {
    json: string;
    png: Uint8Array;
}

/** A pack of `grids`, eight to a row, with `manifest` as its `pet.json`. */
function packOf(manifest: Record<string, unknown>, grids: Grid[] = [frame(0)]): Built {
    const { width, height, rgba } = sheetOf(grids, Math.min(8, grids.length));
    return { json: JSON.stringify(manifest), png: rgbaPng(width, height, rgba) };
}

const BASE = { format: 1, name: 'Testy', species: 'test', size: [16, 16], frames: [{ cell: 0 }] };

function read(manifest: Record<string, unknown>, grids?: Grid[], id = 'testy'): Pack {
    const { json, png } = packOf(manifest, grids);
    const result = readPack(id, json, png, nodeInflate);
    if (!result.ok) throw new Error(result.problem);
    return result.pack;
}

function problem(manifest: unknown, grids?: Grid[], id = 'testy', png?: Uint8Array): string {
    const built = packOf(manifest as Record<string, unknown>, grids);
    const result = readPack(id, typeof manifest === 'string' ? manifest : built.json, png ?? built.png, nodeInflate);
    return result.ok ? 'read' : result.problem;
}

afterEach(() => setPackPets([]));

/** How often the brain draws each trick for `spec`, over `n` draws. */
function pickBrain(spec: Pack['spec'], n: number): Record<string, number> {
    const rng = new Rng(11);
    const temperament: Temperament = { liveliness: 'normal', wander: true, roam: 'floor', reduced: false };
    const counts: Record<string, number> = {};
    for (let i = 0; i < n; i++) {
        const name = nextAct(rng, spec, temperament, null);
        counts[name] = (counts[name] ?? 0) + 1;
    }
    return counts;
}

describe('a pack read', () => {
    it('makes a pet with the format\'s defaults', () => {
        const pack = read(BASE);
        expect(pack.spec).toMatchObject({ kind: 'testy', name: 'Testy', species: 'test', width: 16, height: 16, gait: 'waddle', floats: false, specials: [] });
        expect(pack.frames).toHaveLength(1);
        expect(pack.colours).toBe(2);
        expect(baselineOf(pack.spec)).toBe(16);
        // One picture, whatever the pose.
        for (const pose of [{}, { mood: 'happy' }, { step: 1 }, { arm: 'wave' }, { view: 'top', face: 'left' }] as Pose[])
            expect(pack.spec.draw(pose)).toBe(pack.frames[0]?.grid);
        expect(read({ ...BASE, species: undefined }).spec.species).toBe('pet');
    });

    it('cuts the sheet into cells along each row and then down', () => {
        const grids = [...Array(10).keys()].map(i => frame(i));
        const pack = read({ ...BASE, frames: grids.map((_, i) => ({ cell: i, ...(i > 0 ? { name: `n${i}` } : {}) })) }, grids);
        pack.frames.forEach((f, i) => expect(f.grid.px.slice(0, 16), `cell ${i}`).toEqual(grids[i]?.px.slice(0, 16)));
    });

    it('makes a half-opaque pixel opaque and anything less clear, and counts them', () => {
        const { width, height, rgba } = sheetOf([frame(0)], 1);
        rgba.set([1, 2, 3, 128], (5 * width + 5) * 4);
        rgba.set([1, 2, 3, 127], (5 * width + 6) * 4);
        const result = readPack('testy', JSON.stringify(BASE), rgbaPng(width, height, rgba), nodeInflate);
        expect(result.ok).toBe(true);
        if (!result.ok) return;
        const grid = result.pack.frames[0]?.grid;
        expect(grid?.px[5 * 16 + 5]).toBe('#010203');
        expect(grid?.px[5 * 16 + 6]).toBeNull();
        expect(result.pack.blended).toBe(2);
    });

    it('ignores a field the format does not have', () => {
        expect(read({ ...BASE, sparkle: true, frames: [{ cell: 0, glitter: 3 }] }).spec.name).toBe('Testy');
    });
});

describe('a pack refused', () => {
    const cases: [string, unknown, RegExp, Grid[]?, string?][] = [
        ['a folder name with capitals', BASE, /lower-case/, undefined, 'Testy'],
        ['a folder name too long', BASE, /1 to 32/, undefined, 'a'.repeat(33)],
        ["one of OctoSnap's own", BASE, /octopus is one of OctoSnap's own pets/, undefined, 'octopus'],
        ['pet.json that is not JSON', '{ "format": 1, ', /pet.json is not JSON/],
        ['pet.json that is a list', [], /must be an object/],
        ['no format', { ...BASE, format: undefined }, /has no format/],
        ['a newer format', { ...BASE, format: 2 }, /format 2, made for a newer OctoSnap/],
        ['a format that is a word', { ...BASE, format: '1' }, /format must be 1/],
        ['no name', { ...BASE, name: undefined }, /name must be 1 to 24 characters/],
        ['a blank name', { ...BASE, name: '   ' }, /name must be/],
        ['a name too long', { ...BASE, name: 'x'.repeat(25) }, /name must be/],
        ['a name with a control character', { ...BASE, name: 'Te\nsty' }, /name must be/],
        ['a size that is not two numbers', { ...BASE, size: 16 }, /size must be \[width, height\]/],
        ['a size too small', { ...BASE, size: [4, 16] }, /width must be a whole number from 8 to 64/],
        ['a size too big', { ...BASE, size: [16, 65] }, /height must be a whole number from 8 to 64/],
        ['a gait it does not have', { ...BASE, gait: 'slither' }, /gait must be one of waddle, hop, glide/],
        ['floats that is not true or false', { ...BASE, floats: 'yes' }, /floats must be true or false/],
        ['a sheet not a whole number of cells', { ...BASE, size: [10, 16] }, /16 × 16 pixels, not a whole number of 10 × 16 cells/],
        ['no frames', { ...BASE, frames: [] }, /frames must be a list of 1 to 64/],
        ['a cell past the sheet', { ...BASE, frames: [{ cell: 0 }, { cell: 1 }] }, /frame 2's cell must be a whole number from 0 to 0/],
        ['a face from the side', { ...BASE, frames: [{ cell: 0 }, { cell: 0, face: 'left' }] }, /frame 2 has a face but is not seen from above/],
        ['a mood it does not have', { ...BASE, frames: [{ cell: 0 }, { cell: 0, mood: 'grumpy' }] }, /frame 2's mood must be one of/],
        ['a step that is not 1 or 3', { ...BASE, frames: [{ cell: 0 }, { cell: 0, step: 2 }] }, /step must be 1 or 3/],
        ['a look that is not -1 or 1', { ...BASE, frames: [{ cell: 0 }, { cell: 0, look: 0 }] }, /look must be -1 or 1/],
        ['an empty cell', { ...BASE, size: [16, 16], frames: [{ cell: 0 }, { cell: 1, name: 'x' }] }, /frame 2 shows cell 1, which is empty/, [frame(0), { w: 16, h: 16, px: new Array<string | null>(256).fill(null) }]],
        ['no frame at rest', { ...BASE, frames: [{ cell: 0, mood: 'happy' }] }, /one that says nothing but its cell/],
        ['a trick it cannot have', { ...BASE, tricks: ['camo'] }, /tricks has camo, which is neither one of wave, flap, wiggle, roll, ink nor one of its acts/],
        ['roll on a pet that is not square', { ...BASE, size: [16, 8], frames: [{ cell: 0 }], tricks: ['roll'] }, /size must be square, not 16 × 8/],
        ['an act named as one of the five\'s tricks', { ...BASE, acts: { jump: { steps: [{ ms: 100 }] } } }, /act jump has the name of one of OctoSnap's own tricks/],
        ['an act with a bad name', { ...BASE, acts: { 'Big Spin': { steps: [{ ms: 100 }] } } }, /lower-case letters/],
        ['an act with no steps', { ...BASE, acts: { spin: { steps: [] } } }, /steps must be a list of 1 to 32/],
        ['an act showing a frame no frame is named', { ...BASE, acts: { spin: { steps: [{ frame: 'spin-1', ms: 100 }] } } }, /shows the frame spin-1, which no frame is named/],
        ['a step too long', { ...BASE, acts: { spin: { steps: [{ ms: 5000 }] } } }, /ms must be a whole number from 0 to 4000/],
        ['a step lifted too high', { ...BASE, acts: { spin: { steps: [{ ms: 10, lift: 30 }] } } }, /lift must be a whole number from 0 to 24/],
        ['times the wrong way round', { ...BASE, acts: { spin: { times: [3, 1], steps: [{ ms: 10 }] } } }, /most times must be a whole number from 3 to 10/],
        ['an emote it does not have', { ...BASE, acts: { spin: { steps: [{ emote: 'poop' }] } } }, /emote must be one of/],
        ['too many acts', { ...BASE, acts: Object.fromEntries([...Array(9).keys()].map(i => [`a${i}`, { steps: [{ ms: 1 }] }])) }, /acts has 9 acts, more than 8/],
    ];
    for (const [what, manifest, message, grids, id] of cases) it(what, () => expect(problem(manifest, grids, id)).toMatch(message));

    it('a sheet that is not a PNG', () => {
        expect(problem(BASE, undefined, 'testy', new TextEncoder().encode('not a png'))).toMatch(/sheet.png does not read: it is not a PNG/);
    });

    it('a sheet with more than 255 colours', () => {
        const w = 16;
        const grids: Grid[] = [];
        for (let f = 0; f < 2; f++) {
            const px: string[] = [];
            for (let i = 0; i < w * w; i++) px.push(`#${(f * 256 + i).toString(16).padStart(6, '0')}`);
            grids.push({ w, h: w, px });
        }
        expect(problem({ ...BASE, frames: [{ cell: 0 }] }, grids)).toMatch(/512 colours, more than 255/);
    });
});

describe('the frame a pose shows', () => {
    /** A pack whose frames say these things, each with its own mark. */
    function framesSaying(saying: Record<string, unknown>[]): { frames: readonly PackFrame[]; rest: PackFrame; pick: (pose: Pose) => [number, boolean] } {
        const grids = saying.map((_, i) => frame(i));
        const pack = read({ ...BASE, frames: saying.map((s, i) => ({ cell: i, ...s })) }, grids);
        const rest = pack.frames.find(f => f.view === 'side' && Object.keys(saying[pack.frames.indexOf(f)] ?? {}).length === 0) ?? pack.frames[0];
        if (rest === undefined) throw new Error('no rest');
        return {
            frames: pack.frames,
            rest,
            pick: pose => {
                const { frame: f, mirrored } = pickFrame(pack.frames, rest, pose);
                return [pack.frames.indexOf(f), mirrored];
            },
        };
    }

    it('is the one that says the most, and nothing against it', () => {
        const { pick } = framesSaying([{}, { mood: 'happy' }, { step: 1 }, { mood: 'happy', step: 1 }, { look: -1 }]);
        expect(pick({})).toEqual([0, false]);
        expect(pick({ mood: 'happy' })).toEqual([1, false]);
        expect(pick({ step: 1 })).toEqual([2, false]);
        expect(pick({ mood: 'happy', step: 1 })).toEqual([3, false]);
        expect(pick({ mood: 'happy', step: 3 })).toEqual([1, false]);
        expect(pick({ look: -1 })).toEqual([4, false]);
        expect(pick({ look: 1 })).toEqual([0, false]);
        // A mood outweighs a step and a look.
        expect(pick({ mood: 'happy', look: -1 })).toEqual([1, false]);
    });

    it('takes the first listed between equals', () => {
        const { pick } = framesSaying([{}, { mood: 'happy' }, { mood: 'happy' }]);
        expect(pick({ mood: 'happy' })).toEqual([1, false]);
    });

    it('has moods stand in for one another', () => {
        const closedOnly = framesSaying([{}, { mood: 'closed' }]);
        for (const mood of ['blink', 'sleep', 'half', 'closed'] as const) expect(closedOnly.pick({ mood })[0], mood).toBe(1);
        expect(closedOnly.pick({ mood: 'happy' })[0]).toBe(0);
        const both = framesSaying([{}, { mood: 'blink' }, { mood: 'closed' }, { mood: 'happy' }, { mood: 'wide' }]);
        expect(both.pick({ mood: 'sleep' })[0]).toBe(2);
        expect(both.pick({ mood: 'half' })[0]).toBe(1);
        expect(both.pick({ mood: 'squeeze' })[0]).toBe(3);
        expect(both.pick({ mood: 'dizzy' })[0]).toBe(4);
    });

    it('has arms stand in for one another, and the octopus\'s arms up is an arm up', () => {
        const { pick } = framesSaying([{}, { arm: 'up' }, { arm: 'wave' }]);
        expect(pick({ arm: 'wave2' })[0]).toBe(2);
        expect(pick({ arm: 'wave' })[0]).toBe(2);
        expect(pick({ arm: 'flap' })[0]).toBe(1);
        expect(pick({ arms: 'up' })[0]).toBe(1);
        expect(pick({ arm: 'down' })[0]).toBe(0);
    });

    it('shows a squashed frame at half a squash or more', () => {
        const { pick } = framesSaying([{}, { squash: 1 }, { squash: -1 }]);
        expect(pick({ squash: 0.3 })[0]).toBe(0);
        expect(pick({ squash: 0.5 })[0]).toBe(1);
        expect(pick({ squash: -0.6 })[0]).toBe(2);
    });

    it('from above, with the side frames when it has none', () => {
        const { pick } = framesSaying([{}, { mood: 'happy' }]);
        expect(pick({ view: 'top', face: 'up' })).toEqual([0, false]);
        expect(pick({ view: 'top', face: 'left', mood: 'happy' })).toEqual([1, false]);
    });

    it('from above, the facing it lacks the nearest it has, mirrored across', () => {
        const { pick } = framesSaying([{}, { view: 'top' }, { view: 'top', face: 'right' }]);
        expect(pick({ view: 'top', face: 'down' })).toEqual([1, false]);
        expect(pick({ view: 'top', face: 'right' })).toEqual([2, false]);
        expect(pick({ view: 'top', face: 'left' })).toEqual([2, true]);
        expect(pick({ view: 'top', face: 'up' })).toEqual([1, false]);
        const withLeft = framesSaying([{}, { view: 'top' }, { view: 'top', face: 'left' }]);
        expect(withLeft.pick({ view: 'top', face: 'right' })).toEqual([2, true]);
    });

    it('names a frame only for an act, in the view it is seen in when it can', () => {
        const { pick } = framesSaying([{}, { name: 'spin' }, { view: 'top' }, { view: 'top', name: 'spin' }]);
        expect(pick({})[0]).toBe(0);
        expect(pick({ frame: 'spin' })[0]).toBe(1);
        expect(pick({ frame: 'spin', view: 'top' })[0]).toBe(3);
        const sideOnly = framesSaying([{}, { name: 'spin' }, { view: 'top' }]);
        expect(sideOnly.pick({ frame: 'spin', view: 'top' })[0]).toBe(1);
        expect(sideOnly.pick({ frame: 'nothing' })[0]).toBe(0);
    });

    it('is mirrored and squashed as drawn, and turned for a roll', () => {
        const pack = read({ ...BASE, frames: [{ cell: 0 }, { cell: 1, view: 'top', face: 'right' }] }, [frame(0), frame(5)]);
        const right = pack.frames[1]?.grid;
        const left = pack.spec.draw({ view: 'top', face: 'left' });
        expect(left.px[15]).toBe(right?.px[0]);
        expect(left.px[0]).toBe(right?.px[15]);
        const squashed = pack.spec.draw({ squash: 1 });
        expect(squashed).not.toBe(pack.frames[0]?.grid);
        const rolled = pack.spec.draw({ rot: 1 });
        expect(rolled.px[15]).toBe(pack.frames[0]?.grid.px[0]);
        const stiff = read({ ...BASE, squash: false });
        expect(stiff.spec.draw({ squash: 1 })).toBe(stiff.frames[0]?.grid);
    });
});

describe('the squash', () => {
    const rows = [
        '..........',
        '..........',
        '...aaaa...',
        '..abbbba..',
        '..abccba..',
        '..abccba..',
        '..abccba..',
        '..abddba..',
        '..abbbba..',
        '...e..e...',
    ];
    const grid = gridOf(rows, C);
    const text = (g: Grid) => [...Array(g.h).keys()].map(y => g.px.slice(y * g.w, (y + 1) * g.w).map(c => (c === null ? '.' : Object.entries(C).find(([, v]) => v === c)?.[0])).join(''));

    it('takes out the rows most like the row under them, with the feet where they were', () => {
        expect(text(squashRows(grid, 1))).toEqual(['..........', '..........', '..........', '...aaaa...', '..abbbba..', '..abccba..', '..abccba..', '..abddba..', '..abbbba..', '...e..e...']);
        expect(text(squashRows(grid, 2))).toEqual(['..........', '..........', '..........', '..........', '...aaaa...', '..abbbba..', '..abccba..', '..abddba..', '..abbbba..', '...e..e...']);
    });

    it('stretches by the same rows, as far as there is room above', () => {
        expect(text(squashRows(grid, -1))).toEqual(['..........', '...aaaa...', '..abbbba..', '..abccba..', '..abccba..', '..abccba..', '..abccba..', '..abddba..', '..abbbba..', '...e..e...']);
        // Two clear rows above it: two rows in twice, each six pixels, however many are asked.
        expect(squashRows(grid, -5).px.filter(c => c !== null).length).toBe(grid.px.filter(c => c !== null).length + 2 * 6);
    });

    it('never takes the top two rows or the bottom two, and keeps every colour its own', () => {
        const thin = gridOf(['aaaa', 'bbbb', 'cccc', 'dddd'], C);
        expect(squashRows(thin, 1)).toBe(thin);
        const out = squashRows(grid, 3);
        expect(new Set(out.px.filter(c => c !== null)).size).toBeLessThanOrEqual(new Set(grid.px.filter(c => c !== null)).size);
        expect(out.px.slice(9 * 10)).toEqual(grid.px.slice(9 * 10));
    });

    it('is nothing at no squash', () => {
        expect(squashRows(grid, 0)).toBe(grid);
    });
});

describe("a pack's own acts", () => {
    const pack = () =>
        read(
            {
                ...BASE,
                frames: [{ cell: 0 }, { cell: 1, name: 'spin-1' }, { cell: 2, name: 'spin-2' }],
                tricks: ['spin', 'spin', 'wave'],
                acts: { spin: { times: [2, 4], steps: [{ emote: 'spark' }, { frame: 'spin-1', ms: 100 }, { frame: 'spin-2', ms: 100, lift: 3 }, { mood: 'happy', ms: 200 }] } },
            },
            [frame(0), frame(1), frame(2)],
        );

    function context(spec: Pack['spec'], seed: number): ActContext {
        return {
            rng: new Rng(seed),
            spec,
            room: () => ({ left: 0, right: 0 }),
            desk: () => null,
            toFloor: () => null,
            climb: () => null,
            crowded: () => false,
            fly: () => null,
        };
    }

    it('are its tricks, and the brain draws them as often as they are listed', () => {
        const spec = pack().spec;
        expect(spec.specials).toEqual(['spin', 'spin', 'wave']);
        expect(Object.keys(spec.acts ?? {})).toEqual(['spin']);
        const drawn = pickBrain(spec, 400);
        expect(drawn.spin ?? 0).toBeGreaterThan((drawn.wave ?? 0) * 1.4);
    });

    it('play their steps a number of times between times\' two, at a speed re-rolled each time', () => {
        const spec = pack().spec;
        const factory = spec.acts?.spin;
        if (factory === undefined) throw new Error('no spin');
        const lengths = new Set<number>();
        const counts = new Set<number>();
        for (let seed = 1; seed < 40; seed++) {
            const runner = new Runner(factory(context(spec, seed)), 0);
            let now = 0;
            let sparks = 0;
            let lifted = 0;
            let spins = 0;
            let last = '';
            while (!runner.done) {
                runner.update(now);
                sparks += runner.takeFx().filter(f => f.kind === 'emote' && f.name === 'spark').length;
                if (runner.y > 0) lifted = Math.max(lifted, runner.y);
                if (runner.pose.frame === 'spin-1' && last !== 'spin-1') spins++;
                last = runner.pose.frame ?? '';
                now += 5;
                if (now > 20_000) throw new Error('did not end');
            }
            expect(runner.y).toBe(0);
            expect(lifted).toBe(3);
            expect(sparks).toBe(spins);
            expect(spins).toBeGreaterThanOrEqual(2);
            expect(spins).toBeLessThanOrEqual(4);
            counts.add(spins);
            lengths.add(Math.round(now / spins));
        }
        expect(counts.size).toBe(3);
        expect(lengths.size).toBeGreaterThan(5);
    });

    it('show the frames they name', () => {
        const spec = pack().spec;
        const p = pack();
        expect(spec.draw({ frame: 'spin-2' }).px.slice(0, 16)).toEqual(p.frames[2]?.grid.px.slice(0, 16));
    });
});

describe('the pets there are', () => {
    it('are the five, then the packs\' pets by id', () => {
        const zed = read({ ...BASE, name: 'Zed' }, undefined, 'zed');
        const amy = read({ ...BASE, name: 'Amy' }, undefined, 'amy');
        const changes: string[][] = [];
        const stop = onPetsChanged(() => changes.push(petKinds()));
        setPackPets([zed.spec, amy.spec]);
        expect(petKinds()).toEqual([...PET_KINDS, 'amy', 'zed']);
        expect(isPetKind('amy')).toBe(true);
        expect(petSpec('zed')?.name).toBe('Zed');
        expect(petSpec('octopus')).toBe(PETS.octopus);
        setPackPets([]);
        expect(isPetKind('amy')).toBe(false);
        stop();
        setPackPets([amy.spec]);
        expect(changes).toEqual([[...PET_KINDS, 'amy', 'zed'], [...PET_KINDS]]);
    });

    it('never let a pack take one of the five\'s names', () => {
        const fake = { ...read(BASE).spec, kind: 'octopus' };
        setPackPets([fake]);
        expect(petSpec('octopus')).toBe(PETS.octopus);
        expect(petKinds()).toEqual([...PET_KINDS]);
    });
});

describe('the packs handed to the app', () => {
    it('carry each pet as its preview, every frame its own pixels', () => {
        const pack = read({ ...BASE, frames: [{ cell: 0 }, { cell: 1, mood: 'blink' }], tricks: ['wiggle'] }, [frame(0), frame(1, 'b')]);
        const json = JSON.parse(
            wirePacks({ folder: '/home/u/.local/share/octosnap/pets', pets: [{ pack, from: '/x/testy' }], problems: [{ id: 'bad', from: '/x/bad', problem: 'it has no sheet.png' }] }),
        ) as { folder: string; pets: { id: string; palette: string[]; frames: string[]; loop: [number, number, number][]; baseline: number; width: number; from: string }[]; problems: unknown[] };
        expect(json.folder).toBe('/home/u/.local/share/octosnap/pets');
        expect(json.problems).toEqual([{ id: 'bad', from: '/x/bad', problem: 'it has no sheet.png' }]);
        const [pet] = json.pets;
        if (pet === undefined) throw new Error('no pet');
        expect(pet).toMatchObject({ id: 'testy', from: '/x/testy', width: 16, baseline: 16 });
        // The fields `PackPet` in the app's `pets.rs` reads, whose test reads the same shape.
        expect(Object.keys(pet).sort()).toEqual(['baseline', 'frames', 'from', 'height', 'id', 'loop', 'name', 'palette', 'species', 'width']);
        const preview = previewOf(pack.spec, packParts(pack.spec));
        expect(pet.frames).toHaveLength(preview.frames.length);
        expect(pet.loop).toEqual(preview.loop);
        preview.frames.forEach((grid, i) => {
            const encoded = pet.frames[i] ?? '';
            const px = [...Array(grid.w * grid.h).keys()].map(p => {
                const n = parseInt(encoded.slice(p * 2, p * 2 + 2), 16);
                return n === 0 ? null : (pet.palette[n - 1] ?? 'missing');
            });
            expect(px, `frame ${i}`).toEqual(grid.px);
        });
        // It blinks, and wiggles.
        expect(preview.frames.some(g => g.px[3 * 16 + 2] === C.b)).toBe(true);
    });

    it('leave out of the preview the tricks that go somewhere', () => {
        const spec = read({ ...BASE, size: [16, 16], tricks: ['ink', 'roll', 'wave'] }).spec;
        expect(packParts(spec)).toEqual(['breath', 'blink', 'breath', 'wave', 'breath', 'jump', 'breath', 'blink', 'breath']);
    });
});
