// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Selection geometry for the capture overlay: `CAP-02`, specified in `spec/03` §5.1.
 *
 * **This is a deliberate mirror of `crates/core/src/selection.rs`.** The same arithmetic
 * exists twice because the overlay runs in the shell and the editor and history run in
 * the app, and both must agree about what a selection is. `spec/10` §11 asks for vitest
 * on exactly this kind of pure module; `selection.test.ts` re-runs the Rust test cases
 * with the same expected values, so a divergence fails a test rather than producing a
 * capture whose pixels do not match its label.
 *
 * Imports nothing from `gi://`, on purpose: it has to run under Node to be tested.
 *
 * Integer discipline matters here. The Rust side is `i32`, so every division truncates
 * and every ratio result is rounded. JavaScript numbers are doubles, so each of those
 * points is explicit below. Non-integer coordinates would put the selection frame on a
 * half-pixel and make the dimension label disagree with the resulting PNG.
 */

export interface Rect {
    x: number;
    y: number;
    width: number;
    height: number;
}

export function rect(x: number, y: number, width: number, height: number): Rect {
    return { x, y, width, height };
}

export function isEmpty(r: Rect): boolean {
    return r.width <= 0 || r.height <= 0;
}

/** `spec/03` §5.1: "Click without drag (movement < 4 px)". */
export const CLICK_THRESHOLD = 4;

/** `spec/03` §4: handles are 8x8 px with a 20 px hit area, so ±10 around the centre. */
export const HANDLE_HIT_RADIUS = 10;

/** `spec/03` §5.1: arrows nudge 1 px, Shift+arrows 10 px. */
export const STEP = 1;
export const STEP_FAST = 10;

export type Handle =
    | 'top-left'
    | 'top'
    | 'top-right'
    | 'right'
    | 'bottom-right'
    | 'bottom'
    | 'bottom-left'
    | 'left';

/** In the order `spec/03` §4 lists them. */
export const HANDLES: readonly Handle[] = [
    'top-left',
    'top',
    'top-right',
    'right',
    'bottom-right',
    'bottom',
    'bottom-left',
    'left',
];

export type Hit =
    | { kind: 'handle'; handle: Handle }
    | { kind: 'inside' }
    | { kind: 'outside' };

export interface Modifiers {
    /** Shift: lock the aspect ratio. */
    aspectLock: boolean;
    /** Alt: treat the anchor as the centre rather than a corner. */
    fromCentre: boolean;
}

export const NO_MODIFIERS: Modifiers = { aspectLock: false, fromCentre: false };

export type Point = readonly [number, number];

const movesLeft = (h: Handle) => h === 'top-left' || h === 'left' || h === 'bottom-left';
const movesRight = (h: Handle) => h === 'top-right' || h === 'right' || h === 'bottom-right';
const movesTop = (h: Handle) => h === 'top-left' || h === 'top' || h === 'top-right';
const movesBottom = (h: Handle) =>
    h === 'bottom-left' || h === 'bottom' || h === 'bottom-right';

export function handleCentre(r: Rect, handle: Handle): Point {
    const left = r.x;
    const top = r.y;
    const right = r.x + r.width;
    const bottom = r.y + r.height;
    // Math.trunc, not a bare division: the Rust side is i32 and truncates.
    const midX = r.x + Math.trunc(r.width / 2);
    const midY = r.y + Math.trunc(r.height / 2);

    switch (handle) {
        case 'top-left':
            return [left, top];
        case 'top':
            return [midX, top];
        case 'top-right':
            return [right, top];
        case 'right':
            return [right, midY];
        case 'bottom-right':
            return [right, bottom];
        case 'bottom':
            return [midX, bottom];
        case 'bottom-left':
            return [left, bottom];
        case 'left':
            return [left, midY];
    }
}

