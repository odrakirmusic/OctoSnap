// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Reading a file without holding up gnome-shell's main loop.
 *
 * The extension runs inside the compositor, so a synchronous read stops every frame of the
 * session until the disk answers. A 5K capture's PNG, which the clipboard is given, is
 * megabytes. extensions.gnome.org's analyzer flags any synchronous read for the same reason
 * (shexli's EGO-X-004).
 */

import Gio from 'gi://Gio';

Gio._promisify(Gio.File.prototype, 'load_contents_async');

/** The whole file, read off the main loop. Rejects with GIO's error when it cannot be read. */
export async function readBytes(path: string): Promise<Uint8Array> {
    const [bytes] = await Gio.File.new_for_path(path).load_contents_async(null);
    return bytes;
}
