// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The pixel-art painter every pet is drawn with (`spec/14` §2).
 *
 * A pet is not a picture stored somewhere. It is a few shapes -- ellipses for a body, eye
 * bumps and feet, small stamped rows for eyes and mouths -- painted onto a grid of art
 * pixels, each one given a material ("skin", "belly") and a tone within it (highlight,
 * base, shadow, outline). The palette turns that into colours only at the end. Two things
 * follow. A pose is a handful of numbers, so a squash, a look to one side or a blink costs
 * no artwork, and the same pose always paints the same pixels. And the shading and the
 * outline are worked out from the shapes, so a squashed body is still lit from the top
 * left and still outlined, where a stretched bitmap would smear both.
 *
 * No `gi://` imports: the extension paints with this at runtime, and the Node build that
 * makes the SVG sheets for Settings, the guide and the store paints with the same code
 * (`data/pets/build.mjs`). Nothing here may depend on the time or on randomness.
 *
 * The names here, and in each pet's drawing, are short where they are the geometry's own:
 * `x` and `y`, an ellipse's centre `cx, cy` and radii `rx, ry`, a rotation's cosine and sine
 * `c, s`, and the tones' `h`, `b`, `s` and `o`. A pet's drawing calls its painter `P`.
 */

/** A tone within a material: highlight, base, shadow, outline. */
export type Tone = 'h' | 'b' | 's' | 'o';

/** Colours for one material; `b` is the fallback for a tone a material does not define. */
export type Material = Partial<Record<Tone, string>> & { b: string };

/** Every material a pet is painted with, by name. */
export type Palette = Record<string, Material>;

/** A painted pose: `w` x `h` art pixels, each a colour or `null` for transparent. */
export interface Grid {
    w: number;
    h: number;
    px: (string | null)[];
}

/**
 * Whether the art pixel at `x, y` has its centre inside an ellipse, optionally rotated by
 * `rot` radians about its own centre.
 */
export function inEllipse(
    x: number,
    y: number,
    cx: number,
    cy: number,
    rx: number,
    ry: number,
    rot = 0,
): boolean {
    let dx = x + 0.5 - cx;
    let dy = y + 0.5 - cy;
    if (rot !== 0) {
        const c = Math.cos(rot);
        const s = Math.sin(rot);
        const tx = dx * c + dy * s;
        const ty = -dx * s + dy * c;
        dx = tx;
        dy = ty;
    }
    return (dx * dx) / (rx * rx) + (dy * dy) / (ry * ry) <= 1;
}

/**
 * The tone of a point on an ellipse read as a sphere lit from the top left and a little
 * in front: `h` where it faces the light, `s` where it turns away, `b` between.
 */
export function sphereTone(
    x: number,
    y: number,
    cx: number,
    cy: number,
    rx: number,
    ry: number,
    hiT: number,
    shT: number,
): Tone {
    const nx = (x + 0.5 - cx) / rx;
    const ny = (y + 0.5 - cy) / ry;
    const nz = Math.sqrt(Math.max(0, 1 - nx * nx - ny * ny));
    const d = (nx * LIGHT[0] + ny * LIGHT[1] + nz * LIGHT[2]) / LIGHT_LENGTH;
    if (d > hiT) return 'h';
    if (d < shT) return 's';
    return 'b';
}

const LIGHT = [-0.5, -0.62, 0.6] as const;
const LIGHT_LENGTH = Math.hypot(LIGHT[0], LIGHT[1], LIGHT[2]);

export interface EllipseOptions {
    /** Rotation about the centre, in radians. */
    rot?: number;
    /** Shade as a sphere; otherwise every pixel takes `tone`. */
    shade?: boolean;
    hi?: number;
    sh?: number;
    /** Paint only where this says yes. */
    clip?: (x: number, y: number) => boolean;
    tone?: Tone;
    /** Paint only over pixels already of one of these materials. */
    only?: readonly string[];
}

/** One stamp character's meaning: a material and the tone it takes. */
export type StampKey = Record<string, readonly [string, Tone]>;

/** A grid of art pixels being painted. See the module comment. */
export class Painter {
    readonly w: number;
    readonly h: number;
    readonly mat: (string | null)[];
    readonly tone: (Tone | null)[];
    /** Pixels a stamp placed: shading, outlines and seams leave them alone. */
    readonly lock: boolean[];

