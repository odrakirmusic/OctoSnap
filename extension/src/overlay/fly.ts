// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * `spec/03` §7 step 5: the fly-to-corner animation.
 *
 * The one "hero" animation in the product (`spec/09` §1 item 5). An actor showing the
 * captured pixels starts at the selection rect and scales and translates to where the
 * card will appear -- 400 ms, ease-out-cubic, opacity 1 -> 0.9 (`spec/03` §10).
 *
 * **It never delays the D-Bus call, and the D-Bus call does not delay it.** `spec/03` §7
 * is explicit: the app is told immediately and times its card's fade-in to the animation's
 * end, which is why `animation_ms` is in the capture meta (`spec/10` §3.3). So this
 * function starts the animation and returns. The caller starts it the moment the pixels
 * are read and tells the app once their file is written, which for a large capture is
 * most of a second later (D127); the card counts the flight from the meta's `timestamp`,
 * stamped as the fly set off.
 *
 * The card itself is `M2`. What exists today is the animation and the slot it aims at,
 * which is the half that has to live in the extension either way: the app cannot animate
 * from a rect on the compositor's stage.
 */

import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import { error } from '../log.js';
import { tellPets } from '../pets/events.js';
import { createOverlayHost } from './host.js';
import { FROZEN_FADE, type Motion, easing, played } from './motion.js';
import { type Rect, cardSlot, flyLanding } from './slot.js';

/**
 * `spec/03` §10: "Fly-to-corner after capture | 350-450 ms | ease-out-cubic", and
 * `spec/09` §3's row: 400 ms.
 */
const FLY: Motion = { duration: 400, mode: Clutter.AnimationMode.EASE_OUT_CUBIC };

/** `spec/03` §10: "opacity 1->0.9". Clutter counts opacity in 0-255. */
const END_OPACITY = Math.round(255 * 0.9);

/**
 * How a landed picture gives way to a card that came late. The frozen frame's fade, and
 * for the same reason: long enough to read as one picture becoming the other, short
 * enough that the two are not seen side by side. It leaves, so it eases in.
 */
const LATE_FADE: Motion = FROZEN_FADE;

export interface FlyOptions {
    /** The capture's PNG in the spool. Shown when there are no `pixels`, once written. */
    path: string;
    /** The captured rect, in logical stage coordinates. */
    rect: Rect;
    /** The monitor scale the capture was taken at. */
    scale: number;
    /**
     * The captured pixels on the GPU, cut to the capture (`StartedCapture` in
     * `capture.ts`): the live repaint's texture, or the frozen screen's.
     *
     * Preferred over the file whenever they exist, and not only to save a decode: they
     * *are* the pixels going into the file, so the animation starts on the very next frame
     * without waiting for the encode, and shows exactly what was captured. The file is the
     * fallback for a window capture, which `screenshot_window` reads and encodes in one
     * call and holds nothing of on the GPU afterwards -- a fresh stage snapshot would show
     * the screen as it is *now*, which after a window capture is a different picture.
     */
    pixels: Clutter.Content | null;
    /**
     * Settles once the app has been told of the capture. A picture that lands first stays
     * where the card will be until then, instead of leaving the corner empty for however
     * long the rest of the encode takes.
     */
    handedOver?: Promise<unknown>;
}

/**
 * Starts the animation and returns how long it will run, in milliseconds.
 *
 * Returns 0 when it could not start, and the caller then reports `animation_ms: 0` so the
 * app shows its card immediately rather than waiting for an animation nobody can see.
 * A capture that succeeded must never be lost to a failed animation, so every step here
 * is inside the try.
 *
 * With animations off the flight takes no time at all -- gnome-shell plays the `ease` in
 * none -- and so the duration returned is none either (`motion.ts`'s `played`). The app
 * waits `animation_ms` before its card, and it used to wait 400 ms for a flight the user
 * never saw.
 */
