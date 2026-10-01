// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Pixel capture. `spec/01` §2 rows 4 and 5: `Shell.Screenshot` is the same API GNOME's own
 * screenshot UI uses, and it is only reachable from inside the shell -- the D-Bus
 * interface of the same name is access-controlled (`docs/spikes/14-shell-dbus-access-control.md`),
 * which is precisely why capture lives in the extension.
 *
 * Coordinates in, logical; pixels out, physical. `spec/01` §1.
 */

import Clutter from 'gi://Clutter';
import Cogl from 'gi://Cogl';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import type Graphene from 'gi://Graphene';
import Mtk from 'gi://Mtk';
import Shell from 'gi://Shell';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import System from 'system';

import type { Size } from './place.js';
import type { ViewRate } from './recordChoices.js';
import { bufferScale, imageScale } from './overlay/selection.js';
import { type PixelRect, type Sides, origin, pastPainted, repaintPlan } from './repaint.js';
import type { Rect } from './spool.js';
import { error, info } from './log.js';

// GJS turns the GAsyncResult pair into a promise. Done once, at module load, because
// promisifying the same prototype method twice throws.
Gio._promisify(Shell.Screenshot.prototype, 'screenshot');
Gio._promisify(Shell.Screenshot.prototype, 'screenshot_window');
Gio._promisify(Shell.Screenshot.prototype, 'screenshot_stage_to_content');
Gio._promisify(Shell.Screenshot, 'composite_to_stream');

type PromisifiedShooter = Shell.Screenshot & {
    screenshot(includeCursor: boolean, stream: Gio.OutputStream): Promise<unknown>;
    screenshot_window(
        includeFrame: boolean,
        includeCursor: boolean,
        stream: Gio.OutputStream,
    ): Promise<Mtk.Rectangle | [Mtk.Rectangle] | [boolean, Mtk.Rectangle]>;
    screenshot_stage_to_content(): Promise<
        [Clutter.Content, number, Clutter.Content | null, Graphene.Point | null, number]
    >;
};

type PromisifiedCompositor = {
    composite_to_stream(
        texture: Cogl.Texture,
        x: number,
        y: number,
        width: number,
        height: number,
        scale: number,
        cursor: Cogl.Texture | null,
        cursorX: number,
        cursorY: number,
        cursorScale: number,
        stream: Gio.OutputStream,
    ): Promise<unknown>;
};

/**
 * Where a capture's time went, in milliseconds. `docs/spikes/17` put the whole of it down
 * to the PNG encode from the curve's shape alone; this is the split, measured.
 */
export interface CaptureCost {
    /** Repainting the stage and filling the edges: the compositor's main loop, synchronous. */
    paintMs: number;
    /**
     * `composite_to_stream` before it returns: the texture read back from the GPU and
     * turned into a pixbuf, synchronously on the main loop as well.
     */
    readMs: number;
    /** The PNG encode and the write, on a worker thread, until the promise settles. */
    encodeMs: number;
}

/**
 * A capture whose pixels are read and whose file is still being written.
 *
 * The encode is on a worker thread and is nearly all of a large capture's time
 * (`CaptureCost`: 831 of 870 ms for a 1920x1200 screen in a nested shell), and nothing the
 * user sees has to wait for it. The fly shows `pixels`, the GPU's copy of exactly what is
 * going into the file; only the app waits for `written`, because it must not hear of a
 * file that is not there yet (`spec/10` §8).
 */
export interface StartedCapture {
    /**
     * The captured pixels, physical, cut to the capture and without the pointer the file
     * may have composited in. `null` if they could not be wrapped, and then the fly waits
     * for the file, as every capture's did before D127.
     */
    pixels: Clutter.Content | null;
    /** Settles when the PNG is closed, with where the time went. */
    written: Promise<CaptureCost>;
}

/**
 * `crop` of `texture` as content an actor can show. A sub-texture, which is a view rather
 * than a copy, when the crop is not the whole texture.
 */
function pixelsOf(texture: Cogl.Texture, crop: PixelRect): Clutter.Content | null {
    try {
        const whole = crop.x === 0 && crop.y === 0 &&
            crop.width === texture.get_width() && crop.height === texture.get_height();
        return Clutter.TextureContent.new_from_texture(texture, whole ? null : mtkRect(crop));
    } catch (e) {
        error('could not hand the capture to the fly; it will wait for the file', e);
        return null;
    }
}

/** `toFile` around `composite_to_stream`, its synchronous half timed apart from its thread. */
async function compositeToFile(
    path: string,
    composite: (stream: Gio.OutputStream) => Promise<unknown>,
): Promise<Omit<CaptureCost, 'paintMs'>> {
    let called = 0;
    let returned = 0;
    await toFile(path, stream => {
        called = GLib.get_monotonic_time();
        const pending = composite(stream);
        returned = GLib.get_monotonic_time();
        return pending;
    });
    const settled = GLib.get_monotonic_time();
    return { readMs: (returned - called) / 1000, encodeMs: (settled - returned) / 1000 };
}

