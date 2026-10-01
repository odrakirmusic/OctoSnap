// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The pets' sheets in `data/pets/`, made from the same code the shell draws with (D142),
 * and the test that they still are.
 *
 * - `<pet>.png` is a strip of every pose the pet's preview shows, one pixel to the art
 *   pixel, and `pets.json` says how to play them: which pose, for how long, and how high
 *   off the ground. Settings' Pets page and the welcome window play them (`pets.rs` in
 *   the app), scaled up with no smoothing.
 * - `<pet>.svg` is its resting pose, and `cast.svg` all five in a row, for the guide, the
 *   README and the store: sharp at any size.
 *
 * The previews are the pets' own tricks, played through the shell's `Runner` with a fixed
 * seed and no room to move, so a preview is what the pet does on the desktop.
 *
 * `npx vitest run src/pets/sheets.test.ts` checks the files; with `UPDATE_PET_SHEETS=1`
 * it writes them. The check compares pixels rather than PNG bytes, since two zlib builds
 * may compress the same pixels differently.
 */

import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { crc32, deflateSync, inflateSync } from 'node:zlib';

import { describe, expect, it } from 'vitest';

import { ACTS, type ActContext } from './acts.js';
import { PETS, PET_KINDS, type PetKind, baselineOf } from './art/index.js';
import type { Grid } from './art/painter.js';
import { type Pose, poseKey, quantizePose } from './art/pose.js';
import { BLINK_MS, BREATH_MS } from './brain.js';
import { parseColour } from './frames.js';
import { Rng } from './rng.js';
import { Runner } from './runner.js';

const OUT = join(dirname(fileURLToPath(import.meta.url)), '../../../data/pets');
const UPDATE = process.env.UPDATE_PET_SHEETS === '1';

/**
 * Each pet's preview: breaths, blinks, a jump and its own tricks, about fifteen seconds
 * before it starts again. Tricks that are mostly an effect beside the pet -- the frog's
 * fly, the octopus's ink, the mushroom's spores -- are left for the desktop.
 */
const LOOPS: Record<PetKind, string[]> = {
    octopus: ['breath', 'blink', 'breath', 'camo', 'breath', 'jump', 'breath', 'blink', 'wave', 'breath'],
    penguin: ['breath', 'blink', 'breath', 'flap', 'breath', 'jump', 'breath', 'blink', 'wave', 'breath'],
    frog: ['breath', 'blink', 'croak', 'breath', 'blep', 'breath', 'jump', 'blink', 'wiggle', 'breath'],
    mushroom: ['breath', 'blink', 'tip', 'breath', 'glow', 'breath', 'jump', 'blink', 'breath'],
    potato: ['breath', 'blink', 'sprout', 'breath', 'bask', 'breath', 'jump', 'blink', 'roll', 'breath'],
};

/** A step of a preview: a pose, how long it shows, and how many art pixels off the ground. */
type Step = [Pose, number, number];

/** A trick played to the end on a 16 ms clock, as poses held for so long at so high. */
function played(kind: PetKind, name: string, seed: number): Step[] {
    const factory = ACTS[name];
    if (factory === undefined) throw new Error(`no trick ${name}`);
    const ctx: ActContext = {
        rng: new Rng(seed),
        spec: PETS[kind],
        room: () => ({ left: 0, right: 0 }),
        desk: () => null,
        toFloor: () => null,
        climb: () => null,
        crowded: () => false,
        fly: () => null,
    };
    const runner = new Runner(factory(ctx), 0);
    const steps: Step[] = [];
    for (let now = 0; !runner.done; now += 16) {
        if (now > 60_000) throw new Error(`${kind} ${name} did not finish`);
        runner.update(now);
        runner.takeDx();
        runner.takeFx();
        const pose = quantizePose(runner.pose);
        const y = Math.round(runner.y);
        const last = steps[steps.length - 1];
        if (last !== undefined && poseKey(last[0]) === poseKey(pose) && last[2] === y) last[1] += 16;
        else steps.push([pose, 16, y]);
    }
    return steps;
}

function preview(kind: PetKind): Step[] {
    const quarter = BREATH_MS / 4;
    const steps: Step[] = [];
    LOOPS[kind].forEach((part, i) => {
        if (part === 'breath') steps.push([{}, quarter, 0], [{ squash: 0.1 }, quarter, 0], [{}, quarter, 0], [{ squash: -0.1 }, quarter, 0]);
        else if (part === 'blink') steps.push([{ mood: 'blink' }, BLINK_MS, 0], [{}, quarter, 0]);
        else steps.push(...played(kind, part, 7 + i));
    });
    return steps;
}

