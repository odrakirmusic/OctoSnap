// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The extension's own icons (`spec/09` §4b and §5): `icons/<name>.svg` beside this module,
 * which `build.sh` fills from `extension/icons/` and, for the panel, with the app's own
 * symbolic from `data/icons/`. A name with no file there is the icon theme's.
 *
 * A `Gio.FileIcon` is recoloured like a theme icon when its file name ends in
 * `-symbolic.svg`: that suffix is how GNOME Shell's icon loader knows a symbolic.
 */

import Gio from 'gi://Gio';

/**
 * The panel's icon, the app's symbolic (D121). Bundled rather than looked up in the
 * theme: an extension from extensions.gnome.org can be installed before the app, and a
 * missing icon name is a broken-image box in the top bar on every login.
 */
export const APP_SYMBOLIC = 'io.github.odrakirmusic.OctoSnap-symbolic';

const found = new Map<string, Gio.Icon>();

/** The icon called `name`: the extension's file if it has one, else the theme's. */
export function icon(name: string): Gio.Icon {
    let result = found.get(name);
    if (result === undefined) {
        const file = Gio.File.new_for_uri(import.meta.url).get_parent()?.get_child('icons').get_child(`${name}.svg`);
        result = file?.query_exists(null) === true ? new Gio.FileIcon({ file }) : new Gio.ThemedIcon({ name });
        found.set(name, result);
    }
    return result;
}

/** For `disable()`, which leaves nothing in module scope (D135). */
export function forgetIcons(): void {
    found.clear();
}
