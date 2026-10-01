// SPDX-License-Identifier: GPL-3.0-or-later

import { existsSync, readdirSync, readFileSync } from 'node:fs';

import { describe, expect, it } from 'vitest';

import { CUES, SHUTTERS, SHUTTER_SOUNDS, TICK, cue, shutterSound } from './soundCues.js';

const SOUNDS = new URL('../sounds/', import.meta.url);

function schemaKey(name: string): string {
    const schema = readFileSync(
        new URL('../schemas/org.gnome.shell.extensions.octosnap.gschema.xml', import.meta.url),
        'utf8',
    );
    const start = schema.indexOf(`<key name="${name}"`);
    expect(start).toBeGreaterThan(-1);
    return schema.slice(start, schema.indexOf('</key>', start));
}

describe('the sound files', () => {
    const files = [...Object.values(SHUTTERS), TICK, ...Object.values(CUES)]
        .map(sound => sound.file)
        .filter((file): file is string => file !== undefined);

    // `sound.ts` logs a missing file rather than throwing, since a capture must not fail
    // over its sound; so a cue named here with no file behind it would only be silent.
    it('exist for every cue', () => {
        for (const file of files) expect(existsSync(new URL(`${file}.oga`, SOUNDS)), file).toBe(true);
    });

    it('are all played by something', () => {
        const shipped = readdirSync(SOUNDS).filter(name => name.endsWith('.oga'));
        for (const name of shipped) expect(files, name).toContain(name.replace(/\.oga$/, ''));
    });
});

describe('capture-sound', () => {
    it('offers exactly the shutters there are, in the schema and here', () => {
        const key = schemaKey('capture-sound');
        const choices = [...key.matchAll(/<choice value='([^']+)'\/>/g)].map(m => m[1]);
        expect(choices).toEqual([...SHUTTER_SOUNDS]);
        expect(key).toContain("<default>'classic'</default>");
    });

    it('reads a value this build does not know as the default', () => {
        expect(shutterSound('pop')).toBe('pop');
        expect(shutterSound('none')).toBe('none');
        expect(shutterSound('dubstep')).toBe('classic');
    });

    it('has a preview for every shutter but silence', () => {
        for (const kind of SHUTTER_SOUNDS) {
            if (kind === 'none') expect(cue('shutter-none')).toBeNull();
            else expect(cue(`shutter-${kind}`)).toBe(SHUTTERS[kind]);
        }
    });
});

describe('a cue', () => {
    it('is found by the name the app sends', () => {
        expect(cue('copied')?.file).toBe('tink');
        expect(cue('pinned')?.file).toBe('tink');
        expect(cue('text-copied')?.file).toBe('text-copied');
        expect(cue('countdown')).toBe(TICK);
    });

    // `PlaySound` takes a string off the bus, and an object's inherited names are strings
    // too: `CUES['toString']` is a function, not a sound.
    it('is never a name the table only inherits', () => {
        expect(cue('toString')).toBeNull();
        expect(cue('__proto__')).toBeNull();
        expect(cue('')).toBeNull();
    });
});