/**
 * Opens a PNG for writing and closes it whatever happens.
 *
 * The close is not optional: leaving the stream open leaves a truncated PNG behind, and
 * `spec/10` §8's recovery path would then find a twin pointing at a file that cannot be
 * decoded -- worse than no capture at all.
 */
async function toFile(path: string, write: (stream: Gio.OutputStream) => Promise<unknown>) {
    const stream = Gio.File.new_for_path(path).replace(
        null,
        false,
        Gio.FileCreateFlags.NONE,
        null,
    );
    try {
        await write(stream);
    } finally {
        stream.close(null);
    }
}

/**
 * Captures a logical rect to a PNG file, straight from the live screen.
 *
 * The fast path, and the one `docs/spikes/17` measured: fixed 6.6 ms plus ~80.8 ms per
 * megapixel on this hardware. It cannot include the pointer and it reads the screen as it
 * is *now*, so freeze mode and `CAP-11` go through [`captureFrozenArea`] instead.
 *
 * This is `Shell.Screenshot.screenshot_area` made again from the calls it makes, because
 * it left the last column or row unpainted at a scale that is not a whole number
 * (`repaint.ts`, `docs/decisions.md` D116). The compositor gives the region's size and
 * scale, as it did, and the stage is repainted at that scale over the region grown by a
 * logical pixel, into a cleared texture. The PNG is cut from that at exactly the pixels
 * `screenshot_area` took, so it is the same size (D23) and in the same place (D115). An
 * outermost pixel the stage has nothing to paint is left clear by the repaint, and takes
 * the colour of the one inside it.
 */
export function startAreaCapture(rect: Rect, path: string): StartedCapture {
    const started = GLib.get_monotonic_time();
    const stage = global.stage;
    const [onScreen, width, height, scale] = stage.get_capture_final_size(mtkRect(rect));
    if (!onScreen)
        throw new Error(`${rect.width}x${rect.height} at ${rect.x},${rect.y} is on no monitor`);
    const plan = repaintPlan(rect, scale, { width, height });
    // `screenshotUIGroup` holds capture chrome -- the overlay, the countdown, a fly still in
    // flight from the capture before, the recording frame -- and `screenshot_area` left it
    // out of every capture (D22). A repaint of the stage paints it like everything else,
    // which D116 did not notice because the overlay hides itself first and the countdown
    // never showed. So it is transparent for the repaint and for nothing else: the repaint
    // is synchronous, and the next frame on screen is painted with it back (D130).
    const chrome = Main.layoutManager.screenshotUIGroup;
    const opacity = chrome.opacity;
    chrome.opacity = 0;
    let content: Clutter.Content;
    try {
        content = stage.paint_to_content(
            mtkRect(plan.paint),
            scale,
            null,
            Clutter.PaintFlag.NO_CURSORS | Clutter.PaintFlag.CLEAR,
        );
    } finally {
        chrome.opacity = opacity;
    }
    const painted = textureOf(content);
    if (painted === null) throw new Error('the repaint has no texture to composite from');

    const edged = withEdges(painted, plan.keep, 'clear');
    const paintMs = (GLib.get_monotonic_time() - started) / 1000;
    const compositor = Shell.Screenshot as unknown as PromisifiedCompositor;
    // Called, not awaited: the readback happens inside this call, and the encode after it
    // on the thread that `written` waits for.
    const encoding = compositeToFile(path, stream =>
        compositor.composite_to_stream(
            edged.texture,
            edged.x,
            edged.y,
            width,
            height,
            scale,
            null,
            0,
            0,
            1,
            stream,
        ),
    );
    const written = encoding.then(cost => {
        leftForCollector(
            bytesOf(painted) + (edged.texture === painted ? 0 : bytesOf(edged.texture)),
        );
        return { paintMs, ...cost };
    });
    return {
        pixels: pixelsOf(edged.texture, { x: edged.x, y: edged.y, width, height }),
        written,
    };
}

/** [`startAreaCapture`] for a caller with nothing to show before the file is written. */
export function captureAreaToFile(rect: Rect, path: string): Promise<CaptureCost> {
    return startAreaCapture(rect, path).written;
}

/** A logical rect as the compositor takes it. */
function mtkRect(rect: Rect): Mtk.Rectangle {
    return new Mtk.Rectangle({ x: rect.x, y: rect.y, width: rect.width, height: rect.height });
}

/**
 * How many bytes of texture the captures here have left for the garbage collector since it
 * last ran, and how many they may leave.
 *
 * `screenshot_area` freed its texture in C the moment it had read it. A repaint's texture,
 * and the one its edges are filled in, belong to JavaScript instead, and only a collection
 * frees them. The collector runs by how much JavaScript allocates, not by how much the GPU
 * holds, and a scrolling capture grabs a frame every third of a second: at 1.6 Mpx that
 * leaves 13 MB a frame, with nothing else in the shell bound to allocate enough meanwhile to
 * set the collector off. So past this much, a capture runs it: one pause every twenty or so
 * of those frames, instead of gigabytes held through a minute's capture (D116).
 */
