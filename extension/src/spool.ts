// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The capture spool: `spec/10` §4's `~/.cache/octosnap/spool/<ulid>.png` plus its `.json`
 * twin.
 *
 * `spec/10` §8 is the rule this module exists to honour: **the twin is written before the
 * app is notified**, so a capture survives a lost D-Bus message, an app that is not
 * running, or an app that crashes while handling it. The extension's job is done once the
 * two files are on disk; everything after that is recoverable.
 */

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import { APP_BUS_NAME } from './protocol.js';
import { serviceDirs, serviceFile, spoolFor, startsAFlatpak } from './spoolRule.js';
import { type RandomSource, ulidFrom } from './ulid.js';
import { readBytes } from './files.js';
import { error, info } from './log.js';

export interface Rect {
    x: number;
    y: number;
    width: number;
    height: number;
}

/**
 * `spec/10` §3.3's capture result, flat -- the same shape as the D-Bus dictionary and as
 * the Rust `CaptureResult`'s serialised form, so the twin and the message cannot drift.
 */
export interface CaptureMeta {
    path: string;
    meta_path: string;
    mode: string;
    /** Logical stage rect. */
    rect: Rect;
    /** Monitor or resource scale; `path` is `rect` x `scale` in physical pixels. */
    scale: number;
    display: string;
    cursor_rect?: Rect;
    app_id?: string;
    app_name?: string;
    window_title?: string;
    window_alpha: boolean;
    /** Microseconds since the Unix epoch. */
    timestamp: number;
    /**
     * When the user finished deciding, in microseconds since the Unix epoch.
     *
     * The start of `spec/00` §9's 300 ms budget, and the reason it is a separate field
     * from `timestamp`: everything before this point is a person choosing a region, and
     * none of it belongs in a performance figure. Everything after it is the machine's
     * work, which is what the budget is about. `flow.ts` already drew that line for its
     * own log; the app needs the same instant to be able to say whether the *whole* path
     * -- pixels, IPC, window, placement -- came in under budget, and nobody had measured
     * that end to end because no single process could see both ends of it.
     */
    confirmed_at: number;
    animation_ms: number;
    requested_action?: string;
    modifiers: number;
    /**
     * `spec/07` §2.1's two text shortcuts: keep the line breaks for this read. Only ever
     * on an `ocr` capture, and absent means the app's own setting decides.
     */
    linebreaks?: boolean;
}

const randomDigit: RandomSource = () => GLib.random_int_range(0, 32);

/**
 * The spool the running app announced (`SetSpool`), for as long as that app is on the
 * bus; `dbus.ts` clears it when the app's name goes. See `spoolRule.ts` for why the
 * app's word comes first.
 */
let announced: string | null = null;

export function setAnnouncedSpool(dir: string | null): void {
    if (dir !== announced)
        info(dir === null ? 'the app left; the spool is worked out again' : `the app's spool is ${dir}`);
    announced = dir;
}

/**
 * Where a capture is written: the running app's spool, or, when no app is running, the
 * spool of the app D-Bus will start to read it (D122).
 */
export async function spoolDir(): Promise<string> {
    return announced ?? (await coldStartSpool());
}

/** The service file is read off the main loop (`files.ts`), small as it is. */
async function coldStartSpool(): Promise<string> {
    const dirs = serviceDirs(GLib.get_user_runtime_dir(), GLib.get_user_data_dir(), GLib.get_system_data_dirs());
    const service = serviceFile(dirs, APP_BUS_NAME, path => GLib.file_test(path, GLib.FileTest.EXISTS));
    let sandboxed = false;
    if (service !== null) {
        let text: string | null = null;
        try {
            text = new TextDecoder().decode(await readBytes(service));
        } catch (e) {
            error(`could not read ${service}`, e);
        }
        sandboxed = startsAFlatpak(service, text);
    }
    return spoolFor(GLib.get_home_dir(), GLib.get_user_cache_dir(), APP_BUS_NAME, sandboxed);
}

/** Creates the spool directory if it is missing. Idempotent. */
export async function ensureSpoolDir(): Promise<string> {
    const dir = await spoolDir();
    try {
        Gio.File.new_for_path(dir).make_directory_with_parents(null);
    } catch (e) {
        // Already there is the normal case, not a failure.
        if (!(e instanceof GLib.Error) || !e.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.EXISTS))
            error(`could not create the spool directory ${dir}`, e);
    }
    return dir;
}

export interface SpoolEntry {
    id: string;
    png: string;
    json: string;
}

/** Allocates a new spool entry. The id sorts chronologically (see `ulid.ts`). */
export async function newEntry(): Promise<SpoolEntry> {
    const dir = await ensureSpoolDir();
    const id = ulidFrom(Date.now(), randomDigit);
    return {
        id,
        png: GLib.build_filenamev([dir, `${id}.png`]),
        json: GLib.build_filenamev([dir, `${id}.json`]),
    };
}

/**
 * Writes the JSON twin. Must be called **before** notifying the app (`spec/10` §8).
 *
 * Throws on failure rather than logging, because a capture whose twin could not be
 * written is not recoverable and the caller needs to know that before it tells the app
 * the capture succeeded.
 */
export function writeTwin(meta: CaptureMeta): void {
    const json = JSON.stringify(meta, null, 1);
    const [ok] = Gio.File.new_for_path(meta.meta_path).replace_contents(
        new TextEncoder().encode(json),
        null,
        false,
        Gio.FileCreateFlags.NONE,
        null,
    );
    if (!ok)
        throw new Error(`could not write the capture twin ${meta.meta_path}`);
}
