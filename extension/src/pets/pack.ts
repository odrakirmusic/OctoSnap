// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * A pet pack read into a pet (`spec/14` §14, D196): `pet.json` checked, `sheet.png` cut into
 * its frames, and a `PetSpec` whose `draw` shows, for each pose, the frame that says the
 * most about it and nothing against it.
 *
 * A pack is someone's files read inside GNOME Shell, so everything is checked and nothing
 * is trusted: a field of the wrong kind, a number out of its range, a cell past the sheet,
 * an act naming a frame there is not -- each is a problem said in words that say what to
 * change, and the pack is left out. A field the format does not have is ignored. No `gi://`
 * imports: `packs.ts` reads the files and hands them here, and vitest does the same.
 */

import { ACTS, type ActFactory } from './acts.js';
import type { EmoteName } from './art/emotes.js';
import { type Gait, type PetSpec, isBuiltinKind } from './art/index.js';
import { type Grid, mirrorGrid, rotateGrid } from './art/painter.js';
import { type Face, type Mood, type Pose, type View, quantizePose } from './art/pose.js';
import { type Inflate, type Rgba, decodePng } from './png.js';
import { type Act, fx, hold, move } from './runner.js';

/** The format this release reads (`format` in `pet.json`). */
export const PACK_FORMAT = 1;
/** The five's tricks a pack's pet may have: the ones that need nothing drawn only for them. */
export const PACK_TRICKS = ['wave', 'flap', 'wiggle', 'roll', 'ink'] as const;
/** A pack's id, its folder's name. */
export const PACK_ID = /^[a-z0-9][a-z0-9-]{0,31}$/;
/** The most of each thing a pack may have. */
export const PACK_LIMITS = {
    sheetSide: 512,
    sheetBytes: 1024 * 1024,
    jsonBytes: 64 * 1024,
    side: [8, 64],
    frames: 64,
    colours: 255,
    acts: 8,
    steps: 32,
    stepMs: 4000,
    lift: 24,
    times: 10,
    text: 24,
} as const;

const MOODS: readonly Mood[] = ['open', 'blink', 'half', 'closed', 'sleep', 'happy', 'squeeze', 'wide', 'dizzy'];
const ARMS = ['up', 'wave', 'wave2', 'flap'] as const;
const FACES: readonly Face[] = ['down', 'up', 'left', 'right'];
const GAITS: readonly Gait[] = ['waddle', 'hop', 'glide'];
const EMOTES: readonly EmoteName[] = ['heart', 'bang', 'what', 'note', 'zzz', 'spark', 'text', 'star'];
/** The puff a pack's pet vanishes in for `ink`, the crew's own puff. */
const PACK_INK = ['#ffffff', '#deddda', '#c0bfbc'] as const;

type Arm = (typeof ARMS)[number];

/** What a mood the pack has not drawn takes instead, best first. */
const MOOD_FALLBACK: Record<Mood, readonly Mood[]> = {
    open: ['open'],
    blink: ['blink', 'closed'],
    closed: ['closed', 'blink'],
    half: ['half', 'blink', 'closed'],
    sleep: ['sleep', 'closed', 'blink'],
    happy: ['happy'],
    squeeze: ['squeeze', 'happy', 'closed'],
    wide: ['wide'],
    dizzy: ['dizzy', 'wide'],
};

const ARM_FALLBACK: Record<Arm, readonly Arm[]> = {
    up: ['up'],
    wave: ['wave', 'up'],
    wave2: ['wave2', 'wave', 'up'],
    flap: ['flap', 'up'],
};

/** Which facings stand in for one seen from above, and whether each is mirrored. */
const FACE_FALLBACK: Record<Face, readonly [Face, boolean][]> = {
    down: [['down', false]],
    up: [
        ['up', false],
        ['down', false],
    ],
    left: [
        ['left', false],
        ['right', true],
        ['down', false],
    ],
    right: [
        ['right', false],
        ['left', true],
        ['down', false],
    ],
};

/** One frame of a pack: its picture, and what it says about the pose it shows. */
export interface PackFrame {
    grid: Grid;
    view: View;
    /** Seen from above, the way it faces: `down` when it does not say. */
    face: Face;
    mood?: Mood;
    arm?: Arm;
    step?: 1 | 3;
    look?: -1 | 1;
    lookY?: -1 | 1;
    squash?: -1 | 1;
    name?: string;
}

