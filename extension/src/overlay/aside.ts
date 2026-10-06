// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * OctoSnap's own cards, out of the way while the user picks an area (D162).
 *
 * From hardware: "when choosing a zone to create a screenshot, the preview cards should
 * obviously disappear." They stood in the corner through every selection, over the dim,
 * in the frozen frame and in the loupe, and a selection that reached the corner took
 * them into the picture. A card is a handle to a capture already made; nobody chooses an
 * area in order to photograph one.
 *
 * The window **actors** are hidden, as `DesktopIcons` hides DING's, and not the windows:
 * the app is not asked, so nothing waits on the bus before the freeze, and nothing about
 * a hidden actor outlives the shell or this extension. The overlay holds one of these
 * from before its snapshot until `destroy`, so the frozen frame, the loupe and the pixels
 * are all read without the cards, and every path out of a selection -- confirm, cancel,
 * `disable()` -- puts them back. A card the app maps while the overlay is up (the last
 * capture's, arriving late) is hidden as it maps.
 *
 * Mutter shows a window actor again on its own occasions, so each one's `visible` is
 * watched for as long as the cards are aside.
 */

import GLib from 'gi://GLib';
import type Meta from 'gi://Meta';

import { isCard } from '../cards.js';
import { info } from '../log.js';

interface Watch {
    visible: number;
    destroy: number;
}

export class CardsAside {
    #watched = new Map<Meta.WindowActor, Watch>();
    #createdId = 0;
    #sweepIdle = 0;
    #restored = false;

    constructor() {
        this.#sweep();
        this.#createdId = global.display.connect('window-created', () => {
            if (this.#sweepIdle !== 0) return;
            // The actor exists a moment after the window does.
            this.#sweepIdle = GLib.idle_add(GLib.PRIORITY_DEFAULT, () => {
                this.#sweepIdle = 0;
                this.#sweep();
                return GLib.SOURCE_REMOVE;
            });
        });
        if (this.#watched.size > 0) info(`${this.#watched.size} card(s) aside for a selection`);
    }

    /** Every card back as it was. Safe to call twice. */
    restore(): void {
        if (this.#restored) return;
        this.#restored = true;
        if (this.#createdId !== 0) {
            global.display.disconnect(this.#createdId);
            this.#createdId = 0;
        }
        if (this.#sweepIdle !== 0) {
            GLib.source_remove(this.#sweepIdle);
            this.#sweepIdle = 0;
        }
        for (const [actor, watch] of this.#watched) {
            actor.disconnect(watch.visible);
            actor.disconnect(watch.destroy);
            actor.show();
        }
        this.#watched.clear();
    }

    #sweep(): void {
        if (this.#restored) return;
        for (const actor of global.get_window_actors()) {
            if (this.#watched.has(actor)) continue;
            const window = actor.get_meta_window();
            if (!window || !isCard(window)) continue;
            actor.hide();
            const visible = actor.connect('notify::visible', () => {
                if (actor.visible) actor.hide();
            });
            // A card the app closes meanwhile takes its actor with it, and a disposed actor
            // must not be touched again at `restore`.
            const destroy = actor.connect('destroy', () => this.#watched.delete(actor));
            this.#watched.set(actor, { visible, destroy });
        }
    }
}