/**
 * `spec/03` §5.1 fixes the precedence: handles first, then the interior, then a new
 * selection. Handles come first because their hit areas overhang the selection edge --
 * without that, a corner sitting on the screen edge cannot be grabbed.
 */
export function hitTest(r: Rect | null, point: Point): Hit {
    if (r === null) return { kind: 'outside' };

    for (const handle of HANDLES) {
        const [hx, hy] = handleCentre(r, handle);
        if (
            Math.abs(point[0] - hx) <= HANDLE_HIT_RADIUS &&
            Math.abs(point[1] - hy) <= HANDLE_HIT_RADIUS
        )
            return { kind: 'handle', handle };
    }

    if (
        point[0] >= r.x &&
        point[0] <= r.x + r.width &&
        point[1] >= r.y &&
        point[1] <= r.y + r.height
    )
        return { kind: 'inside' };

    return { kind: 'outside' };
}

/**
 * Applies an aspect ratio by growing the smaller axis, so the selection always contains
 * the pointer's dominant extent and Shift feels like it is following the drag.
 */
function applyRatio(width: number, height: number, ratio: number): [number, number] {
    if (ratio <= 0) return [width, height];
    if (height === 0 || width / height >= ratio)
        return [width, Math.round(width / ratio)];
    return [Math.round(height * ratio), height];
}

/**
 * The ratio a drag keeps, as width/height, or `null` for none (`spec/03` §5.1). The
 * toolbar's lock or ratio holds for as long as it is set, Shift or not. Otherwise it is
 * the ratio the selection had when Shift went down, kept until Shift comes up. `held` is
 * the ratio kept so far and `current` the selection as it is.
 *
 * The engine used to drop the ratio whenever Shift was up, the toolbar's with it, and to
 * apply one only while Shift was held: a plain drag after choosing 16:9 came out free
 * while the toolbar still said 16:9 (D136).
 */
export function dragRatio(
    shift: boolean,
    toolbarLocked: boolean,
    held: number | null,
    current: Rect | null,
): number | null {
    if (toolbarLocked) return held;
    if (!shift) return null;
    if (held !== null) return held;
    return current !== null && current.height > 0 ? current.width / current.height : null;
}

/**
 * Builds a selection from a drag. `lockedRatio` is width/height and is supplied by the
 * caller, because `spec/03` §5.1 says the ratio comes from the moment Shift was pressed
 * or from the toolbar -- the geometry does not choose it. See [`dragRatio`].
 */
export function rectFromDrag(
    anchor: Point,
    pointer: Point,
    modifiers: Modifiers = NO_MODIFIERS,
    lockedRatio: number | null = null,
): Rect {
    let dx = Math.abs(pointer[0] - anchor[0]);
    let dy = Math.abs(pointer[1] - anchor[1]);

    if (modifiers.aspectLock && lockedRatio !== null)
        [dx, dy] = applyRatio(dx, dy, lockedRatio);

    if (modifiers.fromCentre) {
        // Alt: the anchor is the centre, so the drag distance is a half-extent.
        return rect(anchor[0] - dx, anchor[1] - dy, dx * 2, dy * 2);
    }

    const x = pointer[0] < anchor[0] ? anchor[0] - dx : anchor[0];
    const y = pointer[1] < anchor[1] ? anchor[1] - dy : anchor[1];
    return rect(x, y, dx, dy);
}

/**
 * Resizes by dragging a handle. Corner handles keep the opposite corner fixed, edge
 * handles the opposite edge. Dragging past the far edge flips the rect rather than
 * producing a negative size.
 */
