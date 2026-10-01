// SPDX-License-Identifier: GPL-3.0-or-later

import GLib from 'gi://GLib';
import Meta from 'gi://Meta';

import { looksLikeDesktopIcons } from './desktop-match.js';
import { info } from './log.js';

/**
 * `DSK-01` and `CAP-12`: the desktop icons, hidden and put back.
 *
 * `spec/07` §5: "the shell extension hides DING's window actor (or disables DING's icon
 * rendering via its settings) and restores it afterwards". The actor, not the settings,
 * and not DING's enabled state, because of `spec/07` §7 item 7 -- "restored even if the
 * app crashes (extension watchdog)". A hidden actor is not persisted anywhere: if this
 * extension is disabled it shows the actors again in `destroy`, and if the shell itself
 * goes down the next one maps DING's windows visible. A `gsettings` write or a
 * `disableExtension` would survive both, and a crash between hide and restore would leave
 * the user with no desktop icons and no idea why.
 *
 * Mutter re-shows a window actor on its own occasions -- a workspace switch, a stacking
 * change -- so while the icons are meant to be hidden each actor's `visible` is watched
 * and put back, and a desktop window created later (a monitor plugged in) is hidden as it
 * arrives. The app is never asked about any of this: the state is the extension's,
 * reported over the bus when asked.
 */
export class DesktopIcons {
    #hidden = false;
    /** Hidden by a capture rather than by the user, so only a capture restores it. */
    #forCapture = false;
    /** Hidden for a recording, so only its Stop (or the watchdog) restores it. */
    #forRecording = false;
    #createdId = 0;
    /** The idle below, while one is waiting, so `destroy` can remove it (D135). */
    #applyIdle = 0;
    #watched = new Map<Meta.WindowActor, number>();

    constructor() {
        this.#createdId = global.display.connect('window-created', () => {
            if (!this.#hidden || this.#applyIdle !== 0) return;
            // The actor exists a moment after the window does; hide it once it does.
            this.#applyIdle = GLib.idle_add(GLib.PRIORITY_DEFAULT, () => {
                this.#applyIdle = 0;
                this.#apply();
                return GLib.SOURCE_REMOVE;
            });
        });
    }

    get hidden(): boolean {
        return this.#hidden;
    }

    /** Whether there are desktop icons on this session at all. */
    available(): boolean {
        return this.#actors().length > 0;
    }

    /** `toggle`, `hide` or `show`, as the panel menu, the shortcut and the app spell it. */
    apply(what: string): boolean {
        switch (what) {
            case 'hide':
                return this.hide();
            case 'show':
                return this.show();
            default:
                return this.#hidden ? this.show() : this.hide();
        }
    }

    hide(): boolean {
        // Nothing to hide is not a hidden state: a session without desktop icons must
        // not remember a "hidden" it cannot show, and the caller is told `available`.
        if (!this.available()) {
            this.#describeWindows();
            return false;
        }
        this.#hidden = true;
        this.#forCapture = false;
        this.#forRecording = false;
        this.#apply();
        return true;
    }

    show(): boolean {
        this.#hidden = false;
        this.#forCapture = false;
        this.#forRecording = false;
        this.#apply();
        return false;
    }

    /** `CAP-12`: hidden for the length of a capture when the setting says so. */
    beginCapture(wanted: boolean): void {
        if (!wanted || this.#hidden) return;
        this.#hidden = true;
        this.#forCapture = true;
        this.#apply();
    }

    /** Put back after the capture -- only what the capture hid, never a user's choice. */
    endCapture(): void {
        if (!this.#forCapture) return;
        this.#hidden = false;
        this.#forCapture = false;
        this.#apply();
    }

    /**
     * `REC-08`: hidden for the length of a recording when the setting says so.
     *
     * Symmetric to `beginCapture` but with its own flag, because a recording's hide and
     * restore are seconds or minutes apart and driven by the recording state rather than a
     * `finally` -- and because the two must not clear each other's intent.
     */
    beginRecording(wanted: boolean): void {
        if (!wanted || this.#hidden) return;
        this.#hidden = true;
        this.#forRecording = true;
        this.#apply();
    }

    /**
     * Put back after the recording -- only what the recording hid. Called on Stop and by
     * the extension's watchdog when the app leaves the bus mid-recording (`spec/07` §7
     * item 7, `spec/06` §9 item 10).
     */
    endRecording(): void {
        if (!this.#forRecording) return;
        this.#hidden = false;
        this.#forRecording = false;
        this.#apply();
    }

    /** The watchdog's other half: whatever is hidden comes back when the extension goes. */
    destroy(): void {
        if (this.#hidden) {
            this.#hidden = false;
            this.#forCapture = false;
            this.#forRecording = false;
            this.#apply();
        }
        for (const [actor, id] of this.#watched) actor.disconnect(id);
        this.#watched.clear();
        if (this.#createdId !== 0) {
            global.display.disconnect(this.#createdId);
            this.#createdId = 0;
        }
        if (this.#applyIdle !== 0) {
            GLib.source_remove(this.#applyIdle);
            this.#applyIdle = 0;
        }
    }

    #actors(): Meta.WindowActor[] {
        return global.get_window_actors().filter(actor => {
            const window = actor.get_meta_window();
            if (!window) return false;
            return looksLikeDesktopIcons(
                window.get_title(),
                window.get_window_type() === Meta.WindowType.DESKTOP,
            );
        });
    }

    /** What the stage holds when no desktop window was found, so the log says why. */
    #describeWindows(): void {
        const seen = global.get_window_actors().map(actor => {
            const window = actor.get_meta_window();
            if (!window) return '(no window)';
            return `${window.get_title() ?? '(untitled)'}/${window.get_wm_class() ?? '?'}/${window.get_window_type()}`;
        });
        info(`desktop icons: no desktop window among ${seen.length}: ${seen.join(', ') || '(none)'}`);
    }

    #apply(): void {
        const actors = this.#actors();
        for (const actor of actors) {
            if (this.#hidden) {
                actor.hide();
                this.#watch(actor);
            } else {
                actor.show();
            }
        }
        info(
            `desktop icons ${this.#hidden ? 'hidden' : 'shown'}: ${actors.length} window(s)` +
                (this.#forCapture ? ' for a capture' : ''),
        );
    }

    #watch(actor: Meta.WindowActor): void {
        if (this.#watched.has(actor)) return;
        const id = actor.connect('notify::visible', () => {
            if (this.#hidden && actor.visible) actor.hide();
        });
        this.#watched.set(actor, id);
        actor.connect('destroy', () => this.#watched.delete(actor));
    }
}
