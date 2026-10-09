// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * `SYS-01`: the panel indicator and its menu.
 *
 * `spec/01` §2 row 30 puts this in the extension rather than the app, and it has to be:
 * only shell code can put a button in the top bar, and `spec/06` §4 will need this same
 * button to turn red and carry the recording timer.
 *
 * The item list follows appendix A.10, which is a photograph of the real menu rather than
 * a guess, and three things it settles are honoured here. The "Capture Area & Copy" style
 * variants are **shortcut-only** and never appear as items, so the menu stays scannable.
 * There is no "Restore Recently Closed" item. And a shortcut hint appears on a row **only
 * when that action is actually bound**, right-aligned -- which is why every row reads its
 * accelerator from GSettings instead of hard-coding one.
 *
 * Items whose feature does not exist yet are **absent, not disabled**. A greyed row
 * promises something the build cannot do; an absent one just is not there. The exception
 * is Settings, which is present and reports honestly when the app is not installed.
 */

import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import St from 'gi://St';
import * as Animation from 'resource:///org/gnome/shell/ui/animation.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

import { prettyAccelerator } from './accel.js';
import { APP_SYMBOLIC, icon as themedIcon } from './icons.js';
import { activateApp } from './app.js';
import type { CaptureModeName } from './flow.js';
import type { Settings } from './settings.js';
import { activeCountdown } from './overlay/countdown.js';
import { error, info } from './log.js';

/** Where in the top bar the button sits. `right` is where status icons belong. */
const PANEL_POSITION = 'right';

/** The panel timer: tabular figures so it does not jitter, hidden until a recording. */
const TIMER_STYLE = 'margin-left: 4px; font-feature-settings: "tnum";';
/** The same, but `spec/09`'s red while recording (`spec/06` §4). */
const TIMER_STYLE_RECORDING = 'margin-left: 4px; font-feature-settings: "tnum"; color: #e01b24;';

/** One menu row: a capture mode, its label, and the settings key holding its shortcut. */
interface ModeItem {
    mode: CaptureModeName;
    label: string;
    key: string;
}

/** Appendix A.10's order, limited to the modes this build implements. */
const MODE_ITEMS: readonly ModeItem[] = [
    { mode: 'all-in-one', label: 'All-In-One', key: 'all-in-one' },
    { mode: 'area', label: 'Capture Area', key: 'capture-area' },
    { mode: 'previous-area', label: 'Capture Previous Area', key: 'capture-previous-area' },
    { mode: 'fullscreen', label: 'Capture Fullscreen', key: 'capture-fullscreen' },
    { mode: 'window', label: 'Capture Window', key: 'capture-window' },
    { mode: 'self-timer', label: 'Self-Timer', key: 'self-timer' },
];

export interface IndicatorHandlers {
    /** Starts a capture. The indicator never captures itself; it only asks. */
    capture(mode: CaptureModeName): void;
    /** `spec/06` §3: starts a GIF recording (select, count down, hand to the app). */
    record(): void;
    /** `spec/06` §3's Stop, from the menu item shown only while recording. */
    stopRecording(): void;
    /** `spec/07` §4.2: the history strip. */
    openHistory(): void;
    /** Where a scrolling capture is, read as the menu opens (D137). */
    scrolling(): 'none' | 'ready' | 'running';
    /** Presses the scrolling pill's button: Start, and Done once it runs (D137). */
    advanceScrolling(): void;
    /** `DSK-01`, the extension's own. */
    toggleDesktopIcons(): void;
    /** Whether this session has desktop icons, and whether they are hidden now. */
    desktopIcons(): { available: boolean; hidden: boolean };
}

export class Indicator {
    #button: PanelMenu.Button | null = null;
    #settings: Settings;
    #handlers: IndicatorHandlers;
    #settingsChangedId = 0;
    /** The top-bar icon and the timer beside it, so a recording can recolour them. */
    #icon: St.Icon | null = null;
    #timer: St.Label | null = null;
    /** Where the icon was while a recording is being saved (`spec/09` §3). */
    #spinner: Animation.Spinner | null = null;
    /** The Stop item, present only while recording, and the line under it. */
    #stopItem: PopupMenu.PopupMenuItem | null = null;
    #stopSeparator: PopupMenu.PopupSeparatorMenuItem | null = null;
    /** Record GIF, which cannot start a second recording while one runs. */
    #recordItem: PopupMenu.PopupMenuItem | null = null;
    /**
     * The recording label to re-apply if the button is rebuilt mid-recording (the
     * show-indicator watch tears the button down and builds a new one). `null` when idle.
     */
    #recordingLabel: string | null = null;
    /** And whether it was being saved, for the same rebuild. */
    #recordingSaving = false;
    /** Update OctoSnap, and whether the app offers it (D170), which a rebuild keeps. */
    #updateItem: PopupMenu.PopupMenuItem | null = null;
    #updateOffered = false;

