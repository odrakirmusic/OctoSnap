// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The recorder's clock, kept here so it can be tested under Node like `place.ts` -- no
 * `gi://` imports.
 *
 * `elapsedLabel` matches the app pill's `media::text::elapsed_label` exactly (`mm:ss`,
 * `h:mm:ss` past an hour, seconds floored), because the panel timer and the pill show the
 * same recording and a reader who glanced from one to the other must not see two clocks
 * disagree.
 *
 * Where the red frame goes is `outline.ts`'s. Grown outward by its own stroke in logical
 * pixels, as it was until D115, it could land inside the GIF at any scale that is not a
 * whole number.
 */

/** The recording clock, e.g. `00:00`, `01:05`, `1:00:00`. Negative input reads as zero. */
export function elapsedLabel(elapsedMs: number): string {
    const seconds = Math.floor(Math.max(0, elapsedMs) / 1000);
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    const s = seconds % 60;
    const pad = (n: number): string => String(n).padStart(2, '0');
    return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
}
