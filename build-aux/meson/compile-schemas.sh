#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Compiles the schemas in one installed directory, honouring DESTDIR, for a directory
# `gnome.post_install` does not know about (share/octosnap/remote-schemas, D123).
#
#   compile-schemas.sh <installed-directory>
set -eu
DIR="${DESTDIR:-}$1"
glib-compile-schemas --strict "$DIR"
echo "compiled the schemas in $DIR"