export function resizeByHandle(
    r: Rect,
    handle: Handle,
    pointer: Point,
    modifiers: Modifiers = NO_MODIFIERS,
    lockedRatio: number | null = null,
): Rect {
    let left = r.x;
    let top = r.y;
    let right = r.x + r.width;
    let bottom = r.y + r.height;

    if (movesLeft(handle)) left = pointer[0];
    if (movesRight(handle)) right = pointer[0];
    if (movesTop(handle)) top = pointer[1];
    if (movesBottom(handle)) bottom = pointer[1];

    let out = rect(
        Math.min(left, right),
        Math.min(top, bottom),
        Math.abs(right - left),
        Math.abs(bottom - top),
    );

    if (modifiers.aspectLock && lockedRatio !== null) {
        const [w, h] = applyRatio(out.width, out.height, lockedRatio);
        // Grow away from the fixed edge so the anchor stays put.
        if (movesLeft(handle)) out.x -= w - out.width;
        if (movesTop(handle)) out.y -= h - out.height;
        out.width = w;
        out.height = h;
    }

    if (modifiers.fromCentre) {
        const cx = r.x + Math.trunc(r.width / 2);
        const cy = r.y + Math.trunc(r.height / 2);
        const halfW = Math.trunc(Math.max(out.width, 1) / 2);
        const halfH = Math.trunc(Math.max(out.height, 1) / 2);
        out = rect(cx - halfW, cy - halfH, halfW * 2, halfH * 2);
    }

    return out;
}

/**
 * Clamps a selection inside one monitor. `spec/01` §1: a selection spanning two
 * differently-scaled monitors is refused, so clamping is always to a single monitor.
 * Position moves before size shrinks, so dragging off the edge slides the selection.
 */
export function clampToMonitor(r: Rect, monitor: Rect): Rect {
    const width = Math.min(r.width, monitor.width);
    const height = Math.min(r.height, monitor.height);
    const x = clamp(r.x, monitor.x, monitor.x + monitor.width - width);
    const y = clamp(r.y, monitor.y, monitor.y + monitor.height - height);
    return rect(x, y, width, height);
}

function clamp(value: number, low: number, high: number): number {
    return Math.min(Math.max(value, low), high);
}

function overlapArea(a: Rect, b: Rect): number {
    const x = Math.min(a.x + a.width, b.x + b.width) - Math.max(a.x, b.x);
    const y = Math.min(a.y + a.height, b.y + b.height) - Math.max(a.y, b.y);
    return x <= 0 || y <= 0 ? 0 : x * y;
}

/**
 * The part of `a` that lies inside `b`, or `null` when they do not overlap.
 *
 * The overlay's dim needs this per monitor: each root arranges its four dim pieces around
 * **its own** share of the selection, so a root the selection never touches dims all of
 * itself instead of tiling around a rect that is somewhere else entirely.
 */
export function intersect(a: Rect, b: Rect): Rect | null {
    const x = Math.max(a.x, b.x);
    const y = Math.max(a.y, b.y);
    const right = Math.min(a.x + a.width, b.x + b.width);
    const bottom = Math.min(a.y + a.height, b.y + b.height);
    if (right <= x || bottom <= y) return null;
    return rect(x, y, right - x, bottom - y);
}

/**
 * The four dim rectangles for one monitor, in **monitor-local** coordinates.
 *
 * `spec/03` §4 asks for "uniform translucent black outside the selection", and the user's
 * words for the requirement are better than the spec's: *everything that's not the
 * selection is greyed out.* Four rects -- above, below, left, right -- because a
 * rectangle with a rectangular hole is exactly four rectangles, and because moving four
 * existing actors costs nothing on the drag path.
 *
 * Pure and tested here rather than inline in `root.ts` because the invariant is worth
 * asserting and the failure is invisible in a screenshot: `screenshotUIGroup` is excluded
 * from captures (`docs/decisions.md` D22), so the only way this gets checked without a
 * person looking at two physical monitors is arithmetic. The bug that prompted the
 * extraction had one monitor computing a **2700 px wide** dim piece for a 2560 px screen.
 *
 * Guarantees, all asserted in `selection.test.ts`:
 *
 * - every piece lies inside the monitor;
 * - the pieces do not overlap each other;
 * - together they cover the monitor exactly, minus the selection's own share of it;
 * - a monitor the selection does not touch is covered by one full-monitor piece.
 */