/** A pack read: its pet, and what the reading found worth saying. */
export interface Pack {
    spec: PetSpec;
    frames: readonly PackFrame[];
    /** Pixels neither clear nor opaque, made one or the other. */
    blended: number;
    /** The colours its frames use. */
    colours: number;
}

export type PackResult = { ok: true; pack: Pack } | { ok: false; problem: string };

class Problem extends Error {}

function fail(message: string): never {
    throw new Problem(message);
}

/** A plain object, not an array or `null`. */
function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function int(value: unknown, low: number, high: number, what: string): number {
    if (typeof value !== 'number' || !Number.isInteger(value) || value < low || value > high)
        fail(`${what} must be a whole number from ${low} to ${high}`);
    return value;
}

function text(value: unknown, what: string): string {
    // Control characters would garble a log line or a label.
    if (typeof value !== 'string' || value.trim() === '' || [...value].length > PACK_LIMITS.text || /\p{Cc}/u.test(value))
        fail(`${what} must be 1 to ${PACK_LIMITS.text} characters`);
    return value.trim();
}

function oneOf<T extends string>(value: unknown, allowed: readonly T[], what: string): T {
    if (typeof value !== 'string' || !(allowed as readonly string[]).includes(value))
        fail(`${what} must be one of ${allowed.join(', ')}`);
    return value as T;
}

function sign(value: unknown, what: string): -1 | 1 {
    if (value !== -1 && value !== 1) fail(`${what} must be -1 or 1`);
    return value;
}

/** The pose fields a frame or an act's step may say, read from `raw`; `where` names it in a problem. */
function poseFields(raw: Record<string, unknown>, where: string): Omit<PackFrame, 'grid' | 'view' | 'face' | 'name'> {
    const out: Omit<PackFrame, 'grid' | 'view' | 'face' | 'name'> = {};
    if (raw.mood !== undefined) out.mood = oneOf(raw.mood, MOODS, `${where}'s mood`);
    if (raw.arm !== undefined) out.arm = oneOf(raw.arm, ARMS, `${where}'s arm`);
    if (raw.step !== undefined) {
        if (raw.step !== 1 && raw.step !== 3) fail(`${where}'s step must be 1 or 3`);
        out.step = raw.step;
    }
    if (raw.look !== undefined) out.look = sign(raw.look, `${where}'s look`);
    if (raw.lookY !== undefined) out.lookY = sign(raw.lookY, `${where}'s lookY`);
    if (raw.squash !== undefined) out.squash = sign(raw.squash, `${where}'s squash`);
    return out;
}

/** The sheet's pixels as colours, cut into cells; counts what was neither clear nor opaque. */
function cells(rgba: Uint8Array, sheetWidth: number, w: number, h: number, count: number): { grids: Grid[]; blended: number; colours: number } {
    const across = sheetWidth / w;
    const seen = new Set<string>();
    let blended = 0;
    const hex = (n: number) => n.toString(16).padStart(2, '0');
    const grids: Grid[] = [];
    for (let cell = 0; cell < count; cell++) {
        const cx = (cell % across) * w;
        const cy = Math.floor(cell / across) * h;
        const px: (string | null)[] = new Array<string | null>(w * h).fill(null);
        for (let y = 0; y < h; y++) {
            for (let x = 0; x < w; x++) {
                const o = ((cy + y) * sheetWidth + cx + x) * 4;
                const alpha = rgba[o + 3] ?? 0;
                if (alpha !== 0 && alpha !== 255) blended++;
                // Half opaque or more is opaque: no pixel is a blend of pet and wallpaper.
                if (alpha < 128) continue;
                const colour = `#${hex(rgba[o] ?? 0)}${hex(rgba[o + 1] ?? 0)}${hex(rgba[o + 2] ?? 0)}`;
                seen.add(colour);
                px[y * w + x] = colour;
            }
        }
        grids.push({ w, h, px });
    }
    return { grids, blended, colours: seen.size };
}

/**
 * The pack in folder `id`: `json` is `pet.json`'s text and `png` `sheet.png`'s bytes. The
 * first problem found, when there is one, in place of the pet.
 */
