#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Meson's half of a Cargo build: builds the two binaries a desktop runs and copies them to
# where Meson expects its outputs, so `meson install` puts them in the prefix.
#
#   cargo-build.sh <cargo> <source-root> <target-dir> <release|debug> <output-dir>
#
# The target directory is Meson's, not the checkout's, so a Meson build and a
# `cargo build` in the same tree never share or overwrite each other's artefacts. Offline
# builds (the Flatpak's) are Cargo's own business: CARGO_HOME's config.toml and
# CARGO_NET_OFFLINE come from the environment the builder sets.
set -eu
CARGO="$1"
SOURCE="$2"
TARGET="$3"
PROFILE="$4"
OUT="$5"

if [ "$PROFILE" = release ]; then
  FLAGS=--release
else
  FLAGS=
fi

# --locked: the lock file is the one the Flatpak's vendored sources were generated from,
# and a build that quietly resolved something newer would not be the one that was vendored.
# shellcheck disable=SC2086
"$CARGO" build --locked $FLAGS \
  --manifest-path "$SOURCE/Cargo.toml" \
  --target-dir "$TARGET" \
  -p octosnap-app -p octosnap-cli

cp "$TARGET/$PROFILE/octosnap-app" "$OUT/octosnap-app"
cp "$TARGET/$PROFILE/octosnap" "$OUT/octosnap"
