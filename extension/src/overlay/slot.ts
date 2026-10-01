// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Where a capture's card lands, which is where the fly animation aims (`spec/03` §7
 * step 5, `spec/04` §2).
 *
 * Imports nothing from `gi://`, on purpose: like `selection.ts` and `accel.ts` it has to
 * run under Node to be tested. The arithmetic is worth testing because getting it wrong
 * is not a crash -- the capture flies to the wrong place and looks like a design choice.
 *
 * `spec/04` §2's measurements, and they are measurements rather than guesses:
 *
 * - **The anchor is an edge, not a corner.** Cards sit against the left edge, anchored at
 *   the bottom, and the stack grows *upward* as captures arrive.
 * - Margin ~16 pt from the screen edge, ~8 pt between cards.
 * - "Card size measured from the capture: ~207 x 125 pt for a 16:10 source", so the card
 *   keeps the capture's shape rather than cropping it to a fixed box.
 *
 * Only slot 0 is computed here. The stack offset needs the heights of every card below,
 * which the app knows and the extension does not -- that is `M2`'s job. What the fly
 * animation needs today is where the *newest* card appears, and that is always slot 0.
 */

/** `spec/04` §2: the longest side of a card, in logical pixels. */
export const CARD_MAX = 207;

/**
 * The card's shape, which is **the same for every capture** (`core::qao::CARD_ASPECT`,
 * `docs/decisions.md` D43). A card is a handle to a capture, not a preview of its
 * geometry, so a stack of them is a list rather than a ragged column.
 */
export const CARD_SHORT = 125;

/** `spec/04` §2: distance from the screen edge. */
export const EDGE_MARGIN = 16;

/** `spec/04` §2: distance between stacked cards. `M2` needs it; slot 0 does not. */
export const CARD_GAP = 8;

export interface Rect {
    x: number;
    y: number;
    width: number;
    height: number;
}

/**
 * The card every capture gets: one box, `CARD_MAX` x `CARD_SHORT`.
 *
 * The **app's** size slider can make this bigger or smaller and the extension cannot see
 * that setting, so the fly animation always aims at the default step. That is a known
 * imprecision in a 400 ms flourish rather than a bug worth a new option on `BeginCapture`:
 * at a non-default overlay size the capture lands slightly off the card it becomes, and
 * the card fades in over the join.
 */
export function cardSize(): { width: number; height: number } {
    return { width: CARD_MAX, height: CARD_SHORT };
}

/**
 * Slot 0: the newest card's rect, in logical stage coordinates.
 *
 * Takes no capture, and that is the point: every card is the same box (D43), so the slot
 * the capture flies to does not depend on the capture either.
 *
 * `workArea` rather than the monitor rect, so the card clears the top panel and the dock
 * (`Main.layoutManager.getWorkAreaForMonitor`). A card larger than the work area is
 * clamped to it rather than flying off the top of the screen.
 */
export function cardSlot(workArea: Rect): Rect {
    const size = cardSize();
    const height = Math.min(size.height, Math.max(1, workArea.height - 2 * EDGE_MARGIN));
    const width = Math.min(size.width, Math.max(1, workArea.width - 2 * EDGE_MARGIN));
    return {
        x: workArea.x + EDGE_MARGIN,
        // Anchored at the bottom: the card's *bottom* edge sits one margin above the
        // work area's bottom, so its top depends on how tall this particular card is.
        y: workArea.y + workArea.height - EDGE_MARGIN - height,
        width,
        height,
    };
}

/**
 * The uniform scale that lands `source` inside `target`.
 *
 * One factor for both axes, not two. An anisotropic scale would squash the capture into
 * the card's shape on its way down -- and now that every card is the *same* shape
 * (D43), that squash would be arbitrary and different for every capture. The smaller
 * factor keeps the whole thumbnail inside the card instead, and the card's own crop takes
 * over as it fades in.
 */
export function flyScale(source: Rect, target: Rect): number {
    if (source.width <= 0 || source.height <= 0) return 1;
    // Never above 1. The card used to shrink to the capture's own shape, so this could
    // not enlarge; now the card is a fixed box and a capture smaller than it would be
    // blown up on the way down -- flying *backwards*, growing as it goes to the corner,
    // which reads as "something opened" rather than "this was filed away".
    return Math.min(1, target.width / source.width, target.height / source.height);
}

/**
 * Where the flying capture comes to rest, and how far it shrinks.
 *
 * Centred in the card rather than pinned to its top-left corner. The card is one fixed
 * shape now (D43) and the capture is not, so the two almost never match: a portrait
 * capture flying into a landscape card leaves a wide gap on one side, and putting all of
 * it on the right looks like the animation missed.
 */
export function flyLanding(source: Rect, target: Rect): { x: number; y: number; scale: number } {
    const scale = flyScale(source, target);
    return {
        x: Math.round(target.x + (target.width - source.width * scale) / 2),
        y: Math.round(target.y + (target.height - source.height * scale) / 2),
        scale,
    };
}