const UNCOLLECTED_LIMIT = 256 * 1024 * 1024;
let uncollected = 0;

function leftForCollector(bytes: number): void {
    uncollected += bytes;
    if (uncollected < UNCOLLECTED_LIMIT) return;
    uncollected = 0;
    System.gc();
}

/** A texture's size in memory, at four bytes a pixel. */
function bytesOf(texture: Cogl.Texture): number {
    return texture.get_width() * texture.get_height() * 4;
}

/**
 * Where a capture's pixels are to be cut from: a texture of their own with their edges
 * filled, at 0,0, or the texture they were painted into, at the crop's origin.
 */
interface Edged {
    texture: Cogl.Texture;
    x: number;
    y: number;
}

/** Keeps what it is drawn over, and puts the source in only where that is clear. */
const UNDER = 'RGBA = ADD (SRC_COLOR * (1-DST_COLOR[A]), DST_COLOR)';
/** Puts the source in as it is. */
const REPLACE = 'RGBA = ADD (SRC_COLOR, 0)';

/**
 * `crop` of `texture`, with its outermost pixels filled from the ones inside them
 * (`repaint.ts`): the columns and rows on `sides`, or with `'clear'`, for a repaint
 * cleared first, each outermost pixel nothing painted.
 *
 * Nothing on a monitor paints a pixel clear, so a clear one is one the stage had nothing
 * for, whatever the reason; the frozen screen was not cleared, so it says which sides
 * instead. Drawn the way `grab_screenshot_content` copies the pointer's sprite, into an
 * offscreen texture the size of the capture, with texels picked rather than blended. A
 * failure there leaves the crop as it was painted.
 */
function withEdges(texture: Cogl.Texture, crop: PixelRect, sides: Sides | 'clear'): Edged {
    try {
        return { texture: filled(texture, crop, sides), x: 0, y: 0 };
    } catch (e) {
        error('could not fill the edges of a capture', e);
        return { texture, x: crop.x, y: crop.y };
    }
}

function filled(source: Cogl.Texture, crop: PixelRect, sides: Sides | 'clear'): Cogl.Texture {
    const context = source.get_context();
    const { width, height } = crop;
    const target = Cogl.Texture2D.new_with_size(context, width, height);
    const framebuffer = Cogl.Offscreen.new_with_texture(target);
    framebuffer.allocate();
    framebuffer.clear4f(Cogl.BufferBit.COLOR, 0, 0, 0, 0);

    const pipeline = (blend: string) => {
        const made = Cogl.Pipeline.new(context);
        made.set_layer_texture(0, source);
        made.set_layer_filters(0, Cogl.PipelineFilter.NEAREST, Cogl.PipelineFilter.NEAREST);
        made.set_blend(blend);
        return made;
    };
    // Both rects in the capture's pixels, `from` read out of the source at the crop.
    // The target's own coordinates run from -1 to 1, top to bottom as 1 to -1.
    const draw = (through: Cogl.Pipeline, to: PixelRect, from: PixelRect) =>
        framebuffer.draw_textured_rectangle(
            through,
            -1 + (2 * to.x) / width,
            1 - (2 * to.y) / height,
            -1 + (2 * (to.x + to.width)) / width,
            1 - (2 * (to.y + to.height)) / height,
            (crop.x + from.x) / source.get_width(),
            (crop.y + from.y) / source.get_height(),
            (crop.x + from.x + from.width) / source.get_width(),
            (crop.y + from.y + from.height) / source.get_height(),
        );
    draw(pipeline(REPLACE), { x: 0, y: 0, width, height }, { x: 0, y: 0, width, height });

    const fill = pipeline(sides === 'clear' ? UNDER : REPLACE);
    const want = sides === 'clear' ? { left: true, right: true, top: true, bottom: true } : sides;
    const right = width - 1;
    const bottom = height - 1;
    const column = (x: number, from: number) =>
        draw(fill, { x, y: 0, width: 1, height }, { x: from, y: 0, width: 1, height });
    const row = (y: number, from: number) =>
        draw(fill, { x: 0, y, width, height: 1 }, { x: 0, y: from, width, height: 1 });
    const corner = (x: number, y: number, fromX: number, fromY: number) =>
        draw(fill, { x, y, width: 1, height: 1 }, { x: fromX, y: fromY, width: 1, height: 1 });
    // Columns, then rows, then each corner from the pixel diagonally inside it: a corner
    // whose column and row both need filling has only that one painted beside it.
    if (width > 1) {
        if (want.left) column(0, 1);
        if (want.right) column(right, right - 1);
    }
    if (height > 1) {
        if (want.top) row(0, 1);
        if (want.bottom) row(bottom, bottom - 1);
    }
    if (width > 1 && height > 1) {
        if (want.left && want.top) corner(0, 0, 1, 1);
        if (want.right && want.top) corner(right, 0, right - 1, 1);
        if (want.left && want.bottom) corner(0, bottom, 1, bottom - 1);
        if (want.right && want.bottom) corner(right, bottom, right - 1, bottom - 1);
    }
    // Sent now, so that reading the texture back finds it drawn.
    framebuffer.flush();
    return target;
}

