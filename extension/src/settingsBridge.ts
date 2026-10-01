// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The extension's settings, and GNOME's three Print keys, for an app in a sandbox (D123).
 *
 * A Flatpak app has no dconf of the host's: its GSettings are a key file inside the
 * sandbox, which GNOME Shell never reads, and the extension's schema is not installed
 * where it could find one anyway. Everything the app's Settings dialog shows about the
 * shell half -- the shortcuts, the capture switches, the Print-key takeover, the conflict
 * check against GNOME's own bindings -- would be dead. So the extension, which reads and
 * writes these through GNOME Shell's own dconf, answers for them over its interface, and
 * the app keeps a mirror (`crates/app/src/remote_settings.rs`).
 *
 * What may be read and written is a short fixed list, decided here and not by the caller:
 * all of this extension's own schema; the three GNOME keys `spec/08` §3's takeover
 * rebinds; and, for reading only, the keybinding schemas the conflict check compares
 * against.
 */

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import { error } from './log.js';

export const OWN_SCHEMA = 'org.gnome.shell.extensions.octosnap';
export const SHELL_KEYBINDINGS = 'org.gnome.shell.keybindings';
const WM_KEYBINDINGS = 'org.gnome.desktop.wm.keybindings';
const MEDIA_KEYS = 'org.gnome.settings-daemon.plugins.media-keys';
const CUSTOM_KEYBINDING = `${MEDIA_KEYS}.custom-keybinding`;

/** `crates/core/src/shortcuts.rs`'s `PRINT_KEYS`, GNOME's side of them. */
export const PRINT_KEYS = ['show-screenshot-ui', 'screenshot', 'screenshot-window'];

/** Schemas a caller may read whole. */
export function readable(schema: string): boolean {
    return schema === OWN_SCHEMA || schema === SHELL_KEYBINDINGS;
}

/** Whether a caller may write `key` of `schema`. */
export function writable(schema: string, key: string): boolean {
    if (schema === OWN_SCHEMA) return true;
    return schema === SHELL_KEYBINDINGS && PRINT_KEYS.includes(key);
}

/** The keys of `schema` a mirror of it is told about when they change. */
export function watched(schema: string, key: string): boolean {
    return schema === OWN_SCHEMA || (schema === SHELL_KEYBINDINGS && PRINT_KEYS.includes(key));
}

type Emit = (schema: string, key: string, value: GLib.Variant) => void;

export class SettingsBridge {
    /** Every schema opened so far, the extension's own first: the object it already has. */
    #opened = new Map<string, Gio.Settings>();
    #handlers: [Gio.Settings, number][] = [];

    constructor(own: Gio.Settings) {
        this.#opened.set(OWN_SCHEMA, own);
    }

    /**
     * A schema's settings, or `null` when it is not installed. Looked up before it is
     * opened, because `new Gio.Settings` on a missing schema aborts gnome-shell.
     */
    #open(schema: string): Gio.Settings | null {
        const known = this.#opened.get(schema);
        if (known !== undefined) return known;
        const found = Gio.SettingsSchemaSource.get_default()?.lookup(schema, true) ?? null;
        if (found === null) return null;
        const settings = new Gio.Settings({ settings_schema: found });
        this.#opened.set(schema, settings);
        return settings;
    }

    /** Tells `emit` about every change a mirror follows, until `destroy`. */
    watch(emit: Emit): void {
        for (const schema of [OWN_SCHEMA, SHELL_KEYBINDINGS]) {
            const settings = this.#open(schema);
            if (settings === null) continue;
            const id = settings.connect('changed', (_settings: Gio.Settings, key: string) => {
                if (!watched(schema, key)) return;
                try {
                    emit(schema, key, settings.get_value(key));
                } catch (e) {
                    error(`could not report a change to ${schema} ${key}`, e);
                }
            });
            this.#handlers.push([settings, id]);
            // GSettings only emits `changed` for a key that has been read while a handler
            // was connected, so each one the mirror follows is read once, here.
            for (const key of settings.settings_schema.list_keys()) {
                if (watched(schema, key))
                    settings.get_value(key);
            }
        }
    }

    destroy(): void {
        for (const [settings, id] of this.#handlers)
            settings.disconnect(id);
        this.#handlers = [];
    }

    /** Every key of `schema` and its value. Throws for a schema a caller may not read. */
    values(schema: string): Record<string, GLib.Variant> {
        if (!readable(schema)) throw new Error(`${schema} is not shared`);
        const settings = this.#open(schema);
        if (settings === null) throw new Error(`${schema} is not installed`);
        const out: Record<string, GLib.Variant> = {};
        for (const key of settings.settings_schema.list_keys()) {
            if (schema === SHELL_KEYBINDINGS && !PRINT_KEYS.includes(key)) continue;
            out[key] = settings.get_value(key);
        }
        return out;
    }

    /** Writes one key, after checking it may be written and that the value fits it. */
    set(schema: string, key: string, value: GLib.Variant): void {
        const settings = this.#writableSettings(schema, key);
        const schemaKey = settings.settings_schema.get_key(key);
        const expected = schemaKey.get_value_type().dup_string();
        if (value.get_type_string() !== expected)
            throw new Error(`${key} is ${expected}, not ${value.get_type_string()}`);
        if (!schemaKey.range_check(value))
            throw new Error(`${value.print(true)} is out of range for ${key}`);
        if (!settings.set_value(key, value))
            throw new Error(`${schema} ${key} is not writable here`);
    }

    /** Puts one key back to its schema's default. */
    reset(schema: string, key: string): void {
        this.#writableSettings(schema, key).reset(key);
    }

    #writableSettings(schema: string, key: string): Gio.Settings {
        if (!writable(schema, key)) throw new Error(`${schema} ${key} is not the app's to write`);
        const settings = this.#open(schema);
        if (settings === null) throw new Error(`${schema} is not installed`);
        if (!settings.settings_schema.has_key(key)) throw new Error(`${schema} has no key ${key}`);
        return settings;
    }

    /**
     * Every keybinding a new shortcut could collide with, as `(schema, key, accelerators)`:
     * GNOME Shell's, the window manager's, the media keys' fixed and custom ones. What the
     * app's conflict check reads natively from dconf (`prefs.rs`'s `known_bindings`).
     */
    keybindings(): [string, string, string[]][] {
        const out: [string, string, string[]][] = [];
        for (const schema of [SHELL_KEYBINDINGS, WM_KEYBINDINGS, MEDIA_KEYS]) {
            const settings = this.#open(schema);
            if (settings === null) continue;
            for (const key of settings.settings_schema.list_keys()) {
                if (settings.settings_schema.get_key(key).get_value_type().dup_string() !== 'as')
                    continue;
                out.push([schema, key, settings.get_strv(key)]);
            }
        }
        const media = this.#open(MEDIA_KEYS);
        const custom = Gio.SettingsSchemaSource.get_default()?.lookup(CUSTOM_KEYBINDING, true) ?? null;
        if (media !== null && custom !== null && media.settings_schema.has_key('custom-keybindings')) {
            for (const path of media.get_strv('custom-keybindings')) {
                try {
                    const one = new Gio.Settings({ settings_schema: custom, path });
                    out.push([CUSTOM_KEYBINDING, one.get_string('name'), [one.get_string('binding')]]);
                } catch (e) {
                    error(`could not read the custom keybinding at ${path}`, e);
                }
            }
        }
        return out;
    }
}