export function dimPieces(monitor: Rect, selection: Rect | null): [Rect, Rect, Rect, Rect] {
    const whole: [Rect, Rect, Rect, Rect] = [
        rect(0, 0, monitor.width, monitor.height),
        rect(0, 0, 0, 0),
        rect(0, 0, 0, 0),
        rect(0, 0, 0, 0),
    ];
    if (selection === null) return whole;
    const part = intersect(selection, monitor);
    if (part === null) return whole;

    const left = part.x - monitor.x;
    const top = part.y - monitor.y;
    const right = left + part.width;
    const bottom = top + part.height;

    return [
        rect(0, 0, monitor.width, top),
        rect(0, bottom, monitor.width, monitor.height - bottom),
        rect(0, top, left, part.height),
        rect(right, top, monitor.width - right, part.height),
    ];
}

/** The monitor holding the largest share of the selection, or null if none does. */
export function dominantMonitor(r: Rect, monitors: readonly Rect[]): number | null {
    let best: number | null = null;
    let bestArea = 0;
    monitors.forEach((monitor, index) => {
        const area = overlapArea(r, monitor);
        if (area > bestArea) {
            bestArea = area;
            best = index;
        }
    });
    return best;
}

/** A monitor's logical rect plus its scale, for [`resolveRectScale`]. */
export interface ScaledMonitor {
    rect: Rect;
    scale: number;
}

/** What a caller-supplied rect resolves to, or why it cannot be captured. */
export type RectResolution =
    | { ok: true; scale: number; monitor: number }
    | { ok: false; reason: string };

/**
 * Decides the scale for a rect that did **not** come from the overlay, and refuses the
 * cases `spec/01` §1 says must be refused.
 *
 * The overlay clamps a drag to one monitor as it happens, so a selection can never span
 * two. An explicit rect -- from the CLI, a `octosnap://` link, or `previous-area` after
 * the displays have been rearranged -- bypasses that entirely, and at mixed scale the
 * result is a capture whose size nothing can predict. Measured on a real two-monitor rig
 * (eDP-1 1536x960 @1.25 at (443,1440), DP-6 2560x1440 @1 at (0,0)): a logical 600x400
 * rect straddling the boundary produced a **750x500** PNG while the twin claimed scale 1,
 * because `Shell.Screenshot.screenshot_area` picks its own scale across the span. That
 * breaks `spec/10` §3.3's promise that `rect` x `scale` is the file's size, and with it
 * `ACT-07`'s `{w}`/`{h}` tokens.
 *
 * A rect spanning two monitors of the **same** scale is fine and is allowed: there is no
 * ambiguity, and a shot across two matched displays is a reasonable thing to ask for.
 *
 * Mirrors nothing in Rust yet -- the app never resolves a rect itself -- but the rule is
 * `spec/01` §1's and `crates/core/src/selection.rs`'s `dominant_monitor` is its sibling.
 */
export function resolveRectScale(
    r: Rect,
    monitors: readonly ScaledMonitor[],
): RectResolution {
    const touched: number[] = [];
    monitors.forEach((monitor, index) => {
        if (overlapArea(r, monitor.rect) > 0) touched.push(index);
    });

    if (touched.length === 0)
        return { ok: false, reason: 'the rect is not on any monitor' };

    const scales = new Set(touched.map(i => monitors[i]!.scale));
    if (scales.size > 1) {
        const list = touched.map(i => scaleLabel(monitors[i]!.scale)).join(' and ');
        return {
            ok: false,
            reason:
                `the rect spans monitors at different scales (${list}); ` +
                'capture it on one display at a time',
        };
    }

    // The dominant monitor among equals, so the twin names the one holding most of it.
    const dominant = dominantMonitor(r, monitors.map(m => m.rect)) ?? touched[0]!;
    return { ok: true, scale: monitors[dominant]!.scale, monitor: dominant };
}

