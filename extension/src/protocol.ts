// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * Wire constants shared with the Rust half of OctoSnap. Mirrors `spec/10` §3.
 *
 * Anything here that changes shape is a protocol break: bump PROTOCOL_VERSION and
 * handle the mismatch in the app's handshake (`spec/10` §2).
 */

/** Bus name the extension owns, so the app can reach it without going via org.gnome.Shell. */
export const SHELL_BUS_NAME = 'io.github.odrakirmusic.OctoSnap.Shell';
export const SHELL_OBJECT_PATH = '/org/octosnap/Shell';
export const SHELL_INTERFACE = 'io.github.odrakirmusic.OctoSnap.Shell';

/** The app half. D-Bus-activated, so calling it starts it (`spec/10` §2). */
export const APP_BUS_NAME = 'io.github.odrakirmusic.OctoSnap';
export const APP_OBJECT_PATH = '/org/octosnap/App';

/**
 * GApplication's own object path, derived by GTK from the application id.
 *
 * Captures are delivered here, as `org.freedesktop.Application.ActivateAction`, rather
 * than to `App1` on APP_OBJECT_PATH. A method call on a custom interface is *lost* when
 * it is the call that D-Bus-activates the app -- see `docs/spikes/13-cold-activation-race.md`
 * and decision D8. Without this, the first capture after every login would vanish.
 */
export const APP_GAPPLICATION_PATH = '/io/github/odrakirmusic/OctoSnap';
export const FREEDESKTOP_APPLICATION = 'org.freedesktop.Application';
export const APP_INTERFACE = 'io.github.odrakirmusic.OctoSnap.App1';

/** `spec/10` §3: "D-Bus contract (protocol version 1)". */
export const PROTOCOL_VERSION = 1;

/** Extension release, independent of the protocol. Reported by Version(). */
export const EXTENSION_VERSION = '0.1.5';

/** `spec/10` §8: extension logs via console.log with a [octosnap] prefix. */
export const LOG_PREFIX = '[octosnap]';
