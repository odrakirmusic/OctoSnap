// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * `CAP-03`: pick a window, capture the window.
 *
 * `spec/03` §5.2's window mode: every window under the pointer is highlighted as the
 * pointer moves over it, click confirms, Esc cancels. Tab and Shift+Tab move the highlight
 * from window to window, topmost first, and Enter confirms it, so that a window can be
 * captured without the mouse (`spec/00` §9, D137). The highlight is one actor moved
 * around rather than one per window, for the same reason the dim in `root.ts` is four
 * rects: motion events arrive at frame rate and `spec/10` §7 forbids allocating on that
 * path.
 *
 * The one real subtlety is *which* windows count. `global.get_window_actors()` returns
 * every actor the compositor has, which on Ubuntu includes the desktop-icon window that
 * the DING extension paints across the whole primary monitor. Offering that as a
 * capturable window would mean the topmost hit under the pointer is almost always DING,
 * and window mode would look broken while being technically correct.
 */

import Clutter from 'gi://Clutter';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import St from 'gi://St';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import { OverlayRoot, type MonitorGeometry } from './root.js';
import { createOverlayHost } from './host.js';
import { HIGHLIGHT_MOVE, OVERLAY_CANCEL, OVERLAY_OPEN, easing, fadeIn, fadeOutAndDrop } from './motion.js';
import { cycle, type Rect } from './selection.js';
import { isFloating } from '../cards.js';
import { error, info } from '../log.js';
import { tellPets } from '../pets/events.js';

const HINT = 'Point at a window or press Tab · Click or Enter to capture · Esc to cancel';

/** `spec/03` §4: the highlight is the selection frame plus a faint accent wash. */
const HIGHLIGHT_STYLE =
    'border: 2px solid rgba(255,255,255,0.95); ' +
    'background-color: rgba(120,170,255,0.14); ' +
    'box-shadow: 0 0 0 1px rgba(0,0,0,0.35);';

/**
 * Window types that are never a capture target.
 *
 * `DESKTOP` is the important one: it is what DING's icon window is, and it covers the
 * whole primary monitor, so leaving it in makes every hover land on the desktop instead
 * of the window the user is pointing at. The rest are chrome that cannot be meaningfully
 * captured on its own.
 */
const EXCLUDED_TYPES = new Set<Meta.WindowType>([
    Meta.WindowType.DESKTOP,
    Meta.WindowType.DOCK,
    Meta.WindowType.SPLASHSCREEN,
    Meta.WindowType.MENU,
    Meta.WindowType.DROPDOWN_MENU,
    Meta.WindowType.POPUP_MENU,
    Meta.WindowType.TOOLTIP,
    Meta.WindowType.NOTIFICATION,
    Meta.WindowType.COMBO,
    Meta.WindowType.DND,
    Meta.WindowType.OVERRIDE_OTHER,
]);

export interface WindowSelection {
    window: Meta.Window;
    /**
     * The rect the capture will actually contain, in logical stage coordinates.
     *
     * This is the **buffer** rect, not the frame rect, and the difference is not cosmetic:
     * `spec/10` §3.3 promises that `path` is `rect` x `scale` in physical pixels, and the
     * app computes `ACT-07`'s `{w}` and `{h}` tokens from it. `screenshot_window` with
     * `include_frame` captures the window's shadow too, so a frame rect here made the
     * twin claim 400x300 for a PNG that was 450x350 -- caught by comparing the two.
     */
    rect: Rect;
    monitor: MonitorGeometry;
    /** Modifier mask at confirm, for `CAP-15`. */
    modifiers: number;
}

/** One capturable window, the rect drawn around it, and the rect that gets captured. */
interface Candidate {
    window: Meta.Window;
    /** What the user sees and points at: the window's visible edge, without its shadow. */
    frame: Rect;
    /** What the capture will contain, shadow included. */
    buffer: Rect;
}

/** The pickers that hold a grab, from taking it to `destroy` (D135). */
const open = new Set<WindowPicker>();

