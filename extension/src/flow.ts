// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The capture flow, shared by every way of starting one: a global hotkey, the panel menu,
 * `BeginCapture` over D-Bus, the CLI and the `octosnap://` URL scheme all end up here.
 *
 * Deliberately one function rather than one per entry point. `spec/03` §1 lists nine
 * modes across four entry points and says "all modes share the same overlay engine; only
 * the toolbar, accent colour, and confirm behavior differ" -- if the *flow* forks per
 * entry point, they drift. What forks below is the **selection**: how the rect is
 * decided is per mode, and everything after the rect is one code path.
 *
 * The order at the end is fixed by `spec/10` §8 and is not an implementation detail:
 * pixels to disk, then the twin to disk, then tell the app. Any capture that reaches the
 * second step is recoverable without the app ever hearing about it.
 */

import GLib from 'gi://GLib';
import type Meta from 'gi://Meta';

import { notifyCapture } from './app.js';
import {
    type CaptureCost,
    type FrozenScreen,
    type StartedCapture,
    allMonitors,
    captureWindowToFile,
    currentMonitor,
    freezeScreen,
    startAreaCapture,
    startFrozenArea,
} from './capture.js';
import { waitForRedraw } from './later.js';
import { AreaOverlay, type OverlayOptions } from './overlay/engine.js';
import { Countdown } from './overlay/countdown.js';
import { flyToCorner } from './overlay/fly.js';
import { FROZEN_FADE } from './overlay/motion.js';
import { resolveRectScale, scalesUnder } from './overlay/selection.js';
import { WindowPicker, describeWindow } from './overlay/window.js';
import type { Settings } from './settings.js';
import { playShutter } from './sound.js';
import { type CaptureMeta, type Rect, newEntry, writeTwin } from './spool.js';
import { info } from './log.js';
import { tellPets } from './pets/events.js';
import type { GifOverrides } from './recordChoices.js';

/** Modes this build can actually run. `dbus.ts` refuses the rest by name. */
export const IMPLEMENTED_MODES = [
    'all-in-one',
    'area',
    'window',
    'fullscreen',
    'previous-area',
    'self-timer',
    // `spec/07` §2.1's text capture. An area selection in everything the shell does --
    // the same overlay, the same pixels, the same spool -- and a different mode on the
    // twin, which is what tells the app to read it rather than save it. The recognition
    // is entirely the app's: the models are 10 MB apiece and the compositor's main loop
    // is the last place a second of inference belongs.
    'ocr',
] as const;

export type CaptureModeName = (typeof IMPLEMENTED_MODES)[number];

export interface CaptureOptions {
    /** An explicit logical rect, from the CLI, a URL, or `previous-area`. */
    rect?: Rect;
    /** `spec/10` §3.3: `copy`, `save`, `annotate`, `upload`, `pin`. */
    requestedAction?: string;
    /** Modifier mask held at confirm (`CAP-15`). */
    modifiers?: number;
    /** `CAP-09`, overriding `shot-freeze` for this capture. */
    freeze?: boolean;
    /** `CAP-11`, overriding `shot-cursor` for this capture. */
    cursor?: boolean;
    /** `CAP-06`, in seconds. */
    timer?: number;
    /**
     * `CAP-12`: what hides the desktop icons for the capture and puts them back. Handed
     * in by the caller that owns it rather than looked up, so this module stays free of
     * the extension's state and `disable()` has one owner to ask.
     */
    desktopIcons?: { beginCapture(wanted: boolean): void; endCapture(): void } | undefined;
    /**
     * `spec/06` §3 from the All-In-One toolbar: the user chose Record. The flow hands the
     * rect back and finishes as a cancel would -- nothing is captured -- and the caller
     * starts the recording once this flow has let go of the overlay and the desktop icons.
     * A callback rather than a return variant so `runCapture`'s result stays a capture.
     */
    onRecord?: (rect: Rect, gif: GifOverrides) => void;
    /**
     * `spec/07` §1.1: the All-In-One toolbar ended on Scrolling. The same handoff as
     * `onRecord`, and for the same reason -- the overlay has to be gone before anything
     * else happens, and here it has to be gone before the user can scroll the page the
     * selection is over.
     */
    onScrolling?: (rect: Rect) => void;
    /**
     * `spec/07` §2.1's second text shortcut, and `capture-text?linebreaks=`: keep the
     * page's line breaks for this read. Absent leaves it to the app's `ocr-line-breaks`,
     * like `freeze` and `cursor` above -- a shortcut is not a preference.
     *
     * Carried rather than acted on: the extension never reads the text, it only puts the
     * answer on the twin so the app knows which of the two shortcuts asked.
     */
    linebreaks?: boolean;
    /** `spec/03` §4's scrolling URL parameter `start`, passed straight through. */
    start?: boolean;
    /**
     * `spec/14` §10: the mode All-In-One opens on when a pet's menu asked for one, instead
     * of the last used (`last-mode`). Only All-In-One reads it.
     */
    initialMode?: ToolbarModeName;
}

