// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * A pet's right-click menu (`spec/14` §10): All-In-One's seven modes in the toolbar's
 * order with the toolbar's icons, then History, Settings and Hide.
 *
 * It takes a modal grab while it is open, like the capture overlay, because it has to
 * have the keyboard -- arrows, Tab, Enter and Escape -- and has to hear a click anywhere
 * else to close. The grab is on a backdrop over the whole stage; the menu sits in it, and
 * a press anywhere but on the menu's own buttons closes it. Everything it creates goes
 * when it closes, and it closes on `destroy()` whatever it was doing.
 *
 * The pill is the toolbar's material and size, so its labels are as readable as the
 * toolbar's. The ring is round buttons of the same material, with a caption that names
 * the one under the pointer or the focus.
 */

import Clutter from 'gi://Clutter';
import Shell from 'gi://Shell';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import { icon } from '../icons.js';
import { error, info } from '../log.js';
import { TOOLBAR_MODES, type ToolbarMode } from '../overlay/toolbar.js';
import type { Rect } from '../place.js';
import { type MenuPlacement, placePill, placeRing } from './menuPlace.js';

/** A rect as the log writes it: `x,y wxh`, whole logical pixels. */
function box(r: Rect): string {
    return `${Math.round(r.x)},${Math.round(r.y)} ${Math.round(r.width)}x${Math.round(r.height)}`;
}

/** What the menu can be asked for. */
export type MenuChoice = ToolbarMode | 'history' | 'settings' | 'hide';

const EXTRAS: readonly { id: MenuChoice; label: string; icon: string }[] = [
    { id: 'history', label: 'History', icon: 'document-open-recent-symbolic' },
    { id: 'settings', label: 'Settings', icon: 'emblem-system-symbolic' },
    { id: 'hide', label: 'Hide pets', icon: 'view-conceal-symbolic' },
];

/** The toolbar's pill (`overlay/toolbar.ts`), for a menu that reads as part of it. */
const PILL_STYLE =
    'background-color: rgba(28,28,30,0.92); ' +
    'border: 1px solid rgba(255,255,255,0.10); ' +
    'border-radius: 14px; ' +
    'padding: 4px; ' +
    'box-shadow: 0 4px 16px rgba(0,0,0,0.35);';
const BUTTON_STYLE = 'padding: 2px 10px; border-radius: 10px; color: rgba(255,255,255,0.92);';
const BUTTON_FOCUS_STYLE = BUTTON_STYLE + 'background-color: rgba(255,255,255,0.16);';
const LABEL_STYLE = 'font-size: 8pt;';
const SEPARATOR_STYLE = 'background-color: rgba(255,255,255,0.14); width: 1px; margin: 6px 4px;';
const RING_BUTTON = 40;
const RING_STYLE =
    `background-color: rgba(28,28,30,0.92); border: 1px solid rgba(255,255,255,0.12); ` +
    `border-radius: ${RING_BUTTON / 2}px; color: rgba(255,255,255,0.92); ` +
    `width: ${RING_BUTTON}px; height: ${RING_BUTTON}px; box-shadow: 0 2px 8px rgba(0,0,0,0.35);`;
const RING_FOCUS_STYLE = RING_STYLE.replace('rgba(28,28,30,0.92)', 'rgba(72,72,78,0.96)');
const CAPTION_STYLE =
    'background-color: rgba(28,28,30,0.92); border-radius: 8px; padding: 3px 10px; ' +
    'color: #ffffff; font-size: 9pt;';
/** Record's red, as on the toolbar's record overlay (`spec/09`). */
const RECORD_STYLE = 'color: #ff5a5f;';

export interface MenuOptions {
    /** The pet's box on the stage, logical. */
    pet: Rect;
    /** The work area of its monitor. */
    area: Rect;
    placement: MenuPlacement;
    chosen(choice: MenuChoice): void;
    /** Closed with nothing chosen, or after a choice; called once either way. */
    closed(side: 'above' | 'below' | 'around'): void;
}

export class PetMenu {
    #backdrop: St.Widget | null;
    #grab: Clutter.Grab | null = null;
    #buttons: St.Button[] = [];
    /** What a press may land on without closing the menu: the pill, or the ring's buttons and caption. */
    #pieces: Clutter.Actor[] = [];
    #caption: St.Label | null = null;
    #closed = false;
    readonly #options: MenuOptions;
    #side: 'above' | 'below' | 'around' = 'above';

