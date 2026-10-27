// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The calls that differ between the GNOME Shells `metadata.json` names, 46 to 50 (D206).
 *
 * The extension was written on 50 and declared 48 and 49 untried. A user on Ubuntu 24.04,
 * GNOME 46, could not install it, and reading the sources of each release found that 48
 * and 49 could not have loaded it either: the overlay's cursors are a 50 API. Each call
 * here is the one place that knows which release has what. A feature is looked for where
 * it can be; where only an argument changed, the release number is the one thing to go by.
 *
 * | What | 46 | 47 | 48, 49 | 50 |
 * | --- | --- | --- | --- | --- |
 * | a cursor | the display's, `Meta.Cursor` | same | same | the actor's, `Clutter.CursorType` |
 * | an actor's colour | `Clutter.Color` | `Cogl.Color` | same | same |
 * | `St.ImageContent.set_bytes` | no Cogl context | same | context first | same |
 * | `Clutter.Stage.paint_to_content` | rect, scale, flags | same | same | a colour state before the flags |
 * | a texture's Cogl context | the backend's | `Cogl.Texture.get_context` | same | same |
 */

import Clutter from 'gi://Clutter';
import Cogl from 'gi://Cogl';
import type Mtk from 'gi://Mtk';
import Meta from 'gi://Meta';
import St from 'gi://St';
import { PACKAGE_VERSION } from 'resource:///org/gnome/shell/misc/config.js';

/** The running shell's major release: 46 for "46.0". */
const SHELL_MAJOR = Number.parseInt(PACKAGE_VERSION, 10);

/** The cursors the extension shows, by the name of what they are for. */
export type Cursor = 'default' | 'crosshair' | 'move' | 'ns-resize' | 'ew-resize' | 'nwse-resize' | 'nesw-resize';

/** GNOME 50's per-actor cursors, by name: 46 to 49 have no `Clutter.CursorType` to read at import. */
const CURSOR_TYPES: Record<Cursor, string> = {
    'default': 'DEFAULT',
    'crosshair': 'CROSSHAIR',
    'move': 'MOVE',
    'ns-resize': 'NS_RESIZE',
    'ew-resize': 'EW_RESIZE',
    'nwse-resize': 'NWSE_RESIZE',
    'nesw-resize': 'NESW_RESIZE',
};

/** The display's cursors before 50, named as 46's own screenshot UI uses them. */
const META_CURSORS: Record<Cursor, string> = {
    'default': 'DEFAULT',
    'crosshair': 'CROSSHAIR',
    'move': 'MOVE_OR_RESIZE_WINDOW',
    'ns-resize': 'NORTH_RESIZE',
    'ew-resize': 'EAST_RESIZE',
    'nwse-resize': 'NW_RESIZE',
    'nesw-resize': 'NE_RESIZE',
};

/** The shapes of the two cursor APIs, which only one release at a time has. */
interface PerActor {
    set_cursor_type(type: number): void;
}
interface LegacyDisplay {
    set_cursor(cursor: number): void;
}

const perActor = 'set_cursor_type' in Clutter.Actor.prototype;

/** Before 50: the actors that have a cursor, and the one whose cursor is showing. */
const cursors = new WeakMap<Clutter.Actor, Cursor>();
let shown: Clutter.Actor | null = null;

/**
 * `cursor` over `actor` and whatever in it has none of its own, as 50's
 * `Clutter.Actor.set_cursor_type` does.
 *
 * Before 50 there is one cursor, the display's, and it is set as the pointer crosses into
 * and out of an actor that has one: the actor under the pointer is picked and its nearest
 * ancestor with a cursor gives it. That is the rule 50 applies itself. The display is left
 * alone while the pointer is over nothing of ours, and set back to the default when the
 * actor whose cursor it shows is destroyed, so a closed overlay leaves no crosshair.
 */
