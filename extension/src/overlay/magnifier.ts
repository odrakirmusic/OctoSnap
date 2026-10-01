// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * `CAP-10`: the crosshair and the pixel magnifier.
 *
 * `spec/03` §5.1 and `spec/08` §6: full-height and full-width guide lines through the
 * pointer, and a loupe showing the pixels around it magnified with a grid, so a selection
 * can be placed on an exact pixel.
 *
 * The loupe samples a **frozen snapshot of the stage**, not the live screen. That is not
 * a compromise -- it is the only way it can work. Reading pixels per frame would be far
 * outside `spec/10` §7's 2 ms budget, and a loupe showing the live screen would show
 * itself, recursively. `Shell.Screenshot.screenshot_stage_to_content` gives one texture
 * up front and the loupe is then a scaled, offset view of it, which costs the compositor
 * a transform and nothing else.
 *
 * The consequence is worth stating plainly: **in a non-frozen capture the loupe shows the
 * screen as it was when the overlay opened.** For placing a selection on a window edge or
 * a piece of text, which is what it is for, that is exactly right. For pointing it at a
 * playing video it is visibly stale, which is what `CAP-09`'s freeze mode is for.
 */

import Clutter from 'gi://Clutter';
import St from 'gi://St';

import type { FrozenScreen } from '../capture.js';

/** `spec/03` §4: a 1 px line, dark-outlined so it reads on a light background. */
const GUIDE_STYLE =
    'background-color: rgba(255,255,255,0.75); ' +
    'box-shadow: 0 0 0 1px rgba(0,0,0,0.35);';

/** The loupe: a bordered square, offset from the pointer so it never sits under it. */
const LOUPE_STYLE =
    'border: 1px solid rgba(255,255,255,0.9); ' +
    'box-shadow: 0 0 0 1px rgba(0,0,0,0.45), 0 2px 8px rgba(0,0,0,0.35); ' +
    'background-color: rgba(0,0,0,0.9);';

const READOUT_STYLE =
    'background-color: rgba(0,0,0,0.78); ' +
    'color: #ffffff; ' +
    'font-size: 9pt; ' +
    'font-feature-settings: "tnum"; ' +
    'padding: 2px 5px;';

/** Logical size of the loupe on screen. */
const LOUPE_SIZE = 112;

/** How many source pixels across the loupe shows. 7 gives a clear centre pixel. */
const SOURCE_PIXELS = 7;

/** Gap between the pointer and the loupe's nearest corner. */
const LOUPE_OFFSET = 18;

/**
 * The crosshair and loupe for one monitor's overlay root.
 *
 * Created once and moved thereafter, like everything else on the drag path.
 */
export class Crosshair {
    #vertical: St.Widget;
    #horizontal: St.Widget;
    #loupe: St.Widget | null = null;
    #view: St.Widget | null = null;
    #readout: St.Label | null = null;
    #monitor: { x: number; y: number; width: number; height: number };
    /**
     * Whether the overlay wants the crosshair shown: always, or while Ctrl is held
     * (`shot-crosshair`). `update` moves the parts and leaves this alone (D136).
     */
    #wanted = false;

    constructor(
        parent: Clutter.Actor,
        monitor: { x: number; y: number; width: number; height: number },
        frozen: FrozenScreen | null,
        stage: { width: number; height: number },
        withLoupe: boolean,
    ) {
        this.#monitor = monitor;

        this.#vertical = new St.Widget({ style: GUIDE_STYLE, width: 1, visible: false });
        this.#horizontal = new St.Widget({ style: GUIDE_STYLE, height: 1, visible: false });
        parent.add_child(this.#vertical);
        parent.add_child(this.#horizontal);

        // No snapshot, no loupe. Showing an empty box would be worse than showing
        // nothing, and the guides are useful on their own.
        if (withLoupe && frozen !== null) {
            this.#loupe = new St.Widget({
                style: LOUPE_STYLE,
                width: LOUPE_SIZE,
                height: LOUPE_SIZE,
                visible: false,
                // The magnified content must not spill outside the border.
                clip_to_allocation: true,
                layout_manager: new Clutter.FixedLayout(),
            });

            // The whole stage, scaled up and shifted so the pointer's pixel lands in the
            // middle of the loupe. One actor, one transform, no per-frame pixel work.
            this.#view = new St.Widget({
                width: stage.width,
                height: stage.height,
            });
            this.#view.set_content(frozen.content);
            this.#view.set_pivot_point(0, 0);
            const zoom = LOUPE_SIZE / SOURCE_PIXELS;
            this.#view.set_scale(zoom, zoom);
            this.#loupe.add_child(this.#view);