export function readPack(id: string, json: string, png: Uint8Array, inflate: Inflate): PackResult {
    try {
        return { ok: true, pack: read(id, json, png, inflate) };
    } catch (e) {
        if (e instanceof Problem) return { ok: false, problem: e.message };
        throw e;
    }
}

function read(id: string, json: string, png: Uint8Array, inflate: Inflate): Pack {
    if (!PACK_ID.test(id)) fail(`the folder's name must be 1 to 32 lower-case letters, digits and hyphens`);
    if (isBuiltinKind(id)) fail(`${id} is one of OctoSnap's own pets: give the folder another name`);
    if (json.length > PACK_LIMITS.jsonBytes) fail(`pet.json is more than ${PACK_LIMITS.jsonBytes / 1024} KiB`);
    let manifest: unknown;
    try {
        manifest = JSON.parse(json);
    } catch (e) {
        fail(`pet.json is not JSON: ${e instanceof Error ? e.message : String(e)}`);
    }
    if (!isRecord(manifest)) fail('pet.json must be an object');
    const m = manifest;
    if (m.format === undefined) fail('pet.json has no format: say "format": 1');
    if (typeof m.format === 'number' && Number.isInteger(m.format) && m.format > PACK_FORMAT)
        fail(`pet.json is format ${m.format}, made for a newer OctoSnap, which reads format ${PACK_FORMAT}`);
    if (m.format !== PACK_FORMAT) fail(`pet.json's format must be ${PACK_FORMAT}`);

    const name = text(m.name, 'name');
    const species = m.species === undefined ? 'pet' : text(m.species, 'species');
    if (!Array.isArray(m.size) || m.size.length !== 2)
        fail(`size must be [width, height], each ${PACK_LIMITS.side[0]} to ${PACK_LIMITS.side[1]}`);
    const [low, high] = PACK_LIMITS.side;
    const w = int(m.size[0], low, high, 'size\'s width');
    const h = int(m.size[1], low, high, 'size\'s height');
    const gait: Gait = m.gait === undefined ? 'waddle' : oneOf(m.gait, GAITS, 'gait');
    if (m.floats !== undefined && typeof m.floats !== 'boolean') fail('floats must be true or false');
    const floats = m.floats === true;
    if (m.squash !== undefined && typeof m.squash !== 'boolean') fail('squash must be true or false');
    const squashes = m.squash !== false;

    if (png.length > PACK_LIMITS.sheetBytes) fail(`sheet.png is more than ${PACK_LIMITS.sheetBytes / 1024 / 1024} MiB`);
    let sheet: Rgba;
    try {
        sheet = decodePng(png, inflate, PACK_LIMITS.sheetSide);
    } catch (e) {
        fail(`sheet.png does not read: ${e instanceof Error ? e.message : String(e)}`);
    }
    if (sheet.width % w !== 0 || sheet.height % h !== 0)
        fail(`sheet.png is ${sheet.width} × ${sheet.height} pixels, not a whole number of ${w} × ${h} cells`);
    const cellCount = (sheet.width / w) * (sheet.height / h);

    if (!Array.isArray(m.frames) || m.frames.length === 0 || m.frames.length > PACK_LIMITS.frames)
        fail(`frames must be a list of 1 to ${PACK_LIMITS.frames} frames`);
    const { grids, blended, colours } = cells(sheet.rgba, sheet.width, w, h, cellCount);
    if (colours > PACK_LIMITS.colours) fail(`sheet.png has ${colours} colours, more than ${PACK_LIMITS.colours}`);

    const frames: PackFrame[] = m.frames.map((raw: unknown, i: number) => {
        const where = `frame ${i + 1}`;
        if (!isRecord(raw)) fail(`${where} must be an object with a cell`);
        const cell = int(raw.cell, 0, cellCount - 1, `${where}'s cell`);
        const view: View = raw.view === undefined ? 'side' : oneOf(raw.view, ['side', 'top'] as const, `${where}'s view`);
        if (raw.face !== undefined && view !== 'top') fail(`${where} has a face but is not seen from above: add "view": "top"`);
        const face: Face = raw.face === undefined ? 'down' : oneOf(raw.face, FACES, `${where}'s face`);
        const grid = grids[cell];
        if (grid === undefined) fail(`${where}'s cell is past the sheet`);
        const frame: PackFrame = { grid, view, face, ...poseFields(raw, where) };
        if (raw.name !== undefined) frame.name = text(raw.name, `${where}'s name`);
        if (grid.px.every(c => c === null)) fail(`${where} shows cell ${cell}, which is empty`);
        return frame;
    });
    const rest = frames.find(f => f.view === 'side' && says(f) === 0);
    if (rest === undefined) fail('frames must have one that says nothing but its cell: the pet at rest');

    const names = new Set(frames.flatMap(f => (f.name === undefined ? [] : [f.name])));
    const acts = readActs(m.acts, names);
    const tricks = readTricks(m.tricks, acts, w, h);

    const spec: PetSpec = {
        kind: id,
        name,
        species,
        width: w,
        height: h,
        gait,
        floats,
        specials: tricks,
        // Only the frog's tongue reads its mouth, and only the five have a tongue.
        mouth: { x: Math.round(w / 2), y: Math.round(h * 0.6) },
        eyeRow: Math.round(h * 0.4),
        draw: (pose: Pose) => drawPack(frames, rest, squashes, pose),
        acts,
        ink: PACK_INK,
    };
    return { spec, frames, blended, colours };
}

