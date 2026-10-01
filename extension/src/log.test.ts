// SPDX-License-Identifier: GPL-3.0-or-later

import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';

import { KEPT_LINES, error, forgetLog, info, recentLog, setVerbose, warn } from './log.js';

describe('recentLog', () => {
    // The shell's console is not the test runner's business.
    beforeAll(() => {
        vi.spyOn(console, 'log').mockImplementation(() => {});
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        vi.spyOn(console, 'error').mockImplementation(() => {});
    });
    afterAll(() => vi.restoreAllMocks());

    it('keeps each line with its level, newest last', () => {
        info('first');
        warn('second');
        error('third', new Error('why'));
        const lines = recentLog().slice(-3);
        expect(lines[0]).toMatch(/^\d{4}-\d\d-\d\dT[\d:.]+Z INFO first$/);
        expect(lines[1]).toMatch(/ WARN second$/);
        expect(lines[2]).toMatch(/ ERROR third: why$/);
    });

    it(`keeps only the last ${KEPT_LINES} lines`, () => {
        for (let n = 0; n < KEPT_LINES + 20; n++)
            info(`line ${n}`);
        const lines = recentLog();
        expect(lines).toHaveLength(KEPT_LINES);
        expect(lines[0]).toMatch(/ line 20$/);
        expect(lines[KEPT_LINES - 1]).toMatch(new RegExp(` line ${KEPT_LINES + 19}$`));
    });

    it('hands out a copy, so a caller cannot edit what is kept', () => {
        const lines = recentLog();
        lines.length = 0;
        expect(recentLog()).toHaveLength(KEPT_LINES);
    });
});

describe('the journal (D135)', () => {
    afterAll(() => {
        vi.restoreAllMocks();
        forgetLog();
    });

    it('gets routine lines only with debug-log on, and the report gets them either way', () => {
        const log = vi.spyOn(console, 'log').mockImplementation(() => {});
        forgetLog();
        info('quiet');
        expect(log).not.toHaveBeenCalled();
        setVerbose(true);
        info('loud');
        expect(log).toHaveBeenCalledTimes(1);
        expect(recentLog().map(line => line.split(' ').slice(2).join(' '))).toEqual(['quiet', 'loud']);
    });

    it('always gets warnings and errors', () => {
        const warned = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const failed = vi.spyOn(console, 'error').mockImplementation(() => {});
        setVerbose(false);
        warn('careful');
        error('broken');
        expect(warned).toHaveBeenCalledTimes(1);
        expect(failed).toHaveBeenCalledTimes(1);
    });

    it('is left with nothing by disable, which also turns debug-log back off', () => {
        const log = vi.spyOn(console, 'log').mockImplementation(() => {});
        setVerbose(true);
        info('before');
        forgetLog();
        expect(recentLog()).toEqual([]);
        log.mockClear();
        info('after');
        expect(log).not.toHaveBeenCalled();
    });
});
