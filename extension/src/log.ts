// SPDX-License-Identifier: GPL-3.0-or-later

import { LOG_PREFIX } from './protocol.js';

// Note the budget in spec/10 §7: no extension callback may exceed 2 ms, and building a
// string is not free in GJS. Do not call these from inside a hot loop such as the
// pointer stream; log the batch, not the sample.

/**
 * How many lines `recentLog()` keeps: the last few minutes of a busy session, which is
 * what a report about something that just went wrong needs, and a few hundred kilobytes
 * of the shell's memory at most.
 */
export const KEPT_LINES = 500;

const kept: string[] = [];

/**
 * Whether `info` lines reach the journal too (`debug-log`, D135). They are kept for
 * `recentLog()` either way. extensions.gnome.org's review asks that an extension log only
 * what matters, and a capture is a dozen routine lines; warnings and errors always go.
 */
let verbose = false;

/** `debug-log`, as `extension.ts` reads it at enable and on every change. */
export function setVerbose(on: boolean): void {
    verbose = on;
}

/**
 * For `disable()`, which the review requires to leave nothing in module scope. The kept
 * lines go too, so a report saved after the screen was locked, which disables an extension
 * that runs only in the user session, has only the lines since it was unlocked; its
 * warnings and errors are in the journal regardless.
 */
export function forgetLog(): void {
    kept.length = 0;
    verbose = false;
}

/**
 * `spec/11` M7's crash log, from this half: the lines this extension logged since the
 * shell loaded it, newest last, so the app's report can include them (`GetLog`). The
 * journal has them too, but a sandboxed app cannot read the journal, and a person
 * attaching a report should not have to know `journalctl`.
 */
export function recentLog(): string[] {
    return [...kept];
}

function keep(level: string, message: string): void {
    kept.push(`${new Date().toISOString()} ${level} ${message}`);
    if (kept.length > KEPT_LINES)
        kept.splice(0, kept.length - KEPT_LINES);
}

export function info(message: string): void {
    keep('INFO', message);
    if (verbose) console.log(`${LOG_PREFIX} ${message}`);
}

export function warn(message: string): void {
    keep('WARN', message);
    console.warn(`${LOG_PREFIX} ${message}`);
}

export function error(message: string, e?: unknown): void {
    const line = e === undefined
        ? message
        : `${message}: ${e instanceof Error ? e.message : String(e)}`;
    keep('ERROR', line);
    console.error(`${LOG_PREFIX} ${line}`);
}