/** How many of the pose's fields a frame says, `name` among them. */
function says(f: PackFrame): number {
    return [f.mood, f.arm, f.step, f.look, f.lookY, f.squash, f.name].filter(v => v !== undefined).length + (f.view === 'top' ? 1 : 0);
}

function readTricks(raw: unknown, acts: Readonly<Record<string, ActFactory>>, w: number, h: number): string[] {
    if (raw === undefined) return [];
    if (!Array.isArray(raw) || raw.length > 16) fail('tricks must be a list of up to 16 names');
    return raw.map((t: unknown) => {
        if (typeof t !== 'string') fail('tricks must be names');
        if (Object.hasOwn(acts, t)) return t;
        if (!(PACK_TRICKS as readonly string[]).includes(t))
            fail(`tricks has ${t}, which is neither one of ${PACK_TRICKS.join(', ')} nor one of its acts`);
        if (t === 'roll' && w !== h) fail(`roll turns the pet a quarter at a time, so its size must be square, not ${w} × ${h}`);
        return t;
    });
}

interface PackStep {
    pose: Pose;
    ms: number;
    lift: number;
    emote?: EmoteName;
}

function readActs(raw: unknown, names: ReadonlySet<string>): Record<string, ActFactory> {
    if (raw === undefined) return {};
    if (!isRecord(raw)) fail('acts must be an object of acts by name');
    const entries = Object.entries(raw);
    if (entries.length > PACK_LIMITS.acts) fail(`acts has ${entries.length} acts, more than ${PACK_LIMITS.acts}`);
    const acts: Record<string, ActFactory> = {};
    for (const [name, def] of entries) {
        if (!/^[a-z0-9][a-z0-9-]{0,23}$/.test(name)) fail(`the act ${name}'s name must be 1 to 24 lower-case letters, digits and hyphens`);
        if (Object.hasOwn(ACTS, name)) fail(`the act ${name} has the name of one of OctoSnap's own tricks: give it another`);
        if (!isRecord(def)) fail(`the act ${name} must be an object with steps`);
        let times: [number, number] = [1, 1];
        if (def.times !== undefined) {
            if (!Array.isArray(def.times) || def.times.length !== 2) fail(`the act ${name}'s times must be [fewest, most]`);
            const fewest = int(def.times[0], 1, PACK_LIMITS.times, `the act ${name}'s fewest times`);
            const most = int(def.times[1], fewest, PACK_LIMITS.times, `the act ${name}'s most times`);
            times = [fewest, most];
        }
        if (!Array.isArray(def.steps) || def.steps.length === 0 || def.steps.length > PACK_LIMITS.steps)
            fail(`the act ${name}'s steps must be a list of 1 to ${PACK_LIMITS.steps} steps`);
        const steps: PackStep[] = def.steps.map((s: unknown, i: number) => {
            const where = `the act ${name}'s step ${i + 1}`;
            if (!isRecord(s)) fail(`${where} must be an object`);
            const pose: Pose = { ...poseFields(s, where) };
            if (s.frame !== undefined) {
                const frame = text(s.frame, `${where}'s frame`);
                if (!names.has(frame)) fail(`${where} shows the frame ${frame}, which no frame is named`);
                pose.frame = frame;
            }
            const step: PackStep = {
                pose,
                ms: s.ms === undefined && s.emote !== undefined ? 0 : int(s.ms, 0, PACK_LIMITS.stepMs, `${where}'s ms`),
                lift: s.lift === undefined ? 0 : int(s.lift, 0, PACK_LIMITS.lift, `${where}'s lift`),
            };
            if (s.emote !== undefined) step.emote = oneOf(s.emote, EMOTES, `${where}'s emote`);
            return step;
        });
        acts[name] = packAct(steps, times);
    }
    return acts;
}

