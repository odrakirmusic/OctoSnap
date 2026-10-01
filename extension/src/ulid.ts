// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * ULID generation, for spool and history ids.
 *
 * `spec/10` §4 names spool entries `<ulid>.png`, and the app side already uses the `ulid`
 * crate. GJS has no equivalent, so this is the canonical 26-character Crockford base32
 * form: 10 characters of 48-bit millisecond timestamp followed by 16 of randomness. The
 * timestamp prefix is what makes ids sort chronologically as plain strings, which is how
 * the history and the 24-hour spool cleanup in `spec/10` §4 stay cheap.
 *
 * The clock and the random source are parameters rather than globals so this module is
 * pure and testable under Node -- it imports nothing from `gi://`.
 */

/** Crockford base32: no I, L, O or U, so an id cannot be misread aloud. */
const ALPHABET = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';
const ENCODING_LENGTH = 32;

const TIME_LENGTH = 10;
const RANDOM_LENGTH = 16;
export const ULID_LENGTH = TIME_LENGTH + RANDOM_LENGTH;

/** The largest timestamp 10 base32 characters can hold: 2^48 - 1 ms, in AD 10889. */
export const MAX_TIME = 281_474_976_710_655;

/** Returns an integer in [0, 32). */
export type RandomSource = () => number;

function encodeTime(milliseconds: number, length: number): string {
    if (!Number.isFinite(milliseconds) || milliseconds < 0)
        throw new Error(`ulid: timestamp must be a non-negative number, got ${milliseconds}`);
    if (milliseconds > MAX_TIME)
        throw new Error(`ulid: timestamp ${milliseconds} exceeds the 48-bit ULID range`);

    let remaining = Math.floor(milliseconds);
    let out = '';
    for (let i = 0; i < length; i++) {
        const digit = remaining % ENCODING_LENGTH;
        out = ALPHABET[digit] + out;
        remaining = (remaining - digit) / ENCODING_LENGTH;
    }
    return out;
}

function encodeRandom(length: number, random: RandomSource): string {
    let out = '';
    for (let i = 0; i < length; i++) {
        const digit = random();
        if (!Number.isInteger(digit) || digit < 0 || digit >= ENCODING_LENGTH)
            throw new Error(`ulid: random source returned ${digit}, expected 0..31`);
        out += ALPHABET[digit];
    }
    return out;
}

/** Builds a ULID from an explicit timestamp and random source. */
export function ulidFrom(milliseconds: number, random: RandomSource): string {
    return encodeTime(milliseconds, TIME_LENGTH) + encodeRandom(RANDOM_LENGTH, random);
}

/** True when a string is a syntactically valid ULID. */
export function isUlid(value: string): boolean {
    return (
        value.length === ULID_LENGTH &&
        [...value].every(c => ALPHABET.includes(c))
    );
}

/** The millisecond timestamp encoded in a ULID's prefix. */
export function timeOf(value: string): number {
    if (!isUlid(value)) throw new Error(`ulid: '${value}' is not a ULID`);
    let time = 0;
    for (const c of value.slice(0, TIME_LENGTH))
        time = time * ENCODING_LENGTH + ALPHABET.indexOf(c);
    return time;
}
