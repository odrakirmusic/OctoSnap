// SPDX-License-Identifier: GPL-3.0-or-later

import { describe, expect, it } from 'vitest';

import { takenAsPicture } from './captureFeedback.js';

describe('what a capture sounds and looks like (D171)', () => {
    it('gives a text capture no shutter and no flight', () => {
        expect(takenAsPicture('ocr')).toBe(false);
    });

    it('gives every picture its shutter and its flight', () => {
        const pictures = ['area', 'all-in-one', 'fullscreen', 'window', 'previous-area', 'self-timer'];
        for (const mode of pictures) expect(takenAsPicture(mode)).toBe(true);
    });
});
