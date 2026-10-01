// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * `CAP-06`: the self-timer countdown, and `spec/10` §3.1's `ShowCountdown`.
 *
 * `spec/03` §5.4: the selection is made first, then the overlay disappears and a
 * countdown runs over the *live* screen, so the user can arrange whatever they are
 * capturing. That last part is why this is not part of the selection overlay -- it has to
 * outlive it, and it must not take a modal grab, or the user could not touch the thing
 * they are counting down to photograph.
 *
 * No grab, so Esc is taken on its own instead: an accelerator held for as long as the
 * numbers run and given back the moment they stop (`spec/03` §5.4: "Esc cancels"), with
 * the hint `spec/03` §2 puts under the ring. The first version relied on a
 * `CancelCountdown` that was never built, and nothing could stop a countdown once it had
 * started (D130). Esc can be someone else's already, so the panel menu offers Cancel
 * Countdown as well, and the hint says so when it is the only way (`activeCountdown`).
 *
 * The selection stays visible behind a dim lightened to 15 % (§5.4), light enough to say
 * the screen underneath is still the user's, which it is: nothing here is reactive.
 *
 * What it looks like is `spec/09` §2.1's `countdown` token: a 120 px numeral inside a
 * 160 px ring with a 6 px stroke, the accent on a faint white track. The ring is the time
 * left and drains as it goes; each new number lands at 1.2 times its size and settles
 * (`spec/09` §3, D129). With animations off the ring steps once a second instead of
 * sweeping, and the numbers simply change.
 */

import Cairo from 'cairo';
import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import St from 'gi://St';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import { createOverlayHost } from './host.js';
import { type Rect, dimPieces } from './selection.js';
import { COUNTDOWN_FROM_SCALE, COUNTDOWN_TICK, easing } from './motion.js';
import { error, info } from '../log.js';
import { tellPets } from '../pets/events.js';
import { playTick } from '../sound.js';
import type { ShutterSound } from '../soundCues.js';

/** `spec/09` §2.1: the ring's diameter and stroke. */
const RING = 160;
const STROKE = 6;
const CENTRE = RING / 2;
const RADIUS = (RING - STROKE) / 2;
/** Where the time left starts from: twelve o'clock. */
const TOP = -Math.PI / 2;

/**
 * The numeral: 120 px white, over a dark disc so it is legible over anything. Two digits
 * at 120 px would run into the ring, so a ten is set smaller.
 */
const NUMERAL_STYLE =
    'color: #ffffff; ' +
    'font-weight: bold; ' +
    'font-feature-settings: "tnum"; ' +
    'text-align: center;';
const NUMERAL_PX = 120;
const NUMERAL_PX_TWO_DIGITS = 92;

const DISC_STYLE = `background-color: rgba(0,0,0,0.55); border-radius: ${RING / 2}px;`;

/** `spec/03` §5.4: the dim "lightens to 15 %" while the numbers run. */
const DIM_STYLE = 'background-color: rgba(0,0,0,0.15);';

/** `spec/03` §2's "Esc to cancel", on a pill of the disc's dark so it reads over anything. */
const HINT_TEXT = 'Esc to cancel';
/** When Esc is another accelerator's: the panel menu's Cancel Countdown is the way. */
const HINT_TEXT_NO_ESCAPE = 'Cancel it from the OctoSnap menu';

/** The countdown on screen, if one is, for the panel menu's Cancel Countdown. */
let active: Countdown | null = null;

/** The countdown running now, or `null`. There is one screen, so there is one at most. */
export function activeCountdown(): Countdown | null {
    return active;
}
const HINT_STYLE =
    'color: #ffffff; ' +
    'font-size: 13px; ' +
    'background-color: rgba(0,0,0,0.55); ' +
    'border-radius: 999px; ' +
    'padding: 4px 10px;';
const HINT_GAP = 12;
/** The ring and the hint share a column this wide, centred on the ring. */
const COLUMN = RING + 120;

/** `spec/09` §2.1's track, and the accent's fallback, Adwaita blue. */
const TRACK = [1, 1, 1, 0.2] as const;
const ACCENT_FALLBACK = [0x35 / 255, 0x84 / 255, 0xe4 / 255, 1] as const;

