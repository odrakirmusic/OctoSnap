// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * `spec/07` §3.1's pin shadow, as pixels: what `pinShadow.ts` puts around a pinned
 * screenshot (D132). No `gi://` imports, so it is tested under Node like `repaint.ts`.
 *
 * **Why pixels, and why pieces.** The pin's window is the capture's rect to the pixel
 * (D66), so the shadow is drawn by the compositor, outside it. A texture the size of the
 * shadow would be the pin's size again: 9 MB for a 1920×1200 pin at scale 1 and 37 MB at 2,
 * against the extension's 10 MB budget (`spec/10` §7). St's own `box-shadow` is no better,
 * since it blurs a texture that size on the CPU. But a shadow is the same along a straight
 * edge, so eight pieces carry all of it: four corners, whatever the pin's size, and four
 * strips one pixel long, stretched along the edges. About 25 kB at scale 1 and 100 kB at 2,
 * for any pin, and the same pieces serve every pin at that radius and scale.
 *
 * **What a pixel holds.** Each layer is the pin's rounded rect, moved down by the layer's
 * offset and blurred. A pixel's share of it is the Gaussian's integral beyond the pixel's
 * signed distance from that shape. For a straight edge that is exactly a blur. At a corner
 * it is a little fuller than a blur, which reads as the same shadow. The layers are laid
 * over each other, and nothing is kept where the pin is: under a pin made translucent
 * (§3.1's opacity), a shadow would darken the pin.
 */

import type { Rect } from './place.js';

/** One layer of a drop shadow: CSS's `box-shadow` without the spread or the colour. */
export interface ShadowLayer {
    /** CSS's blur radius, twice the Gaussian's standard deviation. */
    blur: number;
    /** How far down the layer falls. Not negative. */
    offsetY: number;
    /** The layer's opacity, black, where it is fully in shadow. */
    alpha: number;
}

/**
 * `spec/07` §3.1's shadow: a broad, soft one that lifts the pin off the desktop, and a
 * tight one that keeps its edge against a background of the same colour. Close to what
 * libadwaita gives a window, a little lighter, because a pin is usually small and over
 * work.
 */
export const PIN_SHADOW: readonly ShadowLayer[] = [
    { blur: 20, offsetY: 4, alpha: 0.24 },
    { blur: 4, offsetY: 1, alpha: 0.2 },
];

/** How far a shadow reaches past each side of the pin, in logical pixels. */
export interface Margins {
    top: number;
    right: number;
    bottom: number;
    left: number;
}

/**
 * Three standard deviations of each layer, which is where a Gaussian's tail is under a
 * two-thousandth: less than one step of an 8-bit alpha at any of these opacities.
 */
export function shadowMargins(layers: readonly ShadowLayer[]): Margins {
    let side = 0;
    let top = 0;
    let bottom = 0;
    for (const layer of layers) {
        const reach = 1.5 * layer.blur;
        side = Math.max(side, reach);
        top = Math.max(top, reach - layer.offsetY);
        bottom = Math.max(bottom, reach + layer.offsetY);
    }
    return {
        top: Math.ceil(Math.max(0, top)),
        right: Math.ceil(side),
        bottom: Math.ceil(bottom),
        left: Math.ceil(side),
    };
}

/**
 * Signed distance from a point to a rounded rect: negative inside, in the point's units.
 * `radius` is clamped to what the rect can hold.
 */
export function roundedRectDistance(
    x: number,
    y: number,
    rect: Rect,
    radius: number,
): number {
    const halfW = rect.width / 2;
    const halfH = rect.height / 2;
    const r = Math.max(0, Math.min(radius, halfW, halfH));
    const qx = Math.abs(x - (rect.x + halfW)) - (halfW - r);
    const qy = Math.abs(y - (rect.y + halfH)) - (halfH - r);
    const outside = Math.hypot(Math.max(qx, 0), Math.max(qy, 0));
    return outside + Math.min(Math.max(qx, qy), 0) - r;
}

/**
 * The complementary error function, to about 1e-7 (Abramowitz and Stegun, 7.1.26). The
 * shadow needs a few thousand of these for a pin's corners, once per scale and radius.
 */
export function erfc(x: number): number {
    const z = Math.abs(x);
    const t = 1 / (1 + 0.3275911 * z);
    const poly =
        t * (0.254829592 + t * (-0.284496736 + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429))));
    const tail = poly * Math.exp(-z * z);
    return x >= 0 ? tail : 2 - tail;
}

