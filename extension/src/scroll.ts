// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Choosing what to scroll-capture (`spec/07` §1.1 item 1), and handing it to the app.
 *
 * Deliberately not part of `flow.ts`, for the same reason `record.ts` is not: a scrolling
 * capture reads no pixels here, writes no PNG, writes no twin and flies nothing to a
 * corner. What it shares with a capture is the area overlay, and that is imported rather
 * than the whole thing being threaded through `runCapture`'s pixel machinery.
 *
 * The overlay is *gone* before the app is told. It holds a modal grab, and the next thing
 * that has to happen is the user scrolling the window underneath -- which a grab would
 * swallow. That is also why the controls are the app's window and not part of the overlay.
 */

import { type ScrollOptions, startScrollCapture } from './app.js';
import { AreaOverlay } from './overlay/engine.js';
import type { Settings } from './settings.js';
import type { Rect } from './spool.js';
import { info } from './log.js';
import { tellPets } from './pets/events.js';

export interface ScrollCaptureOptions extends ScrollOptions {
    /** An explicit logical rect from the All-In-One toolbar, the CLI or a URL. */
    rect?: Rect;
}

/**
 * Runs one scrolling-capture start. Resolves `true` when the app was asked for one.
 */
/**
 * What the selection is for, which the screenshot hint does not say: the area that scrolls,
 * not a picture of it.
 */
const HINT_SCROLL = 'Select the part that scrolls · Drag to select · Space to move · Esc to cancel';

export async function runScrollingCapture(
    settings: Settings,
    options: ScrollCaptureOptions = {},
): Promise<boolean> {
    let rect: Rect;
    if (options.rect !== undefined) {
        rect = options.rect;
    } else {
        // No freeze: the point of the selection is the live, scrollable thing underneath
        // it, and a frozen screen would be a picture of a page that cannot move.
        const overlay = new AreaOverlay();
        try {
            const selection = await overlay.open({
                freeze: false,
                crosshair: settings.crosshair,
                magnifier: false,
                hint: HINT_SCROLL,
            });
            if (selection === null) {
                info('scrolling capture cancelled at selection');
                return false;
            }
            rect = selection.rect;
        } finally {
            overlay.destroy();
        }
    }
    // Not remembered as the last area: `last-area` is what `capture-previous-area`
    // repeats, and a scrolling selection is a different shape of thing to want back.
    //
    // The pets hear of it now, before the app has it: the app opens its live view of the
    // selection -- a ScreenCast stream, which shows them -- before it starts the scroll
    // assist, so the assist's own word came too late to tell that stream from another
    // program sharing the screen. They leave the selection, and the stream is OctoSnap's.
    tellPets({ kind: 'scrolling', active: true, rect });
    startScrollCapture(rect, options);
    info(`scrolling capture requested: ${rect.width}x${rect.height} at ${rect.x},${rect.y}`);
    return true;
}