    constructor(w: number, h: number) {
        this.w = w;
        this.h = h;
        this.mat = new Array<string | null>(w * h).fill(null);
        this.tone = new Array<Tone | null>(w * h).fill(null);
        this.lock = new Array<boolean>(w * h).fill(false);
    }

    inside(x: number, y: number): boolean {
        return x >= 0 && y >= 0 && x < this.w && y < this.h;
    }

    set(x: number, y: number, mat: string, tone: Tone = 'b', lock = false): void {
        x = Math.round(x);
        y = Math.round(y);
        if (!this.inside(x, y)) return;
        const i = y * this.w + x;
        this.mat[i] = mat;
        this.tone[i] = tone;
        if (lock) this.lock[i] = true;
    }

    clear(x: number, y: number): void {
        if (!this.inside(x, y)) return;
        const i = y * this.w + x;
        this.mat[i] = null;
        this.tone[i] = null;
        this.lock[i] = false;
    }

    /** The material at `x, y`, or `null` outside the grid or where nothing is painted. */
    m(x: number, y: number): string | null {
        return this.inside(x, y) ? this.mat[y * this.w + x] : null;
    }

    toneAt(x: number, y: number): Tone | null {
        return this.inside(x, y) ? this.tone[y * this.w + x] : null;
    }

    ellipse(cx: number, cy: number, rx: number, ry: number, mat: string, o: EllipseOptions = {}): void {
        const { rot = 0, shade = true, hi = 0.86, sh = 0.28, clip = null, tone = 'b', only = null } = o;
        // Only the pixels whose centres could be inside: the same pixels as testing the
        // whole grid, at a fraction of the cost, which is what lets a pose be painted
        // inside the shell's frame budget.
        const reach = rot === 0 ? [rx, ry] : [Math.max(rx, ry), Math.max(rx, ry)];
        const x0 = Math.max(0, Math.floor(cx - reach[0] - 0.5));
        const x1 = Math.min(this.w - 1, Math.ceil(cx + reach[0] - 0.5));
        const y0 = Math.max(0, Math.floor(cy - reach[1] - 0.5));
        const y1 = Math.min(this.h - 1, Math.ceil(cy + reach[1] - 0.5));
        for (let y = y0; y <= y1; y++) {
            for (let x = x0; x <= x1; x++) {
                if (!inEllipse(x, y, cx, cy, rx, ry, rot)) continue;
                if (clip !== null && !clip(x, y)) continue;
                if (only !== null && !only.includes(this.m(x, y) ?? '')) continue;
                const t = shade ? sphereTone(x, y, cx, cy, rx, ry, hi, sh) : tone;
                this.set(x, y, mat, t);
            }
        }
    }

    circle(cx: number, cy: number, r: number, mat: string, o: EllipseOptions = {}): void {
        this.ellipse(cx, cy, r, r, mat, o);
    }

    rect(x0: number, y0: number, x1: number, y1: number, mat: string, tone: Tone = 'b'): void {
        for (let y = y0; y <= y1; y++) for (let x = x0; x <= x1; x++) this.set(x, y, mat, tone);
    }

    /**
     * Pixels painted from rows of characters: `.` and space leave a pixel alone, `_`
     * clears it, and any other character paints what `key` says it means.
     */
    stamp(x0: number, y0: number, rows: readonly string[], key: StampKey, lock = true): void {
        rows.forEach((row, dy) => {
            [...row].forEach((ch, dx) => {
                if (ch === '.' || ch === ' ') return;
                if (ch === '_') {
                    this.clear(x0 + dx, y0 + dy);
                    return;
                }
                const k = key[ch];
                if (k !== undefined) this.set(x0 + dx, y0 + dy, k[0], k[1], lock);
            });
        });
    }

