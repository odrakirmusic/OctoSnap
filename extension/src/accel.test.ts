// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The accelerator hint the panel menu shows on each row (appendix A.10: "hints appear per
 * row when bound, right-aligned"). Worth testing because a mistake here does not look
 * like a bug -- it teaches the user the wrong chord.
 */

import { describe, expect, it } from 'vitest';

import { prettyAccelerator } from './accel.js';

describe('prettyAccelerator', () => {
    it('renders the schema defaults the way GNOME writes them', () => {
        expect(prettyAccelerator('<Control><Alt><Super>a')).toBe('Ctrl+Alt+Super+A');
        expect(prettyAccelerator('<Control><Alt><Super>space')).toBe('Ctrl+Alt+Super+Space');
    });

    it('accepts the spellings GSettings uses interchangeably', () => {
        // <Primary> is what GTK writes for Ctrl, and both appear in real dconf.
        expect(prettyAccelerator('<Primary>c')).toBe('Ctrl+C');
        expect(prettyAccelerator('<Ctrl>c')).toBe('Ctrl+C');
        // <Mod1> is Alt, which is how older tools spell it.
        expect(prettyAccelerator('<Mod1>x')).toBe('Alt+X');
    });

    it('names keys as they appear on a keycap', () => {
        expect(prettyAccelerator('Print')).toBe('Print');
        expect(prettyAccelerator('<Shift>Print')).toBe('Shift+Print');
        expect(prettyAccelerator('<Super>Return')).toBe('Super+Enter');
        expect(prettyAccelerator('Escape')).toBe('Esc');
    });

    it('uses arrows for the arrow keys', () => {
        expect(prettyAccelerator('<Super>Up')).toBe('Super+↑');
        expect(prettyAccelerator('<Super>Left')).toBe('Super+←');
    });

    it('uppercases a bare letter, because a keycap is uppercase', () => {
        expect(prettyAccelerator('<Super>4')).toBe('Super+4');
        expect(prettyAccelerator('<Super>f')).toBe('Super+F');
    });

    /**
     * A hand-edited dconf value can be anything. Showing it verbatim is more use than
     * showing nothing: it is still the string the user has to press, and it is what they
     * would have to go and fix.
     */
    it('passes an unrecognised accelerator through rather than dropping it', () => {
        expect(prettyAccelerator('<Frobnicate>q')).toBe('Frobnicate+Q');
        expect(prettyAccelerator('XF86MonBrightnessUp')).toBe('XF86MONBRIGHTNESSUP');
    });

    it('handles a modifier with no key, which is what a half-typed binding looks like', () => {
        expect(prettyAccelerator('<Control>')).toBe('Ctrl');
    });

    it('is empty for an empty accelerator, so the caller can drop the hint', () => {
        expect(prettyAccelerator('')).toBe('');
    });
});