/**
 * For `disable()`: every open picker goes at once, grab and actors, and whoever awaits it
 * hears a cancel, as `closeOverlays` does for the area overlay and for the same reason.
 */
export function closeWindowPickers(): void {
    for (const picker of [...open]) picker.destroy();
}

export class WindowPicker {
    #roots: OverlayRoot[] = [];

    /** See `host.ts`: `screenshotUIGroup` cannot lay a positioned actor out. */
    #host: St.Widget | null = null;
    #highlight: St.Widget | null = null;
    #grab: Clutter.Grab | null = null;
    #resolve: ((value: WindowSelection | null) => void) | null = null;
    #settled = false;
    /** A cancelled picker leaves over `spec/09` §3's 100 ms; a chosen one goes at once. */
    #cancelled = false;

    #candidates: Candidate[] = [];
    #hovered: Candidate | null = null;
    /**
     * What was under the pointer at its last motion. The pointer takes the highlight back
     * only by moving onto another window: any motion used to, and a mouse nudged by a
     * millimetre over the desktop threw away the window Tab had chosen (D137).
     */
    #underPointer: Candidate | null = null;

    open(): Promise<WindowSelection | null> {
        return new Promise(resolve => {
            this.#resolve = resolve;
            this.#candidates = capturableWindows();

            if (this.#candidates.length === 0) {
                info('no capturable windows');
                this.#settle(null);
                return;
            }

            const monitors: MonitorGeometry[] = Main.layoutManager.monitors.map((m, index) => ({
                index,
                x: m.x,
                y: m.y,
                width: m.width,
                height: m.height,
                scale: global.display.get_monitor_scale(index),
            }));

            this.#host = createOverlayHost('octosnap-window-picker-host');

            for (const monitor of monitors) {
                const root = new OverlayRoot(monitor);
                // No dim in window mode: `spec/03` §5.2 highlights the target instead, and
                // dimming everything would hide the very thing being pointed at.
                root.setDimVisible(false);
                root.actor.connect('event', (_a, event) => this.#onEvent(event));
                root.actor.set_cursor_type(Clutter.CursorType.CROSSHAIR);
                this.#host.add_child(root.actor);
                this.#roots.push(root);
            }

            const current = global.display.get_current_monitor();
            this.#roots[current]?.setHint(HINT);
            // On the host, in stage coordinates. It used to hang off `#roots[0]` with
            // that monitor's origin subtracted by hand, which drew in the right place only
            // because Clutter does not clip to the allocation -- a highlight on the second
            // monitor was painting outside its parent's box the whole time. The host
            // *is* the stage, so there is no origin to subtract and nothing to get wrong.
            this.#highlight = new St.Widget({ style: HIGHLIGHT_STYLE, visible: false });
            this.#host.add_child(this.#highlight);
            // `spec/09` §3's open, as the area overlay's (`engine.ts`).
            fadeIn(this.#host, OVERLAY_OPEN);

            const primary = this.#roots[current] ?? this.#roots[0];
            if (!primary) {
                this.#settle(null);
                return;
            }
            this.#grab = Main.pushModal(primary.actor, {
                actionMode: Shell.ActionMode.SYSTEM_MODAL,
            });
            open.add(this);
            // As over an area selection: the pets step aside (`pets/crew.ts`).
            tellPets({ kind: 'overlay', open: true });
            primary.actor.grab_key_focus();

            // Highlight whatever is already under the pointer, so the overlay does not
            // open looking empty until the user twitches the mouse.
            const [x, y] = global.get_pointer();
            this.#hover(x, y);

            info(`window picker open over ${this.#candidates.length} window(s)`);
        });
    }

