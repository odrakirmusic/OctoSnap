// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * A pet on the stage (`spec/14` §3 and §5): one actor showing one texture, the pose the
 * pet is in, scaled up by a whole number of physical pixels to the art pixel -- or by
 * Smol's one and a half where no whole number fits (D166) -- with no smoothing, and
 * answering the pointer only where it is drawn.
 *
 * **Textures.** Each pose is painted once (`art/`), turned into a texture, and kept: a pet
 * that breathes, blinks and walks shows a few dozen poses over and over, and painting one
 * costs a tenth of a millisecond once the code is warm. The cache is bounded, oldest out.
 * A texture is the grid itself, one texel per art pixel, whatever the size setting and
 * whatever the monitor's scale; the actor's size does the scaling, with `NEAREST` both
 * ways, so no screen pixel is ever a blend of two art pixels.
 *
 * **Picking.** Clutter hands a click to the topmost reactive actor whose picked area is
 * under the pointer, and an actor's picked area is its whole allocation unless it says
 * otherwise. A pet's box is mostly empty around its round body, and a box-shaped hole in
 * every window behind a pet would be the worst thing about it. So `vfunc_pick` reports the
 * drawn pixels as a handful of rectangles (`opaqueRects`) and nothing else, and a click in
 * the empty corners goes through to whatever is underneath.
 */

import Clutter from 'gi://Clutter';
import Cogl from 'gi://Cogl';
import GObject from 'gi://GObject';
import St from 'gi://St';

import { type ArtRect, gridToRGBA, opaqueRects } from './frames.js';
import type { Grid } from './art/painter.js';

/** A pose, ready for the stage. */
export interface Frame {
    content: Clutter.Content;
    /** The drawn pixels, for picking. */
    rects: ArtRect[];
    /** In art pixels. */
    width: number;
    height: number;
    /**
     * `rects` as the boxes a pick reports, at one size: made the first time a pick asks for
     * them at that size and kept, since a pet walking or blinking goes back and forth
     * between the same few frames many times a second.
     */
    boxes?: { unit: number; list: Clutter.ActorBox[] };
}

/** A painted grid as a texture. */
export function frameOf(grid: Grid, mirror = false): Frame {
    const content = St.ImageContent.new_with_preferred_size(grid.w, grid.h) as St.ImageContent;
    const context = global.stage.get_context().get_backend().get_cogl_context();
    content.set_bytes(context, gridToRGBA(grid, mirror), Cogl.PixelFormat.RGBA_8888_PRE, grid.w, grid.h, grid.w * 4);
    return { content, rects: opaqueRects(grid, mirror), width: grid.w, height: grid.h };
}

/**
 * Frames by name, painted on first use. Map order is insertion order, so the oldest is
 * first, and a hit moves a frame to the back.
 */
export class FrameCache {
    readonly #frames = new Map<string, Frame>();
    readonly #limit: number;

    constructor(limit: number) {
        this.#limit = limit;
    }

    get size(): number {
        return this.#frames.size;
    }

    has(key: string): boolean {
        return this.#frames.has(key);
    }

    /** The frame called `key`, painting it with `paint` when there is none yet. */
    get(key: string, paint: () => Grid): Frame {
        let frame = this.#frames.get(key);
        if (frame !== undefined) {
            this.#frames.delete(key);
            this.#frames.set(key, frame);
            return frame;
        }
        frame = frameOf(paint());
        this.#frames.set(key, frame);
        while (this.#frames.size > this.#limit) {
            const oldest = this.#frames.keys().next().value;
            if (oldest === undefined) break;
            this.#frames.delete(oldest);
        }
        return frame;
    }

    /** Only frames already painted: for a callback that has no time left to paint one. */
    peek(key: string): Frame | undefined {
        return this.#frames.get(key);
    }

    clear(): void {
        this.#frames.clear();
    }
}

/**
 * The actor. A `GObject` subclass because picking is a virtual function; its fields are
 * underscored rather than `#` private, which GJS's GObject classes do not all take.
 */
export const PetSprite = GObject.registerClass(
    class PetSprite extends Clutter.Actor {
        _frame: Frame | null;
        _unit: number;

        constructor(name: string) {
            super({ name, reactive: true });
            this._frame = null;
            this._unit = 1;
            this.set_content_scaling_filters(Clutter.ScalingFilter.NEAREST, Clutter.ScalingFilter.NEAREST);
            this.set_content_gravity(Clutter.ContentGravity.RESIZE_FILL);
        }

        /**
         * Shows `frame` at `unit` logical pixels to the art pixel. Only what changed is set:
         * this runs every step, and most steps show the frame the last one did.
         */
        showFrame(frame: Frame, unit: number): void {
            if (this._frame === frame && this._unit === unit) return;
            if (this.get_content() !== frame.content) this.set_content(frame.content);
            this.set_size(frame.width * unit, frame.height * unit);
            this._frame = frame;
            this._unit = unit;
        }

        /** Whether the art pixel under the stage point `x, y` is drawn. */
        drawnAt(x: number, y: number): boolean {
            const [ok, lx, ly] = this.transform_stage_point(x, y);
            if (!ok || this._frame === null) return false;
            const ax = lx / this._unit;
            const ay = ly / this._unit;
            return this._frame.rects.some(r => ax >= r.x && ax < r.x + r.w && ay >= r.y && ay < r.y + r.h);
        }

        override vfunc_pick(context: Clutter.PickContext): void {
            // Only the drawn pixels, and no children to pick: see the module comment.
            const frame = this._frame;
            if (frame === null) return;
            const unit = this._unit;
            if (frame.boxes?.unit !== unit) {
                frame.boxes = {
                    unit,
                    list: frame.rects.map(
                        r => new Clutter.ActorBox({ x1: r.x * unit, y1: r.y * unit, x2: (r.x + r.w) * unit, y2: (r.y + r.h) * unit }),
                    ),
                };
            }
            for (const box of frame.boxes.list) this.pick_box(context, box);
        }
    },
);

export type PetSprite = InstanceType<typeof PetSprite>;
