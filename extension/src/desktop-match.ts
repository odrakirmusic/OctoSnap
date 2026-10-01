// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Which windows are the desktop icons (`DSK-01`, `CAP-12`).
 *
 * Its own module with no `gi://` imports, so it can be tested: the project's test runner
 * cannot load GNOME's typelibs, and the rule is the part worth a test.
 *
 * On Ubuntu the icons are DING's (`ding@rastersoft.com`), a separate GTK process whose
 * windows the shell half of DING turns into X11-style desktop windows by **title**: its
 * `emulateX11WindowType.js` reads titles that start with `Desktop Icons ` or carry the
 * `@!` flag marker, and sets `Meta.WindowType.DESKTOP` on them. Both readings are
 * accepted here -- the type once DING has set it, the title before it has -- so a window
 * is recognised whichever of the two arrives first. Vanilla GNOME has no desktop icons
 * and no window answers.
 */
export function looksLikeDesktopIcons(title: string | null, isDesktopType: boolean): boolean {
    if (isDesktopType) return true;
    if (title === null) return false;
    return title.startsWith('Desktop Icons') || title.includes('@!');
}
