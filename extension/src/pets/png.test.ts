// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The PNG reader pet packs' sheets go through (`png.ts`, `spec/14` §14), on PNGs Node's zlib
 * writes: every colour type at every depth, every filter, transparency, and everything a
 * broken or hostile file can be.
 */

import { deflateSync } from 'node:zlib';

import { describe, expect, it } from 'vitest';

import { chunk, encodePng, nodeInflate, rgbaPng, scanlines } from '../../testkit/png.js';
import { decodePng } from './png.js';

const pixel = (rgba: Uint8Array, i: number) => [...rgba.subarray(i * 4, i * 4 + 4)];

describe('the PNG reader', () => {
    it('reads RGBA through every filter', () => {
        const width = 5;
        const height = 5;
        const rgba = new Uint8Array(width * height * 4);
        for (let i = 0; i < rgba.length; i++) rgba[i] = (i * 37 + (i >> 3) * 11) & 0xff;
        for (const filter of [0, 1, 2, 3, 4, [0, 1, 2, 3, 4]]) {
            const got = decodePng(rgbaPng(width, height, rgba, filter), nodeInflate);
            expect([got.width, got.height]).toEqual([width, height]);
            expect(Buffer.from(got.rgba).equals(Buffer.from(rgba)), `filter ${String(filter)}`).toBe(true);
        }
    });

    it('reads 16-bit RGBA by its high bytes', () => {
        const png = encodePng({ width: 2, height: 1, depth: 16, type: 6, samples: [[0xff00, 0x1234, 0x00ff, 0xffff], [0, 0, 0, 0x7fff]], filter: 4 });
        const got = decodePng(png, nodeInflate);
        expect(pixel(got.rgba, 0)).toEqual([0xff, 0x12, 0x00, 0xff]);
        expect(pixel(got.rgba, 1)).toEqual([0, 0, 0, 0x7f]);
    });

    it('reads RGB, with a colour made clear', () => {
        const png = encodePng({ width: 2, height: 1, depth: 8, type: 2, samples: [[10, 20, 30], [1, 2, 3]], trns: [0, 1, 0, 2, 0, 3] });
        const got = decodePng(png, nodeInflate);
        expect(pixel(got.rgba, 0)).toEqual([10, 20, 30, 255]);
        expect(pixel(got.rgba, 1)).toEqual([1, 2, 3, 0]);
    });

    it('reads a palette at every depth, with its transparency', () => {
        for (const depth of [1, 2, 4, 8]) {
            const count = Math.min(4, 1 << depth);
            const palette = [255, 0, 0, 0, 255, 0, 0, 0, 255, 9, 9, 9].slice(0, count * 3);
            const samples = [...Array(7).keys()].map(i => [i % count]);
            const png = encodePng({ width: 7, height: 1, depth, type: 3, samples, palette, trns: [0], filter: 1 });
            const got = decodePng(png, nodeInflate);
            for (let i = 0; i < 7; i++) {
                const index = i % count;
                expect(pixel(got.rgba, i), `depth ${depth}, pixel ${i}`).toEqual([...palette.slice(index * 3, index * 3 + 3), index === 0 ? 0 : 255]);
            }
        }
    });

    it('reads grey at every depth, scaled to eight bits', () => {
        for (const depth of [1, 2, 4, 8, 16]) {
            const max = depth === 16 ? 0xffff : (1 << depth) - 1;
            const png = encodePng({ width: 3, height: 2, depth, type: 0, samples: [[0], [max], [max >> 1], [max], [0], [0]], trns: [0, 0], filter: 2 });
            const got = decodePng(png, nodeInflate);
            expect(pixel(got.rgba, 0), `depth ${depth}`).toEqual([0, 0, 0, 0]);
            expect(pixel(got.rgba, 1), `depth ${depth}`).toEqual([255, 255, 255, 255]);
            const mid = depth === 16 ? (max >> 1) >> 8 : depth === 8 ? max >> 1 : Math.round(((max >> 1) * 255) / max);
            // At depth 1 half of the most is 0, the grey made clear.
            expect(pixel(got.rgba, 2), `depth ${depth}`).toEqual([mid, mid, mid, max >> 1 === 0 ? 0 : 255]);
        }
    });

    it('reads grey with alpha', () => {
        const got8 = decodePng(encodePng({ width: 1, height: 1, depth: 8, type: 4, samples: [[200, 100]] }), nodeInflate);
        expect(pixel(got8.rgba, 0)).toEqual([200, 200, 200, 100]);
        const got16 = decodePng(encodePng({ width: 1, height: 1, depth: 16, type: 4, samples: [[0xc800, 0x6400]] }), nodeInflate);
        expect(pixel(got16.rgba, 0)).toEqual([200, 200, 200, 100]);
    });

    it('takes chunks it does not know and skips them', () => {
        const png = rgbaPng(1, 1, Uint8Array.from([1, 2, 3, 255]));
        const withText = Buffer.concat([png.subarray(0, 33), chunk('tEXt', Buffer.from('Software\0test')), png.subarray(33)]);
        expect(pixel(decodePng(withText, nodeInflate).rgba, 0)).toEqual([1, 2, 3, 255]);
    });

    describe('refuses, saying why', () => {
        const ok = rgbaPng(2, 2, new Uint8Array(16).fill(255));
        const cases: [string, Uint8Array, RegExp][] = [
            ['a file that is not a PNG', Buffer.from('GIF89a and more'), /not a PNG/],
            ['an empty file', new Uint8Array(0), /not a PNG/],
            ['an interlaced one', encodePng({ width: 2, height: 2, depth: 8, type: 6, samples: [[], [], [], []], interlace: 1 }), /interlaced/],
            ['one bigger than the limit', rgbaPng(600, 1, new Uint8Array(2400)), /600 × 1 pixels, more than 512 × 512/],
            ['one cut short', ok.subarray(0, ok.length - 20), /runs past the end|cut short/],
            ['one with no end', ok.subarray(0, ok.length - 12), /cut short/],
            ['a depth its colour type cannot have', encodePng({ width: 1, height: 1, depth: 4, type: 6, samples: [[0, 0, 0, 0]] }), /colour type 6 at depth 4/],
            ['a palette image with no palette', encodePng({ width: 1, height: 1, depth: 8, type: 3, samples: [[0]] }), /no palette/],
            ['a colour past its palette', encodePng({ width: 1, height: 1, depth: 8, type: 3, samples: [[5]], palette: [0, 0, 0] }), /past the end of its palette/],
        ];
        for (const [what, png, message] of cases) it(what, () => expect(() => decodePng(png, nodeInflate)).toThrow(message));

        it('a filter PNG does not define', () => {
            const raw = scanlines({ width: 1, height: 1, depth: 8, type: 6, samples: [[1, 2, 3, 4]] });
            raw[0] = 7;
            const header = Buffer.alloc(13);
            header.writeUInt32BE(1, 0);
            header.writeUInt32BE(1, 4);
            header.set([8, 6, 0, 0, 0], 8);
            const png = Buffer.concat([ok.subarray(0, 8), chunk('IHDR', header), chunk('IDAT', deflateSync(raw)), chunk('IEND', new Uint8Array(0))]);
            expect(() => decodePng(png, nodeInflate)).toThrow(/filter 7/);
        });

        it('pixels that do not decompress', () => {
            const header = Buffer.alloc(13);
            header.writeUInt32BE(1, 0);
            header.writeUInt32BE(1, 4);
            header.set([8, 6, 0, 0, 0], 8);
            const png = Buffer.concat([ok.subarray(0, 8), chunk('IHDR', header), chunk('IDAT', Buffer.from('not zlib at all')), chunk('IEND', new Uint8Array(0))]);
            expect(() => decodePng(png, nodeInflate)).toThrow(/do not decompress/);
        });

        it('more pixels than the image says it has, as a bomb would be', () => {
            const header = Buffer.alloc(13);
            header.writeUInt32BE(1, 0);
            header.writeUInt32BE(1, 4);
            header.set([8, 6, 0, 0, 0], 8);
            const png = Buffer.concat([ok.subarray(0, 8), chunk('IHDR', header), chunk('IDAT', deflateSync(Buffer.alloc(10_000_000))), chunk('IEND', new Uint8Array(0))]);
            expect(() => decodePng(png, nodeInflate)).toThrow(/do not decompress/);
        });

        it('a chunk longer than the file', () => {
            const png = Buffer.from(ok);
            png.writeUInt32BE(0x7fffffff, 8);
            expect(() => decodePng(png, nodeInflate)).toThrow(/runs past the end/);
        });
    });
});
