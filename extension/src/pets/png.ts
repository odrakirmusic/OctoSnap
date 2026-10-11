// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * A PNG reader for pet packs' sheets (`spec/14` §14), to straight 8-bit RGBA.
 *
 * Plain TypeScript with the inflate handed in: the shell passes one made of
 * `Gio.ZlibDecompressor` (`packs.ts`), vitest Node's zlib, so a sheet is read by the same
 * code in both, and nothing depends on which loaders gdk-pixbuf has (D196). It reads what a
 * pixel-art tool writes: every colour type, at every depth the format allows, with a
 * palette's or a colour's transparency, not interlaced. No `gi://` imports.
 *
 * A sheet comes from someone's folder and is read inside GNOME Shell, so nothing here trusts
 * it: every length is checked against what is left, the image's size against a limit given
 * before anything is inflated, and the inflate is told the most it may give back.
 */

/** Inflates a zlib stream, giving back at most `limit` bytes or throwing. */
export type Inflate = (data: Uint8Array, limit: number) => Uint8Array;

export interface Rgba {
    width: number;
    height: number;
    /** Straight alpha, four bytes a pixel, rows top to bottom. */
    rgba: Uint8Array;
}

const SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10];

/** The depths each colour type may have. */
const DEPTHS: Record<number, readonly number[]> = {
    0: [1, 2, 4, 8, 16],
    2: [8, 16],
    3: [1, 2, 4, 8],
    4: [8, 16],
    6: [8, 16],
};

/** Samples a pixel of each colour type has. */
const SAMPLES: Record<number, number> = { 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 };

function u32(bytes: Uint8Array, at: number): number {
    return ((bytes[at] ?? 0) * 0x1000000 + ((bytes[at + 1] ?? 0) << 16)) + ((bytes[at + 2] ?? 0) << 8) + (bytes[at + 3] ?? 0);
}

function u16(bytes: Uint8Array, at: number): number {
    return ((bytes[at] ?? 0) << 8) | (bytes[at + 1] ?? 0);
}

function paeth(a: number, b: number, c: number): number {
    const p = a + b - c;
    const pa = Math.abs(p - a);
    const pb = Math.abs(p - b);
    const pc = Math.abs(p - c);
    if (pa <= pb && pa <= pc) return a;
    return pb <= pc ? b : c;
}

/**
 * `bytes`, a PNG, as RGBA. Throws an `Error` whose message says what is wrong, in words a
 * pack's author can act on, for anything it does not read: not a PNG, interlaced, bigger
 * than `maxSide` either way, or broken.
 */
