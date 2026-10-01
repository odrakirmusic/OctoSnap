// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Starting a GIF recording (`spec/06` §3): choose an area, count down, hand it to the app.
 *
 * Deliberately *not* part of `flow.ts`. A recording is not a screenshot -- it reads no
 * pixels, writes no PNG, writes no twin and flies nothing to a corner. What it shares with
 * a capture is only the area overlay and the countdown, and those are imported here rather
 * than the recording being threaded through `runCapture`'s pixel machinery. The order is:
 * select (red record overlay, or an explicit rect from the CLI/URL), remember the area,
 * hide the desktop icons, count down over the live screen, then `notifyRecord`. Everything
 * after that -- the ScreenCast session, the encoder, the files -- is the app's (`spec/10`
 * §1).
 */

import GLib from 'gi://GLib';
import { armRecord, cancelRecord, notifyRecord } from './app.js';
import type { GifOverrides } from './recordChoices.js';
import type { DesktopIcons } from './desktop.js';
import { AreaOverlay } from './overlay/engine.js';
import { Countdown } from './overlay/countdown.js';
import type { Settings } from './settings.js';
import type { Rect } from './spool.js';
import { info } from './log.js';
import { tellPets } from './pets/events.js';

export interface RecordOptions {
    /** An explicit logical rect from the CLI, a URL or the All-In-One overlay, skipping
     * the record overlay. */
    rect?: Rect;
    /**
     * Remember `rect` as the last recorded area (`REC-01`). True when the rect was drawn
     * by the user in the All-In-One overlay; left off for the CLI and URLs, whose rects
     * are a script's and should not move the area the user comes back to.
     */
    remember?: boolean;
    /**
     * `spec/06` §2 from the All-In-One toolbar: what the recording row was changed to.
     * Only the touched keys are here, and only they are sent (D101).
     */
    gif?: GifOverrides;
}

/**
 * Runs one recording start. Resolves `true` when the app was asked to record, `false` when
 * the user cancelled the selection or the countdown.
 *
 * The desktop icons are hidden here, *before* the countdown, rather than when the app later
 * reports the recording state -- so neither the countdown nor the very first frame has them
 * (`REC-08`). On cancel they go straight back; on success the recording coordinator owns
 * them from `SetRecordingState` on, and restores them at Stop or if the app crashes. The
 * two share `DesktopIcons`' recording flag, so the coordinator's own hide is an idempotent
 * no-op over this one.
 *
 * `settings` and `desktopIcons` are passed in rather than read from module globals, for the
 * same reason `runCapture` takes them: the flow stays testable and `disable()` cannot
 * strand a stale reference.
 */
export async function runRecording(
    settings: Settings,
    desktopIcons: DesktopIcons,
    options: RecordOptions = {},
): Promise<boolean> {
    const requestedAt = GLib.get_monotonic_time();
    // --- decide the rectangle ---------------------------------------------------
    let rect: Rect;
    if (options.rect !== undefined) {
        rect = options.rect;
        if (options.remember === true) settings.rememberRecordArea(rect);
    } else {
        // No freeze and no crosshair: the region is chosen over the live screen it is about
        // to record, and the last recorded area is restored so "record that again" is one
        // keypress (`REC-01`).
        const overlay = new AreaOverlay();
        try {
            const selection = await overlay.open({
                record: true,
                freeze: false,
                crosshair: 'off',
                magnifier: false,
                initialRect: settings.lastRecordArea,
                requestedAt,
            });
            if (selection === null) {
                info('recording cancelled at selection');
                return false;
            }
            rect = selection.rect;
            settings.rememberRecordArea(rect);
        } finally {
            // Gone before the countdown, so the user can arrange what they are recording
            // without the dim over it -- the same reason self-timer drops its overlay.
            overlay.destroy();
        }
    }

    // --- hide icons, count down, hand off ---------------------------------------
    desktopIcons.beginRecording(settings.hideDesktopWhileRecording);

    // The app sets the whole recording up while the numbers count, so that `notifyRecord`
    // below has nothing left to do but keep the frames (D70). Sent after the icons are
    // hidden, so the discarded frames the armed stream produces are already the screen the
    // recording will show -- it is the first *kept* frame that matters, but a stream that
    // never sees the icons cannot leak one.
    armRecord(rect, options.gif);
    // The pets walk out of the area while the numbers count (`pets/crew.ts`): a
    // recording is the one capture they would otherwise be in.
    tellPets({ kind: 'recording-soon', rect, countdownMs: settings.recordCountdown * 1000 });

    const countdown = new Countdown(settings.recordCountdown, settings.shutterSound);
    let completed: boolean;
    try {
        completed = await countdown.run(rect);
    } finally {
        countdown.destroy();
    }
    if (!completed) {
        info('recording cancelled during countdown');
        // The app is holding a ScreenCast session and Mutter is showing its recording
        // indicator for a recording that will not happen. Give both back.
        cancelRecord();
        // No recording will happen, so the coordinator will never restore them: do it here.
        desktopIcons.endRecording();
        tellPets({ kind: 'recording-stopped' });
        return false;
    }

    // Any pet still inside goes now, before the first frame is kept.
    tellPets({ kind: 'recording', rect });
    notifyRecord(rect, options.gif);
    info(`recording requested: ${rect.width}x${rect.height} at ${rect.x},${rect.y}`);
    return true;
}
