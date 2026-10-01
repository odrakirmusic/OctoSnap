// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Who has the keyboard while a Quick Access Overlay card is on screen (`spec/04` §3).
 *
 * `spec/04`'s first principle is that the overlay "never steals focus", and `spec/01` §5
 * says the same thing as a rule: overlays are shown with `set_visible` and never
 * `present()`ed. The app follows that rule exactly, and it is not enough.
 *
 * **A newly mapped Wayland toplevel is focused by Mutter regardless of how the client
 * showed it.** `docs/spikes/01-02` reported "placing a window does not steal focus", and
 * that is true and answers a different question: it measured `move_frame`, `make_above`
 * and `stick` on a window that already existed. Focus is taken earlier than that, when
 * the window maps, and no client-side call prevents it -- xdg-shell has no
 * "focus on map" and GTK4 dropped the GTK3 property that used to carry it. Measured on
 * 2026-09-08 with an editor holding focus: the card took the keyboard every time.
 *
 * So the compositor half has to put it back, which it is in a position to do: it is the
 * only half that can see focus move at all. That is the whole of this file.
 */

import type Meta from 'gi://Meta';

import { error, info } from './log.js';

/** `spec/04` §9's card title, which is how a card is recognised from the compositor side. */
export const CARD_TITLE_PREFIX = 'octosnap-qao:';

/** `spec/07` §3.2's pinned-screenshot title. */
export const PIN_TITLE_PREFIX = 'octosnap-pin:';
/** `spec/07` §4.2's history strip: one window, so no id after the name. */
export const HISTORY_TITLE_PREFIX = 'octosnap-history';

/**
 * A window OctoSnap floats above the desktop: a Quick Access card or a pinned screenshot.
 *
 * Both, not just cards, because both are mapped without the user asking for a window and
 * both would otherwise take the keyboard from whatever they were typing in. A pin can
 * arrive entirely unbidden -- `ACT-01` lets `pin` be an after-capture action -- so it is
 * exactly the case the rule exists for.
 */
export function isFloating(window: Meta.Window): boolean {
    const title = window.get_title();
    if (title === null) return false;
    return (
        title.startsWith(CARD_TITLE_PREFIX) ||
        title.startsWith(PIN_TITLE_PREFIX) ||
        title.startsWith(HISTORY_TITLE_PREFIX)
    );
}

export function isCard(window: Meta.Window): boolean {
    return window.get_title()?.startsWith(CARD_TITLE_PREFIX) === true;
}

/**
 * Keeps the keyboard where the user left it, and lends it to a hovered card.
 *
 * One object for both halves because they are the same fact: "what had focus before a
 * card took it". Two separate memories would disagree the first time a card mapped while
 * another card was hovered, and the user's editor would be the thing that lost.
 */
export class CardFocus {
    /**
     * What had the keyboard before a floating window took or borrowed it.
     *
     * Kept continuously rather than captured at one moment: it is simply the last
     * non-floating window to hold focus, updated every time focus moves. The first
     * version read it once, at the pin's or card's `window-created`, and that is a moment
     * whose answer can already be another card -- click Pin on a hovered card and the
     * card is what had focus.
     */
    #previous: Meta.Window | null = null;

    /**
     * The window the pointer is over, which has been *given* the keyboard on purpose.
     *
     * The whole difficulty here is telling "the user hovered a card, so we lent it the
     * keyboard" from "a window mapped and Mutter handed it the keyboard". They look
     * identical from `notify::focus-window`; this is what distinguishes them.
     */
    #lentTo: Meta.Window | null = null;
    #focusId = 0;

