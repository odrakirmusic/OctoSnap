// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * `CAP-01`'s All-In-One toolbar: `spec/03` §4's two pills.
 *
 * The layout is verified rather than designed -- `[V] overlay-all-in-one.png`, read in
 * `spec/03` §4 -- so it is followed exactly: two dark translucent pills side by side,
 * ~44 pt tall with a ~14 radius; the left one holding seven mode buttons as an icon above
 * a label, in the order Area, Fullscreen, Window, Scrolling, Timer, OCR, Recording; the
 * right one holding the width field, a `×`, the height field, an aspect-lock toggle and a
 * ratio chevron. The active mode is a **lighter fill**, not an accent colour, and there is
 * deliberately **no Capture button**: Enter or a click confirms.
 *
 * All seven modes are built now. While some were not, those were shown dimmed and inert
 * rather than omitted -- the opposite of the panel menu's choice, and for a reason: the
 * panel menu is a list where an absent row costs nothing, while this is a fixed
 * seven-button layout that the specification pins down, and removing buttons would have
 * changed the shape of the thing being built. The `ready` flag below still does that, for
 * a mode a build cannot serve.
 *
 * The dimension fields are the interesting part. `spec/03` §4 records a `[P→V]`
 * correction: an earlier draft had a floating pill for the readout, and the real product
 * puts **editable** W and H fields in the toolbar. So these accept typed values and
 * anchor the resize at the top-left, and the floating pill survives only for the classic
 * modes, which have no toolbar.
 */

import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import St from 'gi://St';

import type { Rect } from './selection.js';
import {
    GIF_FPS_CHOICES,
    GIF_QUALITY_CHOICES,
    GIF_WIDTH_CHOICES,
    SCREEN_RATE,
    type GifOverrides,
    type ViewRate,
    choiceIndex,
    fpsChoiceLabels,
    fpsLabel,
    qualityLabel,
    recordedFps,
    refreshAt,
    widthLabel,
} from '../recordChoices.js';
import { icon } from '../icons.js';
import { error, info } from '../log.js';

/** `spec/03` §4: two dark translucent pills, ~44 pt tall, radius ~14. */
const PILL_STYLE =
    'background-color: rgba(28,28,30,0.88); ' +
    'border: 1px solid rgba(255,255,255,0.10); ' +
    'border-radius: 14px; ' +
    'padding: 4px; ' +
    'box-shadow: 0 4px 16px rgba(0,0,0,0.35);';

const PILL_HEIGHT = 44;

/** The active mode is a lighter fill behind the button, not an accent colour. */
const MODE_BUTTON_STYLE =
    'padding: 2px 10px; ' +
    'border-radius: 10px; ' +
    'color: rgba(255,255,255,0.92);';

const MODE_BUTTON_ACTIVE_STYLE =
    MODE_BUTTON_STYLE + 'background-color: rgba(255,255,255,0.16);';

const MODE_BUTTON_DISABLED_STYLE =
    'padding: 2px 10px; border-radius: 10px; color: rgba(255,255,255,0.32);';

const MODE_LABEL_STYLE = 'font-size: 8pt;';

const FIELD_STYLE =
    'background-color: rgba(255,255,255,0.10); ' +
    'border-radius: 8px; ' +
    'color: #ffffff; ' +
    'font-size: 11pt; ' +
    'font-feature-settings: "tnum"; ' +
    'padding: 2px 6px; ' +
    'min-width: 52px;';

const FIELD_FOCUS_STYLE =
    FIELD_STYLE.replace(
        'background-color: rgba(255,255,255,0.10);',
        'background-color: rgba(255,255,255,0.22);',
    );

const TIMES_STYLE = 'color: rgba(255,255,255,0.55); font-size: 11pt; padding: 0 4px;';

/** The ratio list: the same pill material as the toolbar, so it reads as part of it. */
const MENU_STYLE =
    'background-color: rgba(28,28,30,0.96); ' +
    'border: 1px solid rgba(255,255,255,0.10); ' +
    'border-radius: 10px; ' +
    'padding: 4px; ' +
    'box-shadow: 0 6px 20px rgba(0,0,0,0.45);';

