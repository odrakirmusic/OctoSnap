// SPDX-License-Identifier: GPL-3.0-or-later

import { describe, expect, it } from 'vitest';

import {
    GIF_FPS_CHOICES,
    GIF_FPS_MAX,
    GIF_QUALITY_CHOICES,
    GIF_WIDTH_CHOICES,
    SCREEN_RATE,
    SCREEN_RATE_FALLBACK,
    choiceIndex,
    fpsChoiceLabels,
    fpsLabel,
    gifDefaultsFromCopy,
    qualityLabel,
    recordedFps,
    refreshAt,
    widthLabel,
} from './recordChoices.js';

/** The owner's desk on 2026-09-25, as `DisplayConfig` reported it: the panel and DP-6. */
const PANEL_HZ = 60.025901794433594;
const BIG_HZ = 119.99758911132812;

/**
 * The screen rates `crates/media/src/gif.rs` pins for `GifSettings::fps_for`, with the
 * same answers (D117). The toolbar says the number and the app records at it, so the two
 * sides must agree; change one of these and change `gif.rs` with it.
 */
const SCREEN_RATES: readonly (readonly [number, number])[] = [
    [60, 30],
    [59.94, 30],
    [75, 38],
    [120, 40],
    [144, 48],
    [165, 41],
    [PANEL_HZ, 30],
    [BIG_HZ, 40],
];

describe('the GIF choices', () => {
    it('stop at the rate a GIF can actually play', () => {
        for (const fps of GIF_FPS_CHOICES) expect(fps).toBeLessThanOrEqual(GIF_FPS_MAX);
        expect(GIF_FPS_CHOICES).toContain(GIF_FPS_MAX);
        expect(GIF_FPS_CHOICES).not.toContain(60);
    });

    it('offer the screen rate as a choice with its own words', () => {
        expect(GIF_FPS_CHOICES).toContain(SCREEN_RATE);
        expect(fpsLabel(SCREEN_RATE, 60)).toBe('Screen rate · 30 fps');
        expect(fpsLabel(24, 60)).toBe('24 fps');
    });

    it('call an uncapped width by what it is', () => {
        expect(GIF_WIDTH_CHOICES[0]).toBe(0);
        expect(widthLabel(0)).toBe('Full size');
        expect(widthLabel(1280)).toBe('1280 px');
        expect(qualityLabel(80)).toBe('Quality 80');
    });

    it('do not pretend a typed setting is one of them', () => {
        // Settings typed by hand (`spec/05` §4.13's rule applies to the app's pages too)
        // can be anything in the schema's range; the label still says what it is, and
        // nothing in the list is lit.
        expect(choiceIndex(GIF_FPS_CHOICES, 12)).toBe(-1);
        expect(fpsLabel(12, 60)).toBe('12 fps');
        expect(choiceIndex(GIF_QUALITY_CHOICES, 80)).toBe(2);
    });
});

describe('the screen rate', () => {
    // D111: the refresh divided by the fewest whole times that bring it to fifty or under,
    // so that every frame is the same number of refreshes after the last.
    it('is the rate the app records at, on the rates gif.rs pins', () => {
        for (const [hz, fps] of SCREEN_RATES) {
            expect(recordedFps(SCREEN_RATE, hz), `${hz} Hz`).toBe(fps);
        }
    });

    it('divides by the fewest whole times, counted, and never goes over the ceiling', () => {
        // The rule in D111's words, counted up rather than worked out with `ceil`.
        for (let hz = 1; hz <= 360; hz += 0.25) {
            let fewest = 1;
            while (hz / fewest > GIF_FPS_MAX) fewest += 1;
            const fps = recordedFps(SCREEN_RATE, hz);
            expect(fps, `${hz} Hz`).toBe(Math.round(hz / fewest));
            expect(fps, `${hz} Hz`).toBeLessThanOrEqual(GIF_FPS_MAX);
        }
        expect(recordedFps(SCREEN_RATE, 50), 'a screen at the ceiling').toBe(50);
        expect(recordedFps(SCREEN_RATE, 30), 'a screen under it').toBe(30);
    });

    it('is the fallback when the refresh is not known, or not a rate', () => {
        expect(SCREEN_RATE_FALLBACK).toBe(30);
        for (const hz of [null, Number.NaN, Number.POSITIVE_INFINITY, 0, 0.5, -60]) {
            expect(recordedFps(SCREEN_RATE, hz), `${hz}`).toBe(SCREEN_RATE_FALLBACK);
        }
    });

    it('leaves a set rate as it is, and a request for sixty at fifty', () => {
        expect(recordedFps(15, BIG_HZ)).toBe(15);
        expect(recordedFps(15, null)).toBe(15);
        expect(recordedFps(60, PANEL_HZ)).toBe(GIF_FPS_MAX);
    });
});

