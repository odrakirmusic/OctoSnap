// SPDX-License-Identifier: GPL-3.0-or-later

import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import type Mtk from 'gi://Mtk';
import Meta from 'gi://Meta';
import GLib from 'gi://GLib';
import Shell from 'gi://Shell';
import St from 'gi://St';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import { PickPixel } from 'resource:///org/gnome/shell/ui/screenshot.js';

import { CardFocus, isAlive } from './cards.js';
import type { DesktopIcons } from './desktop.js';
import type { RecordingCoordinator } from './recording.js';
import { currentMonitor } from './capture.js';
import {
    type CaptureModeName,
    type CaptureOptions,
    IMPLEMENTED_MODES,
    runCapture,
} from './flow.js';
import { metaToDict } from './app.js';
import { Handoff } from './handoff.js';
import { placeAtEdge } from './place.js';
import { type ScrollCaptureOptions, runScrollingCapture } from './scroll.js';
import type { Settings } from './settings.js';
import { ulidFrom } from './ulid.js';

import {
    APP_BUS_NAME,
    EXTENSION_VERSION,
    PROTOCOL_VERSION,
    SHELL_BUS_NAME,
    SHELL_INTERFACE,
    SHELL_OBJECT_PATH,
} from './protocol.js';
import { ScrollFrame, ScrollSession, type ScrollDirection, isDirection } from './scrollAssist.js';
import { PinShadows } from './pinShadow.js';
import { tellPets } from './pets/events.js';
import { playCue } from './sound.js';
import { SettingsBridge } from './settingsBridge.js';
import { setAnnouncedSpool, spoolDir } from './spool.js';
import { error, info, recentLog } from './log.js';

/**
 * The slice of the `spec/10` §3.1 interface the milestones so far need. The rest (the
 * pointer stream, `Ping`) arrives with the milestone that uses it, so the extension stays
 * thin (`spec/12`, "Keep the extension small").
 */
const INTERFACE_XML = `
<node>
  <interface name="${SHELL_INTERFACE}">
    <method name="Version">
      <arg type="s" direction="out" name="version"/>
      <arg type="u" direction="out" name="protocol"/>
    </method>
    <method name="GetMonitors">
      <arg type="aa{sv}" direction="out" name="monitors"/>
    </method>
    <method name="GetLog">
      <arg type="as" direction="out" name="lines"/>
    </method>
    <method name="SetSpool">
      <arg type="s" direction="in" name="dir"/>
    </method>
    <method name="GetSettings">
      <arg type="s" direction="in" name="schema"/>
      <arg type="a{sv}" direction="out" name="values"/>
    </method>
    <method name="SetSetting">
      <arg type="s" direction="in" name="schema"/>
      <arg type="s" direction="in" name="key"/>
      <arg type="v" direction="in" name="value"/>
    </method>
    <method name="ResetSetting">
      <arg type="s" direction="in" name="schema"/>
      <arg type="s" direction="in" name="key"/>
    </method>
    <method name="GetKeybindings">
      <arg type="a(ssas)" direction="out" name="bindings"/>
    </method>
    <method name="GetWallpaper">
      <arg type="b" direction="in" name="dark"/>
      <arg type="s" direction="out" name="path"/>
    </method>
    <signal name="SettingChanged">
      <arg type="s" name="schema"/>
      <arg type="s" name="key"/>
      <arg type="v" name="value"/>
    </signal>
    <method name="BeginCapture">
      <arg type="s" direction="in" name="mode"/>
      <arg type="a{sv}" direction="in" name="options"/>
      <arg type="s" direction="out" name="handle"/>
    </method>
    <method name="CancelCapture">
      <arg type="s" direction="in" name="handle"/>
    </method>
    <method name="SetClipboardImage">
      <arg type="s" direction="in" name="path"/>
    </method>
    <method name="SetClipboardText">
      <arg type="s" direction="in" name="text"/>
    </method>
    <method name="PlaySound">
      <arg type="s" direction="in" name="cue"/>
    </method>
    <method name="PlaceWindow">
      <arg type="s" direction="in" name="object_path"/>
      <arg type="s" direction="in" name="role"/>
      <arg type="a{sv}" direction="in" name="options"/>
      <arg type="(iiii)" direction="out" name="landed"/>
    </method>
    <method name="MoveWindowBy">
      <arg type="s" direction="in" name="object_path"/>
      <arg type="i" direction="in" name="dx"/>
      <arg type="i" direction="in" name="dy"/>
      <arg type="(iiii)" direction="out" name="landed"/>
    </method>
    <method name="SetWindowShadow">
      <arg type="s" direction="in" name="object_path"/>
      <arg type="a{sv}" direction="in" name="look"/>
    </method>
    <method name="FocusWindow">
      <arg type="s" direction="in" name="object_path"/>
      <arg type="b" direction="in" name="focus"/>
      <arg type="b" direction="out" name="focused"/>
    </method>
    <method name="PickColor">
      <arg type="b" direction="out" name="ok"/>
      <arg type="d" direction="out" name="r"/>
      <arg type="d" direction="out" name="g"/>
      <arg type="d" direction="out" name="b"/>
    </method>
    <method name="ToggleDesktopIcons">
      <arg type="s" direction="in" name="what"/>
      <arg type="b" direction="out" name="hidden"/>
      <arg type="b" direction="out" name="available"/>
    </method>
    <method name="SetRecordingState">
      <arg type="s" direction="in" name="state"/>
      <arg type="u" direction="in" name="elapsed_ms"/>
    </method>
    <method name="ShowRecordingFrame">
      <arg type="(iiii)" direction="in" name="rect"/>
      <arg type="b" direction="in" name="visible"/>
    </method>
    <method name="ShowScrollFrame">
      <arg type="s" direction="in" name="object_path"/>
      <arg type="(iiii)" direction="in" name="rect"/>
    </method>
    <method name="StartScrollAssist">
      <arg type="(iiii)" direction="in" name="rect"/>
      <arg type="s" direction="in" name="direction"/>
      <arg type="i" direction="in" name="step"/>
      <arg type="s" direction="out" name="handle"/>
    </method>
    <method name="GrabFrame">
      <arg type="s" direction="in" name="handle"/>
      <arg type="s" direction="out" name="path"/>
    </method>
    <method name="EndScrollAssist">
      <arg type="s" direction="in" name="handle"/>
    </method>
    <signal name="CaptureCompleted">
      <arg type="s" name="handle"/>
      <arg type="a{sv}" name="result"/>
    </signal>
    <signal name="CaptureCancelled">
      <arg type="s" name="handle"/>
      <arg type="s" name="reason"/>
    </signal>
    <property name="CaptureActive" type="b" access="read"/>
  </interface>
</node>`;

type MonitorDict = Record<string, GLib.Variant>;

/**
 * The app's window with this GTK object path, or `null`.
 *
 * `spec/01` §3.3 documents the path as `/org/octosnap/App/window/N`; the real value is
 * **`/io/github/odrakirmusic/OctoSnap/window/N`**, because GTK derives it from the
 * application id. Measured off the running app in `docs/spikes/01-02`; the documented
 * value would never have matched.
 */
function findAppWindow(objectPath: string): Meta.Window | null {
    for (const { window } of appWindows()) {
        if (window.get_gtk_window_object_path() === objectPath) return window;
    }
    return null;
}

/** Every window that reports a GTK object path, with the path it reported. */
function appWindows(): { window: Meta.Window; path: string }[] {
    const found: { window: Meta.Window; path: string }[] = [];
    for (const actor of global.get_window_actors()) {
        const window = actor.meta_window;
        if (window === null) continue;
        let path: string | null = null;
        try {
            path = window.get_gtk_window_object_path();
        } catch {
            // Not a GTK window, or too early in its life to have one. Neither is an
            // error worth logging on a call that runs for every card.
            continue;
        }
        if (path !== null) found.push({ window, path });
    }
    return found;
}

/**
 * Why a window could not be found, in enough detail to tell the two causes apart.
 *
 * "no window at <path>" was the whole error, and it cost a debugging session: the two
 * possible causes -- the window is not there *yet*, and the window is there under a
 * different path -- look identical from the app's side and want opposite fixes. Listing
 * what was actually seen distinguishes them at a glance.
 */
