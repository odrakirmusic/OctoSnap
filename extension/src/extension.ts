// SPDX-License-Identifier: GPL-3.0-or-later

import GLib from 'gi://GLib';
import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';

import { activateApp, stopRecording } from './app.js';
import { ShellService } from './dbus.js';
import { DesktopIcons } from './desktop.js';
import { type CaptureModeName, type CaptureOptions, runCapture } from './flow.js';
import { Handoff } from './handoff.js';
import { Indicator } from './indicator.js';
import { Keybindings } from './keybindings.js';
import { type RecordOptions, runRecording } from './record.js';
import { type ScrollCaptureOptions, runScrollingCapture } from './scroll.js';
import { RecordingCoordinator } from './recording.js';
import { Settings } from './settings.js';
import { EXTENSION_VERSION } from './protocol.js';
import { error, forgetLog, info, setVerbose } from './log.js';
import { activeCountdown } from './overlay/countdown.js';
import { closeOverlays } from './overlay/engine.js';
import { closeWindowPickers } from './overlay/window.js';
import { forgetRedrawWaits } from './later.js';
import { forgetIcons } from './icons.js';
import { Crew } from './pets/crew.js';
import { forgetPetEvents, tellPets } from './pets/events.js';

/**
 * One row per bound action: the settings key, the mode it starts, and the after-capture
 * action it forces.
 *
 * `CAP-14`'s "Capture Area and …" variants are exactly this table with `action` set. They
 * are shortcut-only by design -- appendix A.10 confirms they never appear as menu items --
 * and whether they *replace* or *add to* the configured after-capture set is the app's
 * decision, from `area-shortcuts-respect-actions` (`spec/08` §6). The extension only says
 * which one was pressed.
 */
const SHORTCUTS: readonly {
    key: string;
    mode: CaptureModeName;
    action?: string;
    linebreaks?: boolean;
}[] = [
    { key: 'all-in-one', mode: 'all-in-one' },
    { key: 'capture-area', mode: 'area' },
    { key: 'capture-fullscreen', mode: 'fullscreen' },
    { key: 'capture-window', mode: 'window' },
    { key: 'capture-previous-area', mode: 'previous-area' },
    { key: 'self-timer', mode: 'self-timer' },
    { key: 'capture-area-copy', mode: 'area', action: 'copy' },
    { key: 'capture-area-save', mode: 'area', action: 'save' },
    { key: 'capture-area-annotate', mode: 'area', action: 'annotate' },
    { key: 'capture-area-upload', mode: 'area', action: 'upload' },
    { key: 'capture-area-pin', mode: 'area', action: 'pin' },
    // `spec/07` §2.1's two text shortcuts, the second of which differs by one boolean:
    // "Two shortcuts/variants: keep line breaks vs. join into one line". Neither writes
    // the setting -- `linebreaks` rides on this one capture's twin.
    { key: 'capture-text', mode: 'ocr' },
    { key: 'capture-text-single-line', mode: 'ocr', linebreaks: false },
];

/**
 * `spec/08` §3's other groups: a shortcut that activates one of the app's actions. The
 * extension only relays the press; what the action does, and whether it can, is the app's
 * (`spec/10` §2's two halves). `toggle-desktop-icons` is not here because the icons are
 * the extension's own (`desktop.ts`).
 */
const APP_ACTIONS: readonly {
    key: string;
    action: string;
    parameter?: () => GLib.Variant;
}[] = [
    { key: 'open-settings', action: 'open-settings', parameter: () => new GLib.Variant('s', 'general') },
    { key: 'overlays-toggle-visibility', action: 'toggle-overlays' },
    { key: 'overlays-save-all', action: 'save-all-overlays' },
    { key: 'overlays-close-all', action: 'close-all-overlays' },
    { key: 'restore-recently-closed', action: 'restore-recent' },
    { key: 'pins-toggle-visibility', action: 'toggle-pins' },
    { key: 'pins-close-all', action: 'close-all-pins' },
    { key: 'copy-last-capture', action: 'copy-last' },
    { key: 'save-last-capture', action: 'save-last' },
    { key: 'annotate-last-capture', action: 'annotate-last' },
    { key: 'open-from-clipboard', action: 'open-from-clipboard' },
    { key: 'open-history', action: 'open-history' },
    // `spec/06` §3's Stop as a plain relay: the app's `stop-recording` action is a no-op
    // when nothing is recording, so this never has to know the state. Start (`record-gif`)
    // is not here -- it opens the overlay, so it goes through `#record` below.
    { key: 'recording-stop', action: 'stop-recording' },
];

/**
 * The compositor half of OctoSnap (`spec/10` §1): global hotkeys, the capture overlay,
 * pixel capture, and the panel indicator.
 *
 * Everything `enable` creates, `disable` destroys. That is not tidiness: this code runs
 * inside `gnome-shell`, so a leaked actor, grab or keybinding does not leak into
 * OctoSnap -- it leaks into the user's desktop and survives until they log out.
 */
