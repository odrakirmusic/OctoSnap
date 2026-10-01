// SPDX-License-Identifier: GPL-3.0-or-later
//
// Ambient type declarations for the GJS runtime: the `gi://` and
// `resource:///org/gnome/shell/...` module specifiers, plus the globals GNOME Shell
// installs (`global`, `log`, ...). Kept in a .d.ts on purpose -- importing these for
// side effects from a .ts file would survive compilation into dist/ and fail at
// runtime, because GJS cannot resolve a bare npm specifier.
/// <reference types="@girs/gjs/dom" />
/// <reference types="@girs/gnome-shell/ambient" />
/// <reference types="@girs/gnome-shell/extensions/global" />

// The shell's own pixel picker, which `@girs/gnome-shell` does not type. `PickPixel` is
// exported from `ui/screenshot.js` on GNOME 50 (`export const PickPixel =
// GObject.registerClass(...)`) and is what `ScreenshotService.PickColorAsync` runs.
declare module 'resource:///org/gnome/shell/ui/screenshot.js' {
    import type Cogl from 'gi://Cogl';
    import type Shell from 'gi://Shell';
    import St from 'gi://St';

    /** The `color-pick` loupe cursor: a click chooses, Escape backs out. */
    export class PickPixel extends St.Widget {
        constructor(screenshot: Shell.Screenshot);
        /** The chosen pixel's colour, or `null` when the user pressed Escape. */
        pickAsync(): Promise<Cogl.Color | null>;
    }
}
