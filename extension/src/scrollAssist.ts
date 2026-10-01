// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * `spec/07` §1.3's scroll assist: the calls a scrolling capture is made of.
 *
 * > `StartScrollAssist(rect, direction, step_px) -> handle`, `ScrollStep(handle)`,
 * > `GrabFrame(handle) -> path`, `EndScrollAssist(handle)`. **The app runs the loop** so
 * > stitching stays in Rust and the shell never blocks.
 *
 * That last sentence is the whole shape of this file. There is no loop here, no waiting
 * for the page to settle and no idea what a stitched image is: one call grabs once, and
 * `spec/10` §7's budget -- 5 ms of the 120 ms iteration -- is what that buys.
 * `ScrollStep`, which scrolled once, went with auto-scroll on 2026-09-27 (D153): the user
 * scrolls. `StartScrollAssist` still takes its `step_px`, and does nothing with it.
 *
 * Two things `docs/spikes/03-virtual-input.md` settled the hard way and that this file
 * depends on:
 *
 * 1. The seat is `Clutter.get_default_backend().get_default_seat()`. `spec/01` §2 row 20's
 *    `global.backend.get_default_seat()` does not exist on GNOME 50 -- GJS reports the
 *    backend as `unknown_MetaBackendNative` and exposes none of the parent's methods.
 * 2. The virtual device must be **held** for the life of the session. A device created
 *    and dropped inside one call is a real hazard. It parks the pointer when the session
 *    starts and puts it back when it ends.
 *
 * And one that is not a bug but reads like one: in a nested `--devkit` shell an injected
 * scroll never reaches a client, because such a session has no real evdev seat. The
 * spike's original conclusion -- that compositor-side injection does not work at all --
 * was that artefact.
 */

import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import St from 'gi://St';

import { allMonitors, captureAreaToFile, stageSize } from './capture.js';
import { createOverlayHost } from './overlay/host.js';
import { outlineRect } from './outline.js';
import { APP_BUS_NAME } from './protocol.js';
import { type Rect } from './spool.js';
import { framesDir } from './spoolRule.js';
import { error, info } from './log.js';

/** `spec/07` §1.1: vertical and horizontal, in both senses. */
export type ScrollDirection = 'down' | 'up' | 'right' | 'left';

const DIRECTIONS: readonly ScrollDirection[] = ['down', 'up', 'right', 'left'];

/** Whether a string off the bus names a direction. */
export function isDirection(value: string): value is ScrollDirection {
    return (DIRECTIONS as readonly string[]).includes(value);
}

/**
 * How long a session may sit untouched before the extension ends it.
 *
 * A held virtual pointer device that nobody ends is the one way this file can leave the
 * session worse than it found it, and the app can die between two calls. Thirty seconds
 * is far longer than any settle (`spec/07` §1.2 waits 400 ms) and far shorter than a user
 * would take to notice.
 */
const IDLE_MS = 30_000;

/**
 * The outline around the region being captured, in `spec/09`'s blue.
 *
 * `spec/07` §1.1 item 2 asks for a scroll cursor inside the area, which Wayland does not
 * allow: the cursor over another client's surface is that client's, and the only way to
 * change it is to put a reactive actor over the area -- which would then swallow the very
 * scrolls the mode exists to watch. What the cursor was *for* is knowing which rectangle
 * is being captured, and once the selection overlay closes there is nothing else saying
 * so. An outline says it, costs no input, and is drawn outside every pixel a capture of the
 * rectangle takes so the captured pixels never carry it -- which at a scale that is not a
 * whole number is not quite the same as outside the rectangle (`outline.ts`, D115). The
 * recorder's red frame (`spec/06` §4.4) is placed by the same helper.
 */
const FRAME_BORDER = 3;
const FRAME_STYLE = `border: ${FRAME_BORDER}px solid #3584e4; border-radius: 2px;`;

/**
 * That outline, round one region. A session draws one from Start to its end; the service
 * draws one for a pill still waiting for Start, which is when the selection overlay has
 * just closed and nothing else says which rectangle the pill is for (D152). The two are
 * the same pixels, so one drawn over the other shows as one.
 *
 * Through a host rather than on the group directly. The screenshot UI group is a
 * `Clutter.BinLayout`, which reads a positioned child's `(x, y, width, height)` as
 * `(x1, y1, x2, y2)` and lays it out from the wrong numbers (`overlay/host.ts`, D24). The
 * recorder's frame learned that the hard way.
 */
export class ScrollFrame {
    #host: St.Widget | null;

    constructor(rect: Rect) {
        const outer = outlineRect(rect, FRAME_BORDER, allMonitors(), stageSize());
        this.#host = createOverlayHost('octosnap-scroll-frame');
        const frame = new St.Widget({ style: FRAME_STYLE, reactive: false });
        frame.set_position(outer.x, outer.y);
        frame.set_size(outer.width, outer.height);
        this.#host.add_child(frame);
    }

