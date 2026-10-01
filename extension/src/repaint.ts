// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * What an area capture repaints, and which of its pixels the stage has nothing to paint
 * (`docs/decisions.md` D116). No `gi://` imports, so it is tested under Node like
 * `outline.ts`.
 *
 * An area capture repaints the stage into a texture of its own: the region's origin times
 * the scale rounded down, its extent to nearest (D23). `screenshot_area` also makes the
 * region the paint's clip, and the desktop's background paints only the pixels whose
 * centres are inside that clip. Where the extent was rounded up by half a pixel from an
 * origin on a whole pixel, the last column's centre lies *on* the region's far edge, and
 * nothing paints it. The texture is not cleared first, so that column came back
 * transparent, or holding whatever the GPU's memory held. D115's sweep saw it in 61 of 369
 * grabs at 1.25, 4/3, 1.5 and 5/3, and in none at 1 or 2.
 *
 * So a capture repaints the region grown by a logical pixel on every side, into a cleared
 * texture, and keeps exactly the pixels `screenshot_area` would have kept. Every one of
 * them then has its centre more than half a pixel inside the clip. The repaint's viewport
 * is anchored to the stage, not to the region, so a pixel is the same pixel in either
 * repaint, and the capture is the same size and in the same place (D115).
 *
 * A capture can still reach one pixel past everything the stage paints, on any side, and
 * no repaint helps there, [`pastPainted`]:
 *
 * - past a monitor's edge where no other monitor is. A monitor at 443 on a 1.5 panel starts
 *   at 664.5 physical pixels, so it ends on a half pixel too, and a region from an even
 *   coordinate to its far edge rounds up onto the pixel beyond it;
 * - past the stage's far edge. The viewport is the stage's length times the scale,
 *   truncated, so where that is not a whole number the stage's last pixel is outside it.
 *
 * Those pixels are given the colour of the one inside them.
 */

import type { Rect, Size } from './place.js';

/** A rectangle of physical pixels, in a repaint's texture. */
export interface PixelRect {
    x: number;
    y: number;
    width: number;
    height: number;
}

export interface RepaintPlan {
    /** The logical region to repaint: the capture's, a logical pixel larger on every side. */
    paint: Rect;
    /** The capture's pixels within that repaint's texture. */
    keep: PixelRect;
}

/** Which of a capture's outermost columns and rows lie past everything the stage paints. */
export interface Sides {
    left: boolean;
    right: boolean;
    top: boolean;
    bottom: boolean;
}

/**
 * The repaint for a capture of `rect` at `scale`, which is `size` in physical pixels.
 *
 * `scale` and `size` are what `clutter_stage_get_capture_final_size` says for `rect`,
 * which is what `screenshot_area` used: the largest scale under the region, and the
 * extent times it rounded to nearest.
 */
export function repaintPlan(rect: Rect, scale: number, size: Size): RepaintPlan {
    const paint = { x: rect.x - 1, y: rect.y - 1, width: rect.width + 2, height: rect.height + 2 };
    return {
        paint,
        keep: {
            x: origin(rect.x, scale) - origin(paint.x, scale),
            y: origin(rect.y, scale) - origin(paint.y, scale),
            width: size.width,
            height: size.height,
        },
    };
}

/**
 * Which outermost columns and rows of a capture of `rect` at `scale` no monitor paints.
 *
 * A monitor's background paints the pixels whose centres are inside the monitor, and a
 * centre on its near edge counts as inside it, on its far edge as outside. Each column
 * and row is looked at across the middle of the region, which for a region on one monitor
 * is every pixel of it but the corners. `stage` is the stage's logical size and `monitors`
 * their logical rects.
 */
export function pastPainted(
    rect: Rect,
    scale: number,
    stage: Size,
    monitors: readonly Rect[],
): Sides {
    const acrossPer = perLogical(stage.width, scale);
    const downPer = perLogical(stage.height, scale);
    const left = origin(rect.x, scale);
    const top = origin(rect.y, scale);
    const right = left + Math.round(single(rect.width * scale)) - 1;
    const bottom = top + Math.round(single(rect.height * scale)) - 1;
    const middle = {
        x: (rect.x + rect.width / 2) * acrossPer,
        y: (rect.y + rect.height / 2) * downPer,
    };
    const painted = (x: number, y: number) =>
        monitors.some(
            m =>
                x >= m.x * acrossPer &&
                x < (m.x + m.width) * acrossPer &&
                y >= m.y * downPer &&
                y < (m.y + m.height) * downPer,
        );
    return {
        left: !painted(left + 0.5, middle.y),
        right: !painted(right + 0.5, middle.y),
        top: !painted(middle.x, top + 0.5),
        bottom: !painted(middle.x, bottom + 0.5),
    };
}

/**
 * The stage pixel a repaint of a region starting at `start` calls 0. The viewport's origin
 * is `-(start × scale)`, multiplied in single precision and truncated toward zero, so it
 * is the product rounded down for a region on the stage.
 */
export function origin(start: number, scale: number): number {
    return 0 - Math.trunc(-single(start * scale));
}

/**
 * Stage pixels per logical pixel in any repaint along an axis: the viewport's length, the
 * stage's times the scale truncated, over the stage's.
 */
function perLogical(stageLength: number, scale: number): number {
    return Math.trunc(single(stageLength * scale)) / stageLength;
}

/** A coordinate times a scale, as Mutter multiplies them: in single precision. */
function single(value: number): number {
    return Math.fround(value);
}
