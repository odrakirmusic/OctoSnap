# SPDX-License-Identifier: GPL-3.0-or-later
#
# The extension, two ways.
#
# For a distribution package, `-Dextension=true` installs it where GNOME Shell finds system
# extensions, and its settings schema where GSettings finds system schemas. That is the
# split a package needs and the opposite of what extensions.gnome.org wants: a system
# install cannot compile a schema inside the extension directory, and GNOME Shell reads the
# default schema source when that directory has no `schemas/` in it.
#
# For the Flatpak, `-Dbundle-extension=true` puts it in the app's own data, compiled schema
# and all. GNOME Shell does not look there, since a sandboxed app cannot install into the
# host's shell; the app copies it into the user's extensions folder, the one part of the
# host it is given for that (D160).
uuid = 'octosnap@odrakirmusic.github.io'
install_extension = meson.project_source_root() / 'build-aux' / 'meson' / 'install-extension.sh'

if get_option('extension')
  install_data(
    'schemas' / 'org.gnome.shell.extensions.octosnap.gschema.xml',
    install_dir: datadir / 'glib-2.0' / 'schemas',
  )
  meson.add_install_script(
    install_extension,
    meson.current_source_dir(),
    datadir / 'gnome-shell' / 'extensions' / uuid,
  )
endif

if get_option('bundle-extension')
  meson.add_install_script(
    install_extension,
    '--bundled',
    meson.current_source_dir(),
    datadir / 'octosnap' / 'extension',
  )
endif
