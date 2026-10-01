// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The area-selection overlay: `spec/03` §5.1 and §6, classic (non-All-In-One) mode.
 *
 * Classic mode confirms on mouse release (`spec/03` §1), so there is no Adjusting state
 * and no handles yet -- those belong to All-In-One, which needs the toolbar. What is here
 * is the whole drag vocabulary: Space to move, Shift to lock the ratio, Alt to draw from
 * the centre, Esc to cancel.
 *
 * None of the geometry is computed here. It all goes through `selection.ts`, which is
 * unit-tested against the Rust implementation, so this file is only plumbing: grab input,
 * turn events into calls, move actors. That division is deliberate -- the arithmetic is
 * where the bugs live and the plumbing is where the tests cannot reach.
 */

import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import St from 'gi://St';
import Shell from 'gi://Shell';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import { type FrozenScreen, freezeScreen, viewRates } from '../capture.js';
import { waitForRedraw } from '../later.js';
import { Crosshair } from './magnifier.js';
import { createOverlayHost } from './host.js';
import { OverlayRoot, type MonitorGeometry } from './root.js';
import { OVERLAY_CANCEL, OVERLAY_OPEN, type Motion, fadeIn, fadeOutAndDrop, settle } from './motion.js';
import {
    type Direction,
    type Handle,
    type Modifiers,
    type Rect,
    STEP,
    STEP_FAST,
    clampToMonitor,
    cycle,
    dominantMonitor,
    dragRatio,
    hitTest,
    isClick,
    middleHalf,
    nudge,
    rectFromDrag,
    resizeByHandle,
    resizeByKey,
    setHeight,
    setWidth,
} from './selection.js';
import { TOOLBAR_MODES, Toolbar, type ToolbarMode } from './toolbar.js';
import { readGifDefaults } from '../appDefaults.js';
import type { GifOverrides } from '../recordChoices.js';
import type { CrosshairMode } from '../settings.js';
import { error, info } from '../log.js';
import { tellPets } from '../pets/events.js';

/**
 * `spec/10` §7's first row, "hotkey → overlay visible < 100 ms", said at the end of the
 * first stage paint the overlay is in: as near the glass as the shell can see, since the
 * frame reaches the screen at most one refresh later. The snapshot the loupe, the freeze
 * and the captured cursor need is taken before anything is drawn, so its share is named.
 */
function reportVisible(requestedAt: number, snapshotUs: number | null): void {
    const handler = global.stage.connect('after-paint', () => {
        global.stage.disconnect(handler);
        const ms = (GLib.get_monotonic_time() - requestedAt) / 1000;
        const snapshot = snapshotUs === null ? '' : `, ${(snapshotUs / 1000).toFixed(1)} ms of it the snapshot`;
        info(`overlay visible ${ms.toFixed(1)} ms after the request${snapshot}; spec/10 §7 budgets 100 ms`);
    });
}

/** The overlays that hold a grab, from taking it to `destroy` (D135). */
const open = new Set<AreaOverlay>();

/**
 * For `disable()`: every open overlay goes at once, grab and actors, and whoever awaits it
 * hears a cancel. The lock screen disables an extension that runs only in the user
 * session, so an overlay open when the screen locked was still there at the unlock, over
 * an extension that had been started again under it.
 */
export function closeOverlays(): void {
    for (const overlay of [...open]) overlay.destroy();
}

/**
 * `spec/03` §4's hint, on every selection. The spec has it go after the first successful
 * capture, as a quick tip would; this comment said it did, and it never has (D136).
 */
const HINT = 'Drag to select · Space to move · Shift to lock · Alt from centre · Esc to cancel';

/** `spec/03` §5.1's All-In-One hint, which is a different vocabulary. */
const HINT_ADJUSTING =
    'Drag or resize · Arrows nudge, Shift ×10 · Ctrl+arrows resize · Tab changes mode · ' +
    'Enter to capture · Esc to clear';

/** `spec/06` §3's record hint: like Adjusting, but Enter *starts* and an empty Enter takes
 * the whole screen (`REC-01`: area and fullscreen). Esc clears first, as in Adjusting: the
 * selection it opens with is the last area, restored. */
const HINT_RECORD =
    'Drag or resize · Enter to record · Enter with nothing selected records the screen · Esc to clear';

export interface OverlayOptions {
    /** `CAP-09`: snapshot the stage and select over the snapshot. */
    freeze?: boolean;
    /** `CAP-10`: `spec/08` §6's `shot-crosshair`. */
    crosshair?: CrosshairMode;
    magnifier?: boolean;
    /**
     * `CAP-01`. With the toolbar, a release enters `spec/03` §6's **Adjusting** state
     * instead of confirming, so the selection can be tuned before the shot.
     */
    allInOne?: boolean;
    /**
     * `spec/06` §3's record mode: the Adjusting behaviour (release tunes, Enter confirms,
     * handles) but themed red, without the mode-switching toolbar, and with an empty Enter
     * meaning the whole monitor (`REC-01`'s fullscreen). A recording is not a screenshot,
     * so this never freezes and takes no crosshair -- the region is chosen over the live
     * screen it is about to record.
     */
    record?: boolean;
    /** The mode the toolbar opens on, which `spec/03` §1 says is the last one used. */
    mode?: ToolbarMode;
    /** `CAP-01`: the remembered selection, restored on open (`spec/03` §1). */
    initialRect?: Rect | null;
    /**
     * A hint of the caller's own, for a selection that is for something other than a
     * screenshot: a scrolling capture's says what the area is for.
     */
    hint?: string;
    /**
     * `CAP-11`: the capture is to include the mouse pointer.
     *
     * The overlay needs to know because it is the only thing that can help. By capture
     * time the pointer has *become* the picking crosshair (`spec/03` §2), so the arrow the
     * user was pointing with exists only in a snapshot taken before the overlay opened.
     * See `snapshot`. `docs/decisions.md` D25.
     */
    cursor?: boolean;
    /** The app's GIF defaults from its copy in this schema, for the recording row (D125). */
    gifDefaults?: GifOverrides;
    /**
     * `GLib.get_monotonic_time()` when the capture was asked for -- the shortcut's handler
     * or the D-Bus call -- so the first frame the overlay is painted in can be timed
     * against `spec/10` §7's first row.
     */
    requestedAt?: number;
}

/** What the overlay decided to do, for the modes All-In-One can switch to. */
export interface AreaSelectionResult extends AreaSelection {
    /** The mode selected in the toolbar when the capture was confirmed. */
    mode: ToolbarMode;
}

/**
 * The modifier that summons the crosshair when `shot-crosshair` is `modifier`.
 *
 * Ctrl, not Shift or Alt: both of those already mean something during a drag
 * (`spec/03` §5.1's ratio lock and draw-from-centre), and a modifier that did two things
 * at once would make precise placement impossible.
 */