export default class OctoSnapExtension extends Extension {
    #settings: Settings | null = null;
    #service: ShellService | null = null;
    #keybindings: Keybindings | null = null;
    #indicator: Indicator | null = null;
    #desktopIcons: DesktopIcons | null = null;
    #recording: RecordingCoordinator | null = null;
    /** `spec/14`'s desktop pets, out while `pets-enabled` is on. */
    #crew: Crew | null = null;
    /** One capture at a time, for the same reason `ShellService` enforces it. */
    #busy = false;
    /** `changed::debug-log` on the settings, which `disable` disconnects. */
    #debugLogChanged = 0;

    override enable(): void {
        const settings = new Settings(this.getSettings());
        this.#settings = settings;
        // First of all, so that enable's own lines follow it too.
        setVerbose(settings.debugLog);
        this.#debugLogChanged = settings.raw.connect('changed::debug-log', () => setVerbose(settings.debugLog));

        // First, because the service, the captures and the recorder all reach for it;
        // destroyed after them in `disable`, which is what puts hidden icons back
        // (`spec/07` §7 item 7's watchdog).
        const desktopIcons = new DesktopIcons();
        this.#desktopIcons = desktopIcons;

        // The panel indicator, before the recorder that drives its red state.
        const indicator = new Indicator(settings, {
            capture: mode => void this.#capture(mode, {}),
            record: () => void this.#record({}),
            stopRecording: () => stopRecording(),
            openHistory: () => activateApp('open-history'),
            scrolling: () => this.#service?.scrolling ?? 'none',
            advanceScrolling: () => activateApp('scroll-advance'),
            toggleDesktopIcons: () => desktopIcons.apply('toggle'),
            desktopIcons: () => ({ available: desktopIcons.available(), hidden: desktopIcons.hidden }),
        });
        indicator.enable();
        this.#indicator = indicator;

        // The recording coordinator: the red frame, the panel timer, the hidden icons and
        // the crash watchdog. The service hands it the app's `SetRecordingState` and
        // `ShowRecordingFrame`.
        const recording = new RecordingCoordinator({ settings, desktopIcons, panel: indicator });
        this.#recording = recording;

        this.#service = new ShellService(settings, desktopIcons, recording);
        this.#service.export();

        this.#keybindings = new Keybindings(settings.raw);
        for (const shortcut of SHORTCUTS) {
            this.#keybindings.add(shortcut.key, () => {
                const options: CaptureOptions = {};
                if (shortcut.action !== undefined) options.requestedAction = shortcut.action;
                if (shortcut.linebreaks !== undefined) options.linebreaks = shortcut.linebreaks;
                void this.#capture(shortcut.mode, options);
            });
        }
        // Not captures: the app's actions, relayed.
        for (const relay of APP_ACTIONS) {
            this.#keybindings.add(relay.key, () =>
                activateApp(relay.action, relay.parameter?.() ?? null),
            );
        }
        // `spec/06` §3's record start: opens the red overlay, so it is a flow of its own
        // rather than a relay.
        this.#keybindings.add('record-gif', () => void this.#record({}));
        // `spec/07` §1.1's own selection, for the same reason: an overlay of its own
        // rather than a relay to an app action.
        this.#keybindings.add('scrolling-capture', () => void this.#scrolling({}));
        // And the one thing that is the extension's own.
        this.#keybindings.add('toggle-desktop-icons', () => {
            desktopIcons.apply('toggle');
        });
        this.#keybindings.add('toggle-pets', () => settings.setPetsEnabled(!settings.petsEnabled));

        // Last, over everything else: the pets ask the others for captures and pins, and a
        // pet that cannot come out must not keep the capture tool from working.
        const crew = new Crew(settings, {
            capture: (mode, options) => void this.#capture(mode, options),
            activate: (action, parameter) => activateApp(action, parameter),
            pins: () => this.#service?.pinWindows() ?? [],
            scrolling: () => this.#service?.scrolling ?? 'none',
        });
        this.#crew = crew;
        try {
            crew.enable();
        } catch (e) {
            error('the pets could not start', e);
        }

        info(`enabled, version ${EXTENSION_VERSION}`);
    }

    override disable(): void {
        // A capture still on screen goes first, as Esc would take it but without the fade:
        // its grab, its actors and a countdown's timer (D135). Its flow hears a cancel and
        // unwinds after this returns, into objects that are already gone, which a cancel
        // does not touch. One that had hidden its chrome to read the screen is dropped
        // instead: the extension turning off is no moment to read it (`later.ts`).
        closeOverlays();
        closeWindowPickers();
        activeCountdown()?.destroy();
        forgetRedrawWaits();

        // First of the rest: the pets hold a grab while one is dragged, and a menu's modal.
        this.#crew?.destroy();
        this.#crew = null;
        forgetPetEvents();

        this.#indicator?.destroy();
        this.#indicator = null;

        this.#keybindings?.destroy();
        this.#keybindings = null;

        this.#service?.destroy();
        this.#service = null;

        // Before the desktop icons: the coordinator's teardown restores whatever a
        // recording hid and removes the frame (`spec/06` §9 item 10 for the disable case).
        this.#recording?.destroy();
        this.#recording = null;

        // After the service and the coordinator, which could still have asked it something;
        // puts back whatever was hidden.
        this.#desktopIcons?.destroy();
        this.#desktopIcons = null;

        // Dropped last: the service, the indicator and the recorder all hold a reference.
        if (this.#debugLogChanged !== 0) this.#settings?.raw.disconnect(this.#debugLogChanged);
        this.#debugLogChanged = 0;
        this.#settings = null;
        this.#busy = false;

        info('disabled');
        // Nothing of the extension's stays in module scope once it is off (D135).
        forgetIcons();
        forgetLog();
    }

    /**
     * Starts a capture from a hotkey or the panel menu.
     *
     * The `busy` guard is here as well as in `ShellService` because these two entry
     * points do not go through each other: a hotkey pressed while the overlay is up would
     * otherwise open a second overlay, and the second one's modal grab would fight the
     * first for input with no way back for the user.
     */
    async #capture(mode: CaptureModeName, options: CaptureOptions): Promise<void> {
        const settings = this.#settings;
        if (settings === null) return;
        if (this.#busy) {
            info(`ignoring ${mode}: a capture is already in progress`);
            return;
        }

        this.#busy = true;
        const started = GLib.get_monotonic_time();
        // Filled if the All-In-One toolbar ends on Record or Scroll (`handoff.ts`).
        const handoff = new Handoff();
        try {
            // A null result means the user cancelled, which is not a failure.
            await runCapture(mode, settings, {
                ...options,
                desktopIcons: this.#desktopIcons ?? undefined,
                onRecord: handoff.onRecord,
                onScrolling: handoff.onScrolling,
            });
            // Under the same `#busy`, so no second overlay can open between the two.
            await handoff.run(settings, this.#desktopIcons, this.#recording);
        } catch (e) {
            error(`${mode} capture failed`, e);
            tellPets({ kind: 'failed' });
        } finally {
            this.#busy = false;
            // `spec/10` §7 caps a single extension callback at 2 ms. The await above is
            // not a callback -- it spans the user's whole selection -- so what is logged
            // here is the round trip, and the per-phase timings the flow itself reports
            // are the ones the budget applies to.
            const elapsedMs = (GLib.get_monotonic_time() - started) / 1000;
            info(`${mode} finished in ${elapsedMs.toFixed(0)} ms`);
        }
    }

    /**
     * Starts a GIF recording from a hotkey or the panel menu (`spec/06` §3).
     *
     * Two guards, for two different clashes. `#busy` is the same one `#capture` uses: the
     * record overlay takes a modal grab, so a capture or a second record overlay opening
     * over it would fight for input. `isActive()` is the recorder's own: once a recording
     * is running, a second `record-gif` must not start a rival selection -- Stop first.
     */
    async #record(options: RecordOptions): Promise<void> {
        const settings = this.#settings;
        const desktopIcons = this.#desktopIcons;
        if (settings === null || desktopIcons === null) return;
        if (this.#recording?.isActive() === true) {
            info('ignoring record: a recording is already in progress');
            return;
        }
        if (this.#busy) {
            info('ignoring record: the overlay is already open');
            return;
        }

        this.#busy = true;
        try {
            await runRecording(settings, desktopIcons, options);
        } catch (e) {
            error('recording failed to start', e);
        } finally {
            this.#busy = false;
        }
    }

    /**
     * Starts a scrolling capture from a hotkey or the panel menu (`spec/07` §1.1).
     *
     * Guarded by `#busy` like the others, so a second shortcut cannot open a rival
     * overlay. Nothing guards against a *second scrolling capture*, because the app's own
     * `Scroller` refuses one -- the same division as recording, where the coordinator owns
     * the state and the extension owns the overlay.
     */
    async #scrolling(options: ScrollCaptureOptions): Promise<void> {
        const settings = this.#settings;
        if (settings === null) return;
        if (this.#busy) {
            info('ignoring scrolling capture: the overlay is already open');
            return;
        }

        // With the pill up, the key presses its button instead of opening a second
        // selection that the app would refuse: Start, and then Done. The pill never takes
        // the keyboard, which the page it captures needs (`spec/00` §9, D137).
        if ((this.#service?.scrolling ?? 'none') !== 'none') {
            activateApp('scroll-advance');
            return;
        }

        this.#busy = true;
        try {
            await runScrollingCapture(settings, options);
        } catch (e) {
            error('scrolling capture failed to start', e);
        } finally {
            this.#busy = false;
        }
    }
}
