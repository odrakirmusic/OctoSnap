// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * What the shell reads off the clipboard for Annotate the Clipboard's Image (D169), kept
 * here so it can be tested under Node like `clipboardOffer.ts` -- no `gi://` imports.
 *
 * **The app cannot read the clipboard when it is asked to.** Mutter offers the selection
 * to the client with keyboard focus only, and the shortcut, `octosnap open-from-clipboard`
 * and an `octosnap://` link all fire while another application has it. Measured in a
 * nested shell: an app that had never had a focused window read nothing ("The clipboard
 * holds no image"), a picture or a file; one whose editor had had the keyboard earlier
 * kept the types it was offered then, so a file copied after a picture read as nothing.
 * The shell can always read the selection, so it reads it for the app, as it already
 * writes it for the app (D165).
 *
 * Pixels before a file, as the app's own reader took them: a picture copied in a browser
 * or an image viewer is its pixels, `image/png` before any other image type, since it is
 * lossless and GTK 4 offers it with every picture; a file copied in Files, or a GIF
 * OctoSnap copied (D165), is a `text/uri-list`, and only an image file is taken from one.
 */

/** Which of the clipboard's types to read. */
export type ClipboardRead = { kind: 'pixels'; mime: string } | { kind: 'file' };

/** What to read, from the types the clipboard offers, or `null` when none is an image. */
export function chooseRead(mimes: readonly string[]): ClipboardRead | null {
    if (mimes.includes('image/png')) return { kind: 'pixels', mime: 'image/png' };
    const image = mimes.find(mime => mime.startsWith('image/'));
    if (image !== undefined) return { kind: 'pixels', mime: image };
    if (mimes.includes('text/uri-list')) return { kind: 'file' };
    return null;
}

/** RFC 2483: the first URI of a `text/uri-list`, past its comments and blank lines. */
export function firstUri(list: string): string | null {
    for (const line of list.split(/\r?\n/)) {
        const uri = line.trim();
        if (uri !== '' && !uri.startsWith('#')) return uri;
    }
    return null;
}

/**
 * The extension a read is written under, from its MIME type. The app tells a GIF from a
 * still by its name alone (`history::is_gif`), and reads every other picture by its bytes,
 * so only `gif` has to be right; the rest are for whoever looks in the folder.
 */
export function readSuffix(mime: string): string {
    const subtype = mime.slice(mime.indexOf('/') + 1).toLowerCase();
    if (subtype === 'jpeg') return 'jpg';
    if (subtype === 'svg+xml') return 'svg';
    return subtype.replace(/^x-/, '').replace(/[^a-z0-9]/g, '') || 'img';
}

/** The names `readName` gives, and the only ones the folder's pruning removes. */
export const READ_NAME = /^[0-9A-HJKMNP-TV-Z]{26}\.[a-z0-9]+$/;

/** A read's file name: the read's ULID and its type's suffix. */
export function readName(id: string, mime: string): string {
    return `${id}.${readSuffix(mime)}`;
}
