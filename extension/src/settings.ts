// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Typed access to the extension's own GSettings (`spec/08` §11).
 *
 * The keys read at the moment of a grab live in *this* schema rather than the app's, by
 * `docs/decisions.md` D11: the extension reads them on the compositor's main loop inside
 * `spec/10` §7's 2 ms budget, and `ACT-04`'s shutter has to sound when the pixels are
 * read rather than a D-Bus round trip later. Reading the app's schema from here would
 * also make the extension depend on the app being installed, which is a coupling the
 * compositor half must not have.
 *
 * **Every read checks `has_key` first, and that is not defensive habit.** GLib's
 * `g_settings_get_*` on a key the schema does not have calls `g_error`, which aborts the
 * process -- and this process is `gnome-shell`. Getting it wrong does not break OctoSnap;
 * it logs the user out. The schema and the JavaScript are separate files, GNOME 50
 * reloads neither without a logout, and `install-dev.sh` can leave one newer than the
 * other, so a key that is missing at runtime is a state a working day actually reaches.
 * The app half hit the same trap from the safer side and it is recorded as D11.
 */

import GLib from 'gi://GLib';
import type Gio from 'gi://Gio';

import { error } from './log.js';
import { type ShutterSound, shutterSound } from './soundCues.js';
import { type GifOverrides, gifDefaultsFromCopy } from './recordChoices.js';
import { type PetKind, isPetKind } from './pets/art/index.js';
import { type Liveliness, type Roam, isLiveliness, isRoam } from './pets/brain.js';
import { type PetSize, isPetSize } from './pets/frames.js';
import { type MenuPlacement, isMenuPlacement } from './pets/menuPlace.js';

/** `spec/08` §6's `shot-crosshair`. */
export type CrosshairMode = 'off' | 'always' | 'modifier';

export interface StoredRect {
    x: number;
    y: number;
    width: number;
    height: number;
}

export class Settings {
    #settings: Gio.Settings;
    /** Keys already reported as missing, so one stale schema does not flood the journal. */
    #warned = new Set<string>();

    constructor(settings: Gio.Settings) {
        this.#settings = settings;
    }

    /** SYS-01. */
    get showIndicator(): boolean {
        return this.#boolean('show-indicator', true);
    }

    /** `CAP-11`. */
    get captureCursor(): boolean {
        return this.#boolean('shot-cursor', false);
    }

    /** `CAP-09`. */
    get freezeScreen(): boolean {
        return this.#boolean('shot-freeze', false);
    }

    /** `CAP-12`: hide the desktop icons for the length of a capture. */
    get hideDesktop(): boolean {
        return this.#boolean('shot-hide-desktop', false);
    }

    /** `CAP-10`. */
    get crosshair(): CrosshairMode {
        const value = this.#string('shot-crosshair', 'modifier');
        return value === 'off' || value === 'always' || value === 'modifier'
            ? value
            : 'modifier';
    }

    get magnifier(): boolean {
        return this.#boolean('shot-magnifier', true);
    }

    /** Whether routine lines reach the journal as well as the report (`log.ts`, D135). */
    get debugLog(): boolean {
        return this.#boolean('debug-log', false);
    }

