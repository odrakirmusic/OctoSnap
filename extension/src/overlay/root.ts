// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * One overlay root per monitor: `spec/03` §2's actor stack.
 *
 * Two structural choices worth stating, both driven by `spec/10` §7's rule that no
 * extension callback may exceed 2 ms and that nothing may allocate per frame:
 *
 * 1. **The dim is four rects around the selection, not a mask.** `spec/03` §2 offers
 *    either. Four `St.Widget`s need no shader and no per-frame pixel work; a mask would
 *    mean redrawing on every motion event, which is exactly what the budget forbids.
 * 2. **Every actor is created once and moved thereafter.** `setSelection` only ever calls
 *    `set_position` and `set_size`, never `new`. A drag fires motion events at frame
 *    rate, and GJS allocation on that path is what makes an overlay stutter the whole
 *    desktop rather than just itself.
 */

import Clutter from 'gi://Clutter';
import St from 'gi://St';

import type { Rect } from './selection.js';
import { HANDLES, type Handle, dimPieces, handleCentre, intersect } from './selection.js';
import { HANDLES_APPEAR, HANDLES_FROM_SCALE, easing } from './motion.js';

/** `spec/03` §4: uniform translucent black outside the selection. [M] 0.3-0.45. */
const DIM_COLOR = 'rgba(0,0,0,0.35)';

/** `spec/03` §4: 8x8 px handles, white fill with a 1 px dark border. */
const HANDLE_SIZE = 8;

/** `spec/03` §4: 1 px white line with a dark outer outline for light backgrounds. */
const FRAME_STYLE =
    'border: 1px solid rgba(255,255,255,0.95); ' +
    'box-shadow: 0 0 0 1px rgba(0,0,0,0.25);';

const HANDLE_STYLE =
    'background-color: #ffffff; ' +
    'border: 1px solid rgba(0,0,0,0.55); ' +
    'border-radius: 1px;';

/**
 * `spec/06` §3's record accent: the selection frame and handles go red so the overlay
 * reads as "this starts a recording", not "this takes a screenshot". The handles are
 * circular (a large radius on the 8x8 square) rather than the capture overlay's tiny
 * squares, which is the other half of the same signal.
 */
export type Accent = 'default' | 'record';

/** `spec/09`'s destructive/record red, the same #e01b24 the pill and panel dot use. */
const FRAME_STYLE_RECORD =
    'border: 2px solid rgba(224,27,36,0.95); ' +
    'box-shadow: 0 0 0 1px rgba(0,0,0,0.35);';

const HANDLE_STYLE_RECORD =
    'background-color: #e01b24; ' +
    'border: 1px solid rgba(255,255,255,0.85); ' +
    'border-radius: 999px;';

/**
 * `spec/03` §4: for the classic modes, which have no toolbar, the dimension readout is a
 * floating pill. All-In-One puts editable W/H fields in the toolbar instead
 * (`[P→V] 2026-09-03`), so with a toolbar there is no pill (D136).
 */
const LABEL_STYLE =
    'background-color: rgba(0,0,0,0.72); ' +
    'color: #ffffff; ' +
    'border-radius: 6px; ' +
    'padding: 3px 8px; ' +
    'font-size: 11pt; ' +
    'font-feature-settings: "tnum";';

const HINT_STYLE =
    'background-color: rgba(0,0,0,0.6); ' +
    'color: rgba(255,255,255,0.85); ' +
    'border-radius: 6px; ' +
    'padding: 4px 10px; ' +
    'font-size: 10pt;';

/** Gap between the selection edge and the dimension pill. */
const LABEL_GAP = 6;

/**
 * `spec/03` §2: "over handles it changes to the matching resize cursor".
 *
 * Set once per handle actor rather than recomputed on motion. On GNOME 50 the cursor is
 * a per-actor Clutter property, so Clutter picks the right one from whatever is under the
 * pointer and the engine never has to think about it.
 */
const HANDLE_CURSORS: Record<Handle, Clutter.CursorType> = {
    'top-left': Clutter.CursorType.NWSE_RESIZE,
    'top': Clutter.CursorType.NS_RESIZE,
    'top-right': Clutter.CursorType.NESW_RESIZE,
    'right': Clutter.CursorType.EW_RESIZE,
    'bottom-right': Clutter.CursorType.NWSE_RESIZE,
    'bottom': Clutter.CursorType.NS_RESIZE,
    'bottom-left': Clutter.CursorType.NESW_RESIZE,
    'left': Clutter.CursorType.EW_RESIZE,
};