export function decodePng(bytes: Uint8Array, inflate: Inflate, maxSide = 512): Rgba {
    if (bytes.length < 8 || SIGNATURE.some((b, i) => bytes[i] !== b)) throw new Error('it is not a PNG');
    let at = 8;
    let header: { width: number; height: number; depth: number; type: number } | null = null;
    let palette: Uint8Array | null = null;
    let trns: Uint8Array | null = null;
    const data: Uint8Array[] = [];
    let ended = false;
    while (at + 8 <= bytes.length) {
        const length = u32(bytes, at);
        const type = String.fromCharCode(bytes[at + 4] ?? 0, bytes[at + 5] ?? 0, bytes[at + 6] ?? 0, bytes[at + 7] ?? 0);
        const start = at + 8;
        // The chunk and its CRC have to fit in what is left.
        if (length > bytes.length - start - 4) throw new Error(`its ${type} chunk runs past the end of the file`);
        const body = bytes.subarray(start, start + length);
        at = start + length + 4;
        if (type === 'IHDR') {
            if (length !== 13) throw new Error('its header is broken');
            const width = u32(body, 0);
            const height = u32(body, 4);
            const depth = body[8] ?? 0;
            const colour = body[9] ?? 0;
            if (width === 0 || height === 0) throw new Error('it has no pixels');
            if (width > maxSide || height > maxSide)
                throw new Error(`it is ${width} × ${height} pixels, more than ${maxSide} × ${maxSide}`);
            if (!(DEPTHS[colour] ?? []).includes(depth)) throw new Error(`its colour type ${colour} at depth ${depth} is not a PNG's`);
            if ((body[10] ?? 0) !== 0 || (body[11] ?? 0) !== 0) throw new Error('it uses a compression or filter PNG does not define');
            if ((body[12] ?? 0) !== 0) throw new Error('it is interlaced: save it without interlacing');
            header = { width, height, depth, type: colour };
        } else if (header === null) {
            throw new Error('its first chunk is not its header');
        } else if (type === 'PLTE') {
            if (length % 3 !== 0 || length === 0 || length > 768) throw new Error('its palette is broken');
            palette = body;
        } else if (type === 'tRNS') {
            trns = body;
        } else if (type === 'IDAT') {
            data.push(body);
        } else if (type === 'IEND') {
            ended = true;
            break;
        }
    }
    if (header === null) throw new Error('it has no header');
    if (!ended) throw new Error('it is cut short');
    const { width, height, depth, type } = header;
    if (type === 3 && palette === null) throw new Error('it has no palette');

    const bits = depth * (SAMPLES[type] ?? 1);
    const stride = Math.ceil((width * bits) / 8);
    const expected = height * (stride + 1);
    const joined = new Uint8Array(data.reduce((n, d) => n + d.length, 0));
    let o = 0;
    for (const d of data) {
        joined.set(d, o);
        o += d.length;
    }
    let raw: Uint8Array;
    try {
        raw = inflate(joined, expected);
    } catch {
        throw new Error('its pixels do not decompress');
    }
    if (raw.length < expected) throw new Error('its pixels are cut short');

    // Unfiltered in place, a row at a time, against the row before.
    const bpp = Math.max(1, bits >> 3);
    const rows = new Uint8Array(height * stride);
    for (let y = 0; y < height; y++) {
        const filter = raw[y * (stride + 1)] ?? 0;
        const src = y * (stride + 1) + 1;
        const row = y * stride;
        const prev = row - stride;
        for (let x = 0; x < stride; x++) {
            const value = raw[src + x] ?? 0;
            const a = x >= bpp ? (rows[row + x - bpp] ?? 0) : 0;
            const b = y > 0 ? (rows[prev + x] ?? 0) : 0;
            const c = x >= bpp && y > 0 ? (rows[prev + x - bpp] ?? 0) : 0;
            let out: number;
            switch (filter) {
                case 0:
                    out = value;
                    break;
                case 1:
                    out = value + a;
                    break;
                case 2:
                    out = value + b;
                    break;
                case 3:
                    out = value + ((a + b) >> 1);
                    break;
                case 4:
                    out = value + paeth(a, b, c);
                    break;
                default:
                    throw new Error(`row ${y} has filter ${filter}, which PNG does not define`);
            }
            rows[row + x] = out & 0xff;
        }
    }

    const rgba = new Uint8Array(width * height * 4);
    const max = (1 << depth) - 1;
    // A sample of `depth` bits, the `i`th along row `y`.
    const sample = (y: number, i: number): number => {
        const row = y * stride;
        if (depth === 8) return rows[row + i] ?? 0;
        if (depth === 16) return u16(rows, row + i * 2);
        const bit = i * depth;
        return ((rows[row + (bit >> 3)] ?? 0) >> (8 - depth - (bit & 7))) & max;
    };
    const to8 = (v: number): number => (depth === 16 ? v >> 8 : depth === 8 ? v : Math.round((v * 255) / max));
    for (let y = 0; y < height; y++) {
        for (let x = 0; x < width; x++) {
            const p = (y * width + x) * 4;
            let r: number;
            let g: number;
            let b: number;
            let alpha = 255;
            if (type === 3) {
                const index = sample(y, x);
                if (palette === null || index * 3 + 2 >= palette.length) throw new Error(`a pixel uses colour ${index}, past the end of its palette`);
                r = palette[index * 3] ?? 0;
                g = palette[index * 3 + 1] ?? 0;
                b = palette[index * 3 + 2] ?? 0;
                if (trns !== null && index < trns.length) alpha = trns[index] ?? 255;
            } else if (type === 0 || type === 4) {
                const n = SAMPLES[type] ?? 1;
                const v = sample(y, x * n);
                r = g = b = to8(v);
                if (type === 4) alpha = to8(sample(y, x * n + 1));
                else if (trns !== null && trns.length >= 2 && v === u16(trns, 0)) alpha = 0;
            } else {
                const n = SAMPLES[type] ?? 3;
                const vr = sample(y, x * n);
                const vg = sample(y, x * n + 1);
                const vb = sample(y, x * n + 2);
                r = to8(vr);
                g = to8(vg);
                b = to8(vb);
                if (type === 6) alpha = to8(sample(y, x * n + 3));
                else if (trns !== null && trns.length >= 6 && vr === u16(trns, 0) && vg === u16(trns, 2) && vb === u16(trns, 4)) alpha = 0;
            }
            rgba[p] = r;
            rgba[p + 1] = g;
            rgba[p + 2] = b;
            rgba[p + 3] = alpha;
        }
    }
    return { width, height, rgba };
}
