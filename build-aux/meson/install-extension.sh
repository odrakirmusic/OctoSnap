#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds the extension and copies it into a system prefix, for `-Dextension=true`.
#
#   install-extension.sh <extension-source-dir> <installed-extension-dir>
#
# The TypeScript compiler comes from the extension's own node_modules, so `npm ci` has to
# have run in that directory first; a package build does that in its source step, where
# the network is allowed. The schemas directory is left out on purpose: the schema is
# installed system-wide beside this (extension/meson.build), and an extension directory
# with a `schemas/` that has no compiled schema in it makes getSettings() fail.
set -eu
SOURCE="$1"
DEST="${DESTDIR:-}$2"

"$SOURCE/build.sh"
rm -rf "$DEST"
mkdir -p "$DEST"
cp -r "$SOURCE/dist/." "$DEST/"
rm -rf "$DEST/schemas"
echo "installed the extension to $DEST"
