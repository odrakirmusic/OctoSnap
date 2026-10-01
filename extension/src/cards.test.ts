// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * `isAlive`, which is the whole of the guard in `dbus.ts`'s `whenDrawable` and the
 * pointer-leave path in this file.
 *
 * Only this much of `cards.ts` is testable under Node: its `gi://Meta` import is
 * type-only and so erased, while `dbus.ts` -- where the bug in `docs/decisions.md` D49
 * actually lived -- imports six gi modules as values and cannot be loaded here at all.
 * `whenDrawable`'s sequencing is therefore checked in a nested shell, and what is checked
 * here is the predicate it leans on.
 *
 * The interesting case is the third one. A disposed GObject is not merely a wrapper that
 * answers `null`; touching it at all is the fault being guarded against, and the stub
 * below is a proxy that throws on any access for that reason. A guard that reached into
 * the actor to decide would pass the first two cases and fail this one, which is exactly
 * the mistake the real code made.
 */

import { describe, expect, it } from 'vitest';

import { CARD_TITLE_PREFIX, PIN_TITLE_PREFIX, isAlive, isCard, isFloating } from './cards.js';

type WindowLike = Parameters<typeof isAlive>[0];

/** A window Mutter is compositing, with an actor nobody is allowed to look inside. */
function live(): WindowLike {
    const actor = new Proxy(
        {},
        {
            get(_target, prop) {
                throw new Error(`touched the actor (${String(prop)}); it may be disposed`);
            },
        },
    );
    return { get_compositor_private: () => actor } as unknown as WindowLike;
}

/** A window Mutter has unmanaged: the `MetaWindow` answers, its actor is gone. */
function unmanaged(): WindowLike {
    return { get_compositor_private: () => null } as unknown as WindowLike;
}

describe('isAlive', () => {
    it('says no to a window that is not there', () => {
        expect(isAlive(null)).toBe(false);
    });

    it('says no once Mutter has unmanaged the window', () => {
        // The 500 ms fallback in `whenDrawable` outliving the card it was waiting for.
        expect(isAlive(unmanaged())).toBe(false);
    });

    it('says yes without touching the actor', () => {
        // Asking the actor anything is the bug: GJS logs `has been already disposed` and
        // then calls into C anyway, and neither complaint can be caught. So the question
        // has to be answerable from the window alone.
        expect(isAlive(live())).toBe(true);
    });
});

describe('the titles a floating window is recognised by', () => {
    it('tells a card from a pin, and both from an ordinary window', () => {
        const at = (title: string): WindowLike =>
            ({ get_title: () => title }) as unknown as WindowLike;

        const card = at(`${CARD_TITLE_PREFIX}01J`);
        const pin = at(`${PIN_TITLE_PREFIX}01J`);
        const other = at('Firefox');

        expect([isFloating(card), isFloating(pin), isFloating(other)]).toEqual([
            true,
            true,
            false,
        ]);
        // Only a card is a card: `spec/07` pins are floating but are not stack members.
        expect([isCard(card), isCard(pin), isCard(other)]).toEqual([true, false, false]);
    });

    it('survives a window with no title yet', () => {
        // A window can be found before it has one, and this runs on every focus change.
        const untitled = { get_title: () => null } as unknown as WindowLike;
        expect(isFloating(untitled)).toBe(false);
        expect(isCard(untitled)).toBe(false);
    });
});
