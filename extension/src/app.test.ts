// SPDX-License-Identifier: GPL-3.0-or-later

import { readFileSync } from 'node:fs';

import { describe, expect, it, vi } from 'vitest';

// The dictionary is built from `GLib.Variant`s; a stand-in that remembers each type is
// enough to see which keys go out and as what.
vi.mock('gi://GLib', () => {
    class Variant {
        constructor(public type: string, public value: unknown) {}
        static new_string(value: string) { return new Variant('s', value); }
        static new_double(value: number) { return new Variant('d', value); }
        static new_boolean(value: boolean) { return new Variant('b', value); }
        static new_uint64(value: number) { return new Variant('t', value); }
        static new_uint32(value: number) { return new Variant('u', value); }
    }
    return { default: { Variant } };
});
vi.mock('gi://Gio', () => ({ default: {} }));

const { metaToDict } = await import('./app.js');
import type { CaptureMeta } from './spool.js';

/** Every field set, so every optional key the extension can send is sent. */
const FULL: CaptureMeta = {
    path: '/spool/01.png',
    meta_path: '/spool/01.json',
    mode: 'window',
    rect: { x: 1, y: 2, width: 3, height: 4 },
    scale: 2,
    display: 'eDP-1',
    cursor_rect: { x: 5, y: 6, width: 7, height: 8 },
    app_id: 'org.gnome.TextEditor',
    app_name: 'Text Editor',
    window_title: 'notes',
    window_alpha: true,
    timestamp: 1_790_000_000_000_000,
    confirmed_at: 1_790_000_000_000_001,
    animation_ms: 400,
    requested_action: 'copy',
    modifiers: 1,
    linebreaks: false,
};

/** The keys `crates/shell/src/capture.rs`'s `from_variant` reads, from its own source. */
function keysTheAppReads(): string[] {
    const source = readFileSync(new URL('../../crates/shell/src/capture.rs', import.meta.url), 'utf8');
    const start = source.indexOf('pub fn from_variant(');
    const body = source.slice(start, source.indexOf('\n}\n', start));
    return [...body.matchAll(/variant::[a-z0-9_]+\(&dict, "([a-z_]+)"\)/g)].map(m => m[1]!).sort();
}

describe('what a capture tells the app', () => {
    it('sends every key the app reads, and nothing it does not', () => {
        const sent = Object.keys(metaToDict(FULL)).sort();
        expect(keysTheAppReads().length).toBeGreaterThan(10);
        expect(sent).toEqual(keysTheAppReads());
    });

    it('sends the confirmation stamp the capture-to-card budget is measured from', () => {
        const dict = metaToDict(FULL) as unknown as Record<string, { type: string; value: unknown }>;
        expect(dict['confirmed_at']).toEqual({ type: 't', value: FULL.confirmed_at });
    });
});