/**
 * `CAP-03`: the window that currently has focus, with its frame and shadow.
 *
 * `Shell.Screenshot.screenshot_window` captures the **focused** window rather than one
 * passed as an argument, which is why the window picker focuses its target before calling
 * this rather than handing over an actor. That also gives `spec/03`'s "occluded windows
 * are captured correctly" for free: Mutter renders the window's own contents, not what is
 * visible of it on screen.
 *
 * Returns the rect the PNG actually contains, **read back from the file**, because three
 * plausible sources for it all turned out to be wrong. For one window, measured:
 *
 * | source | said | PNG was |
 * |---|---|---|
 * | `get_frame_rect()` | 360x240 | 410x290 |
 * | `get_buffer_rect()` | 360x240 | 410x290 |
 * | `screenshot_window`'s own return | 360x240 | 410x290 |
 *
 * Mutter adds a 25 px shadow margin that none of them account for. `spec/10` §3.3
 * promises the twin's `rect` times `scale` is the PNG's size, and `ACT-07`'s `{w}`/`{h}`
 * tokens are computed from it, so any of those three would have produced a filename that
 * lied about the image. The header is 24 bytes and cannot disagree with the file.
 *
 * On GNOME 50 the buffer rect has that margin in it (D138). An X11 window with Mutter's
 * frame measured 420x197 framed, 470x247 buffered and 470x247 in the file, and a Wayland
 * client's image is its buffer, its own shadow included.
 *
 * `buffer` is the window's buffer rect, as the picker found it. The scale is read from the
 * file as well, among `scales`, those of the monitors under the window, largest first. An
 * image that is the buffer exactly is at the scale that makes it so (`bufferScale`), and
 * one with a margin round the frame is placed by that margin (`imageScale`, D138). The
 * monitor's scale is what the window's buffer is at only when its client draws at that
 * scale.
 *
 * `include_frame` stays on: it is what keeps a legacy X11 window's server-side title bar
 * in the shot. Whether OctoSnap should be *removing* the compositor's shadow so its own
 * `window-shadow` setting (`spec/08` §2) can add one back is an M6 question for the
 * background tool, not a correctness one now.
 */
export async function captureWindowToFile(
    path: string,
    buffer: Rect,
    scales: readonly number[],
    options: { includeFrame?: boolean; includeCursor?: boolean } = {},
): Promise<{ rect: Rect; scale: number }> {
    const shooter = new Shell.Screenshot() as PromisifiedShooter;
    let reported: Rect | null = null;
    await toFile(path, async stream => {
        const result = await shooter.screenshot_window(
            options.includeFrame ?? true,
            options.includeCursor ?? false,
            stream,
        );
        reported = rectFromResult(result);
    });
    const said = reported as Rect | null;
    const window: Rect = said ?? buffer;
    // The largest scale under the window, when the file says nothing better.
    const fallback = scales[0] ?? 1;

    const physical = pngSize(path);
    const at = (r: Rect) => `${r.width}x${r.height}+${r.x}+${r.y}`;
    info(
        `window capture: ${physical === null ? 'a file of no size' : `${physical.width}x${physical.height} px`}, ` +
        `Mutter's rect ${said === null ? 'none' : at(said)}, the buffer ${at(buffer)}, ` +
        `on scales ${scales.join(', ')}`,
    );
    if (physical === null) return { rect: window, scale: fallback };

    // The scale too comes from the file: the window's buffer is at whatever scale its
    // client drew it, which is not always its monitor's (D138). A Wayland client's image is
    // that buffer exactly, and then the twin is the buffer's rect, origin and all.
    const exact = bufferScale(buffer, physical, scales);
    if (exact !== null) return { rect: buffer, scale: exact };
    // Otherwise it has a margin round the frame Mutter reports, and the scale is the one at
    // which that margin is even (`imageScale`).
    const scale = imageScale(window, physical, scales) ?? fallback;

    // The header is in physical pixels; the twin's rect is logical (`spec/01` §1). At
    // scale 1 these are the same number, which is why the conversion has to be written
    // deliberately rather than discovered on a HiDPI machine.
    const logical = {
        width: Math.round(physical.width / scale),
        height: Math.round(physical.height / scale),
    };

    // The origin stays the window's, from the reported rect; only the extent comes from
    // the file. Mutter's margin is symmetric, so the true origin is half the excess out.
    return {
        rect: {
            x: window.x - Math.round((logical.width - window.width) / 2),
            y: window.y - Math.round((logical.height - window.height) / 2),
            width: logical.width,
            height: logical.height,
        },
        scale,
    };
}

/**
 * Reads a PNG's pixel dimensions from its IHDR chunk.
 *
 * 24 bytes, no decoder, no dependency: an 8-byte signature, an 8-byte chunk header, then
 * width and height as big-endian 32-bit integers. Cheap enough to do on the capture path.
 */
