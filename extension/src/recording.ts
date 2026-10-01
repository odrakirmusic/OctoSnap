// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The compositor half of a recording (`spec/06` §3, §4): the red frame around the recorded
 * region, the panel timer, the desktop icons hidden for its length, and the watchdog that
 * puts the desktop back if the app dies mid-recording.
 *
 * The app drives this over the bus: `SetRecordingState` (idle → recording → processing →
 * idle) and `ShowRecordingFrame` land in `dbus.ts` and call straight through to here. The
 * app owns the ScreenCast session and the pill it can place outside the rectangle; this
 * owns everything that has to be shell code -- an actor above every window, the top-bar
 * button, DING's window actors, and a name-watch that only the compositor process can keep
 * running after the app is gone.
 *
 * Why the frame is never in the GIF: the recorder crops the monitor's stream to the
 * selection, and the frame is placed in that stream's own pixels, with its stroke on the
 * far side of every pixel the crop takes, whatever the scale (`outline.ts`, D115).
 */

import Gio from 'gi://Gio';
import St from 'gi://St';

import { allMonitors, stageSize } from './capture.js';
import type { DesktopIcons } from './desktop.js';
import { outlineRect } from './outline.js';
import type { Rect } from './place.js';
import { APP_BUS_NAME } from './protocol.js';
import { elapsedLabel } from './recording-format.js';
import { createOverlayHost } from './overlay/host.js';
import type { Settings } from './settings.js';
import { error, info } from './log.js';
import { tellPets } from './pets/events.js';

/** `spec/06` §3: a red outline around the recorded region. #e01b24 is `spec/09`'s red. */
const FRAME_BORDER = 3;
const FRAME_STYLE = `border: ${FRAME_BORDER}px solid #e01b24; border-radius: 2px;`;

/**
 * What the panel indicator has to do for a recording, kept to the two calls this needs so
 * the coordinator does not depend on the whole `Indicator`. The `Indicator` implements it.
 */
export interface RecordingPanel {
    /**
     * Turn the top-bar button red with `label`, and offer Stop; or restore it when off.
     * `saving` puts a spinner where the red dot is while the file is written.
     */
    setRecording(active: boolean, label: string, saving?: boolean): void;
}

/**
 * Holds the on-screen recording state. One per extension, created in `enable`, torn down in
 * `disable` -- which restores the desktop like the watchdog does, because a recording in
 * progress when the extension is disabled must not leave hidden icons or a red frame behind.
 */
export class RecordingCoordinator {
    #settings: Settings;
    #desktopIcons: DesktopIcons;
    #panel: RecordingPanel;

    #frame: St.Widget | null = null;

    /** The frame's FixedLayout host (`overlay/host.ts`); owns the frame. */

    #frameHost: St.Widget | null = null;
    #watchId = 0;
    /** The app was seen owning the bus name, so a later vanish is a crash, not a slow start. */
    #appeared = false;
    #active = false;

    constructor(deps: { settings: Settings; desktopIcons: DesktopIcons; panel: RecordingPanel }) {
        this.#settings = deps.settings;
        this.#desktopIcons = deps.desktopIcons;
        this.#panel = deps.panel;
    }

    /** Whether a recording is in progress, for the record shortcut's "one at a time" guard. */
    isActive(): boolean {
        return this.#active;
    }

    /**
     * `spec/10` §3.1's `SetRecordingState`. The four states M5 uses collapse to two here:
     * anything that is not idle means "a recording is on screen", and idle tears it down.
     * `processing` is Stop pressed with the GIF still being written, so the timer freezes
     * to a "Saving…" while the panel stays red.
     */
    setState(wire: string, elapsedMs: number): void {
        switch (wire) {
            case 'countdown':
            case 'recording':
            case 'processing':
                this.#begin();
                this.#panel.setRecording(
                    true,
                    wire === 'processing' ? 'Saving…' : elapsedLabel(elapsedMs),
                    wire === 'processing',
                );
                break;
            case 'idle':
            default:
                this.#end();
                break;
        }
    }

    /** `spec/10` §3.1's `ShowRecordingFrame`: the red outline appears, moves, or goes. */
    showFrame(rect: Rect, visible: boolean): void {
        if (!visible) {
            this.#dropFrame();
            return;
        }
        const outer = outlineRect(rect, FRAME_BORDER, allMonitors(), stageSize());
        if (this.#frame === null) {
            // On the screenshot UI group, like the countdown: a high layer above every
            // application window. It is outside the crop regardless, so the layer is for
            // stacking, not for keeping it out of the GIF. **Through a host, not
            // directly**: the group is a `Clutter.BinLayout`, which lays a positioned
            // child out from the wrong numbers -- `(x, y, width, height)` read as
            // `(x1, y1, x2, y2)` (`overlay/host.ts`, D24). Added to the group directly, a
            // frame for 300,200 640x400 came out 347x205 and one for 900,700 400x240 not
            // at all; a `FixedLayout` host lays it out where it says (nested shell,
            // 2026-09-14).
            this.#frameHost = createOverlayHost('octosnap-recording-frame');
            this.#frame = new St.Widget({ style: FRAME_STYLE, reactive: false });
            this.#frameHost.add_child(this.#frame);
        }
        this.#frame.set_position(outer.x, outer.y);
        this.#frame.set_size(outer.width, outer.height);
        this.#frame.show();
    }

    #begin(): void {
        if (this.#active) return;
        this.#active = true;
        // `REC-08`: default on, and the extension's own setting decides -- the app never
        // touches DING.
        this.#desktopIcons.beginRecording(this.#settings.hideDesktopWhileRecording);
        this.#watch();
        info('recording state: on');
    }

    #end(): void {
        if (!this.#active && this.#frame === null && this.#watchId === 0) return;
        this.#active = false;
        this.#appeared = false;
        this.#unwatch();
        this.#dropFrame();
        this.#desktopIcons.endRecording();
        this.#panel.setRecording(false, '');
        tellPets({ kind: 'recording-stopped' });
        info('recording state: off');
    }

    #dropFrame(): void {
        try {
            // The host owns the frame: destroying it takes the frame with it.
            this.#frameHost?.destroy();
        } catch (e) {
            error('could not remove the recording frame', e);
        }
        this.#frameHost = null;
        this.#frame = null;
    }

    /**
     * `spec/06` §9 item 10 and `spec/07` §7 item 7: the desktop is restored "even if the
     * app crashes". Only the compositor can promise that, because only it outlives the app,
     * so the extension watches the app's bus name for the length of a recording and, if it
     * vanishes after having been there, runs the same teardown Stop would.
     */
    #watch(): void {
        if (this.#watchId !== 0) return;
        this.#appeared = false;
        this.#watchId = Gio.bus_watch_name(
            Gio.BusType.SESSION,
            APP_BUS_NAME,
            Gio.BusNameWatcherFlags.NONE,
            () => {
                this.#appeared = true;
            },
            () => {
                // A vanish before we ever saw the name is the initial "not owned" callback,
                // not a crash. Only a vanish *after* an appear, while still recording, is
                // the app going down under a live recording.
                if (this.#active && this.#appeared) {
                    error('the app left the bus mid-recording; restoring the desktop');
                    this.#end();
                }
            },
        );
    }

    #unwatch(): void {
        if (this.#watchId !== 0) {
            Gio.bus_unwatch_name(this.#watchId);
            this.#watchId = 0;
        }
    }

    /** `spec/10` §2: disable() undoes everything -- the frame, the hidden icons, the watch. */
    destroy(): void {
        this.#end();
    }
}
