// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Handing a finished capture to the application.
 *
 * Delivered as `org.freedesktop.Application.ActivateAction("handle-capture", …)`, not as
 * a method on the app's own interface. Decision D8, forced by
 * `docs/spikes/13-cold-activation-race.md`: a call to a custom interface is answered by
 * GDBus's worker thread before the app has exported it, if that call is the one that
 * activates the app -- so the first capture after every login would be dropped.
 * `org.freedesktop.Application` is exported during registration and survives a cold start.
 */

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import {
    APP_BUS_NAME,
    APP_GAPPLICATION_PATH,
    FREEDESKTOP_APPLICATION,
} from './protocol.js';
import type { CaptureMeta, Rect } from './spool.js';
import { error, info } from './log.js';
import type { GifOverrides } from './recordChoices.js';

function rectVariant(rect: Rect): GLib.Variant {
    return new GLib.Variant('(iiii)', [rect.x, rect.y, rect.width, rect.height]);
}

/**
 * Builds `spec/10` §3.3's `a{sv}`.
 *
 * Empty optional fields are omitted rather than sent as empty strings, matching the
 * `skip_serializing_if` on the Rust side so the D-Bus dictionary and the JSON twin stay
 * the same shape.
 */
export function metaToDict(meta: CaptureMeta): Record<string, GLib.Variant> {
    const dict: Record<string, GLib.Variant> = {
        'path': GLib.Variant.new_string(meta.path),
        'meta_path': GLib.Variant.new_string(meta.meta_path),
        'mode': GLib.Variant.new_string(meta.mode),
        'rect': rectVariant(meta.rect),
        'scale': GLib.Variant.new_double(meta.scale),
        'display': GLib.Variant.new_string(meta.display),
        'window_alpha': GLib.Variant.new_boolean(meta.window_alpha),
        'timestamp': GLib.Variant.new_uint64(meta.timestamp),
        // The start of `spec/00` §9's capture-to-card budget, which the card measures
        // from. The twin had it from M2, and this dictionary -- what the app reads -- did
        // not, so the card's budget line never printed (`app.test.ts` holds the two to
        // the keys `capture.rs` reads).
        'confirmed_at': GLib.Variant.new_uint64(meta.confirmed_at),
        'animation_ms': GLib.Variant.new_uint32(meta.animation_ms),
        'modifiers': GLib.Variant.new_uint32(meta.modifiers),
    };

    if (meta.cursor_rect) dict['cursor_rect'] = rectVariant(meta.cursor_rect);
    if (meta.app_id) dict['app_id'] = GLib.Variant.new_string(meta.app_id);
    if (meta.app_name) dict['app_name'] = GLib.Variant.new_string(meta.app_name);
    if (meta.window_title) dict['window_title'] = GLib.Variant.new_string(meta.window_title);
    if (meta.requested_action)
        dict['requested_action'] = GLib.Variant.new_string(meta.requested_action);
    // `!== undefined` and not a truthiness test: `false` is `capture-text-single-line`
    // asking for exactly that, and dropping it would send the user's setting instead.
    if (meta.linebreaks !== undefined)
        dict['linebreaks'] = GLib.Variant.new_boolean(meta.linebreaks);

    return dict;
}

/** The same dictionary as a single `a{sv}` variant, for a method argument. */
export function metaToVariant(meta: CaptureMeta): GLib.Variant {
    return new GLib.Variant('a{sv}', metaToDict(meta));
}

/**
 * Activates one of the app's GApplication actions, starting the app if it is not running.
 *
 * Fire and forget by design, and that is the contract rather than a shortcut:
 * `ActivateAction` has no reply that carries the action's own result, so nothing the app
 * decides afterwards can come back here. Anything the *user* needs to know about a
 * failure is the app's to say, in the journal or in a notification (`spec/10` §8).
 *
 * The reply is still collected, but only to log whether the message was delivered, and in
 * a callback rather than awaited -- blocking the compositor's main loop on a D-Bus round
 * trip is what `spec/10` §7's 2 ms budget exists to prevent.
 */
export function activateApp(
    action: string,
    parameter: GLib.Variant | null = null,
    describe = action,
): void {
    Gio.DBus.session.call(
        APP_BUS_NAME,
        APP_GAPPLICATION_PATH,
        FREEDESKTOP_APPLICATION,
        'ActivateAction',
        new GLib.Variant('(sava{sv})', [
            action,
            parameter === null ? [] : [parameter],
            {},
        ]),
        null,
        Gio.DBusCallFlags.NONE,
        5000,
        null,
        (connection, result) => {
            try {
                connection!.call_finish(result!);
                info(`app took '${describe}'`);
            } catch (e) {
                error(`app did not take '${describe}'`, e);
            }
        },
    );
}

/**
 * Tells the app about a capture, starting it if it is not running.
 *
 * The capture is not lost if this fails: it is in the spool with its twin (`spec/10` §8),
 * and the app recovers from there on its next start.
 */