function pngSize(path: string): { width: number; height: number } | null {
    try {
        const stream = Gio.File.new_for_path(path).read(null);
        try {
            const bytes = stream.read_bytes(24, null).get_data();
            if (bytes === null || bytes.length < 24) return null;
            // Signature, then "IHDR" at offset 12.
            if (bytes[0] !== 0x89 || bytes[1] !== 0x50) return null;
            const be32 = (at: number) =>
                (bytes[at]! << 24) | (bytes[at + 1]! << 16) | (bytes[at + 2]! << 8) | bytes[at + 3]!;
            const width = be32(16);
            const height = be32(20);
            if (width <= 0 || height <= 0) return null;
            return { width, height };
        } finally {
            stream.close(null);
        }
    } catch (e) {
        error(`could not read the size of ${path}`, e);
        return null;
    }
}

/**
 * Reads the `Mtk.Rectangle` out of a promisified screenshot call.
 *
 * `Gio._promisify` resolves these to a **one-element array** holding the rectangle: the
 * `gboolean` in `screenshot_window_finish`'s signature is a `throws` success flag, so GJS
 * turns it into a rejection and leaves only the out-parameter. An earlier version read
 * index 1 and got `undefined` on every call.
 *
 * The last element rather than the first, so a build that does surface the boolean still
 * works. And note that `JSON.stringify` on an `Mtk.Rectangle` prints `{}`: its fields are
 * getters on a boxed struct, not own properties, which is what made the first diagnostic
 * of this useless.
 */
function rectFromResult(
    result: Mtk.Rectangle | [Mtk.Rectangle] | [boolean, Mtk.Rectangle],
): Rect | null {
    const rectangle = Array.isArray(result) ? result[result.length - 1] : result;
    if (typeof rectangle !== 'object' || rectangle === null) return null;
    const { x, y, width, height } = rectangle as Mtk.Rectangle;
    if (typeof width !== 'number' || width <= 0 || height <= 0) return null;
    return { x, y, width, height };
}

/**
 * A frozen copy of the whole stage, plus the pointer as it was at that instant.
 *
 * This is the primitive behind three features at once, which is why it is worth its own
 * type: `CAP-09` shows the content behind the overlay so nothing moves while the user
 * selects, `CAP-11` composites the cursor texture into the capture, and `CAP-10`'s
 * magnifier samples the same content instead of reading the screen per frame.
 *
 * `scale` is the content's own scale, and `content` is in **physical** pixels while every
 * rect this module takes is logical (`spec/01` §1). [`captureFrozenArea`] is the only
 * place that conversion happens.
 */
export interface FrozenScreen {
    content: Clutter.Content;
    scale: number;
    cursorContent: Clutter.Content | null;
    cursorPoint: Graphene.Point | null;
    cursorScale: number;
    /** The stage at each other scale its monitors have, when asked for (D138). */
    layers: FrozenLayer[];
}

/**
 * The stage painted again at a monitor scale the frozen screen is not at, over the
 * monitors at that scale.
 *
 * `screenshot_stage_to_content` paints the whole stage at the largest scale of its
 * monitors, which is what GNOME's own screenshot UI shows and crops from. On monitors at
 * different scales that is the wrong scale for every monitor but the finest. The matrix
 * found a 480x360 rect on a 100 % monitor, beside a 200 % one, coming out of the frozen
 * screen as 960x720, windows painted at 1x drawn at 2x, where the live path gave 480x360,
 * and the twin said 1x (D138). So a snapshot that captures are to be cut from also holds
 * one of these for each other scale, and a rect on those monitors is cut from the layer at
 * its own scale, exactly as [`startAreaCapture`] cuts one from its repaint.
 */
export interface FrozenLayer {
    scale: number;
    /** The logical region painted: those monitors' bounds, a logical pixel larger all round. */
    rect: Rect;
    texture: Cogl.Texture;
}

/**
 * A frozen copy of the stage. With `layers`, the other scales too: for a snapshot a capture
 * will be cut from, which on a single scale costs nothing.
 *
 * The layers are painted in the same turn of the main loop as the frozen screen is asked
 * for, not after it arrives, so the two are at most a frame apart, and on a still screen
 * the same.
 */
export async function freezeScreen(options: { layers?: boolean } = {}): Promise<FrozenScreen> {
    const shooter = new Shell.Screenshot() as PromisifiedShooter;
    const grabbed = shooter.screenshot_stage_to_content();
    const layers = options.layers === true ? paintLayers() : [];
    const [content, scale, cursorContent, cursorPoint, cursorScale] = await grabbed;
    return { content, scale, cursorContent, cursorPoint, cursorScale, layers };
}