/** The scales of the monitors a rect lies on, largest first; none for a rect on none. */
export function scalesUnder(r: Rect, monitors: readonly ScaledMonitor[]): number[] {
    const on = monitors.filter(m => overlapArea(r, m.rect) > 0).map(m => m.scale);
    return [...new Set(on)].sort((a, b) => b - a);
}

/**
 * The scales a window's pixels could be at (D138): 1, each monitor's under it, and the
 * whole number above each.
 *
 * A window is captured from its own buffer, and its client chose the scale of that: the
 * monitor's for a client drawing at fractional scales, and the next whole number for one
 * that does not. The nested shell's software-drawn GTK gets a 2x buffer on a 125 % monitor
 * and a 3x one on a 250 % monitor, and an X11 client through Xwayland was 1x on a 100 %
 * monitor and 2x on a 125 % one. Across two monitors it is the larger scale. Taking the
 * monitor's scale made a twin 813x558 for a 508x349 window on a 125 % monitor, and twice a
 * window's size across a 100 % and a 200 % one.
 */
function windowScales(scales: readonly number[]): number[] {
    return [...new Set([1, ...scales, ...scales.map(s => Math.ceil(s))])];
}

/**
 * The scale at which a window capture's PNG is exactly the window's buffer, or `null` when
 * it is at none (D138).
 *
 * A Wayland client's image is its buffer and nothing else, so its size is the buffer rect's
 * at the client's scale, to a pixel of rounding. That is exact where [`imageScale`] has to
 * judge a margin, and it keeps the buffer's own origin for the twin.
 */
export function bufferScale(
    buffer: { width: number; height: number },
    png: { width: number; height: number },
    scales: readonly number[],
): number | null {
    let found: number | null = null;
    for (const scale of windowScales(scales)) {
        const width = Math.round(Math.fround(buffer.width * scale));
        const height = Math.round(Math.fround(buffer.height * scale));
        if (Math.abs(png.width - width) > 1 || Math.abs(png.height - height) > 1) continue;
        if (found === null || scale > found) found = scale;
    }
    return found;
}

/**
 * The scale a window capture's pixels are at, told from the file and the window's frame
 * (D138), or `null` when the file fits none. For an image that is more than the buffer,
 * which [`bufferScale`] cannot place.
 *
 * The PNG is the window with a margin round its frame: the shadow, the compositor's or the
 * client's own. A shadow is about as wide on every side, so of the scales the window could be
 * at, it is one at which the file is the frame with an even margin, and of those the
 * largest: at a smaller one, the margin would have to be the window drawn larger. Even to a
 * logical pixel and a physical one: a client's shadow falls a little lower than it spreads,
 * and GTK 4's is 14 at each side, 12 above and 17 below. With a physical pixel alone that
 * missed the 3x buffer on a 250 % monitor. No Rust twin: the app is handed the scale in the
 * twin and never sees a window's buffer.
 */
export function imageScale(
    frame: { width: number; height: number },
    png: { width: number; height: number },
    scales: readonly number[],
): number | null {
    let found: number | null = null;
    for (const scale of windowScales(scales)) {
        const marginX = (png.width / scale - frame.width) / 2;
        const marginY = (png.height / scale - frame.height) / 2;
        // A physical pixel of rounding, in logical ones.
        const slack = 1 / scale;
        if (marginX < -slack || marginY < -slack || Math.abs(marginX - marginY) > 1 + slack) continue;
        if (found === null || scale > found) found = scale;
    }
    return found;
}

export type Direction = 'left' | 'right' | 'up' | 'down';

export function nudge(r: Rect, direction: Direction, step: number): Rect {
    const dx = direction === 'left' ? -step : direction === 'right' ? step : 0;
    const dy = direction === 'up' ? -step : direction === 'down' ? step : 0;
    return rect(r.x + dx, r.y + dy, r.width, r.height);
}