export function setCursor(actor: Clutter.Actor, cursor: Cursor): void {
    if (perActor) {
        const types = Clutter as unknown as { CursorType: Record<string, number> };
        (actor as unknown as PerActor).set_cursor_type(types.CursorType[CURSOR_TYPES[cursor]]!);
        return;
    }
    const fresh = !cursors.has(actor);
    // The engine sets its cursor on every motion; a pick for each would be spent on nothing.
    if (!fresh && cursors.get(actor) === cursor) return;
    cursors.set(actor, cursor);
    if (fresh) {
        actor.connect('enter-event', () => {
            showCursorUnderPointer();
            return Clutter.EVENT_PROPAGATE;
        });
        actor.connect('leave-event', () => {
            showCursorUnderPointer();
            return Clutter.EVENT_PROPAGATE;
        });
        actor.connect('destroy', () => {
            cursors.delete(actor);
            if (shown === actor) showCursorOf(null);
        });
    }
    showCursorUnderPointer();
}

function showCursorUnderPointer(): void {
    const [x, y] = global.get_pointer();
    let actor: Clutter.Actor | null = global.stage.get_actor_at_pos(Clutter.PickMode.REACTIVE, x, y);
    while (actor !== null && !cursors.has(actor)) actor = actor.get_parent();
    // Nothing of ours under the pointer, and nothing of ours was: not our cursor to set.
    if (actor === null && shown === null) return;
    showCursorOf(actor);
}

function showCursorOf(actor: Clutter.Actor | null): void {
    shown = actor;
    const cursor = (actor !== null ? cursors.get(actor) : undefined) ?? 'default';
    const legacy = Meta as unknown as { Cursor: Record<string, number> };
    (global.display as unknown as LegacyDisplay).set_cursor(legacy.Cursor[META_CURSORS[cursor]]!);
}

/**
 * An actor's `background_color`, from 8-bit channels: a `Cogl.Color` from 47 on, and the
 * `Clutter.Color` 46 still had.
 */
export function actorColour(red: number, green: number, blue: number, alpha: number): Cogl.Color {
    const legacy = (Clutter as unknown as { Color?: new (props: object) => Cogl.Color }).Color;
    const Made = legacy ?? Cogl.Color;
    return new Made({ red, green, blue, alpha });
}

/**
 * Premultiplied RGBA bytes as a texture the stage can draw. 48 asks for the Cogl context
 * first; 46 and 47, where `St.ImageContent` is still a `Clutter.Image`, take none.
 */
export function imageContent(pixels: Uint8Array, width: number, height: number): St.ImageContent {
    const content = St.ImageContent.new_with_preferred_size(width, height) as St.ImageContent;
    const format = Cogl.PixelFormat.RGBA_8888_PRE;
    if (SHELL_MAJOR >= 48) {
        const context = global.stage.get_context().get_backend().get_cogl_context();
        content.set_bytes(context, pixels, format, width, height, width * 4);
    } else {
        const legacy = content as unknown as {
            set_bytes(data: Uint8Array, format: Cogl.PixelFormat, w: number, h: number, stride: number): boolean;
        };
        legacy.set_bytes(pixels, format, width, height, width * 4);
    }
    return content;
}

/** `St.ThemeContext.get_accent_color`, from 47, or null on a shell without accents. */
export function accentColour(): Cogl.Color | null {
    const context = St.ThemeContext.get_for_stage(global.stage) as unknown as {
        get_accent_color?: () => [Cogl.Color, Cogl.Color];
    };
    if (typeof context.get_accent_color !== 'function') return null;
    return context.get_accent_color()[0];
}

/**
 * `rect` of the stage painted at `scale` with `flags`. 50 takes a colour state before the
 * flags, null for the stage's own; 46 to 49 take the flags third, and given four arguments
 * they read the null as the flags and drop the real ones, the pointer and the clear with
 * them.
 */
export function paintToContent(
    stage: Clutter.Stage,
    rect: Mtk.Rectangle,
    scale: number,
    flags: Clutter.PaintFlag,
): Clutter.Content {
    if (SHELL_MAJOR >= 50) return stage.paint_to_content(rect, scale, null, flags);
    const legacy = stage as unknown as {
        paint_to_content(rect: Mtk.Rectangle, scale: number, flags: Clutter.PaintFlag): Clutter.Content;
    };
    return legacy.paint_to_content(rect, scale, flags);
}

/** The Cogl context `texture` belongs to. 46 does not export the texture's own getter. */
export function coglContextOf(texture: Cogl.Texture): Cogl.Context {
    if (typeof texture.get_context === 'function') return texture.get_context();
    return Clutter.get_default_backend().get_cogl_context();
}
