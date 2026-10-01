// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * `spec/07` §3.1's pin shadow, drawn by the compositor around the pin's window (D132).
 *
 * D66 put it off because a pin's window is the capture's rect to the pixel: its placement,
 * its lock-mode input region, the arrow keys, Hide/Show and `MoveWindowBy` all mean that
 * rect, and a transparent band for a shadow to be drawn into would move every one of them.
 * The compositor can draw outside a window, so the shadow is drawn here and the pin's
 * window does not change at all.
 *
 * **Where it lives.** Beside the pin's window actor in `global.window_group`, just below
 * it, so that the pin covers its own shadow and every window above the pin covers both.
 * Mutter restacks window actors among themselves and drops anything else to the top when
 * it does (`sync_actor_stacking`), so the shadow is put back under its pin after every
 * restack. It is bound to the actor's position and size, which Mutter sets before the
 * frame is laid out, so the shadow moves with a drag in the same frame. And it follows the
 * actor's visibility, opacity, scale and pivot: the shell's own map and close animations
 * scale and fade a pin like any window, and a shadow that stayed put while the pin grew
 * out of its bottom edge would show the trick.
 *
 * **Through a workspace switch.** The shell slides clones of the windows in a group of its
 * own above `window_group`, and a clone of a window actor is the window without the actor
 * beside it: the shadow went off for the length of every switch and came back at the end.
 * So each clone of a shaded window gets a clone of its shadow, just below it, as soon as
 * the group is added. A shell that builds the switch differently finds nothing to match
 * and goes back to the blink, not to anything worse.
 *
 * **What it is made of.** Eight small textures from `shadow.ts`, shared by every pin at the
 * same radius and scale: four corners, and four strips one pixel long stretched along the
 * edges. A pin too small for straight edges gets one texture of its own.
 *
 * The app says which windows have one, with what corner radius and at what opacity, over
 * `SetWindowShadow`: a pin's look is the app's setting, and the pin's opacity is the
 * app's to change (§3.1's scroll to fade), and neither can be seen from here.
 */

import Clutter from 'gi://Clutter';
import Cogl from 'gi://Cogl';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Meta from 'gi://Meta';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import { error, info } from './log.js';
import {
    type Margins,
    PIN_SHADOW,
    type PinShape,
    type ShadowPiece,
    pieceRects,
    piecePixels,
    shadowMargins,
    wholeShadow,
} from './shadow.js';

type PieceName = Exclude<ShadowPiece['name'], 'whole'>;

/** Premultiplied RGBA bytes as a texture the stage can draw. */
function contentOf(piece: ShadowPiece): Clutter.Content {
    const content = St.ImageContent.new_with_preferred_size(piece.width, piece.height) as St.ImageContent;
    const context = global.stage.get_context().get_backend().get_cogl_context();
    content.set_bytes(
        context,
        piece.pixels,
        Cogl.PixelFormat.RGBA_8888_PRE,
        piece.width,
        piece.height,
        piece.width * 4,
    );
    return content;
}

/**
 * The eight pieces' textures for a radius at a scale, made the first time a pin needs them
 * and kept: about 25 kB at scale 1 and 100 kB at 2 for each pair, whatever the pins.
 */
const pieceContents = new Map<string, Map<PieceName, Clutter.Content>>();

function contentsFor(radius: number, scale: number): Map<PieceName, Clutter.Content> {
    const key = `${radius}@${scale}`;
    let contents = pieceContents.get(key);
    if (contents === undefined) {
        const started = GLib.get_monotonic_time();
        contents = new Map();
        for (const piece of piecePixels(radius, scale, PIN_SHADOW))
            contents.set(piece.name as PieceName, contentOf(piece));
        pieceContents.set(key, contents);
        info(
            `pin shadow pieces made for radius ${radius} at scale ${scale} in ` +
            `${((GLib.get_monotonic_time() - started) / 1000).toFixed(1)} ms`,
        );
    }
    return contents;
}

/** What the app asked for a window: its corners, and how opaque the pin is. */
export interface ShadowLook {
    radius: number;
    /** 0 to 1: the pin's own opacity. */
    opacity: number;
}