/**
 * The shadow's opacity at a point of a pin of `width`×`height` with corners of `radius`,
 * in the pin's own logical coordinates, before the pin is taken out of it.
 */
export function shadowAt(
    x: number,
    y: number,
    width: number,
    height: number,
    radius: number,
    layers: readonly ShadowLayer[],
): number {
    let clear = 1;
    for (const layer of layers) {
        const sigma = layer.blur / 2;
        const shape = { x: 0, y: layer.offsetY, width, height };
        const d = roundedRectDistance(x, y, shape, radius);
        // The half-plane's blur at distance `d`: a half at the edge, all of it deep inside.
        const share = sigma > 0 ? 0.5 * erfc(d / (sigma * Math.SQRT2)) : d <= 0 ? 1 : 0;
        clear *= 1 - layer.alpha * share;
    }
    return 1 - clear;
}

/**
 * How much of a device pixel the pin covers: its antialiased edge, so that the shadow
 * meets the pin's own rounded corner without a seam. `d` is the signed distance in
 * logical pixels and `scale` the device pixels to one of them.
 */
function pinCoverage(d: number, scale: number): number {
    return Math.min(1, Math.max(0, 0.5 - d * scale));
}

/**
 * A device pixel's alpha, 0 to 255, centred at `x`,`y` in the pin's logical coordinates:
 * the shadow, less as much of it as the pin covers.
 */
export function shadowAlpha(
    x: number,
    y: number,
    pin: PinShape,
    scale: number,
    layers: readonly ShadowLayer[],
): number {
    const shadow = shadowAt(x, y, pin.width, pin.height, pin.radius, layers);
    const d = roundedRectDistance(x, y, { x: 0, y: 0, width: pin.width, height: pin.height }, pin.radius);
    return Math.round(255 * shadow * (1 - pinCoverage(d, scale)));
}

/** One piece of the shadow, placed relative to the pin's top-left in logical pixels. */
export interface ShadowPiece {
    /** Which piece, for the cache and the log. */
    name: 'top-left' | 'top' | 'top-right' | 'left' | 'right' | 'bottom-left' | 'bottom' | 'bottom-right' | 'whole';
    /** Where the piece goes. */
    rect: Rect;
    /** The texture, in device pixels; a strip is one pixel long along its length. */
    width: number;
    height: number;
    /** Premultiplied RGBA, all black but for alpha. */
    pixels: Uint8Array;
}

/** A pin's size and corner radius, in logical pixels. */
export interface PinShape {
    width: number;
    height: number;
    radius: number;
}

/**
 * The pieces' rects for a pin, or `null` when the pin is too small for straight edges to
 * stretch, which `wholeShadow` is for. Rects only: the pixels of the eight pieces do not
 * depend on the pin's size, so they are made once per radius and scale (`piecePixels`).
 */
export function pieceRects(
    pin: PinShape,
    layers: readonly ShadowLayer[],
): Record<Exclude<ShadowPiece['name'], 'whole'>, Rect> | null {
    const m = shadowMargins(layers);
    const { width: w, height: h } = pin;
    const r = Math.max(0, Math.min(pin.radius, w / 2, h / 2));
    // Everything a strip stretches has to be the same all the way along it, and two
    // things end sooner than the pin's straight edges do. Down the sides, a layer's own
    // corner ends its offset lower than the pin's, so the side strips start below both.
    // Along the bottom, a layer's shape reaches `drop` past the pin, and a pixel in that
    // band within `drop` of a side is nearer the side than the bottom edge: so the corners
    // reach at least that far along it, even when the pin's corners are square.
    const drop = Math.max(0, ...layers.map(l => l.offsetY));
    const inset = Math.max(r, drop);
    const top = r + drop;
    const bottom = h - r;
    if (w - 2 * inset <= 0 || bottom - top <= 0) return null;

    return {
        'top-left': { x: -m.left, y: -m.top, width: m.left + inset, height: m.top + top },
        top: { x: inset, y: -m.top, width: w - 2 * inset, height: m.top },
        'top-right': { x: w - inset, y: -m.top, width: inset + m.right, height: m.top + top },
        left: { x: -m.left, y: top, width: m.left, height: bottom - top },
        right: { x: w, y: top, width: m.right, height: bottom - top },
        'bottom-left': { x: -m.left, y: bottom, width: m.left + inset, height: r + m.bottom },
        bottom: { x: inset, y: h, width: w - 2 * inset, height: m.bottom },
        'bottom-right': { x: w - inset, y: bottom, width: inset + m.right, height: r + m.bottom },
    };
}

