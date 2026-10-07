# OctoSnap

A screenshot, annotation and screen-recording tool for **GNOME on Wayland**, built to the standard of the best macOS capture tools rather than to the standard of what Linux currently has.

Take an area, a window or the whole screen with a key, and the capture lands in a small card in the corner, ready to copy, save, drag into another app, pin above your windows or open in the editor. The editor has arrows, shapes, text, counters, a spotlight, redaction, a highlighter, crop and backgrounds. OctoSnap also stitches scrolling pages into one image, reads the text in any part of the screen, records GIFs and keeps a history of what you captured. And if you would like company, pixel-art pets can live on your desktop, and they keep out of your captures.

How to use it is in the [user guide](docs/guide.md).

## Installing

OctoSnap is two pieces, and it needs both: **the app**, and **its GNOME Shell extension**, which draws the capture overlay and reads the screen. It is made for GNOME 50 on Wayland; the extension also declares GNOME 48 and 49, which have not been tested.

Neither piece is on Flathub or extensions.gnome.org. Each [release](https://github.com/odrakirmusic/OctoSnap/releases) has both as downloads, and both can be built from this repository. The Flatpak carries the extension and installs it for you, so the extension's own download is for the native app, or for installing it by hand.

### The app, as a Flatpak

From a release:

```bash
flatpak install --user --bundle OctoSnap-0.1.5.flatpak
```

The bundle takes its runtime, GNOME 50, from Flathub, and it does not update itself: a new release is a new bundle. To build it instead:

```bash
flatpak-builder --user --install --force-clean _build/flatpak build-aux/flatpak/io.github.odrakirmusic.OctoSnap.json
```

This needs `flatpak-builder` and three things from Flathub: `org.gnome.Platform//50`, `org.gnome.Sdk//50` and `org.freedesktop.Sdk.Extension.rust-stable//25.08`. The Rust crates, ONNX Runtime, which text recognition runs on, and TypeScript, which compiles the extension the app carries, are sources of the manifest, so the build itself needs no network after they are fetched.

### The app, built natively

```bash
meson setup _build
```

```bash
meson compile -C _build
```

```bash
sudo meson install -C _build
```

This needs Rust 1.92 or newer, Meson 1.1 or newer, the GTK 4 and libadwaita 1.9 of GNOME 50, and GStreamer with its PipeWire plugin for recording. Text recognition needs ONNX Runtime 1.17 or newer from the system (Ubuntu's `libonnxruntime1.23`, for one), which the Flatpak brings with it. The text models are language packs, installed from Settings → Advanced. `-Dextension=true` also installs the extension system-wide, which is what a distribution package wants. `-Dbundle-extension=true` is the Flatpak's: the app carries the extension, and installs it itself.

### The extension

The Flatpak installs it from the welcome window ([The first run](#the-first-run)), and a later Flatpak updates it: the next login loads the new one. An update that changes more than the extension's code is written as you log out, since GNOME Shell reads the rest of it again at every unlock. For the native app, or by hand, from a release:

```bash
gnome-extensions install --force octosnap@odrakirmusic.github.io.shell-extension.zip
```

`--force` replaces what is already there: an older release, or the empty folder the Flatpak makes for its own install.

Or built from here:

```bash
cd extension && npm ci && ./build.sh && ./install-dev.sh
```

Then, either way, turn it on and **log out and back in**:

```bash
gnome-extensions enable octosnap@odrakirmusic.github.io
```

GNOME Shell loads an extension only if it was there when you logged in, and the same goes for every update of it. Where it loads none from your own folder, because the computer does not allow extensions installed by users (`allow-extension-installation`), the extension has to be installed for everyone: the zip's contents in `/usr/share/gnome-shell/extensions/octosnap@odrakirmusic.github.io`, or a native build with `-Dextension=true`.

`build-aux/ego/pack.sh` makes the zip that extensions.gnome.org takes, checked against its review rules.

### The first run

Open OctoSnap from the app grid. The welcome window checks the extension and says what is missing, if anything, with a button for the remedy where there is one: installing it, turning it on, or logging out to load it. The Flatpak's **Install** puts the extension it carries in your own extensions folder; log out and back in, and the next start turns it on. When all is well, it can take over the Print Screen keys from GNOME's own screenshot tool. After that, opening OctoSnap opens its settings, and the extension's menu in the top bar is where captures start.

## When something is wrong

- **Nothing happens on a shortcut, or Settings has a banner.** The banner and the welcome window both say which state the extension is in: not installed, turned off, installed after you logged in, not loaded by GNOME Shell, broken, or from another release than the app. The last two usually mean one half was updated without the other. One GNOME Shell does not load is in your own folder on a computer that does not allow extensions installed by users ([The extension](#the-extension)).
- **A report.** Settings → About → Troubleshooting → **Save a report** writes one file with the versions, the extension's state, the monitors and both halves' recent logs. Nothing is sent anywhere; attach the file to an issue yourself. **Log files** opens the folder with the app's own log and the report of its last crash, if it had one.
- **More detail.** Both halves keep their journal quiet until Settings → Advanced → Troubleshooting → **Debug logging** is on. Then the app's lines are under its own name, and the extension's are among GNOME Shell's, each starting with `[octosnap]`:

  ```bash
  journalctl --user -f -t octosnap-app
  ```

  ```bash
  journalctl --user -f -g octosnap
  ```

## Uninstalling

```bash
gnome-extensions uninstall octosnap@odrakirmusic.github.io
```

```bash
flatpak uninstall io.github.odrakirmusic.OctoSnap
```

A native install goes with `sudo ninja -C _build uninstall`. What OctoSnap kept is in `~/.local/share/octosnap` (the history), `~/.cache/octosnap` (captures not yet closed) and `~/.local/state/octosnap` (logs), or all under `~/.var/app/io.github.odrakirmusic.OctoSnap` for the Flatpak. Its settings are under `/org/octosnap/App/` and `/org/gnome/shell/extensions/octosnap/` in dconf.

## How it is built

On GNOME Wayland a normal application cannot read pixels, learn another window's geometry, place its own windows, stay above other windows, grab a global hotkey, or synthesise input. Every one of those is required for a capture tool that feels instant. GNOME's own screenshot UI has them because it *is* shell code.

So OctoSnap is two halves:

| Half | What it is | What it owns |
|---|---|---|
| `octosnap-shell` | A GNOME Shell extension, TypeScript compiled to GJS | Global hotkeys, the capture overlay, pixel capture, window geometry and hover, placing and raising the app's windows, pointer streaming, virtual scroll, clipboard, sounds |
| `octosnap` | A GTK4 + libadwaita application in Rust, running as a background service | The overlay cards, the annotation editor, the recorder, pinned screenshots, history, settings, OCR, notifications |

They speak over D-Bus. Images cross the boundary as files, never as byte arrays.

OctoSnap is written by Claude, Anthropic's AI coding agent, under its maintainer's direction. Each release's commit names the Claude models that wrote it.

## Building and testing

The app's tests and lints:

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

The extension's, from `extension/` after `npm ci`:

```bash
npx tsc --noEmit && npx vitest run
```

`meson test -C _build` runs the validators for the desktop entry, the metainfo and the settings schemas.

The code's comments cite the design notes it was built from, by number (D154) and by section (`spec/14` §3). Those notes are not part of this repository.

## Licence

OctoSnap is GPL-3.0-or-later (`COPYING`); every source file carries the SPDX header. One
dependency is under a different licence: the GIF encoder, the `gifski` crate, is
AGPL-3.0-or-later. GPLv3 §13 allows the combination, and the AGPL's network clause concerns a
program that users interact with over a network, which a desktop screenshot app is not. Binaries
built from this repository are therefore distributed under the GPL-3.0-or-later with that one
AGPL-3.0-or-later component. The icon, the tool icons and the sounds are
[CC-BY-SA-4.0](https://creativecommons.org/licenses/by-sa/4.0/). The pets are drawn by the
extension's code, so their pictures, the sheets in `data/pets` included, are GPL-3.0-or-later
like it.
