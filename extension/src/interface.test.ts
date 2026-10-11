// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The two halves' contract, read from both sides' sources: every method the app calls on
 * the extension (`crates/shell/src/gnome.rs`) is one the extension's interface declares
 * (`dbus.ts`). A name spelt differently on one side fails every call at run time with
 * `UnknownMethod`, which the app takes for an older extension and says nothing about --
 * D170's `SetUpdateOffered` would have been an item that never appeared.
 *
 * Read as text: `dbus.ts` imports gi modules as values and cannot be loaded under Node.
 */

import { readFileSync } from 'node:fs';

import { describe, expect, it } from 'vitest';

const read = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8');

const declared = new Set([...read('./dbus.ts').matchAll(/<method name="(\w+)">/g)].map(m => m[1]));
const called = new Set([
    ...[...read('../../crates/shell/src/gnome.rs').matchAll(/\.call(?:_with_timeout)?\(\s*"(\w+)"/g)].map(m => m[1]),
    // The pet packs' calls, which Settings makes itself (D196).
    ...[...read('../../crates/app/src/pets.rs').matchAll(/SHELL_INTERFACE,\s*"(\w+)"/g)].map(m => m[1]),
]);

describe('the shell interface', () => {
    it('is read from both sides', () => {
        expect(declared.size).toBeGreaterThan(20);
        expect(called.size).toBeGreaterThan(20);
    });

    it('declares every method the app calls', () => {
        expect([...called].filter(name => !declared.has(name))).toEqual([]);
    });

    it('has the pet packs D196 added', () => {
        for (const name of ['GetPetPacks', 'OpenPetFolder']) {
            expect(declared.has(name), name).toBe(true);
            expect(called.has(name), name).toBe(true);
        }
        expect(read('./dbus.ts')).toContain('<signal name="PetPacksChanged"/>');
        expect(read('../../crates/app/src/pets.rs')).toContain('Some("PetPacksChanged")');
    });

    it('has the update item D170 added', () => {
        expect(declared.has('SetUpdateOffered')).toBe(true);
        expect(called.has('SetUpdateOffered')).toBe(true);
    });
});
