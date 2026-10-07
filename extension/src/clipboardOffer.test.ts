// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * D165: the clipboard's type follows the bytes, never the file's name, and a GIF is
 * offered as its file.
 */

import { describe, expect, it } from 'vitest';

import { SNIFF_LENGTH, gifOffer, pngOffer, sniff, uriList } from './clipboardOffer.js';

const bytes = (...values: (number | string)[]): Uint8Array =>
    new Uint8Array(values.flatMap(v => (typeof v === 'string' ? [...v].map(c => c.charCodeAt(0)) : [v])));

describe('sniff', () => {
    it('knows a PNG by its signature', () => {
        expect(sniff(bytes(0x89, 'PNG', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13))).toBe('png');
    });

    it('knows both GIF versions', () => {
        expect(sniff(bytes('GIF89a', 0xf0, 0))).toBe('gif');
        expect(sniff(bytes('GIF87a', 0xf0, 0))).toBe('gif');
    });

    // The bug: a recording's GIF went out as image/png because nothing looked at it.
    it('does not take GIF bytes for a PNG, or the other way round', () => {
        expect(sniff(bytes('GIF89a', 0, 0))).not.toBe('png');
        expect(sniff(bytes(0x89, 'PNG', 0x0d, 0x0a, 0x1a, 0x0a))).not.toBe('gif');
    });

    it('offers nothing for anything else, or for too little to tell', () => {
        expect(sniff(bytes(0xff, 0xd8, 0xff, 0xe0, 0, 0x10, 'JF'))).toBeNull(); // a JPEG
        expect(sniff(bytes('GIF8'))).toBeNull();
        expect(sniff(bytes(0x89, 'PNG'))).toBeNull();
        expect(sniff(new Uint8Array())).toBeNull();
    });

    it('needs no more than SNIFF_LENGTH bytes', () => {
        const png = bytes(0x89, 'PNG', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, 'IHDR');
        expect(sniff(png.slice(0, SNIFF_LENGTH))).toBe('png');
        expect(sniff(bytes('GIF89a', 1, 2, 3, 4).slice(0, SNIFF_LENGTH))).toBe('gif');
    });
});

describe('the offers', () => {
    it('gives a PNG as its own bytes, as image/png', () => {
        const png = bytes(0x89, 'PNG', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3);
        expect(pngOffer(png)).toEqual({ mime: 'image/png', bytes: png });
    });

    it('gives a GIF as its URI, as RFC 2483 text/uri-list', () => {
        const offer = gifOffer('file:///home/me/.cache/octosnap/clipboard/01K/Take%201.gif');
        expect(offer.mime).toBe('text/uri-list');
        expect(new TextDecoder().decode(offer.bytes)).toBe(
            'file:///home/me/.cache/octosnap/clipboard/01K/Take%201.gif\r\n',
        );
    });

    it('ends every line of a list with CRLF', () => {
        expect(new TextDecoder().decode(uriList(['file:///a', 'file:///b']))).toBe('file:///a\r\nfile:///b\r\n');
    });
});
