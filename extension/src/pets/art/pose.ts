// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * A pose: everything that decides which pixels a pet paints in one frame (`spec/14` §2).
 *
 * Every field is optional and every pet reads the ones it has a use for, so the same pose
 * can be asked of any pet: `{ mood: 'happy', squash: 0.8 }` is a happy crouch for all five.
 * The specials are the few fields only one pet reads (`capLift` is the mushroom's cap,
 * `throat` the frog's), and a pet that has no use for one ignores it.
 */

/**
 * How a pet is seen (`spec/14` §4): from the side, standing on the floor or on a pin, or
 * from above, on the desk -- the screen read as a table it sits on.
 */
export type View = 'side' | 'top';

/** Which way a pet seen from above faces: `down` is towards the person at the screen. */
export type Face = 'down' | 'up' | 'left' | 'right';

/** What the eyes (and for some pets the mouth) do. */
export type Mood =
    | 'open'
    | 'blink'
    | 'half'
    | 'closed'
    | 'sleep'
    | 'happy'
    | 'squeeze'
    | 'wide'
    | 'dizzy';

export interface Pose {
    /**
     * -1 stretched tall to 1 squashed flat. Painted in steps of 0.1 (`quantizePose`), so a
     * squash that eases through a landing reuses a few frames rather than painting new ones.
     */
    squash?: number;
    /** Where the pupils point: -1 left, 0 ahead, 1 right. */
    look?: number;
    /** -1 up, 0 ahead, 1 down. */
    lookY?: number;
    mood?: Mood;
    /** The walk cycle: 0 both feet down, 1 left foot up, 3 right foot up. */
    step?: number;
    /** The penguin's flippers and the mushroom's arms. */
    arm?: 'down' | 'up' | 'flap' | 'wave' | 'wave2';
    /** The octopus's two outer arms. */
    arms?: 'down' | 'up';
    /** The octopus's tentacle wiggle, 0 to 3. */
    phase?: number;
    /** The frog's mouth. */
    mouth?: 'smile' | 'open' | 'o';
    /** The frog's closed mouth: `w` happy, `u` content, `blep` a tongue tip out. */
    mouthStyle?: 'w' | 'u' | 'blep';
    /** The frog's croak, 0 to 1. */
    throat?: number;
    /** The mushroom's cap, raised in art pixels to tip it. */
    capLift?: number;
    /** The mushroom's blue glow. */
    glow?: boolean;
    /** The potato's sprout: 0 none, 1 small, 2 grown. */
    sprout?: number;
    /** The potato's pat of butter. */
    butter?: boolean;
    /** The octopus's colour, one of `OCTOPUS_HUES`; absent is its own purple. */
    hue?: string;
    /** Quarter turns clockwise, for a pet that rolls. */
    rot?: number;
    /** Seen from above rather than from the side. The engine sets it, never a trick. */
    view?: View;
    /**
     * Which way it faces, seen from above. A trick that goes somewhere says so (`travel`),
     * and the engine keeps the last way said until the next trick, which starts facing the
     * screen; seen from the side, the engine leaves it out.
     */
    face?: Face;
}

/** The order `poseKey` writes fields in, so two equal poses always have one key. */
const FIELDS: readonly (keyof Pose)[] = [
    'squash',
    'look',
    'lookY',
    'mood',
    'step',
    'arm',
    'arms',
    'phase',
    'mouth',
    'mouthStyle',
    'throat',
    'capLift',
    'glow',
    'sprout',
    'butter',
    'hue',
    'rot',
    'view',
    'face',
];

/** What each field is when a pose leaves it out; a field at its default is left out of keys. */
const DEFAULTS: Required<Pose> = {
    squash: 0,
    look: 0,
    lookY: 0,
    mood: 'open',
    step: 0,
    arm: 'down',
    arms: 'down',
    phase: 0,
    mouth: 'smile',
    mouthStyle: 'u',
    throat: 0,
    capLift: 0,
    glow: false,
    sprout: 1,
    butter: false,
    hue: '',
    rot: 0,
    view: 'side',
    face: 'down',
};

function clamp(value: number, low: number, high: number): number {
    return Math.max(low, Math.min(high, value));
}

/**
 * The pose as it will be painted: numbers clamped and rounded to the steps the art has.
 * Two poses that paint the same pixels come out equal, which is what makes the frame cache
 * work -- a squash easing from 0.83 to 0.79 is one frame, not two.
 */
export function quantizePose(pose: Pose): Pose {
    const out: Pose = { ...pose };
    if (out.squash !== undefined) out.squash = Math.round(clamp(out.squash, -1, 1) * 10) / 10;
    if (out.look !== undefined) out.look = Math.round(clamp(out.look, -1, 1));
    if (out.lookY !== undefined) out.lookY = Math.round(clamp(out.lookY, -1, 1));
    if (out.phase !== undefined) out.phase = ((Math.round(out.phase) % 4) + 4) % 4;
    if (out.throat !== undefined) out.throat = Math.round(clamp(out.throat, 0, 1) * 4) / 4;
    if (out.capLift !== undefined) out.capLift = Math.round(clamp(out.capLift, 0, 4));
    if (out.sprout !== undefined) out.sprout = Math.round(clamp(out.sprout, 0, 2));
    if (out.rot !== undefined) out.rot = ((Math.round(out.rot) % 4) + 4) % 4;
    if (out.step !== undefined) out.step = out.step === 1 || out.step === 3 ? out.step : 0;
    return out;
}

/** A stable name for a quantized pose, for caching the frame it paints. */
export function poseKey(pose: Pose): string {
    const q = quantizePose(pose);
    const parts: string[] = [];
    for (const field of FIELDS) {
        const value = q[field];
        if (value === undefined || value === DEFAULTS[field]) continue;
        // `-0` and `0` are one squash.
        const text = typeof value === 'number' ? String(value === 0 ? 0 : value) : String(value);
        parts.push(`${field}=${text}`);
    }
    return parts.join('|');
}
