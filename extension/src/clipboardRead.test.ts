// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * D169: the shell reads the clipboard for the app, its pixels before a file, and only an
 * image of either.
 */

import { describe, expect, it } from 'vitest';

import { READ_NAME, chooseRead, firstUri, readName, readSuffix } from './clipboardRead.js';
import { ulidFrom } from './ulid.js';

describe('chooseRead', () => {
    // What a GTK 4 app's Copy of a picture offered in the nested shell, in its order.
    const gtkPicture = ['image/png', 'image/tiff', 'image/jpeg', 'image/jxl', 'image/webp', 'audio/x-riff',
        'image/avif', 'image/bmp', 'image/x-icon'];
    // And a GTK 4 app's Copy of a file list, as GDK offers one.
    const gtkFile = ['text/plain;charset=utf-8', 'text/uri-list', 'application/vnd.portal.filetransfer',
        'application/vnd.portal.files'];

    it('reads a picture as PNG whenever PNG is offered, wherever it is in the list', () => {
        expect(chooseRead(gtkPicture)).toEqual({ kind: 'pixels', mime: 'image/png' });
        expect(chooseRead([...gtkPicture].reverse())).toEqual({ kind: 'pixels', mime: 'image/png' });
    });

    it('takes another image type when PNG is not offered', () => {
        expect(chooseRead(['text/html', 'image/jpeg', 'image/bmp'])).toEqual({ kind: 'pixels', mime: 'image/jpeg' });
    });

    it('reads a file list when there are no pixels', () => {
        expect(chooseRead(gtkFile)).toEqual({ kind: 'file' });
        // OctoSnap's own copy of a GIF (D165).
        expect(chooseRead(['text/uri-list'])).toEqual({ kind: 'file' });
    });

    it('prefers pixels to a file when both are offered', () => {
        expect(chooseRead(['text/uri-list', 'image/png'])).toEqual({ kind: 'pixels', mime: 'image/png' });
    });

    it('reads nothing from text, or from an empty clipboard', () => {
        expect(chooseRead(['text/plain;charset=utf-8', 'UTF8_STRING', 'text/html'])).toBeNull();
        expect(chooseRead([])).toBeNull();
        // Not an image, whatever the RIFF in a GTK picture's offer is.
        expect(chooseRead(['audio/x-riff'])).toBeNull();
    });
});

describe('firstUri', () => {
    it('takes the first URI of a CRLF list', () => {
        expect(firstUri('file:///tmp/a.png\r\nfile:///tmp/b.png\r\n')).toBe('file:///tmp/a.png');
    });

    it('skips comments and blank lines, and takes a bare LF too', () => {
        expect(firstUri('# copied\n\n  file:///tmp/Shot%201.gif  \n')).toBe('file:///tmp/Shot%201.gif');
    });

    it('finds none in an empty list', () => {
        expect(firstUri('')).toBeNull();
        expect(firstUri('# only a comment\r\n\r\n')).toBeNull();
    });
});

describe('readName', () => {
    const id = '01M4D4XD98A5CG004H15FTMR0G';

    it('gives a GIF the suffix the app tells one by', () => {
        expect(readName(id, 'image/gif')).toBe(`${id}.gif`);
    });

    it('gives the usual suffixes to the usual types', () => {
        expect(readSuffix('image/png')).toBe('png');
        expect(readSuffix('image/jpeg')).toBe('jpg');
        expect(readSuffix('image/svg+xml')).toBe('svg');
        expect(readSuffix('image/x-MS-bmp')).toBe('msbmp');
        expect(readSuffix('image/')).toBe('img');
    });

    it('matches every name it gives, and nothing else, for the folder\'s pruning', () => {
        for (const mime of ['image/png', 'image/gif', 'image/jpeg', 'image/x-icon', 'image/svg+xml'])
            expect(READ_NAME.test(readName(id, mime))).toBe(true);
        // Every ULID the shell makes for one, whatever its random digits.
        for (const digit of [0, 7, 31])
            expect(READ_NAME.test(readName(ulidFrom(Date.now(), () => digit), 'image/png'))).toBe(true);
        expect(READ_NAME.test('Shot 1.gif')).toBe(false);
        expect(READ_NAME.test(`${id}.png.partial`)).toBe(false);
        expect(READ_NAME.test(`../${id}.png`)).toBe(false);
    });
});
