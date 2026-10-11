// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * A pet's preview, as Settings and the welcome window play it (`spec/14` §2): breaths,
 * blinks, a jump and its own tricks, played through the shell's own `Runner` with a fixed
 * seed and no room to move, so a preview is what the pet does on the desktop.
 *
 * The five's are written into `data/pets/` by `sheets.test.ts`. A pack's is made here, in
 * the shell, when the app asks for the packs (`GetPetPacks`, `spec/14` §14). No `gi://`
 * imports.
 */

import { ACTS, type ActContext } from './acts.js';
import type { PetSpec } from './art/index.js';
import type { Grid } from './art/painter.js';
import { type Pose, poseKey, quantizePose } from './art/pose.js';
import { BLINK_MS, BREATH_MS } from './brain.js';
import { Rng } from './rng.js';
import { Runner } from './runner.js';

/** A step of a preview: a pose, how long it shows, and how many art pixels off the ground. */
export type PreviewStep = [Pose, number, number];

/** A preview: the frames it shows, and `[frame, milliseconds, lift]` steps to play them in. */
export interface Preview {
    frames: Grid[];
    loop: [number, number, number][];
}

/** A trick played to the end on a 16 ms clock, as poses held for so long at so high. */
export function played(spec: PetSpec, name: string, seed: number): PreviewStep[] {
    const factory = spec.acts?.[name] ?? ACTS[name];
    if (factory === undefined) throw new Error(`no trick ${name}`);
    const ctx: ActContext = {
        rng: new Rng(seed),
        spec,
        room: () => ({ left: 0, right: 0 }),
        desk: () => null,
        toFloor: () => null,
        climb: () => null,
        crowded: () => false,
        fly: () => null,
    };
    const runner = new Runner(factory(ctx), 0);
    const steps: PreviewStep[] = [];
    for (let now = 0; !runner.done; now += 16) {
        if (now > 60_000) throw new Error(`${spec.kind} ${name} did not finish`);
        runner.update(now);
        runner.takeDx();
        runner.takeFx();
        const pose = quantizePose(runner.pose);
        const y = Math.round(runner.y);
        const last = steps[steps.length - 1];
        if (last !== undefined && poseKey(last[0]) === poseKey(pose) && last[2] === y) last[1] += 16;
        else steps.push([pose, 16, y]);
    }
    return steps;
}

/** `parts` played one after the other: `breath`, `blink`, or a trick's name. */
export function previewSteps(spec: PetSpec, parts: readonly string[]): PreviewStep[] {
    const quarter = BREATH_MS / 4;
    const steps: PreviewStep[] = [];
    parts.forEach((part, i) => {
        if (part === 'breath') steps.push([{}, quarter, 0], [{ squash: 0.1 }, quarter, 0], [{}, quarter, 0], [{ squash: -0.1 }, quarter, 0]);
        else if (part === 'blink') steps.push([{ mood: 'blink' }, BLINK_MS, 0], [{}, quarter, 0]);
        else steps.push(...played(spec, part, 7 + i));
    });
    return steps;
}

/**
 * A pack's preview's parts: as the five's are, with the first two of its tricks that stay
 * put. A trick that is mostly going somewhere -- `ink`, `roll` -- is left for the desktop.
 */
export function packParts(spec: PetSpec): string[] {
    const tricks = [...new Set(spec.specials)].filter(t => t !== 'ink' && t !== 'roll');
    const [first, second] = tricks;
    return [
        'breath',
        'blink',
        'breath',
        ...(first === undefined ? [] : [first, 'breath']),
        'jump',
        'breath',
        'blink',
        ...(second === undefined ? [] : [second]),
        'breath',
    ];
}

/** The preview of `parts`: each pose painted once, and the steps that play them. */
export function previewOf(spec: PetSpec, parts: readonly string[]): Preview {
    const keys: string[] = [];
    const frames: Grid[] = [];
    const loop: [number, number, number][] = [];
    for (const [pose, ms, y] of previewSteps(spec, parts)) {
        const key = poseKey(pose);
        let index = keys.indexOf(key);
        if (index < 0) {
            index = keys.length;
            keys.push(key);
            frames.push(spec.draw(pose));
        }
        loop.push([index, ms, y]);
    }
    return { frames, loop };
}