const MENU_ITEM_STYLE =
    'padding: 4px 14px; border-radius: 6px; font-size: 10pt; color: rgba(255,255,255,0.85);';
const MENU_ITEM_ACTIVE_STYLE =
    'padding: 4px 14px; border-radius: 6px; font-size: 10pt; color: #ffffff; ' +
    'background-color: rgba(255,255,255,0.16);';

const TOGGLE_STYLE = 'padding: 4px 8px; border-radius: 8px; color: rgba(255,255,255,0.75);';
const TOGGLE_ON_STYLE =
    'padding: 4px 8px; border-radius: 8px; color: #ffffff; ' +
    'background-color: rgba(255,255,255,0.16);';

/** Gap between the selection and the toolbar, and between the two pills. */
const GAP = 10;

/**
 * `spec/03` §4's seven modes, in the order the photograph shows them.
 *
 * `mode-*` are OctoSnap's own (`extension/icons/`, `spec/09` §4b). Adwaita's nearest were
 * a tick in a circle for Area, "go to the bottom" for Scrolling and a magnifier, which
 * says "find", for Text. The other four are Adwaita's and say what they mean.
 */
export const TOOLBAR_MODES = [
    { id: 'area', label: 'Area', icon: 'mode-area-symbolic', ready: true },
    { id: 'fullscreen', label: 'Fullscreen', icon: 'video-display-symbolic', ready: true },
    { id: 'window', label: 'Window', icon: 'focus-windows-symbolic', ready: true },
    { id: 'scrolling', label: 'Scrolling', icon: 'mode-scrolling-symbolic', ready: true },
    { id: 'self-timer', label: 'Timer', icon: 'alarm-symbolic', ready: true },
    { id: 'ocr', label: 'Text', icon: 'mode-text-symbolic', ready: true },
    { id: 'record', label: 'Record', icon: 'media-record-symbolic', ready: true },
] as const;

export type ToolbarMode = (typeof TOOLBAR_MODES)[number]['id'];

/** `spec/03` §5.1's ratio menu. `null` is "free". */
export const RATIOS: readonly { label: string; value: number | null }[] = [
    { label: 'Free', value: null },
    { label: '1:1', value: 1 },
    { label: '4:3', value: 4 / 3 },
    { label: '3:2', value: 3 / 2 },
    { label: '16:9', value: 16 / 9 },
    { label: '16:10', value: 16 / 10 },
];

/** An actor's box on the stage as the log writes it: `x,y wxh`, whole logical pixels. */
function boxOf(actor: Clutter.Actor): string {
    const [x, y] = actor.get_transformed_position();
    const [w, h] = actor.get_transformed_size();
    return `${Math.round(x)},${Math.round(y)} ${Math.round(w)}x${Math.round(h)}`;
}

/** Whether a stage point falls inside an actor's transformed bounds. */
function within(actor: Clutter.Actor, x: number, y: number): boolean {
    const [ax, ay] = actor.get_transformed_position();
    const [aw, ah] = actor.get_transformed_size();
    return x >= ax && x < ax + aw && y >= ay && y < ay + ah;
}

function clamp(value: number, low: number, high: number): number {
    return Math.max(low, Math.min(value, Math.max(low, high)));
}

/** Enough of a monitor to place against: geometry plus the scale the label reports in. */
export interface ToolbarMonitor {
    x: number;
    y: number;
    width: number;
    height: number;
    scale: number;
}

export interface ToolbarHandlers {
    /** A mode button was pressed. */
    modeChanged(mode: ToolbarMode): void;
    /** A dimension field was committed. Values are **physical** pixels, as displayed. */
    sizeTyped(width: number | null, height: number | null): void;
    /** The aspect-lock toggle changed. */
    lockChanged(locked: boolean): void;
    /** A ratio was chosen from the chevron menu. */
    ratioChosen(ratio: number | null): void;
    /**
     * `spec/06` §2 from the All-In-One toolbar: the recording row was touched. What
     * arrives is only what the user changed; a value shown and left alone is the app's
     * own setting and is not sent (D101).
     */
    recordOptionsChanged(overrides: GifOverrides): void;
}

/**
 * The toolbar for one monitor. Only the monitor holding the pointer gets one
 * (`spec/03` §2).
 */
export class Toolbar {
    readonly actor: St.BoxLayout;