/** One pin's shadow. */
class WindowShadow {
    readonly window: Meta.Window;
    readonly #actor: Meta.WindowActor;
    #look: ShadowLook;
    #margins: Margins = shadowMargins(PIN_SHADOW);
    #box: Clutter.Actor;
    /** The size and scale the pieces were last laid out for. */
    #laidOut = '';
    #relayout = 0;
    #handlers: [GObject.Object, number][] = [];
    #bindings: GObject.Binding[] = [];
    readonly #onGone: () => void;

    constructor(window: Meta.Window, actor: Meta.WindowActor, look: ShadowLook, onGone: () => void) {
        this.window = window;
        this.#actor = actor;
        this.#look = look;
        this.#onGone = onGone;

        this.#box = new Clutter.Actor({
            name: 'octosnap-pin-shadow',
            reactive: false,
            layout_manager: new Clutter.FixedLayout(),
        });
        const parent = actor.get_parent();
        if (parent === null) throw new Error('the pin has no place in the stage yet');
        parent.insert_child_below(this.#box, actor);
        this.#bind();

        // The actor goes before the window is forgotten, and a window can be unmanaged
        // without its actor going first; either one is the end of the shadow.
        this.#connect(actor, 'destroy', () => this.#gone());
        this.#connect(window, 'unmanaged', () => this.#gone());
        this.#connect(actor, 'notify::width', () => this.#queueLayout());
        this.#connect(actor, 'notify::height', () => this.#queueLayout());
        this.#connect(actor, 'notify::opacity', () => this.#syncOpacity());
        this.#connect(actor, 'notify::pivot-point', () => this.#syncPivot());
        this.#connect(this.#box, 'resource-scale-changed', () => this.#queueLayout());
        this.#layOut();
        this.#syncOpacity();
    }

    /** A new radius or opacity from the app. */
    update(look: ShadowLook): void {
        const radiusChanged = look.radius !== this.#look.radius;
        this.#look = look;
        if (radiusChanged) {
            this.#laidOut = '';
            this.#layOut();
        }
        this.#syncOpacity();
    }

    /** The window actor it shades, which is what a switch's clones are clones of. */
    get actor(): Meta.WindowActor {
        return this.#actor;
    }

    /**
     * A copy of the shadow under a copy of the pin (see the head of this file). The copy
     * goes when the switch's group does; one whose pin closed during the switch has no
     * source left and draws nothing until then.
     */
    shadeClone(windowClone: Clutter.Clone): void {
        const parent = windowClone.get_parent();
        if (parent === null) return;
        const box = this.#box.get_allocation_box();
        const actor = this.#actor.get_allocation_box();
        const copy = new Clutter.Clone({
            name: 'octosnap-pin-shadow-clone',
            source: this.#box,
            reactive: false,
            x: windowClone.x + (box.x1 - actor.x1),
            y: windowClone.y + (box.y1 - actor.y1),
            width: box.get_width(),
            height: box.get_height(),
            // A clone paints its source at its own opacity, not the source's.
            opacity: this.#box.opacity,
        });
        parent.insert_child_below(copy, windowClone);
    }

    /** Back under its pin: Mutter's restack has just put it on top of everything. */
    restack(): void {
        const parent = this.#actor.get_parent();
        if (parent === null) return;
        if (this.#box.get_parent() !== parent) {
            // The shell moves a window actor into another group for some animations; the
            // shadow goes with it, or it would be left drawn where the pin used to be.
            this.#box.get_parent()?.remove_child(this.#box);
            parent.insert_child_below(this.#box, this.#actor);
            return;
        }
        parent.set_child_below_sibling(this.#box, this.#actor);
    }

    destroy(): void {
        if (this.#relayout !== 0) {
            GLib.source_remove(this.#relayout);
            this.#relayout = 0;
        }
        for (const binding of this.#bindings) binding.unbind();
        this.#bindings = [];
        for (const [object, id] of this.#handlers) object.disconnect(id);
        this.#handlers = [];
        this.#box.destroy();
    }

    #gone(): void {
        this.destroy();
        this.#onGone();
    }

    #connect(object: GObject.Object, signal: string, handler: () => void): void {
        this.#handlers.push([object, object.connect(signal, handler)]);
    }

    /**
     * Bound to the pin's frame rect, the rect the user sees, grown by the shadow's margins.
     * The frame and the buffer are the same rect for a pin, which draws no shadow of its
     * own; the offset is read rather than assumed all the same.
     */
    #bind(): void {
        const frame = this.window.get_frame_rect();
        const buffer = this.window.get_buffer_rect();
        const dx = frame.x - buffer.x;
        const dy = frame.y - buffer.y;
        const dw = frame.width - buffer.width;
        const dh = frame.height - buffer.height;
        const m = this.#margins;
        const bind = (coordinate: Clutter.BindCoordinate, offset: number) =>
            this.#box.add_constraint(new Clutter.BindConstraint({ source: this.#actor, coordinate, offset }));
        bind(Clutter.BindCoordinate.X, dx - m.left);
        bind(Clutter.BindCoordinate.Y, dy - m.top);
        bind(Clutter.BindCoordinate.WIDTH, dw + m.left + m.right);
        bind(Clutter.BindCoordinate.HEIGHT, dh + m.top + m.bottom);

        const flags = GObject.BindingFlags.SYNC_CREATE;
        this.#bindings.push(
            this.#actor.bind_property('visible', this.#box, 'visible', flags),
            this.#actor.bind_property('scale-x', this.#box, 'scale-x', flags),
            this.#actor.bind_property('scale-y', this.#box, 'scale-y', flags),
        );
        this.#syncPivot();
    }

    /** The pin's size without the shadow's margins. */
    #pinSize(): { width: number; height: number } {
        const frame = this.window.get_frame_rect();
        return { width: frame.width, height: frame.height };
    }

    /**
     * The pin's pivot in the shadow's own terms, so that the two scale about the same
     * point on the screen. The shell's map animation grows a window out of its bottom
     * edge (`pivot 0.5, 1`), and a shadow scaled about its own middle would drift off it.
     */
    #syncPivot(): void {
        const pivot = this.#actor.pivot_point;
        const { width, height } = this.#pinSize();
        const m = this.#margins;
        const outerW = width + m.left + m.right;
        const outerH = height + m.top + m.bottom;
        if (outerW <= 0 || outerH <= 0) return;
        this.#box.set_pivot_point(
            (m.left + pivot.x * width) / outerW,
            (m.top + pivot.y * height) / outerH,
        );
    }

    /** The pin's opacity, times whatever the shell is fading the window to. */
    #syncOpacity(): void {
        const opacity = Math.max(0, Math.min(1, this.#look.opacity));
        this.#box.opacity = Math.round(this.#actor.opacity * opacity);
    }

    /**
     * Laid out off the signal: a size notify can arrive in the middle of a layout, which
     * is no time to add and remove actors. One frame late, and only when the size changed.
     */
    #queueLayout(): void {
        if (this.#relayout !== 0) return;
        this.#relayout = GLib.idle_add(GLib.PRIORITY_HIGH_IDLE, () => {
            this.#relayout = 0;
            this.#layOut();
            return GLib.SOURCE_REMOVE;
        });
    }

    #layOut(): void {
        const { width, height } = this.#pinSize();
        // Not yet known before the first paint on a view; its change lays out again.
        const reported = this.#box.get_resource_scale();
        const scale = reported > 0 ? reported : 1;
        const radius = this.#look.radius;
        const key = `${width}x${height}@${scale}r${radius}`;
        if (key === this.#laidOut) return;
        this.#laidOut = key;
        this.#syncPivot();

        this.#box.destroy_all_children();
        const m = this.#margins;
        const pin: PinShape = { width, height, radius };
        const rects = pieceRects(pin, PIN_SHADOW);
        if (rects === null) {
            // Too small for straight edges: one texture, made for this pin.
            const whole = wholeShadow(pin, scale, PIN_SHADOW);
            this.#box.add_child(this.#piece(contentOf(whole), whole.rect, m));
            return;
        }
        const contents = contentsFor(radius, scale);
        for (const [name, rect] of Object.entries(rects) as [PieceName, (typeof rects)[PieceName]][]) {
            const content = contents.get(name);
            if (content !== undefined) this.#box.add_child(this.#piece(content, rect, m));
        }
    }

    /** One piece, placed in the shadow's own coordinates, its texture stretched to fit. */
    #piece(
        content: Clutter.Content,
        rect: { x: number; y: number; width: number; height: number },
        m: Margins,
    ): Clutter.Actor {
        const piece = new Clutter.Actor({
            reactive: false,
            x: rect.x + m.left,
            y: rect.y + m.top,
            width: rect.width,
            height: rect.height,
            content,
            content_gravity: Clutter.ContentGravity.RESIZE_FILL,
        });
        return piece;
    }
}