    /**
     * Every pixel of `mat` shaded again as one sphere, so shapes painted separately --
     * a body and its haunches -- read as one form.
     */
    reshade(mat: string, cx: number, cy: number, rx: number, ry: number, hi = 0.86, sh = 0.28): void {
        for (let y = 0; y < this.h; y++) {
            for (let x = 0; x < this.w; x++) {
                const i = y * this.w + x;
                if (this.mat[i] !== mat || this.lock[i]) continue;
                const nx = Math.max(-1, Math.min(1, (x + 0.5 - cx) / rx));
                const ny = Math.max(-1, Math.min(1, (y + 0.5 - cy) / ry));
                const r = Math.hypot(nx, ny);
                const k = r > 0.999 ? 0.999 / r : 1;
                this.tone[i] = sphereTone(cx + nx * k * rx - 0.5, cy + ny * k * ry - 0.5, cx, cy, rx, ry, hi, sh);
            }
        }
    }

    /** The silhouette: every painted pixel with an empty side takes its material's `o`. */
    outline(except: readonly string[] = []): void {
        const out: number[] = [];
        for (let y = 0; y < this.h; y++) {
            for (let x = 0; x < this.w; x++) {
                const i = y * this.w + x;
                const mt = this.mat[i];
                if (mt === null || this.lock[i] || except.includes(mt)) continue;
                if (!this.m(x - 1, y) || !this.m(x + 1, y) || !this.m(x, y - 1) || !this.m(x, y + 1)) out.push(i);
            }
        }
        for (const i of out) this.tone[i] = 'o';
    }

    /** An inner line: pixels of `mat` touching `other` take `tone`. */
    seam(mat: string, other: string, tone: Tone = 's'): void {
        const hits: number[] = [];
        for (let y = 0; y < this.h; y++) {
            for (let x = 0; x < this.w; x++) {
                const i = y * this.w + x;
                if (this.mat[i] !== mat || this.lock[i] || this.tone[i] === 'o') continue;
                if (
                    this.m(x, y + 1) === other ||
                    this.m(x, y - 1) === other ||
                    this.m(x - 1, y) === other ||
                    this.m(x + 1, y) === other
                )
                    hits.push(i);
            }
        }
        for (const i of hits) this.tone[i] = tone;
    }

    /** The finished grid, in `palette`'s colours. Throws on a material it has no colour for. */
    toGrid(palette: Palette): Grid {
        const px = new Array<string | null>(this.w * this.h).fill(null);
        for (let i = 0; i < px.length; i++) {
            const mt = this.mat[i];
            if (mt === null) continue;
            const p = palette[mt];
            if (p === undefined) throw new Error(`no palette entry for ${mt}`);
            const tone = this.tone[i] ?? 'b';
            px[i] = p[tone] ?? p.b;
        }
        return { w: this.w, h: this.h, px };
    }
}

/** A grid turned by `quarters` quarter turns clockwise, for a pet that rolls. */
export function rotateGrid(grid: Grid, quarters: number): Grid {
    let { w, h, px } = grid;
    const turns = ((quarters % 4) + 4) % 4;
    for (let r = 0; r < turns; r++) {
        const out = new Array<string | null>(px.length);
        for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) out[x * h + (h - 1 - y)] = px[y * w + x];
        px = out;
        [w, h] = [h, w];
    }
    return { w, h, px };
}

/** A grid mirrored left to right. */
export function mirrorGrid(grid: Grid): Grid {
    const px = new Array<string | null>(grid.px.length);
    for (let y = 0; y < grid.h; y++)
        for (let x = 0; x < grid.w; x++) px[y * grid.w + x] = grid.px[y * grid.w + (grid.w - 1 - x)];
    return { w: grid.w, h: grid.h, px };
}

/** Stamp rows mirrored left to right. */
export function mirrorRows(rows: readonly string[]): string[] {
    return rows.map(r => [...r].reverse().join(''));
}

/**
 * A small grid from rows of characters and a colour for each, for the pieces that are not
 * pets: a heart, a note, a spark.
 */
export function gridFromRows(rows: readonly string[], colours: Record<string, string>): Grid {
    const w = Math.max(...rows.map(r => r.length));
    const px: (string | null)[] = [];
    for (const row of rows) {
        for (let x = 0; x < w; x++) {
            const ch = row[x] ?? '.';
            px.push(ch === '.' || ch === ' ' ? null : (colours[ch] ?? null));
        }
    }
    return { w, h: rows.length, px };
}
