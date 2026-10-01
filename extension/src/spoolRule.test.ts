// SPDX-License-Identifier: GPL-3.0-or-later

import { describe, expect, it } from 'vitest';

import { framesDir, serviceDirs, serviceFile, spoolFor, startsAFlatpak } from './spoolRule.js';

const ID = 'io.github.odrakirmusic.OctoSnap';
const HOME = '/home/u';

describe('the spool rule', () => {
    const dirs = serviceDirs('/run/user/1000', `${HOME}/.local/share`, [
        `${HOME}/.local/share/flatpak/exports/share`,
        '/var/lib/flatpak/exports/share',
        '/usr/local/share',
        '/usr/share',
    ]);

    it('searches the directories in the session bus order', () => {
        expect(dirs.slice(0, 3)).toEqual([
            '/run/user/1000/dbus-1/services',
            `${HOME}/.local/share/dbus-1/services`,
            `${HOME}/.local/share/flatpak/exports/share/dbus-1/services`,
        ]);
    });

    it('takes the first service file, as the bus does', () => {
        const present = new Set([
            `${HOME}/.local/share/dbus-1/services/${ID}.service`,
            `${HOME}/.local/share/flatpak/exports/share/dbus-1/services/${ID}.service`,
        ]);
        expect(serviceFile(dirs, ID, path => present.has(path))).toBe(
            `${HOME}/.local/share/dbus-1/services/${ID}.service`,
        );
        expect(serviceFile(dirs, ID, () => false)).toBeNull();
    });

    it('knows a Flatpak by its Exec line', () => {
        const flatpak = '[D-BUS Service]\nName=x\nExec=/usr/bin/flatpak run --branch=stable --arch=x86_64 --command=octosnap-app io.github.odrakirmusic.OctoSnap\n';
        const native = '[D-BUS Service]\nName=x\nExec=/usr/bin/octosnap-app\n';
        expect(startsAFlatpak('/any/where.service', flatpak)).toBe(true);
        expect(startsAFlatpak('/any/where.service', native)).toBe(false);
        // A development build that happens to live under a folder called flatpak is not one.
        expect(startsAFlatpak('/any/where.service', 'Exec=/home/u/flatpak/target/release/octosnap-app\n')).toBe(false);
    });

    it('falls back on the export directory when the file cannot be read', () => {
        expect(startsAFlatpak('/var/lib/flatpak/exports/share/dbus-1/services/x.service', null)).toBe(true);
        expect(startsAFlatpak('/usr/share/dbus-1/services/x.service', null)).toBe(false);
    });

    it('puts a sandboxed app\'s spool in its own cache', () => {
        expect(spoolFor(HOME, `${HOME}/.cache`, ID, false)).toBe(`${HOME}/.cache/octosnap/spool`);
        expect(spoolFor(HOME, `${HOME}/.cache`, ID, true)).toBe(`${HOME}/.var/app/${ID}/cache/octosnap/spool`);
    });

    it('puts scroll frames where Flatpak shares a runtime directory', () => {
        expect(framesDir('/run/user/1000', ID, '01H')).toBe(`/run/user/1000/app/${ID}/scroll-01H`);
    });
});