/** Every pin's shadow, by window. */
export class PinShadows {
    #shadows = new Map<Meta.Window, WindowShadow>();
    #restacked = 0;
    #uiAdded = 0;

    /**
     * `SetWindowShadow`: gives a window a shadow, changes the one it has, or with an
     * opacity of 0 takes it away.
     */
    set(window: Meta.Window, look: ShadowLook): void {
        const existing = this.#shadows.get(window);
        if (look.opacity <= 0) {
            existing?.destroy();
            this.#shadows.delete(window);
            this.#stopWatching();
            return;
        }
        if (existing !== undefined) {
            existing.update(look);
            return;
        }
        const actor = window.get_compositor_private<Meta.WindowActor>();
        if (actor === null) throw new Error('the window has no actor to shade');
        const shadow = new WindowShadow(window, actor, look, () => {
            this.#shadows.delete(window);
            this.#stopWatching();
        });
        this.#shadows.set(window, shadow);
        this.#watch();
    }

    /** How many windows have one, for the log and the harness. */
    get count(): number {
        return this.#shadows.size;
    }

    destroy(): void {
        for (const shadow of this.#shadows.values()) shadow.destroy();
        this.#shadows.clear();
        this.#stopWatching();
        // `spec/10` §2: nothing of the extension's outlives disable(), its textures included.
        pieceContents.clear();
    }

    #watch(): void {
        if (this.#restacked !== 0) return;
        this.#restacked = global.display.connect('restacked', () => {
            for (const shadow of this.#shadows.values()) {
                try {
                    shadow.restack();
                } catch (e) {
                    error('could not put a pin shadow back under its pin', e);
                }
            }
        });
        this.#uiAdded = Main.uiGroup.connect('child-added', (_group, child) => {
            try {
                this.#shadeClones(child);
            } catch (e) {
                error('could not shade the pins in a workspace switch', e);
            }
        });
    }

    #stopWatching(): void {
        if (this.#shadows.size > 0 || this.#restacked === 0) return;
        global.display.disconnect(this.#restacked);
        this.#restacked = 0;
        Main.uiGroup.disconnect(this.#uiAdded);
        this.#uiAdded = 0;
    }

    /**
     * Shadows for the clones of shaded windows in a group just added to the stage. A
     * workspace switch's group holds its clones three levels down at most: the monitor's
     * group, a workspace's, the clone.
     */
    #shadeClones(root: Clutter.Actor): void {
        const byActor = new Map<Clutter.Actor, WindowShadow>();
        for (const shadow of this.#shadows.values()) byActor.set(shadow.actor, shadow);
        let shaded = 0;
        const walk = (actor: Clutter.Actor, depth: number): void => {
            for (const child of actor.get_children()) {
                if (child instanceof Clutter.Clone) {
                    const shadow = byActor.get(child.get_source());
                    if (shadow === undefined) continue;
                    shadow.shadeClone(child);
                    shaded++;
                } else if (depth > 1) {
                    walk(child, depth - 1);
                }
            }
        };
        walk(root, 3);
        if (shaded > 0) info(`pin shadows put under ${shaded} copied pins`);
    }
}