/**
 * The pixels of one area of a pin's shadow: `rect` in the pin's logical coordinates, as a
 * texture of `columns`×`rows` device pixels sampled at their centres.
 */
function render(
    rect: Rect,
    columns: number,
    rows: number,
    pin: PinShape,
    scale: number,
    layers: readonly ShadowLayer[],
): Uint8Array {
    const pixels = new Uint8Array(columns * rows * 4);
    for (let j = 0; j < rows; j++) {
        const y = rect.y + ((j + 0.5) * rect.height) / rows;
        for (let i = 0; i < columns; i++) {
            const x = rect.x + ((i + 0.5) * rect.width) / columns;
            // Premultiplied black: only the alpha is not zero.
            pixels[(j * columns + i) * 4 + 3] = shadowAlpha(x, y, pin, scale, layers);
        }
    }
    return pixels;
}

/** Device pixels for a length, never none. */
function device(length: number, scale: number): number {
    return Math.max(1, Math.round(length * scale));
}

/**
 * The eight pieces' pixels for a corner radius and a scale. The strips are rendered where
 * the pin is long enough for them to be straight, which any length past the corners is:
 * the pin here is only big enough to hold the corners apart, since a strip one pixel long
 * is the same pixel wherever along the edge it is taken.
 */
export function piecePixels(
    radius: number,
    scale: number,
    layers: readonly ShadowLayer[],
): ShadowPiece[] {
    const drop = Math.max(0, ...layers.map(l => l.offsetY));
    // A pin with straight runs just long enough for the strips (`pieceRects` says what
    // that takes), and tall enough that a pixel in the band below it is nearer the bottom
    // of a layer's shape than its top. The corners' rects are the ones any bigger pin gets.
    const inset = Math.max(radius, drop);
    const pin = { width: 2 * inset + 2, height: 2 * radius + 2 * drop + 2, radius };
    const rects = pieceRects(pin, layers);
    if (rects === null) throw new Error('a pin of straight runs has no pieces');
    const pieces: ShadowPiece[] = [];
    for (const [name, rect] of Object.entries(rects) as [Exclude<ShadowPiece['name'], 'whole'>, Rect][]) {
        const alongX = name === 'top' || name === 'bottom';
        const alongY = name === 'left' || name === 'right';
        const width = alongX ? 1 : device(rect.width, scale);
        const height = alongY ? 1 : device(rect.height, scale);
        // A strip is sampled one device pixel long, in the middle of its run.
        const sampled = alongX
            ? { ...rect, x: rect.x + rect.width / 2 - 0.5 / scale, width: 1 / scale }
            : alongY
              ? { ...rect, y: rect.y + rect.height / 2 - 0.5 / scale, height: 1 / scale }
              : rect;
        pieces.push({ name, rect, width, height, pixels: render(sampled, width, height, pin, scale, layers) });
    }
    return pieces;
}

/** A pin too small to have straight edges, as one piece around all of it. */
export function wholeShadow(
    pin: PinShape,
    scale: number,
    layers: readonly ShadowLayer[],
): ShadowPiece {
    const m = shadowMargins(layers);
    const rect = {
        x: -m.left,
        y: -m.top,
        width: m.left + pin.width + m.right,
        height: m.top + pin.height + m.bottom,
    };
    const width = device(rect.width, scale);
    const height = device(rect.height, scale);
    return { name: 'whole', rect, width, height, pixels: render(rect, width, height, pin, scale, layers) };
}