    #onEvent(event: Clutter.Event): boolean {
        switch (event.type()) {
            case Clutter.EventType.MOTION: {
                const [x, y] = event.get_coords();
                this.#hover(Math.round(x), Math.round(y));
                return Clutter.EVENT_STOP;
            }
            case Clutter.EventType.BUTTON_PRESS: {
                if (event.get_button() !== Clutter.BUTTON_PRIMARY || this.#hovered === null) {
                    this.#settle(null);
                    return Clutter.EVENT_STOP;
                }
                this.#choose(this.#hovered, event.get_state());
                return Clutter.EVENT_STOP;
            }
            case Clutter.EventType.KEY_PRESS: {
                const symbol = event.get_key_symbol();
                if (symbol === Clutter.KEY_Escape) {
                    this.#settle(null);
                } else if (symbol === Clutter.KEY_Tab || symbol === Clutter.KEY_ISO_Left_Tab) {
                    const back =
                        symbol === Clutter.KEY_ISO_Left_Tab ||
                        (event.get_state() & Clutter.ModifierType.SHIFT_MASK) !== 0;
                    this.#show(cycle(this.#candidates, this.#hovered, back ? -1 : 1));
                } else if (
                    symbol === Clutter.KEY_Return ||
                    symbol === Clutter.KEY_KP_Enter ||
                    symbol === Clutter.KEY_ISO_Enter
                ) {
                    // With nothing highlighted Enter waits, where a click on nothing
                    // cancels: a click there points at the desktop, Enter points nowhere.
                    if (this.#hovered !== null) this.#choose(this.#hovered, event.get_state());
                }
                return Clutter.EVENT_STOP;
            }
            default:
                return Clutter.EVENT_PROPAGATE;
        }
    }