const CROSSHAIR_MODIFIER = Clutter.ModifierType.CONTROL_MASK;

export interface AreaSelection {
    /** Logical stage rect, clamped to one monitor. */
    rect: Rect;
    monitor: MonitorGeometry;
    /** Modifier mask at confirm, for `CAP-15`. */
    modifiers: number;
}

/**
 * `spec/03` §6's states, minus the ones that belong to other controllers.
 *
 * `adjusting` is All-In-One's: after a release the selection stays with handles and a
 * toolbar, and Enter is what confirms. `dragging-inside` and `resizing` are its two ways
 * of changing an existing selection, which `spec/03` §5.1 separates from starting a new
 * one by hit test rather than by modifier.
 */
type Phase =
    | 'idle'
    | 'sizing'
    | 'moving'
    | 'adjusting'
    | 'dragging-inside'
    | 'resizing';

export class AreaOverlay {
    #roots: OverlayRoot[] = [];

    /** See `host.ts`: `screenshotUIGroup` cannot lay a positioned actor out. */
    #host: St.Widget | null = null;
    #grab: Clutter.Grab | null = null;
    #resolve: ((value: AreaSelection | null) => void) | null = null;
    #settled = false;
    /**
     * How the selection ended: nothing yet, a cancel, or a selection. A cancelled overlay
     * leaves over `spec/09` §3's 100 ms and anything else goes at once, since a capture
     * has already hidden the chrome and a mode switch hands the screen to something else.
     */
    #outcome: 'pending' | 'cancelled' | 'selected' = 'pending';

    #phase: Phase = 'idle';
    #anchor: readonly [number, number] = [0, 0];
    #pointer: readonly [number, number] = [0, 0];
    /** Offset from the pointer to the selection origin while Space-moving. */
    #moveOffset: readonly [number, number] = [0, 0];
    #rect: Rect | null = null;
    #lockedRatio: number | null = null;

    /**
     * A snapshot of the stage, taken for `CAP-09`'s freeze **or** `CAP-10`'s loupe.
     *
     * Two callers, one texture -- but only one of them may govern the capture, which is
     * what `#freezeRequested` below is for. See its comment: conflating the two produced
     * a capture of stale pixels under the default settings.
     */
    #snapshot: FrozenScreen | null = null;
    #crosshairs: Crosshair[] = [];
    #crosshairMode: CrosshairMode = 'modifier';
    /** The modifier mask at the moment the selection was confirmed (`CAP-15`). */
    #confirmModifiers = 0;

    /** `CAP-01` state. */
    #allInOne = false;
    /** `spec/06` §3: record mode -- Adjusting behaviour, red accent, no toolbar. */
    #record = false;
    #mode: ToolbarMode = 'area';
    #toolbar: Toolbar | null = null;
    /** Which handle is being dragged, in `resizing`. */
    #activeHandle: Handle | null = null;
    /** True when the toolbar's lock or ratio holds the ratio across drags. */
    #toolbarRatioLocked = false;
    /** Where the pointer sat inside the selection when a move began. */
    #dragOffset: readonly [number, number] = [0, 0];
    /** The selection when a press inside it began, so a click can confirm it unmoved. */
    #rectAtPress: Rect | null = null;
    /**
     * What Fullscreen replaced, and the monitor it put there. Fullscreen's selection is
     * the whole monitor, and Tab on its way to Window or Scrolling passes through it, so
     * the area the user had arrived there as the whole screen. Another mode chosen with
     * the monitor untouched gives the area back, after a click as after a Tab (D137).
     */
    #beforeFullscreen: { replaced: Rect | null; monitor: Rect } | null = null;

    /** The mode the toolbar was on when the capture was confirmed. */
    get mode(): ToolbarMode {
        return this.#mode;
    }

    /** What the recording row was changed to, if anything (D101). Empty unless Record. */
    #gifOverrides: GifOverrides = {};

    get recordOverrides(): GifOverrides {
        return this.#mode === 'record' ? this.#gifOverrides : {};
    }

    /**
     * True when the *user* asked to freeze, as opposed to the overlay merely holding a
     * snapshot for the magnifier.
     *
     * This distinction is not pedantry. The loupe needs a texture whenever the crosshair
     * is on, which is the default, and an earlier version exposed that texture as
     * `frozen` -- so the capture path composited from it and **every area capture
     * silently returned the screen as it was when the overlay opened**. Caught in a
     * nested shell by a log line reading "(frozen)" for a capture nobody had asked to
     * freeze. The snapshot and the intent are now separate properties.
     */
    #freezeRequested = false;

    /**
     * `CAP-09`: the frozen stage to capture from, or `null` when the capture should read
     * the live screen. Deliberately `null` for a snapshot taken only for the loupe.
     */
    get frozen(): FrozenScreen | null {
        return this.#freezeRequested ? this.#snapshot : null;
    }

    /**
     * The pre-overlay snapshot itself, whatever it was taken for.
     *
     * Deliberately separate from `frozen`, which answers "should the capture come from a
     * frozen frame?" and is `null` unless the user asked to freeze. This answers "is there
     * a picture of the screen from before the overlay existed?" -- which is a different
     * question with a different caller: `CAP-11` needs that picture's **cursor**, because
     * by capture time the real pointer has been replaced by the picking crosshair.
     *
     * The distinction is the whole bug this fixed. `flow.ts` used to take its own snapshot
     * at capture time for the cursor, and composited the crosshair. D25.
     */
    get snapshot(): FrozenScreen | null {
        return this.#snapshot;
    }

    /** `CAP-15`: Ctrl and Shift as they were when the drag was released. */
    get confirmModifiers(): number {
        return this.#confirmModifiers;
    }

