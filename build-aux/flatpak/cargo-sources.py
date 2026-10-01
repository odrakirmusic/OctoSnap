#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Writes the Flatpak sources that vendor every crate in Cargo.lock.

    build-aux/flatpak/cargo-sources.py [Cargo.lock] [out.json]

flatpak-builder builds with the network off, and Flathub's builders do too, so cargo can
fetch nothing: every crate the lockfile names has to arrive as a source of the manifest,
unpacked where a vendored-sources directory expects it. This does what
flatpak-builder-tools' `flatpak-cargo-generator.py` does for a lockfile with crates.io
dependencies only, which is all this workspace has. It is written here rather than fetched
because it is sixty lines, and a generator that walks into a git dependency should stop
rather than guess (see `main`).

For each crate: the `.crate` archive from static.crates.io with the lockfile's own
checksum as its sha256, unpacked into `cargo/vendor/<name>-<version>`, and beside it the
`.cargo-checksum.json` a directory source requires. The package checksum is the one the
lockfile holds and the file list is empty, which tells cargo not to verify files one by
one. Last, `cargo/config.toml`, which replaces crates.io with that directory. The manifest
points `CARGO_HOME` at `cargo/`, so cargo reads it without being told.
"""

import json
import sys
import tomllib

CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
VENDOR = "cargo/vendor"

CONFIG = f"""[source.vendored-sources]
directory = "{VENDOR}"

[source.crates-io]
replace-with = "vendored-sources"
"""


def sources(lock: dict) -> list[dict]:
    out = []
    for package in lock.get("package", []):
        source = package.get("source")
        if source is None:
            # A workspace member: it is the checkout itself, not a download.
            continue
        if source != CRATES_IO:
            raise SystemExit(
                f"{package['name']} {package['version']} comes from {source}; only "
                "crates.io is handled here. Use flatpak-builder-tools' generator."
            )
        name, version, checksum = package["name"], package["version"], package["checksum"]
        dest = f"{VENDOR}/{name}-{version}"
        out.append({
            "type": "archive",
            "archive-type": "tar-gzip",
            "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
            "sha256": checksum,
            "dest": dest,
        })
        out.append({
            "type": "inline",
            "contents": json.dumps({"package": checksum, "files": {}}),
            "dest": dest,
            "dest-filename": ".cargo-checksum.json",
        })
    out.append({
        "type": "inline",
        "contents": CONFIG,
        "dest": "cargo",
        "dest-filename": "config.toml",
    })
    return out


def main() -> None:
    lock_path = sys.argv[1] if len(sys.argv) > 1 else "Cargo.lock"
    out_path = sys.argv[2] if len(sys.argv) > 2 else "build-aux/flatpak/cargo-sources.json"
    with open(lock_path, "rb") as f:
        lock = tomllib.load(f)
    result = sources(lock)
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(result, f, indent=4)
        f.write("\n")
    crates = sum(1 for s in result if s["type"] == "archive")
    print(f"wrote {out_path}: {crates} crates")


if __name__ == "__main__":
    main()
