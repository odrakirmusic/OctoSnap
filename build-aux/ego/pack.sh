#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The extension as extensions.gnome.org takes it: one zip, built from the sources in
# this checkout, and checked against the review rules before anyone uploads it.
#
#   build-aux/ego/pack.sh [out-dir]        (default: target/ego)
#
# Not `gnome-extensions pack`. That tool wants every file beyond extension.js named with
# --extra-source, and this extension is thirty-odd modules in two directories, ten sounds
# and four icons. It also writes the zip with the files' own times, so two packs of the same
# tree differ. This writes the files in sorted order with one fixed time, so the same
# sources give the same zip, byte for byte.
#
# What differs from dist/, which is what `install-dev.sh` copies:
#   - no schemas/gschemas.compiled. GNOME Shell 44 and later compile the schema when they
#     install an extension from the site, and the review asks for the XML alone;
#   - nothing else. dist/ holds only what the shell loads, so the zip is dist/.
#
# Needs npm's packages in extension/node_modules (`npm ci` there), and python3.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:-$ROOT/target/ego}"
EXT="$ROOT/extension"

"$EXT/build.sh"
mkdir -p "$OUT"

python3 - "$EXT/dist" "$OUT" <<'PY'
import json
import os
import sys
import zipfile

dist, out = sys.argv[1], sys.argv[2]
metadata = json.load(open(os.path.join(dist, 'metadata.json')))
uuid = metadata['uuid']
zip_path = os.path.join(out, f'{uuid}.shell-extension.zip')

problems = []

# `metadata.json`, as the review guidelines ask for it.
for field in ('uuid', 'name', 'description', 'shell-version', 'url'):
    if not metadata.get(field):
        problems.append(f'metadata.json has no {field}')
if 'session-modes' in metadata and metadata['session-modes'] == ['user']:
    problems.append('metadata.json lists session-modes ["user"], which the review asks to drop')
if not all(str(v).isdigit() for v in metadata.get('shell-version', [])):
    problems.append(f'shell-version has a non-release entry: {metadata.get("shell-version")}')
if 'settings-schema' in metadata and not metadata['settings-schema'].startswith('org.gnome.shell.extensions.'):
    problems.append('the settings schema is not under org.gnome.shell.extensions')

files = []
for here, _, names in os.walk(dist):
    for name in names:
        path = os.path.join(here, name)
        files.append(os.path.relpath(path, dist))
files.sort()

kept = []
for rel in files:
    if rel == os.path.join('schemas', 'gschemas.compiled'):
        continue
    if rel.endswith('.ts') or '.test.' in rel or rel.endswith('.map'):
        problems.append(f'{rel} is a source or test file, not something the shell loads')
    if os.access(os.path.join(dist, rel), os.X_OK):
        problems.append(f'{rel} is executable, and the review takes no executables')
    kept.append(rel)

schema = metadata.get('settings-schema')
if schema and os.path.join('schemas', f'{schema}.gschema.xml') not in kept:
    problems.append(f'the zip would have no schemas/{schema}.gschema.xml')
for needed in ('extension.js', 'metadata.json'):
    if needed not in kept:
        problems.append(f'the zip would have no {needed}')

if problems:
    for line in problems:
        print('NOT PACKED: ' + line, file=sys.stderr)
    sys.exit(1)

# One time for every entry: the zip format's earliest, so nothing of the build machine's
# clock is in the file.
FIXED = (1980, 1, 1, 0, 0, 0)
with zipfile.ZipFile(zip_path, 'w', zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    for rel in kept:
        info = zipfile.ZipInfo(rel.replace(os.sep, '/'), FIXED)
        info.external_attr = 0o644 << 16
        info.compress_type = zipfile.ZIP_DEFLATED
        with open(os.path.join(dist, rel), 'rb') as f:
            z.writestr(info, f.read())

size = os.path.getsize(zip_path)
print(f'packed {len(kept)} files, {size / 1024:.0f} kB: {zip_path}')
print(f'  {metadata["name"]} {metadata.get("version-name", "")}, GNOME Shell {", ".join(metadata["shell-version"])}')
PY
