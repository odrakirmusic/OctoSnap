// SPDX-License-Identifier: GPL-3.0-or-later
//
// What `tsc` writes to dist/, made with GJS instead of node (D160).
//
//   gjs -m transpile.js <typescript.js>
//
// Run from this directory, as `build.sh --gjs` does. The Flatpak's SDK has GJS and no node,
// and TypeScript's compiler is plain JavaScript, so the bundled extension is built from
// these sources there too. Each source is transpiled on its own, with tsconfig.json's
// options, and that is what tsc emits as well: `isolatedModules` and
// `verbatimModuleSyntax` keep types out of the output, so it needs no type packages and no
// checking. `publish.sh --flatpak` checks that the two builds agree, file for file.

import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import System from 'system';

const decoder = new TextDecoder();

function read(path) {
    const [, bytes] = GLib.file_get_contents(path);
    return decoder.decode(bytes);
}

function fail(message) {
    printerr(`transpile: ${message}`);
    System.exit(1);
}

const [typescript] = System.programArgs;
if (!typescript)
    fail('usage: gjs -m transpile.js <typescript.js>');
// typescript.js declares `var ts` and fills it in.
const ts = new Function(`${read(typescript)}\nreturn ts;`)();

const config = ts.parseConfigFileTextToJson('tsconfig.json', read('tsconfig.json'));
if (config.error)
    fail(ts.flattenDiagnosticMessageText(config.error.messageText, '\n'));
// The sources tsc compiles, which is all this walks: change tsconfig.json's and this has
// to change with it.
const {include, exclude, compilerOptions} = config.config;
if (JSON.stringify(include) !== '["src/**/*.ts"]' || JSON.stringify(exclude) !== '["src/**/*.test.ts"]')
    fail(`tsconfig.json compiles ${JSON.stringify(include)} without ${JSON.stringify(exclude)}, which this does not`);
const {options, errors} = ts.convertCompilerOptionsFromJson(compilerOptions, GLib.get_current_dir());
if (errors.length > 0)
    fail(errors.map(e => ts.flattenDiagnosticMessageText(e.messageText, '\n')).join('\n'));

function sources(dir, under = '') {
    const found = [];
    const children = Gio.File.new_for_path(dir).enumerate_children(
        'standard::name,standard::type', Gio.FileQueryInfoFlags.NOFOLLOW_SYMLINKS, null);
    for (let info = children.next_file(null); info !== null; info = children.next_file(null)) {
        const name = info.get_name();
        const rel = under ? `${under}/${name}` : name;
        if (info.get_file_type() === Gio.FileType.DIRECTORY)
            found.push(...sources(`${dir}/${name}`, rel));
        else if (name.endsWith('.ts') && !name.endsWith('.d.ts') && !name.endsWith('.test.ts'))
            found.push(rel);
    }
    return found;
}

let written = 0;
for (const rel of sources(options.rootDir).sort()) {
    const file = `${options.rootDir}/${rel}`;
    const out = ts.transpileModule(read(file), {
        compilerOptions: options,
        fileName: file,
        reportDiagnostics: true,
    });
    if (out.diagnostics.length > 0) {
        fail(out.diagnostics.map(d =>
            `${rel}: ${ts.flattenDiagnosticMessageText(d.messageText, '\n')}`).join('\n'));
    }
    const path = `${options.outDir}/${rel.replace(/\.ts$/, '.js')}`;
    GLib.mkdir_with_parents(GLib.path_get_dirname(path), 0o755);
    GLib.file_set_contents(path, out.outputText);
    written++;
}
print(`transpiled ${written} files with TypeScript ${ts.version}`);
