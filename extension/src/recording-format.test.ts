// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The recorder's clock, which is the kind of thing that looks right and is off by one: it
 * has to read the same as the app pill's Rust `elapsed_label`. Where its frame goes is
 * `outline.test.ts`'s.
 */

import { describe, expect, it } from 'vitest';

import { elapsedLabel } from './recording-format.js';

describe('elapsedLabel', () => {
    it('matches the app pill (media::text::elapsed_label) at the same instants', () => {
        expect(elapsedLabel(0)).toBe('00:00');
        expect(elapsedLabel(42_900)).toBe('00:42'); // seconds floored, not rounded
        expect(elapsedLabel(65_000)).toBe('01:05');
        expect(elapsedLabel(3_600_000)).toBe('1:00:00');
    });

    it('reads a negative or absurd input as zero rather than a stray minus', () => {
        expect(elapsedLabel(-5)).toBe('00:00');
    });
});