    /** `ACT-04`, and whether the countdown ticks. */
    get shutterSound(): ShutterSound {
        return shutterSound(this.#string('capture-sound', 'classic'));
    }

    /**
     * `CAP-05`'s remembered selection, or `null` when there is none yet.
     *
     * An all-zero rect is the schema's default and means "nothing selected yet", not
     * "selected nothing": `previous-area` falls back to the overlay rather than capturing
     * an empty region.
     */
    get lastArea(): StoredRect | null {
        if (!this.#has('last-area')) return null;
        try {
            const [x, y, width, height] = this.#settings
                .get_value('last-area')
                .deep_unpack() as [number, number, number, number];
            if (width <= 0 || height <= 0) return null;
            return { x, y, width, height };
        } catch (e) {
            error('could not read last-area', e);
            return null;
        }
    }

    get lastDisplay(): string {
        return this.#string('last-display', '');
    }

    /**
     * `spec/03` §1: All-In-One opens on "the last used" mode.
     *
     * Validated against the modes this build can run rather than trusted: a value written
     * by a newer version would otherwise open the toolbar on a mode with nothing behind
     * it.
     */
    get lastMode():
        'area' | 'fullscreen' | 'window' | 'self-timer' | 'record' | 'scrolling' | 'ocr' {
        const value = this.#string('last-mode', 'area');
        return value === 'fullscreen' ||
            value === 'window' ||
            value === 'self-timer' ||
            value === 'record' ||
            value === 'scrolling' ||
            value === 'ocr'
            ? value
            : 'area';
    }

    rememberMode(mode: string): void {
        if (!this.#has('last-mode')) return;
        try {
            this.#settings.set_string('last-mode', mode);
        } catch (e) {
            error('could not remember the last mode', e);
        }
    }

    /**
     * Remembers a selection for `CAP-05` and All-In-One's restore (`spec/03` §1).
     *
     * Failure is logged and swallowed: a capture the user already took must not be
     * reported as failed because the *next* one will not be able to repeat it.
     */
    rememberArea(rect: StoredRect, display: string): void {
        if (!this.#has('last-area')) return;
        try {
            this.#settings.set_value(
                'last-area',
                new GLib.Variant('(iiii)', [rect.x, rect.y, rect.width, rect.height]),
            );
            if (this.#has('last-display')) this.#settings.set_string('last-display', display);
        } catch (e) {
            error('could not remember the last area', e);
        }
    }

    /** `spec/08` §1's self-timer interval (`shot-timer`, D136). */
    get selfTimer(): number {
        return this.#int('shot-timer', 5);
    }

    /**
     * `spec/08` §5: seconds to count down over the live screen before recording starts
     * (`rec-countdown`). The overlay runs this countdown, exactly as self-timer runs
     * `timer` -- so a recording is arranged the same way a delayed capture is.
     */
    get recordCountdown(): number {
        return this.#int('rec-countdown', 3);
    }

    /**
     * `REC-08`: hide the desktop icons for the length of a recording. Defaults on, unlike
     * a capture's `shot-hide-desktop`, because a recording lingers and a cluttered desktop
     * shows for its whole length rather than a single frame.
     */
    get hideDesktopWhileRecording(): boolean {
        return this.#boolean('rec-hide-desktop', true);
    }

    /**
     * The last area recorded, or `null` when there is none yet. Same all-zero-means-none
     * rule as `lastArea`: the record overlay opens on it when it exists and from an empty
     * selection otherwise.
     */
    get lastRecordArea(): StoredRect | null {
        if (!this.#has('last-record-area')) return null;
        try {
            const [x, y, width, height] = this.#settings
                .get_value('last-record-area')
                .deep_unpack() as [number, number, number, number];
            if (width <= 0 || height <= 0) return null;
            return { x, y, width, height };
        } catch (e) {
            error('could not read last-record-area', e);
            return null;
        }
    }

    /** Remembers the recorded area, so the next recording opens on it. Never throws. */
    rememberRecordArea(rect: StoredRect): void {
        if (!this.#has('last-record-area')) return;
        try {
            this.#settings.set_value(
                'last-record-area',
                new GLib.Variant('(iiii)', [rect.x, rect.y, rect.width, rect.height]),
            );
        } catch (e) {
            error('could not remember the last recorded area', e);
        }
    }

    /**
     * The app's GIF defaults as its copy here has them (D125), for the recording row to
     * prefer over the app's own schema, which a sandboxed app's is not. Empty until the
     * app has run, and never throws.
     */
    get appGifDefaults(): GifOverrides {
        if (!this.#has('app-gif-defaults')) return {};
        try {
            const copy = this.#settings.get_value('app-gif-defaults').recursiveUnpack() as Record<string, unknown>;
            return gifDefaultsFromCopy(copy);
        } catch (e) {
            error('could not read app-gif-defaults', e);
            return {};
        }
    }

    // --- spec/14's pets ----------------------------------------------------------

    get petsEnabled(): boolean {
        return this.#boolean('pets-enabled', false);
    }

    setPetsEnabled(on: boolean): void {
        if (!this.#has('pets-enabled')) return;
        try {
            this.#settings.set_boolean('pets-enabled', on);
        } catch (e) {
            error('could not switch the pets', e);
        }
    }

    /** Which pets are out, known kinds only, each once, in the stored order. */
    get pets(): PetKind[] {
        if (!this.#has('pets')) return ['octopus'];
        try {
            const kinds: PetKind[] = [];
            for (const kind of this.#settings.get_strv('pets'))
                if (isPetKind(kind) && !kinds.includes(kind)) kinds.push(kind);
            return kinds;
        } catch (e) {
            error("could not read 'pets'", e);
            return ['octopus'];
        }
    }

    get petSize(): PetSize {
        const value = this.#string('pet-size', 'medium');
        return isPetSize(value) ? value : 'medium';
    }

    get petMenu(): MenuPlacement {
        const value = this.#string('pet-menu', 'auto');
        return isMenuPlacement(value) ? value : 'auto';
    }

    /** "Move on their own": false keeps each pet where it was put. */
    get petWander(): boolean {
        return this.#boolean('pet-wander', true);
    }

    /** Where the pets can be: along the bottom only, or anywhere on the screen (`spec/14` §4). */
    get petRoam(): Roam {
        const value = this.#string('pet-roam', 'floor');
        return isRoam(value) ? value : 'floor';
    }

    get petActivity(): Liveliness {
        const value = this.#string('pet-activity', 'normal');
        return isLiveliness(value) ? value : 'normal';
    }

    get petHideSharing(): boolean {
        return this.#boolean('pet-hide-sharing', true);
    }

    /** Each pet's last place: its monitor's index, and how far along that work area. */
    get petPlaces(): Record<string, [number, number]> {
        if (!this.#has('pet-places')) return {};
        try {
            return this.#settings.get_value('pet-places').deepUnpack() as Record<string, [number, number]>;
        } catch (e) {
            error("could not read 'pet-places'", e);
            return {};
        }
    }

    /**
     * Each pet's last spot: its monitor's index, how far along and how far down that work
     * area its middle and its feet are, and whether it was on the desk (`spec/14` §4).
     * `pet-places` before it, which had no height, is read for a pet with no spot yet.
     */
    get petSpots(): Record<string, [number, number, number, boolean]> {
        const spots: Record<string, [number, number, number, boolean]> = {};
        for (const [kind, [index, along]] of Object.entries(this.petPlaces)) spots[kind] = [index, along, 1, false];
        if (!this.#has('pet-spots')) return spots;
        try {
            const saved = this.#settings.get_value('pet-spots').deepUnpack() as Record<string, [number, number, number, boolean]>;
            return { ...spots, ...saved };
        } catch (e) {
            error("could not read 'pet-spots'", e);
            return spots;
        }
    }

    rememberPetSpots(spots: Record<string, [number, number, number, boolean]>): void {
        if (!this.#has('pet-spots')) return;
        try {
            this.#settings.set_value('pet-spots', new GLib.Variant('a{s(iddb)}', spots));
        } catch (e) {
            error('could not remember where the pets were', e);
        }
    }

    /** The raw settings, for `Keybindings`, which needs the object rather than a value. */
    get raw(): Gio.Settings {
        return this.#settings;
    }

    /** True when this key exists in the *installed* schema. See the module comment. */
    #has(key: string): boolean {
        // `settings_schema` is the schema this object was built from, so this asks the
        // installed file rather than what the source tree believes.
        if (this.#settings.settings_schema?.has_key(key)) return true;
        if (!this.#warned.has(key)) {
            this.#warned.add(key);
            error(
                `the installed schema has no key '${key}'; using the built-in default. ` +
                'The extension\'s schema is older than its code -- reinstall and log out.',
            );
        }
        return false;
    }

    #boolean(key: string, fallback: boolean): boolean {
        if (!this.#has(key)) return fallback;
        try {
            return this.#settings.get_boolean(key);
        } catch (e) {
            error(`could not read '${key}'`, e);
            return fallback;
        }
    }

    #string(key: string, fallback: string): string {
        if (!this.#has(key)) return fallback;
        try {
            return this.#settings.get_string(key);
        } catch (e) {
            error(`could not read '${key}'`, e);
            return fallback;
        }
    }

    #int(key: string, fallback: number): number {
        if (!this.#has(key)) return fallback;
        try {
            return this.#settings.get_int(key);
        } catch (e) {
            error(`could not read '${key}'`, e);
            return fallback;
        }
    }
}