    /**
     * Maps every monitor, because `spec/03` §2 requires the overlay to cover **every**
     * monitor so nothing underneath stays clickable.
     */
    /**
     * Builds the overlay and resolves when the user confirms or cancels.
     *
     * Async because `CAP-09`'s freeze has to grab the stage **before** any overlay actor
     * exists -- a snapshot taken afterwards would contain the dim. That ordering is the
     * whole reason this is not a plain constructor.
     */
    async open(options: OverlayOptions = {}): Promise<AreaSelection | null> {
        this.#crosshairMode = options.crosshair ?? 'modifier';
        // Record dress comes from the dedicated record flow (`record.ts`) or from
        // All-In-One reopening on the mode it last used (`spec/03` §1).
        this.#record = options.record === true || options.mode === 'record';
        // Record borrows All-In-One's Adjusting behaviour (release tunes, Enter confirms,
        // handles) -- so everything keyed on `#allInOne` applies -- but its toolbar is
        // built separately below, gated on the real All-In-One flag.
        this.#allInOne = options.allInOne === true || this.#record;
        this.#mode = options.mode ?? 'area';

        this.#freezeRequested = options.freeze === true;
        const wantsLoupe = options.magnifier !== false && this.#crosshairMode !== 'off';
        const wantsCursor = options.cursor === true;

        // One snapshot serves all three, and it must be taken before any overlay actor
        // exists or the dim ends up inside it. That ordering is why `open` is async.
        //
        // `wantsCursor` is in this condition for a reason worth stating: **during a
        // selection there is no mouse pointer to capture.** `spec/03` §2 turns the cursor
        // into a crosshair for picking a pixel, so a snapshot taken at capture time holds
        // the *picker* sprite, not the arrow the user was pointing with. The only cursor
        // worth compositing is the one from before the overlay opened, and this is the
        // only moment it exists. `docs/decisions.md` D25.
        let snapshotUs: number | null = null;
        if (this.#freezeRequested || wantsLoupe || wantsCursor) {
            const snapshotStarted = GLib.get_monotonic_time();
            try {
                // With a layer at each other scale when a capture will be cut from it,
                // which the loupe alone does not need (D138).
                this.#snapshot = await freezeScreen({
                    layers: this.#freezeRequested || wantsCursor,
                });
            } catch (e) {
                // Degrade rather than fail: a capture over the live screen with no loupe
                // is much better than no capture.
                error(
                    this.#freezeRequested
                        ? 'could not freeze the screen; selecting over the live screen'
                        : 'could not snapshot the stage; the magnifier and the captured ' +
                          'cursor are unavailable',
                    e,
                );
                this.#freezeRequested = false;
            }
            snapshotUs = GLib.get_monotonic_time() - snapshotStarted;
        }

        return new Promise(resolve => {
            this.#resolve = resolve;

            const monitors: MonitorGeometry[] = Main.layoutManager.monitors.map((m, index) => ({
                index,
                x: m.x,
                y: m.y,
                width: m.width,
                height: m.height,
                scale: global.display.get_monitor_scale(index),
            }));

            const stage = {
                width: global.stage.width,
                height: global.stage.height,
            };

            this.#host = createOverlayHost('octosnap-overlay-host', { interactive: true });
            this.#host.connect('event', (_a, event) => this.#onEvent(event));
            // An open list on the toolbar -- ratio, frame rate, width, quality -- closes on
            // a press anywhere but on itself, and that press does nothing else, as outside
            // any menu (D150). Heard on the way down, so a press on another of the
            // toolbar's buttons closes the list rather than reaching the button.
            this.#host.connect('captured-event', (_a, event: Clutter.Event) => {
                const type = event.type();
                if (type !== Clutter.EventType.BUTTON_PRESS && type !== Clutter.EventType.TOUCH_BEGIN)
                    return Clutter.EVENT_PROPAGATE;
                const [x, y] = event.get_coords();
                return this.#toolbar?.dismissPopupsOutside(x, y) === true ? Clutter.EVENT_STOP : Clutter.EVENT_PROPAGATE;
            });

            for (const monitor of monitors) {
                const root = new OverlayRoot(monitor, this.#record ? 'record' : 'default');

                // `CAP-09`: the frozen image goes behind the dim, so the dim darkens the
                // snapshot rather than letting the live screen move underneath it.
                if (this.#freezeRequested && this.#snapshot !== null)
                    root.setBackdrop(this.#snapshot.content, stage.width, stage.height);

                // No per-root handler: `event` bubbles, and the host below catches
                // everything from every monitor in one place. One connection instead of
                // one per monitor also removes the question of which root a key event
                // belongs to, which has no good answer.
                this.#host.add_child(root.actor);
                this.#roots.push(root);

                if (this.#crosshairMode !== 'off') {
                    this.#crosshairs.push(
                        new Crosshair(root.actor, monitor, this.#snapshot, stage, wantsLoupe),
                    );
                }
            }

            // `spec/09` §3's open, on the host and so on every monitor at once. Input is
            // taken from the first frame: the fade is how the overlay arrives, not a wait
            // before it can be used. A frozen frame fades in over the very pixels it
            // froze, so it arrives without a seam.
            fadeIn(this.#host, OVERLAY_OPEN);

            // The hint and the toolbar go on the monitor the pointer is on
            // (`spec/03` §2: "the toolbar exists only on the current monitor").
            const current = this.#pointerMonitorIndex(monitors);
            this.#roots[current]?.setHint(
                options.hint ?? (this.#record ? HINT_RECORD : this.#allInOne ? HINT_ADJUSTING : HINT),
            );

            const primary = this.#roots[current] ?? this.#roots[0];
            if (!primary) {
                this.#settle(null);
                return;
            }

            // The **host**, not one monitor's root. A `Clutter.Grab` confines input to
            // the grabbed actor's subtree, so grabbing a root meant only that monitor's
            // actors could be clicked -- which is why the All-In-One toolbar could not be
            // used on a second screen. `docs/decisions.md` D28.
            this.#grab = Main.pushModal(this.#host, {
                actionMode: Shell.ActionMode.SYSTEM_MODAL,
            });
            open.add(this);
            // The pets step aside while a selection is made, so what the user sees is what
            // the capture will have: they are never in it (`pets/crew.ts`).
            tellPets({ kind: 'overlay', open: true });
            this.#host.grab_key_focus();
            this.#setCursor(Clutter.CursorType.CROSSHAIR);

            // Built *after* the grab and after the root has key focus, and with its
            // fields unfocusable until both are true. An `St.Entry` created before this
            // point acquired focus on its own whenever an application window had it --
            // which is every capture after the first -- and the engine then treated every
            // keystroke as text, killing arrows and Enter alike.
            // The mode-switching toolbar belongs to All-In-One only. Record reuses the
            // Adjusting behaviour but not the seven-mode switcher -- it is one mode.
            if (options.allInOne === true) {
                // Said once, so which source the row shows is on record (D125): a
                // sandboxed app's copy, the app's own schema, or `spec/08` §5.
                const gifDefaults = readGifDefaults(options.gifDefaults);
                info(`recording row opens on ${JSON.stringify(gifDefaults)}`);
                this.#toolbar = new Toolbar(
                    this.#host,
                    primary.monitor,
                    primary.monitor.scale,
                    this.#mode,
                    gifDefaults,
                    viewRates(),
                    {
                        modeChanged: mode => this.#onModeChanged(mode),
                        sizeTyped: (w, h) => this.#onSizeTyped(w, h),
                        lockChanged: locked => this.#onLockChanged(locked),
                        ratioChosen: ratio => this.#onRatioChosen(ratio),
                        recordOptionsChanged: overrides => {
                            this.#gifOverrides = overrides;
                            info(`recording row -> ${JSON.stringify(overrides)}`);
                        },
                    },
                );
                this.#toolbar.setFieldsFocusable(true);
            }

            // `spec/03` §1: All-In-One opens with the last selection restored, handles
            // and all, so the common case of "the same region again" is one keypress.
            if (this.#allInOne && options.initialRect != null) {
                this.#rect = options.initialRect;
                this.#applyClamp();
                this.#phase = 'adjusting';
                this.#draw();
            }

            // `always` shows the crosshair from the start; `modifier` waits for Ctrl.
            this.#setCrosshairVisible(this.#crosshairMode === 'always');
            if (this.#crosshairMode === 'always') {
                const [x, y] = global.get_pointer();
                this.#updateCrosshair(x, y);
            }

            info(
                `overlay open on ${this.#roots.length} monitor(s)` +
                (this.#freezeRequested ? ' frozen' : '') +
                (this.#snapshot !== null && !this.#freezeRequested ? ' (loupe snapshot)' : '') +
                ` crosshair=${this.#crosshairMode}`,
            );
            if (options.requestedAt !== undefined) reportVisible(options.requestedAt, snapshotUs);
        });
    }

    // --- event handling ------------------------------------------------------

    #onEvent(event: Clutter.Event): boolean {
        switch (event.type()) {
            case Clutter.EventType.BUTTON_PRESS:
                return this.#onButtonPress(event);
            case Clutter.EventType.MOTION:
                return this.#onMotion(event);
            case Clutter.EventType.BUTTON_RELEASE:
                return this.#onButtonRelease(event);
            case Clutter.EventType.KEY_PRESS:
                return this.#onKeyPress(event);
            case Clutter.EventType.KEY_RELEASE:
                return this.#onKeyRelease(event);
            default:
                return Clutter.EVENT_PROPAGATE;
        }
    }

    #onButtonPress(event: Clutter.Event): boolean {
        const [rawX, rawY] = event.get_coords();

        // The toolbar handles its own clicks; the overlay must not treat a press on a
        // mode button as the start of a new selection.
        if (this.#toolbar?.containsStagePoint(rawX, rawY) === true)
            return Clutter.EVENT_PROPAGATE;

        // `spec/03` §5.1: right-click cancels in the classic modes, and in All-In-One it
        // clears the selection rather than closing -- the same as the first Esc, because
        // the same "undo my selection" intent is behind both.
        if (event.get_button() !== Clutter.BUTTON_PRIMARY) {
            if (this.#allInOne && this.#rect !== null) {
                this.#clearSelection();
                return Clutter.EVENT_STOP;
            }
            this.#settle(null);
            return Clutter.EVENT_STOP;
        }

        const point: readonly [number, number] = [Math.round(rawX), Math.round(rawY)];

        // `spec/03` §5.1: "inside selection -> move, on a handle -> resize, anywhere else
        // -> begin a new selection". A hit test, not a modifier, which is what makes an
        // existing selection feel like an object rather than a mode.
        if (this.#allInOne && this.#phase === 'adjusting') {
            const hit = hitTest(this.#rect, point);
            if (hit.kind === 'handle') {
                this.#activeHandle = hit.handle;
                this.#phase = 'resizing';
                this.#pointer = point;
                return Clutter.EVENT_STOP;
            }
            if (hit.kind === 'inside' && this.#rect !== null) {
                this.#dragOffset = [this.#rect.x - point[0], this.#rect.y - point[1]];
                // Where the press was, and the selection it found, so the release can
                // tell a click, which confirms, from a move.
                this.#anchor = point;
                this.#rectAtPress = this.#rect;
                this.#phase = 'dragging-inside';
                this.#pointer = point;
                this.#setCursor(Clutter.CursorType.MOVE);
                return Clutter.EVENT_STOP;
            }
        }

        this.#anchor = point;
        this.#pointer = point;
        this.#phase = 'sizing';
        // A fresh drag starts a fresh ratio lock, captured when Shift is first pressed --
        // unless the toolbar has one set, which outlives individual drags.
        if (!this.#toolbarRatioLocked) {
            // Shift *already* held at the press is the case `spec/03` §5.1's "the ratio at
            // the moment Shift was pressed" leaves undefined: at that moment there is no
            // selection and so no ratio. Left to the general rule it locked to whatever
            // the first motion event happened to produce, which made the same gesture give
            // a different answer at a different pointer speed -- measured, a 400x300 drag
            // came out 400x304. Square is the conventional reading and the only
            // deterministic one. `docs/decisions.md` D21.
            this.#lockedRatio =
                (event.get_state() & Clutter.ModifierType.SHIFT_MASK) !== 0 ? 1 : null;
        }
        this.#update();
        return Clutter.EVENT_STOP;
    }

    #onMotion(event: Clutter.Event): boolean {
        const [rawX, rawY] = event.get_coords();

        // The crosshair tracks the pointer whether or not a drag is in progress: its job
        // is to help *start* one on the right pixel. `spec/03` §2 hides it over the
        // toolbar, where the pointer is choosing a control rather than a pixel (D136).
        if (this.#crosshairMode !== 'off') {
            const wanted =
                this.#crosshairMode === 'always' || (event.get_state() & CROSSHAIR_MODIFIER) !== 0;
            const overToolbar = this.#toolbar?.containsStagePoint(rawX, rawY) === true;
            this.#setCrosshairVisible(wanted && !overToolbar);
            this.#updateCrosshair(Math.round(rawX), Math.round(rawY));
        }

        if (this.#phase === 'idle' || this.#phase === 'adjusting') {
            // In Adjusting the pointer only changes the cursor, so the user can see what
            // a press would do before making it.
            if (this.#phase === 'adjusting') this.#updateAdjustCursor(rawX, rawY);
            return Clutter.EVENT_STOP;
        }

        const [x, y] = [rawX, rawY];
        this.#pointer = [Math.round(x), Math.round(y)];

        if (this.#phase === 'resizing' && this.#rect !== null && this.#activeHandle !== null) {
            this.#rect = resizeByHandle(
                this.#rect,
                this.#activeHandle,
                this.#pointer,
                this.#modifiersFrom(event),
                this.#lockedRatio,
            );
            this.#applyClamp();
            this.#draw();
            return Clutter.EVENT_STOP;
        }

        if (this.#phase === 'dragging-inside' && this.#rect !== null) {
            this.#rect = {
                x: this.#pointer[0] + this.#dragOffset[0],
                y: this.#pointer[1] + this.#dragOffset[1],
                width: this.#rect.width,
                height: this.#rect.height,
            };
            this.#applyClamp();
            this.#draw();
            return Clutter.EVENT_STOP;
        }

        if (this.#phase === 'moving' && this.#rect !== null) {
            // Space-move: the size is frozen and the whole selection follows the pointer.
            this.#rect = {
                x: this.#pointer[0] + this.#moveOffset[0],
                y: this.#pointer[1] + this.#moveOffset[1],
                width: this.#rect.width,
                height: this.#rect.height,
            };
            this.#applyClamp();
            this.#draw();
            return Clutter.EVENT_STOP;
        }

        this.#update(this.#modifiersFrom(event));
        return Clutter.EVENT_STOP;
    }

    #onButtonRelease(event: Clutter.Event): boolean {
        if (this.#phase === 'idle' || this.#phase === 'adjusting')
            return Clutter.EVENT_PROPAGATE;
        if (event.get_button() !== Clutter.BUTTON_PRIMARY) return Clutter.EVENT_STOP;

        const [x, y] = event.get_coords();
        const release: readonly [number, number] = [Math.round(x), Math.round(y)];

        // `spec/03` §6: "Adjusting --> Capturing: Enter / primary button". A click on the
        // selection is the pointer's confirm, since the toolbar has no Capture button, and
        // the selection goes back to where the press found it, so the pixel or two a hand
        // moves in clicking is not in the shot. It used to be a move of nothing, back to
        // Adjusting, while the comments here said it confirmed (D136). Not in record
        // mode, where a stray click would start a recording: Enter does that.
        if (
            this.#phase === 'dragging-inside' &&
            !this.#record &&
            this.#rectAtPress !== null &&
            isClick(this.#anchor, release)
        ) {
            this.#rect = this.#rectAtPress;
            this.#rectAtPress = null;
            this.#phase = 'adjusting';
            this.#confirm(event.get_state());
            return Clutter.EVENT_STOP;
        }

        // Finishing a move or a resize returns to Adjusting; it never confirms.
        if (this.#phase === 'dragging-inside' || this.#phase === 'resizing') {
            this.#rectAtPress = null;
            this.#activeHandle = null;
            this.#enterAdjusting();
            return Clutter.EVENT_STOP;
        }

        // A click without a drag. `spec/03` §5.1 has it capture the window under the
        // pointer, as window mode would; here it selects nothing and cancels, which beats
        // capturing a 1 px region the user did not mean, and a window is one key away
        // (D137). In All-In-One a click on an existing selection is the confirm gesture
        // instead, which is handled above: the hit test put the press in `dragging-inside`.
        if (this.#phase === 'sizing' && isClick(this.#anchor, release)) {
            if (this.#allInOne) {
                // A stray click on the dim clears any selection rather than cancelling
                // the whole overlay: All-In-One is a place the user stays in.
                this.#clearSelection();
                return Clutter.EVENT_STOP;
            }
            this.#settle(null);
            return Clutter.EVENT_STOP;
        }

        if (this.#rect === null || this.#rect.width <= 0 || this.#rect.height <= 0) {
            if (this.#allInOne) {
                this.#clearSelection();
                return Clutter.EVENT_STOP;
            }
            this.#settle(null);
            return Clutter.EVENT_STOP;
        }

        // `spec/03` §6: Dragging -> Adjusting on release in All-In-One, rather than
        // Dragging -> Capturing. This is the whole difference between the two families
        // of mode.
        if (this.#allInOne) {
            this.#enterAdjusting();
            return Clutter.EVENT_STOP;
        }

        this.#confirm(event.get_state());
        return Clutter.EVENT_STOP;
    }

    /**
     * Resolves the selection against a monitor and settles the promise.
     *
     * The monitor is the **dominant** one rather than the pointer's: a drag can end on a
     * different output from where it started, and the scale has to be the one the pixels
     * will be captured at (`spec/01` §1).
     */
    #confirm(modifiers: number): void {
        if (this.#rect === null) {
            this.#settle(null);
            return;
        }

        const monitorIndex = dominantMonitor(
            this.#rect,
            this.#roots.map(r => ({
                x: r.monitor.x,
                y: r.monitor.y,
                width: r.monitor.width,
                height: r.monitor.height,
            })),
        );
        const root = monitorIndex === null ? this.#roots[0] : this.#roots[monitorIndex];
        if (root === undefined) {
            this.#settle(null);
            return;
        }

        this.#confirmModifiers = modifiers;
        this.#settle({
            rect: this.#rect,
            monitor: root.monitor,
            modifiers: this.#confirmModifiers,
        });
    }

    // --- All-In-One (`CAP-01`) -----------------------------------------------

    /** `spec/03` §6: Dragging -> Adjusting. The selection gains handles and a toolbar. */
    #enterAdjusting(): void {
        this.#phase = 'adjusting';
        this.#setCursor(Clutter.CursorType.CROSSHAIR);
        this.#draw();
    }

    /**
     * A selection for the keyboard to work on when there is none: the middle half of the
     * pointer's monitor, with handles, as if it had just been drawn (D137).
     */
    #startFromKeyboard(): void {
        const root = this.#roots[this.#pointerMonitorIndex()] ?? this.#roots[0];
        if (root === undefined) return;
        this.#rect = middleHalf(root.monitor);
        this.#applyClamp();
        this.#enterAdjusting();
    }

    /** `spec/03` §5.1: Esc, or a right-click, clears without closing. */
    #clearSelection(): void {
        this.#rect = null;
        this.#activeHandle = null;
        this.#phase = 'idle';
        this.#lockedRatio = this.#toolbarRatioLocked ? this.#lockedRatio : null;
        this.#setCursor(Clutter.CursorType.CROSSHAIR);
        this.#draw();
    }

    /**
     * `spec/03` §2: "over handles it changes to the matching resize cursor".
     *
     * Set on the root actor from the hit test rather than declaratively per handle, which
     * `root.ts` also does: the handle actors carry their own cursors, but the *interior*
     * of a selection has to read as movable and only the engine knows the rect.
     */
    #updateAdjustCursor(x: number, y: number): void {
        if (this.#toolbar?.containsStagePoint(x, y) === true) return;
        const hit = hitTest(this.#rect, [Math.round(x), Math.round(y)]);
        this.#setCursor(
            hit.kind === 'inside' ? Clutter.CursorType.MOVE : Clutter.CursorType.CROSSHAIR,
        );
    }

    #onModeChanged(mode: ToolbarMode): void {
        const leaving = this.#mode;
        this.#mode = mode;
        this.#toolbar?.setActiveMode(mode);
        info(`toolbar mode -> ${mode}`);

        // `spec/06` §3 from the toolbar: Record re-dresses the open overlay -- red frame
        // and handles, its own hint, Enter records -- and any other mode dresses it back.
        // The selection itself is kept: the rect the user drew is what will be recorded.
        this.#setRecordDress(mode === 'record');

        // `spec/03` §5.3: in All-In-One, fullscreen highlights a whole monitor. Selecting
        // that mode therefore replaces the selection with the pointer's monitor, which is
        // both the highlight and what Enter would capture.
        if (mode === 'fullscreen') {
            const root = this.#roots[this.#pointerMonitorIndex()] ?? this.#roots[0];
            if (root !== undefined) {
                const monitor = {
                    x: root.monitor.x,
                    y: root.monitor.y,
                    width: root.monitor.width,
                    height: root.monitor.height,
                };
                if (leaving !== 'fullscreen') {
                    this.#beforeFullscreen = { replaced: this.#rect, monitor };
                }
                this.#rect = { ...monitor };
                this.#enterAdjusting();
            }
        } else if (leaving === 'fullscreen') {
            this.#giveBackFullscreen();
        }
    }

    /** See `#beforeFullscreen`. A monitor the user has since resized or moved stays. */
    #giveBackFullscreen(): void {
        const kept = this.#beforeFullscreen;
        this.#beforeFullscreen = null;
        if (kept === null || kept.replaced === null || this.#rect === null) return;
        const [now, monitor] = [this.#rect, kept.monitor];
        const untouched =
            now.x === monitor.x &&
            now.y === monitor.y &&
            now.width === monitor.width &&
            now.height === monitor.height;
        if (!untouched) return;
        this.#rect = kept.replaced;
        this.#applyClamp();
        this.#enterAdjusting();
    }

    /** Turns record dress on or off for an overlay that is already open. */
    #setRecordDress(on: boolean): void {
        if (this.#record === on) return;
        this.#record = on;
        for (const root of this.#roots) root.setAccent(on ? 'record' : 'default');
        // One hint, on the monitor the pointer is on (`spec/03` §2); the others cleared,
        // in case the pointer has crossed monitors since the overlay opened.
        const current = this.#pointerMonitorIndex();
        this.#roots.forEach((root, index) =>
            root.setHint(index === current ? (on ? HINT_RECORD : HINT_ADJUSTING) : null),
        );
    }

    /**
     * `spec/03` §5.1: "typing into the W/H fields sets exact sizes anchored at the
     * top-left; the lock keeps the ratio".
     *
     * The fields carry **physical** pixels, because that is what they display and what
     * the file will contain, so they are converted back to logical here -- the one place
     * that conversion belongs.
     */
    #onSizeTyped(physicalWidth: number | null, physicalHeight: number | null): void {
        if (this.#rect === null) return;
        const scale = this.#toolbarScale();

        let next = this.#rect;
        if (physicalWidth !== null)
            next = setWidth(next, Math.max(1, Math.round(physicalWidth / scale)), this.#lockedRatio);
        if (physicalHeight !== null)
            next = setHeight(next, Math.max(1, Math.round(physicalHeight / scale)), this.#lockedRatio);

        this.#rect = next;
        this.#applyClamp();
        this.#phase = 'adjusting';
        this.#draw();
    }

    #onLockChanged(locked: boolean): void {
        this.#toolbarRatioLocked = locked;
        // Locking captures the ratio as it is now, which is the same rule Shift follows
        // during a drag (`spec/03` §5.1).
        this.#lockedRatio =
            locked && this.#rect !== null && this.#rect.height > 0
                ? this.#rect.width / this.#rect.height
                : null;
        info(`aspect lock ${locked ? 'on' : 'off'} at ${this.#lockedRatio ?? 'free'}`);
    }

    #onRatioChosen(ratio: number | null): void {
        this.#toolbarRatioLocked = ratio !== null;
        this.#lockedRatio = ratio;
        if (ratio === null || this.#rect === null) return;
        // Apply it immediately: choosing 16:9 and seeing nothing happen would read as a
        // broken control. Width is kept and height follows, which is the less surprising
        // of the two.
        this.#rect = setHeight(this.#rect, Math.round(this.#rect.width / ratio), null);
        this.#applyClamp();
        this.#draw();
    }

    #toolbarScale(): number {
        const root = this.#roots[this.#pointerMonitorIndex()] ?? this.#roots[0];
        return root?.monitor.scale ?? 1;
    }

    /**
     * The index of the monitor holding the pointer.
     *
     * `global.get_pointer()`, not `global.display.get_current_monitor()`, for the reason
     * recorded in `docs/decisions.md` D16: that call returned the same monitor for every
     * pointer position on a real two-monitor rig.
     */
    #pointerMonitorIndex(monitors?: readonly MonitorGeometry[]): number {
        const list = monitors ?? this.#roots.map(r => r.monitor);
        const [px, py] = global.get_pointer();
        const found = list.findIndex(
            m => px >= m.x && px < m.x + m.width && py >= m.y && py < m.y + m.height,
        );
        return found === -1 ? 0 : found;
    }

    #onKeyPress(event: Clutter.Event): boolean {
        const symbol = event.get_key_symbol();

        // A dimension field has focus: the keystroke is a number, not a command. Only Esc
        // is taken back, so there is always a way out of a field.
        if (this.#toolbar?.editing === true && symbol !== Clutter.KEY_Escape)
            return Clutter.EVENT_PROPAGATE;

        if (symbol === Clutter.KEY_Escape) {
            // A ratio list is the innermost thing open, so Esc closes that first --
            // `spec/03` §5.1's two-step Esc only starts once nothing is layered over it.
            if (this.#toolbar?.dismissPopups() === true) return Clutter.EVENT_STOP;

            // `spec/03` §5.1: "first Esc clears a selection in All-In-One, second
            // closes". The two-step is what makes a mis-drag cheap to undo.
            if (this.#toolbar?.editing === true) {
                this.#toolbar.unfocusFields();
                return Clutter.EVENT_STOP;
            }
            if (this.#allInOne && this.#rect !== null) {
                this.#clearSelection();
                return Clutter.EVENT_STOP;
            }
            this.#settle(null);
            return Clutter.EVENT_STOP;
        }

        // `spec/03` §5.1: Enter confirms. In All-In-One that is the *only* keyboard
        // confirm, which is why there is no Capture button in the toolbar.
        if (symbol === Clutter.KEY_Return || symbol === Clutter.KEY_KP_Enter) {
            if (this.#rect !== null && this.#rect.width > 0 && this.#rect.height > 0) {
                this.#confirm(event.get_state());
                return Clutter.EVENT_STOP;
            }
            // Nothing selected: in All-In-One, fullscreen mode confirms the whole
            // monitor, which is `spec/03` §5.3's "click or Enter". Record has no toolbar
            // to pick fullscreen from, so an empty Enter *is* its fullscreen (`REC-01`).
            // Window mode picks a window, not a rect, so it needs no selection either: the
            // flow opens the picker whatever was here, and with none, Enter did nothing,
            // on an overlay whose toolbar hides without a selection (D136).
            const needsNoSelection = this.#mode === 'fullscreen' || this.#mode === 'window';
            if (this.#record || (this.#allInOne && needsNoSelection)) {
                const root = this.#roots[this.#pointerMonitorIndex()] ?? this.#roots[0];
                if (root !== undefined) {
                    this.#rect = {
                        x: root.monitor.x,
                        y: root.monitor.y,
                        width: root.monitor.width,
                        height: root.monitor.height,
                    };
                    this.#confirm(event.get_state());
                }
            }
            return Clutter.EVENT_STOP;
        }

        // `spec/00` §9: every capture reachable without the mouse. The toolbar's modes
        // were buttons and nothing else, so from the keyboard All-In-One could only take
        // the area it opened on. Tab and Shift+Tab step through them in the toolbar's
        // order, as Tab steps through the window picker's windows (D137).
        if (
            this.#toolbar !== null &&
            (symbol === Clutter.KEY_Tab || symbol === Clutter.KEY_ISO_Left_Tab)
        ) {
            const back =
                symbol === Clutter.KEY_ISO_Left_Tab ||
                (event.get_state() & Clutter.ModifierType.SHIFT_MASK) !== 0;
            const modes = TOOLBAR_MODES.filter(m => m.ready).map(m => m.id);
            const next = cycle<ToolbarMode>(modes, this.#mode, back ? -1 : 1);
            // The toolbar is hidden while nothing is selected, and a mode chosen then
            // would be chosen out of sight.
            if (this.#rect === null) this.#startFromKeyboard();
            if (next !== null) this.#onModeChanged(next);
            return Clutter.EVENT_STOP;
        }

        // `spec/03` §5.1: arrows nudge 1 px, Shift+arrows 10, Ctrl+arrows resize by 1,
        // Ctrl+Shift by 10. Only in Adjusting: during a drag the pointer is in charge.
        const direction = arrowDirection(symbol);
        if (direction !== null && this.#allInOne && this.#phase === 'idle' && this.#rect === null) {
            // Nothing to move yet, and nothing but the pointer to draw one with (D137).
            this.#startFromKeyboard();
            return Clutter.EVENT_STOP;
        }
        if (direction !== null && this.#phase === 'adjusting' && this.#rect !== null) {
            const state = event.get_state();
            const fast = (state & Clutter.ModifierType.SHIFT_MASK) !== 0;
            const step = fast ? STEP_FAST : STEP;
            const resizing = (state & Clutter.ModifierType.CONTROL_MASK) !== 0;
            this.#rect = resizing
                ? resizeByKey(this.#rect, direction, step)
                : nudge(this.#rect, direction, step);
            this.#applyClamp();
            this.#draw();
            return Clutter.EVENT_STOP;
        }

        // `spec/08` §6's `modifier` crosshair: Ctrl summons it, releasing hides it. Held
        // here as well as on motion so it appears without the pointer having to move.
        if (
            this.#crosshairMode === 'modifier' &&
            (symbol === Clutter.KEY_Control_L || symbol === Clutter.KEY_Control_R)
        ) {
            this.#setCrosshairVisible(true);
            const [px, py] = global.get_pointer();
            this.#updateCrosshair(px, py);
            return Clutter.EVENT_STOP;
        }

        if (symbol === Clutter.KEY_space && this.#phase === 'sizing' && this.#rect !== null) {
            // Freeze the size and remember where the pointer sits inside the selection,
            // so the selection does not jump when the move begins.
            this.#moveOffset = [
                this.#rect.x - this.#pointer[0],
                this.#rect.y - this.#pointer[1],
            ];
            this.#phase = 'moving';
            this.#setCursor(Clutter.CursorType.MOVE);
            return Clutter.EVENT_STOP;
        }

        return Clutter.EVENT_STOP;
    }

    #onKeyRelease(event: Clutter.Event): boolean {
        const released = event.get_key_symbol();
        if (
            this.#crosshairMode === 'modifier' &&
            (released === Clutter.KEY_Control_L || released === Clutter.KEY_Control_R)
        ) {
            this.#setCrosshairVisible(false);
            return Clutter.EVENT_STOP;
        }

        if (released === Clutter.KEY_space && this.#phase === 'moving') {
            this.#phase = 'sizing';
            this.#setCursor(Clutter.CursorType.CROSSHAIR);
            // Re-anchor to the corner furthest from the pointer, so continuing the drag
            // resizes from where the selection now is rather than snapping back to the
            // original press.
            if (this.#rect !== null) {
                const nearRight = this.#pointer[0] > this.#rect.x + this.#rect.width / 2;
                const nearBottom = this.#pointer[1] > this.#rect.y + this.#rect.height / 2;
                this.#anchor = [
                    nearRight ? this.#rect.x : this.#rect.x + this.#rect.width,
                    nearBottom ? this.#rect.y : this.#rect.y + this.#rect.height,
                ];
            }
            return Clutter.EVENT_STOP;
        }
        return Clutter.EVENT_STOP;
    }

    // --- selection state -----------------------------------------------------

    #modifiersFrom(event: Clutter.Event): Modifiers {
        const state = event.get_state();
        const shift = (state & Clutter.ModifierType.SHIFT_MASK) !== 0;
        const fromCentre = (state & Clutter.ModifierType.MOD1_MASK) !== 0;

        // `spec/03` §5.1: the toolbar's ratio for as long as it is set, else the one the
        // selection had when Shift was pressed, released when Shift is (D136).
        this.#lockedRatio = dragRatio(shift, this.#toolbarRatioLocked, this.#lockedRatio, this.#rect);
        return { aspectLock: this.#lockedRatio !== null, fromCentre };
    }

    #update(modifiers: Modifiers = { aspectLock: false, fromCentre: false }): void {
        this.#rect = rectFromDrag(this.#anchor, this.#pointer, modifiers, this.#lockedRatio);
        this.#applyClamp();
        this.#draw();
    }

    /**
     * `spec/01` §1: a selection spanning two differently-scaled monitors is refused, so
     * it is clamped to whichever monitor holds most of it.
     */
    #applyClamp(): void {
        if (this.#rect === null) return;
        const rects = this.#roots.map(r => ({
            x: r.monitor.x,
            y: r.monitor.y,
            width: r.monitor.width,
            height: r.monitor.height,
        }));
        const index = dominantMonitor(this.#rect, rects);
        if (index !== null) this.#rect = clampToMonitor(this.#rect, rects[index]!);
    }

    #draw(): void {
        // `spec/03` §4: "While dragging, no handles; once released (All-In-One) 8
        // handles." So the handles are exactly the Adjusting state made visible.
        const showHandles = this.#allInOne && this.#phase === 'adjusting';
        const showLabel = this.#toolbar === null;
        for (const root of this.#roots) root.setSelection(this.#rect, showHandles, showLabel);
        // The toolbar follows the selection's monitor, not the one the overlay opened on
        // (`spec/03` §8). With no selection there is nothing to follow, so it keeps the
        // monitor it has and simply hides.
        this.#toolbar?.update(this.#rect, this.#monitorForRect(this.#rect));
    }

    /** The monitor holding most of a rect, for anything that has to sit beside it. */
    #monitorForRect(rect: Rect | null): MonitorGeometry | undefined {
        if (rect === null) return undefined;
        const index = dominantMonitor(
            rect,
            this.#roots.map(r => ({
                x: r.monitor.x,
                y: r.monitor.y,
                width: r.monitor.width,
                height: r.monitor.height,
            })),
        );
        return index === null ? undefined : this.#roots[index]?.monitor;
    }

    #setCrosshairVisible(visible: boolean): void {
        for (const crosshair of this.#crosshairs) crosshair.setVisible(visible);
    }

    #updateCrosshair(x: number, y: number): void {
        for (const crosshair of this.#crosshairs) crosshair.update(x, y);
    }

    // --- teardown ------------------------------------------------------------

    /**
     * Hides the chrome and waits for a redraw before returning.
     *
     * `spec/03` §3: "The overlay hides itself for one frame before grabbing so its chrome
     * is never in the shot."
     *
     * **On GNOME 50 the chrome would not end up in the shot anyway**, and this docstring
     * used to claim it would. Measured by disabling the hiding and capturing the same
     * rect twice: the two PNGs were byte-identical, difference bounding box `None`.
     * `Main.layoutManager.screenshotUIGroup` is excluded from `Shell.Screenshot`, which is
     * exactly what that group is for. `docs/decisions.md` D22.
     *
     * It stays, for three reasons that outlive the measurement: the user *sees* the chrome
     * vanish, which is the feedback that says the shot was taken; the exclusion is a
     * property of one capture API, and the portal fallback in `spec/10` §12 has no such
     * guarantee; and it costs one frame. Belt-and-braces, honestly labelled, rather than
     * load-bearing.
     */
    async hideForCapture(): Promise<void> {
        // An opening fade still running would leave a frozen frame half there under the
        // fly (`motion.ts`).
        if (this.#host !== null) settle(this.#host);
        // The toolbar is a child of a root, so hiding the roots hides it too -- but it is
        // hidden explicitly as well, because it is the one piece of chrome that sits
        // *inside* the selection when there is no room below it, and a stale reference to
        // it would put a dark pill in the middle of the shot.
        this.#toolbar?.update(null);
        for (const crosshair of this.#crosshairs) crosshair.destroy();
        this.#crosshairs = [];
        // In freeze mode the frozen frame stays: it is not a source of pixels for its own
        // capture, and `spec/03` §7 step 5 needs it on screen to fade out of. See
        // `OverlayRoot.hideChrome`.
        for (const root of this.#roots) root.hideChrome(this.#freezeRequested);
        await waitForRedraw();
    }

    /**
     * `spec/03` §2 wants a crosshair over the overlay and a move cursor while
     * Space-moving.
     *
     * `spec/01` §2 row 2 specifies `global.display.set_cursor(Meta.Cursor.CROSSHAIR)`.
     * On GNOME 50 neither `Meta.Cursor` nor `Display.set_cursor` exists any more:
     * cursors are a per-actor property, `Clutter.Actor.set_cursor_type`. That is a
     * better fit for this overlay -- the handle actors carry their own resize cursors
     * declaratively (see `root.ts`) instead of the engine recomputing one on every
     * motion event, which the 2 ms budget in `spec/10` §7 would rather it did not.
     * `[P→V] 2026-09-07`.
     */
    #setCursor(cursor: Clutter.CursorType): void {
        for (const root of this.#roots) root.actor.set_cursor_type(cursor);
    }

    #settle(value: AreaSelection | null): void {
        if (this.#settled) return;
        this.#settled = true;
        this.#outcome = value === null ? 'cancelled' : 'selected';
        this.#phase = 'idle';
        const resolve = this.#resolve;
        this.#resolve = null;
        resolve?.(value);
    }

    /**
     * Releases the grab, the cursor and every actor.
     *
     * `spec/03` §12 item 6 makes this an acceptance test: "Esc at every state returns to
     * Idle with no leftover actors or grabs". Each step is guarded separately so one
     * failure cannot strand the others -- a leaked modal grab locks the user's desktop
     * until they log out, which is the worst thing this extension could do.
     *
     * @param motion fade the roots out on this curve instead of cutting them. `spec/03`
     *   §7 step 5 wants the frozen frame to fade over ~150 ms while the capture flies to
     *   the corner. Without one, a cancelled overlay fades over `spec/09` §3's 100 ms and
     *   any other goes at once. **The grab is released immediately regardless** -- only
     *   the pixels linger, and they linger with `reactive` off so clicks reach the
     *   desktop underneath. Anything that fails during the fade still ends in `destroy`,
     *   because the fade's completion handler is the only thing that owns those actors.
     */
    destroy(motion: Motion | null = null): void {
        if (open.delete(this)) tellPets({ kind: 'overlay', open: false });
        const fade = motion ?? (this.#outcome === 'cancelled' ? OVERLAY_CANCEL : null);
        if (this.#grab !== null) {
            try {
                Main.popModal(this.#grab);
            } catch (e) {
                error('could not pop the overlay modal grab', e);
            }
            this.#grab = null;
        }

        // The cursor was per-actor, so destroying the actors restores it; nothing
        // global was changed and there is nothing to put back.

        try {
            this.#toolbar?.destroy();
        } catch (e) {
            error('could not destroy the toolbar', e);
        }
        this.#toolbar = null;

        for (const crosshair of this.#crosshairs) crosshair.destroy();
        this.#crosshairs = [];

        // The roots live in the host (see `host.ts`), so the host is what fades and the
        // host is what goes. One fade rather than one per monitor, which is also the only
        // way a two-monitor freeze fades as a single gesture instead of a race.
        const host = this.#host;
        this.#host = null;
        const roots = this.#roots;
        this.#roots = [];
        const drop = () => {
            for (const root of roots) {
                try {
                    root.destroy();
                } catch (e) {
                    error('could not destroy an overlay root', e);
                }
            }
            try {
                host?.destroy();
            } catch (e) {
                error('could not destroy the overlay host', e);
            }
        };

        if (fade !== null && host !== null) {
            try {
                fadeOutAndDrop(host, fade, drop);
            } catch (e) {
                error('could not fade the overlay out; dropping it now', e);
                drop();
            }
        } else {
            drop();
        }

        // The snapshot holds a whole screen's worth of texture. Dropping the reference
        // here rather than leaving it on a destroyed overlay is the difference between
        // a few megabytes freed on Esc and a few megabytes held until the next GC.
        this.#snapshot = null;
        this.#settle(null);
    }
}

/** Maps a key symbol to a nudge direction, or `null` for anything else. */
function arrowDirection(symbol: number): Direction | null {
    switch (symbol) {
        case Clutter.KEY_Left:
        case Clutter.KEY_KP_Left:
            return 'left';
        case Clutter.KEY_Right:
        case Clutter.KEY_KP_Right:
            return 'right';
        case Clutter.KEY_Up:
        case Clutter.KEY_KP_Up:
            return 'up';
        case Clutter.KEY_Down:
        case Clutter.KEY_KP_Down:
            return 'down';
        default:
            return null;
    }
}
