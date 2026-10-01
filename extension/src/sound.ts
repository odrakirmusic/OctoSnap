// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Every sound OctoSnap makes, played on the compositor (`spec/09` §4, D134).
 *
 * `ACT-04`'s shutter is here, in the extension and not the app, and not by accident. The
 * sound has to land at the instant the pixels are read -- that simultaneity *is* the
 * feedback, and a D-Bus round trip to the app would put it tens of milliseconds late,
 * which reads as a lag in the capture rather than a lag in the sound. `docs/decisions.md`
 * D11. The app's cues are played here too, when it asks (`PlaySound`), so that one player
 * and one volume serve them all.
 *
 * `Meta.SoundPlayer` is what GNOME's own screenshot UI uses. It is fire-and-forget by
 * design: it hands the request to libcanberra on a thread and returns, so it costs the
 * calling frame nothing and cannot exceed `spec/10` §7's 2 ms budget however slow the
 * audio stack is. libcanberra is also why the files are Ogg Vorbis rather than
 * `spec/09`'s Opus: it reads Vorbis and WAV and nothing else.
 */

import Gio from 'gi://Gio';

import { error } from './log.js';
import { SHUTTERS, TICK, cue, type ShutterSound, type Sound } from './soundCues.js';

/** Plays the chosen shutter, or nothing for `none`. Never throws, as below. */
export function playShutter(kind: ShutterSound): void {
    if (kind !== 'none') play(SHUTTERS[kind], 'the shutter sound');
}

/**
 * The countdown's tick. Silent with the shutter: a user who silenced the capture has
 * asked for a quiet capture, and the countdown is the capture's first second.
 */
export function playTick(shutter: ShutterSound): void {
    if (shutter !== 'none') play(TICK, 'the countdown tick');
}

/** `PlaySound`'s cue called `name`. `false` for a name that is not a cue. */
export function playCue(name: string): boolean {
    const sound = cue(name);
    if (sound === null) return false;
    play(sound, `the ${name} sound`);
    return true;
}

/**
 * Never throws. A capture that succeeded must not be reported as failed because the
 * sound daemon was not there, and on a machine with no audio at all that would be every
 * capture.
 */
function play(sound: Sound, what: string): void {
    try {
        const player = global.display.get_sound_player();
        if (sound.file === undefined) {
            player.play_from_theme(sound.event ?? '', sound.description, null);
            return;
        }
        const file = soundFile(sound.file);
        // A stat, not a read: microseconds, and the one way a packaging slip that left the
        // sounds out would say so rather than be silent.
        if (!file.query_exists(null)) {
            error(`${what} is missing: ${file.get_path()}`);
            return;
        }
        player.play_from_file(file, sound.description, null);
    } catch (e) {
        error(`could not play ${what}`, e);
    }
}

/** `sounds/<name>.oga`, beside this module in the installed extension. */
function soundFile(name: string): Gio.File {
    const here = Gio.File.new_for_uri(import.meta.url).get_parent();
    if (here === null) throw new Error(`no directory above ${import.meta.url}`);
    return here.get_child('sounds').get_child(`${name}.oga`);
}
