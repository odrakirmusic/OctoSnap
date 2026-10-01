// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The app's GIF defaults, read for *display* (D101).
 *
 * The All-In-One toolbar's recording row shows what the recording will be made with, and
 * those numbers live in the app's schema, not the extension's -- D11 put the keys read at
 * the moment of a grab in the extension's own schema, and these are not those: nothing
 * here is read on a capture's hot path, and nothing here changes what the app does. A
 * value shown and left alone is never sent, so the app records with its own setting; a
 * value the user changes travels as an override. If the schema is not installed, the row
 * shows `spec/08` §5's defaults and the overrides still work.
 *
 * `Gio.Settings.new` on a schema this process cannot find aborts *gnome-shell*, so the
 * schema is looked up first and every key is checked before it is read -- the same rule
 * `settings.ts` follows, for the same reason.
 *
 * A Flatpak app's schema is never where this process can find it, and its values live in
 * a key file inside the sandbox anyway. So the app also keeps a copy of the four in this
 * extension's `app-gif-defaults` (D125), and a value in the copy wins: it is what the app
 * last saw, wherever the app runs. The app's schema covers an app that has not written
 * the copy yet, and `spec/08` §5 covers the rest.
 */

import Gio from 'gi://Gio';

import { type GifOverrides } from './recordChoices.js';
import { error } from './log.js';

const APP_SCHEMA = 'io.github.odrakirmusic.OctoSnap';

/** `spec/08` §5: 15 fps, 1280 px, quality 80, cursor shown. */
const FALLBACK: Required<GifOverrides> = { fps: 15, maxWidth: 1280, quality: 80, cursor: true };

/** `copy` is `Settings.appGifDefaults`: the app's copy, which wins where it has a value. */
export function readGifDefaults(copy: GifOverrides = {}): Required<GifOverrides> {
    return { ...fromAppSchema(), ...copy };
}

function fromAppSchema(): Required<GifOverrides> {
    let settings: Gio.Settings;
    try {
        const source = Gio.SettingsSchemaSource.get_default();
        const schema = source?.lookup(APP_SCHEMA, true) ?? null;
        if (schema === null) return { ...FALLBACK };
        settings = new Gio.Settings({ settings_schema: schema });
    } catch (e) {
        error('could not open the app\'s settings for the recording row', e);
        return { ...FALLBACK };
    }
    const has = (key: string): boolean => settings.settings_schema.has_key(key);
    return {
        fps: has('gif-fps') ? settings.get_int('gif-fps') : FALLBACK.fps,
        maxWidth: has('gif-max-width') ? settings.get_int('gif-max-width') : FALLBACK.maxWidth,
        quality: has('gif-quality') ? settings.get_int('gif-quality') : FALLBACK.quality,
        cursor: has('rec-cursor') ? settings.get_boolean('rec-cursor') : FALLBACK.cursor,
    };
}