function noWindowError(objectPath: string): Error {
    const seen = appWindows().map(w => w.path);
    const detail =
        seen.length === 0
            ? 'no window on the stage reports a GTK object path'
            : `saw ${seen.join(', ')}`;
    return new Error(`no window at ${objectPath}; ${detail}`);
}

/**
 * Why a placement was abandoned for a window that *was* there.
 *
 * Deliberately not `noWindowError`. That one exists to separate "not there yet" from
 * "there under a different path", and this is a third cause wanting a third fix: the
 * window was found, waited for, and closed while we waited. Reported as "no window" it
 * would send the next reader hunting a path mismatch that never happened -- which is the
 * debugging session `noWindowError` was written to prevent, so borrowing it here would
 * undo it.
 */
function windowGoneError(objectPath: string): Error {
    return new Error(`window at ${objectPath} closed before it could be placed`);
}

/**
 * Makes a window that has just been moved appear to slide there.
 *
 * `spec/04` §5: "New card pushes existing cards away from the corner with a 200 ms
 * ease-out slide. Closing a card collapses the gap with the same animation."
 *
 * The window is moved first and only its **actor** is animated, which is the shell's own
 * idiom for this: the real window is immediately where it belongs -- correct for input,
 * stacking and anyone asking where it is -- and only the picture of it catches up. The
 * alternative, easing `move_frame` itself, would be a real reconfigure per frame.
 *
 * This is also why the animation lives here rather than in the app. `docs/decisions.md`
 * D31 concluded that a card can only animate inside its own 16 pt shadow band, and that
 * is true *of the app*: a GTK widget cannot draw outside its window. The compositor has
 * no such limit -- the actor is on the stage, not in the window -- so a card falling a
 * whole card-height is free here and impossible there. D44.
 */
function slideActor(window: Meta.Window, from: Mtk.Rectangle, durationMs: number): void {
    if (durationMs <= 0) return;
    const actor = window.get_compositor_private<Meta.WindowActor>();
    if (actor === null) return;

    const landed = window.get_frame_rect();
    const dx = from.x - landed.x;
    const dy = from.y - landed.y;
    // Nothing moved, or it moved so little that a 200 ms slide would read as a stutter.
    if (Math.abs(dx) < 2 && Math.abs(dy) < 2) return;

    actor.set_position(actor.x + dx, actor.y + dy);
    actor.ease({
        x: actor.x - dx,
        y: actor.y - dy,
        duration: durationMs,
        mode: Clutter.AnimationMode.EASE_OUT_CUBIC,
    });
}

/**
 * Runs `then` once the window is one the compositor can be asked about, and **never from
 * inside a frame**.
 *
 * "Drawable" here means Mutter has processed the window's first buffer, which is when it
 * gains a stack position and a real frame rect. The proxy for that is a non-empty frame
 * rect; the signal for it is the window actor's `first-frame`, which is the idiom the
 * shell's own extensions use for the same question.
 *
 * The `later` around the callback is not tidiness -- without it this **crashes the
 * session**:
 *
 * ```text
 * libmutter:ERROR:../src/compositor/compositor.c:467:
 *   invalidate_top_window_actor_for_views: assertion failed: (!priv->frame_in_progress)
 * ```
 *
 * `first-frame` is emitted *during* the frame that paints the window. `make_above` and
 * `stick` restack, restacking invalidates the top window actor, and doing that mid-paint
 * is the one thing that assertion forbids. Mutter does not survive it: the whole shell
 * goes down, taking every application's windows with it. A `BEFORE_REDRAW` later runs
 * outside the paint, which is where any restack belongs. `docs/decisions.md` D36.
 *
 * The timeout is not defensive decoration either. A window that never draws -- a client
 * that mapped a surface and then hung -- would otherwise leave the caller's D-Bus call
 * unanswered until *its* timeout, turning a slow window into an error the caller reports
 * as a failed placement. Placing it late and imperfectly is better than not answering.
 *
 * Which is why there are two outcomes and not one. That 500 ms of grace is long enough
 * for a window to be created and closed inside it, and `then` cannot run for a window
 * that is no longer there: it would measure and move something Mutter has unmanaged, and
 * `PlaceWindow` would answer with the rect of nothing. So `gone` says so instead. What
 * neither outcome may do is stay silent, for the reason in the paragraph above.
 */
/**
 * Every `whenDrawable` still waiting, for `ShellService.destroy` (D135). Its timeout is a
 * main-loop source, and the review at extensions.gnome.org requires `disable()` to remove
 * every one.
 */
const drawableWaits = new Set<() => void>();

function whenDrawable(
    window: Meta.Window,
    then: () => void,
    gone: () => void,
    stopped: () => void,
): void {
    const actor = window.get_compositor_private<Meta.WindowActor>();
    if (actor === null) {
        // No actor is not "too early", it is "not composited". Mutter clears this when it
        // unmanages a window, so for one `findAppWindow` just handed us *out of the actor
        // list* it means the window closed in between. Waiting for a first frame that
        // cannot arrive would burn the full fallback before saying so.
        gone();
        return;
    }

    const frame = window.get_frame_rect();
    if (frame.width > 0 && frame.height > 0) {
        then();
        return;
    }

    let done = false;
    let handler = 0;
    let timeout = 0;

    const finish = (): void => {
        if (done) return;
        done = true;
        drawableWaits.delete(abandon);

        // Release only what is still there to be released. One `actor.disconnect(handler)`
        // on the fallback path, for a card created and closed inside the 500 ms, produced
        // two complaints:
        //
        // ```text
        // Gjs-CRITICAL: Object MetaWindowActorWayland (0x...), has been already disposed
        //   -- impossible to access it.
        // GLib-GObject-CRITICAL: gsignal.c:2723: instance '0x...' has no handler with id
        // ```
        //
        // Neither is catchable, which is the part that decides the shape of the fix: GJS
        // logs the disposed access and calls into `g_signal_handler_disconnect` anyway,
        // and the second line is that C function warning about the pointer it was handed.
        // A `try`/`catch` would have hidden nothing. The disconnect has to *not happen*,
        // so aliveness is asked of the window, which outlives its actor -- never of the
        // actor, which is the object already unsafe to touch. `docs/decisions.md` D49.
        if (handler !== 0 && isAlive(window)) actor.disconnect(handler);
        if (timeout !== 0) GLib.source_remove(timeout);

        global.compositor.get_laters().add(Meta.LaterType.BEFORE_REDRAW, () => {
            // Asked again rather than carried down from above: the later is a frame away,
            // and a frame is time enough for the window to go.
            if (isAlive(window)) then();
            else gone();
            return GLib.SOURCE_REMOVE;
        });
    };

    handler = actor.connect('first-frame', finish);
    timeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, FIRST_FRAME_TIMEOUT_MS, () => {
        info('placing a window that never reported a first frame');
        // Dropped before `finish` can reach for it. D32: removing a GLib source that has
        // already fired is a `GLib-CRITICAL`, and the rule it left is that whichever side
        // ends a one-shot source releases the handle.
        timeout = 0;
        finish();
        return GLib.SOURCE_REMOVE;
    });

    // The extension turned off first: release what `finish` would, the same way, and
    // answer the caller rather than leave it waiting for its own timeout.
    const abandon = (): void => {
        if (done) return;
        done = true;
        drawableWaits.delete(abandon);
        if (handler !== 0 && isAlive(window)) actor.disconnect(handler);
        if (timeout !== 0) GLib.source_remove(timeout);
        stopped();
    };
    drawableWaits.add(abandon);
}

/** Comfortably inside the app's 2 s call timeout, and far longer than a frame. */
const FIRST_FRAME_TIMEOUT_MS = 500;

/**
 * Fails a D-Bus call with a message rather than a stack trace.
 *
 * An async method has no `throw` path: GJS turns a synchronous throw into a D-Bus error
 * for it, but by the time the first frame arrives the call has long since returned to the
 * main loop, so an uncaught throw there would log in the shell's journal and leave the
 * caller waiting for its timeout.
 */
