# SPDX-License-Identifier: GPL-3.0-or-later
#
# The install half of the build: the two Rust binaries, and every file a desktop needs to
# find, activate and describe the app (`spec/10` §10, the Loupe pattern). Cargo stays the
# build system for the Rust code. Meson is what puts the results where GNOME looks, which
# is what the Flatpak and a distribution package both need, and what `cargo install`
# cannot do.
#
#   meson setup _build && meson compile -C _build && meson install -C _build
#
# `scripts/install-dev.sh` is still the development loop. It points the D-Bus service at
# `target/release` in the checkout instead of copying, so a rebuild is live without an
# install step.
project(
  'octosnap',
  version: '0.1.2',
  license: 'GPL-3.0-or-later',
  meson_version: '>= 1.1',
)

gnome = import('gnome')

app_id = 'io.github.odrakirmusic.OctoSnap'
bindir = get_option('prefix') / get_option('bindir')
datadir = get_option('prefix') / get_option('datadir')

subdir('data')

cargo = find_program('cargo')
cargo_build = find_program('build-aux/meson/cargo-build.sh')
custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  console: true,
  output: ['octosnap-app', 'octosnap'],
  install: true,
  install_dir: bindir,
  command: [
    cargo_build,
    cargo,
    meson.project_source_root(),
    meson.project_build_root() / 'target',
    get_option('profile'),
    '@OUTDIR@',
  ],
)

if get_option('extension') or get_option('bundle-extension')
  subdir('extension')
endif

gnome.post_install(
  glib_compile_schemas: true,
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
