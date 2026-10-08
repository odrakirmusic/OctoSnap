// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The pets, together (`spec/14`): which are out, where they live on the stage, the one
 * clock they share, the pointer, their menu, when they hide, and what they do when the
 * capture tool does something.
 *
 * **Where they live.** In a host of their own in `Main.layoutManager.screenshotUIGroup`
 * (`overlay/host.ts`), the layer every capture leaves out: `Shell.Screenshot` never paints
 * it, and the stage repaints that read an area set it to nothing for the length of the read
 * (D22, D130). So no screenshot, window capture or text capture ever has a pet in it, with
 * nothing done per capture. A recording and a scrolling capture are the exceptions: both
 * read a ScreenCast stream -- a scrolling capture's live view is one (D106) -- and a stream
 * is what the monitor shows. So pets get out of the area before its first frame and stay
 * out until it ends (`recording-soon`, `scrolling`).
 *
 * **One clock.** Every pet says when it next looks different (`Pet.step`), and one GLib
 * timeout wakes the crew then. A pet that is walking wants a frame every tenth of a
 * second; one that is falling, thirty a second; one that sits there asleep, none. So pets
 * at rest cost a wake-up every few seconds for a blink, and sleeping ones nothing at all.
 * `spec/10` §7's 2 ms is kept by painting at most one new pose a step, across the crew.
 *
 * **When they hide.** In the overview, behind a system dialog, on a monitor with a
 * fullscreen window, and while the screen is shared or recorded by something that is not
 * OctoSnap (`pet-hide-sharing`). Each is a reason; a pet shows again when none is left.
 *
 * **What they hear.** `events.ts`. What a capture changes about the world -- an area to
 * stay out of -- is taken in at once, since the next thing to happen may be a ScreenCast
 * session starting; what the pets *do* about it waits for the next step, so a capture's
 * own 2 ms never pays for a pet's reaction.
 *
 * Everything this creates, `destroy()` takes away, synchronously: it runs in `disable()`.
 */

import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import type Meta from 'gi://Meta';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import type { CaptureModeName, CaptureOptions } from '../flow.js';
import { error, info } from '../log.js';
import { createOverlayHost } from '../overlay/host.js';
import type { Settings } from '../settings.js';
import { lickReach } from './acts.js';
import { PETS, PET_KINDS, type PetKind } from './art/index.js';
import type { Temperament } from './brain.js';
import { type PetEvent, listenToPets } from './events.js';
import { type PetSize, artPixel } from './frames.js';
import { type MenuChoice, PetMenu } from './menu.js';
import { type Box, type HideReason, Pet, type Surroundings } from './pet.js';
import {
    BAND_CLEARANCE_ART,
    FLOOR_BAND_ART,
    type Platform,
    type World,
    type WorldMonitor,
    besideArea,
    besideAreaY,
    monitorIndexOf,
    reaches,
    spotOn,
    throwVelocity,
} from './physics.js';

/** A pet's saved spot (`pet-spots`): its monitor, how far along and down its work area, and whether on the desk. */
type Spot = [number, number, number, boolean];

/** What the crew asks of the rest of the extension. */
export interface CrewHooks {
    capture(mode: CaptureModeName, options: CaptureOptions): void;
    activate(action: string, parameter: GLib.Variant | null): void;
    /** The pin windows on screen, for pets to sit on. */
    pins(): readonly Meta.Window[];
    /** Where a scrolling capture is: no controls, controls waiting for Start, or running. */
    scrolling(): 'none' | 'ready' | 'running';
}

/** How long a press has to be held before it picks the pet up, when the pointer is still. */
const HOLD_MS = 180;
/** How far the pointer moves before a press is a drag rather than a click. */
const DRAG_SLOP = 4;
/** `spec/10` §7: what one step of the crew may take on the compositor's main loop. */
const STEP_BUDGET_MS = 2;
/** A step this long costs the desktop a frame, and the log says what was in it. */
const FRAME_MS = 16;
/** Idle this long and the pets doze off (`spec/14` §7). */
const SLEEP_AFTER_MS = 90_000;
/** A copy this soon after a capture is the capture's own: one reaction, not two. */
const COPY_AFTER_CAPTURE_MS = 3000;
/** How long the pets cheer a scrolling capture along. */
const SCROLL_RUN_MS = 6000;
/**
 * How long a scrolling capture handed to the app may go without its controls showing
 * before the pets stop waiting for it: an app that is not running starts first, in a
 * second or two.
 */
const SCROLL_CONTROLS_WAIT_MS = 10_000;
/** A walk's pace times `leave`'s 2.6, in art pixels a second. */
const LEAVE_SPEED = 37;
/** `leave`'s start (a "!" and a gasp) and end, for whether a walk out fits a countdown. */
const LEAVE_EXTRA_MS = 900;
/** The puff where a pet reappears. */
const PUFF = ['#ffffff', '#deddda', '#c0bfbc'] as const;

interface Drag {
    pet: Pet;
    grab: Clutter.Grab;
    lifted: boolean;
    from: { x: number; y: number };
    /** Where on the pet it was held, from its left edge and its feet. */
    offset: { x: number; y: number };
    trail: [number, number, number][];
    holdTimer: number;
}

/**
 * The pets' actors still on the stage, found by name: after `#stop`, none. Walks the whole
 * stage, which is why only `#stop` asks, and says so in the log for the harness.
 */
function leftOnStage(): number {
    let count = 0;
    const visit = (actor: Clutter.Actor) => {
        if (actor.name?.startsWith('octosnap-pet')) count++;
        for (const child of actor.get_children()) visit(child);
    };
    visit(global.stage);
    return count;
}

function now(): number {
    return GLib.get_monotonic_time() / 1000;
}