interface Sheet {
    kind: PetKind;
    frames: Grid[];
    loop: [number, number, number][];
}

function sheetOf(kind: PetKind): Sheet {
    const spec = PETS[kind];
    const keys: string[] = [];
    const frames: Grid[] = [];
    const loop: [number, number, number][] = [];
    for (const [pose, ms, y] of preview(kind)) {
        const key = poseKey(pose);
        let index = keys.indexOf(key);
        if (index < 0) {
            index = keys.length;
            keys.push(key);
            frames.push(spec.draw(pose));
        }
        loop.push([index, ms, y]);
    }
    return { kind, frames, loop };
}

/** The strip's pixels, straight alpha, frames side by side. */
function stripPixels(sheet: Sheet): { width: number; height: number; rgba: Uint8Array } {
    const { width: w, height: h } = PETS[sheet.kind];
    const width = w * sheet.frames.length;
    const rgba = new Uint8Array(width * h * 4);
    sheet.frames.forEach((grid, f) => {
        for (let y = 0; y < h; y++) {
            for (let x = 0; x < w; x++) {
                const colour = grid.px[y * grid.w + x];
                if (colour === null || colour === undefined) continue;
                const o = (y * width + f * w + x) * 4;
                rgba.set(parseColour(colour), o);
            }
        }
    });
    return { width, height: h, rgba };
}

function chunk(type: string, data: Buffer): Buffer {
    const head = Buffer.alloc(8);
    head.writeUInt32BE(data.length, 0);
    head.write(type, 4, 'ascii');
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), data])) >>> 0, 0);
    return Buffer.concat([head, data, crc]);
}

/** An RGBA PNG with no filtering: the smallest encoder that is exact. */
function encodePng(width: number, height: number, rgba: Uint8Array): Buffer {
    const raw = Buffer.alloc((width * 4 + 1) * height);
    for (let y = 0; y < height; y++) Buffer.from(rgba.buffer, rgba.byteOffset + y * width * 4, width * 4).copy(raw, y * (width * 4 + 1) + 1);
    const header = Buffer.alloc(13);
    header.writeUInt32BE(width, 0);
    header.writeUInt32BE(height, 4);
    header.set([8, 6, 0, 0, 0], 8);
    return Buffer.concat([
        Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
        chunk('IHDR', header),
        chunk('IDAT', deflateSync(raw, { level: 9 })),
        chunk('IEND', Buffer.alloc(0)),
    ]);
}

/** The pixels of a PNG this file wrote: RGBA, eight bits, no filtering. */
function decodePng(png: Buffer): { width: number; height: number; rgba: Uint8Array } {
    let at = 8;
    let width = 0;
    let height = 0;
    const idat: Buffer[] = [];
    while (at < png.length) {
        const length = png.readUInt32BE(at);
        const type = png.toString('ascii', at + 4, at + 8);
        const data = png.subarray(at + 8, at + 8 + length);
        if (type === 'IHDR') {
            width = data.readUInt32BE(0);
            height = data.readUInt32BE(4);
            if (data[8] !== 8 || data[9] !== 6) throw new Error('not an 8-bit RGBA PNG');
        } else if (type === 'IDAT') {
            idat.push(data);
        }
        at += 12 + length;
    }
    const raw = inflateSync(Buffer.concat(idat));
    const rgba = new Uint8Array(width * height * 4);
    for (let y = 0; y < height; y++) {
        const row = y * (width * 4 + 1);
        if (raw[row] !== 0) throw new Error('the sheet was re-encoded with filters; write it again with UPDATE_PET_SHEETS=1');
        rgba.set(raw.subarray(row + 1, row + 1 + width * 4), y * width * 4);
    }
    return { width, height, rgba };
}

/** A grid as rectangles, one per run of a colour along a row. */
function rects(grid: Grid, dx: number, dy: number): string[] {
    const out: string[] = [];
    for (let y = 0; y < grid.h; y++) {
        let x = 0;
        while (x < grid.w) {
            const colour = grid.px[y * grid.w + x];
            let end = x + 1;
            while (end < grid.w && grid.px[y * grid.w + end] === colour) end++;
            if (colour !== null && colour !== undefined) {
                const [r, g, b, a] = parseColour(colour);
                const hex = `#${[r, g, b].map(v => v.toString(16).padStart(2, '0')).join('')}`;
                const alpha = a === 255 ? '' : ` fill-opacity="${(a / 255).toFixed(3)}"`;
                out.push(`<rect x="${x + dx}" y="${y + dy}" width="${end - x}" height="1" fill="${hex}"${alpha}/>`);
            }
            x = end;
        }
    }
    return out;
}

