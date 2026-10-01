// SPDX-License-Identifier: GPL-3.0-or-later

import { describe, expect, it, vi } from 'vitest';

// The bridge class talks to GSettings; the rules beside it do not, and are what is tested.
vi.mock('gi://Gio', () => ({ default: {} }));
vi.mock('gi://GLib', () => ({ default: {} }));

const { OWN_SCHEMA, PRINT_KEYS, SHELL_KEYBINDINGS, readable, watched, writable } = await import('./settingsBridge.js');

describe('what the settings bridge shares', () => {
    it('shares the extension\'s own schema whole', () => {
        expect(readable(OWN_SCHEMA)).toBe(true);
        expect(writable(OWN_SCHEMA, 'capture-area')).toBe(true);
        expect(watched(OWN_SCHEMA, 'shot-cursor')).toBe(true);
    });

    it('writes only the three Print keys of GNOME Shell\'s', () => {
        for (const key of PRINT_KEYS)
            expect(writable(SHELL_KEYBINDINGS, key)).toBe(true);
        expect(writable(SHELL_KEYBINDINGS, 'toggle-overview')).toBe(false);
        expect(watched(SHELL_KEYBINDINGS, 'toggle-overview')).toBe(false);
    });

    it('shares nothing else', () => {
        expect(readable('org.gnome.desktop.wm.keybindings')).toBe(false);
        expect(writable('org.gnome.desktop.wm.keybindings', 'close')).toBe(false);
        expect(writable('org.gnome.shell', 'enabled-extensions')).toBe(false);
        expect(readable('org.gnome.shell')).toBe(false);
    });
});
