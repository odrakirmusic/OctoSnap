// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The actor every overlay puts its own actors inside.
 *
 * `Main.layoutManager.screenshotUIGroup` is the right *place* -- it sits above the panel
 * and the windows, which is what a capture overlay needs -- but it is an `St.Widget` with
 * a **`Clutter.BinLayout`**, and a BinLayout does not lay children out by their own
 * coordinates. Worse than ignoring them, it produces an allocation assembled from the
 * wrong numbers.
 *
 * Measured on a two-monitor rig, from the roots this replaced:
 *
 * ```
 * m0 wants    0,0 2560x1440  ->  box    0,0,2560,1440   (2560x1440)   correct
 * m1 wants 2560,0 1920x1200  ->  box 2560,0,2560,1200   (   0x1200)   width zero
 * ```
 *
 * The box comes out as `(x, y, width, height)` read as `(x1, y1, x2, y2)`, so it is right
 * exactly when the actor sits at the stage origin and wrong everywhere else. **The monitor
 * at 0,0 was correct by accident**, which is why a single-monitor rig -- and a two-monitor
 * rig tested only for the geometry its captures *report* -- never showed it. What the user
 * saw was the dim failing to cover the second screen.
 *
 * A `Clutter.FixedLayout` host inside that group fixes it for everything at once: roots,
 * the window picker's roots, the fly animation, and (M5) the recording frame.
 * `docs/decisions.md` D24.
 */

import Clutter from 'gi://Clutter';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';

/**
 * A full-stage host that lays its children out by their own coordinates.
 *
 * The caller owns it and must `destroy()` it, which takes the children with it. Sized to
 * the stage explicitly: Clutter does not clip to the allocation by default, so children
 * would paint outside a smaller host anyway, but a host whose size means what it says is
 * one less thing to reason about when the next actor goes missing.
 */
export function createOverlayHost(
    name: string,
    options: { interactive?: boolean } = {},
): St.Widget {
    // `interactive` hosts are the ones that take the modal grab, and taking it *here*
    // rather than on one monitor's root is what lets the toolbar live on any monitor: a
    // `Clutter.Grab` confines input to the grabbed actor's subtree, so anything the user
    // must be able to click has to be inside it. One host over the whole stage contains
    // every root and every piece of chrome, whichever monitor they are on.
    const interactive = options.interactive === true;
    const host = new St.Widget({
        name,
        reactive: interactive,
        can_focus: interactive,
        x: 0,
        y: 0,
        width: global.stage.width,
        height: global.stage.height,
        layout_manager: new Clutter.FixedLayout(),
    });
    Main.layoutManager.screenshotUIGroup.add_child(host);
    return host;
}
