// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * What All-In-One hands on when its toolbar ends on Record or Scroll (`spec/06` §3,
 * `spec/07` §1.1). The capture flow closes its overlay and returns no capture; the
 * recording or the scrolling capture starts here, once the overlay's modal grab is gone,
 * which is the one thing a scrolling capture cannot start without.
 *
 * Shared by every way All-In-One opens. It used to live in `extension.ts`, under the
 * shortcuts, the panel and the pets' menu, and `BeginCapture` -- the CLI's and a link's way
 * in -- passed no hand-off at all: Record or Scroll chosen there closed the overlay and
 * nothing started. `pets-test.sh` found it on 2026-09-26, starting a recording the way a
 * user would.
 */

import type { DesktopIcons } from './desktop.js';
import { info } from './log.js';
import { runRecording } from './record.js';
import type { GifOverrides } from './recordChoices.js';
import type { RecordingCoordinator } from './recording.js';
import { runScrollingCapture } from './scroll.js';
import type { Settings } from './settings.js';
import type { Rect } from './spool.js';

export class Handoff {
    #record: { rect: Rect; gif: GifOverrides } | null = null;
    #scroll: Rect | null = null;

    /** For `runCapture`'s options: what the toolbar ended on, kept for `run`. */
    readonly onRecord = (rect: Rect, gif: GifOverrides): void => {
        this.#record = { rect, gif };
    };

    readonly onScrolling = (rect: Rect): void => {
        this.#scroll = rect;
    };

    /**
     * Starts what was handed on, if anything. Awaited under the caller's own guard, so no
     * second overlay can open between the capture's and the recording's countdown.
     * `isActive` is the recorder's own guard: a running recording is stopped first.
     */
    async run(settings: Settings, desktopIcons: DesktopIcons | null, recording: RecordingCoordinator | null): Promise<void> {
        const scroll = this.#scroll;
        const record = this.#record;
        this.#scroll = null;
        this.#record = null;
        if (scroll !== null) await runScrollingCapture(settings, { rect: scroll });
        if (record === null || desktopIcons === null) return;
        if (recording?.isActive() === true) {
            info('ignoring record from All-In-One: a recording is already in progress');
            return;
        }
        await runRecording(settings, desktopIcons, { rect: record.rect, remember: true, gif: record.gif });
    }
}
