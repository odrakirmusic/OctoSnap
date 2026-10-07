// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * What the clipboard is given for a capture the app asks to copy (D165), kept here so it
 * can be tested under Node like `place.ts` -- no `gi://` imports.
 *
 * **One type per copy, because that is all GNOME Shell 50 lets an extension offer.**
 * `St.Clipboard.set_content` makes one `Meta.SelectionSourceMemory`, which answers one
 * MIME type and refuses every other ("Mimetype not in selection"). A source of our own
 * that offers several would have to implement `Meta.SelectionSource`'s `read_async`, and
 * GJS 1.88 will not register a virtual function that takes a callback. A Wayland client
 * may offer several, but Mutter only takes a selection from the client with keyboard
 * focus, and copy-on-capture happens without the app ever having it.
 *
 * So the type follows the content:
 *
 * - **a PNG is its pixels**, `image/png`. Measured in a nested shell: a GTK 4 app reads a
 *   texture, Chrome hands the page an `image.png` and inserts the picture, and Files pastes
 *   it as "Pasted image.png".
 * - **a GIF is its file**, `text/uri-list`. Its frames are what a GIF is for, and only the
 *   file carries them: Chrome hands the page the `.gif` itself, Files copies it, a GTK app
 *   reads a file list. As `image/gif` Chrome's paste got nothing at all and Files and GTK
 *   both made a still of the first frame; as `image/png`, which is what it used to be
 *   labelled, Files made a still too.
 *
 * The file a GIF's URI names has to outlive the card, whose closing takes the spool copy
 * away; the app hands over one that does (`crates/app/src/flow.rs`).
 */

export type ClipboardFormat = 'png' | 'gif';

/** What `St.Clipboard.set_content` is given: one MIME type and its bytes. */
export interface ClipboardOffer {
    mime: string;
    bytes: Uint8Array;
}

/** How many leading bytes `sniff` needs. */
export const SNIFF_LENGTH = 8;

const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/**
 * The format, from the file's first bytes rather than its name: a label is only as good
 * as whoever named the file, and labelling GIF bytes `image/png` is the bug D165 fixed.
 */
export function sniff(head: Uint8Array): ClipboardFormat | null {
    if (head.length >= PNG_SIGNATURE.length && PNG_SIGNATURE.every((b, i) => head[i] === b)) return 'png';
    // "GIF87a" or "GIF89a".
    const gif = String.fromCharCode(...head.slice(0, 6));
    if (gif === 'GIF87a' || gif === 'GIF89a') return 'gif';
    return null;
}

/** RFC 2483's `text/uri-list`: one URI a line, each ended by CRLF. */
export function uriList(uris: string[]): Uint8Array {
    return new TextEncoder().encode(uris.map(uri => `${uri}\r\n`).join(''));
}

/** A PNG's offer: its bytes as they are. */
export function pngOffer(bytes: Uint8Array): ClipboardOffer {
    return { mime: 'image/png', bytes };
}

/** A GIF's offer: the URI of its file, which the caller has made sure will last. */
export function gifOffer(uri: string): ClipboardOffer {
    return { mime: 'text/uri-list', bytes: uriList([uri]) };
}