/** [`FrozenLayer`]s for every monitor scale but the largest, which the frozen screen is at. */
function paintLayers(): FrozenLayer[] {
    const monitors = allMonitors();
    const finest = Math.max(...monitors.map(m => m.scale));
    const scales = [...new Set(monitors.map(m => m.scale))].filter(s => s < finest);
    if (scales.length === 0) return [];

    const stage = global.stage;
    // Capture chrome is left out, as `startAreaCapture` leaves it out: a fly still in the
    // air belongs in no capture.
    const chrome = Main.layoutManager.screenshotUIGroup;
    const opacity = chrome.opacity;
    chrome.opacity = 0;
    const layers: FrozenLayer[] = [];
    try {
        for (const scale of scales) {
            const bounds = boundsOf(monitors.filter(m => m.scale === scale).map(m => m.rect));
            const rect = {
                x: bounds.x - 1,
                y: bounds.y - 1,
                width: bounds.width + 2,
                height: bounds.height + 2,
            };
            const content = stage.paint_to_content(
                mtkRect(rect),
                scale,
                null,
                Clutter.PaintFlag.NO_CURSORS | Clutter.PaintFlag.CLEAR,
            );
            const texture = textureOf(content);
            if (texture === null) throw new Error(`the stage at scale ${scale} has no texture`);
            leftForCollector(bytesOf(texture));
            layers.push({ scale, rect, texture });
        }
    } finally {
        chrome.opacity = opacity;
    }
    return layers;
}

/** The smallest rect holding all of `rects`. */
function boundsOf(rects: Rect[]): Rect {
    const left = Math.min(...rects.map(r => r.x));
    const top = Math.min(...rects.map(r => r.y));
    const right = Math.max(...rects.map(r => r.x + r.width));
    const bottom = Math.max(...rects.map(r => r.y + r.height));
    return { x: left, y: top, width: right - left, height: bottom - top };
}

/** Whether `inner` lies wholly inside `outer`. */
function inside(inner: Rect, outer: Rect): boolean {
    return inner.x >= outer.x && inner.y >= outer.y &&
        inner.x + inner.width <= outer.x + outer.width &&
        inner.y + inner.height <= outer.y + outer.height;
}

/**
 * Writes a logical rect out of a [`FrozenScreen`], optionally with the pointer in it.
 *
 * **`composite_to_stream` takes its rectangle in texture pixels, not logical ones**, and
 * that is the whole reason this function exists rather than the call being inlined.
 * `screenshot_area` takes a *logical* rect and produces physical pixels, so the two
 * capture paths disagree about coordinates and the `scale` argument here only tells the
 * compositor where the cursor goes. Handing this call a logical rect produces a capture
 * at 1/scale of the size the user selected -- and it is **invisible at scale 1.0**, which
 * is what the development machine happens to be set to today.
 *
 * Measured, not reasoned about: at scale 2, a logical 200x150 selection came out of
 * `screenshot_area` as 400x300 and out of this call as 200x150. `spec/01` §1 exists
 * because of exactly this, and `docs/decisions.md` D14 records it.
 *
 * The rounding matches `Rect::to_physical` in `crates/core/src/geometry.rs`, so the two
 * halves agree on the size the PNG will be -- and matches Mutter, so the two *capture
 * paths* agree with each other. `docs/decisions.md` D23.
 *
 * `rectScale` is the rect's own, its monitor's. A rect on a monitor at a smaller scale than
 * the frozen screen's is cut from that scale's layer (D138), and without one it is refused
 * rather than written at a scale its twin would not say.
 */
