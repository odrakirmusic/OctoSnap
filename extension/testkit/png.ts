// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * PNGs written for tests (`src/pets/png.test.ts`, `src/pets/pack.test.ts`): any colour type
 * and depth, any filter, with a palette and transparency when asked. Outside `src/`, so it is
 * neither built into the extension nor taken for a test file.
 */

import { crc32, deflateSync, inflateSync } from 'node:zlib';

import type { Grid } from '../src/pets/art/painter.js';
import type { Inflate } from '../src/pets/png.js';

/** Node's inflate, stopped past `limit` as the shell's is. */
export const nodeInflate: Inflate = (data, limit) => new Uint8Array(inflateSync(data, { maxOutputLength: Math.max(1, limit) }));

export interface PngSpec {
    width: number;
    height: number;
    depth: number;
    /** 0 grey, 2 RGB, 3 palette, 4 grey and alpha, 6 RGBA. */
    type: number;
    /** Every pixel's samples, rows top to bottom. */
    samples: number[][];
    palette?: number[];
    trns?: number[];
    /** The filter for every row, or each row's own. */
    filter?: number | number[];
    interlace?: number;
}

export function chunk(type: string, data: Uint8Array): Buffer {
    const head = Buffer.alloc(8);
    head.writeUInt32BE(data.length, 0);
    head.write(type, 4, 'ascii');
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), Buffer.from(data)])) >>> 0, 0);
    return Buffer.concat([head, Buffer.from(data), crc]);
}

const SAMPLES: Record<number, number> = { 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 };

function paeth(a: number, b: number, c: number): number {
    const p = a + b - c;
    const pa = Math.abs(p - a);
    const pb = Math.abs(p - b);
    const pc = Math.abs(p - c);
    if (pa <= pb && pa <= pc) return a;
    return pb <= pc ? b : c;
}

/** The scanlines, filtered, before deflate. */
export function scanlines(spec: PngSpec): Buffer {
    const { width, height, depth, type, samples } = spec;
    const n = SAMPLES[type] ?? 1;
    const bits = depth * n;
    const stride = Math.ceil((width * bits) / 8);
    const bpp = Math.max(1, bits >> 3);
    const rows: Buffer[] = [];
    for (let y = 0; y < height; y++) {
        const row = Buffer.alloc(stride);
        for (let x = 0; x < width; x++) {
            const px = samples[y * width + x] ?? [];
            for (let s = 0; s < n; s++) {
                const v = px[s] ?? 0;
                const i = x * n + s;
                if (depth === 16) row.writeUInt16BE(v, i * 2);
                else if (depth === 8) row[i] = v;
                else {
                    const bit = i * depth;
                    row[bit >> 3] = (row[bit >> 3] ?? 0) | (v << (8 - depth - (bit & 7)));
                }
            }
        }
        rows.push(row);
    }
    const out = Buffer.alloc(height * (stride + 1));
    for (let y = 0; y < height; y++) {
        const filter = Array.isArray(spec.filter) ? (spec.filter[y] ?? 0) : (spec.filter ?? 0);
        const row = rows[y] ?? Buffer.alloc(stride);
        const prev = y > 0 ? (rows[y - 1] ?? Buffer.alloc(stride)) : Buffer.alloc(stride);
        out[y * (stride + 1)] = filter;
        for (let x = 0; x < stride; x++) {
            const a = x >= bpp ? (row[x - bpp] ?? 0) : 0;
            const b = prev[x] ?? 0;
            const c = x >= bpp ? (prev[x - bpp] ?? 0) : 0;
            const predict = [0, a, b, (a + b) >> 1, paeth(a, b, c)][filter] ?? 0;
            out[y * (stride + 1) + 1 + x] = ((row[x] ?? 0) - predict) & 0xff;
        }
    }
    return out;
}

export function encodePng(spec: PngSpec): Buffer {
    const header = Buffer.alloc(13);
    header.writeUInt32BE(spec.width, 0);
    header.writeUInt32BE(spec.height, 4);
    header.set([spec.depth, spec.type, 0, 0, spec.interlace ?? 0], 8);
    const parts = [Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk('IHDR', header)];
    if (spec.palette !== undefined) parts.push(chunk('PLTE', Uint8Array.from(spec.palette)));
    if (spec.trns !== undefined) parts.push(chunk('tRNS', Uint8Array.from(spec.trns)));
    parts.push(chunk('IDAT', deflateSync(scanlines(spec))), chunk('IEND', new Uint8Array(0)));
    return Buffer.concat(parts);
}

/** An 8-bit RGBA PNG of `rgba`. */
export function rgbaPng(width: number, height: number, rgba: Uint8Array, filter: number | number[] = 0): Buffer {
    const samples: number[][] = [];
    for (let i = 0; i < width * height; i++) samples.push([...rgba.subarray(i * 4, i * 4 + 4)]);
    return encodePng({ width, height, depth: 8, type: 6, samples, filter });
}

/** Grids side by side and in rows, `across` to a row, as a sheet's RGBA. */
export function sheetOf(grids: Grid[], across: number): { width: number; height: number; rgba: Uint8Array } {
    const w = grids[0]?.w ?? 0;
    const h = grids[0]?.h ?? 0;
    const rows = Math.ceil(grids.length / across);
    const width = w * across;
    const height = h * rows;
    const rgba = new Uint8Array(width * height * 4);
    grids.forEach((grid, i) => {
        const ox = (i % across) * w;
        const oy = Math.floor(i / across) * h;
        for (let y = 0; y < h; y++) {
            for (let x = 0; x < w; x++) {
                const colour = grid.px[y * w + x];
                if (colour === null || colour === undefined) continue;
                const o = ((oy + y) * width + ox + x) * 4;
                rgba.set([parseInt(colour.slice(1, 3), 16), parseInt(colour.slice(3, 5), 16), parseInt(colour.slice(5, 7), 16), 255], o);
            }
        }
    });
    return { width, height, rgba };
}

/** A grid from rows of characters: `.` clear, and each other character the colour `colours` gives it. */
export function gridOf(rows: string[], colours: Record<string, string>): Grid {
    const w = Math.max(...rows.map(r => r.length));
    const px: (string | null)[] = [];
    for (const row of rows) for (let x = 0; x < w; x++) px.push(row[x] === undefined || row[x] === '.' ? null : (colours[row[x] ?? ''] ?? null));
    return { w, h: rows.length, px };
}
