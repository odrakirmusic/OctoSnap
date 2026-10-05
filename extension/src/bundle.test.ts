// SPDX-License-Identifier: GPL-3.0-or-later

import { readdirSync, readFileSync } from 'node:fs';

import { describe, expect, it } from 'vitest';

// D160: the Flatpak builds the extension it carries with TypeScript from its own manifest,
// and the zip is built with the TypeScript in node_modules. They are one compiler only while
// the manifest follows package-lock.json.
describe('the extension the Flatpak carries', () => {
    const read = (path: string) => JSON.parse(readFileSync(new URL(path, import.meta.url), 'utf8'));

    it('is compiled by the TypeScript package-lock.json pins', () => {
        const locked = read('../package-lock.json').packages['node_modules/typescript'];
        const manifest = read('../../build-aux/flatpak/io.github.odrakirmusic.OctoSnap.json');
        const app = manifest.modules.find((m: { name?: string }) => m.name === 'octosnap');
        const source = app.sources.find((s: { dest?: string }) => s.dest === 'extension/node_modules/typescript');
        expect(source.url).toBe(locked.resolved);
        // The lock's integrity is the tarball's sha512 in base64; the manifest's, in hex.
        expect(`sha512-${Buffer.from(source.sha512, 'hex').toString('base64')}`).toBe(locked.integrity);
        expect(app['config-opts']).toContain('-Dbundle-extension=true');
    });

    // D161: the app writes an update that changes only the code under the copy GNOME Shell
    // is running, because GNOME Shell imports the code once, at login, and keeps it. A module
    // imported later would be read from whatever is on disk by then.
    it('imports every module at login, none of them later', () => {
        const src = new URL('.', import.meta.url);
        const sources = (readdirSync(src, { recursive: true }) as string[])
            .filter(path => path.endsWith('.ts') && !path.endsWith('.test.ts') && !path.endsWith('.d.ts'));
        expect(sources.length).toBeGreaterThan(40);
        for (const path of sources) {
            const text = readFileSync(new URL(path, src), 'utf8');
            expect(text, path).not.toMatch(/\bimport\s*\(/);
            expect(text, path).not.toMatch(/\bimports\.[a-z]/);
        }
    });
});
