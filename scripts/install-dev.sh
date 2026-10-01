#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Wires a build into the user's session so both halves can talk: builds the Rust
# binaries, installs a D-Bus activation file pointing at them, and builds and links the
# extension. Nothing here touches system directories and nothing needs sudo.
#
# The build is **optimised** unless `--debug` is given (D102). The installed app is the one
# that gets used, and an unoptimised build is not a slower copy of it but a different
# program: gifski encodes about two frames a second without optimisation against
# thirty-five with it, so a 50 fps recording backed up until Stop gave up and threw the GIF
# away, and the scroll matcher and the PNG decoder run four to ten times slower. `--debug`
# is for a debugger, not for daily use.
#
# It deliberately does NOT enable the extension. Loading extension code into a live
# GNOME session is the user's call, and on Wayland a bad one costs a logout.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$(pwd)"

PROFILE=release
if [ "${1:-}" = "--debug" ]; then
    PROFILE=debug
fi

echo "==> building the Rust workspace (${PROFILE})"
if [ "${PROFILE}" = release ]; then
    cargo build --workspace --release
else
    cargo build --workspace
fi

# The build's real directory. A worktree's `target` can be a link to a shared one, and a
# path through the link would stop working when the worktree is removed.
BINDIR="$(cd "${ROOT}/target/${PROFILE}" && pwd -P)"

# A no-op once there is nothing old left (D141). The script moved one person's data from
# the app's old name, and the public tree goes without it (D156).
MIGRATED=""
if [ -x scripts/migrate-legacy-name.sh ]; then
  echo "==> moving anything left under the app's old name"
  MIGRATED=$(scripts/migrate-legacy-name.sh "${BINDIR}")
fi

SERVICE_DIR="${HOME}/.local/share/dbus-1/services"
SERVICE="${SERVICE_DIR}/io.github.odrakirmusic.OctoSnap.service"

echo "==> installing D-Bus activation -> ${SERVICE}"
mkdir -p "${SERVICE_DIR}"
# The malloc settings as a native `meson install` writes them (data/meson.build, D128).
sed -e "s|@BINDIR@|${BINDIR}|g" \
    -e "s|@LAUNCH_ENV@|/usr/bin/env MALLOC_ARENA_MAX=2 MALLOC_MMAP_THRESHOLD_=131072 |g" \
    data/io.github.odrakirmusic.OctoSnap.service.in > "${SERVICE}"

echo "==> installing the app's GSettings schema"
# $XDG_DATA_HOME/glib-2.0/schemas is a default GSettings search path, so no environment
# variable is needed and the D-Bus-activated service finds it. gio::Settings::new aborts
# the process on a missing schema, so this is not optional.
SCHEMA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/glib-2.0/schemas"
mkdir -p "${SCHEMA_DIR}"
cp data/io.github.odrakirmusic.OctoSnap.gschema.xml "${SCHEMA_DIR}/"
glib-compile-schemas "${SCHEMA_DIR}"

echo "==> installing the desktop entry and spec/05 §8's project MIME type"
# Both under $XDG_DATA_HOME, which is already on the search path for each. The MIME
# database has to be rebuilt afterwards or the type is declared and nothing knows it;
# `update-desktop-database` is what associates the .desktop file with it, so a
# double-clicked project opens the editor rather than a "no application" dialog.
APP_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
MIME_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/mime/packages"
mkdir -p "${APP_DIR}" "${MIME_DIR}"
sed "s|@BINDIR@|${BINDIR}|g" data/io.github.odrakirmusic.OctoSnap.desktop.in \
    > "${APP_DIR}/io.github.odrakirmusic.OctoSnap.desktop"
cp data/io.github.odrakirmusic.OctoSnap.xml "${MIME_DIR}/"
update-mime-database "${XDG_DATA_HOME:-$HOME/.local/share}/mime" 2>/dev/null \
    || echo "    (no update-mime-database; .octosnap will not be recognised by type)"
update-desktop-database "${APP_DIR}" 2>/dev/null \
    || echo "    (no update-desktop-database; double-clicking a project may not work)"

echo "==> installing the app icon"
# The app draws its own icon from its resources; the app grid, the dash and the
# notifications read the icon theme, which is this.
ICON_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"
mkdir -p "${ICON_DIR}/scalable/apps" "${ICON_DIR}/symbolic/apps"
cp data/icons/hicolor/scalable/apps/io.github.odrakirmusic.OctoSnap.svg "${ICON_DIR}/scalable/apps/"
cp data/icons/hicolor/symbolic/apps/io.github.odrakirmusic.OctoSnap-symbolic.svg "${ICON_DIR}/symbolic/apps/"
# A cache already in the user's own hicolor directory is brought up to date. GNOME trusts
# it over the files for as long as the directory itself is no newer, and it lists only
# the icons there when it was written: on 2026-09-27 one another app's installer had left
# in August hid this icon, and the app grid showed a gear (D149). None is written where
# there is none, since a new one would hide every icon installed after it the same way,
# and GNOME notices a new icon in an uncached directory by itself.
if [ -f "${ICON_DIR}/icon-theme.cache" ]; then
    gtk-update-icon-cache --force --ignore-theme-index --quiet "${ICON_DIR}" 2>/dev/null \
        || echo "    (could not refresh ${ICON_DIR}/icon-theme.cache; the app may show a gear)"
fi

echo "==> building the extension"
( cd extension && npm install --silent && ./build.sh && ./install-dev.sh )

UUID=$(python3 -c "import json;print(json.load(open('extension/metadata.json'))['uuid'])")
# Asked, not assumed: this line used to say "not enabled yet" on every run, including
# the ones on a machine where the extension had been enabled and running for weeks.
INFO=$(gnome-extensions info "${UUID}" 2>/dev/null || true)
case "${INFO}" in
    *"Enabled: Yes"*) STATE="Both halves are installed, and the extension is enabled." ;;
    *) STATE="Both halves are installed but the extension is not enabled yet." ;;
esac

cat <<NEXT

Done. ${STATE}${MIGRATED:+

${MIGRATED}}

  Check what is running:      ${BINDIR}/octosnap status
  Enable the extension:       gnome-extensions enable ${UUID}
  Watch the extension's log:  journalctl --user -f -o cat /usr/bin/gnome-shell | grep octosnap
  Watch the app's log:        journalctl --user -f -t octosnap-app

GNOME Shell 50 needs a full logout to pick up a new extension, AND to pick up changed
extension code -- it caches the ES modules, so 'gnome-extensions disable' then 'enable'
is not enough. That applies to a *failed* load too: an extension that errored stays
errored until the shell restarts, however many times you fix the file and re-enable.

Iterate in a nested shell instead, on a session bus of its own:

  dbus-run-session gnome-shell --devkit --virtual-monitor 1920x1200@60

and keep the real session for acceptance runs. Two things the nested shell cannot answer,
both learned the hard way: it has no real evdev seat, so a *negative* result about input
means nothing, and it has no cursor sprite, so the pointer in a capture cannot be judged
there. Its timings are also an order of magnitude off; only the real session measures.

The extension half is installed as a copy rather than a symlink, deliberately -- see
extension/install-dev.sh for why. Re-run this script after changing extension code.
NEXT