    constructor(host: Clutter.Actor, options: MenuOptions) {
        this.#options = options;
        this.#backdrop = new St.Widget({
            name: 'octosnap-pet-menu',
            reactive: true,
            can_focus: true,
            x: 0,
            y: 0,
            width: global.stage.width,
            height: global.stage.height,
            layout_manager: new Clutter.FixedLayout(),
        });
        host.add_child(this.#backdrop);
        // A press anywhere but on the menu closes it, and goes no further, as a click
        // outside any GNOME menu does (D150). Heard on the way down, before a button can
        // take it, and asked of the stage: `Clutter.Event.get_source()` is null for a
        // press on GNOME 50, so the check this replaced never matched, and a menu opened
        // by mistake held the whole desktop until Escape.
        this.#backdrop.connect('captured-event', (_actor, event: Clutter.Event) => {
            const type = event.type();
            if (type !== Clutter.EventType.BUTTON_PRESS && type !== Clutter.EventType.TOUCH_BEGIN)
                return Clutter.EVENT_PROPAGATE;
            if (this.#holds(global.stage.get_event_actor(event))) return Clutter.EVENT_PROPAGATE;
            info('pet menu: closed by a press outside it');
            this.close();
            return Clutter.EVENT_STOP;
        });
        this.#backdrop.connect('key-press-event', (_actor, event: Clutter.Event) => this.#key(event));

        if (options.placement === 'around') this.#buildRing();
        else this.#buildPill(options.placement);

        try {
            this.#grab = Main.pushModal(this.#backdrop, { actionMode: Shell.ActionMode.POPUP });
        } catch (e) {
            error('the pet menu could not take the keyboard', e);
        }
        this.#buttons[0]?.grab_key_focus();
    }

    /** Which side it opened on, for the pet to look at it. */
    get side(): 'above' | 'below' | 'around' {
        return this.#side;
    }

    close(): void {
        if (this.#closed) return;
        this.#closed = true;
        if (this.#grab !== null) {
            Main.popModal(this.#grab);
            this.#grab = null;
        }
        this.#backdrop?.destroy();
        this.#backdrop = null;
        this.#buttons = [];
        this.#pieces = [];
        this.#caption = null;
        this.#options.closed(this.#side);
    }

    destroy(): void {
        this.close();
    }

    /** Whether `actor` is part of the menu itself, rather than the backdrop or anything under it. */
    #holds(actor: Clutter.Actor | null): boolean {
        if (actor === null) return false;
        return this.#pieces.some(piece => piece === actor || piece.contains(actor));
    }

    #choose(choice: MenuChoice): void {
        // Closed first, so the grab is gone before whatever the choice opens takes its own.
        this.close();
        this.#options.chosen(choice);
    }

    #key(event: Clutter.Event): boolean {
        const symbol = event.get_key_symbol();
        if (symbol === Clutter.KEY_Escape) {
            this.close();
            return Clutter.EVENT_STOP;
        }
        const focused = this.#buttons.findIndex(b => b.has_key_focus());
        const step = (by: number) => {
            const n = this.#buttons.length;
            if (n === 0) return;
            const next = this.#buttons[(((focused < 0 ? 0 : focused + by) % n) + n) % n];
            next?.grab_key_focus();
        };
        switch (symbol) {
            case Clutter.KEY_Right:
            case Clutter.KEY_Down:
                step(1);
                return Clutter.EVENT_STOP;
            case Clutter.KEY_Left:
            case Clutter.KEY_Up:
            case Clutter.KEY_ISO_Left_Tab:
                step(-1);
                return Clutter.EVENT_STOP;
            case Clutter.KEY_Tab:
                step((event.get_state() & Clutter.ModifierType.SHIFT_MASK) !== 0 ? -1 : 1);
                return Clutter.EVENT_STOP;
            default:
                return Clutter.EVENT_PROPAGATE;
        }
    }

    #entries(): { id: MenuChoice; label: string; icon: string }[] {
        return [...TOOLBAR_MODES.map(m => ({ id: m.id as MenuChoice, label: m.label, icon: m.icon })), ...EXTRAS];
    }

    #buildPill(placement: 'auto' | 'above' | 'below'): void {
        const pill = new St.BoxLayout({ style: PILL_STYLE, reactive: true });
        const entries = this.#entries();
        entries.forEach((entry, i) => {
            if (i === TOOLBAR_MODES.length) pill.add_child(new St.Widget({ style: SEPARATOR_STYLE }));
            const extra = i >= TOOLBAR_MODES.length;
            const content = new St.BoxLayout({ vertical: true, x_align: Clutter.ActorAlign.CENTER });
            content.add_child(
                new St.Icon({
                    gicon: icon(entry.icon),
                    icon_size: 16,
                    x_align: Clutter.ActorAlign.CENTER,
                    style: entry.id === 'record' ? RECORD_STYLE : '',
                }),
            );
            // The extras are icons alone, as on the prototype the owner approved; their
            // names are the accessible names and the tooltip-like caption.
            if (!extra) content.add_child(new St.Label({ text: entry.label, style: LABEL_STYLE, x_align: Clutter.ActorAlign.CENTER }));
            const button = this.#button(content, entry, BUTTON_STYLE, BUTTON_FOCUS_STYLE);
            if (extra) button.y_align = Clutter.ActorAlign.CENTER;
            pill.add_child(button);
        });
        this.#backdrop?.add_child(pill);
        this.#pieces.push(pill);
        const [, , natural] = pill.get_preferred_size();
        const [, naturalHeight] = pill.get_preferred_height(natural);
        const place = placePill(this.#options.pet, { width: natural, height: naturalHeight }, this.#options.area, placement);
        pill.set_position(place.x, place.y);
        this.#side = place.side;
        info(
            `pet menu: ${placement} opened ${place.side}, ${box({ x: place.x, y: place.y, width: natural, height: naturalHeight })}; ` +
                `the pet ${box(this.#options.pet)} in ${box(this.#options.area)}`,
        );
    }

    #buildRing(): void {
        const entries = this.#entries().slice(0, TOOLBAR_MODES.length);
        const caption = new St.Label({ text: 'Choose a capture', style: CAPTION_STYLE });
        this.#backdrop?.add_child(caption);
        this.#pieces.push(caption);
        const [, captionWidth] = caption.get_preferred_width(-1);
        const [, captionHeight] = caption.get_preferred_height(captionWidth);
        const ring = placeRing(this.#options.pet, entries.length, RING_BUTTON, this.#options.area, {
            width: captionWidth + 40,
            height: captionHeight,
        });
        entries.forEach((entry, i) => {
            const centre = ring.centres[i];
            if (centre === undefined) return;
            const content = new St.Icon({
                gicon: icon(entry.icon),
                icon_size: 18,
                style: entry.id === 'record' ? RECORD_STYLE : '',
            });
            const button = this.#button(content, entry, RING_STYLE, RING_FOCUS_STYLE);
            this.#backdrop?.add_child(button);
            this.#pieces.push(button);
            button.set_position(centre.x - RING_BUTTON / 2, centre.y - RING_BUTTON / 2);
        });
        caption.set_position(ring.caption.x + 20, ring.caption.y);
        this.#caption = caption;
        this.#side = 'around';
        const buttons = ring.centres.map(c =>
            box({ x: c.x - RING_BUTTON / 2, y: c.y - RING_BUTTON / 2, width: RING_BUTTON, height: RING_BUTTON }),
        );
        info(`pet menu: around opened as ${buttons.join('; ')}; the pet ${box(this.#options.pet)} in ${box(this.#options.area)}`);
    }

    #button(child: Clutter.Actor, entry: { id: MenuChoice; label: string }, style: string, focus: string): St.Button {
        const button = new St.Button({
            child,
            style,
            can_focus: true,
            track_hover: true,
            reactive: true,
            accessible_name: entry.label,
        });
        const lit = () => {
            const on = button.hover || button.has_key_focus();
            button.style = on ? focus : style;
            if (on && this.#caption !== null) this.#caption.text = entry.label;
        };
        button.connect('notify::hover', lit);
        button.connect('key-focus-in', lit);
        button.connect('key-focus-out', lit);
        button.connect('clicked', () => this.#choose(entry.id));
        this.#buttons.push(button);
        return button;
    }
}