    constructor(settings: Settings, handlers: IndicatorHandlers) {
        this.#settings = settings;
        this.#handlers = handlers;
    }

    /**
     * Adds the button if `show-indicator` is on, and follows that setting from then on.
     *
     * The setting is watched rather than read once, because `spec/08` §1 offers it as a
     * plain toggle and a user who turns it off expects the icon to go away now, not after
     * a logout -- which for extension *code* is the only other option (`docs/spikes/16`).
     */
    enable(): void {
        this.#apply();
        try {
            this.#settingsChangedId = this.#settings.raw.connect(
                'changed::show-indicator',
                () => this.#apply(),
            );
        } catch (e) {
            // A schema too old to have the key: the indicator stays as `#apply` left it.
            error('could not watch show-indicator', e);
        }
    }

    #apply(): void {
        const wanted = this.#settings.showIndicator;
        if (wanted && this.#button === null) this.#add();
        else if (!wanted && this.#button !== null) this.#remove();
    }

    #add(): void {
        const button = new PanelMenu.Button(0.0, 'OctoSnap', false);

        // An icon and a timer side by side: the timer is empty and hidden until a
        // recording turns them both red (`spec/06` §4). A box rather than a bare icon so
        // there is somewhere for the timer to sit.
        const box = new St.BoxLayout({ style_class: 'panel-status-menu-box' });
        const icon = new St.Icon({
            // OctoSnap's own symbolic, from the extension's files (`icons.ts`).
            gicon: themedIcon(APP_SYMBOLIC),
            style_class: 'system-status-icon',
        });
        const timer = new St.Label({
            text: '',
            y_align: Clutter.ActorAlign.CENTER,
            visible: false,
            style: TIMER_STYLE,
        });
        // The Shell's own spinner, the size of a status icon, shown at once rather than
        // after the second its fade would wait: a save is usually shorter than that, and
        // the icon it stands in for has already gone.
        const spinner = new Animation.Spinner(16, { animate: false, hideOnStop: true });
        box.add_child(icon);
        box.add_child(spinner);
        box.add_child(timer);
        button.add_child(box);
        this.#icon = icon;
        this.#timer = timer;
        this.#spinner = spinner;

        const menu = button.menu as PopupMenu.PopupMenu;

        // `spec/03` §5.4's Esc, again, for when Esc is not the countdown's to take: shown
        // at the top while a countdown runs, and read each time the menu opens.
        const cancelItem = new PopupMenu.PopupMenuItem('Cancel Countdown');
        const cancelKey = new St.Label({
            text: 'Esc',
            style_class: 'popup-menu-item-accel',
            x_align: Clutter.ActorAlign.END,
            x_expand: true,
        });
        cancelItem.add_child(cancelKey);
        cancelItem.connect('activate', () => activeCountdown()?.cancel());
        const cancelSeparator = new PopupMenu.PopupSeparatorMenuItem();
        menu.addMenuItem(cancelItem);
        menu.addMenuItem(cancelSeparator);
        cancelItem.visible = false;
        cancelSeparator.visible = false;

        // The scrolling pill's button, at the top while a pill is up. The pill never takes
        // the keyboard, and the top bar can be reached without the mouse, so this is the
        // keyboard's way to Start and Done when no key is bound (`spec/00` §9, D137).
        const scrollItem = new PopupMenu.PopupMenuItem('Start Scrolling Capture');
        // Read as the menu opens, like the item itself: the key can be bound after the
        // menu was built, and a key bound for this is the one worth showing here.
        const scrollKey = new St.Label({
            style_class: 'popup-menu-item-accel',
            x_align: Clutter.ActorAlign.END,
            x_expand: true,
        });
        scrollItem.add_child(scrollKey);
        scrollItem.connect('activate', () => this.#handlers.advanceScrolling());
        const scrollSeparator = new PopupMenu.PopupSeparatorMenuItem();
        menu.addMenuItem(scrollItem);
        menu.addMenuItem(scrollSeparator);
        scrollItem.visible = false;
        scrollSeparator.visible = false;

        menu.connect('open-state-changed', (_menu, open: boolean) => {
            if (!open) return;
            const countdown = activeCountdown();
            cancelItem.visible = countdown !== null;
            cancelSeparator.visible = countdown !== null;
            cancelKey.visible = countdown?.cancelsOnEscape ?? false;
            const scrolling = this.#handlers.scrolling();
            scrollItem.visible = scrolling !== 'none';
            scrollSeparator.visible = scrolling !== 'none';
            scrollItem.label.text =
                scrolling === 'running' ? 'Finish Scrolling Capture' : 'Start Scrolling Capture';
            const key = scrolling === 'none' ? null : this.#acceleratorFor('scrolling-capture');
            scrollKey.text = key ?? '';
            scrollKey.visible = key !== null;
        });

        for (const item of MODE_ITEMS) {
            const row = new PopupMenu.PopupMenuItem(item.label);
            // The accelerator hint, right-aligned, and only when bound (appendix A.10).
            const accelerator = this.#acceleratorFor(item.key);
            if (accelerator !== null) {
                row.add_child(
                    new St.Label({
                        text: accelerator,
                        style_class: 'popup-menu-item-accel',
                        x_align: Clutter.ActorAlign.END,
                        x_expand: true,
                    }),
                );
            }
            row.connect('activate', () => this.#handlers.capture(item.mode));
            menu.addMenuItem(row);
        }

        // `spec/06` §3's "Record GIF", after the capture modes. Not in appendix A.10's
        // photograph, which predates the recorder, so it takes the modes' shape: a label
        // with its shortcut hint when `record-gif` is bound.
        const recordItem = new PopupMenu.PopupMenuItem('Record GIF');
        const recordAccelerator = this.#acceleratorFor('record-gif');
        if (recordAccelerator !== null) {
            recordItem.add_child(
                new St.Label({
                    text: recordAccelerator,
                    style_class: 'popup-menu-item-accel',
                    x_align: Clutter.ActorAlign.END,
                    x_expand: true,
                }),
            );
        }
        recordItem.connect('activate', () => this.#handlers.record());
        menu.addMenuItem(recordItem);
        this.#recordItem = recordItem;

        menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

        // `SYS-01`'s "history, hide icons": appendix A.10 has both. The history row
        // carries its shortcut like the modes do; the icons row reads its label from the
        // state each time the menu opens, and is absent on a session with no icons.
        const historyItem = new PopupMenu.PopupMenuItem('History…');
        const historyAccelerator = this.#acceleratorFor('open-history');
        if (historyAccelerator !== null) {
            historyItem.add_child(
                new St.Label({
                    text: historyAccelerator,
                    style_class: 'popup-menu-item-accel',
                    x_align: Clutter.ActorAlign.END,
                    x_expand: true,
                }),
            );
        }
        historyItem.connect('activate', () => this.#handlers.openHistory());
        menu.addMenuItem(historyItem);

        const iconsItem = new PopupMenu.PopupMenuItem('Hide Desktop Icons');
        iconsItem.connect('activate', () => this.#handlers.toggleDesktopIcons());
        menu.addMenuItem(iconsItem);
        // `spec/14` §11: the pets out or in, a switch on `pets-enabled`.
        const petsItem = new PopupMenu.PopupSwitchMenuItem('Desktop Pets', this.#settings.petsEnabled);
        petsItem.connect('toggled', (_item, on: boolean) => this.#settings.setPetsEnabled(on));
        menu.addMenuItem(petsItem);
        menu.connect('open-state-changed', (_menu, open: boolean) => {
            if (!open) return;
            const state = this.#handlers.desktopIcons();
            iconsItem.visible = state.available;
            iconsItem.label.text = state.hidden ? 'Show Desktop Icons' : 'Hide Desktop Icons';
            petsItem.setToggleState(this.#settings.petsEnabled);
        });

        menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

        // D170: there while the app says a newer release can be installed from it. No
        // ellipsis: the app installs it, and asks nothing more, unless the portal asks
        // once whether the app may update itself at all.
        const updateItem = new PopupMenu.PopupMenuItem('Update OctoSnap');
        updateItem.visible = this.#updateOffered;
        updateItem.connect('activate', () => activateApp('update'));
        menu.addMenuItem(updateItem);
        this.#updateItem = updateItem;

        const settingsItem = new PopupMenu.PopupMenuItem('Settings…');
        settingsItem.connect('activate', () => {
            // Through the app, because the Preferences dialog is GTK and lives there. The
            // app is D-Bus-activated, so this starts it if it is not running.
            activateApp('open-settings', new GLib.Variant('s', 'general'));
        });
        menu.addMenuItem(settingsItem);

        Main.panel.addToStatusArea('octosnap', button, 0, PANEL_POSITION);
        this.#button = button;

        // If a recording was already on when the button was (re)built -- the show-indicator
        // watch can rebuild it mid-recording -- restore its red state and Stop item.
        if (this.#recordingLabel !== null) {
            const label = this.#recordingLabel;
            this.#recordingLabel = null;
            this.setRecording(true, label, this.#recordingSaving);
        }

        info('panel indicator added');
    }

    /**
     * `RecordingPanel`: turn the top-bar button red with a timer and offer Stop, or restore
     * it. Driven by the app's `SetRecordingState` through the recording coordinator.
     *
     * A no-op when the button is absent (`show-indicator` off): the recording still has its
     * frame and, for an area recording, the app's pill; the panel is simply not a surface
     * this session shows. The label is remembered so a rebuild can restore it.
     *
     * `saving` is Stop pressed with the file still being written: `spec/09` §3's
     * "stop → processing" row, a spinner where the red dot was until the card is up.
     */
    setRecording(active: boolean, label: string, saving = false): void {
        this.#recordingLabel = active ? label : null;
        this.#recordingSaving = active && saving;
        const icon = this.#icon;
        const timer = this.#timer;
        if (icon === null || timer === null) return;

        // One recording at a time: a second Record GIF would be dropped with a log line.
        this.#recordItem?.setSensitive(!active);
        if (active) {
            icon.gicon = themedIcon('media-record-symbolic');
            icon.style = 'color: #e01b24;';
            timer.text = label;
            timer.style = TIMER_STYLE_RECORDING;
            timer.visible = true;
            this.#ensureStopItem();
        } else {
            icon.gicon = themedIcon(APP_SYMBOLIC);
            icon.style = null;
            timer.text = '';
            timer.style = TIMER_STYLE;
            timer.visible = false;
            this.#removeStopItem();
        }
        this.#setSaving(this.#recordingSaving);
    }

    #setSaving(saving: boolean): void {
        // `spec/13` #19: Stop while the file is being written would do nothing at all.
        this.#stopItem?.setSensitive(!saving);
        const icon = this.#icon;
        const spinner = this.#spinner;
        if (icon === null || spinner === null) return;
        try {
            if (saving) {
                icon.visible = false;
                spinner.play();
            } else {
                spinner.stop();
                icon.visible = true;
            }
        } catch (e) {
            // A spinner that cannot turn must not cost the panel its icon.
            error('could not show the saving spinner', e);
            icon.visible = true;
        }
    }

    /** Adds the "Stop Recording" item at the top of the menu, once. */
    #ensureStopItem(): void {
        if (this.#stopItem !== null || this.#button === null) return;
        const menu = this.#button.menu as PopupMenu.PopupMenu;
        const item = new PopupMenu.PopupMenuItem('Stop Recording');
        const accelerator = this.#acceleratorFor('recording-stop');
        if (accelerator !== null) {
            item.add_child(
                new St.Label({
                    text: accelerator,
                    style_class: 'popup-menu-item-accel',
                    x_align: Clutter.ActorAlign.END,
                    x_expand: true,
                }),
            );
        }
        item.connect('activate', () => this.#handlers.stopRecording());
        // At the very top: while recording, Stop is the thing the user reaches for. A line
        // under it, as every other group in the menu has.
        const separator = new PopupMenu.PopupSeparatorMenuItem();
        menu.addMenuItem(item, 0);
        menu.addMenuItem(separator, 1);
        this.#stopItem = item;
        this.#stopSeparator = separator;
    }

    #removeStopItem(): void {
        try {
            this.#stopItem?.destroy();
            this.#stopSeparator?.destroy();
        } catch (e) {
            error('could not remove the Stop Recording item', e);
        }
        this.#stopItem = null;
        this.#stopSeparator = null;
    }

    #remove(): void {
        try {
            this.#button?.destroy();
        } catch (e) {
            error('could not destroy the panel indicator', e);
        }
        // Children of the button, gone with it. `#recordingLabel` is kept so a rebuild
        // (show-indicator toggled mid-recording) can restore the red state.
        this.#button = null;
        this.#icon = null;
        this.#timer = null;
        this.#spinner = null;
        this.#stopItem = null;
        this.#updateItem = null;
        info('panel indicator removed');
    }

    /**
     * `SetUpdateOffered` (D170): the Update OctoSnap item shown or taken away. Kept for a
     * rebuild, as the recording's label is.
     */
    setUpdateOffered(offered: boolean): void {
        this.#updateOffered = offered;
        if (this.#updateItem !== null) this.#updateItem.visible = offered;
        info(`update ${offered ? 'offered' : 'not offered'} in the panel menu`);
    }

    /** The first binding for an action, as a person reads it, or `null` when unbound. */
    #acceleratorFor(key: string): string | null {
        try {
            if (!this.#settings.raw.settings_schema?.has_key(key)) return null;
            const accelerator = this.#settings.raw.get_strv(key)[0];
            if (accelerator === undefined || accelerator === '') return null;
            return prettyAccelerator(accelerator);
        } catch (e) {
            error(`could not read the shortcut for '${key}'`, e);
            return null;
        }
    }

    /** `spec/10` §2: disable() must undo everything enable() did. */
    destroy(): void {
        if (this.#settingsChangedId !== 0) {
            try {
                this.#settings.raw.disconnect(this.#settingsChangedId);
            } catch (e) {
                error('could not disconnect the show-indicator watch', e);
            }
            this.#settingsChangedId = 0;
        }
        this.#remove();
    }
}