/** The modes All-In-One's toolbar can open on, as `Settings.lastMode` names them. */
export type ToolbarModeName = Settings['lastMode'];

/** What a mode decided to capture, before any pixels are read. */
interface Target {
    rect: Rect;
    scale: number;
    /** Set for window mode, so the twin carries `spec/10` §3.3's source-window fields. */
    window?: Meta.Window;
    /** True when the pixels must come from `screenshot_window` rather than a rect. */
    wholeWindow?: boolean;
}

/**
 * Runs one capture. Resolves to `null` when the user cancelled.
 *
 * `settings` is passed in rather than read from a module global so the flow stays
 * testable and so `disable()` cannot leave a stale `Gio.Settings` alive behind it.
 */
export async function runCapture(
    mode: CaptureModeName,
    settings: Settings,
    options: CaptureOptions = {},
): Promise<CaptureMeta | null> {
    const opened = GLib.get_monotonic_time();

    const wantsFreeze = options.freeze ?? settings.freezeScreen;
    const wantsCursor = options.cursor ?? settings.captureCursor;

    // What the twin will say this capture was. The same as `mode` for every entry point
    // but All-In-One, whose toolbar is allowed to change its mind after the flow started.
    let reported: CaptureModeName = mode;
    // Whether the rect is one the user drew, which All-In-One and Previous Area may offer
    // again. All-In-One's Window and Fullscreen modes end on a window's frame or a whole
    // monitor, which were remembered as if drawn (D136).
    let drawn = mode === 'area' || mode === 'all-in-one';

    let overlay: AreaOverlay | null = null;
    let picker: WindowPicker | null = null;
    let countdown: Countdown | null = null;
    let frozen: FrozenScreen | null = null;
    let modifiers = options.modifiers ?? 0;

    // Everything the capture holds for the user's sake -- the grab, the chrome, the icons
    // it hid -- given back once, as soon as the pixels are read, or in the `finally` on
    // every path that never got that far. `spec/03` §12 item 6 requires no leftover actors
    // or grabs, and a leaked modal grab locks the desktop until logout.
    //
    // The fade is `spec/03` §7 step 5's other half: "in Freeze mode the frozen frame fades
    // out (~150 ms) at the same time" as the capture flies away. Only when there was a
    // frozen frame -- a live capture has nothing on screen to fade, and holding the dim for
    // an extra 150 ms after a capture would be a regression. A cancelled overlay fades over
    // `spec/09` §3's 100 ms by itself (`engine.ts`). The grab goes immediately either way;
    // only the pixels linger.
    let released = false;
    const release = () => {
        if (released) return;
        released = true;
        overlay?.destroy(frozen !== null ? FROZEN_FADE : null);
        picker?.destroy();
        countdown?.destroy();
        // The icons come back on every path too, cancelled or failed included.
        options.desktopIcons?.endCapture();
    };
    // Settled once the app has been told, or the flow has given up. The fly's landed
    // picture stands in for a card that is late until then (`fly.ts`).
    let handOver = () => {};
    const handedOver = new Promise<void>(resolve => {
        handOver = resolve;
    });

    try {
        // `CAP-12`, before anything is frozen or read, so neither the frozen frame nor the
        // pixels have the icons in them.
        options.desktopIcons?.beginCapture(settings.hideDesktop);

        // --- decide what to capture ------------------------------------------------

        let target: Target | null;
        switch (mode) {
            case 'fullscreen':
                target = fullscreenTarget(options.rect);
                break;

            case 'previous-area': {
                // `CAP-05`. An explicit rect wins, because the CLI and the URL scheme use
                // this mode to mean "capture exactly this".
                const remembered = options.rect ?? settings.lastArea;
                if (remembered === null) {
                    // Nothing to repeat. Falling back to the overlay is more useful than
                    // failing: the user asked for a capture, and one selection later the
                    // shortcut works for good.
                    info('no previous area remembered; falling back to a selection');
                    overlay = new AreaOverlay();
                    target = await selectArea(overlay, settings, wantsFreeze, wantsCursor, opened);
                    if (target !== null) modifiers |= overlay.confirmModifiers;
                } else {
                    target = rectTarget(remembered);
                }
                break;
            }

            case 'window': {
                picker = new WindowPicker();
                const chosen = await picker.open();
                if (chosen === null) return null;
                modifiers |= chosen.modifiers;
                target = {
                    rect: chosen.rect,
                    scale: chosen.monitor.scale,
                    window: chosen.window,
                    wholeWindow: true,
                };
                break;
            }

            case 'self-timer': {
                // `spec/03` §5.4: select first, *then* count down over the live screen.
                if (options.rect !== undefined) {
                    target = rectTarget(options.rect);
                } else {
                    overlay = new AreaOverlay();
                    target = await selectArea(overlay, settings, wantsFreeze, wantsCursor, opened);
                    if (target !== null) modifiers |= overlay.confirmModifiers;
                }
                if (target === null) return null;

                // The overlay goes away before the countdown starts, or the user cannot
                // arrange what they are photographing.
                overlay?.destroy();
                overlay = null;

                countdown = new Countdown(options.timer ?? settings.selfTimer, settings.shutterSound);
                const completed = await countdown.run(target.rect);
                if (!completed) {
                    info('countdown cancelled');
                    return null;
                }
                await clearCountdown(countdown);
                countdown = null;
                break;
            }

            case 'all-in-one': {
                // `CAP-01`. The one mode whose overlay can change its own mind: the
                // toolbar is a mode switcher, so what comes back is a mode as well as a
                // rect and the flow has to honour whichever the user ended on.
                overlay = new AreaOverlay();
                target = await selectArea(overlay, settings, wantsFreeze, wantsCursor, opened, {
                    allInOne: true,
                    initialRect: settings.lastArea,
                    mode: options.initialMode ?? settings.lastMode,
                    gifDefaults: settings.appGifDefaults,
                });
                if (target === null) return null;
                modifiers |= overlay.confirmModifiers;

                const chosen = overlay.mode;
                // Remembered for next time, which is what makes `spec/03` §1's "mode =
                // last used" true across invocations.
                settings.rememberMode(chosen);

                // `spec/07` §2.1's text capture, chosen from the toolbar rather than from
                // its own shortcut. Everything about the capture is the same -- the same
                // overlay, the same pixels, the same spool -- so the only thing that
                // changes is the mode the twin carries, which is the whole of what tells
                // the app to read the picture instead of opening a card for it.
                if (chosen === 'ocr') reported = 'ocr';

                if (chosen === 'record') {
                    // Not a screenshot: no pixels, no twin, no fly. The rect goes back to
                    // the caller, which runs `record.ts` after this flow's `finally` has
                    // destroyed the overlay and released the icons (`REC-01`).
                    options.onRecord?.(target.rect, overlay.recordOverrides);
                    return null;
                }

                if (chosen === 'scrolling') {
                    // Not a screenshot either: the rect goes back to the caller, which
                    // runs `scroll.ts` once this flow's `finally` has dropped the overlay's
                    // modal grab -- without which nothing could scroll (`spec/07` §1.1).
                    options.onScrolling?.(target.rect);
                    return null;
                }

                if (chosen === 'fullscreen') drawn = false;

                if (chosen === 'window') {
                    drawn = false;
                    // `spec/03` §5.2 has All-In-One highlight windows inside the same
                    // overlay. This hands over to the standalone picker instead: the
                    // picker is a real, tested implementation of the feature, and a
                    // toolbar button that opened it is far better than one that switched
                    // a mode nothing acted on. Folding it into one overlay is a
                    // refinement, not a missing feature.
                    overlay.destroy();
                    overlay = null;
                    picker = new WindowPicker();
                    const chosenWindow = await picker.open();
                    if (chosenWindow === null) return null;
                    modifiers |= chosenWindow.modifiers;
                    target = {
                        rect: chosenWindow.rect,
                        scale: chosenWindow.monitor.scale,
                        window: chosenWindow.window,
                        wholeWindow: true,
                    };
                    break;
                }

                if (chosen === 'self-timer') {
                    // `spec/03` §5.4: the overlay goes away first, so the user can
                    // arrange whatever they are photographing while the clock runs.
                    overlay.destroy();
                    overlay = null;
                    countdown = new Countdown(options.timer ?? settings.selfTimer, settings.shutterSound);
                    if (!(await countdown.run(target.rect))) {
                        info('countdown cancelled');
                        return null;
                    }
                    await clearCountdown(countdown);
                    countdown = null;
                }
                break;
            }

            case 'area':
            default: {
                if (options.rect !== undefined) {
                    target = rectTarget(options.rect);
                } else {
                    overlay = new AreaOverlay();
                    target = await selectArea(overlay, settings, wantsFreeze, wantsCursor, opened);
                    if (target !== null) modifiers |= overlay.confirmModifiers;
                }
                break;
            }
        }

        if (target === null) return null;

        // Everything before this point is the user deciding, and none of it belongs in a
        // performance figure. `spec/10` §7's budget is for the machine's work.
        const confirmed = GLib.get_monotonic_time();
        // The same instant on the wall clock, because the app cannot read this process's
        // monotonic clock and the budget spans both of them.
        const confirmedAt = GLib.get_real_time();

        // --- read the pixels -------------------------------------------------------

        // `spec/03` §3: hide the chrome and let the compositor paint before grabbing, or
        // the dim and the frame end up in the capture.
        if (overlay !== null) {
            frozen = overlay.frozen;
            await overlay.hideForCapture();
        }
        if (picker !== null && target.window !== undefined) {
            picker.focusForCapture(target.window);
            await waitForRedraw();
            // `screenshot_window` takes whichever window has the focus, so a window that
            // did not take it would be captured as another under its name (D138).
            if (global.display.focus_window !== target.window) {
                throw new Error(
                    `${target.window.get_title() ?? 'the chosen window'} did not take the ` +
                    'focus, so it cannot be captured',
                );
            }
        }

        const entry = await newEntry();
        // Where the time went, for the paths that can say (`CaptureCost`).
        let cost: CaptureCost | null = null;
        // Every path but the window's reads the pixels on this turn and writes them on the
        // encoder's thread. D127: the user sees the first, and only the app waits for the
        // second.
        let started: StartedCapture | null = null;

        // The shutter sounds *here*, between hiding the chrome and reading the pixels,
        // because that instant is what the user perceives as the capture. `ACT-04`.
        playShutter(settings.shutterSound);
        // And the pets squint at the flash. They are never in the pixels (`pets/crew.ts`).
        tellPets({ kind: 'captured', rect: target.rect, mode: reported });

        if (target.wholeWindow === true) {
            // The scale is the window's buffer's, told from the file among the scales of the
            // monitors it is on: the picker's monitor's scale was wrong for a window its
            // client draws at another, and for one across two monitors (D138).
            const monitors = allMonitors().map(m => ({ rect: m.rect, scale: m.scale }));
            const under = scalesUnder(target.rect, monitors);
            const captured = await captureWindowToFile(
                entry.png,
                target.rect,
                under.length > 0 ? under : [target.scale],
                { includeCursor: wantsCursor },
            );
            // The compositor's shadow puts the capture outside both the frame and the
            // buffer rect, so the twin takes the rect the call reports rather than the
            // window's own geometry. See `captureWindowToFile`.
            const { rect, scale } = captured;
            if (rect.width !== target.rect.width || rect.height !== target.rect.height)
                info(
                    `window capture is ${rect.width}x${rect.height}; the ` +
                    `window itself reports ${target.rect.width}x${target.rect.height}, ` +
                    "so Mutter's shadow margin is in the shot",
                );
            target = { ...target, rect, scale };
        } else if (frozen !== null) {
            // Freeze mode already holds the pixels, so this composites out of the
            // snapshot the user was actually looking at rather than re-reading a screen
            // that has moved on since.
            started = startFrozenArea(frozen, target.rect, target.scale, entry.png, wantsCursor);
        } else if (wantsCursor) {
            // `CAP-11` with no freeze. The pixels come from a snapshot because the live
            // path cannot include the pointer at all -- but *which* snapshot is the whole
            // question, and this used to take a fresh one here.
            //
            // By this line there is no mouse pointer left to capture: `spec/03` §2 turned
            // it into the picking crosshair the moment the overlay opened, so a snapshot
            // taken now composites **the crosshair** into the shot. Reported from a real
            // session -- "the mouse doesn't exist, it becomes the screenshot pixel
            // picker" -- and that is exactly right.
            //
            // The overlay already holds a snapshot from before it existed (it needs one
            // for the loupe), and that one has the real arrow in it. Falling back to a
            // fresh snapshot is still correct for the modes that never open an overlay,
            // where nothing has replaced the pointer. `docs/decisions.md` D25.
            const snapshot = overlay?.snapshot ?? (await freezeScreen({ layers: true }));
            started = startFrozenArea(snapshot, target.rect, target.scale, entry.png, true);
        } else {
            started = startAreaCapture(target.rect, entry.png);
        }
        // With no copy of the pixels on the GPU, the fly has only the file to show.
        if (started !== null && started.pixels === null) cost = await started.written;

        // --- hand it over ----------------------------------------------------------

        // `spec/03` §7 step 5, the moment the pixels are read. The fly does not wait for
        // the file: its encode is most of a large capture's time, and `spec/10` §7's
        // "Capture -> card visible" counts to the fly's start (D127). `flyToCorner` starts a
        // Clutter transition and returns. The app times its card's fade-in to the
        // animation's end from `animation_ms` and `timestamp`, which is stamped here.
        const flying = GLib.get_monotonic_time();
        const flewAt = GLib.get_real_time();
        const animationMs = flyToCorner({
            path: entry.png,
            rect: target.rect,
            scale: target.scale,
            pixels: started?.pixels ?? null,
            handedOver,
        });
        // The pixels are read, so the grab, the chrome and the icons have no more to do,
        // and the desktop is the user's again while the file is written.
        release();

        // `spec/10` §8: on disk before the app hears of it.
        if (started !== null && cost === null) cost = await started.written;

        const meta: CaptureMeta = {
            path: entry.png,
            meta_path: entry.json,
            mode: reported,
            rect: target.rect,
            scale: target.scale,
            // The connector is resolved by the app from DisplayConfig, since the shell
            // does not expose one -- docs/spikes/15-monitor-metadata.md.
            display: '',
            window_alpha: false,
            // When the pixels were read and the fly set off, not when the file was done:
            // the card counts the flight from here (`remaining_flight` in the app).
            timestamp: flewAt,
            confirmed_at: confirmedAt,
            animation_ms: animationMs,
            modifiers,
        };
        if (options.requestedAction) meta.requested_action = options.requestedAction;
        if (options.linebreaks !== undefined) meta.linebreaks = options.linebreaks;
        if (target.window !== undefined) Object.assign(meta, describeWindow(target.window));

        // `CAP-05` and All-In-One's restore. Only for a real selection: remembering a
        // whole monitor or a window's frame would make "previous area" mean something
        // the user never selected.
        if (drawn && options.rect === undefined) {
            settings.rememberArea(target.rect, meta.display);
        }

        // Before notifying, per spec/10 §8. Throws if it cannot, so the caller never
        // reports a capture it could not make recoverable.
        writeTwin(meta);
        notifyCapture(meta);

        const done = GLib.get_monotonic_time();
        const captureMs = (done - confirmed) / 1000;
        const selectionMs = (confirmed - opened) / 1000;
        const flyMs = (flying - confirmed) / 1000;
        info(
            `captured ${mode} ${target.rect.width}x${target.rect.height} logical ` +
            `at scale ${target.scale} -> ${entry.id}.png in ${captureMs.toFixed(1)} ms` +
            (selectionMs > 0 ? ` (after ${selectionMs.toFixed(0)} ms selecting)` : '') +
            `, flying after ${flyMs.toFixed(1)} ms` +
            (cost === null
                ? ''
                : `; repaint ${cost.paintMs.toFixed(1)} ms and readback ` +
                  `${cost.readMs.toFixed(1)} ms on the main loop, encode ` +
                  `${cost.encodeMs.toFixed(1)} ms on a thread`),
        );
        // `docs/spikes/17`: 60 ms holds for the sizes people select and breaks only for a
        // 4K fullscreen. Reported per megapixel so the log says which case this was.
        const megapixels = (target.rect.width * target.rect.height * target.scale ** 2) / 1e6;
        if (captureMs > 60) {
            info(
                `capture took ${captureMs.toFixed(1)} ms for ${megapixels.toFixed(2)} Mpx ` +
                `(${(captureMs / Math.max(megapixels, 0.01)).toFixed(0)} ms/Mpx); ` +
                'the budget is 60 ms',
            );
        }
        return meta;
    } finally {
        release();
        handOver();
    }
}

