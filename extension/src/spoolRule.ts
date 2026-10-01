// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Where the capture spool is, decided without GLib so it can be tested (D122).
 *
 * The spool is the app's: it reads each capture from there, files it into history and
 * sweeps what is left. The app knows its own spool and tells the extension (`SetSpool`)
 * whenever it starts. A capture taken while no app is running has to be written before
 * anything can be asked, and the app that will read it is the one D-Bus is about to
 * start. So the extension looks for the service file the bus will use: the first
 * `io.github.odrakirmusic.OctoSnap.service` in the session bus's own search order. A
 * Flatpak's service file runs `flatpak run`, and that app's cache is
 * `~/.var/app/<id>/cache`, not `~/.cache`.
 */

/** The session bus's service directories, in the order it searches them. */
export function serviceDirs(runtimeDir: string, dataHome: string, dataDirs: string[]): string[] {
    return [
        `${runtimeDir}/dbus-1/services`,
        `${dataHome}/dbus-1/services`,
        ...dataDirs.map(dir => `${dir}/dbus-1/services`),
    ];
}

/** The service file the bus would start `name` from: the first one found wins. */
export function serviceFile(dirs: string[], name: string, exists: (path: string) => boolean): string | null {
    for (const dir of dirs) {
        const path = `${dir}/${name}.service`;
        if (exists(path))
            return path;
    }
    return null;
}

/**
 * Whether a service file starts a Flatpak. Flatpak rewrites an exported service file's
 * `Exec` to `…/flatpak run …`; the directory it exports into is the fallback evidence.
 */
export function startsAFlatpak(path: string, text: string | null): boolean {
    if (text !== null) {
        const exec = text.split('\n').find(line => line.trim().startsWith('Exec='));
        if (exec !== undefined)
            return /(^|[/\s=])flatpak\s+run\b/.test(exec);
    }
    return path.includes('/flatpak/exports/');
}

/** The spool the app `id` reads, native or sandboxed. */
export function spoolFor(home: string, cacheHome: string, id: string, sandboxed: boolean): string {
    const cache = sandboxed ? `${home}/.var/app/${id}/cache` : cacheHome;
    return `${cache}/octosnap/spool`;
}

/**
 * Where a scrolling capture's frames go: `$XDG_RUNTIME_DIR/app/<id>/`, which Flatpak
 * binds into the sandbox at the same path, and which a native app reads like any other
 * directory. One rule for both, so nothing has to be decided per capture.
 */
export function framesDir(runtimeDir: string, id: string, handle: string): string {
    return `${runtimeDir}/app/${id}/scroll-${handle}`;
}
