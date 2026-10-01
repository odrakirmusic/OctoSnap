// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Rendering a GSettings accelerator for a person to read.
 *
 * Its own module, with no `gi://` imports, for the same reason `selection.ts` is: the
 * project's test runner cannot load GNOME's typelibs, so anything that needs a test has
 * to be reachable without them. That constraint is worth keeping -- it is what has kept
 * the arithmetic and the string handling out of the actor plumbing.
 */

/**
 * Turns a GSettings accelerator into something a person reads: `<Super><Shift>4` becomes
 * `Super+Shift+4`.
 *
 * Written out rather than borrowed from `Gtk.accelerator_get_label`, because Gtk is not
 * loaded inside `gnome-shell` and pulling it in for one label would put a second toolkit
 * in the compositor's process.
 *
 * Unknown key names pass through unchanged. An accelerator this function does not
 * recognise is still the string the user has to press, so showing it beats showing
 * nothing.
 */
export function prettyAccelerator(accelerator: string): string {
    const parts: string[] = [];
    // Modifiers are `<Name>` prefixes; whatever follows the last `>` is the key.
    const pattern = /<([A-Za-z0-9_]+)>/g;
    let match: RegExpExecArray | null;
    while ((match = pattern.exec(accelerator)) !== null) {
        parts.push(MODIFIER_LABELS[match[1]!.toLowerCase()] ?? match[1]!);
    }

    const key = accelerator.slice(accelerator.lastIndexOf('>') + 1);
    if (key !== '') parts.push(KEY_LABELS[key.toLowerCase()] ?? key.toUpperCase());

    return parts.join('+');
}

/** GSettings modifier names, in the spelling GNOME's own shortcut list uses. */
const MODIFIER_LABELS: Record<string, string> = {
    'primary': 'Ctrl',
    'control': 'Ctrl',
    'ctrl': 'Ctrl',
    'shift': 'Shift',
    'alt': 'Alt',
    'mod1': 'Alt',
    'super': 'Super',
    'meta': 'Meta',
    'hyper': 'Hyper',
};

/** Keys whose GDK name is not what a user would recognise on a keycap. */
const KEY_LABELS: Record<string, string> = {
    'space': 'Space',
    'return': 'Enter',
    'escape': 'Esc',
    'print': 'Print',
    'backspace': 'Backspace',
    'delete': 'Delete',
    'tab': 'Tab',
    'up': '\u2191',
    'down': '\u2193',
    'left': '\u2190',
    'right': '\u2192',
};
