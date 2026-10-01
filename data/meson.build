# SPDX-License-Identifier: GPL-3.0-or-later
#
# The files a desktop reads about the app, each installed where it looks and each checked
# by the validator the desktop's own tooling would run on it. `meson test -C _build`
# answers "would Flathub's linter or a distribution's lintian reject this" before either
# is asked.

substitutions = configuration_data()
substitutions.set('BINDIR', bindir)
# D128: two malloc arenas rather than one per thread that ever allocated, and a fixed mmap
# threshold rather than one that rises to the largest buffer freed, so that neither the
# decode workers' freed buffers nor a screen's worth of pixels stay with the service. Set
# where the service is started, since glibc reads them once, at exec. Not in a Flatpak
# (prefix /app): there they are the manifest's `--env`, because `flatpak run` would take an
# `env` in `Exec` as the command to run.
substitutions.set(
  'LAUNCH_ENV',
  get_option('prefix') == '/app' ? '' : '/usr/bin/env MALLOC_ARENA_MAX=2 MALLOC_MMAP_THRESHOLD_=131072 ',
)

desktop_file = configure_file(
  input: app_id + '.desktop.in',
  output: app_id + '.desktop',
  configuration: substitutions,
  install: true,
  install_dir: datadir / 'applications',
)

configure_file(
  input: app_id + '.service.in',
  output: app_id + '.service',
  configuration: substitutions,
  install: true,
  install_dir: datadir / 'dbus-1' / 'services',
)

install_data(
  app_id + '.gschema.xml',
  install_dir: datadir / 'glib-2.0' / 'schemas',
)

# spec/05 §8's project type. Installed under the app id so a package that also ships the
# thumbnailer (M8) adds a file beside it rather than replacing this one.
install_data(
  app_id + '.xml',
  install_dir: datadir / 'mime' / 'packages',
)

metainfo_file = files(app_id + '.metainfo.xml')
install_data(
  metainfo_file,
  install_dir: datadir / 'metainfo',
)

desktop_file_validate = find_program('desktop-file-validate', required: false)
if desktop_file_validate.found()
  test('desktop file', desktop_file_validate, args: [desktop_file])
endif

compile_schemas = find_program('glib-compile-schemas', required: false)
if compile_schemas.found()
  test(
    'gsettings schema',
    compile_schemas,
    args: ['--strict', '--dry-run', meson.current_source_dir()],
  )
endif

appstreamcli = find_program('appstreamcli', required: false)
if appstreamcli.found()
  test(
    'metainfo',
    appstreamcli,
    args: ['validate', '--no-net', '--explain', metainfo_file],
  )
endif

# D123: the schemas the sandboxed app mirrors from the extension -- the extension's own,
# and the three Print keys of GNOME Shell's -- in a directory of the app's, which is not a
# GSettings search path, so only the mirror ever reads them.
remote_schemas_dir = datadir / 'octosnap' / 'remote-schemas'
install_data(
  meson.project_source_root() / 'extension' / 'schemas' / 'org.gnome.shell.extensions.octosnap.gschema.xml',
  'remote-schemas' / 'org.gnome.shell.keybindings.gschema.xml',
  install_dir: remote_schemas_dir,
)
meson.add_install_script(
  meson.project_source_root() / 'build-aux' / 'meson' / 'compile-schemas.sh',
  remote_schemas_dir,
)
if compile_schemas.found()
  test(
    'mirrored schemas',
    compile_schemas,
    args: ['--strict', '--dry-run', meson.current_source_dir() / 'remote-schemas'],
  )
endif

# The application icon and its symbolic twin (spec/09 §4b), where the icon theme looks.
install_data(
  'icons' / 'hicolor' / 'scalable' / 'apps' / app_id + '.svg',
  install_dir: datadir / 'icons' / 'hicolor' / 'scalable' / 'apps',
)
install_data(
  'icons' / 'hicolor' / 'symbolic' / 'apps' / app_id + '-symbolic.svg',
  install_dir: datadir / 'icons' / 'hicolor' / 'symbolic' / 'apps',
)
