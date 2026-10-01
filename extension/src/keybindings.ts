// SPDX-License-Identifier: GPL-3.0-or-later

import type Gio from 'gi://Gio';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import { error, info } from './log.js';

/**
 * Thin owner for `Main.wm.addKeybinding`, whose contract is that every binding added on
 * enable is removed on disable (`spec/10` §2). Keeping the added names in one place is
 * what makes that guarantee cheap to honour.
 *
 * `Shell.ActionMode.ALL` is deliberate: `spec/01` §2 row 1 requires hotkeys to fire even
 * over a fullscreen application, which is the whole reason capture lives in an extension.
 */
export class Keybindings {
    #settings: Gio.Settings;
    #added: string[] = [];

    constructor(settings: Gio.Settings) {
        this.#settings = settings;
    }

    add(name: string, handler: () => void): void {
        // A key the installed schema does not have would abort the shell inside
        // `get_strv`. The two halves ship separately, so it is a line and a skipped binding.
        if (!this.#settings.settings_schema.has_key(name)) {
            error(`keybinding '${name}' is not in the installed schema; skipped`);
            return;
        }
        const ok = Main.wm.addKeybinding(
            name,
            this.#settings,
            Meta.KeyBindingFlags.IGNORE_AUTOREPEAT,
            Shell.ActionMode.ALL,
            handler,
        );
        if (ok === Meta.KeyBindingAction.NONE) {
            error(`keybinding '${name}' was rejected, most likely a conflict`);
            return;
        }
        this.#added.push(name);
        info(`keybinding '${name}' bound to ${this.#settings.get_strv(name).join(', ') || '(unset)'}`);
    }

    destroy(): void {
        for (const name of this.#added) {
            try {
                Main.wm.removeKeybinding(name);
            } catch (e) {
                error(`failed to remove keybinding '${name}'`, e);
            }
        }
        this.#added = [];
    }
}
