// SPDX-License-Identifier: GPL-3.0-or-later

import { readFileSync } from 'node:fs';

import { describe, expect, it } from 'vitest';

import { EXTENSION_VERSION } from './protocol.js';

describe('the extension version', () => {
    // The app compares the version the running extension reports with the one GNOME Shell
    // reads from metadata.json, and a difference means "log out to finish updating"
    // (crates/app/src/setup.rs). Two spellings of one number that drifted apart would say
    // that forever.
    it('is the same in metadata.json and protocol.ts', () => {
        const metadata = JSON.parse(readFileSync(new URL('../metadata.json', import.meta.url), 'utf8'));
        expect(metadata['version-name']).toBe(EXTENSION_VERSION);
    });
});
