// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * What the All-In-One toolbar offers for a GIF, and the words it uses (D101).
 *
 * Pure data and pure functions, with no GJS in them, so the choices and their labels can
 * be tested the way `slot.ts` is. The app owns the *meaning* of every value here: an
 * override travels as a key in the `record` action's dictionary and the app clamps and
 * applies it; a key that is not sent leaves the user's setting alone (`spec/10` §3.1's
 * rule for `BeginCapture`, kept for `record`).
 *
 * One of those meanings is worked out here as well, so that it can be shown: the rate "the
 * screen's rate" becomes. [`recordedFps`] is the app's `GifSettings::fps_for`
 * (`crates/media/src/gif.rs`) step for step, so the row can say "Screen rate · 30 fps"
 * before the app is asked, and say the number the app will record at (D117).
 */

/** The four things a GIF recording can be asked to do differently this once. */
export interface GifOverrides {
    /** Frames a second; `0` is [`SCREEN_RATE`], the monitor's own. */
    fps?: number;
    /** The widest the GIF may be, in pixels; `0` is the selection's own width. */
    maxWidth?: number;
    /** gifski's 1–100. */
    quality?: number;
    /** Whether the compositor draws the pointer into the stream. */
    cursor?: boolean;
}

/**
 * The most a GIF can play at. A frame's delay is stored in hundredths of a second, and
 * browsers treat a delay of one hundredth as a tenth, so gifski writes nothing shorter
 * than two -- which is fifty frames a second. Asking for sixty does not drop frames; it
 * stretches time, and a ten-second recording plays for twelve. The list therefore stops
 * at fifty, and "the screen's rate" is brought under it -- divided, not capped, so a
 * sixty-hertz screen records at thirty (D111, [`recordedFps`]).
 */
export const GIF_FPS_MAX = 50;

/**
 * The app's GIF defaults from its copy in `app-gif-defaults` (D125), already unpacked:
 * each of the four values that is there, of its type and in the app schema's range.
 * Anything else is left out, so the row falls back to what it would have shown without
 * the copy -- a value this checks is one the app's own schema would have accepted.
 */
export function gifDefaultsFromCopy(copy: Record<string, unknown>): GifOverrides {
    const out: GifOverrides = {};
    const int = (value: unknown, min: number, max: number): number | undefined =>
        typeof value === 'number' && Number.isInteger(value) && value >= min && value <= max
            ? value
            : undefined;
    const fps = int(copy['fps'], 0, GIF_FPS_MAX);
    if (fps !== undefined) out.fps = fps;
    const maxWidth = int(copy['max-width'], 0, 7680);
    if (maxWidth !== undefined) out.maxWidth = maxWidth;
    const quality = int(copy['quality'], 1, 100);
    if (quality !== undefined) out.quality = quality;
    if (typeof copy['cursor'] === 'boolean') out.cursor = copy['cursor'];
    return out;
}

/** `fps` meaning "whatever the recorded monitor runs at", resolved by the app. */
export const SCREEN_RATE = 0;

/**
 * The screen's rate on a monitor whose refresh is not known: the app's
 * `SCREEN_RATE_FALLBACK`, thirty, which is what a sixty-hertz screen gives.
 */
export const SCREEN_RATE_FALLBACK = 30;

/** `spec/08` §5's rates, with sixty replaced by the format's ceiling, then the screen's. */
export const GIF_FPS_CHOICES: readonly number[] = [10, 15, 24, 30, GIF_FPS_MAX, SCREEN_RATE];

/** Zero first because it is the default in `spec/08` §5's sense of "no cap". */
export const GIF_WIDTH_CHOICES: readonly number[] = [0, 640, 800, 1024, 1280, 1920];

/** gifski's useful range is 50–100 (its own documentation says so); 80 is the default. */
export const GIF_QUALITY_CHOICES: readonly number[] = [50, 65, 80, 90, 100];

/**
 * The rate a GIF is recorded at for a setting of `fps`, on a monitor refreshing at `hz`
 * hertz (`null` when that is not known).
 *
 * **The app's `GifSettings::fps_for`, step for step** (`crates/media/src/gif.rs`). A set
 * rate is recorded as set, never above [`GIF_FPS_MAX`]. [`SCREEN_RATE`] is the refresh
 * divided by the fewest whole times that bring it to the ceiling or under, rounded to the
 * frame (D111): 30 at 60 Hz and at 59.94, 38 at 75, 40 at 120, 48 at 144, 41 at 165. A
 * refresh that is not known, or not a rate, is [`SCREEN_RATE_FALLBACK`]. The Rust works in
 * doubles as well, and where `f64::round` takes a half away from zero `Math.round` takes it
 * up, which is the same thing for the positive quotient rounded here. `gif.rs` pins the
 * rates `recordChoices.test.ts` pins, with the same answers: a change to either side
 * fails its own test, and the test names the other side (D117).
 */
export function recordedFps(fps: number, hz: number | null): number {
    if (fps !== SCREEN_RATE) return Math.min(fps, GIF_FPS_MAX);
    if (hz === null || !Number.isFinite(hz) || hz < 1) return SCREEN_RATE_FALLBACK;
    const every = Math.max(Math.ceil(hz / GIF_FPS_MAX), 1);
    return Math.min(Math.max(Math.round(hz / every), 1), GIF_FPS_MAX);
}

/**
 * A frame rate as the row says it, on a monitor at `hz` ([`recordedFps`]).
 *
 * The screen's rate carries its number. "Screen rate" alone read as sixty on a sixty-hertz
 * screen, and the GIF editor then said thirty, which is right (D111, D117).
 */
export function fpsLabel(fps: number, hz: number | null): string {
    const rate = recordedFps(fps, hz);
    return fps === SCREEN_RATE ? `Screen rate · ${rate} fps` : `${rate} fps`;
}

/**
 * The frame-rate list, in [`GIF_FPS_CHOICES`]' order, for a monitor at `hz`.
 *
 * A function of its own because `GIF_FPS_CHOICES.map(fpsLabel)` type-checks and is wrong:
 * `map` passes each choice's index as the second argument, which is then taken for hertz.
 */
export function fpsChoiceLabels(hz: number | null): string[] {
    return GIF_FPS_CHOICES.map(fps => fpsLabel(fps, hz));
}

/** A stage view as [`refreshAt`] needs it: where it starts, and its refresh in hertz. */
export interface ViewRate {
    x: number;
    y: number;
    hz: number;
}

/**
 * The refresh rate of the monitor whose logical origin is `(x, y)`, or `null` when no
 * stage view starts there.
 *
 * Mutter paints each monitor through a view made from the monitor's current mode, and the
 * view keeps that mode's refresh rate: the number the app reads from `DisplayConfig` and
 * records with (`capture.ts`'s `viewRates`, D117). Found by the origin, the key the app
 * matches `DisplayConfig`'s monitors by (`display_config::refresh_at`), since two monitors
 * can share a size and a scale but never an origin. A monitor driven as tiles has a view
 * per tile, and the one found is the tile at the monitor's origin.
 */
export function refreshAt(views: readonly ViewRate[], x: number, y: number): number | null {
    return views.find(view => view.x === x && view.y === y)?.hz ?? null;
}

export function widthLabel(width: number): string {
    return width === 0 ? 'Full size' : `${width} px`;
}

export function qualityLabel(quality: number): string {
    return `Quality ${quality}`;
}

/** The index of `value` in `choices`, or `-1` when a typed setting is not one on offer. */
export function choiceIndex(choices: readonly number[], value: number): number {
    return choices.indexOf(value);
}