/**
 * Resizes from the bottom-right. A selection never shrinks below 1x1: `spec/03` §5.1
 * says 1x1 is legal, and zero is not a selection at all.
 */
export function resizeByKey(r: Rect, direction: Direction, step: number): Rect {
    const dw = direction === 'left' ? -step : direction === 'right' ? step : 0;
    const dh = direction === 'up' ? -step : direction === 'down' ? step : 0;
    return rect(r.x, r.y, Math.max(r.width + dw, 1), Math.max(r.height + dh, 1));
}

/** Sets an exact width, anchored at the top-left. The lock drives the other field. */
export function setWidth(r: Rect, width: number, lockedRatio: number | null = null): Rect {
    const w = Math.max(width, 1);
    const h =
        lockedRatio !== null && lockedRatio > 0
            ? Math.max(Math.round(w / lockedRatio), 1)
            : r.height;
    return rect(r.x, r.y, w, h);
}

/** Sets an exact height, anchored at the top-left. */
export function setHeight(r: Rect, height: number, lockedRatio: number | null = null): Rect {
    const h = Math.max(height, 1);
    const w =
        lockedRatio !== null && lockedRatio > 0
            ? Math.max(Math.round(h * lockedRatio), 1)
            : r.width;
    return rect(r.x, r.y, w, h);
}

/** True when a press-release pair was a click rather than a drag. */
export function isClick(anchor: Point, release: Point): boolean {
    return (
        Math.abs(release[0] - anchor[0]) < CLICK_THRESHOLD &&
        Math.abs(release[1] - anchor[1]) < CLICK_THRESHOLD
    );
}

/**
 * The item `step` places after `current`, wrapping at both ends: Tab and Shift+Tab through
 * the All-In-One toolbar's modes and through the window picker's windows (D137). From
 * nothing, or from something no longer in the list, a step forward lands on the first
 * item and a step back on the last.
 */
export function cycle<T>(items: readonly T[], current: T | null, step: number): T | null {
    const count = items.length;
    if (count === 0) return null;
    const at = current === null ? -1 : items.indexOf(current);
    if (at < 0) return (step >= 0 ? items[0] : items[count - 1]) ?? null;
    return items[(((at + step) % count) + count) % count] ?? null;
}

/**
 * The middle half of a monitor, which the keyboard starts from when there is no
 * selection. Arrows and Tab act on a selection, and with none the only way to make one
 * was the pointer (D137).
 */
export function middleHalf(monitor: Rect): Rect {
    const width = Math.max(Math.round(monitor.width / 2), 1);
    const height = Math.max(Math.round(monitor.height / 2), 1);
    return rect(
        monitor.x + Math.round((monitor.width - width) / 2),
        monitor.y + Math.round((monitor.height - height) / 2),
        width,
        height,
    );
}

/**
 * A monitor scale as a person reads it: `133%`, not `1.3333333730697632x`.
 *
 * The reason this exists is that the refusal message in `resolveRectScale` is shown to the
 * user, and the raw number is what Mutter reports -- a `float32` approximation of 4/3,
 * printed in full. "spans monitors at different scales (1.3333333730697632x and 1x)" is
 * technically complete and unreadable. GNOME's own Displays panel says 133 %, so the
 * message now speaks the same language as the setting the user would go and change.
 */
export function scaleLabel(scale: number): string {
    return `${Math.round(scale * 100)}%`;
}

/**
 * Converts a logical rect to physical pixels, **rounding to nearest as Mutter does**
 * (`spec/01` §1). Mirrors `Rect::to_physical`; `docs/decisions.md` D23 is why it is
 * nearest rather than outward.
 */
export function toPhysical(r: Rect, scale: number): [number, number] {
    return [
        Math.max(Math.round(r.width * scale), 0),
        Math.max(Math.round(r.height * scale), 0),
    ];
}
