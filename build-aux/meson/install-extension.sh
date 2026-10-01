#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds the extension and copies it into a system prefix, for `-Dextension=true`, or
# into the app's own data, for `-Dbundle-extension=true`.
#
#   install-extension.sh [--bundled] <extension-source-dir> <installed-extension-dir>
#
# The TypeScript compiler comes from the extension's own node_modules, so `npm ci` has to
# have run in that directory first; a package build does that in its source step, where
# the network is allowed. For a system install the schemas directory is left out on
# purpose: the schema is installed system-wide beside this (extension/meson.build), and an
# extension directory with a `schemas/` that has no compiled schema in it makes
# getSettings() fail.
#
# --bundled is the copy the app carries and installs for the user (D160). It is built with
# GJS (`build.sh --gjs`), which needs only `typescript` in node_modules, and it keeps its
# compiled schema: where the app puts it, the schema has to be beside it.
set -eu
BUNDLED=0
if [ "${1:-}" = --bundled ]; then
  BUNDLED=1
  shift
fi
SOURCE="$1"
DEST="${DESTDIR:-}$2"

if [ "$BUNDLED" = 1 ]; then
  "$SOURCE/build.sh" --gjs
else
  "$SOURCE/build.sh"
fi
rm -rf "$DEST"
mkdir -p "$DEST"
cp -r "$SOURCE/dist/." "$DEST/"
[ "$BUNDLED" = 1 ] || rm -rf "$DEST/schemas"
echo "installed the extension to $DEST"