describe('the frame-rate label', () => {
    it('says the number the screen rate records at', () => {
        expect(fpsLabel(SCREEN_RATE, PANEL_HZ)).toBe('Screen rate · 30 fps');
        expect(fpsLabel(SCREEN_RATE, BIG_HZ)).toBe('Screen rate · 40 fps');
        expect(fpsLabel(SCREEN_RATE, 144)).toBe('Screen rate · 48 fps');
    });

    it('says the fallback when the refresh is not known', () => {
        expect(fpsLabel(SCREEN_RATE, null)).toBe('Screen rate · 30 fps');
    });

    it('lists the choices for the monitor the toolbar is on', () => {
        expect(fpsChoiceLabels(BIG_HZ)).toEqual([
            '10 fps',
            '15 fps',
            '24 fps',
            '30 fps',
            '50 fps',
            'Screen rate · 40 fps',
        ]);
        // Not `GIF_FPS_CHOICES.map(fpsLabel)`, which would take the screen rate's index,
        // five, for its refresh and say "Screen rate · 5 fps".
        expect(fpsChoiceLabels(null).at(-1)).toBe('Screen rate · 30 fps');
    });
});

describe('a monitor refresh', () => {
    // The owner's desk: DP-6 at the stage's origin, the panel under it at 443.
    const desk = [
        { x: 0, y: 0, hz: BIG_HZ },
        { x: 443, y: 1440, hz: PANEL_HZ },
    ];

    it('is found by the monitor origin, as the app finds it', () => {
        expect(refreshAt(desk, 0, 0)).toBe(BIG_HZ);
        expect(refreshAt(desk, 443, 1440)).toBe(PANEL_HZ);
    });

    it('is not known where no view starts', () => {
        expect(refreshAt(desk, 443, 0)).toBeNull();
        expect(refreshAt([], 0, 0)).toBeNull();
    });

    it('is the first tile of a monitor driven as tiles', () => {
        // One 5120 x 2880 monitor as two 2560-wide tiles, each a view of its own.
        const tiled = [
            { x: 0, y: 0, hz: 60 },
            { x: 2560, y: 0, hz: 60 },
        ];
        expect(refreshAt(tiled, 0, 0)).toBe(60);
    });
});

describe('the app\'s GIF defaults, from its copy (D125)', () => {
    it('reads the four values the app writes', () => {
        expect(gifDefaultsFromCopy({ fps: 24, 'max-width': 0, quality: 90, cursor: false }))
            .toEqual({ fps: 24, maxWidth: 0, quality: 90, cursor: false });
    });

    it('is empty before the app has run', () => {
        expect(gifDefaultsFromCopy({})).toEqual({});
    });

    it('leaves out what the app\'s schema would not have accepted', () => {
        expect(gifDefaultsFromCopy({
            fps: 60,
            'max-width': -1,
            quality: 0,
            cursor: 'yes',
        })).toEqual({});
        expect(gifDefaultsFromCopy({ fps: 12.5, quality: 101, 'max-width': 7681 })).toEqual({});
        expect(gifDefaultsFromCopy({ fps: SCREEN_RATE, quality: 1 })).toEqual({ fps: 0, quality: 1 });
    });

    it('ignores keys it does not know', () => {
        expect(gifDefaultsFromCopy({ fps: 10, format: 'webm' })).toEqual({ fps: 10 });
    });
});