/**
 * Whether a D-Bus sender is the process that owns the app's well-known name.
 *
 * `spec/10` §7's rule for the methods that inject input. A unique name is assigned by the
 * bus and cannot be spoofed, so comparing it to whoever currently owns
 * `io.github.odrakirmusic.OctoSnap` answers "is this our app" exactly -- and answers it
 * *now*, so a name handed on to a different process does not carry the permission with it.
 *
 * Synchronous, with a short timeout, because it runs on the compositor's main loop: once
 * per scrolling capture, against the bus daemon, which is the cheapest call there is.
 */
function callerIsTheApp(sender: string | null): boolean {
    if (sender === null) return false;
    try {
        const reply = Gio.DBus.session.call_sync(
            'org.freedesktop.DBus',
            '/org/freedesktop/DBus',
            'org.freedesktop.DBus',
            'GetNameOwner',
            new GLib.Variant('(s)', [APP_BUS_NAME]),
            new GLib.VariantType('(s)'),
            Gio.DBusCallFlags.NONE,
            200,
            null,
        );
        const [owner] = reply.deep_unpack() as [string];
        return owner === sender;
    } catch (e) {
        // Nobody owns the name: the app is not running, so this is not the app.
        error(`could not resolve ${APP_BUS_NAME}`, e);
        return false;
    }
}

/** How many wallpaper copies are kept: the light and dark ones, and the two before them. */
const WALLPAPER_COPIES = 4;

/**
 * The wallpaper `GetWallpaper` hands over: the file, and where its copy beside the spool
 * goes -- named by the picture's URI and modification time, so a changed wallpaper is a
 * new copy and an unchanged one is copied once. `null` when there is no wallpaper file.
 */
function wallpaperCopy(dark: boolean): { source: Gio.File; copy: string } | null {
    const id = 'org.gnome.desktop.background';
    const schema = Gio.SettingsSchemaSource.get_default()?.lookup(id, true) ?? null;
    if (schema === null) return null;
    const settings = new Gio.Settings({ settings_schema: schema });
    const key = dark && schema.has_key('picture-uri-dark') ? 'picture-uri-dark' : 'picture-uri';
    if (!schema.has_key(key)) return null;
    const uri = settings.get_string(key);
    if (uri === '') return null;
    const source = Gio.File.new_for_uri(uri);
    const path = source.get_path();
    if (path === null || !GLib.file_test(path, GLib.FileTest.IS_REGULAR)) return null;
    const modified = source
        .query_info('time::modified', Gio.FileQueryInfoFlags.NONE, null)
        .get_modification_date_time()?.to_unix() ?? 0;
    const dot = path.lastIndexOf('.');
    const suffix = dot > path.lastIndexOf('/') ? path.slice(dot) : '';
    const name = GLib.compute_checksum_for_string(GLib.ChecksumType.SHA256, `${uri}\n${modified}`, -1);
    const dir = GLib.build_filenamev([GLib.path_get_dirname(spoolDir()), 'wallpaper']);
    GLib.mkdir_with_parents(dir, 0o700);
    return { source, copy: GLib.build_filenamev([dir, `${(name ?? 'wallpaper').slice(0, 24)}${suffix}`]) };
}

/**
 * Keeps the newest [`WALLPAPER_COPIES`] copies in `dir`, the one just made among them.
 * Only names this module gives -- 24 hex digits and a suffix -- are ever removed.
 */
function pruneWallpaperCopies(dir: string): void {
    const found: [string, number][] = [];
    const children = Gio.File.new_for_path(dir).enumerate_children(
        'standard::name,time::modified', Gio.FileQueryInfoFlags.NOFOLLOW_SYMLINKS, null);
    for (let info = children.next_file(null); info !== null; info = children.next_file(null)) {
        if (/^[0-9a-f]{24}(\.[A-Za-z0-9]+)?$/.test(info.get_name()))
            found.push([info.get_name(), info.get_modification_date_time()?.to_unix() ?? 0]);
    }
    children.close(null);
    found.sort((a, b) => b[1] - a[1]);
    for (const [name] of found.slice(WALLPAPER_COPIES))
        Gio.File.new_for_path(GLib.build_filenamev([dir, name])).delete(null);
}

function returnError(invocation: Gio.DBusMethodInvocation, e: unknown): void {
    invocation.return_error_literal(
        Gio.DBusError.quark(),
        Gio.DBusError.FAILED,
        e instanceof Error ? e.message : String(e),
    );
}

/**
 * Which monitor a placement is measured against.
 *
 * Not `window.get_monitor()` by default, which is where Mutter happened to map the
 * window -- for a never-presented utility surface that is the primary display, not the
 * one the user is looking at. `spec/04` §10 item 1 asks for the card to appear "on the
 * pointer's monitor", and `spec/04` §2's "Move to active screen" setting makes that a
 * choice, so the caller says which rule it wants and this resolves it.
 *
 * The pointer lookup is `currentMonitor()`'s, and for the reason recorded there:
 * `global.display.get_current_monitor()` was measured returning the same monitor for
 * every pointer position on a two-display rig, so it is a fallback rather than the
 * question.
 */
function placementMonitor(window: Meta.Window, options: MonitorDict): number {
    switch (stringOption(options, 'monitor')) {
        case 'pointer': {
            // Logged, and not merely for diagnosis. `spec/11`'s working agreement asks
            // for every acceptance list to pass on mixed-scale dual monitors, and on one
            // display this rule is unfalsifiable: every answer is monitor 0, so a
            // placement that ignored the pointer entirely would look correct. The choice
            // is stated here so a harness can assert on the *rule's* answer separately
            // from where the window landed -- two ends of one fact, the way a card
            // measures its own controls rather than trusting its layout arithmetic.
            const [px, py] = global.get_pointer();
            const chosen = currentMonitor();
            info(`pointer at ${px},${py} is on monitor ${chosen.index} ` +
                 `(${chosen.rect.width}x${chosen.rect.height}+${chosen.rect.x}+${chosen.rect.y})`);
            return chosen.index;
        }
        case 'primary':
            return Main.layoutManager.primaryIndex;
        case 'window':
            return window.get_monitor();
        default:
            return window.get_monitor();
    }
}

function numberOption(options: MonitorDict, key: string): number | null {
    const value = options[key];
    if (value === undefined) return null;
    const type = value.get_type_string();
    if (type === 'i' || type === 'u' || type === 'x' || type === 'd')
        return Number(value.deep_unpack());
    return null;
}

function stringOption(options: MonitorDict, key: string): string | null {
    const value = options[key];
    if (value === undefined || value.get_type_string() !== 's') return null;
    return value.deep_unpack() as string;
}

function booleanOption(options: MonitorDict, key: string): boolean | null {
    const value = options[key];
    if (value === undefined || value.get_type_string() !== 'b') return null;
    return value.deep_unpack() as boolean;
}

/** Modes `BeginCapture` accepts (`spec/10` §3.1). */
const KNOWN_MODES = [
    'all-in-one', 'area', 'window', 'fullscreen', 'previous-area',
    'self-timer', 'scrolling', 'ocr', 'record',
] as const;

export class ShellService {
    #exported: Gio.DBusExportedObject | null = null;
    #nameOwnerId = 0;
    #settings: Settings;
    #desktopIcons: DesktopIcons | null;
    #recording: RecordingCoordinator | null;
    /**
     * `spec/03` §6's state machine allows exactly one capture at a time: the overlay
     * takes a modal grab, so a second one would fight the first for input. A second
     * request is refused with `busy` rather than queued.
     */
    #activeHandle: string | null = null;
    /**
     * Who has the keyboard while cards are up (`spec/04` §3), shared with the watcher
     * that puts focus back when a card maps -- see `cards.ts` for why that is needed.
     */
    #focus = new CardFocus();
    /** `spec/07` §3.1's pin shadows, which the compositor draws (D132). */
    #shadows = new PinShadows();
    /**
     * `spec/07` §1.3's scroll assist, at most one at a time.
     *
     * One, because the session holds a virtual pointer device and parks the real pointer
     * inside its selection: two would fight over where the pointer is, and every scroll
     * would land in whichever selection won.
     */
    #scroll: ScrollSession | null = null;