/** The pins the pets can stand on: shown, on this workspace, with a size. */
export function platformsOf(windows: readonly Meta.Window[]): Platform[] {
    const workspace = global.workspace_manager.get_active_workspace();
    const platforms: Platform[] = [];
    for (const window of windows) {
        if (window.minimized || !window.located_on_workspace(workspace)) continue;
        const actor = window.get_compositor_private<Meta.WindowActor>();
        if (actor === null || !actor.visible) continue;
        const r = window.get_frame_rect();
        if (r.width <= 0 || r.height <= 0) continue;
        platforms.push({ id: String(window.get_stable_sequence()), rect: { x: r.x, y: r.y, width: r.width, height: r.height } });
    }
    return platforms;
}

type Signals = { connect(signal: string, callback: (...args: never[]) => unknown): number; disconnect(id: number): void };

export class Crew {
    readonly #settings: Settings;
    readonly #hooks: CrewHooks;
    #running = false;
    #host: St.Widget | null = null;
    #layers: { shadows: Clutter.Actor; pets: Clutter.Actor; fx: Clutter.Actor } | null = null;
    #pets = new Map<PetKind, Pet>();
    #timer = 0;
    #timerAt = Infinity;
    #monitors: WorldMonitor[] = [];
    /** Whether this step has painted its one new pose. */
    #painted = false;
    #drag: Drag | null = null;
    #menu: PetMenu | null = null;
    #signals: [Signals, number][] = [];
    #settingSignals: number[] = [];
    #unlisten: (() => void) | null = null;
    #idleWatch = 0;
    #activeWatch = 0;
    /** The system dialogs, and each one's `notify::visible`. */
    #dialogs = new Map<Clutter.Actor, number>();
    /** Screen-sharing and recording sessions that are not OctoSnap's, and their `stopped`. */
    #shares = new Map<Meta.RemoteAccessHandle, number>();
    /** The area being recorded, or about to be. */
    #recording: Box | null = null;
    /** The area a scrolling capture is scrolling, from when its selection is handed to the app. */
    #scrolling: Box | null = null;
    #scrollingSince = 0;
    /** A look now and then at whether that capture's controls are still up. */
    #scrollCheck = 0;
    /** A window being dragged: pets on pins follow it every frame rather than every breath. */
    #moving = false;
    #lastCaptureAt = -Infinity;
    /** What the pets heard, for the next step to act on. */
    #heard: PetEvent[] = [];
    /** The steps since the pets came out, for the budget: how many, the slowest, and how many went over. */
    #steps = { count: 0, slowest: 0, over: 0 };
    /** A save of `pet-places` waiting for the pet the user moved to land. */
    #saveTimer = 0;
    /** Capture overlays open now: the pets step aside while a selection is made. */
    #overlays = 0;
    /**
     * `pet-size` and how lively the pets are, read when one changes rather than every step:
     * a setting read is three calls into GObject, and a step made three of them.
     */
    #look: { size: PetSize; temperament: Temperament } | null = null;

    constructor(settings: Settings, hooks: CrewHooks) {
        this.#settings = settings;
        this.#hooks = hooks;
    }