            this.#readout = new St.Label({ style: READOUT_STYLE, visible: false });
            parent.add_child(this.#loupe);
            parent.add_child(this.#readout);
        }
    }

    /**
     * Moves everything to follow the pointer. `x`/`y` are stage coordinates.
     *
     * It shows the parts only if `setVisible` asked for them. It used to show them
     * outright, and the overlay calls it on every motion right after deciding whether Ctrl
     * is held, so `modifier`, the default, drew the crosshair and its loupe the moment the
     * pointer moved, Ctrl or no Ctrl (D136).
     */
    update(x: number, y: number): void {
        // Only the monitor the pointer is actually on. Without this every monitor's
        // crosshair made itself visible on every motion event, and because the loupe is
        // clamped to its own monitor's edge, the off-monitor one parked in a corner and
        // stayed there -- a second, frozen loupe on the other screen for the whole
        // selection. Reported from a real two-monitor session; `docs/decisions.md` D26.
        if (!this.#contains(x, y)) {
            this.#show(false);
            return;
        }

        const local = { x: x - this.#monitor.x, y: y - this.#monitor.y };

        this.#vertical.set_position(local.x, 0);
        this.#vertical.set_size(1, this.#monitor.height);
        this.#vertical.visible = this.#wanted;

        this.#horizontal.set_position(0, local.y);
        this.#horizontal.set_size(this.#monitor.width, 1);
        this.#horizontal.visible = this.#wanted;

        if (this.#loupe === null || this.#view === null) return;

        // Flip the loupe to whichever side of the pointer has room, so it never runs off
        // the monitor edge and never covers the pixel being aimed at.
        const right = local.x + LOUPE_OFFSET;
        const below = local.y + LOUPE_OFFSET;
        const loupeX =
            right + LOUPE_SIZE <= this.#monitor.width
                ? right
                : local.x - LOUPE_OFFSET - LOUPE_SIZE;
        const loupeY =
            below + LOUPE_SIZE <= this.#monitor.height
                ? below
                : local.y - LOUPE_OFFSET - LOUPE_SIZE;
        this.#loupe.set_position(Math.max(loupeX, 0), Math.max(loupeY, 0));
        this.#loupe.visible = this.#wanted;

        // Centre the pointer's own pixel: shift the scaled stage so that stage pixel
        // (x, y) sits at the loupe's midpoint.
        const zoom = LOUPE_SIZE / SOURCE_PIXELS;
        this.#view.set_position(
            Math.round(LOUPE_SIZE / 2 - x * zoom),
            Math.round(LOUPE_SIZE / 2 - y * zoom),
        );

        if (this.#readout !== null) {
            // `spec/08` §6 asks for a hex readout too. The colour needs a pixel read,
            // which is a round trip this budget cannot afford per motion event, so the
            // readout carries the coordinate -- the part that is free and the part a user
            // placing a selection actually needs. The pipette (`PickColor`) is where a
            // colour belongs.
            this.#readout.text = `${x}, ${y}`;
            const [, w] = this.#readout.get_preferred_width(-1);
            const [, h] = this.#readout.get_preferred_height(-1);
            this.#readout.set_position(
                Math.max(Math.min(loupeX, this.#monitor.width - w), 0),
                Math.max(loupeY - h - 2, 0),
            );
            this.#readout.visible = this.#wanted;
        }
    }

    /** Whether a stage point is on this crosshair's own monitor. */
    #contains(x: number, y: number): boolean {
        return (
            x >= this.#monitor.x &&
            x < this.#monitor.x + this.#monitor.width &&
            y >= this.#monitor.y &&
            y < this.#monitor.y + this.#monitor.height
        );
    }

    /** Whether the crosshair should be up; it appears where `update` last put it. */
    setVisible(visible: boolean): void {
        this.#wanted = visible;
        this.#show(visible);
    }

    #show(visible: boolean): void {
        this.#vertical.visible = visible;
        this.#horizontal.visible = visible;
        if (this.#loupe !== null) this.#loupe.visible = visible;
        if (this.#readout !== null) this.#readout.visible = visible;
    }

    /** The actors are children of the root, so they die with it; this only clears refs. */
    destroy(): void {
        this.#loupe = null;
        this.#view = null;
        this.#readout = null;
    }
}