export interface MonitorGeometry {
    index: number;
    x: number;
    y: number;
    width: number;
    height: number;
    scale: number;
}

export class OverlayRoot {
    readonly monitor: MonitorGeometry;
    readonly actor: St.Widget;

    /** Four dim rects: above, below, left of and right of the selection. */
    #dim: St.Widget[] = [];
    #frame: St.Widget;
    #handles = new Map<Handle, St.Widget>();
    /** Whether the handles are up, so that only their arrival pops (`spec/09` §3). */
    #handlesShown = false;

    /** See `destroy`: a fading teardown can reach it twice. */
    #destroyed = false;

    /** `CAP-09`'s frozen frame, when there is one. See `hideChrome`. */
    #backdrop: St.Widget | null = null;
    #label: St.Label;
    #hint: St.Label;

    constructor(monitor: MonitorGeometry, accent: Accent = 'default') {
        this.monitor = monitor;
        const frameStyle = accent === 'record' ? FRAME_STYLE_RECORD : FRAME_STYLE;
        const handleStyle = accent === 'record' ? HANDLE_STYLE_RECORD : HANDLE_STYLE;

        this.actor = new St.Widget({
            name: `octosnap-overlay-${monitor.index}`,
            reactive: true,
            can_focus: true,
            x: monitor.x,
            y: monitor.y,
            width: monitor.width,
            height: monitor.height,
            // Absolute placement of children; no layout manager runs on the drag path.
            layout_manager: new Clutter.FixedLayout(),
        });

        for (let i = 0; i < 4; i++) {
            const piece = new St.Widget({ style: `background-color: ${DIM_COLOR};` });
            this.#dim.push(piece);
            this.actor.add_child(piece);
        }

        this.#frame = new St.Widget({ style: frameStyle, visible: false });
        this.actor.add_child(this.#frame);

        for (const handle of HANDLES) {
            const actor = new St.Widget({
                style: handleStyle,
                width: HANDLE_SIZE,
                height: HANDLE_SIZE,
                visible: false,
                reactive: true,
            });
            actor.set_cursor_type(HANDLE_CURSORS[handle]);
            // Scaled about its own centre when it pops in, so it grows where it sits.
            actor.set_pivot_point(0.5, 0.5);
            this.#handles.set(handle, actor);
            this.actor.add_child(actor);
        }

        this.#label = new St.Label({ style: LABEL_STYLE, visible: false });
        this.actor.add_child(this.#label);

        this.#hint = new St.Label({ style: HINT_STYLE, visible: false });
        this.actor.add_child(this.#hint);

        // No selection yet: dim the whole monitor.
        this.setSelection(null, false, false);
    }

    /**
     * Re-dresses the frame and the handles: All-In-One switching to Record turns the
     * open overlay red (`spec/06` §3), and switching to any other mode turns it back.
     * The dim, the label and the hint are untouched -- only the accent changes.
     */
    setAccent(accent: Accent): void {
        this.#frame.set_style(accent === 'record' ? FRAME_STYLE_RECORD : FRAME_STYLE);
        const handleStyle = accent === 'record' ? HANDLE_STYLE_RECORD : HANDLE_STYLE;
        for (const actor of this.#handles.values()) actor.set_style(handleStyle);
    }

    /**
     * Positions everything for a selection, or dims the whole monitor when there is
     * none. `rect` is in **stage** coordinates; children are placed relative to the
     * monitor, so the monitor origin is subtracted exactly once, here.
     *
     * `showLabel` is false under a toolbar, whose W and H fields are the readout. Both
     * went below the selection, so the pill sat under the toolbar and said the same thing
     * twice (D136).
     */
    setSelection(rect: Rect | null, showHandles: boolean, showLabel: boolean): void {
        const m = this.monitor;

        // **The part of the selection that is on *this* monitor**, which is what the dim
        // must be arranged around. A root whose monitor the selection does not touch dims
        // all of itself, and a root that holds only part of a selection -- `spec/01` §1
        // permits a span across displays of equal scale -- tiles around its own part.
        //
        // Tiling around the raw rect was the bug the user reported as "the greyed-out
        // background does not fill the whole second screen". Measured: with a selection at
        // 2700,200 on the monitor at 2560,0, the *other* root computed a left-hand dim
        // piece **2700 px wide on a 2560 px monitor**, which overhung its neighbour and
        // double-dimmed a strip of it, while the four pieces no longer described that
        // monitor's own geometry at all. `docs/decisions.md` D24.
        //
        // No selection at all, or none of it here: dim the whole monitor and show no
        // chrome. The frame, handles and label belong to the selection, so a monitor the
        // selection does not touch must not draw them -- otherwise two monitors draw two
        // frames for one selection.
        const part = rect === null ? null : intersect(rect, m);
        if (rect === null || part === null) {
            const pieces = dimPieces(m, rect);
            pieces.forEach((p, i) => this.#placeDim(i, p.x, p.y, p.width, p.height));
            this.#frame.visible = false;
            this.#label.visible = false;
            for (const actor of this.#handles.values()) actor.visible = false;
            this.#handlesShown = false;
            return;
        }

        // The tiling itself is pure and unit-tested; see `dimPieces`.
        const pieces = dimPieces(m, rect);
        pieces.forEach((piece, i) => this.#placeDim(i, piece.x, piece.y, piece.width, piece.height));

        // The frame, handles and label describe the *whole* selection rather than this
        // monitor's part of it, so they are placed from `rect` and not from `part`.
        // Clutter does not clip to the allocation, so a frame that runs past this
        // monitor's edge still draws -- which is what a selection spanning two equally
        // scaled displays should look like.
        this.#frame.set_position(rect.x - m.x, rect.y - m.y);
        this.#frame.set_size(rect.width, rect.height);
        this.#frame.visible = true;

        // `spec/09` §3's "handles appear", once per arrival: a resize or a move keeps them
        // up and only places them, on the path the drag runs at frame rate.
        const appearing = showHandles && !this.#handlesShown;
        this.#handlesShown = showHandles;
        for (const [handle, actor] of this.#handles) {
            if (!showHandles) {
                actor.visible = false;
                continue;
            }
            const [cx, cy] = handleCentre(rect, handle);
            actor.set_position(
                cx - m.x - Math.trunc(HANDLE_SIZE / 2),
                cy - m.y - Math.trunc(HANDLE_SIZE / 2),
            );
            actor.visible = true;
            if (appearing) popIn(actor);
        }

        // The label follows the selection, so it takes the selection's own local edges
        // rather than this monitor's share of them -- a label pinned to the clipped edge
        // of a spanning selection would sit in the middle of it.
        if (!showLabel) {
            this.#label.visible = false;
            return;
        }
        this.#placeLabel(
            rect,
            rect.x - m.x,
            rect.y - m.y,
            rect.y - m.y + rect.height,
        );
    }

    /**
     * `spec/03` §4: the readout shows the size that will be captured. Physical pixels,
     * not logical, because that is what the file will contain (`spec/01` §1) and
     * `spec/03` §12 item 2 requires the label to match the PNG exactly at every scale.
     */
    #placeLabel(rect: Rect, left: number, top: number, bottom: number): void {
        const scale = this.monitor.scale;
        const physicalWidth = Math.ceil(rect.width * scale);
        const physicalHeight = Math.ceil(rect.height * scale);
        this.#label.text = `${physicalWidth} × ${physicalHeight}`;
        this.#label.visible = true;

        // Below the selection by default, flipping above when there is no room, and
        // clamped so it never leaves the monitor.
        const [, labelWidth] = this.#label.get_preferred_width(-1);
        const [, labelHeight] = this.#label.get_preferred_height(-1);

        let labelY = bottom + LABEL_GAP;
        if (labelY + labelHeight > this.monitor.height)
            labelY = top - labelHeight - LABEL_GAP;
        if (labelY < 0) labelY = top + LABEL_GAP;

        const labelX = Math.min(
            Math.max(left, 0),
            Math.max(this.monitor.width - labelWidth, 0),
        );
        this.#label.set_position(labelX, labelY);
    }

    /** `spec/03` §4: an optional one-line hint at the bottom of the monitor. */
    setHint(text: string | null): void {
        if (text === null) {
            this.#hint.visible = false;
            return;
        }
        this.#hint.text = text;
        this.#hint.visible = true;
        const [, w] = this.#hint.get_preferred_width(-1);
        const [, h] = this.#hint.get_preferred_height(-1);
        this.#hint.set_position(
            Math.trunc((this.monitor.width - w) / 2),
            this.monitor.height - h - 48,
        );
    }

    #placeDim(index: number, x: number, y: number, width: number, height: number): void {
        const piece = this.#dim[index]!;
        piece.set_position(x, y);
        piece.set_size(Math.max(width, 0), Math.max(height, 0));
    }

    /** Hides the whole overlay without destroying it, for the pre-capture frame. */
    setVisible(visible: boolean): void {
        this.actor.visible = visible;
    }

    /**
     * Hides everything the capture must not contain, and optionally keeps the frozen
     * frame on screen.
     *
     * `spec/03` §7 step 1: "Hide overlay chrome (toolbar, frame, handles, magnifier,
     * crosshair) and the dim -- **or in Freeze mode simply keep the frozen frame**."
     *
     * Keeping it is safe precisely in the freeze case and nowhere else: those pixels come
     * from the stored texture through `composite_to_stream`, not from the live stage, so
     * a frozen backdrop still on screen cannot appear in its own capture. In live mode
     * the whole root goes, because there the stage *is* the source.
     *
     * What it buys is the end of `spec/03` §7 step 5 -- something for the frozen frame to
     * fade out *from* while the capture flies to the corner. Hiding it a frame earlier
     * would make the screen snap back to live before the animation even started.
     */
    hideChrome(keepBackdrop: boolean): void {
        if (!keepBackdrop || this.#backdrop === null) {
            this.actor.visible = false;
            return;
        }
        for (const child of this.actor.get_children())
            if (child !== this.#backdrop) child.visible = false;
    }

    /**
     * Turns the dim off, for modes that do not want it.
     *
     * `spec/03` §5.2's window mode highlights its target instead of dimming around it,
     * because dimming everything would hide the thing being pointed at. The rects stay
     * allocated so the mode can be switched back without rebuilding the root, which
     * All-In-One needs.
     */
    setDimVisible(visible: boolean): void {
        for (const piece of this.#dim) piece.visible = visible;
    }

    /**
     * Adds a caller-owned actor in front of the dim, positioned in monitor-local
     * coordinates.
     *
     * Used for the window highlight and the freeze backdrop, both of which belong to one
     * mode rather than to every overlay. The actor is destroyed with the root, so the
     * caller keeps the reference but not the responsibility.
     */
    addFloating(style: string): St.Widget {
        const actor = new St.Widget({ style, visible: false });
        this.actor.add_child(actor);
        return actor;
    }

    /**
     * Puts a `Clutter.Content` behind everything else, filling the monitor.
     *
     * `CAP-09`'s freeze: the content is a snapshot of the whole stage in physical pixels,
     * so the actor is sized to the monitor in logical pixels and the content is offset to
     * the monitor's own origin. Behind the dim, so the dim still darkens the frozen image
     * rather than the live screen showing through.
     */
    setBackdrop(content: Clutter.Content, stageWidth: number, stageHeight: number): St.Widget {
        const backdrop = new St.Widget({
            // The whole stage, shifted so this monitor's part of it lands at 0,0.
            x: -this.monitor.x,
            y: -this.monitor.y,
            width: stageWidth,
            height: stageHeight,
        });
        backdrop.set_content(content);
        this.actor.insert_child_below(backdrop, this.#dim[0] ?? null);
        this.#backdrop = backdrop;
        return backdrop;
    }

    /**
     * Idempotent, because it is no longer only called once.
     *
     * A fading teardown (`engine.ts`'s `fadeOutRoot`) destroys the root from the
     * transition's completion handler, and a transition that is *interrupted* reports
     * completion too -- so a display change mid-fade can reach here twice. Destroying an
     * already-destroyed Clutter actor is a warning in the journal and a puzzle for
     * whoever reads it later.
     */
    destroy(): void {
        if (this.#destroyed) return;
        this.#destroyed = true;
        this.actor.destroy();
        this.#dim = [];
        this.#handles.clear();
    }
}

/**
 * "Handles appear | 100 ms | ease-out-back (subtle) | scale 0.6→1". The back curve's
 * overshoot is Clutter's own, a few per cent past full size, which on an 8 px square is
 * under a pixel: the subtle kind.
 */
function popIn(actor: Clutter.Actor): void {
    actor.set_scale(HANDLES_FROM_SCALE, HANDLES_FROM_SCALE);
    actor.ease(
        easing({
            scale_x: 1,
            scale_y: 1,
            duration: HANDLES_APPEAR.duration,
            mode: HANDLES_APPEAR.mode,
        }),
    );
}