export function notifyCapture(meta: CaptureMeta): void {
    activateApp('handle-capture', metaToVariant(meta), `capture ${meta.path}`);
}

/**
 * Asks the app to record `rect` as a GIF, starting it if it is not running (`spec/06` §3).
 *
 * A recording is a *request*, not pixels: the extension owns the area selection and the
 * countdown, and the app owns the ScreenCast session, the encoder and the files. Delivered
 * as the `record` GApplication action for the same reason captures use `handle-capture` --
 * `org.freedesktop.Application` survives a cold start where a custom-interface call would be
 * lost (D8, `docs/spikes/13`).
 *
 * `display` is deliberately omitted: the extension cannot resolve a connector
 * (`docs/spikes/15`), so the app resolves the monitor from the rectangle. `cursor` is
 * omitted too, so the app's `rec-cursor` setting decides. `record-format` is `gif` so a
 * future build that also records video (M9) can tell the two apart.
 */
export function notifyRecord(rect: Rect, gif: GifOverrides = {}): void {
    activateApp(
        'record',
        new GLib.Variant('a{sv}', recordDict(rect, gif)),
        `record ${rect.width}x${rect.height}`,
    );
}

/**
 * The `record` and `arm-record` payload: the rectangle, the format, and only the GIF
 * settings the user changed on the toolbar's recording row (D101). An absent key leaves
 * the user's setting alone -- `spec/10` §3.1's rule for `BeginCapture`, kept here -- so a
 * row nobody touched sends exactly what the hotkey sends.
 */
function recordDict(rect: Rect, gif: GifOverrides): Record<string, GLib.Variant> {
    const dict: Record<string, GLib.Variant> = {
        'rect': rectVariant(rect),
        'record-format': GLib.Variant.new_string('gif'),
    };
    if (gif.fps !== undefined) dict['fps'] = GLib.Variant.new_int32(gif.fps);
    if (gif.maxWidth !== undefined) dict['max-width'] = GLib.Variant.new_int32(gif.maxWidth);
    if (gif.quality !== undefined) dict['quality'] = GLib.Variant.new_int32(gif.quality);
    if (gif.cursor !== undefined) dict['cursor'] = GLib.Variant.new_boolean(gif.cursor);
    return dict;
}

/**
 * Asks the app to set a recording of `rect` up *now*, to be begun when the countdown ends.
 *
 * The countdown is three seconds the user is made to wait anyway (`spec/06` §3), and the
 * app's setup -- ScreenCast session, PipeWire node, GStreamer graph, gifski encoder, the
 * stream's own negotiation -- is a quarter to half a second that used to run after it. Sent
 * here, the two overlap and Stop-to-first-frame becomes an atomic store (D70). Cold starts
 * are absorbed too: this is what activates the app, so its launch happens over the numbers.
 *
 * Best-effort in both directions. An app too old to know the action ignores it, and the
 * `record` that follows starts a recording the slow way; an arming that is never followed
 * by a `record` is given back by the app's own guard as well as by `cancelRecord`.
 */
export function armRecord(rect: Rect, gif: GifOverrides = {}): void {
    activateApp(
        'arm-record',
        new GLib.Variant('a{sv}', recordDict(rect, gif)),
        `arm ${rect.width}x${rect.height}`,
    );
}

/** Tells the app the countdown was cancelled, so it gives the armed session back. */
/**
 * `spec/07` §1.1: the user has chosen an area to scroll-capture.
 *
 * Like `notifyRecord`, this hands over a *request* rather than pixels: the app owns the
 * loop (`spec/10` §7, "move anything loop-shaped to the app"), puts the controls up and
 * asks the extension for the frames. Absent options are left out
 * rather than sent as defaults, so a shortcut that says nothing about the direction gets
 * the user's setting and not the wire's.
 */
export function startScrollCapture(rect: Rect, options: ScrollOptions = {}): void {
    const dict: Record<string, GLib.Variant> = { 'rect': rectVariant(rect) };
    if (options.direction !== undefined) {
        dict['direction'] = GLib.Variant.new_string(options.direction);
    }
    if (options.start === true) dict['start'] = GLib.Variant.new_boolean(true);
    activateApp(
        'scroll-capture',
        new GLib.Variant('a{sv}', dict),
        `scroll ${rect.width}x${rect.height}`,
    );
}

export interface ScrollOptions {
    /** `down`, `up`, `right` or `left`. */
    direction?: string;
    /** `spec/07`'s URL parameter `start`: begin without waiting for the button. */
    start?: boolean;
}

export function cancelRecord(): void {
    activateApp('cancel-record', null, 'cancel recording');
}

/** Asks the app to stop the recording in progress (`spec/06` §3's Stop). */
export function stopRecording(): void {
    activateApp('stop-recording', null, 'stop recording');
}
