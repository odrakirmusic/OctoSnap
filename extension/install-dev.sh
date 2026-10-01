#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Installs dist/ into the user's extension directory as a **copy**, not a symlink.
#
# A symlink into the working tree would be the obvious choice and it is the wrong one.
# GNOME Shell 50 caches an extension's ES modules, so a logout is the only way to load
# changed extension code -- which means what the real
# session loads at the *next* login should be a snapshot taken at a known-good point, not
# whatever the tree happens to contain when the user logs out. With a symlink, editing a
# file at 5pm changes what boots tomorrow morning.
#
# Iterate in a nested shell instead:
#   dbus-run-session gnome-shell --devkit --virtual-monitor 1920x1200@60
set -euo pipefail
cd "$(dirname "$0")"

UUID=$(python3 -c "import json;print(json.load(open('metadata.json'))['uuid'])")
TARGET="${HOME}/.local/share/gnome-shell/extensions/${UUID}"

[ -d dist ] || { echo "dist/ missing: run ./build.sh first" >&2; exit 1; }

rm -rf "$TARGET"
mkdir -p "$TARGET"
cp -r dist/. "$TARGET/"
echo "installed $(find "$TARGET" -type f | wc -l) files to $TARGET"
echo
echo "GNOME Shell 50 will not load this until a full logout, and 'gnome-extensions"
echo "disable' then 'enable' is NOT enough -- it serves the cached modules."
echo "  first install:  gnome-extensions enable ${UUID}, then log out and back in"
echo "  code changed:   log out and back in"