export function flyToCorner(options: FlyOptions): number {
    try {
        const { rect, pixels } = options;
        if (rect.width <= 0 || rect.height <= 0) return 0;

        const index = monitorIndexFor(rect);
        const workArea = Main.layoutManager.getWorkAreaForMonitor(index);
        const target = cardSlot({
            x: workArea.x,
            y: workArea.y,
            width: workArea.width,
            height: workArea.height,
        });
        const landing = flyLanding(rect, target);

        // Whether the app has been told by the time the picture lands, known without
        // waiting on the promise then: a callback registered now runs before one
        // registered at the landing.
        const handedOver: Handover = {
            done: options.handedOver === undefined,
            promise: options.handedOver,
        };
        options.handedOver?.then(
            () => (handedOver.done = true),
            () => (handedOver.done = true),
        );

        // The cell is what moves. Its child is the picture, which is the captured pixels
        // or the PNG loaded back, either sized to fill the cell.
        const cell = new Clutter.Actor({
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
            clip_to_allocation: true,
            reactive: false,
        });

        const picture = pixels !== null
            ? pixelsPicture(pixels, rect)
            : filePicture(options.path, rect, options.scale);
        if (picture === null) {
            cell.destroy();
            return 0;
        }

        cell.add_child(picture);

        // Above the overlay, which is still up for another frame or two while its frozen
        // backdrop fades (`spec/03` §7 step 6 releases the grab after this). Adding last
        // puts it on top of the hosts already in the group.
        //
        // **Through a host, not directly** -- `screenshotUIGroup` is a `Clutter.BinLayout`
        // and cannot lay a positioned actor out at all. `host.ts` carries the measurement
        // and `docs/decisions.md` D20 and D24 the history: this was found here first, on
        // an actor that animated perfectly while painting nothing, and it turned out to be
        // breaking the overlay's dim on every monitor away from the stage origin too.
        const host = createOverlayHost('octosnap-fly-host');
        host.add_child(cell);

        // Scale about the top-left, so easing x and y to the landing point puts the
        // shrunken picture exactly there. With a centred pivot the actor would converge on
        // the landing point's centre and overhang it by half the remaining size.
        cell.set_pivot_point(0, 0);

        // `scale_x`, not `scaleX`, and `easing`'s cast is the price of being right rather
        // than type-clean. `@girs` types these keys as the camelCase JS accessors, but
        // gnome-shell's `_easeActor` keeps only the properties for which
        // `actor.find_property(key.replace(/_/g, '-'))` is non-null, and GObject
        // canonicalises underscores while rejecting camelCase. Measured here rather than
        // assumed: `find_property('use_markup')` and `('use-markup')` are both true,
        // `('useMarkup')` is false. A `scaleX` key would therefore be dropped in silence
        // and the picture would slide to the corner at full size. `docs/decisions.md` D19.
        cell.ease(
            easing({
                x: landing.x,
                y: landing.y,
                scale_x: landing.scale,
                scale_y: landing.scale,
                opacity: END_OPACITY,
                duration: FLY.duration,
                mode: FLY.mode,
                onComplete: () => land(cell, host, handedOver),
            }),
        );

        const ms = played(FLY.duration);
        // The pets watch it go (`spec/14` §8). Not when nothing flies.
        if (ms > 0) {
            tellPets({
                kind: 'flew',
                from: rect,
                to: { x: landing.x, y: landing.y, width: rect.width * landing.scale, height: rect.height * landing.scale },
            });
        }
        return ms;
    } catch (e) {
        error('the fly animation could not start; the capture is unaffected', e);
        return 0;
    }
}

/** Whether the app has been told yet, and the promise that says when. */
interface Handover {
    done: boolean;
    promise: Promise<unknown> | undefined;
}

/**
 * The flight is over. A card that is already on its way takes the picture's place as it
 * always did; one that is not stays behind the picture, which holds the slot until the
 * app has been told and then fades as the card comes up where it is.
 */
function land(cell: Clutter.Actor, host: Clutter.Actor, handedOver: Handover): void {
    if (handedOver.done || handedOver.promise === undefined) {
        host.destroy();
        return;
    }
    const fade = () => {
        try {
            cell.ease({
                opacity: 0,
                duration: LATE_FADE.duration,
                mode: LATE_FADE.mode,
                onComplete: () => host.destroy(),
            });
        } catch (e) {
            error('the landed capture could not fade', e);
            host.destroy();
        }
    };
    handedOver.promise.then(fade, fade);
}

/**
 * The captured pixels, filling the cell.
 *
 * The content is in physical pixels and the actor in logical ones: `Clutter.Content`
 * scales itself to its actor, so sizing the actor to the logical rect is what puts the
 * pixels where they belong -- the trick `root.ts`'s `setBackdrop` uses too.
 */
function pixelsPicture(content: Clutter.Content, rect: Rect): Clutter.Actor {
    const picture = new St.Widget({ x: 0, y: 0, width: rect.width, height: rect.height });
    picture.set_content(content);
    return picture;
}

/**
 * The capture, loaded back from the file it was just written to.
 *
 * Asynchronous on purpose. The decode of a full-screen PNG is not something to do on the
 * frame that has to start an animation, and `load_file_async` hands back an actor
 * straight away and fills it in when the pixels arrive. Worst case the first frame or two
 * of the flight are empty, which is a great deal better than a stalled compositor.
 */
function filePicture(path: string, rect: Rect, scale: number): Clutter.Actor | null {
    const file = Gio.File.new_for_path(path);
    const picture = St.TextureCache.get_default().load_file_async(
        file,
        rect.width,
        rect.height,
        1,
        scale,
    );
    if (picture === null) return null;
    picture.set_position(0, 0);
    picture.set_size(rect.width, rect.height);
    return picture;
}

/**
 * The monitor a capture belongs to: the one holding its centre.
 *
 * The centre rather than the origin, because `spec/01` §1 allows a selection to span two
 * monitors of equal scale and the card should appear on whichever one holds most of it.
 * Falls back to the primary rather than throwing -- a card in the wrong corner is a
 * blemish, a thrown exception here would lose the capture.
 */
function monitorIndexFor(rect: Rect): number {
    const cx = rect.x + rect.width / 2;
    const cy = rect.y + rect.height / 2;
    const monitors = Main.layoutManager.monitors;
    for (let i = 0; i < monitors.length; i++) {
        const m = monitors[i];
        if (m === undefined) continue;
        if (cx >= m.x && cx < m.x + m.width && cy >= m.y && cy < m.y + m.height) return i;
    }
    return Main.layoutManager.primaryIndex;
}
