#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Compiles the TypeScript sources to GJS-loadable ESM and assembles dist/ into a
# directory GNOME Shell can load directly (spec/10 §10).
#
#   build.sh [--gjs]
#
# --gjs compiles with TypeScript's own compiler run by GJS (`transpile.js`) instead of
# node's tsc, for a build that has GJS and no node: the Flatpak's, which carries the
# extension (D160). It needs only `typescript` in node_modules, and gives the same files.
set -euo pipefail
cd "$(dirname "$0")"

rm -rf dist
if [ "${1:-}" = --gjs ]; then
  # GJS warns once about a line of TypeScript's own code, which is not this repository's.
  gjs -m transpile.js node_modules/typescript/lib/typescript.js
else
  ./node_modules/.bin/tsc
fi

# GNOME Shell loads metadata.json, stylesheet.css and the compiled schema from the
# extension directory root, so they have to sit beside the emitted JS, not above it.
cp metadata.json stylesheet.css dist/
mkdir -p dist/schemas
cp schemas/*.gschema.xml dist/schemas/
glib-compile-schemas dist/schemas/

# The sounds, beside the module that plays them (`src/sound.ts`). Only the files:
# `sounds/synth.py` is how they are made, not part of the extension.
mkdir -p dist/sounds
cp sounds/*.oga dist/sounds/

# The icons the shell draws that no theme has (`src/icons.ts`): the overlay's modes, and
# the app's own symbolic for the panel -- a copy, because an extension from
# extensions.gnome.org can be installed without the app, and the app's icon with it.
mkdir -p dist/icons
cp icons/*.svg dist/icons/
cp ../data/icons/hicolor/symbolic/apps/io.github.odrakirmusic.OctoSnap-symbolic.svg dist/icons/

echo "built dist/ ($(find dist -type f | wc -l) files)"
