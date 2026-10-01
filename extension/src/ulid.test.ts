// SPDX-License-Identifier: GPL-3.0-or-later

import { describe, expect, it } from 'vitest';

import { MAX_TIME, ULID_LENGTH, isUlid, timeOf, ulidFrom } from './ulid.js';

/** A deterministic source so ids are reproducible in tests. */
const zeros = () => 0;
const counter = () => {
    let n = 0;
    return () => n++ % 32;
};

describe('ulid', () => {
    it('is 26 Crockford base32 characters', () => {
        const id = ulidFrom(1_757_251_200_000, zeros);
        expect(id).toHaveLength(ULID_LENGTH);
        expect(isUlid(id)).toBe(true);
    });

    it('round-trips its timestamp', () => {
        for (const ms of [0, 1, 1_757_251_200_000, MAX_TIME])
            expect(timeOf(ulidFrom(ms, zeros))).toBe(ms);
    });

    // The whole reason for the timestamp prefix: spool cleanup and history ordering
    // work on the id alone, without opening anything.
    it('sorts chronologically as a plain string', () => {
        const early = ulidFrom(1_757_251_200_000, zeros);
        const later = ulidFrom(1_757_251_200_001, zeros);
        const muchLater = ulidFrom(1_800_000_000_000, zeros);
        expect([muchLater, early, later].sort()).toEqual([early, later, muchLater]);
    });

    it('varies in its random half within the same millisecond', () => {
        const source = counter();
        const a = ulidFrom(1_757_251_200_000, source);
        const b = ulidFrom(1_757_251_200_000, source);
        expect(a).not.toBe(b);
        // Same millisecond, so the time prefix must be identical.
        expect(a.slice(0, 10)).toBe(b.slice(0, 10));
    });

    it('excludes the ambiguous letters', () => {
        const id = ulidFrom(MAX_TIME, counter());
        for (const c of 'ILOU') expect(id).not.toContain(c);
    });

    it('rejects a timestamp outside the 48-bit range', () => {
        expect(() => ulidFrom(-1, zeros)).toThrow();
        expect(() => ulidFrom(MAX_TIME + 1, zeros)).toThrow();
    });

    // A silently truncated id would collide, which for a spool is data loss.
    it('rejects a random source that is out of range', () => {
        expect(() => ulidFrom(0, () => 32)).toThrow();
        expect(() => ulidFrom(0, () => -1)).toThrow();
        expect(() => ulidFrom(0, () => 1.5)).toThrow();
    });

    it('rejects non-ULIDs', () => {
        expect(isUlid('')).toBe(false);
        expect(isUlid('short')).toBe(false);
        expect(isUlid('I'.repeat(26))).toBe(false);
        expect(() => timeOf('nope')).toThrow();
    });
});
