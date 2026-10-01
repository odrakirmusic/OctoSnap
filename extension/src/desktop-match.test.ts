// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';

import { looksLikeDesktopIcons } from './desktop-match.js';

describe('looksLikeDesktopIcons', () => {
    it('accepts a window Mutter already types as the desktop', () => {
        expect(looksLikeDesktopIcons('anything', true)).toBe(true);
        expect(looksLikeDesktopIcons(null, true)).toBe(true);
    });

    it("accepts DING's titles before the type is set", () => {
        expect(looksLikeDesktopIcons('Desktop Icons 0', false)).toBe(true);
        expect(looksLikeDesktopIcons('@!0,0;H', false)).toBe(true);
        expect(looksLikeDesktopIcons('@!HTD', false)).toBe(true);
    });

    it('rejects ordinary windows', () => {
        expect(looksLikeDesktopIcons('Firefox', false)).toBe(false);
        expect(looksLikeDesktopIcons('octosnap-qao:3', false)).toBe(false);
        expect(looksLikeDesktopIcons(null, false)).toBe(false);
        expect(looksLikeDesktopIcons('', false)).toBe(false);
    });
});
