// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Reading and writing a file without holding up gnome-shell's main loop.
 *
 * The extension runs inside the compositor, so a synchronous read stops every frame of the
 * session until the disk answers. A 5K capture's PNG, which the clipboard is given, is
 * megabytes. extensions.gnome.org's analyzer flags any synchronous read for the same reason
 * (shexli's EGO-X-004).
 */

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

Gio._promisify(Gio.File.prototype, 'load_contents_async');
Gio._promisify(Gio.File.prototype, 'replace_contents_bytes_async', 'replace_contents_finish');
Gio._promisify(Gio.File.prototype, 'read_async');
Gio._promisify(Gio.InputStream.prototype, 'read_bytes_async');
Gio._promisify(Gio.InputStream.prototype, 'close_async');

/** The whole file, read off the main loop. Rejects with GIO's error when it cannot be read. */
export async function readBytes(path: string): Promise<Uint8Array> {
    const [bytes] = await Gio.File.new_for_path(path).load_contents_async(null);
    return bytes;
}

/**
 * At most the first `length` bytes, read off the main loop: enough to tell what a file is
 * without bringing a GIF of many megabytes into the compositor's memory (D165). Shorter
 * when the file is. Rejects with GIO's error when it cannot be read.
 */
export async function readHead(path: string, length: number): Promise<Uint8Array> {
    const stream = await Gio.File.new_for_path(path).read_async(0, null);
    try {
        const head = new Uint8Array(length);
        let filled = 0;
        // A read may return less than was asked; an empty one is the end of the file.
        while (filled < length) {
            const chunk = (await stream.read_bytes_async(length - filled, 0, null)).toArray();
            if (chunk.length === 0) break;
            head.set(chunk, filled);
            filled += chunk.length;
        }
        return head.slice(0, filled);
    } finally {
        await stream.close_async(0, null);
    }
}

/** Writes `bytes` to a file that only its owner can read, off the main loop. */
export async function writeBytes(path: string, bytes: GLib.Bytes): Promise<void> {
    await Gio.File.new_for_path(path).replace_contents_bytes_async(
        bytes, null, false, Gio.FileCreateFlags.PRIVATE, null);
}

/**
 * Copies a file, off the main loop. Rejects when the target is already there.
 *
 * Its callback by hand: `copy_async` takes a progress callback as well, and the bindings
 * type no promise for it.
 */
export function copyFile(source: Gio.File, target: string): Promise<void> {
    return new Promise((resolve, reject) => {
        source.copy_async(Gio.File.new_for_path(target), Gio.FileCopyFlags.NONE, GLib.PRIORITY_DEFAULT, null,
            null, (_file: Gio.File | null, result: Gio.AsyncResult) => {
                try {
                    source.copy_finish(result);
                    resolve();
                } catch (e) {
                    reject(e);
                }
            });
    });
}