    /**
     * Starts putting the keyboard back whenever a floating window takes it uninvited.
     *
     * Watches focus itself rather than the map. The first version hooked `window-created`
     * and restored one frame after `shown`, which **raced**: Mutter sometimes focuses the
     * window after that frame, the handler saw nothing wrong and returned, and the pin
     * kept the keyboard. A race that resolves the wrong way only sometimes is worse than
     * a plain bug -- the card path passed its test while the pin path failed the same
     * one. Focus changes cannot be missed the way a single deferred check can.
     */
    watch(): void {
        if (this.#focusId !== 0) return;
        this.#focusId = global.display.connect('notify::focus-window', () => this.#onFocus());
    }

    stop(): void {
        if (this.#focusId !== 0) {
            global.display.disconnect(this.#focusId);
            this.#focusId = 0;
        }
        this.#previous = null;
        this.#lentTo = null;
    }

    #onFocus(): void {
        const now = global.display.focus_window;
        if (now === null) return;

        if (!isFloating(now)) {
            this.#previous = now;
            return;
        }
        // A card or pin the pointer is over is allowed to hold it.
        if (now === this.#lentTo) return;

        // Anything else took it uninvited. Hand it back -- to the hovered card if there
        // is one, since the pointer is still there, otherwise to the application.
        const back = isAlive(this.#lentTo) ? this.#lentTo : this.previous;
        if (back === null) {
            // Nothing to hand it back to: an empty session. Taking focus off everything
            // would swallow the user's next keystroke, which is worse than leaving it.
            info('a floating window has the keyboard, and there is nothing else to hold it');
            return;
        }
        back.focus(global.get_current_time());
    }

    /**
     * Lends the keyboard to a hovered card or pin (`FocusWindow(true)`).
     *
     * `spec/04` §3 for cards; `spec/07` §3.1 for pins, whose arrow keys, Esc and Ctrl+W
     * need a keyboard the pin is never allowed to take for itself.
     */
    lend(window: Meta.Window): void {
        this.#lentTo = window;
        window.focus(global.get_current_time());
    }

    /**
     * Gives the keyboard back (`FocusWindow(false)`), returning whether it went anywhere.
     *
     * Restoring rather than merely unfocusing: unfocusing alone would leave the session
     * with no focused window and the user's next keystroke would go nowhere. `spec/04` §3
     * promises that typing in another app is not interrupted, which means the keystroke
     * after the pointer leaves must land where the one before it did.
     */
    restore(): boolean {
        this.#lentTo = null;
        const previous = this.previous;
        if (previous === null) return false;
        previous.focus(global.get_current_time());
        return true;
    }

    /** What the keyboard should go back to. */
    get previous(): Meta.Window | null {
        return isAlive(this.#previous) ? this.#previous : null;
    }

    /**
     * `spec/01` §5's "never focus-steal", checked at a moment it is observable.
     *
     * Asserted rather than trusted, because the failure is silent: a window that took the
     * keyboard does not look any different, and the only symptom is that the user's next
     * keystroke goes somewhere they did not expect. This check is what caught the steal in
     * the first place (`docs/decisions.md` D37) and what caught the race that replaced it.
     *
     * A focused card the pointer is *over* is not a violation -- that one was lent.
     * Neither is one with nothing to hand the keyboard back to, which is an empty session.
     */
    check(context: string): void {
        const focused = global.display.focus_window;
        if (focused === null || !isFloating(focused) || focused === this.#lentTo) return;
        if (this.previous === null) {
            info(`${context}: a floating window holds the keyboard, and nothing else can`);
            return;
        }
        error(
            `${context}: ${focused.get_title()} has the keyboard focus; ` +
            'spec/01 §5 says overlays never take it',
        );
    }
}

/**
 * Whether a window is still on the stage.
 *
 * A card that closed while the pointer was leaving it is the common case, and focusing a
 * destroyed window throws on a path that runs on every pointer-leave.
 *
 * It asks the **window** about its actor and never the actor about itself, which is the
 * whole value of it rather than an implementation detail: Mutter disposes the
 * `MetaWindowActor` when it unmanages a window but leaves the `MetaWindow` answering, so
 * this stays a safe question to ask after the answer has become "no". `whenDrawable` in
 * `dbus.ts` depends on exactly that -- by the time its fallback fires, the actor it
 * captured 500 ms earlier may be gone. `docs/decisions.md` D49.
 */
export function isAlive(window: Meta.Window | null): window is Meta.Window {
    return window !== null && window.get_compositor_private() !== null;
}
