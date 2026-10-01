// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Where an app window goes, for `spec/10` §3.1's `PlaceWindow`.
 *
 * On Wayland a client cannot position itself, so every floating surface OctoSnap owns --
 * the Quick Access Overlay's cards, pins, the recorder bar -- is placed by the extension
 * on the app's behalf. This module is the arithmetic half of that, with no `gi://`
 * imports so it can be tested under Node like `selection.ts` and `slot.ts`.
 *
 * Two findings from `docs/spikes/01-02-placement-and-focus.md` shape all of it:
 *
 * 1. **`move_frame` is clamped, silently.** Mutter keeps the window fully on-screen and
 *    clear of the panel, so `(0, 0)` comes back as `(0, 32)` and an off-screen request is
 *    pulled inside. There is no error and no signal. So placement is computed from the
 *    **work area**, and `PlaceWindow` *returns the rect the window actually got* rather
 *    than the one it asked for -- the caller stacks against reality.
 * 2. **Placing a window does not steal focus.** Verified with `make_above`, `stick` and
 *    `raise` against a focused window: `focus_changed: false`. So no focus restoration
 *    dance is needed, which `spec/01` §2 row 11 had allowed for.
 */

/** `spec/04` §2, measured: distance from the screen edge. */
export const QAO_MARGIN = 16;

/** `spec/04` §2, measured: distance between stacked cards. */
export const QAO_GAP = 8;

export interface Rect {
    x: number;
    y: number;
    width: number;
    height: number;
}

export interface Size {
    width: number;
    height: number;
}

/**
 * Which screen edge a stack hugs.
 *
 * `spec/04` §2: "**Anchor is an edge, not a corner.** The setting reads 'Position on
 * screen: Left'. Cards sit against the left edge, anchored at the bottom, and the stack
 * grows **upward** as captures arrive." So the choice is which *side*; the vertical
 * anchor is always the bottom, and only the side is a setting.
 */
export type Edge = 'left' | 'right';

/**
 * The top-left corner for a window of `size` at `offset` pixels up the stack.
 *
 * `offset` is a distance, not an index, and that is deliberate. `spec/10` §3.1 documents
 * an `index i`, but `spec/04` §2 also says a card takes its size from the capture, so
 * cards in one stack have **different heights** and an index cannot locate the third one.
 * The app knows every card's height because `PlaceWindow` told it; so the app sums them
 * and the extension owns only the anchor and the margins. Each half knows exactly what it
 * is in a position to know.
 *
 * `inset` is the transparent band the window carries around its card for the drop shadow
 * (`spec/04` §9). It matters here because every measurement in `spec/04` §2 is of the
 * *card* -- 16 pt from the screen edge, 8 pt between cards are distances between things
 * you can see -- while `move_frame` positions the *window*. Placing the window at the
 * card's margin would push the card `inset` pixels further in and open the gaps to
 * `8 + 2 * inset`, which is a visibly different layout from the one that was measured. So
 * the window is pulled outward by `inset` and the offsets stay in card space. Consecutive
 * windows then overlap, which costs nothing: the bands are transparent and each window's
 * input region is only its card.
 *
 * `inset` is capped at `QAO_MARGIN`, because a wider band would put the window outside
 * the work area, where Mutter's silent clamp would pull it back and move the card with
 * it -- turning a measured margin into whatever the clamp decided.
 *
 * Clamped to the work area on both axes, because a window Mutter has to pull back inside
 * lands somewhere the caller did not choose, and a card taller than the work area cannot
 * be placed as asked at all.
 */
export function placeAtEdge(
    workArea: Rect,
    size: Size,
    edge: Edge,
    offset: number,
    inset = 0,
): { x: number; y: number } {
    const band = clamp(inset, 0, QAO_MARGIN);
    const outer = QAO_MARGIN - band;

    const x =
        edge === 'left'
            ? workArea.x + outer
            : workArea.x + workArea.width - outer - size.width;

    // Bottom-anchored, growing upward: the *bottom* edge of the newest card sits one
    // margin above the work area's bottom, and each card above it clears the ones below.
    const y = workArea.y + workArea.height - outer - offset - size.height;

    return {
        x: clamp(x, workArea.x, workArea.x + Math.max(0, workArea.width - size.width)),
        y: clamp(y, workArea.y, workArea.y + Math.max(0, workArea.height - size.height)),
    };
}

/**
 * The stack offset for the card after these ones.
 *
 * Heights in order from the anchored edge outward, so `stackOffset([])` is 0 for the first
 * card and each later card clears the ones below it plus one gap each.
 */
export function stackOffset(heightsBelow: readonly number[]): number {
    return heightsBelow.reduce((total, h) => total + h + QAO_GAP, 0);
}

/**
 * Whether a stack of these heights still fits the work area.
 *
 * `spec/04` §2 records that six cards were visible at once with no "+N more" collapse, so
 * there is no fold-away behaviour to implement -- but there is a point past which the top
 * card would be clamped on top of its neighbour, and the caller wants to know before that
 * looks like a bug rather than a limit.
 */
export function stackFits(workArea: Rect, heights: readonly number[]): boolean {
    if (heights.length === 0) return true;
    const total = heights.reduce((sum, h) => sum + h, 0) + QAO_GAP * (heights.length - 1);
    return total + 2 * QAO_MARGIN <= workArea.height;
}

function clamp(value: number, low: number, high: number): number {
    return Math.max(low, Math.min(value, high));
}
