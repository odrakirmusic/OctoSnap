// SPDX-License-Identifier: GPL-3.0-or-later

/**
 * The pet packs on this machine (`spec/14` §14, D196): found, read and handed to the pets,
 * and read again when the user's folder changes.
 *
 * Packs are in `octosnap/pets/<id>/` under the user's data folder and under each system
 * data folder, the user's first. Their files are read asynchronously, so a pack never holds
 * up a frame; `pack.ts` checks them and makes the pet. The user's folder and each pack in it
 * are watched, and a change is read a second after the last one, so a pack saved file by
 * file is read once. The system folders are read when the extension starts.
 *
 * The inflate a sheet needs is `Gio.ZlibDecompressor` over the sheet's bytes in memory: at
 * most a megabyte, and stopped at the size the image says it is.
 */

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import { error, info } from '../log.js';
import { setPackPets } from './art/index.js';
import { PACK_ID, PACK_LIMITS, type PackResult, readPack } from './pack.js';
import type { PackListing } from './packWire.js';

Gio._promisify(Gio.File.prototype, 'enumerate_children_async');
Gio._promisify(Gio.FileEnumerator.prototype, 'next_files_async');
Gio._promisify(Gio.File.prototype, 'query_info_async');
Gio._promisify(Gio.File.prototype, 'load_contents_async');

/** The most packs read, the first by id. */
const MAX_PACKS = 32;
/** How long after the last change in the user's folder it is read again. */
const SETTLE_MS = 1000;

/** `data`, a zlib stream, inflated, or a throw past `limit` bytes. */
export function gioInflate(data: Uint8Array, limit: number): Uint8Array {
    const converter = new Gio.ZlibDecompressor({ format: Gio.ZlibCompressorFormat.ZLIB });
    const input = Gio.MemoryInputStream.new_from_bytes(new GLib.Bytes(data));
    const stream = new Gio.ConverterInputStream({ base_stream: input, converter });
    const chunks: Uint8Array[] = [];
    let total = 0;
    try {
        for (;;) {
            const bytes = stream.read_bytes(64 * 1024, null);
            const size = bytes.get_size();
            if (size === 0) break;
            total += size;
            if (total > limit) throw new Error('more pixels than the image says it has');
            chunks.push(bytes.toArray());
        }
    } finally {
        stream.close(null);
    }
    const out = new Uint8Array(total);
    let at = 0;
    for (const chunk of chunks) {
        out.set(chunk, at);
        at += chunk.length;
    }
    return out;
}

/** The user's packs' folder. */
export function userPackFolder(): string {
    return GLib.build_filenamev([GLib.get_user_data_dir(), 'octosnap', 'pets']);
}

/** The folders packs are found in, the user's first. */
function roots(): string[] {
    const system = GLib.get_system_data_dirs().map(dir => GLib.build_filenamev([dir, 'octosnap', 'pets']));
    return [userPackFolder(), ...system.filter(dir => dir !== userPackFolder())];
}

/** The names of the folders in `path`, or none when it is not there. */
async function folders(path: string, cancellable: Gio.Cancellable): Promise<string[]> {
    const dir = Gio.File.new_for_path(path);
    let enumerator: Gio.FileEnumerator;
    try {
        enumerator = await dir.enumerate_children_async(
            'standard::name,standard::type',
            Gio.FileQueryInfoFlags.NONE,
            GLib.PRIORITY_LOW,
            cancellable,
        );
    } catch (e) {
        if (e instanceof GLib.Error && e.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.NOT_FOUND)) return [];
        throw e;
    }
    const names: string[] = [];
    try {
        for (;;) {
            const infos = await enumerator.next_files_async(64, GLib.PRIORITY_LOW, cancellable);
            if (infos.length === 0) break;
            for (const fileInfo of infos)
                if (fileInfo.get_file_type() === Gio.FileType.DIRECTORY) names.push(fileInfo.get_name());
            // Past any sane count; the rest are not read anyway.
            if (names.length > 4 * MAX_PACKS) break;
        }
    } finally {
        enumerator.close(null);
    }
    return names;
}

/** A file's bytes, refused before it is read when it is not a file or is bigger than `limit`. */
async function load(path: string, limit: number, cancellable: Gio.Cancellable): Promise<Uint8Array> {
    const file = Gio.File.new_for_path(path);
    const name = GLib.path_get_basename(path);
    let fileInfo: Gio.FileInfo;
    try {
        fileInfo = await file.query_info_async('standard::type,standard::size', Gio.FileQueryInfoFlags.NONE, GLib.PRIORITY_LOW, cancellable);
    } catch (e) {
        if (e instanceof GLib.Error && e.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.NOT_FOUND)) throw new Error(`it has no ${name}`);
        throw e;
    }
    if (fileInfo.get_file_type() !== Gio.FileType.REGULAR) throw new Error(`${name} is not a file`);
    if (fileInfo.get_size() > limit) throw new Error(`${name} is more than ${limit >= 1024 * 1024 ? `${limit / 1024 / 1024} MiB` : `${limit / 1024} KiB`}`);
    const [contents] = await file.load_contents_async(cancellable);
    return contents;
}

export class PetPacks {
    #listing: PackListing = { folder: userPackFolder(), pets: [], problems: [] };
    #ready = false;
    #cancellable: Gio.Cancellable | null = null;
    /** Each reading's number: one overtaken by a later one is thrown away. */
    #generation = 0;
    #settle = 0;
    #monitors = new Map<string, Gio.FileMonitor>();
    #listeners = new Set<() => void>();
    #running = false;

    /** Reads the packs, and from then on watches the user's folder. */
    start(): void {
        this.#running = true;
        void this.#read();
    }