function svg(width: number, height: number, title: string, body: string[]): string {
    return [
        `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} ${height}" width="${width * 8}" height="${height * 8}" shape-rendering="crispEdges">`,
        `<title>${title}</title>`,
        ...body,
        '</svg>',
        '',
    ].join('\n');
}

const TITLES: Record<PetKind, string> = {
    octopus: 'Omni the octopus',
    penguin: 'Pengu the penguin',
    frog: 'Hops the frog',
    mushroom: 'Morel the mushroom',
    potato: 'Spud the potato',
};

/** All five resting, feet on one line, two art pixels apart. */
function castSvg(): string {
    const gap = 2;
    const floor = Math.max(...PET_KINDS.map(k => baselineOf(k)));
    const width = PET_KINDS.reduce((sum, k) => sum + PETS[k].width, 0) + gap * (PET_KINDS.length - 1);
    const height = Math.max(...PET_KINDS.map(k => PETS[k].height));
    const body: string[] = [];
    let x = 0;
    for (const kind of PET_KINDS) {
        body.push(`<g aria-label="${TITLES[kind]}">`, ...rects(PETS[kind].draw({}), x, floor - baselineOf(kind)), '</g>');
        x += PETS[kind].width + gap;
    }
    return svg(width, height, 'The OctoSnap pets', body);
}

function manifest(sheets: Sheet[]): string {
    const pets: Record<string, unknown> = {};
    for (const sheet of sheets) {
        const spec = PETS[sheet.kind];
        pets[sheet.kind] = {
            name: spec.name,
            width: spec.width,
            height: spec.height,
            baseline: baselineOf(sheet.kind),
            frames: sheet.frames.length,
            loop: sheet.loop,
        };
    }
    // One step to a line: readable in a diff, and still JSON.
    return `${JSON.stringify(pets, null, 2).replace(/\[\s+(\d+),\s+(\d+),\s+(-?\d+)\s+\]/g, '[$1, $2, $3]')}\n`;
}

describe('the pet sheets', () => {
    const sheets = PET_KINDS.map(sheetOf);

    if (UPDATE) {
        it('are written', () => {
            mkdirSync(OUT, { recursive: true });
            for (const sheet of sheets) {
                const { width, height, rgba } = stripPixels(sheet);
                writeFileSync(join(OUT, `${sheet.kind}.png`), encodePng(width, height, rgba));
                writeFileSync(join(OUT, `${sheet.kind}.svg`), svg(PETS[sheet.kind].width, PETS[sheet.kind].height, TITLES[sheet.kind], rects(PETS[sheet.kind].draw({}), 0, 0)));
            }
            writeFileSync(join(OUT, 'cast.svg'), castSvg());
            writeFileSync(join(OUT, 'pets.json'), manifest(sheets));
        });
        return;
    }

    it('have a strip for every pet with the pixels the shell would draw', () => {
        for (const sheet of sheets) {
            const path = join(OUT, `${sheet.kind}.png`);
            expect(existsSync(path), `${path}: write the sheets with UPDATE_PET_SHEETS=1`).toBe(true);
            const want = stripPixels(sheet);
            const got = decodePng(readFileSync(path));
            expect([got.width, got.height], sheet.kind).toEqual([want.width, want.height]);
            expect(Buffer.from(got.rgba).equals(Buffer.from(want.rgba)), `${sheet.kind}.png is out of date`).toBe(true);
        }
    });

    it('say how to play them, and the SVGs are the resting poses', () => {
        expect(readFileSync(join(OUT, 'pets.json'), 'utf8')).toBe(manifest(sheets));
        expect(readFileSync(join(OUT, 'cast.svg'), 'utf8')).toBe(castSvg());
        for (const kind of PET_KINDS)
            expect(readFileSync(join(OUT, `${kind}.svg`), 'utf8')).toBe(svg(PETS[kind].width, PETS[kind].height, TITLES[kind], rects(PETS[kind].draw({}), 0, 0)));
    });

    it('play for a while, and stay small', () => {
        for (const sheet of sheets) {
            const ms = sheet.loop.reduce((sum, [, t]) => sum + t, 0);
            expect(ms, sheet.kind).toBeGreaterThan(10_000);
            expect(ms, sheet.kind).toBeLessThan(40_000);
            // A preview that needed more than this would be a texture Settings should not hold.
            expect(sheet.frames.length, sheet.kind).toBeLessThan(80);
            for (const [frame, t, y] of sheet.loop) {
                expect(frame).toBeLessThan(sheet.frames.length);
                expect(t).toBeGreaterThan(0);
                expect(y).toBeGreaterThanOrEqual(0);
            }
        }
    });
});