export function startFrozenArea(
    frozen: FrozenScreen,
    rect: Rect,
    rectScale: number,
    path: string,
    includeCursor: boolean,
): StartedCapture {
    if (Math.abs(rectScale - frozen.scale) > 1e-6) {
        const layer = frozen.layers.find(
            l => Math.abs(l.scale - rectScale) < 1e-6 && inside(rect, l.rect),
        );
        if (layer === undefined) {
            throw new Error(
                `the frozen screen is at scale ${frozen.scale}, and has nothing at ${rectScale}`,
            );
        }
        return startLayerArea(frozen, layer, rect, path, includeCursor);
    }
    const started = GLib.get_monotonic_time();
    const texture = textureOf(frozen.content);
    if (texture === null)
        throw new Error('the frozen screen has no texture to composite from');

    const cursorTexture = includeCursor ? textureOf(frozen.cursorContent) : null;
    if (includeCursor) {
        // Logged because `CAP-11` cannot be judged from the output alone: a capture with
        // no visible pointer might mean the pointer was outside the rect, hidden, or
        // scaled to nothing, and those need different fixes. A nested `--devkit` shell
        // has no real cursor sprite, so this is the line that tells a real session apart
        // from a test one.
        info(
            `cursor requested: texture=${cursorTexture === null ? 'none' : 'present'} ` +
            `at ${frozen.cursorPoint?.x ?? '?'},${frozen.cursorPoint?.y ?? '?'} ` +
            `scale=${frozen.cursorScale}`,
        );
    }
    const compositor = Shell.Screenshot as unknown as PromisifiedCompositor;

    const scale = frozen.scale;
    const physical = {
        // **Floor the origin, round the extent.** Not a compromise between two opinions:
        // it is what `screenshot_area` was measured to do, and the point of this rect is
        // to be the one Mutter would have used for the same selection, so that the same
        // drag gives the same PNG whether or not freeze was on.
        //
        // The mixture is easy to get half right, and both halves were, at different
        // times. Ceiling the extent made the twin claim a size the file did not have. Then
        // rounding the origin too made the two paths differ by exactly one pixel in x at
        // 1.6666666269302368 -- found by aligning the two PNGs and discovering that
        // shifting one by (+1, 0) made them identical, difference 0. 37 x 1.6666666269 is
        // 61.67: floored 61, rounded 62, and Mutter takes 61.
        //
        // And both in single precision, as Mutter multiplies (D115): 576 x 5/3 is
        // 959.99998 as a double, so a double floors it to 959, where Mutter starts the
        // live capture at 960. `docs/decisions.md` D23 and D116.
        x: origin(rect.x, scale),
        y: origin(rect.y, scale),
        width: Math.round(Math.fround(rect.width * scale)),
        height: Math.round(Math.fround(rect.height * scale)),
    };

    // The frozen screen was painted whole and not cleared, so a pixel it had nothing to
    // paint holds whatever the GPU's memory did. A capture can reach one on any side,
    // past a monitor with nothing beyond it or past the stage, and takes the colour of the
    // one inside it instead, as the live path's does (D116).
    const sides = pastPainted(rect, scale, stageSize(), allMonitors().map(m => m.rect));
    const edged = sides.left || sides.right || sides.top || sides.bottom
        ? withEdges(texture, physical, sides)
        : { texture, x: physical.x, y: physical.y };
    const paintMs = (GLib.get_monotonic_time() - started) / 1000;

    const encoding = compositeToFile(path, stream =>
        compositor.composite_to_stream(
            edged.texture,
            edged.x,
            edged.y,
            physical.width,
            physical.height,
            scale,
            cursorTexture,
            // The pointer goes where it did in the frozen screen, wherever the crop is cut.
            cursorAt(frozen.cursorPoint?.x, scale, 0) - physical.x + edged.x,
            cursorAt(frozen.cursorPoint?.y, scale, 0) - physical.y + edged.y,
            frozen.cursorScale,
            stream,
        ),
    );
    const written = encoding.then(cost => {
        if (edged.texture !== texture) leftForCollector(bytesOf(edged.texture));
        return { paintMs, ...cost };
    });
    return {
        pixels: pixelsOf(edged.texture, {
            x: edged.x,
            y: edged.y,
            width: physical.width,
            height: physical.height,
        }),
        written,
    };
}

/**
 * [`startFrozenArea`] for a rect on monitors at a scale of their own: cut from that scale's
 * [`FrozenLayer`] at the pixels `startAreaCapture` keeps from its repaint (`repaintPlan`),
 * the layer being a repaint of the same kind over a larger region. Its edges are filled the
 * same way, since it was cleared first, and the pointer goes where the frozen screen had it.
 */
function startLayerArea(
    frozen: FrozenScreen,
    layer: FrozenLayer,
    rect: Rect,
    path: string,
    includeCursor: boolean,
): StartedCapture {
    const started = GLib.get_monotonic_time();
    const { scale } = layer;
    const keep: PixelRect = {
        x: origin(rect.x, scale) - origin(layer.rect.x, scale),
        y: origin(rect.y, scale) - origin(layer.rect.y, scale),
        width: Math.round(Math.fround(rect.width * scale)),
        height: Math.round(Math.fround(rect.height * scale)),
    };
    const cursorTexture = includeCursor ? textureOf(frozen.cursorContent) : null;
    const edged = withEdges(layer.texture, keep, 'clear');
    const paintMs = (GLib.get_monotonic_time() - started) / 1000;
    const compositor = Shell.Screenshot as unknown as PromisifiedCompositor;
    const encoding = compositeToFile(path, stream =>
        compositor.composite_to_stream(
            edged.texture,
            edged.x,
            edged.y,
            keep.width,
            keep.height,
            scale,
            cursorTexture,
            cursorAt(frozen.cursorPoint?.x, scale, layer.rect.x) - keep.x + edged.x,
            cursorAt(frozen.cursorPoint?.y, scale, layer.rect.y) - keep.y + edged.y,
            frozen.cursorScale,
            stream,
        ),
    );
    const written = encoding.then(cost => {
        if (edged.texture !== layer.texture) leftForCollector(bytesOf(edged.texture));
        return { paintMs, ...cost };
    });
    return {
        pixels: pixelsOf(edged.texture, {
            x: edged.x,
            y: edged.y,
            width: keep.width,
            height: keep.height,
        }),
        written,
    };
}