/**
 * An act made of a pack's steps: played between `times`' two times, at 0.7 to 1.4 times
 * its speed, both re-rolled every time (`spec/14` §6).
 */
function packAct(steps: readonly PackStep[], times: readonly [number, number]): ActFactory {
    return function* (ctx): Act {
        const n = ctx.rng.int(times[0], times[1]);
        const k = ctx.rng.range(0.7, 1.4);
        let lifted = false;
        for (let i = 0; i < n; i++) {
            for (const step of steps) {
                if (step.emote !== undefined) yield fx({ kind: 'emote', name: step.emote });
                if (step.ms <= 0) continue;
                yield move(0, step.pose, step.lift);
                lifted = lifted || step.lift > 0;
                yield hold(step.pose, step.ms / k);
            }
        }
        if (lifted) yield move(0, {}, 0);
    };
}

/** A field's worth, most first: a match in an earlier field outweighs any number in later ones. */
const FIELD_ORDER = ['face', 'arm', 'mood', 'squash', 'step', 'look', 'lookY'] as const;

/**
 * How well `frame` fits `pose`, mirrored or not: `-1` when it says something against it,
 * else a score where every field it says counts by its worth and by how near the pose's own
 * value it is -- 3 for the value itself, 2 and 1 for what stands in for it.
 */
function fit(frame: PackFrame, pose: Pose, faces: readonly [Face, boolean][], mirrored: boolean): number {
    const rank = (list: readonly string[], value: string): number => {
        const i = list.indexOf(value);
        return i < 0 ? -1 : 3 - Math.min(2, i);
    };
    const scores: number[] = [];
    if (frame.view === 'top') {
        const i = faces.findIndex(([face, mirror]) => face === frame.face && mirror === mirrored);
        if (i < 0) return -1;
        scores.push(3 - Math.min(2, i));
    } else {
        if (mirrored) return -1;
        scores.push(0);
    }
    const arm = pose.arm ?? (pose.arms === 'up' ? 'up' : undefined);
    if (frame.arm !== undefined) {
        if (arm === undefined || arm === 'down') return -1;
        const r = rank(ARM_FALLBACK[arm], frame.arm);
        if (r < 0) return -1;
        scores.push(r);
    } else scores.push(0);
    if (frame.mood !== undefined) {
        const r = rank(MOOD_FALLBACK[pose.mood ?? 'open'], frame.mood);
        if (r < 0) return -1;
        scores.push(r);
    } else scores.push(0);
    const squash = pose.squash ?? 0;
    if (frame.squash !== undefined) {
        if (Math.abs(squash) < 0.5 || Math.sign(squash) !== frame.squash) return -1;
        scores.push(3);
    } else scores.push(0);
    for (const [have, want] of [
        [frame.step, pose.step ?? 0],
        [frame.look === undefined ? undefined : mirrored ? -frame.look : frame.look, pose.look ?? 0],
        [frame.lookY, pose.lookY ?? 0],
    ] as const) {
        if (have === undefined) scores.push(0);
        else if (have !== want) return -1;
        else scores.push(3);
    }
    return scores.reduce((sum, s, i) => sum + s * 4 ** (FIELD_ORDER.length - 1 - i), 0);
}