    /**
     * Topmost candidate containing the point wins, which is what the user can see.
     *
     * Hit-tested against the **frame**, not the buffer: a window's shadow is transparent,
     * so a pointer over it is visually over whatever is behind, and hit-testing the
     * buffer would make windows grab a ~25 px halo of their neighbours.
     */
    #hover(x: number, y: number): void {
        const found =
            this.#candidates.find(
                c =>
                    x >= c.frame.x &&
                    x < c.frame.x + c.frame.width &&
                    y >= c.frame.y &&
                    y < c.frame.y + c.frame.height,
            ) ?? null;
        if (found === this.#underPointer) return;
        this.#underPointer = found;
        this.#show(found);
    }

    /** The click's confirm and Enter's: `found` is what gets captured. */
    #choose(found: Candidate, modifiers: number): void {
        const root = this.#roots[found.window.get_monitor()] ?? this.#roots[0];
        if (!root) {
            this.#settle(null);
            return;
        }
        this.#settle({ window: found.window, rect: found.buffer, monitor: root.monitor, modifiers });
    }

    /** Highlights `found`, whether the pointer went there or Tab did. */
    #show(found: Candidate | null): void {
        if (found === this.#hovered) return;
        this.#hovered = found;

        if (this.#highlight === null) return;
        if (found === null) {
            this.#highlight.visible = false;
            for (const root of this.#roots) root.setHint(null);
            return;
        }

        // Drawn around the frame, so it traces the window's visible edge rather than
        // floating out in its shadow. The hint reports the *buffer* size, because that is
        // what the file will be.
        //
        // `spec/09` §3's "window highlight move": from one window to the next it travels,
        // and the first one it simply appears on. Only a change of window gets here, so
        // this runs once per window and not once per motion event.
        const frame = found.frame;
        if (this.#highlight.visible) {
            this.#highlight.ease(
                easing({
                    x: frame.x,
                    y: frame.y,
                    width: frame.width,
                    height: frame.height,
                    duration: HIGHLIGHT_MOVE.duration,
                    mode: HIGHLIGHT_MOVE.mode,
                }),
            );
        } else {
            this.#highlight.remove_all_transitions();
            this.#highlight.set_position(frame.x, frame.y);
            this.#highlight.set_size(frame.width, frame.height);
            this.#highlight.visible = true;
        }

        // On the window's monitor, and off every other: a Tab to a window on the next
        // monitor, or the pointer crossing to one, left the last window's name behind.
        const monitorIndex = found.window.get_monitor();
        const scale = global.display.get_monitor_scale(monitorIndex);
        const title = found.window.get_title() ?? '';
        const hint =
            `${title || found.window.get_wm_class() || 'Window'} · ` +
            `${Math.ceil(found.buffer.width * scale)} × ` +
            `${Math.ceil(found.buffer.height * scale)}`;
        this.#roots.forEach((root, index) => root.setHint(index === monitorIndex ? hint : null));
    }

    /**
     * `spec/03` §3, and the reason this is a separate step from `destroy`: the overlay's
     * own chrome must be off screen and painted before the grab, or the highlight lands
     * in the PNG. Window capture also needs the target **focused**, because
     * `Shell.Screenshot.screenshot_window` captures whatever has focus.
     */
    focusForCapture(window: Meta.Window): void {
        for (const root of this.#roots) root.setVisible(false);
        window.activate(global.get_current_time());
    }

    #settle(value: WindowSelection | null): void {
        if (this.#settled) return;
        this.#settled = true;
        this.#cancelled = value === null;
        const resolve = this.#resolve;
        this.#resolve = null;
        resolve?.(value);
    }

    destroy(): void {
        if (open.delete(this)) tellPets({ kind: 'overlay', open: false });
        if (this.#grab !== null) {
            try {
                Main.popModal(this.#grab);
            } catch (e) {
                error('could not pop the window picker modal grab', e);
            }
            this.#grab = null;
        }
        const roots = this.#roots;
        this.#roots = [];
        const host = this.#host;
        this.#host = null;
        const drop = () => {
            for (const root of roots) {
                try {
                    root.destroy();
                } catch (e) {
                    error('could not destroy a window picker root', e);
                }
            }
            try {
                host?.destroy();
            } catch (e) {
                error('could not destroy the window picker host', e);
            }
        };
        if (this.#cancelled && host !== null) {
            try {
                fadeOutAndDrop(host, OVERLAY_CANCEL, drop);
            } catch (e) {
                error('could not fade the window picker out; dropping it now', e);
                drop();
            }
        } else {
            drop();
        }
        this.#highlight = null;
        this.#candidates = [];
        this.#hovered = null;
        this.#underPointer = null;
        this.#settle(null);
    }
}

/**
 * Every window a user could sensibly ask to capture, topmost first.
 *
 * `get_window_actors()` is already in stacking order, bottom to top, so reversing gives
 * "what the user sees on top" and the first hit in `#hover` is the right one.
 */
export function capturableWindows(): Candidate[] {
    const workspace = global.workspace_manager.get_active_workspace();
    const out: Candidate[] = [];

    for (const actor of global.get_window_actors()) {
        const window = actor.meta_window;
        if (window === null) continue;
        if (EXCLUDED_TYPES.has(window.get_window_type())) continue;
        if (window.is_override_redirect()) continue;
        // OctoSnap's own cards, pins and history strip. They are capture chrome as the
        // overlay is, and they give the keyboard back as soon as they get it (`cards.ts`),
        // so the focused window `screenshot_window` takes would have been another one
        // (D138).
        if (isFloating(window)) continue;
        // Minimised windows have no current contents to capture, and a hidden window
        // cannot be pointed at anyway.
        if (window.minimized) continue;
        if (!window.located_on_workspace(workspace)) continue;

        const frame = window.get_frame_rect();
        if (frame.width <= 0 || frame.height <= 0) continue;
        const buffer = window.get_buffer_rect();

        out.push({
            window,
            frame: { x: frame.x, y: frame.y, width: frame.width, height: frame.height },
            buffer: {
                x: buffer.x,
                y: buffer.y,
                width: buffer.width,
                height: buffer.height,
            },
        });
    }

    return out.reverse();
}

/** `spec/10` §3.3's source-window fields, for the capture twin. */
export function describeWindow(window: Meta.Window): {
    app_id: string;
    app_name: string;
    window_title: string;
} {
    const tracker = Shell.WindowTracker.get_default();
    const app = tracker.get_window_app(window);
    return {
        // The Shell app's id is the desktop file name, which is what spec/08's {app}
        // token and the app-side filename renderer both want.
        app_id: app?.get_id() ?? window.get_gtk_application_id() ?? '',
        app_name: app?.get_name() ?? window.get_wm_class() ?? '',
        window_title: window.get_title() ?? '',
    };
}