    #handlers: ToolbarHandlers;
    #monitor: ToolbarMonitor;
    #scale: number;

    #modeButtons = new Map<ToolbarMode, St.Button>();
    #width: St.Entry;
    #height: St.Entry;
    #lock: St.Button;
    #ratio: St.Button;
    #ratioIndex = 0;
    #ratioLabel: St.Label | null = null;
    /** The one open list -- ratio, frame rate, width or quality -- or `null`. */
    #menu: St.BoxLayout | null = null;
    /** The button the open list belongs to, so it can be placed and toggled. */
    #menuAnchor: St.Button | null = null;
    #locked = false;

    /** `spec/06` §2's settings row, shown under the pills while the mode is Record. */
    #recordBar: St.BoxLayout;
    /** What the row shows: the app's defaults, then whatever the user picked. */
    #gif: Required<GifOverrides>;
    /** Which of the four the user has touched, which is exactly what gets sent. */
    #touched = new Set<keyof GifOverrides>();
    /** Where each stage view starts and how fast it refreshes (`capture.ts`' `viewRates`). */
    #rates: readonly ViewRate[];
    /** The refresh of the monitor the toolbar is on, in hertz; `null` when not known. */
    #hz: number | null;
    #fps: St.Button | null = null;
    #widthButton: St.Button | null = null;
    #qualityButton: St.Button | null = null;
    #fpsLabel: St.Label | null = null;
    #widthLabel: St.Label | null = null;
    #qualityLabel: St.Label | null = null;
    #cursor: St.Button | null = null;
    /** The last selection placed against, so a row appearing can re-place the toolbar. */
    #lastSelection: Rect | null = null;
    /** Where the list buttons were last said to be, and the wait for a layout to say it. */
    #controlsSaid = '';
    #controlsIdle = 0;