/** The frame a pack shows for `pose`, and whether it is mirrored; see the module comment. */
export function pickFrame(frames: readonly PackFrame[], rest: PackFrame, pose: Pose): { frame: PackFrame; mirrored: boolean } {
    const q = quantizePose(pose);
    const top = q.view === 'top' && frames.some(f => f.view === 'top' && f.name === undefined);
    const faces = top ? FACE_FALLBACK[q.face ?? 'down'] : [];
    const view: View = top ? 'top' : 'side';
    // A named frame only when an act asks for it, in this view if there is one there.
    let pool = frames.filter(f => f.name === undefined && f.view === view);
    if (q.frame !== undefined && q.frame !== '') {
        const named = frames.filter(f => f.name === q.frame);
        const here = named.filter(f => f.view === view);
        if (here.length > 0) pool = here;
        else if (named.length > 0) return { frame: named.find(f => f.view === 'side') ?? named[0] ?? rest, mirrored: false };
    }
    let best: { frame: PackFrame; mirrored: boolean } | null = null;
    let bestScore = -1;
    for (const mirrored of top ? [false, true] : [false]) {
        for (const frame of pool) {
            const score = fit(frame, q, faces, mirrored);
            if (score > bestScore) {
                best = { frame, mirrored };
                bestScore = score;
            }
        }
    }
    return best ?? { frame: rest, mirrored: false };
}

/**
 * `grid` squashed by `rows` whole rows of art pixels, or stretched by as many when it is
 * negative, with its feet where they were. The rows taken out, or put in twice, are those
 * most like the row under them, never the pet's top two or bottom two; a stretch puts in no
 * more rows than there are clear ones above the pet. Every pixel stays one of the grid's own.
 */
export function squashRows(grid: Grid, rows: number): Grid {
    if (rows === 0) return grid;
    const { w, h, px } = grid;
    const clear = (y: number) => {
        for (let x = 0; x < w; x++) if (px[y * w + x] !== null) return false;
        return true;
    };
    let top = 0;
    while (top < h && clear(top)) top++;
    let bottom = h - 1;
    while (bottom > top && clear(bottom)) bottom--;
    const lo = top + 2;
    const hi = bottom - 2;
    if (hi < lo) return grid;
    const n = Math.min(Math.abs(rows), hi - lo + 1, rows < 0 ? top : Infinity);
    if (n <= 0) return grid;
    const middle = (lo + hi) / 2;
    const differs = (y: number) => {
        let d = 0;
        for (let x = 0; x < w; x++) if (px[y * w + x] !== px[(y + 1) * w + x]) d++;
        return d;
    };
    const candidates: { y: number; d: number }[] = [];
    for (let y = lo; y <= hi; y++) candidates.push({ y, d: differs(y) });
    candidates.sort((a, b) => a.d - b.d || Math.abs(a.y - middle) - Math.abs(b.y - middle) || a.y - b.y);
    const chosen = new Set(candidates.slice(0, n).map(c => c.y));
    const kept: number[] = [];
    for (let y = top; y <= bottom; y++) {
        if (rows > 0 && chosen.has(y)) continue;
        kept.push(y);
        if (rows < 0 && chosen.has(y)) kept.push(y);
    }
    const out = new Array<string | null>(w * h).fill(null);
    // Bottom-aligned: the pet's last row stays on `bottom`, its feet's row.
    const shift = bottom + 1 - kept.length;
    kept.forEach((from, i) => {
        const to = shift + i;
        for (let x = 0; x < w; x++) out[to * w + x] = px[from * w + x] ?? null;
    });
    for (let y = bottom + 1; y < h; y++) for (let x = 0; x < w; x++) out[y * w + x] = px[y * w + x] ?? null;
    return { w, h, px: out };
}

function drawPack(frames: readonly PackFrame[], rest: PackFrame, squashes: boolean, pose: Pose): Grid {
    const q = quantizePose(pose);
    const { frame, mirrored } = pickFrame(frames, rest, q);
    let grid = mirrored ? mirrorGrid(frame.grid) : frame.grid;
    const squash = q.squash ?? 0;
    if (squashes && frame.squash === undefined && squash !== 0) grid = squashRows(grid, Math.round((squash * grid.h) / 16));
    if (q.rot !== undefined && q.rot !== 0) grid = rotateGrid(grid, q.rot);
    return grid;
}