    /** Takes it down. The host owns the frame, so destroying it takes both. Idempotent. */
    destroy(): void {
        try {
            this.#host?.destroy();
        } catch (e) {
            error('could not take a scroll frame down', e);
        }
        this.#host = null;
    }
}

/**
 * `spec/07` §1.2: "freeze the pointer position during auto-scroll (move it to the area
 * centre first)", which outlived the auto-scroll it was written for (D153).
 *
 * The wheel scrolls whatever is under the pointer, and at Start the pointer is on the pill,
 * where the user pressed Start. It is put in the middle of the selection, so the first
 * turn of the wheel scrolls the page, and back where it was at the end, which is what
 * `#origin` is for.
 */
function centreOf(rect: Rect): [number, number] {
    return [rect.x + rect.width / 2, rect.y + rect.height / 2];
}

/** One live scroll-assist session. */
export class ScrollSession {
    readonly handle: string;
    readonly rect: Rect;
    readonly direction: ScrollDirection;

    #device: Clutter.VirtualInputDevice | null = null;
    #origin: [number, number] | null = null;
    #frame: ScrollFrame | null = null;
    #dir: string;
    #frames = 0;
    #idle = 0;
    #onIdle: () => void;

    constructor(handle: string, rect: Rect, direction: ScrollDirection, onIdle: () => void) {
        this.handle = handle;
        this.rect = rect;
        this.direction = direction;
        this.#onIdle = onIdle;
        // Where a sandboxed app can read them too (D122).
        this.#dir = framesDir(GLib.get_user_runtime_dir(), APP_BUS_NAME, handle);
    }

    /** Creates the device, remembers where the pointer was, and parks it in the area. */
    start(): void {
        GLib.mkdir_with_parents(this.#dir, 0o700);
        const seat = Clutter.get_default_backend().get_default_seat();
        this.#device = seat.create_virtual_device(Clutter.InputDeviceType.POINTER_DEVICE);
        const [x, y] = global.get_pointer();
        this.#origin = [x, y];
        const [cx, cy] = centreOf(this.rect);
        this.#device.notify_absolute_motion(GLib.get_monotonic_time(), cx, cy);
        this.#frame = new ScrollFrame(this.rect);
        this.#touch();
        info(`scroll assist ${this.handle}: ${this.direction} over ${this.rect.width}x${this.rect.height}`);
    }

    /** One frame of the selection, as a PNG, and where it landed. */
    async grab(): Promise<string> {
        this.#touch();
        const path = GLib.build_filenamev([this.#dir, `${String(this.#frames).padStart(4, '0')}.png`]);
        this.#frames += 1;
        await captureAreaToFile(this.rect, path);
        return path;
    }

    /** Drops the device, puts the pointer back, and takes the frames with it. */
    end(): void {
        if (this.#idle !== 0) {
            GLib.source_remove(this.#idle);
            this.#idle = 0;
        }
        if (this.#device !== null && this.#origin !== null) {
            const [x, y] = this.#origin;
            this.#device.notify_absolute_motion(GLib.get_monotonic_time(), x, y);
        }
        // Dropping the reference is what releases the device; there is no close().
        this.#device = null;
        this.#origin = null;
        this.#frame?.destroy();
        this.#frame = null;
        removeTree(this.#dir);
        info(`scroll assist ${this.handle}: ended after ${this.#frames} frames`);
    }

    /** Restarts the idle watchdog. Every call the app makes counts as a sign of life. */
    #touch(): void {
        if (this.#idle !== 0) GLib.source_remove(this.#idle);
        this.#idle = GLib.timeout_add(GLib.PRIORITY_DEFAULT, IDLE_MS, () => {
            this.#idle = 0;
            error(`scroll assist ${this.handle}: no call for ${IDLE_MS} ms, ending it`);
            this.#onIdle();
            return GLib.SOURCE_REMOVE;
        });
    }
}

/**
 * Deletes a directory of frames.
 *
 * `Gio.File.delete` refuses a directory with anything in it, and the frames are the
 * point, so the children go first. Failures are logged rather than thrown: this runs from
 * `end()`, which runs from `destroy()`, and a capture that cannot tidy up is not a reason
 * to leave a virtual pointer device held.
 */
function removeTree(path: string): void {
    const dir = Gio.File.new_for_path(path);
    try {
        const children = dir.enumerate_children('standard::name', Gio.FileQueryInfoFlags.NONE, null);
        let info_ = children.next_file(null);
        while (info_ !== null) {
            dir.get_child(info_.get_name()).delete(null);
            info_ = children.next_file(null);
        }
        children.close(null);
        dir.delete(null);
    } catch (e) {
        error(`could not remove ${path}`, e);
    }
}
