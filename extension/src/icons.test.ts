// SPDX-License-Identifier: GPL-3.0-or-later

import { existsSync, readdirSync, readFileSync } from 'node:fs';

import { describe, expect, it } from 'vitest';

const SETS = [
    new URL('../icons/', import.meta.url),
    new URL('../../crates/app/resources/icons/scalable/actions/', import.meta.url),
];

function svgs(dir: URL): [string, string][] {
    return readdirSync(dir)
        .filter(name => name.endsWith('.svg'))
        .map(name => [name, readFileSync(new URL(name, dir), 'utf8')]);
}

describe("OctoSnap's symbolic icons", () => {
    // GTK's and St's symbolic loaders recolour `fill` and nothing else. A stroke stays
    // black, which on a dark panel or in dark mode is an icon that is not there.
    it('are fills of the foreground colour, never strokes', () => {
        for (const dir of SETS) {
            for (const [name, svg] of svgs(dir)) {
                expect(name, name).toMatch(/-symbolic\.svg$/);
                expect(svg, name).toContain('viewBox="0 0 16 16"');
                expect(svg, name).not.toMatch(/stroke/);
                for (const fill of svg.matchAll(/fill="([^"]*)"/g)) expect(fill[1], name).toBe('currentColor');
            }
        }
    });

    it('include every mode icon the toolbar names', () => {
        const toolbar = readFileSync(new URL('./overlay/toolbar.ts', import.meta.url), 'utf8');
        const named = [...toolbar.matchAll(/icon: '(mode-[a-z-]+)'/g)].map(m => m[1]);
        expect(named.length).toBeGreaterThan(0);
        for (const name of named) expect(existsSync(new URL(`../icons/${name}.svg`, import.meta.url)), name).toBe(true);
    });
});