export class Countdown {
    /** See `host.ts`: `screenshotUIGroup` cannot lay a positioned actor out. */
    #host: St.Widget | null = null;
    /** The ring over the hint, which is what is placed. */
    #column: St.BoxLayout | null = null;
    #group: St.Widget | null = null;
    #ring: St.DrawingArea | null = null;
    #label: St.Label | null = null;
    /** The ring's sweep, when it sweeps; `null` with animations off. */
    #timeline: Clutter.Timeline | null = null;
    #timer = 0;
    #total: number;
    #remaining: number;
    #accent: readonly [number, number, number, number] = ACCENT_FALLBACK;
    /** Whether the ring has been painted yet, which is when it is known to be on screen. */
    #shown = false;
    /** Esc's accelerator while it is held, and the handler that hears it. */
    #escape: number = Meta.KeyBindingAction.NONE;
    #escapeHandler = 0;
    #resolve: ((completed: boolean) => void) | null = null;
    #settled = false;
    /** `capture-sound`, which says whether each number ticks (`sound.ts`). */
    readonly #shutter: ShutterSound;

    constructor(seconds: number, shutter: ShutterSound) {
        // A zero or negative timer is a capture with no countdown, not an error, and
        // `spec/08` §6's own default is 5.
        this.#remaining = Math.max(0, Math.trunc(seconds));
        this.#total = this.#remaining;
        this.#shutter = shutter;
    }