/**
 * Kept so the M0 acceptance path and its tests keep working while `dbus.ts` and the
 * keybindings move to `runCapture`.
 */
export async function runAreaCapture(
    settings: Settings,
    options: CaptureOptions = {},
): Promise<CaptureMeta | null> {
    return runCapture('area', settings, options);
}

/** Opens the selection overlay and turns its result into a target. */
async function selectArea(
    overlay: AreaOverlay,
    settings: Settings,
    freeze: boolean,
    cursor: boolean,
    requestedAt: number,
    extra: Partial<OverlayOptions> = {},
): Promise<Target | null> {
    const selection = await overlay.open({
        freeze,
        cursor,
        crosshair: settings.crosshair,
        magnifier: settings.magnifier,
        requestedAt,
        ...extra,
    });
    if (selection === null) return null;
    // The selection's own monitor, not the pointer's: a drag can end on a different
    // output from where it started, and the scale has to be the one the pixels were
    // captured at (`spec/01` §1).
    return { rect: selection.rect, scale: selection.monitor.scale };
}

/**
 * `CAP-04`: the monitor under the pointer, or the one an explicit rect names.
 *
 * `spec/01` §2 row 35 and `CAP-18`: fullscreen means "the display the cursor is on", not
 * the primary one. An explicit rect arrives when the app has resolved a `display`
 * connector to a monitor, which it can do and the extension cannot
 * (`docs/spikes/15-monitor-metadata.md`).
 */
