# SPDX-License-Identifier: GPL-3.0-or-later
#
# The extension, for a distribution package: `-Dextension=true` installs it where GNOME
# Shell finds system extensions, and its settings schema where GSettings finds system
# schemas. That is the split a package needs and the opposite of what extensions.gnome.org
# wants: a system install cannot compile a schema inside the extension directory, and GNOME
# Shell reads the default schema source when that directory has no `schemas/` in it.
#
# The Flatpak never sets this. A sandboxed app cannot install into the host's shell, so
# its users get the extension from extensions.gnome.org (`build-aux/ego/pack.sh`).
uuid = 'octosnap@odrakirmusic.github.io'

install_data(
  'schemas' / 'org.gnome.shell.extensions.octosnap.gschema.xml',
  install_dir: datadir / 'glib-2.0' / 'schemas',
)

meson.add_install_script(
  meson.project_source_root() / 'build-aux' / 'meson' / 'install-extension.sh',
  meson.current_source_dir(),
  datadir / 'gnome-shell' / 'extensions' / uuid,
)
