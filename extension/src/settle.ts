// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * How a card that has just moved settles into its new place (D201).
 *
 * The reference app's cards were measured going about 4 pt past their place and back as a
 * gap closed, settled in a little over 200 ms. The overshoot is a fixed distance, not a share
 * of the move: Clutter's EASE_OUT_BACK throws a card about a tenth of its way past, which for
 * a card-height collapse is a whole margin, and a stack that bounced that far would read as
 * loose rather than settled. No `gi://` imports, so it is tested under Node.
 */

/** How far past its place a moving card goes, in logical pixels. */
export const SETTLE_PX = 4;

/** How long the way back from past its place takes. */
export const SETTLE_BACK_MS = 90;

/** A move shorter than this goes straight there: an overshoot on a nudge is a wobble. */
export const SETTLE_MIN_MOVE_PX = 24;

export interface Settle {
    /** Where the actor goes first, relative to its place. */
    pastX: number;
    pastY: number;
    /** How long getting past its place takes (the move's own duration), and coming back. */
    outMs: number;
    backMs: number;
}

/**
 * The settle for a move of (`moveX`, `moveY`) that was asked to take `durationMs`, or `null`
 * when the move should go straight to its place: a short one, or one with no time to spend,
 * as when animations are off.
 */
export function settleFor(moveX: number, moveY: number, durationMs: number): Settle | null {
    const length = Math.hypot(moveX, moveY);
    if (!(durationMs > 0) || length < SETTLE_MIN_MOVE_PX) return null;
    return {
        pastX: (moveX / length) * SETTLE_PX,
        pastY: (moveY / length) * SETTLE_PX,
        outMs: durationMs,
        backMs: SETTLE_BACK_MS,
    };
}
