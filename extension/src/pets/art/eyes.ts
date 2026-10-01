// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Bead eyes, shared by the pets that have them: the penguin, the mushroom and the potato
 * wear the `mid` size, the octopus the `big` one. The frog draws its own, on top of its
 * eye bumps (`frog.ts`).
 *
 * `e` is the dark of the eye and `w` its catchlight.
 */

import type { Mood } from './pose.js';
import { type Painter, type StampKey, mirrorRows } from './painter.js';

export type EyeStyle = 'mid' | 'big';

interface EyeShape {
    rows: string[];
    /** Rows down from the eye line. */
    dy: number;
    /** Mirror the rows for the right eye (a `>` becomes a `<`). */
    mirror?: boolean;
    /** Three pixels wide on a two-pixel eye: centred by moving out a pixel. */
    wide?: boolean;
}

function shape(style: EyeStyle, mood: Mood): EyeShape {
    if (style === 'big') {
        switch (mood) {
            case 'closed':
            case 'sleep':
                return { rows: ['...', 'e.e', '.e.'], dy: 1 };
            case 'blink':
                return { rows: ['...', '...', 'eee'], dy: 0 };
            case 'half':
                return { rows: ['eee', 'eee', 'eew'], dy: 1 };
            case 'happy':
                return { rows: ['.e.', 'e.e'], dy: 1 };
            case 'squeeze':
                return { rows: ['e..', '.e.', 'e..'], dy: 0, mirror: true };
            case 'wide':
                return { rows: ['wwe', 'wee', 'eee', 'eee'], dy: -1 };
            case 'dizzy':
                return { rows: ['e.e', '.e.', 'e.e'], dy: 0 };
            default:
                return { rows: ['wwe', 'wee', 'eee', 'eew'], dy: 0 };
        }
    }
    switch (mood) {
        case 'closed':
        case 'sleep':
            return { rows: ['...', 'e.e', '.e.'], dy: 0, wide: true };
        case 'blink':
            return { rows: ['..', '..', 'ee'], dy: 0 };
        case 'half':
            return { rows: ['ee', 'ee'], dy: 1 };
        case 'happy':
            return { rows: ['.e.', 'e.e'], dy: 0, wide: true };
        case 'squeeze':
            return { rows: ['e..', '.e.', 'e..'], dy: 0, mirror: true, wide: true };
        case 'wide':
            return { rows: ['wee', 'eee', 'eee'], dy: 0, wide: true };
        case 'dizzy':
            return { rows: ['e.e', '.e.', 'e.e'], dy: 0, wide: true };
        default:
            return { rows: ['we', 'ee', 'ee'], dy: 0 };
    }
}

/**
 * Both eyes: the left one's left edge at `lx`, the right one's right edge at `rx`, their
 * tops on row `y`. `look` moves the pupils a pixel to either side.
 */
export function drawEyes(
    P: Painter,
    lx: number,
    rx: number,
    y: number,
    style: EyeStyle,
    mood: Mood,
    look = 0,
): void {
    const spec = shape(style, mood);
    const key: StampKey = { e: ['eye', 'b'], w: ['eyeHi', 'b'] };
    const dx = Math.max(-1, Math.min(1, Math.round(look)));
    const width = spec.rows[0]?.length ?? 0;
    const shift = spec.wide === true && style !== 'big' ? 1 : 0;
    const right = spec.mirror === true ? mirrorRows(spec.rows) : spec.rows;
    P.stamp(lx + dx - shift, y + spec.dy, spec.rows, key);
    P.stamp(rx - width + 1 + dx + shift, y + spec.dy, right, key);
}