    /** Stops watching, forgets every pack, and takes their pets away. */
    stop(): void {
        this.#running = false;
        this.#generation++;
        this.#cancellable?.cancel();
        this.#cancellable = null;
        if (this.#settle !== 0) GLib.source_remove(this.#settle);
        this.#settle = 0;
        for (const monitor of this.#monitors.values()) monitor.cancel();
        this.#monitors.clear();
        this.#listeners.clear();
        this.#listing = { folder: userPackFolder(), pets: [], problems: [] };
        this.#ready = false;
        setPackPets([]);
    }

    /** The packs as last read. */
    get listing(): PackListing {
        return this.#listing;
    }

    /** Whether the first reading is done, even with no packs at all. */
    get ready(): boolean {
        return this.#ready;
    }

    /** Calls `listener` after every reading; the function returned stops it. */
    onChange(listener: () => void): () => void {
        this.#listeners.add(listener);
        return () => this.#listeners.delete(listener);
    }

    /**
     * The user's folder, made if it is missing, opened in the file manager: Settings' Open
     * Pets Folder (`OpenPetFolder`).
     */
    openFolder(): void {
        const folder = userPackFolder();
        if (GLib.mkdir_with_parents(folder, 0o755) !== 0) throw new Error(`could not make ${folder}`);
        // Watched from now on, if it was not there before.
        this.#readSoon();
        Gio.AppInfo.launch_default_for_uri(GLib.filename_to_uri(folder, null), global.create_app_launch_context(0, -1));
    }

    #readSoon(): void {
        if (!this.#running) return;
        if (this.#settle !== 0) GLib.source_remove(this.#settle);
        this.#settle = GLib.timeout_add(GLib.PRIORITY_DEFAULT, SETTLE_MS, () => {
            this.#settle = 0;
            void this.#read();
            return GLib.SOURCE_REMOVE;
        });
    }

    async #read(): Promise<void> {
        const generation = ++this.#generation;
        this.#cancellable?.cancel();
        const cancellable = new Gio.Cancellable();
        this.#cancellable = cancellable;
        const listing: PackListing = { folder: userPackFolder(), pets: [], problems: [] };
        try {
            // Each id once, from the first folder that has it.
            const found = new Map<string, string>();
            for (const root of roots()) {
                for (const id of await folders(root, cancellable)) if (!found.has(id)) found.set(id, root);
            }
            const ids = [...found.keys()].sort();
            for (const [i, id] of ids.entries()) {
                const root = found.get(id) ?? '';
                const from = GLib.build_filenamev([root, id]);
                if (i >= MAX_PACKS) {
                    listing.problems.push({ id, from, problem: `only the first ${MAX_PACKS} packs are read, by name` });
                    continue;
                }
                if (!PACK_ID.test(id)) {
                    listing.problems.push({ id, from, problem: "the folder's name must be 1 to 32 lower-case letters, digits and hyphens" });
                    continue;
                }
                let result: PackResult;
                try {
                    const json = await load(GLib.build_filenamev([from, 'pet.json']), PACK_LIMITS.jsonBytes, cancellable);
                    const png = await load(GLib.build_filenamev([from, 'sheet.png']), PACK_LIMITS.sheetBytes, cancellable);
                    if (generation !== this.#generation) return;
                    result = readPack(id, new TextDecoder().decode(json), png, gioInflate);
                } catch (e) {
                    if (e instanceof GLib.Error && e.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED)) return;
                    result = { ok: false, problem: e instanceof Error ? e.message : String(e) };
                }
                if (result.ok) {
                    const { pack } = result;
                    listing.pets.push({ pack, from });
                    info(
                        `pet pack ${id} read from ${root}: ${pack.spec.name} the ${pack.spec.species}, ` +
                            `${pack.frames.length} frames, ${pack.colours} colours` +
                            (pack.blended > 0 ? `, ${pack.blended} pixels made clear or opaque` : ''),
                    );
                } else {
                    listing.problems.push({ id, from, problem: result.problem });
                    info(`pet pack ${id} left out: ${result.problem}`);
                }
            }
        } catch (e) {
            if (e instanceof GLib.Error && e.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED)) return;
            error('could not read the pet packs', e);
        }
        if (generation !== this.#generation || !this.#running) return;
        this.#cancellable = null;
        this.#listing = listing;
        this.#watch(listing);
        const first = !this.#ready;
        this.#ready = true;
        setPackPets(listing.pets.map(p => p.pack.spec));
        if (first || listing.pets.length > 0 || listing.problems.length > 0)
            info(`pet packs: ${listing.pets.length} read, ${listing.problems.length} left out`);
        for (const listener of [...this.#listeners]) {
            try {
                listener();
            } catch (e) {
                error('a pet packs listener failed', e);
            }
        }
    }

    /** Watches the user's folder and every pack in it; the system folders are not watched. */
    #watch(listing: PackListing): void {
        const folder = listing.folder;
        const wanted = new Set([folder]);
        for (const { from } of [...listing.pets, ...listing.problems])
            if (GLib.path_get_dirname(from) === folder) wanted.add(from);
        for (const [path, monitor] of this.#monitors) {
            if (wanted.has(path)) continue;
            monitor.cancel();
            this.#monitors.delete(path);
        }
        for (const path of wanted) {
            if (this.#monitors.has(path)) continue;
            try {
                // A missing folder is watched too: GLib notices when it is made.
                const monitor = Gio.File.new_for_path(path).monitor_directory(Gio.FileMonitorFlags.WATCH_MOVES, null);
                monitor.connect('changed', () => this.#readSoon());
                this.#monitors.set(path, monitor);
            } catch (e) {
                error(`could not watch ${path} for pet packs`, e);
            }
        }
    }
}
