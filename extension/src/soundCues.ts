// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * What OctoSnap's sounds are and which file plays each (`spec/09` §4, D134). Kept apart
 * from `sound.ts`, which plays them, so that a test can hold the table against the files.
 */

/**
 * `spec/08` §1's `capture-sound`: OctoSnap's five shutters, the sound theme's
 * `camera-shutter`, or silence. In the order Settings lists them.
 */
export const SHUTTER_SOUNDS = ['classic', 'pop', 'subtle', '8bit', 'soft', 'system', 'none'] as const;
export type ShutterSound = (typeof SHUTTER_SOUNDS)[number];

/** A stored `capture-sound`, or the schema's default for one this build does not know. */
export function shutterSound(stored: string): ShutterSound {
    return (SHUTTER_SOUNDS as readonly string[]).includes(stored) ? (stored as ShutterSound) : 'classic';
}

/** One sound: a file in `sounds/`, or an event of the user's sound theme. */
export interface Sound {
    /** `sounds/<file>.oga`, made by `sounds/synth.py`. */
    readonly file?: string;
    /** A freedesktop sound-naming event, for the one sound the theme plays. */
    readonly event?: string;
    /** What it is *for*: read by a screen reader and shown by some sound daemons. */
    readonly description: string;
}

/**
 * The shutters `capture-sound` chooses between. `system` is the freedesktop sound theme's
 * camera shutter, present in every standard theme, so a lookup rather than a file.
 */
export const SHUTTERS: Readonly<Record<Exclude<ShutterSound, 'none'>, Sound>> = {
    classic: { file: 'shutter-classic', description: 'Screenshot taken' },
    pop: { file: 'shutter-pop', description: 'Screenshot taken' },
    subtle: { file: 'shutter-subtle', description: 'Screenshot taken' },
    '8bit': { file: 'shutter-8bit', description: 'Screenshot taken' },
    soft: { file: 'shutter-soft', description: 'Screenshot taken' },
    system: { event: 'camera-shutter', description: 'Screenshot taken' },
};

/** The countdown's tick, once for each number it shows. */
export const TICK: Sound = { file: 'countdown-tick', description: 'Countdown' };

/**
 * The cues the app asks for by name, over `PlaySound`. The names say what happened and
 * the table says what it sounds like, so the app never picks a sound. Copy and pin share
 * one "tink" (`spec/09` §4).
 *
 * `shutter-*` is Settings' preview of each shutter, played whatever `capture-sound` says,
 * since the user is choosing. There is no upload cue until M8 has somewhere to upload to,
 * and no pause cue until a recording can pause (D68).
 */
export const CUES: Readonly<Record<string, Sound>> = {
    'text-copied': { file: 'text-copied', description: 'Text copied' },
    copied: { file: 'tink', description: 'Copied' },
    pinned: { file: 'tink', description: 'Pinned' },
    'record-start': { file: 'record-start', description: 'Recording started' },
    'record-stop': { file: 'record-stop', description: 'Recording stopped' },
    countdown: TICK,
    ...Object.fromEntries(Object.entries(SHUTTERS).map(([kind, sound]) => [`shutter-${kind}`, sound])),
};

/** The cue called `name`, or `null` for a name that is not one. */
export function cue(name: string): Sound | null {
    return Object.hasOwn(CUES, name) ? CUES[name] ?? null : null;
}