    /**
     * The scrolling capture's pill while it is on screen, known from its placement, so
     * the Scrolling Capture key and the panel menu can press its button: the pill never
     * takes the keyboard, since the page it captures needs it (D137).
     */
    #scroller: { window: Meta.Window; unmanaged: number; frame: ScrollFrame | null } | null = null;

    /** The pins on screen, known from their placement, for the pets to sit on (`spec/14` §4). */
    #pins = new Map<Meta.Window, number>();

    /** Watches the app that announced its spool, so the announcement leaves with it. */
    #spoolWatch = 0;

    /** D123: the settings a sandboxed app mirrors, from here. */
    #bridge: SettingsBridge | null = null;

    constructor(
        settings: Settings,
        desktopIcons: DesktopIcons | null = null,
        recording: RecordingCoordinator | null = null,
    ) {
        this.#settings = settings;
        this.#desktopIcons = desktopIcons;
        this.#recording = recording;
    }

    export(): void {
        // Before the name is owned, so a card cannot map into an unwatched session.
        this.#focus.watch();
        this.#nameOwnerId = Gio.bus_own_name(
            Gio.BusType.SESSION,
            SHELL_BUS_NAME,
            Gio.BusNameOwnerFlags.NONE,
            (connection: Gio.DBusConnection) => {
                try {
                    this.#exported = Gio.DBusExportedObject.wrapJSObject(INTERFACE_XML, this);
                    this.#exported.export(connection, SHELL_OBJECT_PATH);
                    info(`exported ${SHELL_INTERFACE} at ${SHELL_OBJECT_PATH}`);
                } catch (e) {
                    error('failed to export the shell interface', e);
                }
                try {
                    this.#bridge = new SettingsBridge(this.#settings.raw);
                    this.#bridge.watch((schema, key, value) => {
                        this.#exported?.emit_signal(
                            'SettingChanged',
                            new GLib.Variant('(ssv)', [schema, key, value]),
                        );
                    });
                } catch (e) {
                    error('could not share the settings over the bus', e);
                }
            },
            null,
            () => error(`lost or could not acquire the bus name ${SHELL_BUS_NAME}`),
        );
    }

    /**
     * `spec/10` §2 requires disable() to restore everything the extension changed.
     * Unexport before releasing the name so a caller never sees the name owned with no
     * object behind it.
     */
    destroy(): void {
        for (const abandon of [...drawableWaits]) abandon();
        this.#focus.stop();
        this.#shadows.destroy();
        this.#endScroll();
        this.#forgetScroller();
        for (const window of [...this.#pins.keys()]) this.#forgetPin(window);
        if (this.#spoolWatch !== 0) {
            Gio.bus_unwatch_name(this.#spoolWatch);
            this.#spoolWatch = 0;
        }
        setAnnouncedSpool(null);
        this.#bridge?.destroy();
        this.#bridge = null;
        if (this.#exported) {
            try {
                this.#exported.unexport();
            } catch (e) {
                error('failed to unexport the shell interface', e);
            }
            this.#exported = null;
        }
        if (this.#nameOwnerId !== 0) {
            Gio.bus_unown_name(this.#nameOwnerId);
            this.#nameOwnerId = 0;
        }
        info('shell interface withdrawn');
    }

    // --- D-Bus methods. Names must match the interface XML exactly. ---------------

    /**
     * `spec/10` §3.1. Puts a PNG on the clipboard as `image/png`.
     *
     * This has to be the extension rather than the app: `spec/01` §2 row 15 notes that
     * `Gdk.Clipboard` only works once the app has had focus or input, and the whole point
     * of copy-on-capture is that it happens without the app ever taking focus. `St.Clipboard`
     * is what GNOME's own screenshot UI uses. Verified in M0 spike 10.
     *
     * Throws a D-Bus error on failure, because the app needs to know whether to tell the
     * user the copy happened.
     */
    SetClipboardImage(path: string): void {
        const file = Gio.File.new_for_path(path);
        const [ok, bytes] = file.load_contents(null);
        if (!ok) throw new Error(`could not read ${path}`);

        St.Clipboard.get_default().set_content(
            St.ClipboardType.CLIPBOARD,
            'image/png',
            new GLib.Bytes(bytes),
        );
        info(`clipboard set from ${path} (${bytes.length} bytes)`);
        tellPets({ kind: 'copied' });
    }

    /**
     * `spec/07` §2.1's "recognized text is copied", for the same reason the image goes
     * through here: on Wayland the selection belongs to whoever has a surface and a
     * recent input serial, and a text capture copies without the app ever taking focus.
     */
    SetClipboardText(text: string): void {
        St.Clipboard.get_default().set_text(St.ClipboardType.CLIPBOARD, text);
        info(`clipboard set to ${text.length} characters of text`);
        tellPets({ kind: 'copied' });
    }

    /**
     * The app's cues, by name (`soundCues.ts`): text recognised, copied, pinned, a
     * recording started or stopped, and Settings' preview of each shutter. None can ride
     * on the shutter -- recognition finishes a second or more after the pixels were read,
     * so the app says when.
     *
     * Unconditional, unlike the shutter. The shutter is the extension's own event and it
     * gates itself on `capture-sound`; these are the app's, `spec/08` §1's `ui-sounds` is
     * the app's key, and a switch read in two places is a switch that disagrees with
     * itself.
     */
    PlaySound(name: string): void {
        if (!playCue(name)) throw new Error(`OctoSnap has no sound called "${name}"`);
        info(`played the ${name} sound`);
    }

    /**
     * `spec/10` §3.1's `PlaceWindow`: put one of the app's windows where it belongs.
     *
     * On Wayland a client cannot position itself, which is the whole reason this exists.
     * The app maps a card, asks for it to be placed, and gets back **the rect the window
     * actually got** -- because `move_frame` is clamped silently (`docs/spikes/01-02`:
     * `(0,0)` comes back as `(0,32)`), so the request is not the answer and a caller that
     * stacked against its request would drift.
     *
     * Options, per `spec/10` §3.1: `x`/`y` place explicitly; `edge` and `offset` place a
     * stack member from the **work area** (`spec/04` §2); `above` and `sticky` default on
     * for the roles that need them. `offset` is a distance rather than the documented
     * `index` because cards take their size from their capture and so have different
     * heights -- `place.ts` explains who knows what.
     *
     * **Asynchronous**, because a window can be findable before it is placeable. A card
     * asks to be placed as soon as it has painted its first frame, and at that moment
     * Mutter has a `MetaWindow` for it but has not put it in the stack or processed its
     * buffer. Touching it then produced two complaints on every capture:
     *
     * ```text
     * meta_window_set_stack_position_no_sync: assertion 'window->stack_position >= 0' failed
     * Buggy client (io.github.odrakirmusic.OctoSnap) committed initial non-empty
     *   content without acknowledging configuration, working around.
     * ```
     *
     * Mutter says "working around" and means it -- neither was fatal -- but a compositor
     * that logs an assertion failure every time you take a screenshot is not a state to
     * ship, and a worked-around race is still a race. So the reply waits for the window
     * actor's first frame; see `whenDrawable`. `docs/decisions.md` D34.
     *
     * A window that closes *during* that wait is answered with an error and not a rect.
     * The alternative is a rect measured off an unmanaged window, which the caller cannot
     * tell from a real one and would stack against. `docs/decisions.md` D49.
     */
    PlaceWindowAsync(
        [objectPath, role, options]: [string, string, MonitorDict],
        invocation: Gio.DBusMethodInvocation,
    ): void {
        let window: Meta.Window;
        try {
            const found = findAppWindow(objectPath);
            if (found === null) throw noWindowError(objectPath);
            window = found;
        } catch (e) {
            returnError(invocation, e);
            return;
        }

        whenDrawable(
            window,
            () => {
                try {
                    const landed = this.#place(window, role, options);
                    if (role === 'scroller') this.#trackScroller(window);
                    if (role === 'pin') this.#trackPin(window);
                    invocation.return_value(new GLib.Variant('((iiii))', [landed]));
                } catch (e) {
                    returnError(invocation, e);
                }
            },
            () => returnError(invocation, windowGoneError(objectPath)),
            () => returnError(invocation, new Error('the extension was turned off before the window drew')),
        );
    }

    #place(
        window: Meta.Window,
        role: string,
        options: MonitorDict,
    ): [number, number, number, number] {

        // The caller's size, not Mutter's, when the caller offers one.
        //
        // `get_frame_rect()` answers `0x0` for a window whose first buffer the compositor
        // has not processed yet -- and a card asks to be placed the moment it is drawable,
        // which is exactly then. Placing a zero-height window against the bottom edge put
        // the card at `y = 1200` on a 1200 px display: correct arithmetic, wrong input.
        //
        // Waiting for a non-zero frame rect would be polling for something the client
        // already knows. A GTK window is the size the client made it, so the client says
        // so and the compositor answers where it ended up. Each half is asked only what
        // it is in a position to know, which is the same division `place.ts` describes.
        const frame = window.get_frame_rect();
        const size = {
            width: numberOption(options, 'width') ?? frame.width,
            height: numberOption(options, 'height') ?? frame.height,
        };

        const explicitX = numberOption(options, 'x');
        const explicitY = numberOption(options, 'y');

        let target: { x: number; y: number };
        if (explicitX !== null && explicitY !== null) {
            target = { x: explicitX, y: explicitY };
        } else {
            const workArea = Main.layoutManager.getWorkAreaForMonitor(placementMonitor(window, options));
            const edge = stringOption(options, 'edge') === 'right' ? 'right' : 'left';
            target = placeAtEdge(
                { x: workArea.x, y: workArea.y, width: workArea.width, height: workArea.height },
                size,
                edge,
                numberOption(options, 'offset') ?? 0,
                numberOption(options, 'inset') ?? 0,
            );
        }

        // `user_op: false` -- this is the application placing its own furniture, not the
        // user dragging a window, and the flag does not lift the clamp anyway.
        const before = window.get_frame_rect();
        window.move_frame(false, target.x, target.y);
        slideActor(window, before, numberOption(options, 'animate_ms') ?? 0);
        if (booleanOption(options, 'above') !== false) window.make_above();
        if (booleanOption(options, 'sticky') !== false) window.stick();

        // `spec/01` §5's "never focus-steal", checked at the one moment it is observable.
        this.#focus.check(`placing ${role}`);

        // Read back, always. This is the return value the spec asks for and the reason
        // the caller can stack accurately.
        const landed = window.get_frame_rect();
        if (landed.x !== target.x || landed.y !== target.y) {
            info(
                `placed ${role} at ${landed.x},${landed.y} after asking for ` +
                `${target.x},${target.y}; Mutter clamped it`,
            );
        }
        return [landed.x, landed.y, landed.width, landed.height];
    }

    /**
     * `spec/07` §3.2's `MoveWindowBy`: nudge a window by a delta.
     *
     * Exists because a Wayland client cannot move itself, and a pinned screenshot's arrow
     * keys (`spec/07` §3.1: "arrow keys move 1 px, Shift 10 px") are a client-side
     * gesture with a compositor-side effect. Relative rather than absolute so the caller
     * does not have to track where Mutter last put the window -- which it cannot know
     * between calls, since the user can drag it in the meantime.
     *
     * Returns the landed rect for the same reason `PlaceWindow` does: `move_frame` is
     * clamped silently, so a pin nudged against the edge of the screen stops there and
     * the caller should know rather than accumulate a phantom offset.
     */
    MoveWindowBy(objectPath: string, dx: number, dy: number): [number, number, number, number] {
        const window = findAppWindow(objectPath);
        if (window === null) throw noWindowError(objectPath);

        const frame = window.get_frame_rect();
        window.move_frame(false, frame.x + dx, frame.y + dy);
        const landed = window.get_frame_rect();
        return [landed.x, landed.y, landed.width, landed.height];
    }

    /**
     * `spec/07` §3.1's pin shadow (D132), drawn here around the window because a pin's
     * window is the capture's rect to the pixel. `look` carries `radius`, the pin's corners
     * in logical pixels, and `opacity`, the pin's own from 0 to 1. An opacity of 0 takes
     * the shadow away. The app calls it again whenever the pin's opacity changes, and
     * after every placing, since a pin shown again is a new window.
     */
    SetWindowShadow(objectPath: string, look: MonitorDict): void {
        const window = findAppWindow(objectPath);
        if (window === null) throw noWindowError(objectPath);
        this.#shadows.set(window, {
            radius: Math.max(0, numberOption(look, 'radius') ?? 0),
            opacity: numberOption(look, 'opacity') ?? 1,
        });
    }

    /**
     * `spec/04` §3's keyboard focus policy: lend a card the keyboard while the pointer is
     * over it, and give it back when the pointer leaves.
     *
     * This is what makes Ctrl+C on a hovered card work the way CleanShot's Cmd+C does
     * without the card ever interrupting anything. It has to live in the extension for
     * two reasons: a Wayland client cannot focus itself, and only the compositor knows
     * what was focused *before* -- the app would have to guess, and would guess wrong for
     * exactly the case that matters, where the previous window is another application's.
     *
     * `focus: false` restores rather than merely unfocusing. Unfocusing alone would leave
     * the session with no focused window, and the user's next keystroke would go nowhere;
     * `spec/04` §3's promise is that "typing in another app is not interrupted", which
     * means the keystroke after the pointer leaves must land where the one before it did.
     */
    FocusWindow(objectPath: string, focus: boolean): boolean {
        const window = findAppWindow(objectPath);
        if (window === null) throw noWindowError(objectPath);

        if (focus) {
            this.#focus.lend(window);
            return true;
        }
        return this.#focus.restore();
    }

    /**
     * `spec/10` §3.1's `PickColor() -> (b ok, d r, d g, d b)`: `spec/05` §2's pipette,
     * "via the extension, from anywhere on screen".
     *
     * The shell's own picker, not a new one. `PickPixel` is what GNOME's screenshot
     * service runs for its `PickColor` -- the `color-pick` loupe cursor recoloured to the
     * pixel under it, a click to choose, Escape to back out -- and it is exported from
     * `ui/screenshot.js`, so the extension can run the same widget the user already knows
     * from Settings' colour pickers. It reads the pixel through `Shell.Screenshot`'s
     * `pick_color`, which is the API `spec/01` §2 row 8 verified.
     *
     * `ok` is false when the user pressed Escape; that is an answer, not an error.
     * Refused while a capture is running, for the same reason `BeginCapture` refuses a
     * second capture: the overlay holds a modal grab and two grabs would fight.
     *
     * **Asynchronous**, like `PlaceWindow`: the reply waits for a person.
     */
    PickColorAsync(_params: [], invocation: Gio.DBusMethodInvocation): void {
        if (this.#activeHandle !== null) {
            returnError(invocation, new Error('busy: a capture is in progress'));
            return;
        }
        void (async () => {
            try {
                const picker = new PickPixel(new Shell.Screenshot());
                const color = await picker.pickAsync();
                if (color === null || color === undefined) {
                    info('pick colour: cancelled');
                    invocation.return_value(new GLib.Variant('(bddd)', [false, 0, 0, 0]));
                    return;
                }
                // `Cogl.Color` carries 8-bit channels; the wire form is unit floats, as
                // the shell's own `PickColor` answers.
                const { red, green, blue } = color;
                info(`pick colour: ${red},${green},${blue}`);
                invocation.return_value(
                    new GLib.Variant('(bddd)', [true, red / 255, green / 255, blue / 255]),
                );
            } catch (e) {
                error('pick colour failed', e);
                returnError(invocation, e);
            }
        })();
    }

    /**
     * `spec/07` §1.3: `StartScrollAssist(rect, direction, step_px) -> handle`.
     *
     * **Asynchronous only so the caller can be checked.** `spec/10` §7:
     *
     * > The extension exposes powerful methods (screen capture, input injection).
     * > Restrict the D-Bus service to the session bus (default) and check the caller's
     * > unique name is the app's well-known name owner for `StartScrollAssist`.
     *
     * The sender is on the invocation and nowhere else, and GJS only hands the invocation
     * to an `*Async` method. The two calls that follow need no check of their own: they
     * take a handle nobody else has been told.
     *
     * `step_px` is read and dropped. It was how far `ScrollStep` scrolled, and that went
     * with auto-scroll (D153); it stays in the signature so an app and a shell from either
     * side of that still agree on it.
     */
    StartScrollAssistAsync(
        params: [[number, number, number, number], string, number],
        invocation: Gio.DBusMethodInvocation,
    ): void {
        const [[x, y, width, height], direction] = params;
        if (!callerIsTheApp(invocation.get_sender())) {
            returnError(invocation, new Error('scroll assist is the app\'s to start'));
            return;
        }
        if (!isDirection(direction)) {
            returnError(invocation, new Error(`unknown scroll direction ${direction}`));
            return;
        }
        if (width <= 0 || height <= 0) {
            returnError(invocation, new Error('an empty selection cannot be scrolled'));
            return;
        }
        // A second session would fight the first for the pointer, so the older one goes.
        // Ending it rather than refusing is deliberate: the app that started it may have
        // died, and a user who asks for a scrolling capture twice should get one.
        this.#endScroll();
        const handle = ulidFrom(Date.now(), () => GLib.random_int_range(0, 32));
        const session = new ScrollSession(
            handle,
            { x, y, width, height },
            direction as ScrollDirection,
            () => this.#endScroll(),
        );
        try {
            session.start();
        } catch (e) {
            error('could not start scroll assist', e);
            returnError(invocation, e);
            return;
        }
        this.#scroll = session;
        tellPets({ kind: 'scrolling', active: true, rect: { x, y, width, height } });
        invocation.return_value(new GLib.Variant('(s)', [handle]));
    }

    /** `spec/07` §1.3: `GrabFrame(handle) -> path`. One frame of the selection, as a PNG. */
    GrabFrameAsync(params: [string], invocation: Gio.DBusMethodInvocation): void {
        const session = this.#session(params[0]);
        if (session === null) {
            returnError(invocation, new Error(`no scroll assist at ${params[0]}`));
            return;
        }
        void (async () => {
            try {
                const path = await session.grab();
                invocation.return_value(new GLib.Variant('(s)', [path]));
            } catch (e) {
                error('could not grab a scroll frame', e);
                returnError(invocation, e);
            }
        })();
    }

    /**
     * `spec/07` §1.3: `EndScrollAssist(handle)`.
     *
     * Silent about a handle that is already gone. The app calls this from the end of its
     * loop *and* from every failure path, which is what makes the device impossible to
     * leak; refusing the second call would make the belt-and-braces into a log of errors.
     */
    EndScrollAssist(handle: string): void {
        if (this.#scroll?.handle === handle) this.#endScroll();
    }

    /**
     * Where a scrolling capture is: no pill, a pill waiting for Start, or one whose
     * session is running. Read by the key and the menu, each when it is pressed (D137).
     */
    get scrolling(): 'none' | 'ready' | 'running' {
        if (this.#scroller === null) return 'none';
        return this.#scroll === null ? 'ready' : 'running';
    }

    #trackScroller(window: Meta.Window): void {
        if (this.#scroller?.window === window) return;
        this.#forgetScroller();
        const unmanaged = window.connect('unmanaged', () => {
            this.#forgetScroller();
            // The controls going is the capture over, however it ended -- Done, the x,
            // or a failure before Start -- which is when the pets it moved come back.
            tellPets({ kind: 'scrolling', active: false, rect: null });
        });
        this.#scroller = { window, unmanaged, frame: null };
    }

    /** The pin windows on screen now, for `pets/crew.ts`. */
    pinWindows(): Meta.Window[] {
        return [...this.#pins.keys()];
    }

    #trackPin(window: Meta.Window): void {
        if (this.#pins.has(window)) return;
        this.#pins.set(window, window.connect('unmanaged', () => this.#forgetPin(window)));
    }

    #forgetPin(window: Meta.Window): void {
        const id = this.#pins.get(window);
        if (id === undefined) return;
        this.#pins.delete(window);
        try {
            window.disconnect(id);
        } catch (e) {
            error('could not stop watching a pin', e);
        }
    }

    #forgetScroller(): void {
        const scroller = this.#scroller;
        if (scroller === null) return;
        this.#scroller = null;
        scroller.frame?.destroy();
        try {
            scroller.window.disconnect(scroller.unmanaged);
        } catch (e) {
            error('could not stop watching the scrolling pill', e);
        }
    }

    /** The live session, if that is the handle it has. */
    #session(handle: string): ScrollSession | null {
        if (this.#scroll === null || this.#scroll.handle !== handle) {
            error(`no scroll assist at ${handle}`);
            return null;
        }
        return this.#scroll;
    }

    #endScroll(): void {
        if (this.#scroll === null) return;
        const session = this.#scroll;
        this.#scroll = null;
        tellPets({ kind: 'scrolling', active: false, rect: null });
        try {
            session.end();
        } catch (e) {
            error('could not end scroll assist cleanly', e);
        }
    }

    /** `spec/10` §3.1's `CaptureActive` property. */
    get CaptureActive(): boolean {
        return this.#activeHandle !== null;
    }

    /**
     * `spec/10` §3.1: "Returns immediately; result arrives via `HandleCapture` on the app
     * **and** the `CaptureCompleted` signal."
     *
     * Returning a handle before the work is done is what lets the caller stay responsive
     * and lets `CancelCapture` refer to a capture in flight. It also means every failure
     * path has to report through `CaptureCancelled` rather than by throwing, or a caller
     * that is waiting for a signal waits forever.
     */
    BeginCapture(mode: string, options: Record<string, GLib.Variant>): string {
        const handle = ulidFrom(Date.now(), () => GLib.random_int_range(0, 32));

        if (this.#activeHandle !== null) {
            this.#emitCancelled(handle, 'busy');
            return handle;
        }

        if (!(KNOWN_MODES as readonly string[]).includes(mode)) {
            this.#emitCancelled(handle, `error:unknown mode '${mode}'`);
            return handle;
        }
        // `spec/07` §1.1's selection, which is the only part of a scrolling capture the
        // extension owns: the app runs the loop, so this handle covers the *overlay* and
        // no `CaptureCompleted` will ever follow it. Documented on the method rather than
        // given a new cancellation reason, because a caller that wants the result asks the
        // app, and the CLI -- the only caller there is -- only wants the overlay opened.
        if (mode === 'scrolling') {
            this.#activeHandle = handle;
            void this.#runScrolling(handle, parseOptions(options));
            return handle;
        }

        if (!(IMPLEMENTED_MODES as readonly string[]).includes(mode)) {
            // Honest rather than silently capturing the wrong thing: these modes need the
            // overlay (spec/03) or their own controllers (spec/06, spec/07).
            this.#emitCancelled(handle, `error:mode '${mode}' is not implemented yet`);
            return handle;
        }

        this.#activeHandle = handle;
        void this.#runCapture(handle, mode as CaptureModeName, parseOptions(options));
        return handle;
    }

    /**
     * `spec/10` §3.1. Cancelling a handle that is not running is not an error.
     *
     * What this does **not** do is tear down the overlay, and that is a real gap rather
     * than an oversight: the overlay owns a modal grab and lives inside the promise
     * `#runCapture` is awaiting, so interrupting it from here needs a cancel token
     * threaded through the flow. Until that exists, this marks the handle cancelled and
     * the overlay's own Esc is the way out. Recorded so the next person does not read the
     * `CaptureActive` property going false as proof the overlay is gone.
     */
    CancelCapture(handle: string): void {
        if (this.#activeHandle !== handle) return;
        this.#activeHandle = null;
        this.#emitCancelled(handle, 'user');
    }

    /**
     * The selection half of a scrolling capture, for a `octosnap://scrolling-capture`
     * with no rectangle.
     *
     * The handle's life is the overlay's, not the capture's: once the app has been handed
     * the rectangle the extension is out of it until the app calls `StartScrollAssist`,
     * and holding `#activeHandle` past that would block every other capture for as long as
     * the user kept scrolling.
     */
    async #runScrolling(handle: string, options: CaptureOptions): Promise<void> {
        try {
            // Built key by key rather than spread, because `exactOptionalPropertyTypes`
            // makes `{ rect: undefined }` a different thing from `{}` -- and the
            // difference is exactly the one that matters here: an absent rect means "ask
            // the user", not "capture nothing".
            const ask: ScrollCaptureOptions = {};
            if (options.rect !== undefined) ask.rect = options.rect;
            if (options.start !== undefined) ask.start = options.start;
            const started = await runScrollingCapture(this.#settings, ask);
            if (!started) this.#emitCancelled(handle, 'user');
        } catch (e) {
            error(`scrolling capture ${handle} failed`, e);
            this.#emitCancelled(handle, `error:${e instanceof Error ? e.message : String(e)}`);
        } finally {
            this.#activeHandle = null;
        }
    }

    async #runCapture(
        handle: string,
        mode: CaptureModeName,
        options: CaptureOptions,
    ): Promise<void> {
        try {
            const handoff = new Handoff();
            const meta = await runCapture(mode, this.#settings, {
                ...options,
                desktopIcons: this.#desktopIcons ?? undefined,
                onRecord: handoff.onRecord,
                onScrolling: handoff.onScrolling,
            });
            if (meta === null) {
                // The user dismissed the overlay, or All-In-One handed on to a recording or
                // a scrolling capture, which this handle does not cover -- as it does not
                // for `scrolling` itself. `spec/10` §3.1's reason vocabulary has no other
                // word for either. Emitted first, then the hand-off runs with the handle
                // still held, so a second `BeginCapture` is refused through the countdown.
                this.#emitCancelled(handle, 'user');
                await handoff.run(this.#settings, this.#desktopIcons, this.#recording);
                return;
            }
            this.#emitCompleted(handle, meta);
        } catch (e) {
            error(`capture ${handle} failed`, e);
            this.#emitCancelled(handle, `error:${e instanceof Error ? e.message : String(e)}`);
        } finally {
            this.#activeHandle = null;
        }
    }

    #emitCompleted(handle: string, meta: Parameters<typeof metaToDict>[0]): void {
        try {
            this.#exported?.emit_signal(
                'CaptureCompleted',
                new GLib.Variant('(sa{sv})', [handle, metaToDict(meta)]),
            );
        } catch (e) {
            error('could not emit CaptureCompleted', e);
        }
    }

    #emitCancelled(handle: string, reason: string): void {
        info(`capture ${handle} cancelled: ${reason}`);
        try {
            this.#exported?.emit_signal(
                'CaptureCancelled',
                new GLib.Variant('(ss)', [handle, reason]),
            );
        } catch (e) {
            error('could not emit CaptureCancelled', e);
        }
    }

    Version(): [string, number] {
        return [EXTENSION_VERSION, PROTOCOL_VERSION];
    }

    /**
     * `spec/11` M7's crash log: this extension's recent lines, for the report the app
     * writes when asked to (`log.ts`). Added without a protocol bump, like `PickColor`:
     * an app that finds the method missing writes its report without them.
     */
    GetLog(): string[] {
        return recentLog();
    }

    /**
     * D123: one shared schema's keys and values, for the app's mirror of it. What may be
     * read is `settingsBridge.ts`'s list, not the caller's.
     */
    GetSettingsAsync(params: [string], invocation: Gio.DBusMethodInvocation): void {
        const [schema] = params;
        if (!callerIsTheApp(invocation.get_sender())) {
            returnError(invocation, new Error('the settings are the app\'s to read'));
            return;
        }
        try {
            if (this.#bridge === null) throw new Error('the settings are not shared');
            invocation.return_value(new GLib.Variant('(a{sv})', [this.#bridge.values(schema)]));
        } catch (e) {
            returnError(invocation, e);
        }
    }

    /** D123: one write from the app's mirror, checked against the key's type and range. */
    SetSettingAsync(params: [string, string, GLib.Variant], invocation: Gio.DBusMethodInvocation): void {
        const [schema, key, value] = params;
        if (!callerIsTheApp(invocation.get_sender())) {
            returnError(invocation, new Error('the settings are the app\'s to write'));
            return;
        }
        try {
            if (this.#bridge === null) throw new Error('the settings are not shared');
            this.#bridge.set(schema, key, value);
            invocation.return_value(null);
        } catch (e) {
            error(`refused to set ${schema} ${key}`, e);
            returnError(invocation, e);
        }
    }

    /** D123: a key back to its default, which is how the Print keys are given back. */
    ResetSettingAsync(params: [string, string], invocation: Gio.DBusMethodInvocation): void {
        const [schema, key] = params;
        if (!callerIsTheApp(invocation.get_sender())) {
            returnError(invocation, new Error('the settings are the app\'s to reset'));
            return;
        }
        try {
            if (this.#bridge === null) throw new Error('the settings are not shared');
            this.#bridge.reset(schema, key);
            invocation.return_value(null);
        } catch (e) {
            returnError(invocation, e);
        }
    }

    /** D123: every binding a new shortcut could collide with, for the conflict check. */
    GetKeybindingsAsync(_params: [], invocation: Gio.DBusMethodInvocation): void {
        if (!callerIsTheApp(invocation.get_sender())) {
            returnError(invocation, new Error('the keybindings are the app\'s to read'));
            return;
        }
        try {
            if (this.#bridge === null) throw new Error('the settings are not shared');
            invocation.return_value(new GLib.Variant('(a(ssas))', [this.#bridge.keybindings()]));
        } catch (e) {
            returnError(invocation, e);
        }
    }

    /**
     * D124: the desktop wallpaper, for the background behind a window capture, copied to
     * where the app can read it. A sandboxed app cannot see the host's background setting
     * (its GSettings are its own) or the file it names (`/usr/share/backgrounds`, the
     * user's Pictures). The copy goes beside the spool rather than in it, so the history
     * janitor, which sweeps the spool's PNGs, never mistakes it for a capture, and it is
     * named for the file and its modification time, so it is copied once. `''` when there
     * is no wallpaper to give.
     */
    GetWallpaperAsync(params: [boolean], invocation: Gio.DBusMethodInvocation): void {
        const [dark] = params;
        if (!callerIsTheApp(invocation.get_sender())) {
            returnError(invocation, new Error('the wallpaper is the app\'s to ask for'));
            return;
        }
        let found: { source: Gio.File; copy: string } | null;
        try {
            found = wallpaperCopy(dark);
        } catch (e) {
            error('could not find the wallpaper to hand over', e);
            returnError(invocation, e);
            return;
        }
        if (found === null) {
            invocation.return_value(new GLib.Variant('(s)', ['']));
            return;
        }
        const { source, copy } = found;
        if (GLib.file_test(copy, GLib.FileTest.EXISTS)) {
            invocation.return_value(new GLib.Variant('(s)', [copy]));
            return;
        }
        // Copied off the compositor's main loop, since a wallpaper is megabytes and this is
        // gnome-shell; and under another name until it is whole, so that a copy cut short
        // is never what the next call hands over.
        const partial = Gio.File.new_for_path(`${copy}.partial`);
        source.copy_async(partial, Gio.FileCopyFlags.OVERWRITE, GLib.PRIORITY_DEFAULT, null, null,
            (_file: Gio.File | null, result: Gio.AsyncResult) => {
                try {
                    source.copy_finish(result);
                    partial.move(Gio.File.new_for_path(copy), Gio.FileCopyFlags.OVERWRITE, null, null);
                    invocation.return_value(new GLib.Variant('(s)', [copy]));
                } catch (e) {
                    error('could not copy the wallpaper', e);
                    returnError(invocation, e);
                    return;
                }
                try {
                    pruneWallpaperCopies(GLib.path_get_dirname(copy));
                } catch (e) {
                    error('could not prune the wallpaper copies', e);
                }
            });
    }

    /**
     * D122: the app says where its spool is, so a capture lands where the app that reads
     * it can see it -- `~/.var/app/<id>/cache` for the Flatpak, `~/.cache` for a native
     * build. Only the app may say it, and what it said holds while it is on the bus;
     * with no app running, `spool.ts` works out which app D-Bus will start. Added without
     * a protocol bump: an older app never calls it, and the cold-start rule serves it.
     */
    SetSpoolAsync(params: [string], invocation: Gio.DBusMethodInvocation): void {
        const [dir] = params;
        if (!callerIsTheApp(invocation.get_sender())) {
            returnError(invocation, new Error('the spool is the app\'s to set'));
            return;
        }
        if (!GLib.path_is_absolute(dir)) {
            returnError(invocation, new Error(`a spool is an absolute path, not ${dir}`));
            return;
        }
        setAnnouncedSpool(dir);
        if (this.#spoolWatch === 0) {
            this.#spoolWatch = Gio.bus_watch_name(
                Gio.BusType.SESSION,
                APP_BUS_NAME,
                Gio.BusNameWatcherFlags.NONE,
                null,
                () => setAnnouncedSpool(null),
            );
        }
        invocation.return_value(null);
    }

    /**
     * `DSK-01` over the bus, for the app's `desktop-icons` action and so the CLI and
     * `octosnap://toggle-desktop-icons`. Answers the state after the change and whether
     * this session has desktop icons at all, so the caller can say "nothing to hide"
     * rather than "hidden" on vanilla GNOME. Added without a protocol bump, like
     * `PickColor`: an app that finds the method missing knows the extension is older.
     */
    ToggleDesktopIcons(what: string): [boolean, boolean] {
        const icons = this.#desktopIcons;
        if (icons === null) return [false, false];
        const available = icons.available();
        const hidden = icons.apply(what);
        return [hidden, available];
    }

    /**
     * `spec/10` §3.1's `SetRecordingState`: the app tells the compositor the recording's
     * phase, and the compositor drives the panel red-and-timer, hides the desktop icons and
     * watches the app for a crash (`spec/06` §4). `state` is one of `idle`, `countdown`,
     * `recording`, `processing`; `elapsed_ms` is the recording's own clock.
     *
     * Added without a protocol bump, like `PickColor`: an app calling a method this
     * extension does not have knows the extension is older than it.
     */
    SetRecordingState(state: string, elapsedMs: number): void {
        this.#recording?.setState(state, elapsedMs);
    }

    /**
     * `spec/10` §3.1's `ShowRecordingFrame`: the red outline around the recorded region
     * appears, moves, or is taken away. GJS hands the `(iiii)` argument in as a four-number
     * array.
     */
    ShowRecordingFrame(rect: [number, number, number, number], visible: boolean): void {
        const [x, y, width, height] = rect;
        this.#recording?.showFrame({ x, y, width, height }, visible);
    }

    /**
     * The blue outline round a scrolling capture's selection, while its pill waits for
     * Start as well as after (D152).
     *
     * The scroll assist draws the same outline once Start is pressed (D78). Before that,
     * with the selection overlay closed, nothing said which rectangle the pill was for.
     * This one belongs to the pill's window and goes with it -- Done, the x, a failure
     * before Start, or the app itself going away -- so the app has nothing to take down
     * and nothing it can leave behind.
     */
    ShowScrollFrame(objectPath: string, rect: [number, number, number, number]): void {
        const [x, y, width, height] = rect;
        if (width <= 0 || height <= 0) throw new Error('an empty selection has no outline');
        const window = findAppWindow(objectPath);
        if (window === null) throw noWindowError(objectPath);
        this.#trackScroller(window);
        const scroller = this.#scroller;
        if (scroller === null) return;
        scroller.frame?.destroy();
        scroller.frame = new ScrollFrame({ x, y, width, height });
        info(`the scrolling pill's selection is outlined: ${width}x${height} at ${x},${y}`);
    }

    /**
     * `spec/10` §3.1 asks for connector, x, y, w, h, scale, primary, current, work_area.
     * **`connector` is not reported here.** GNOME Shell's `Monitor` is a plain JS object
     * carrying only `index, x, y, width, height, geometry_scale` -- verified by
     * introspection on GNOME 50.1 -- and `MetaMonitorManager` exposes nothing useful to
     * JS either. The connector has to come from `org.gnome.Mutter.DisplayConfig`, which
     * any session process can call, so the app resolves it rather than the extension
     * growing a D-Bus round trip. See `docs/spikes/15-monitor-metadata.md`.
     *
     * Everything here is in stage/logical coordinates, per the single coordinate policy
     * in `spec/01` §1: the extension never speaks physical pixels across the boundary.
     * `scale` is the fractional monitor scale (1.25 on the current target machine);
     * `geometry-scale` is Mutter's integer ceiling of it, and the two differ on any
     * fractionally scaled output -- which is why both are reported rather than one.
     */
    GetMonitors(): MonitorDict[] {
        const layout = Main.layoutManager;
        const display = global.display;
        const currentIndex = display.get_current_monitor();
        const primaryIndex = layout.primaryIndex;

        return layout.monitors.map((monitor, index): MonitorDict => {
            const workArea = layout.getWorkAreaForMonitor(index);

            // Snake case, and not optional. @girs types this as `geometryScale`, but at
            // runtime the property is `geometry_scale`: Shell's Monitor is a plain JS
            // class, not a GObject, so there is no automatic camelCase alias. Reading
            // the camelCase name yields undefined, which GLib then coerces to 0 -- a
            // silent wrong answer rather than an error, which is how this survived a
            // clean typecheck.
            const geometryScale =
                (monitor as unknown as { geometry_scale?: number }).geometry_scale ?? 1;

            return {
                'index': GLib.Variant.new_int32(index),
                'x': GLib.Variant.new_int32(monitor.x),
                'y': GLib.Variant.new_int32(monitor.y),
                'width': GLib.Variant.new_int32(monitor.width),
                'height': GLib.Variant.new_int32(monitor.height),
                'scale': GLib.Variant.new_double(display.get_monitor_scale(index)),
                'geometry-scale': GLib.Variant.new_int32(geometryScale),
                'primary': GLib.Variant.new_boolean(index === primaryIndex),
                'current': GLib.Variant.new_boolean(index === currentIndex),
                'work-area': new GLib.Variant('(iiii)', [
                    workArea.x, workArea.y, workArea.width, workArea.height,
                ]),
            };
        });
    }
}

/**
 * Reads `BeginCapture`'s options dictionary (`spec/10` §3.1).
 *
 * Unknown keys are ignored and a wrongly typed one is treated as absent, because the app
 * and the extension are versioned separately -- refusing a capture over an option this
 * build has not heard of would be the wrong trade.
 */
function parseOptions(options: Record<string, GLib.Variant>): CaptureOptions {
    const out: CaptureOptions = {};

    const rect = options['rect'];
    if (rect?.get_type_string() === '(iiii)') {
        const [x, y, width, height] = rect.deep_unpack() as [number, number, number, number];
        if (width > 0 && height > 0) out.rect = { x, y, width, height };
    }

    const action = options['action'];
    if (action?.get_type_string() === 's') {
        const value = action.deep_unpack() as string;
        if (value !== '') out.requestedAction = value;
    }

    const modifiers = options['modifiers'];
    if (modifiers?.get_type_string() === 'u')
        out.modifiers = modifiers.deep_unpack() as number;

    // `spec/08` §6 makes these user settings, so an *absent* option must leave the
    // setting alone rather than arriving as false. That is why the wire form omits them
    // instead of sending a default, and why these three stay `undefined` here.
    const freeze = options['freeze'];
    if (freeze?.get_type_string() === 'b') out.freeze = freeze.deep_unpack() as boolean;

    const cursor = options['cursor'];
    if (cursor?.get_type_string() === 'b') out.cursor = cursor.deep_unpack() as boolean;

    const timer = options['timer'];
    if (timer?.get_type_string() === 'i') {
        const seconds = timer.deep_unpack() as number;
        if (seconds >= 0) out.timer = seconds;
    }

    // `spec/07` §2.1's second text shortcut, absent-means-the-setting for the same reason.
    const linebreaks = options['linebreaks'];
    if (linebreaks?.get_type_string() === 'b')
        out.linebreaks = linebreaks.deep_unpack() as boolean;

    const start = options['start'];
    if (start?.get_type_string() === 'b') out.start = start.deep_unpack() as boolean;

    return out;
}