/**
 * Where the frozen pointer is in a texture painted at `scale`, whose first pixel is the
 * logical coordinate `from`.
 *
 * `composite_to_stream` takes the pointer in the source texture's pixels, and the frozen
 * screen's `cursorPoint` is in the stage's logical ones: GNOME's screenshot UI sets its
 * pointer actor at the point, then hands `composite_to_stream` `this._cursor.x *
 * this._scale`. This passed the logical point through as a pixel, so at 200 % the pointer
 * was drawn as if at half its distance from the stage's corner: in the wrong place, or
 * outside the crop altogether. At scale 1 the two are the same number, and in the nested
 * shell the pointer's sprite leaves no pixel in a capture, so no harness could see it
 * (D138).
 */
function cursorAt(at: number | undefined, scale: number, from: number): number {
    return Math.round(Math.fround((at ?? 0) * scale)) - origin(from, scale);
}

/**
 * The texture behind a `Clutter.Content`.
 *
 * `screenshot_stage_to_content` returns a `ClutterTextureContent`, whose `get_texture` is
 * not on the `Clutter.Content` interface, so this is a narrowing rather than a cast for
 * its own sake.
 */
function textureOf(content: Clutter.Content | null): Cogl.Texture | null {
    if (content === null) return null;
    const withTexture = content as unknown as { get_texture?: () => Cogl.Texture | null };
    return withTexture.get_texture?.() ?? null;
}

export interface MonitorInfo {
    index: number;
    rect: Rect;
    scale: number;
}

/**
 * The monitor the pointer is on -- where a capture defaults to (`spec/01` §2 row 35,
 * `CAP-18`).
 *
 * Derived from `global.get_pointer()` rather than `global.display.get_current_monitor()`,
 * which `spec/01` §2 row 35 suggests. Measured on a two-monitor nested rig:
 * `get_current_monitor()` returned **the same monitor for every pointer position**,
 * including positions squarely inside the other display, so `CAP-04` fullscreen captured
 * the wrong screen. Whatever it tracks -- the focused window, or a cached value -- it is
 * not the cursor, and `CAP-18` asks for the cursor.
 *
 * `global.get_pointer()` is the cursor's stage position, which is exactly the question.
 * `get_current_monitor()` remains the fallback for the case the pointer is somehow on no
 * monitor at all.
 */
export function currentMonitor(): MonitorInfo {
    const monitors = allMonitors();
    const [px, py] = global.get_pointer();
    const found = monitors.find(
        m =>
            px >= m.rect.x &&
            px < m.rect.x + m.rect.width &&
            py >= m.rect.y &&
            py < m.rect.y + m.rect.height,
    );
    if (found !== undefined) return found;

    const index = global.display.get_current_monitor();
    return monitors[index] ?? monitors[0]!;
}

/**
 * Every monitor, in logical stage coordinates (`spec/01` §1).
 *
 * Note what is deliberately *not* alongside this: a lookup by connector name. The
 * extension cannot see connector names at all (`docs/spikes/15-monitor-metadata.md`), so
 * `BeginCapture`'s `display` option is resolved to a rect by the **app**, which can ask
 * `org.gnome.Mutter.DisplayConfig`, before the request ever arrives here.
 */
export function allMonitors(): MonitorInfo[] {
    return Main.layoutManager.monitors.map((monitor, index) => ({
        index,
        rect: { x: monitor.x, y: monitor.y, width: monitor.width, height: monitor.height },
        scale: global.display.get_monitor_scale(index),
    }));
}

/**
 * The stage's logical size: the box around every monitor, which is what the compositor
 * repaints a capture from (`outline.ts`).
 */
export function stageSize(): Size {
    return { width: global.stage.width, height: global.stage.height };
}

/**
 * Where each of the stage's views starts, and how fast it refreshes, for the recording
 * row's "Screen rate · 30 fps" (`recordChoices.ts`' `refreshAt`, D117).
 *
 * D101 read the rate in the app, from `DisplayConfig`, and said the extension could not see
 * it. It can: Mutter paints each monitor through a `Clutter.StageView` made from the
 * monitor's current mode, and the view keeps that mode's refresh rate, the same
 * single-precision number `GetCurrentState` sends the app. The stage lists the views it
 * was last laid out on, which are the monitors as they are unless they changed in the
 * last frame. A shell that cannot answer costs the number, not the overlay: the row then
 * says what the app says for a rate it does not know.
 */
export function viewRates(): ViewRate[] {
    try {
        return global.stage.peek_stage_views().map(view => {
            const layout = view.layout;
            return { x: layout.x, y: layout.y, hz: view.get_refresh_rate() };
        });
    } catch (e) {
        error('could not read the refresh rates of the screens', e);
        return [];
    }
}

/**
 * A rect of the given logical size, centred on a monitor and clamped inside it.
 *
 * Used by the M0 acceptance path and by nothing on the real capture routes any more.
 */
export function centredRect(monitor: MonitorInfo, width: number, height: number): Rect {
    const w = Math.min(width, monitor.rect.width);
    const h = Math.min(height, monitor.rect.height);
    return {
        x: monitor.rect.x + Math.trunc((monitor.rect.width - w) / 2),
        y: monitor.rect.y + Math.trunc((monitor.rect.height - h) / 2),
        width: w,
        height: h,
    };
}