    /**
     * Runs the countdown. Resolves `true` when it finished, `false` when cancelled.
     *
     * The numeral is placed over `rect` when there is one -- so it sits on what is about
     * to be captured -- and centred on the pointer's monitor otherwise, which is the
     * fullscreen case.
     */
    run(rect: Rect | null): Promise<boolean> {
        return new Promise(resolve => {
            this.#resolve = resolve;

            if (this.#remaining === 0) {
                this.#settle(true);
                return;
            }

            this.#accent = accentColour();
            // Not reactive, any of it: the whole point is that the user can keep using
            // the screen underneath while the timer runs.
            const group = new St.Widget({
                width: RING,
                height: RING,
                reactive: false,
                x_align: Clutter.ActorAlign.CENTER,
                layout_manager: new Clutter.BinLayout(),
            });
            group.add_child(new St.Widget({ style: DISC_STYLE, width: RING, height: RING }));

            const ring = new St.DrawingArea({ width: RING, height: RING });
            ring.connect('repaint', () => this.#paintRing());
            group.add_child(ring);

            const label = new St.Label({
                style: this.#numeralStyle(),
                text: String(this.#remaining),
                x_align: Clutter.ActorAlign.CENTER,
                y_align: Clutter.ActorAlign.CENTER,
            });
            // Scaled about its own centre, so a number settles where it stands.
            label.set_pivot_point(0.5, 0.5);
            group.add_child(label);

            const column = new St.BoxLayout({
                orientation: Clutter.Orientation.VERTICAL,
                style: `spacing: ${HINT_GAP}px;`,
                width: COLUMN,
                reactive: false,
            });
            column.add_child(group);
            const hint = new St.Label({
                style: HINT_STYLE,
                text: HINT_TEXT,
                x_align: Clutter.ActorAlign.CENTER,
            });
            column.add_child(hint);

            // Through a host, like every overlay: added to `screenshotUIGroup` itself, the
            // ring was given a box of no size anywhere but the stage's origin, and the
            // countdown ran with nothing on screen (D130, D24).
            this.#host = createOverlayHost('octosnap-countdown');
            this.#dim(rect);
            this.#host.add_child(column);
            this.#column = column;
            this.#group = group;
            this.#ring = ring;
            this.#label = label;
            this.#place(rect);
            this.#takeEscape();
            if (!this.cancelsOnEscape) hint.text = HINT_TEXT_NO_ESCAPE;
            active = this;

            // The sweep is one timeline for the whole countdown, so the ring moves on the
            // frame clock and a late timer tick does not show as a jump.
            if (St.Settings.get().enable_animations) {
                const timeline = new Clutter.Timeline({
                    actor: ring,
                    duration: this.#total * 1000,
                });
                timeline.connect('new-frame', () => ring.queue_repaint());
                timeline.start();
                this.#timeline = timeline;
            }
            this.#land();
            playTick(this.#shutter);
            tellPets({ kind: 'tick', remaining: this.#remaining });

            info(`countdown started at ${this.#remaining}s`);
            // `timeout_add`, not `timeout_add_seconds`: the seconds variant fires on the
            // clock's whole-second boundaries so that timers can share a wakeup, which
            // made a three-second countdown last anything from 2.85 to 3.24 s and put the
            // numbers out of step with the ring's sweep.
            this.#timer = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 1000, () => {
                this.#remaining -= 1;
                if (this.#remaining <= 0) {
                    this.#timer = 0;
                    this.#settle(true);
                    return GLib.SOURCE_REMOVE;
                }
                if (this.#label !== null) {
                    this.#label.text = String(this.#remaining);
                    this.#label.set_style(this.#numeralStyle());
                    this.#land();
                }
                playTick(this.#shutter);
                tellPets({ kind: 'tick', remaining: this.#remaining });
                if (this.#timeline === null) this.#ring?.queue_repaint();
                return GLib.SOURCE_CONTINUE;
            });
        });
    }

    /** "Countdown number | 200 ms per tick | ease-out-cubic | scale 1.2→1". */
    #land(): void {
        const label = this.#label;
        if (label === null) return;
        label.remove_all_transitions();
        label.set_scale(COUNTDOWN_FROM_SCALE, COUNTDOWN_FROM_SCALE);
        label.ease(
            easing({
                scale_x: 1,
                scale_y: 1,
                duration: COUNTDOWN_TICK.duration,
                mode: COUNTDOWN_TICK.mode,
            }),
        );
    }

    #numeralStyle(): string {
        const px = this.#remaining >= 10 ? NUMERAL_PX_TWO_DIGITS : NUMERAL_PX;
        return `${NUMERAL_STYLE} font-size: ${px}px;`;
    }

    /**
     * The track, then the accent over the part of it that is time still to go, from the
     * top, clockwise. The time left is the timeline's while it sweeps and the whole
     * seconds left while it steps.
     */
    #paintRing(): void {
        const ring = this.#ring;
        if (ring === null) return;
        // In logical pixels: the drawing area's surface carries the monitor's scale
        // itself, so the ring is as crisp at 2x as at 1x. Nothing is built here beyond
        // the context, since at 60 frames a second this is a hot path (`spec/10` §7).
        const cr = ring.get_context();
        if (!this.#shown) this.#reportShown();
        try {
            cr.setLineWidth(STROKE);
            cr.setSourceRGBA(...TRACK);
            cr.arc(CENTRE, CENTRE, RADIUS, 0, 2 * Math.PI);
            cr.stroke();

            const left = this.#timeline !== null
                ? 1 - this.#timeline.get_progress()
                : this.#remaining / Math.max(1, this.#total);
            if (left > 0) {
                cr.setLineCap(Cairo.LineCap.ROUND);
                cr.setSourceRGBA(...this.#accent);
                cr.arc(CENTRE, CENTRE, RADIUS, TOP, TOP + 2 * Math.PI * left);
                cr.stroke();
            }
        } catch (e) {
            error('could not paint the countdown ring', e);
        } finally {
            // Handed back now rather than at the next collection: a repaint a frame for
            // ten seconds would otherwise leave a few hundred contexts to the GC.
            (cr as unknown as { $dispose(): void }).$dispose();
        }
    }

    /**
     * Where the countdown landed on the stage, once. A countdown that runs with nothing on
     * screen is otherwise indistinguishable from one that shows (D24's zero-sized box did
     * exactly that), so the harness reads this line rather than trusting the start.
     */
    #reportShown(): void {
        this.#shown = true;
        const box = this.#group?.get_transformed_extents();
        if (box === undefined) return;
        info(
            `countdown on screen at ${Math.round(box.origin.x)},${Math.round(box.origin.y)} ` +
            `${Math.round(box.size.width)}x${Math.round(box.size.height)}`,
        );
    }

    /** `spec/03` §5.4's lightened dim, on every monitor, around a selection. */
    #dim(rect: Rect | null): void {
        const host = this.#host;
        if (host === null || rect === null || rect.width <= 0 || rect.height <= 0) return;
        for (const monitor of Main.layoutManager.monitors) {
            const pieces = dimPieces(
                { x: monitor.x, y: monitor.y, width: monitor.width, height: monitor.height },
                rect,
            );
            for (const piece of pieces) {
                if (piece.width <= 0 || piece.height <= 0) continue;
                host.add_child(new St.Widget({
                    style: DIM_STYLE,
                    x: monitor.x + piece.x,
                    y: monitor.y + piece.y,
                    width: piece.width,
                    height: piece.height,
                    reactive: false,
                }));
            }
        }
    }

    /** Puts the ring's centre on the selection's, or on the pointer's monitor's. */
    #place(rect: Rect | null): void {
        if (this.#column === null) return;

        let centre: { x: number; y: number };
        if (rect !== null && rect.width > 0 && rect.height > 0) {
            centre = { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 };
        } else {
            const index = global.display.get_current_monitor();
            const monitor = Main.layoutManager.monitors[index];
            if (monitor === undefined) return;
            centre = { x: monitor.x + monitor.width / 2, y: monitor.y + monitor.height / 2 };
        }
        this.#column.set_position(
            Math.round(centre.x - COLUMN / 2),
            Math.round(centre.y - RING / 2),
        );
    }

    /**
     * Holds Esc for as long as the numbers run. An accelerator rather than a grab, so
     * every other key and the pointer still reach the windows underneath; allowed in the
     * normal action mode only, so a shell menu or the overview opened during the countdown
     * still closes on Esc as it always does.
     */
    #takeEscape(): void {
        try {
            const action = global.display.grab_accelerator('Escape', Meta.KeyBindingFlags.NONE);
            if (action === Meta.KeyBindingAction.NONE) {
                info('Esc is already taken, so the countdown cannot be cancelled from the keyboard');
                return;
            }
            this.#escape = action;
            Main.wm.allowKeybinding(
                Meta.external_binding_name_for_action(action),
                Shell.ActionMode.NORMAL,
            );
            this.#escapeHandler = global.display.connect(
                'accelerator-activated',
                (_display: Meta.Display, activated: number) => {
                    if (activated !== this.#escape) return;
                    info('countdown cancelled with Esc');
                    this.cancel();
                },
            );
        } catch (e) {
            error('could not take Esc for the countdown', e);
        }
    }

    #giveEscapeBack(): void {
        if (this.#escapeHandler !== 0) {
            global.display.disconnect(this.#escapeHandler);
            this.#escapeHandler = 0;
        }
        if (this.#escape === Meta.KeyBindingAction.NONE) return;
        try {
            Main.wm.allowKeybinding(
                Meta.external_binding_name_for_action(this.#escape),
                Shell.ActionMode.NONE,
            );
            global.display.ungrab_accelerator(this.#escape);
        } catch (e) {
            error('could not give Esc back after the countdown', e);
        }
        this.#escape = Meta.KeyBindingAction.NONE;
    }

    /** Whether Esc cancels this one, which is what the panel menu's hint says. */
    get cancelsOnEscape(): boolean {
        return this.#escape !== Meta.KeyBindingAction.NONE;
    }

    /** Stops the numbers without capturing: Esc, and the panel menu's Cancel Countdown. */
    cancel(): void {
        this.#settle(false);
    }

    #settle(completed: boolean): void {
        if (this.#settled) return;
        this.#settled = true;
        if (active === this) active = null;
        // First: Esc belongs to the windows again the moment the numbers stop.
        this.#giveEscapeBack();
        if (this.#timer !== 0) {
            GLib.source_remove(this.#timer);
            this.#timer = 0;
        }
        this.#timeline?.stop();
        const resolve = this.#resolve;
        this.#resolve = null;
        resolve?.(completed);
    }

    /**
     * Always safe to call twice, and always called from a `finally`: a leaked timeout
     * would keep firing into a destroyed actor after the extension is disabled, which is
     * the kind of leak that survives until logout (`spec/03` §12 item 6).
     */
    destroy(): void {
        if (active === this) active = null;
        this.#settle(false);
        this.#timeline = null;
        this.#host?.destroy();
        this.#host = null;
        this.#column = null;
        this.#group = null;
        this.#ring = null;
        this.#label = null;
    }
}

/**
 * The desktop's accent, as cairo takes a colour. `-st-accent-color` cannot reach a cairo
 * drawing, so it is asked of the theme context; a shell that cannot answer gets Adwaita
 * blue, which is the accent's own default.
 */
function accentColour(): readonly [number, number, number, number] {
    try {
        const [accent] = St.ThemeContext.get_for_stage(global.stage).get_accent_color();
        if (accent !== null)
            return [accent.red / 255, accent.green / 255, accent.blue / 255, accent.alpha / 255];
    } catch (e) {
        error('could not read the accent colour; the countdown ring uses Adwaita blue', e);
    }
    return ACCENT_FALLBACK;
}