function fullscreenTarget(explicit?: Rect): Target {
    // An explicit rect here is a whole monitor the app resolved from a connector, so it
    // goes through the same mixed-scale check as any other caller-supplied rect.
    if (explicit !== undefined) return rectTarget(explicit);
    const monitor = currentMonitor();
    return { rect: monitor.rect, scale: monitor.scale };
}

/**
 * A caller-supplied rect, with the scale of the monitor it is actually on.
 *
 * Throws rather than guessing when `spec/01` §1 says the rect cannot be captured -- it
 * spans monitors at different scales, or it is off-screen entirely. The error reaches the
 * caller as `CaptureCancelled(handle, "error:...")`, which is `spec/10` §3.1's channel for
 * exactly this and is far better than a PNG whose size the twin cannot describe.
 *
 * Not the monitor holding the *origin*, which is what an earlier version used: for a rect
 * that starts just off the left edge of a display, the origin's monitor and the rect's
 * monitor are different, and the origin's answer is the wrong one.
 */
function rectTarget(rect: Rect): Target {
    const monitors = allMonitors();
    const resolved = resolveRectScale(
        rect,
        monitors.map(m => ({ rect: m.rect, scale: m.scale })),
    );
    if (!resolved.ok) throw new Error(resolved.reason);
    return { rect, scale: resolved.scale };
}

/**
 * Takes a finished countdown off the screen and lets the compositor paint once without it,
 * before any pixels are read. The stage repaint leaves capture chrome out by itself
 * (`startAreaCapture`), but a pointer capture reads a snapshot of the screen as it was
 * last painted, and the countdown would be in it (D130).
 */
async function clearCountdown(countdown: Countdown): Promise<void> {
    countdown.destroy();
    await waitForRedraw();
}
