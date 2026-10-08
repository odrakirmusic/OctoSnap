// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * What a capture sounds and looks like as it is taken (D171). Decided without GLib so it
 * can be tested.
 *
 * A screenshot plays the shutter as its pixels are read (`ACT-04`), and the picture then
 * flies to where its card will be (`spec/03` §7 step 5). Both say "a picture was taken,
 * and here it comes".
 *
 * A text capture does neither, because both would be untrue. Nothing is kept as a
 * picture, and no card comes for it: `spec/07` §2.1's sequence is "select an area ->
 * sound -> recognized text is copied -> notification", and the sound there is the text's,
 * which the app plays when the read lands. Until D171 a text capture took the shutter and
 * the flight from the screenshot path it shares. When the read could not happen, a
 * notification was the only thing to say so, and a user who missed it was left with what
 * looked and sounded like a screenshot that never arrived.
 */

/** Whether a capture reported as `mode` sounds the shutter and flies to its card. */
export function takenAsPicture(mode: string): boolean {
    return mode !== 'ocr';
}
