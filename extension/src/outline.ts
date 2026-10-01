// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Where to draw an outline around a region that is being captured, so that no capture of
 * the region carries it (`docs/decisions.md` D115). No `gi://` imports, so it is tested
 * under Node like `place.ts`.
 *
 * Two outlines use it: the recorder's red frame (`spec/06` §3) and the scroll assist's blue
 * one (D78). An `St.Widget` draws its border inside its own allocation, so each is an actor
 * the size of the region grown outward by the stroke, and the stroke's inner edge meets
 * the region's edge. That edge is in logical pixels and the captures are in physical ones,
 * and where the two grids do not line up they disagree about the pixel the edge cuts:
 *
 * - the compositor fills a pixel when the stroke covers its centre, so a stroke that
 *   reaches three quarters of the way into a pixel fills all of it;
 * - `GrabFrame` takes the region's origin rounded down and its extent rounded to nearest,
 *   the pixels `screenshot_area` took (D23, D116);
 * - the live view (D106) and the recorder crop the monitor's stream, offset and extent
 *   rounded to nearest (`GifPlan::new`, `crates/media/src/gif.rs`).
 *
 * So on the laptop panel at 1.25, a column of blue sat in every scrolling frame with a row
 * under it, and a column of red ran down the left of a GIF (2026-09-24). In the sandbox
 * every scale that is not a whole number did the same, and 1 and 2 never did.
 *
 * Nor is a capture's grid simply `logical x scale`. Each one repaints the stage into a
 * buffer of its own through a GL viewport, which is whole pixels and **truncated**: the
 * buffer starts at its origin times the scale rounded down -- a monitor at 443 on a 1.25
 * panel starts at 553, not 553.75 -- and the stage's length times the scale is cut to a
 * whole number too, so where it is not one, everything lands a little short of where the
 * scale alone puts it. And Mutter multiplies in single precision. On a nested shell this
 * model said where every one of 4 440 measured stroke edges would land; without the
 * truncated length it was a pixel out on 154 of them.
 *
 * So the outline is placed in the captures' own pixels: its inner edge goes on the
 * outermost pixel boundary that any capture of the region can take, and only then is it
 * turned back into the logical units Clutter lays it out in. Where the grids line up, that
 * boundary is the region's own edge, and the outline is exactly what it always was.
 */

import type { Rect, Size } from './place.js';

/** A monitor as `capture.ts`'s `allMonitors` gives it: its logical rect and its scale. */
export interface ScaledMonitor {
    rect: Rect;
    scale: number;
}

/** A logical span along one axis, `[start, end)`. */
interface Span {
    start: number;
    end: number;
}

/** One monitor as one axis sees it. */
interface Along {
    origin: number;
    scale: number;
}

/**
 * The rectangle for the outline *actor*: `rect` grown outward by `stroke`, plus whatever
 * part of a physical pixel keeps the stroke out of every capture of `rect`. Fractional,
 * and Clutter lays it out as it is.
 *
 * `monitors` is all of them. The ones `rect` touches are the ones whose captures can hold
 * it, which is more than any one capture uses -- the live view and the recorder read one
 * monitor -- and keeping clear of all of them costs at most the pixel each would round
 * to. `stage` is the stage's logical size, which the repaints are made to.
 */
export function outlineRect(
    rect: Rect,
    stroke: number,
    monitors: readonly ScaledMonitor[],
    stage: Size,
): Rect {
    const under = monitors.filter(m => overlaps(m.rect, rect));
    const xs = under.map(m => ({ origin: m.rect.x, scale: m.scale }));
    const ys = under.map(m => ({ origin: m.rect.y, scale: m.scale }));
    const across = clearOf(rect.x, rect.width, stage.width, xs);
    const down = clearOf(rect.y, rect.height, stage.height, ys);
    return {
        x: across.start - stroke,
        y: down.start - stroke,
        width: across.end - across.start + 2 * stroke,
        height: down.end - down.start + 2 * stroke,
    };
}

/**
 * Along one axis: the logical span of every physical pixel that some capture of
 * `[start, start + length)` takes. The stroke goes outside it.
 */
function clearOf(
    start: number,
    length: number,
    stageLength: number,
    monitors: readonly Along[],
): Span {
    // Nothing captures a region that no monitor shows, so it keeps its own edges.
    if (monitors.length === 0) return { start, end: start + length };
    // `GrabFrame`: the whole region at the highest scale under it, as
    // `clutter_stage_get_capture_final_size` picks it; origin down, extent to nearest.
    const shotScale = Math.max(1, ...monitors.map(m => m.scale));
    const shotAt = Math.floor(single(start * shotScale));
    const shotEnd = shotAt + Math.round(single(length * shotScale));
    const span = logicalSpan(shotAt, shotEnd, shotScale, stageLength);
    for (const { origin, scale } of monitors) {
        // The live view and the recorder: the monitor's stream, whose first pixel is the
        // monitor's origin rounded down, cropped the way `GifPlan::new` crops it.
        const at = Math.floor(single(origin * scale)) + roundAway((start - origin) * scale);
        const stream = logicalSpan(at, at + roundAway(length * scale), scale, stageLength);
        span.start = Math.min(span.start, stream.start);
        span.end = Math.max(span.end, stream.end);
    }
    return span;
}

/**
 * Where the physical pixels `[first, end)` of a capture at `scale` lie, in logical units.
 *
 * Not `pixel / scale`: the repaint's viewport is the stage's length times the scale,
 * truncated, so a logical coordinate lands at `x * truncated / stageLength`. At 4/3 on a
 * stage 4 000 wide that is 5 333 pixels for 5 333.33, and a third of a pixel short at the
 * far side of the stage.
 */
function logicalSpan(first: number, end: number, scale: number, stageLength: number): Span {
    const truncated = Math.trunc(single(stageLength * scale));
    const perLogical = stageLength > 0 ? truncated / stageLength : scale;
    return { start: first / perLogical, end: end / perLogical };
}

/**
 * A logical coordinate times a scale, as Mutter multiplies them: in single precision.
 *
 * The scales are already floats, so the product of two of them is exact as a double and
 * rounding it to a float is what C does. The difference shows at the scales that do not
 * terminate: 576 x 1.6666666269302368 is 959.99998 as a double and 960 as a float, which
 * is a pixel's difference to a floor.
 */
function single(value: number): number {
    return Math.fround(value);
}

/** `f64::round`, which the app's crop uses: half away from zero, not half up. */
function roundAway(value: number): number {
    return Math.sign(value) * Math.round(Math.abs(value));
}

function overlaps(a: Rect, b: Rect): boolean {
    return (
        a.x < b.x + b.width &&
        b.x < a.x + a.width &&
        a.y < b.y + b.height &&
        b.y < a.y + a.height
    );
}