    constructor(
        parent: Clutter.Actor,
        monitor: ToolbarMonitor,
        scale: number,
        activeMode: ToolbarMode,
        gifDefaults: Required<GifOverrides>,
        rates: readonly ViewRate[],
        handlers: ToolbarHandlers,
    ) {
        this.#handlers = handlers;
        this.#monitor = monitor;
        this.#scale = scale;
        this.#gif = { ...gifDefaults };
        this.#rates = rates;
        this.#hz = refreshAt(rates, monitor.x, monitor.y);
        this.#logScreenRate();

        // Two rows: the pills, and under them the recording row, which is only there in
        // Record mode. A column rather than a third pill on the same line, because the
        // line is already two pills wide and a third would not fit a laptop panel beside
        // a wide selection -- and because `spec/06` §2 draws its settings as a row *under*
        // the size fields, not beside them.
        this.actor = new St.BoxLayout({
            reactive: true,
            vertical: true,
            style: `spacing: ${GAP}px;`,
            visible: false,
        });
        // Over the toolbar the pointer is a pointer, not a crosshair (`spec/03` §2).
        this.actor.set_cursor_type(Clutter.CursorType.DEFAULT);

        // Laid out horizontally: the two pills side by side.
        const pills = new St.BoxLayout({ style: `spacing: ${GAP}px;` });
        pills.add_child(this.#buildModePill(activeMode));

        const right = new St.BoxLayout({ style: PILL_STYLE, height: PILL_HEIGHT });
        this.#width = this.#buildField('width');
        this.#height = this.#buildField('height');
        this.#lock = this.#buildLock();
        this.#ratio = this.#buildRatio();
        right.add_child(this.#width);
        right.add_child(new St.Label({ text: '×', style: TIMES_STYLE, y_align: Clutter.ActorAlign.CENTER }));
        right.add_child(this.#height);
        right.add_child(this.#lock);
        right.add_child(this.#ratio);
        pills.add_child(right);
        this.actor.add_child(pills);

        this.#recordBar = this.#buildRecordBar();
        this.#recordBar.visible = activeMode === 'record';
        this.actor.add_child(this.#recordBar);

        parent.add_child(this.actor);
    }

    /**
     * `spec/06` §2's settings row, for the one format this build records: frame rate,
     * the width cap, gifski's quality and the pointer. Each shows the app's default
     * until it is touched, and only what is touched is sent (D101).
     */
    #buildRecordBar(): St.BoxLayout {
        const bar = new St.BoxLayout({
            style: PILL_STYLE,
            height: PILL_HEIGHT,
            x_align: Clutter.ActorAlign.CENTER,
        });

        const [fps, fpsText] =
            this.#buildChoice(fpsLabel(this.#gif.fps, this.#hz), 'Frame rate');
        this.#fps = fps;
        this.#fpsLabel = fpsText;
        fps.connect('clicked', () => this.#toggleFpsMenu());
        bar.add_child(fps);

        const [width, widthText] =
            this.#buildChoice(widthLabel(this.#gif.maxWidth), 'Maximum width');
        this.#widthButton = width;
        this.#widthLabel = widthText;
        width.connect('clicked', () =>
            this.#toggleMenu(
                width,
                GIF_WIDTH_CHOICES.map(widthLabel),
                choiceIndex(GIF_WIDTH_CHOICES, this.#gif.maxWidth),
                index => this.#setGif('maxWidth', GIF_WIDTH_CHOICES[index] ?? this.#gif.maxWidth),
                'Maximum width',
            ),
        );
        bar.add_child(width);

        const [quality, qualityText] =
            this.#buildChoice(qualityLabel(this.#gif.quality), 'Quality');
        this.#qualityButton = quality;
        this.#qualityLabel = qualityText;
        quality.connect('clicked', () =>
            this.#toggleMenu(
                quality,
                GIF_QUALITY_CHOICES.map(qualityLabel),
                choiceIndex(GIF_QUALITY_CHOICES, this.#gif.quality),
                index => this.#setGif('quality', GIF_QUALITY_CHOICES[index] ?? this.#gif.quality),
                'Quality',
            ),
        );
        bar.add_child(quality);

        // The pointer, as a toggle like the aspect lock: on is the lit fill.
        const cursor = new St.Button({
            child: new St.Icon({ icon_name: 'input-mouse-symbolic', icon_size: 14 }),
            style: this.#gif.cursor ? TOGGLE_ON_STYLE : TOGGLE_STYLE,
            reactive: true,
            can_focus: true,
            accessible_name: 'Record the pointer',
        });
        cursor.connect('clicked', () => this.#setGif('cursor', !this.#gif.cursor));
        this.#cursor = cursor;
        bar.add_child(cursor);

        return bar;
    }

    /** A label with a chevron, the shape the ratio button already has. */
    #buildChoice(text: string, name: string): [St.Button, St.Label] {
        const box = new St.BoxLayout({});
        const label = new St.Label({
            text,
            style: 'font-size: 10pt; color: rgba(255,255,255,0.9);',
            y_align: Clutter.ActorAlign.CENTER,
        });
        box.add_child(label);
        box.add_child(new St.Icon({ icon_name: 'pan-down-symbolic', icon_size: 12 }));
        const button = new St.Button({
            child: box,
            style: TOGGLE_STYLE,
            reactive: true,
            can_focus: true,
            accessible_name: name,
        });
        return [button, label];
    }

    /** The frame-rate list, its screen's rate worked out for the monitor under the toolbar. */
    #toggleFpsMenu(): void {
        if (this.#fps === null) return;
        this.#toggleMenu(
            this.#fps,
            fpsChoiceLabels(this.#hz),
            choiceIndex(GIF_FPS_CHOICES, this.#gif.fps),
            index => this.#setGif('fps', GIF_FPS_CHOICES[index] ?? this.#gif.fps),
            'Frame rate',
        );
    }

    /**
     * The toolbar went to another monitor. At another refresh the screen's rate is another
     * number, so the row says the new one, and so does the frame-rate list if it is open.
     */
    #followRefresh(hz: number | null): void {
        if (hz === this.#hz) return;
        this.#hz = hz;
        this.#logScreenRate();
        if (this.#fpsLabel !== null) this.#fpsLabel.text = fpsLabel(this.#gif.fps, hz);
        if (this.#fps !== null && this.#menuAnchor === this.#fps) {
            this.#closeMenu();
            this.#toggleFpsMenu();
        }
    }

    /** What "Screen rate" means where the toolbar is, for the journal (D117). */
    #logScreenRate(): void {
        const m = this.#monitor;
        const hz = this.#hz === null ? 'an unknown refresh' : `${this.#hz.toFixed(3)} Hz`;
        info(`screen rate at ${m.x},${m.y}: ${recordedFps(SCREEN_RATE, this.#hz)} fps, from ${hz}`);
    }

    /** One of the four changed: show it, remember that it was touched, and say so. */
    #setGif<K extends keyof GifOverrides>(key: K, value: Required<GifOverrides>[K]): void {
        this.#gif[key] = value;
        this.#touched.add(key);
        if (this.#fpsLabel !== null) this.#fpsLabel.text = fpsLabel(this.#gif.fps, this.#hz);
        if (this.#widthLabel !== null) this.#widthLabel.text = widthLabel(this.#gif.maxWidth);
        if (this.#qualityLabel !== null) this.#qualityLabel.text = qualityLabel(this.#gif.quality);
        this.#cursor?.set_style(this.#gif.cursor ? TOGGLE_ON_STYLE : TOGGLE_STYLE);
        this.#handlers.recordOptionsChanged(this.recordOverrides);
    }

    /** What the user changed on the recording row, and nothing else (D101). */
    get recordOverrides(): GifOverrides {
        const out: GifOverrides = {};
        if (this.#touched.has('fps')) out.fps = this.#gif.fps;
        if (this.#touched.has('maxWidth')) out.maxWidth = this.#gif.maxWidth;
        if (this.#touched.has('quality')) out.quality = this.#gif.quality;
        if (this.#touched.has('cursor')) out.cursor = this.#gif.cursor;
        return out;
    }

    #buildModePill(activeMode: ToolbarMode): St.BoxLayout {
        const pill = new St.BoxLayout({ style: PILL_STYLE, height: PILL_HEIGHT });

        for (const mode of TOOLBAR_MODES) {
            // Icon above label, which is what the photograph shows.
            const content = new St.BoxLayout({
                vertical: true,
                x_align: Clutter.ActorAlign.CENTER,
            });
            content.add_child(
                new St.Icon({
                    gicon: icon(mode.icon),
                    icon_size: 16,
                    x_align: Clutter.ActorAlign.CENTER,
                }),
            );
            content.add_child(
                new St.Label({
                    text: mode.label,
                    style: MODE_LABEL_STYLE,
                    x_align: Clutter.ActorAlign.CENTER,
                }),
            );

            const button = new St.Button({
                child: content,
                style: mode.ready
                    ? mode.id === activeMode
                        ? MODE_BUTTON_ACTIVE_STYLE
                        : MODE_BUTTON_STYLE
                    : MODE_BUTTON_DISABLED_STYLE,
                reactive: mode.ready,
                can_focus: mode.ready,
                track_hover: mode.ready,
                // St.Button has no tooltip; the accessible name is the only place to say
                // why a dimmed button does nothing, and a screen reader is exactly the
                // user who cannot see that it is dimmed.
                accessible_name: mode.ready
                    ? mode.label
                    : `${mode.label} (not in this build yet)`,
            });

            if (mode.ready) {
                const id = mode.id;
                button.connect('clicked', () => this.#handlers.modeChanged(id));
            }

            this.#modeButtons.set(mode.id, button);
            pill.add_child(button);
        }

        return pill;
    }

    #buildField(which: 'width' | 'height'): St.Entry {
        const entry = new St.Entry({
            style: FIELD_STYLE,
            // Off until the overlay has taken key focus; see `setFieldsFocusable`.
            can_focus: false,
            y_align: Clutter.ActorAlign.CENTER,
            // A label a screen reader can use, since the `×` between them is the only
            // visual cue as to which is which.
            accessible_name: which === 'width' ? 'Width in pixels' : 'Height in pixels',
        });

        const text = entry.clutter_text;
        text.set_single_line_mode(true);

        text.connect('key-focus-in', () => {
            entry.set_style(FIELD_FOCUS_STYLE);
            // Select the whole value, so typing replaces rather than appends.
            text.set_selection(0, -1);
        });
        text.connect('key-focus-out', () => {
            entry.set_style(FIELD_STYLE);
            this.#commit();
        });
        // `activate` is Enter inside the entry. Committing here rather than letting the
        // key reach the overlay is what stops Enter in a field from confirming the
        // capture -- the user is finishing a number, not taking the shot.
        text.connect('activate', () => this.#commit());

        return entry;
    }

    #buildLock(): St.Button {
        const button = new St.Button({
            child: new St.Icon({ icon_name: 'changes-prevent-symbolic', icon_size: 14 }),
            style: TOGGLE_STYLE,
            reactive: true,
            can_focus: true,
            accessible_name: 'Lock the aspect ratio',
        });
        button.connect('clicked', () => {
            this.#locked = !this.#locked;
            button.set_style(this.#locked ? TOGGLE_ON_STYLE : TOGGLE_STYLE);
            (button.child as St.Icon).icon_name = this.#locked
                ? 'changes-prevent-symbolic'
                : 'changes-allow-symbolic';
            this.#handlers.lockChanged(this.#locked);
        });
        return button;
    }

    #buildRatio(): St.Button {
        const label = new St.BoxLayout({});
        this.#ratioLabel = new St.Label({
            text: RATIOS[0]!.label,
            style: 'font-size: 10pt; color: rgba(255,255,255,0.9);',
            y_align: Clutter.ActorAlign.CENTER,
        });
        label.add_child(this.#ratioLabel);
        label.add_child(new St.Icon({ icon_name: 'pan-down-symbolic', icon_size: 12 }));

        const button = new St.Button({
            child: label,
            style: TOGGLE_STYLE,
            reactive: true,
            can_focus: true,
            accessible_name: 'Aspect ratio',
        });
        // `spec/03` §4 asks for "a ratio menu chevron", and a chevron is a promise of a
        // list. This cycled instead, one ratio per click, which is how the user found it:
        // "the aspect ratios are not shown -- you can cycle through them." You could not
        // see what was on offer, or reach 16:10 without passing through four others.
        //
        // The menu is plain `St` actors parented to the same host as the toolbar, **not**
        // a `PopupMenu`. A popup takes its own grab, and inside the overlay's modal grab
        // the two fight; this is just another child of the actor the grab already covers.
        // `docs/decisions.md` D29.
        button.connect('clicked', () =>
            this.#toggleMenu(button, RATIOS.map(ratio => ratio.label), this.#ratioIndex, index => {
                const ratio = RATIOS[index];
                if (ratio === undefined) return;
                this.#ratioIndex = index;
                if (this.#ratioLabel !== null) this.#ratioLabel.text = ratio.label;
                this.#handlers.ratioChosen(ratio.value);
            }, 'Aspect ratio'),
        );
        return button;
    }

    /**
     * Opens a list under `anchor`, or closes it if that same list is already open. One
     * list at a time: opening the frame rate closes the ratio, which is what a second
     * chevron means. `active` is lit; `-1` lights nothing, for a setting typed by hand
     * that is not one of the choices.
     */
    #toggleMenu(
        anchor: St.Button,
        labels: readonly string[],
        active: number,
        pick: (index: number) => void,
        name: string,
    ): void {
        const reopening = this.#menuAnchor === anchor;
        this.#closeMenu();
        if (reopening) return;

        const menu = new St.BoxLayout({
            vertical: true,
            style: MENU_STYLE,
            reactive: true,
        });

        labels.forEach((label, index) => {
            const item = new St.Button({
                label,
                style: index === active ? MENU_ITEM_ACTIVE_STYLE : MENU_ITEM_STYLE,
                reactive: true,
                can_focus: true,
                x_expand: true,
                accessible_name: `${name} ${label}`,
            });
            item.connect('clicked', () => {
                this.#closeMenu();
                pick(index);
            });
            menu.add_child(item);
        });

        this.actor.get_parent()?.add_child(menu);
        this.#menu = menu;
        this.#menuAnchor = anchor;
        this.#placeMenu();
        info(`toolbar list ${name} opened`);
    }

    #closeMenu(): void {
        if (this.#menu !== null) info('toolbar list closed');
        this.#menu?.destroy();
        this.#menu = null;
        this.#menuAnchor = null;
    }

    /**
     * Where the buttons that open a list are, once the toolbar is laid out where it was
     * just put: a line for the log, and only when it changed. The harnesses press them
     * from it (`popups-test.sh`).
     */
    #sayControlsSoon(): void {
        if (this.#controlsIdle !== 0) return;
        this.#controlsIdle = GLib.idle_add(GLib.PRIORITY_LOW, () => {
            this.#controlsIdle = 0;
            const named: [string, St.Widget | null][] = [
                ['ratio', this.#ratio],
                ['fps', this.#fps],
                ['width', this.#widthButton],
                ['quality', this.#qualityButton],
            ];
            const said = named
                .filter(([, actor]) => actor !== null && actor.is_mapped())
                .map(([name, actor]) => `${name} ${boxOf(actor as St.Widget)}`)
                .join('; ');
            if (said !== '' && said !== this.#controlsSaid) {
                this.#controlsSaid = said;
                info(`toolbar controls: ${said}`);
            }
            return GLib.SOURCE_REMOVE;
        });
    }

    /**
     * Above the chevron, or below it when there is no room, and always inside the
     * monitor the toolbar is on. Stage coordinates, like the toolbar itself.
     */
    #placeMenu(): void {
        const menu = this.#menu;
        const anchor = this.#menuAnchor;
        if (menu === null || anchor === null) return;
        const m = this.#monitor;
        const [, width] = menu.get_preferred_width(-1);
        const [, height] = menu.get_preferred_height(-1);
        const [bx, by] = anchor.get_transformed_position();
        const [, bh] = anchor.get_transformed_size();

        let y = by - height - GAP;
        if (y < m.y + GAP) y = by + bh + GAP;
        menu.set_position(
            clamp(Math.round(bx), m.x + GAP, m.x + m.width - width - GAP),
            clamp(Math.round(y), m.y + GAP, m.y + m.height - height - GAP),
        );
    }

    /** Reads both fields and reports them, ignoring anything that is not a number. */
    #commit(): void {
        const parse = (entry: St.Entry): number | null => {
            const value = Number.parseInt(entry.get_text().trim(), 10);
            return Number.isFinite(value) && value > 0 ? value : null;
        };
        this.#handlers.sizeTyped(parse(this.#width), parse(this.#height));
    }

    /** Which mode button is lit, and whether the recording row is there. */
    setActiveMode(mode: ToolbarMode): void {
        for (const [id, button] of this.#modeButtons) {
            const known = TOOLBAR_MODES.find(m => m.id === id);
            if (known?.ready !== true) continue;
            button.set_style(id === mode ? MODE_BUTTON_ACTIVE_STYLE : MODE_BUTTON_STYLE);
        }
        const recording = mode === 'record';
        if (this.#recordBar.visible !== recording) {
            this.#recordBar.visible = recording;
            // The toolbar just changed height, and nothing else will move it until the
            // selection does; a row that appeared *over* the selection's bottom edge
            // would be this method's fault.
            this.#closeMenu();
            if (this.#lastSelection !== null) this.#place(this.#lastSelection);
        }
    }

    /**
     * Updates the fields from the live selection and places the toolbar.
     *
     * The fields show **physical** pixels, matching the classic modes' pill and
     * `spec/03` §12 item 2's requirement that the readout equals the PNG at every scale.
     * Skipped while a field has focus: overwriting a half-typed number on every motion
     * event would make the fields unusable.
     */
    /**
     * @param monitor the monitor holding the selection, which is not necessarily the one
     *   the overlay opened on. `spec/03` §8: the toolbar "moves with the pointer's
     *   monitor when idle", and a selection dragged onto another screen must take its
     *   toolbar with it.
     */
    update(selection: Rect | null, monitor?: ToolbarMonitor): void {
        if (selection === null || selection.width <= 0 || selection.height <= 0) {
            this.actor.visible = false;
            return;
        }
        if (monitor !== undefined) {
            const moved = monitor.x !== this.#monitor.x || monitor.y !== this.#monitor.y;
            this.#monitor = monitor;
            this.#scale = monitor.scale;
            if (moved) this.#followRefresh(refreshAt(this.#rates, monitor.x, monitor.y));
        }

        if (!this.editing) {
            this.#width.set_text(String(Math.round(selection.width * this.#scale)));
            this.#height.set_text(String(Math.round(selection.height * this.#scale)));
        }

        this.actor.visible = true;
        this.#lastSelection = selection;
        this.#place(selection);
    }

    /**
     * `spec/03` §4: "centred horizontally and sits below the selection, moving inside it
     * when there is no room underneath."
     */
    #place(selection: Rect): void {
        const [, width] = this.actor.get_preferred_width(-1);
        const [, height] = this.actor.get_preferred_height(-1);
        const m = this.#monitor;

        // **Stage coordinates, clamped to the selection's own monitor.** These used to be
        // local to the root the toolbar was built on, and clamped to *that* monitor's
        // width -- so a selection on a second screen pinned the toolbar to the first
        // screen's right edge and it never arrived. `docs/decisions.md` D28.
        let x = Math.round(selection.x + (selection.width - width) / 2);
        x = clamp(x, m.x + GAP, m.x + m.width - width - GAP);

        let y = selection.y + selection.height + GAP;
        if (y + height > m.y + m.height - GAP) {
            // No room underneath: inside the selection, against its bottom edge.
            y = selection.y + selection.height - height - GAP;
        }
        y = clamp(y, m.y + GAP, m.y + m.height - height - GAP);

        this.actor.set_position(x, y);
        this.#placeMenu();
        this.#sayControlsSoon();
    }

    /** True when the pointer is over the toolbar, so the overlay leaves the event alone. */
    containsStagePoint(x: number, y: number): boolean {
        // The open ratio list counts as part of the toolbar. Without this a click on
        // "16:9" would be read by the overlay as the start of a new drag, and the
        // selection would vanish as the ratio was chosen.
        if (this.#menu !== null && within(this.#menu, x, y)) return true;
        if (!this.actor.visible) return false;
        return within(this.actor, x, y);
    }

    /** Closes any transient child, such as the ratio list. Safe to call at any time. */
    dismissPopups(): boolean {
        if (this.#menu === null) return false;
        this.#closeMenu();
        return true;
    }

    /**
     * Closes the open list when a press at `x, y`, stage coordinates, is not on it, and
     * says whether it did: the press is then spent on closing it (D150).
     */
    dismissPopupsOutside(x: number, y: number): boolean {
        if (this.#menu === null || within(this.#menu, x, y)) return false;
        this.#closeMenu();
        return true;
    }

    /**
     * True while a dimension field really has keyboard focus.
     *
     * Asked of the stage every time rather than cached in a flag set from
     * `key-focus-in`. That flag was a bug with teeth: an `St.Entry` can acquire focus
     * without the user asking -- it did so whenever the overlay opened while an
     * application window had focus, which is every capture after the first -- and the
     * engine's "a field is focused, so this keystroke is a number" guard then swallowed
     * **the entire keyboard vocabulary**. Arrows stopped nudging, Enter stopped
     * confirming, and only the mouse still worked. A live query cannot go stale.
     */
    get editing(): boolean {
        const focus = global.stage.get_key_focus();
        return focus === this.#width.clutter_text || focus === this.#height.clutter_text;
    }

    /** Hands focus back to the overlay, so arrow keys nudge instead of moving a caret. */
    unfocusFields(): void {
        try {
            global.stage.set_key_focus(null);
        } catch (e) {
            error('could not release focus from the toolbar fields', e);
        }
    }

    /**
     * Whether the fields may take focus at all.
     *
     * They are built unfocusable and only become focusable once the overlay has taken its
     * own key focus, so building the toolbar can never steal it. See [`editing`].
     */
    setFieldsFocusable(focusable: boolean): void {
        this.#width.can_focus = focusable;
        this.#height.can_focus = focusable;
    }

    destroy(): void {
        if (this.#controlsIdle !== 0) GLib.source_remove(this.#controlsIdle);
        this.#controlsIdle = 0;
        this.#modeButtons.clear();
        // The menu is a sibling of the toolbar, not a child, so destroying the toolbar
        // would leave it on screen with nothing to close it.
        this.#closeMenu();
        this.#ratioLabel = null;
        this.#fps = null;
        this.#fpsLabel = null;
        this.#widthButton = null;
        this.#qualityButton = null;
        this.#widthLabel = null;
        this.#qualityLabel = null;
        this.#cursor = null;
        this.actor.destroy();
    }
}