    /** Watches `pets-enabled` and the other pet settings, and brings the pets out if on. */
    enable(): void {
        const raw = this.#settings.raw;
        this.#settingSignals.push(
            raw.connect('changed::pets-enabled', () => this.#sync()),
            raw.connect('changed::pets', () => this.#syncKinds()),
            raw.connect('changed::pet-size', () => {
                this.#look = null;
                this.#resize();
            }),
            raw.connect('changed::pet-activity', () => {
                this.#look = null;
                // Silent starts nothing, and a walk down the whole desk already under way
                // ends too, as for *Move on their own* turned off (D166).
                if (this.#settings.petActivity === 'silent') this.#stopMoving();
                else this.#schedule(now());
            }),
            raw.connect('changed::pet-wander', () => {
                this.#look = null;
                if (!this.#settings.petWander) this.#stopMoving();
            }),
            raw.connect('changed::pet-roam', () => this.#roamChanged()),
            raw.connect('changed::pet-hide-sharing', () => this.#syncHiding()),
        );
        this.#sync();
    }

    destroy(): void {
        for (const id of this.#settingSignals) this.#settings.raw.disconnect(id);
        this.#settingSignals = [];
        this.#stop();
        // After `#lost`, what the crew watched is still connected.
        if (this.#signals.length > 0) this.#unwatch();
    }

    // --- out and in -------------------------------------------------------------------

    #sync(): void {
        const wanted = this.#settings.petsEnabled;
        if (wanted && !this.#running) this.#start();
        else if (!wanted && this.#running) this.#stop();
    }

    #start(): void {
        try {
            this.#running = true;
            // A crew that lost its host last time is still connected to what it watched.
            if (this.#signals.length > 0) this.#unwatch();
            const host = createOverlayHost('octosnap-pets');
            this.#host = host;
            host.connect('destroy', () => {
                if (this.#host === host) this.#lost();
            });
            const layer = (name: string) => {
                const actor = new Clutter.Actor({ name, reactive: false, layout_manager: new Clutter.FixedLayout() });
                host.add_child(actor);
                return actor;
            };
            // Shadows under the pets and effects over them; the menu, when it opens, over all.
            this.#layers = {
                shadows: layer('octosnap-pet-shadows'),
                pets: layer('octosnap-pet-sprites'),
                fx: layer('octosnap-pet-fx'),
            };
            this.#measureMonitors();
            this.#watch();
            this.#syncKinds();
            info(`pets out: ${[...this.#pets.keys()].join(', ')}`);
        } catch (e) {
            error('the pets could not come out', e);
            this.#stop();
        }
    }

    #stop(): void {
        if (!this.#running) return;
        this.#savePlaces();
        this.#endDrag(false);
        this.#menu?.destroy();
        this.#menu = null;
        if (this.#timer !== 0) GLib.source_remove(this.#timer);
        this.#timer = 0;
        this.#timerAt = Infinity;
        if (this.#saveTimer !== 0) GLib.source_remove(this.#saveTimer);
        this.#saveTimer = 0;
        if (this.#scrollCheck !== 0) GLib.source_remove(this.#scrollCheck);
        this.#scrollCheck = 0;
        this.#unwatch();
        for (const pet of this.#pets.values()) pet.destroy();
        this.#pets.clear();
        // Forgotten first, so its `destroy` is not taken for the stage's (`#lost`).
        const host = this.#host;
        this.#host = null;
        host?.destroy();
        this.#layers = null;
        this.#recording = null;
        this.#scrolling = null;
        this.#moving = false;
        this.#heard = [];
        this.#overlays = 0;
        // The animations setting is only watched while they are out.
        this.#look = null;
        this.#running = false;
        const { count, slowest, over } = this.#steps;
        this.#steps = { count: 0, slowest: 0, over: 0 };
        info(
            `pets in after ${count} steps, the slowest ${slowest.toFixed(2)} ms and ${over} over ` +
                `${STEP_BUDGET_MS} ms; ${leftOnStage()} of their actors left on the stage`,
        );
    }

    /**
     * The host went without `#stop`. The shell does not turn its extensions off when the
     * session ends: it takes its actors down under them, and this host with them, while
     * the crew's clock is still set, and then runs a main loop of its own to wait for its
     * time-limits manager. A step in that loop moved the actors that had gone, and GJS
     * logged each one at every logout (D154). So the clock stops here, and the actors,
     * which went with the host, are forgotten rather than destroyed a second time. What
     * the crew watches stays connected until the next `#start`, since at the end of a
     * session what it watches is going too; every handler does nothing until then.
     */
    #lost(): void {
        // A pet moved in the last few seconds keeps its new spot: the save waits five, and
        // at the end of a session there are not five left. A spot is the pet's own numbers,
        // not its actor's.
        if (this.#saveTimer !== 0) this.#savePlaces();
        for (const source of [this.#timer, this.#saveTimer, this.#scrollCheck]) {
            if (source !== 0) GLib.source_remove(source);
        }
        this.#timer = 0;
        this.#timerAt = Infinity;
        this.#saveTimer = 0;
        this.#scrollCheck = 0;
        if (this.#drag !== null && this.#drag.holdTimer !== 0) GLib.source_remove(this.#drag.holdTimer);
        this.#drag = null;
        this.#menu = null;
        this.#pets.clear();
        this.#host = null;
        this.#layers = null;
        // What `#stop` forgets too, for a crew started again: a capture's overlay counted
        // open here would hide the next pets, and its close is not heard from now on.
        this.#recording = null;
        this.#scrolling = null;
        this.#moving = false;
        this.#heard = [];
        this.#overlays = 0;
        this.#look = null;
        this.#steps = { count: 0, slowest: 0, over: 0 };
        this.#running = false;
        info('the pets went with the stage');
    }

    #syncKinds(): void {
        const layers = this.#layers;
        if (!this.#running || layers === null) return;
        const wanted = this.#settings.pets;
        for (const [kind, pet] of this.#pets) {
            if (wanted.includes(kind)) continue;
            if (this.#drag?.pet === pet) this.#endDrag(false);
            pet.destroy();
            this.#pets.delete(kind);
        }
        const world = this.#world();
        const spots = this.#settings.petSpots;
        wanted.forEach((kind, i) => {
            if (this.#pets.has(kind)) return;
            const seed = (GLib.random_int() ^ (i * 0x45d9f3b)) >>> 0;
            const pet = new Pet(kind, seed, layers);
            this.#connectInput(pet);
            this.#home(pet, i, wanted.length, spots[kind], world);
            this.#pets.set(kind, pet);
        });
        this.#reactive();
        this.#syncHiding();
    }

    /**
     * Where a pet comes out: where it was last time (`pet-spots`), or spread along the
     * right-hand end of the primary monitor's floor, clear of a dock in the middle.
     */
    #home(pet: Pet, i: number, count: number, spot: Spot | undefined, world: World): void {
        const monitor = spot !== undefined ? world.monitors[spot[0]] : undefined;
        if (spot !== undefined && monitor !== undefined) {
            this.#put(pet, monitor, spot, world);
            return;
        }
        const size = this.#settings.petSize;
        const primary = world.monitors[Main.layoutManager.primaryIndex] ?? world.monitors[0];
        if (primary === undefined) {
            pet.place(world, size, 0, world.height / 2);
            return;
        }
        // The widest pet's width and a half again, at this size on this monitor.
        const unit = artPixel(size, primary.scale) / primary.scale;
        const step = Math.max(...PET_KINDS.map(k => PETS[k].width)) * unit * 1.5;
        pet.place(world, size, primary.work.x + primary.work.width - step * (count - i) + step / 2, primary.work.y + primary.work.height / 2);
    }

    /**
     * A pet at a spot on `monitor`: on the desk when it was on it and the pets may still be
     * anywhere, and otherwise on the floor, as far along.
     */
    #put(pet: Pet, monitor: WorldMonitor, [, along, down, desk]: Spot, world: World): void {
        const [middle, y] = spotOn(monitor, along);
        const size = this.#settings.petSize;
        if (desk && this.#settings.petRoam === 'anywhere')
            pet.placeOnDesk(world, size, middle, monitor.work.y + Math.max(0, Math.min(1, down)) * monitor.work.height);
        else pet.place(world, size, middle, y);
    }

    /** A pet's spot: its monitor, by index, how far along and down its work area it is, and whether on the desk. */
    #placeOf(pet: Pet): Spot | null {
        const index = monitorIndexOf(this.#monitors, pet.body);
        const monitor = this.#monitors[index];
        if (monitor === undefined) return null;
        const { work } = monitor;
        return [
            index,
            (pet.body.x + pet.body.width / 2 - work.x) / Math.max(1, work.width),
            (pet.body.y - work.y) / Math.max(1, work.height),
            pet.view === 'top',
        ];
    }

    #savePlaces(): void {
        if (this.#pets.size === 0) return;
        const spots: Record<string, Spot> = {};
        for (const [kind, pet] of this.#pets) {
            const spot = this.#placeOf(pet);
            if (spot !== null) spots[kind] = spot;
        }
        this.#settings.rememberPetSpots({ ...this.#settings.petSpots, ...spots });
    }

    /**
     * The pets allowed anywhere, or kept to the bottom again: then every pet on the desk
     * falls to the floor under it, from where it is.
     */
    /**
     * Stops every pet on its way somewhere by itself, for *Move on their own* turned off or
     * reduced motion turned on: the brain chooses no such trick from then on, and one
     * already under way -- a walk down the whole desk takes half a minute -- ends too.
     */
    #stopMoving(): void {
        for (const pet of this.#pets.values()) if (pet.movingOnItsOwn) pet.interrupt();
        this.#schedule(now());
    }

    #roamChanged(): void {
        this.#look = null;
        if (!this.#running) return;
        if (this.#settings.petRoam !== 'anywhere') {
            this.#menu?.destroy();
            for (const pet of this.#pets.values()) pet.leaveDesk();
            this.#savePlacesSoon();
        }
        this.#schedule(now());
    }

    /**
     * `pet-spots` a little after the user moved a pet, once it has landed. The shell
     * does not turn its extensions off when the session ends, so `#stop` alone would
     * only ever save when the pets were put away by hand.
     */
    #savePlacesSoon(): void {
        if (this.#saveTimer !== 0) GLib.source_remove(this.#saveTimer);
        this.#saveTimer = GLib.timeout_add_seconds(GLib.PRIORITY_LOW, 5, () => {
            this.#saveTimer = 0;
            this.#savePlaces();
            return GLib.SOURCE_REMOVE;
        });
    }

    /**
     * The monitors changed -- one added or taken away, a scale, an arrangement. Each pet
     * keeps its spot on its monitor rather than its logical position, which at a new scale
     * can be off the monitor's end; and one whose monitor went goes to the first.
     */
    #monitorsChanged(): void {
        // Still connected after `#lost`, when the stage it would measure has gone.
        if (!this.#running) return;
        this.#menu?.destroy();
        this.#endDrag(false);
        const spots = new Map<Pet, Spot>();
        for (const pet of this.#pets.values()) {
            const spot = this.#placeOf(pet);
            if (spot !== null) spots.set(pet, spot);
        }
        this.#measureMonitors();
        const world = this.#world();
        for (const pet of this.#pets.values()) {
            const spot = spots.get(pet) ?? [0, 0.9, 1, false];
            const monitor = world.monitors[spot[0]] ?? world.monitors[0];
            if (monitor === undefined) continue;
            pet.interrupt();
            this.#put(pet, monitor, spot, world);
        }
        this.#syncHiding();
    }

    // --- the world ------------------------------------------------------------------

    #measureMonitors(): void {
        const layout = Main.layoutManager;
        this.#monitors = layout.monitors.map((m, i) => {
            const work = layout.getWorkAreaForMonitor(i);
            return {
                rect: { x: m.x, y: m.y, width: m.width, height: m.height },
                work: { x: work.x, y: work.y, width: work.width, height: work.height },
                scale: global.display.get_monitor_scale(i),
            };
        });
        this.#host?.set_size(global.stage.width, global.stage.height);
    }

    #world(): World {
        let platforms: Platform[] = [];
        try {
            platforms = platformsOf(this.#hooks.pins());
        } catch (e) {
            error('could not read the pins for the pets', e);
        }
        return { monitors: this.#monitors, platforms, width: global.stage.width, height: global.stage.height };
    }

    #temperament(): Temperament {
        return {
            liveliness: this.#settings.petActivity,
            wander: this.#settings.petWander,
            roam: this.#settings.petRoam,
            reduced: !St.Settings.get().enable_animations,
        };
    }

    #surroundings(world: World = this.#world()): Surroundings {
        this.#look ??= { size: this.#settings.petSize, temperament: this.#temperament() };
        return {
            world,
            size: this.#look.size,
            temperament: this.#look.temperament,
            quiet: this.#recording !== null,
            avoid: this.#recording ?? this.#scrolling,
            crowded: (pet, x, y = pet.body.y) => {
                const avoid = this.#recording ?? this.#scrolling;
                const there = { x, y: y - pet.body.height, width: pet.body.width, height: pet.body.height };
                if (avoid !== null && reaches(there, avoid, pet.unit * 2, pet.unit * 2)) return true;
                for (const other of this.#pets.values()) {
                    if (other === pet || !other.visible) continue;
                    if (Math.abs(other.body.x - x) < pet.body.width * 0.8 && Math.abs(other.body.y - y) < pet.body.height) return true;
                }
                return false;
            },
            paint: () => {
                if (this.#painted) return false;
                this.#painted = true;
                return true;
            },
        };
    }

    #resize(): void {
        if (!this.#running) return;
        const world = this.#world();
        for (const pet of this.#pets.values()) pet.resize(world, this.#settings.petSize);
        this.#schedule(now());
    }

    // --- the clock ------------------------------------------------------------------

    /** Wakes the crew at `at`, or sooner if it is already due sooner. */
    #schedule(at: number): void {
        if (!this.#running || !Number.isFinite(at)) return;
        if (this.#timer !== 0 && this.#timerAt <= at) return;
        if (this.#timer !== 0) GLib.source_remove(this.#timer);
        this.#timerAt = at;
        this.#timer = GLib.timeout_add(GLib.PRIORITY_DEFAULT, Math.max(1, Math.round(at - now())), () => {
            this.#timer = 0;
            this.#timerAt = Infinity;
            this.#tick();
            return GLib.SOURCE_REMOVE;
        });
    }

    #tick(): void {
        if (!this.#running) return;
        const t = now();
        this.#painted = false;
        const s = this.#surroundings();
        const heard = this.#heard;
        this.#heard = [];
        for (const event of heard) this.#react(event, t, s);
        let next = Infinity;
        // The pet whose step took longest, for a step that costs a frame (below).
        let slowest: { pet: Pet; ms: number } | null = null;
        for (const pet of this.#pets.values()) {
            const began = now();
            try {
                next = Math.min(next, pet.step(t, s));
            } catch (e) {
                error(`${pet.spec.name} tripped`, e);
                pet.interrupt();
            }
            const took = now() - began;
            if (slowest === null || took > slowest.ms) slowest = { pet, ms: took };
        }
        // A pin being dragged carries its pets every frame.
        if (this.#moving && [...this.#pets.values()].some(p => p.platform !== null)) next = Math.min(next, t + 33);
        this.#schedule(next);
        const ms = now() - t;
        this.#steps.count++;
        this.#steps.slowest = Math.max(this.#steps.slowest, ms);
        if (ms > STEP_BUDGET_MS) this.#steps.over++;
        if (ms > FRAME_MS)
            info(
                `a step of the pets took ${ms.toFixed(2)} ms, with ${heard.length} reactions and ` +
                    `${this.#painted ? 'a new pose' : 'no new pose'} in it; the slowest was ` +
                    (slowest === null ? 'no pet' : `${slowest.pet.spec.name}'s, ${slowest.ms.toFixed(2)} ms, ${slowest.pet.doing}`),
            );
    }

    // --- the pointer ---------------------------------------------------------------------

    #connectInput(pet: Pet): void {
        const sprite = pet.sprite;
        sprite.connect('button-press-event', (_actor: Clutter.Actor, event: Clutter.Event) => this.#press(pet, event));
        sprite.connect('motion-event', (_actor: Clutter.Actor, event: Clutter.Event) => this.#motion(pet, event));
        sprite.connect('button-release-event', (_actor: Clutter.Actor, event: Clutter.Event) => this.#release(pet, event));
    }

    /**
     * Pets take the pointer only on their own pixels (`PetSprite`), and not at all while
     * a scrolling capture runs: its scroll assist scrolls under the pointer, and a pet
     * there would take the scroll the page was meant to get.
     */
    #reactive(): void {
        for (const pet of this.#pets.values()) pet.sprite.reactive = this.#scrolling === null;
    }

    #press(pet: Pet, event: Clutter.Event): boolean {
        const [x, y] = event.get_coords();
        if (!pet.visible || !pet.sprite.drawnAt(x, y)) return Clutter.EVENT_PROPAGATE;
        const button = event.get_button();
        if (button === Clutter.BUTTON_SECONDARY) {
            if (this.#drag === null) this.#openMenu(pet);
            return Clutter.EVENT_STOP;
        }
        if (button !== Clutter.BUTTON_PRIMARY || this.#drag !== null || this.#menu !== null) return Clutter.EVENT_PROPAGATE;
        // Caught in mid-air: held at once.
        const caught = pet.state === 'air';
        if (caught) pet.lift();
        const drag: Drag = {
            pet,
            grab: global.stage.grab(pet.sprite),
            lifted: caught,
            from: { x, y },
            offset: { x: x - pet.body.x, y: y - pet.body.y },
            trail: [[now(), x, y]],
            holdTimer: 0,
        };
        drag.holdTimer = GLib.timeout_add(GLib.PRIORITY_DEFAULT, HOLD_MS, () => {
            drag.holdTimer = 0;
            if (this.#drag === drag && !drag.lifted) this.#lift(drag);
            return GLib.SOURCE_REMOVE;
        });
        this.#drag = drag;
        return Clutter.EVENT_STOP;
    }

    #lift(drag: Drag): void {
        drag.lifted = true;
        drag.pet.lift();
        this.#schedule(now());
    }

    #motion(pet: Pet, event: Clutter.Event): boolean {
        const drag = this.#drag;
        if (drag === null || drag.pet !== pet) return Clutter.EVENT_PROPAGATE;
        // Let go somewhere the release did not reach: it ends here.
        if ((event.get_state() & Clutter.ModifierType.BUTTON1_MASK) === 0) {
            this.#endDrag(true);
            return Clutter.EVENT_STOP;
        }
        const [x, y] = event.get_coords();
        if (!drag.lifted && Math.hypot(x - drag.from.x, y - drag.from.y) > DRAG_SLOP) this.#lift(drag);
        if (!drag.lifted) return Clutter.EVENT_STOP;
        const t = now();
        drag.trail.push([t, x, y]);
        if (drag.trail.length > 12) drag.trail.shift();
        const body = pet.body;
        body.x = Math.max(-body.width / 2, Math.min(global.stage.width - body.width / 2, x - drag.offset.x));
        body.y = Math.max(body.height / 2, Math.min(global.stage.height + body.height / 2, y - drag.offset.y));
        this.#painted = false;
        pet.step(t, this.#surroundings());
        return Clutter.EVENT_STOP;
    }

    #release(pet: Pet, event: Clutter.Event): boolean {
        const drag = this.#drag;
        if (drag === null || drag.pet !== pet || event.get_button() !== Clutter.BUTTON_PRIMARY) return Clutter.EVENT_PROPAGATE;
        this.#endDrag(true);
        return Clutter.EVENT_STOP;
    }

    /** Lets go: a throw when the pet was lifted, a pat when it was only clicked. */
    #endDrag(asked: boolean): void {
        const drag = this.#drag;
        if (drag === null) return;
        this.#drag = null;
        if (drag.holdTimer !== 0) GLib.source_remove(drag.holdTimer);
        drag.grab.dismiss();
        const t = now();
        if (drag.lifted) {
            // The release is the trail's last point: a pet held still and let go drops,
            // however fast it was moving a second before.
            const [x, y] = global.get_pointer();
            drag.trail.push([t, x, y]);
            const velocity = asked ? throwVelocity(drag.trail, drag.pet.unit) : { vx: 0, vy: 0 };
            drag.pet.letGo(velocity.vx, velocity.vy, t, this.#surroundings());
            this.#savePlacesSoon();
        } else if (asked && drag.pet.free) {
            const s = this.#surroundings();
            drag.pet.wake(t, s);
            drag.pet.queue('heart', t, s);
        }
        this.#schedule(t);
    }

    // --- the menu ------------------------------------------------------------------------

    #openMenu(pet: Pet): void {
        const host = this.#host;
        if (this.#menu !== null || host === null || !pet.visible || pet.state === 'air') return;
        const area = this.#monitorOf(pet)?.work ?? { x: 0, y: 0, width: global.stage.width, height: global.stage.height };
        try {
            const menu = new PetMenu(host, {
                pet: pet.box,
                area,
                placement: this.#settings.petMenu,
                chosen: choice => this.#choose(pet, choice),
                closed: () => {
                    this.#menu = null;
                    const t = now();
                    pet.release(t, this.#surroundings());
                    this.#schedule(t);
                },
            });
            this.#menu = menu;
            // It looks at its menu, happily, with its arms up.
            pet.hold({ mood: 'happy', arms: 'up', lookY: menu.side === 'below' ? 1 : -1 });
        } catch (e) {
            error("could not open a pet's menu", e);
        }
        this.#schedule(now());
    }

    #choose(pet: Pet, choice: MenuChoice): void {
        switch (choice) {
            case 'fullscreen': {
                // The pet's own monitor, at once (`spec/14` §10).
                const monitor = this.#monitorOf(pet);
                this.#hooks.capture('fullscreen', monitor === null ? {} : { rect: monitor.rect });
                break;
            }
            case 'history':
                this.#hooks.activate('open-history', null);
                break;
            case 'settings':
                this.#hooks.activate('open-settings', new GLib.Variant('s', 'pets'));
                break;
            case 'hide':
                this.#settings.setPetsEnabled(false);
                break;
            default:
                this.#hooks.capture('all-in-one', { initialMode: choice });
        }
    }

    #monitorOf(pet: Pet): WorldMonitor | null {
        const middle = pet.body.x + pet.body.width / 2;
        const y = pet.body.y - 1;
        return (
            this.#monitors.find(m => middle >= m.rect.x && middle < m.rect.x + m.rect.width && y >= m.rect.y && y < m.rect.y + m.rect.height) ??
            this.#monitors[0] ??
            null
        );
    }

    // --- watching the desktop ----------------------------------------------------------

    #watch(): void {
        const connect = (object: unknown, signal: string, callback: (...args: never[]) => unknown) => {
            const target = object as Signals;
            this.#signals.push([target, target.connect(signal, callback)]);
        };
        connect(Main.layoutManager, 'monitors-changed', () => this.#monitorsChanged());
        connect(global.display, 'workareas-changed', () => {
            // At the end of a session the work areas change after the pets' host has gone,
            // and sizing it logged at every logout (D154).
            if (!this.#running) return;
            this.#measureMonitors();
            this.#schedule(now());
        });
        connect(global.display, 'in-fullscreen-changed', () => this.#syncHiding());
        connect(global.display, 'grab-op-begin', () => {
            this.#moving = true;
            this.#schedule(now());
        });
        connect(global.display, 'grab-op-end', () => {
            this.#moving = false;
        });
        connect(global.workspace_manager, 'active-workspace-changed', () => this.#schedule(now()));
        connect(Main.overview, 'showing', () => this.#syncHiding());
        connect(Main.overview, 'hidden', () => this.#syncHiding());
        // A dialog joins this group when it is made, not when it opens -- the shell makes
        // some at startup and keeps them hidden -- so what counts is a child that shows.
        const dialogs = Main.layoutManager.modalDialogGroup;
        for (const dialog of dialogs.get_children()) this.#watchDialog(dialog);
        connect(dialogs, 'child-added', (_group: unknown, dialog: Clutter.Actor) => {
            this.#watchDialog(dialog);
            this.#syncHiding();
        });
        connect(dialogs, 'child-removed', (_group: unknown, dialog: Clutter.Actor) => {
            this.#forgetDialog(dialog);
            this.#syncHiding();
        });
        connect(St.Settings.get(), 'notify::enable-animations', () => {
            this.#look = null;
            if (!St.Settings.get().enable_animations) this.#stopMoving();
            this.#schedule(now());
        });
        connect(global.backend.get_remote_access_controller(), 'new-handle', (_controller: unknown, handle: Meta.RemoteAccessHandle) =>
            this.#shared(handle),
        );

        const idle = global.backend.get_core_idle_monitor();
        this.#idleWatch = idle.add_idle_watch(SLEEP_AFTER_MS, () => this.#doze());

        this.#unlisten = listenToPets(event => this.#hear(event));
    }

    #watchDialog(dialog: Clutter.Actor): void {
        if (this.#dialogs.has(dialog)) return;
        this.#dialogs.set(dialog, dialog.connect('notify::visible', () => this.#syncHiding()));
    }

    #forgetDialog(dialog: Clutter.Actor): void {
        const id = this.#dialogs.get(dialog);
        if (id === undefined) return;
        this.#dialogs.delete(dialog);
        dialog.disconnect(id);
    }

    #unwatch(): void {
        for (const [object, id] of this.#signals) object.disconnect(id);
        this.#signals = [];
        for (const dialog of [...this.#dialogs.keys()]) this.#forgetDialog(dialog);
        for (const [handle, id] of this.#shares) handle.disconnect(id);
        this.#shares.clear();
        const idle = global.backend.get_core_idle_monitor();
        if (this.#idleWatch !== 0) idle.remove_watch(this.#idleWatch);
        if (this.#activeWatch !== 0) idle.remove_watch(this.#activeWatch);
        this.#idleWatch = 0;
        this.#activeWatch = 0;
        this.#unlisten?.();
        this.#unlisten = null;
    }

    /**
     * A ScreenCast session started. OctoSnap's own -- a recording that is about to start
     * or is on, or a scrolling capture's live view -- is known by the area the pets were
     * told to leave, which they hear of first; any other hides them for as long as it
     * lasts, when `pet-hide-sharing` is on.
     */
    #shared(handle: Meta.RemoteAccessHandle): void {
        if (this.#recording !== null || this.#scrolling !== null || this.#shares.has(handle)) return;
        const id = handle.connect('stopped', () => {
            handle.disconnect(id);
            this.#shares.delete(handle);
            this.#syncHiding();
        });
        this.#shares.set(handle, id);
        this.#syncHiding();
    }

    #syncHiding(): void {
        if (!this.#running) return;
        const t = now();
        const s = this.#surroundings();
        const overview = Main.overview.visible;
        const modal = Main.layoutManager.modalDialogGroup.get_children().some(dialog => dialog.visible);
        const sharing = this.#settings.petHideSharing && this.#shares.size > 0;
        for (const pet of this.#pets.values()) {
            const monitor = this.#monitorOf(pet);
            const index = monitor === null ? -1 : this.#monitors.indexOf(monitor);
            const set = (reason: HideReason, on: boolean) => (on ? pet.hide(reason) : pet.show(reason, t, s));
            set('overview', overview);
            set('modal', modal);
            set('sharing', sharing);
            set('fullscreen', index >= 0 && global.display.get_monitor_in_fullscreen(index));
            set('capture', this.#overlays > 0);
        }
        this.#schedule(t);
    }

    // --- sleep ------------------------------------------------------------------------

    #doze(): void {
        if (!this.#running) return;
        const t = now();
        const s = this.#surroundings();
        for (const pet of this.#pets.values()) if (pet.free) pet.sleep(t, s);
        // Where they are when the desk goes quiet is where they were at the end of the day.
        this.#savePlaces();
        if (this.#activeWatch === 0) {
            this.#activeWatch = global.backend.get_core_idle_monitor().add_user_active_watch(() => {
                this.#activeWatch = 0;
                this.#wakeAll();
            });
        }
        this.#schedule(t);
    }

    #wakeAll(): void {
        if (!this.#running) return;
        const t = now();
        const s = this.#surroundings();
        for (const pet of this.#pets.values()) pet.wake(t, s);
        this.#schedule(t);
    }

    /**
     * While a scrolling capture is up, a look every few seconds at whether its controls
     * still are. The pets hear of the capture when its selection is handed to the app, and
     * the controls going is its ordinary end (`dbus.ts`); an app that never took it up --
     * one that failed to start -- would otherwise leave them out of the area, and off the
     * pointer, for good.
     */
    #watchScrolling(): void {
        if (this.#scrolling === null) {
            if (this.#scrollCheck !== 0) GLib.source_remove(this.#scrollCheck);
            this.#scrollCheck = 0;
            return;
        }
        if (this.#scrollCheck !== 0) return;
        this.#scrollCheck = GLib.timeout_add_seconds(GLib.PRIORITY_LOW, 5, () => {
            if (this.#scrolling === null) {
                this.#scrollCheck = 0;
                return GLib.SOURCE_REMOVE;
            }
            if (this.#hooks.scrolling() !== 'none' || now() - this.#scrollingSince < SCROLL_CONTROLS_WAIT_MS)
                return GLib.SOURCE_CONTINUE;
            this.#scrollCheck = 0;
            info('a scrolling capture was handed on and its controls never showed: the pets stop waiting for it');
            this.#hear({ kind: 'scrolling', active: false, rect: null });
            return GLib.SOURCE_REMOVE;
        });
    }

    // --- what the pets hear ---------------------------------------------------------

    /** Takes in what an event changes about the world now, and leaves the reaction to the next step. */
    #hear(event: PetEvent): void {
        if (!this.#running) return;
        switch (event.kind) {
            case 'recording-soon':
            case 'recording':
                this.#recording = event.rect;
                break;
            case 'recording-stopped':
                if (this.#recording === null) return;
                this.#recording = null;
                break;
            case 'scrolling':
                if (event.active && this.#scrolling === null) this.#scrollingSince = now();
                this.#scrolling = event.active ? event.rect : null;
                this.#reactive();
                this.#watchScrolling();
                break;
            case 'captured':
                this.#lastCaptureAt = now();
                break;
            case 'copied':
                if (now() - this.#lastCaptureAt < COPY_AFTER_CAPTURE_MS) return;
                break;
            case 'overlay':
                this.#overlays = Math.max(0, this.#overlays + (event.open ? 1 : -1));
                this.#syncHiding();
                return;
            default:
                break;
        }
        this.#heard.push(event);
        this.#schedule(now());
    }

    #react(event: PetEvent, t: number, s: Surroundings): void {
        const everyone = (name: string, arg?: number) => {
            for (const pet of this.#pets.values()) {
                if (!pet.free && !pet.aside) continue;
                if (pet.asleep) pet.wake(t, s);
                pet.queue(name, t, s, arg);
            }
        };
        switch (event.kind) {
            case 'captured':
                everyone(event.mode === 'ocr' ? 'read' : 'squint');
                break;
            case 'tick':
                everyone('tick');
                break;
            case 'recording-soon':
                this.#clear(event.rect, event.countdownMs, t, s);
                break;
            case 'recording':
                this.#clear(event.rect, 0, t, s);
                break;
            case 'recording-stopped':
                for (const pet of this.#pets.values()) pet.show('recording', t, s);
                everyone('clap');
                break;
            case 'scrolling':
                if (event.active && event.rect !== null) {
                    // Its live view is a ScreenCast stream, which shows them (D106).
                    this.#clear(event.rect, 0, t, s, 'scrolling');
                    everyone('run', SCROLL_RUN_MS);
                } else if (!event.active) {
                    for (const pet of this.#pets.values()) pet.show('scrolling', t, s);
                }
                break;
            case 'reading':
                everyone('read');
                break;
            case 'copied':
                everyone('love');
                break;
            case 'failed':
                everyone('shrug');
                break;
            case 'flew':
                this.#watchFlight(event.to, t, s);
                break;
            case 'overlay':
                break;
        }
    }

    /**
     * A capture flying to its card: every pet free to watch follows it there with its
     * eyes, and a frog it lands within reach of licks it. Played at once rather than
     * queued, in place of the squint the capture itself started a moment before, which
     * would otherwise use up the flight.
     */
    #watchFlight(card: Box, t: number, s: Surroundings): void {
        const middle = card.x + card.width / 2;
        for (const pet of this.#pets.values()) {
            if (!pet.free || pet.asleep) continue;
            const box = pet.box;
            // A lick is level with the card, from the side; from above the frog only watches.
            const reach = pet.kind === 'frog' && pet.view === 'side' ? lickReach(box, pet.unit, pet.spec.mouth, card) : null;
            if (reach !== null) pet.play('lick', t, s, reach);
            else pet.play('watch', t, s, (middle - (box.x + box.width / 2)) / pet.unit);
        }
    }

    /**
     * Every pet out of `rect` before its first frame is kept: a run to the nearer side
     * when there is time and room, a puff and there when there is room but no time, and
     * out of sight until the recording or the scrolling capture ends when the area is its
     * monitor's whole width. A pet the user is holding is theirs to put where they like.
     *
     * The pets that go stand side by side, the one nearest a way out nearest the area,
     * and clear of the pets already beside it. One with no room to stand clear on either
     * side hides until the end too: on a small screen, beside a wide area, any place left
     * would have it standing on another pet. A pet on the desk can also go up or down it,
     * whichever way out is nearest, but not into the floor's band.
     */
    #clear(rect: Box, countdownMs: number, t: number, s: Surroundings, why: 'recording' | 'scrolling' = 'recording'): void {
        const what = why === 'recording' ? 'recording' : 'scrolling capture';
        const boxOf = (pet: Pet): Box => ({ x: pet.body.x, y: pet.body.y - pet.body.height, width: pet.body.width, height: pet.body.height });
        const going: Pet[] = [];
        const taken: Box[] = [];
        for (const pet of this.#pets.values()) {
            if (pet.state === 'held') continue;
            if (reaches(boxOf(pet), rect, pet.unit * 2)) going.push(pet);
            else taken.push(boxOf(pet));
        }
        const toEdge = (pet: Pet) => {
            const middle = pet.body.x + pet.body.width / 2;
            return Math.min(middle - rect.x, rect.x + rect.width - middle);
        };
        going.sort((p, q) => toEdge(p) - toEdge(q));
        const known = <T>(value: T | null): value is T => value !== null;
        for (const pet of going) {
            const b = pet.body;
            const box = boxOf(pet);
            const gap = pet.unit * 2;
            const work = this.#monitorOf(pet)?.work ?? { x: 0, y: 0, width: global.stage.width, height: global.stage.height };
            const ways = [besideArea(rect, -1, box, gap, taken, work), besideArea(rect, 1, box, gap, taken, work)]
                .filter(known)
                .map(x => ({ x, y: b.y }));
            if (pet.view === 'top' && pet.state !== 'held') {
                const desk = { ...work, height: work.height - (FLOOR_BAND_ART + BAND_CLEARANCE_ART) * pet.unit };
                for (const top of [besideAreaY(rect, -1, box, gap, taken, desk), besideAreaY(rect, 1, box, gap, taken, desk)].filter(known))
                    ways.push({ x: b.x, y: top + b.height });
            }
            const target = ways.sort((p, q) => Math.hypot(p.x - b.x, p.y - b.y) - Math.hypot(q.x - b.x, q.y - b.y))[0];
            if (target === undefined) {
                info(`${pet.spec.name} hides until the ${what} ends: no room beside it`);
                pet.hide(why);
                continue;
            }
            taken.push({ ...box, x: target.x, y: target.y - b.height });
            const where = `${Math.round(target.x)},${Math.round(target.y)}, ${Math.round(b.width)} wide`;
            const across = (target.x - b.x) / pet.unit;
            const down = (target.y - b.y) / pet.unit;
            const walkMs = (Math.hypot(across, down) / LEAVE_SPEED) * 1000 + LEAVE_EXTRA_MS;
            if (pet.visible && pet.state === 'ground' && walkMs < countdownMs) {
                info(`${pet.spec.name} walks out of the ${what} to ${where}`);
                if (pet.asleep) pet.wake(t, s);
                pet.play('leave', t, s, across, down);
            } else {
                info(`${pet.spec.name} pops out of the ${what} to ${where}`);
                pet.relocate(target.x, target.y);
                if (pet.visible) pet.effects.burst(PUFF, 12, false, { x: pet.spec.width / 2, y: 18 }, t);
            }
        }
    }
}
