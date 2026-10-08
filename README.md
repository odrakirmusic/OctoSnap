# OctoSnap

A screenshot, annotation and screen-recording tool for **GNOME on Wayland**, built to the standard of the best macOS capture tools rather than to the standard of what Linux currently has.

Take an area, a window or the whole screen with a key, and the capture lands in a small card in the corner, ready to copy, save, drag into another app, pin above your windows or open in the editor. The editor has arrows, shapes, text, counters, a spotlight, redaction, a highlighter, crop and backgrounds. OctoSnap also stitches scrolling pages into one image, reads the text in any part of the screen, records GIFs and keeps a history of what you captured. And if you would like company, pixel-art pets can live on your desktop, and they keep out of your captures.

How to use it is in the [user guide](docs/guide.md).

## How it compares

<!-- comparison:glance -->
OctoSnap set beside Gradia, Kooha, Flameshot, GNOME's own screenshot tool, Shutter and ksnip, read from each one's code: of the 353 capabilities found there, OctoSnap has **272** on GNOME Wayland, and the next, ksnip, has 168. Every mark links to the line of code it rests on; hover over one for what that tool does there.

| | OctoSnap | Gradia | Kooha | Flameshot | GNOME Shell | Shutter | ksnip |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| Capture an area | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/extension/src/extension.ts#L44 "Own overlay on every monitor. Capture Area (Ctrl+Alt+Super+A) shoots on mouse release (a click with no drag…") | [◐](https://github.com/AlexanderVanhee/Gradia/blob/3ac3d8170233/gradia/ui/image_loaders.py#L237 "No selector of its own: a libportal INTERACTIVE request opens GNOME Shell's screenshot UI…") | [◐](https://github.com/SeaDve/Kooha/blob/247bd2557953/data/resources/ui/window.ui#L67 "Recording only (video/GIF, no still). Selection mode: portal picks a monitor, then Kooha's own windowed…") | [✓](https://github.com/flameshot-org/flameshot/blob/3455978ec0b7/src/utils/screengrabber.cpp#L137 "Own full-screen overlay on one monitor, drawn over a portal grab (interactive=false). Drag a rectangle, then…") | [✓](https://github.com/GNOME/gnome-shell/blob/29380c2662da/js/ui/screenshot.js#L1604 "Own overlay, Selection mode is the default: drag, move or resize a rectangle, then confirm with Capture…") | [◐](https://github.com/shutter-project/shutter/blob/cb78390f546e/bin/shutter#L592 "Wayland: Selection button, menu or -s sends a portal Screenshot request (target 4 if the portal advertises…") | [◐](https://github.com/ksnip/ksnip/blob/df69d12a08f0/src/dependencyInjector/DependencyInjectorBootstrapper.cpp#L173 "Hand-off: Rectangular Area (Shift+R, -r) sends a portal Screenshot with interactive=true; GNOME's dialog does…") |
| Capture a window | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/extension/src/extension.ts#L46 "Own picker (Ctrl+Alt+Super+W, panel menu, All-In-One Window): hover highlight with title and size…") | [◐](https://github.com/AlexanderVanhee/Gradia/blob/3ac3d8170233/gradia/ui/image_loaders.py#L237 "Hand-off to the portal: GNOME Shell's screenshot UI Window mode (window thumbnails, captures the window's own…") | [◐](https://github.com/SeaDve/Kooha/blob/247bd2557953/src/experimental.rs#L38 "Recording only, behind KOOHA_EXPERIMENTAL=window-recording: WINDOW source type is added so the portal's…") | – | [✓](https://github.com/GNOME/gnome-shell/blob/29380c2662da/js/ui/screenshot.js#L1166 "Window mode (W): overview-style thumbnails per monitor of the active workspace's windows, click or arrow…") | [◐](https://github.com/shutter-project/shutter/blob/cb78390f546e/bin/shutter#L1000 "Wayland: Window button or -w sends a portal request (target 2 if advertised, else interactive); the portal's…") | [◐](https://github.com/ksnip/ksnip/blob/df69d12a08f0/src/backend/imageGrabber/WaylandImageGrabber.cpp#L30 "Window Under Cursor (Shift+U, -u) is listed on GNOME 41+ but is just the portal's interactive dialog; the…") |
| Own overlay with a pixel magnifier | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/extension/src/overlay/magnifier.ts#L47 "112 px loupe showing 7x7 logical px (16x logical, about 8x physical at 200 %) beside the pointer, flipping to…") | – | – | [◐](https://github.com/flameshot-org/flameshot/blob/3455978ec0b7/src/widgets/capture/magnifierwidget.h#L20 "Optional loupe: 17x17 px at 10x with a cross, round or square, off by default (Settings > General), hidden…") | – | [⊘](https://github.com/shutter-project/shutter/blob/cb78390f546e/share/shutter/resources/modules/Shutter/Screenshot/SelectorAdvanced.pm#L216 "Not on GNOME Wayland: Popup 105x105 px at 5x, nearest-neighbour, with pointer X/Y and the selection's W x H…") | [⊘](https://github.com/ksnip/ksnip/blob/df69d12a08f0/src/gui/snippingArea/AdornerMagnifyingGlass.cpp#L24 "Not on GNOME Wayland: Round loupe: 100x100 px scaled to 600x600, a 200 px circle cut from the centre (6x…") |
| Self-timer | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/extension/src/flow.ts#L234 "Select first, then a countdown over the live screen: 1-10 s in Settings (default 5), any whole seconds by CLI…") | [✓](https://github.com/AlexanderVanhee/Gradia/blob/3ac3d8170233/gradia/gradia.in#L67 "The wait is set on the command line only: `gradia --screenshot[=INTERACTIVE\|FULL] --delay=MS` (integer ms, no…") | [◐](https://github.com/SeaDve/Kooha/blob/247bd2557953/data/resources/ui/preferences_dialog.ui#L12 "Recording only. A 0-10 s delay (1 s steps) set in Preferences > Delay (Seconds), stored in GSettings key…") | [✓](https://github.com/flameshot-org/flameshot/blob/3455978ec0b7/src/main.cpp#L310 "CLI -d/--delay in ms for gui, screen and full; launcher spin box 0-999 s with a countdown label. The timer…") | – | [⊘](https://github.com/shutter-project/shutter/blob/cb78390f546e/bin/shutter#L1461 "Not on GNOME Wayland: Delay 0-99 s (Preferences, status bar, -d) runs after the area is picked, with optional…") | [✓](https://github.com/ksnip/ksnip/blob/df69d12a08f0/src/widgets/MainToolBar.cpp#L36 "Toolbar spin box 0-100 s, CLI -d seconds, per-action delay. A plain QTimer runs before the portal call, so…") |
| Scrolling capture | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/extension/src/flow.ts#L294 "The user scrolls (no auto-scroll). Frames from a Mutter ScreenCast live view at up to 30 fps, else per-frame…") | – | – | – | – | [–](https://github.com/shutter-project/shutter/blob/cb78390f546e/share/shutter/resources/modules/Shutter/Screenshot/Main.pm#L159 "In the code, but not reachable: Only a commented-out get_scrollable_from_drawable remains in Main.pm (xdotool…") | – |
| A card for each capture, to act on later | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/crates/app/src/main.rs#L270 "Quick Access card per screenshot or GIF, stacked at a screen edge, 5 sizes; hover Copy/Save(Trash) pills…") | – | – | – | [◐](https://github.com/GNOME/gnome-shell/blob/29380c2662da/js/ui/screenshot.js#L2745 "Only a notification banner with a thumbnail of the shot (click opens the file, 'Show in Files'); it times out…") | – | – |
| Annotation editor | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/crates/scene/src/tool.rs#L134 "Arrow tool (key A): drag from tail to tip; Shift snaps to 45-degree steps keeping the length; options colour…") | [✓](https://github.com/AlexanderVanhee/Gradia/blob/3ac3d8170233/gradia/backend/tool_config.py#L299 "Drag draws a straight arrow with an open two-line head at 30 degrees, 3x the width and shrunk on short…") | – | [✓](https://github.com/flameshot-org/flameshot/blob/3455978ec0b7/src/tools/arrow/arrowtool.cpp#L249 "Key A. Drag draws a straight shaft with a filled triangle head at the tip; head size grows with thickness…") | – | [✓](https://github.com/shutter-project/shutter/blob/cb78390f546e/share/shutter/resources/modules/Shutter/Draw/UIManager.pm#L385 "Alt+4 Arrow tool: drag draws a straight shaft with a head at the end point. Head width, length and tip length…") | [✓](https://github.com/ksnip/kImageAnnotator/blob/d184dbd77da3/src/widgets/ToolPicker.cpp#L60 "Arrow tool (key A): straight shaft with one filled kite head at the tip; head grows with width 1-20; colour…") |
| Pixelate or blur | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/crates/scene/src/tool.rs#L144 "Pixelate tool (B) drags a rectangle. Style menu Pixelate / Blur (secure) / Blur (smooth) / Black Out…") | [✓](https://github.com/AlexanderVanhee/Gradia/blob/3ac3d8170233/gradia/backend/tool_config.py#L342 "Censor tool (keys 8/c): drag a rectangle that is pixelated with 8 px blocks at the zoom it was drawn at; no…") | – | [✓](https://github.com/flameshot-org/flameshot/blob/3455978ec0b7/src/tools/pixelate/pixelatetool.cpp#L63 "Pixelate (B): drag a box and it is filled with coarse blocks; tool size 1-50 sets coarseness. Default blocks…") | – | [✓](https://github.com/shutter-project/shutter/blob/cb78390f546e/share/shutter/resources/modules/Shutter/Draw/UIManager.pm#L400 "Pixelize tool (Alt+Ctrl+8): drag a region; the picture under it is scaled to 10% and back up (about 10 px…") | [✓](https://github.com/ksnip/kImageAnnotator/blob/d184dbd77da3/src/widgets/ToolPicker.cpp#L101 "Pixelate (X): drag a box; the scene beneath is smooth-downscaled to 0.5/factor then upscaled with nearest…") |
| Spotlight | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/crates/scene/src/tool.rs#L341 "Spotlight tool (S): rectangle, rounded rectangle or ellipse drag; one mask layer cuts a hole per spotlight in…") | – | – | – | – | – | – |
| Backgrounds behind a screenshot | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/crates/app/src/editor/window.rs#L763 "Background sidebar (toolbar button or G): presets, None, gradients, wallpapers, blurred, plain colour…") | [✓](https://github.com/AlexanderVanhee/Gradia/blob/3ac3d8170233/gradia/ui/background_selector.py#L33 "The sidebar always carries a background chooser: None, Solid, Gradient or Image, with padding 0-75 %, corner…") | – | – | – | – | [◐](https://github.com/ksnip/kImageAnnotator/blob/d184dbd77da3/src/gui/canvasModifier/ModifyCanvasWidget.cpp#L91 "Modify Canvas is a dedicated editor mode that pads the screenshot by growing the canvas with typed X, Y, W…") |
| Editable project files | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/crates/app/src/editor/actions.rs#L1373 "Save as... with a .octosnap name writes a zip (manifest, base image, objects.json, assets, thumbnail)…") | – | – | – | – | – | – |
| Text recognition | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/crates/ocr/src/rapid/mod.rs#L79 "Own PaddleOCR PP-OCRv5 mobile pipeline (DBNet detect + CTC recognise, no angle classifier) in crates/ocr on…") | [✓](https://github.com/AlexanderVanhee/Gradia/blob/3ac3d8170233/gradia/backend/ocr.py#L136 "Tesseract 5.5.1 (built from git by Gradia's own Flatpak manifest) through pytesseract. Reached by the…") | – | – | – | – | [◐](https://github.com/ksnip/ksnip/blob/df69d12a08f0/src/plugins/interfaces/IPluginOcr.h#L32 "Options > OCR runs the open image through an IPluginOcr plugin and shows the text in a window. No engine or…") |
| QR codes | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/crates/ocr/src/qr.rs#L42 "QR only (rqrr), checked before OCR; payloads joined by newlines and copied; result window offers 'Open' when…") | – | – | – | – | – | – |
| Pin a capture above other windows | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/crates/app/src/pin/window.rs#L138 "Undecorated GTK window placed by the Shell extension, which calls make_above() and stick() (every workspace)…") | [◐](https://github.com/AlexanderVanhee/Gradia/blob/3ac3d8170233/gradia/ui/pin_window.py#L25 "A separate non-resizable Adw.ApplicationWindow per image file, with an always-visible OSD header bar (window…") | – | [◐](https://github.com/flameshot-org/flameshot/blob/3455978ec0b7/src/tools/pin/pinwidget.cpp#L38 "Pin is a separate borderless floating window hosted by the daemon, but keep-above is only…") | – | – | [◐](https://github.com/ksnip/ksnip/blob/df69d12a08f0/src/gui/modelessWindows/pinWindow/PinWindow.cpp#L31 "Pin (Options menu, Shift+P) opens the rendered editor image in a frameless parentless window with…") |
| Capture history | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/crates/app/src/qao/mod.rs#L621 "Every closed card (saved or not, unless 'keep saved' is off) is filed under…") | – | – | – | – | [✓](https://github.com/shutter-project/shutter/blob/cb78390f546e/bin/shutter#L6910 "Every capture joins the Session list; its file paths are written to ~/.shutter/session.xml on quit and…") | – |
| GIF recording | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/crates/media/src/encoder.rs#L161 "The recording is written as a looping GIF by gifski from the kept frames, on Save/Copy/editor or the…") | – | [✓](https://github.com/SeaDve/Kooha/blob/247bd2557953/data/resources/profiles.yml#L52 "'GIF' format in the same list: gifenc with infinite repeat written straight to a .gif, no audio; rates above…") | – | – | – | – |
| Trim a recording | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/crates/app/src/gif_editor.rs#L292 "Frame-accurate start/end marks via I/O keys, Set start/Set end buttons or filmstrip handles, Reset; the kept…") | – | – | – | – | – | – |
| Video recording | – | – | [✓](https://github.com/SeaDve/Kooha/blob/247bd2557953/data/resources/profiles.yml#L8 "WebM (VP8 + Opus, default), MP4 (x264 H.264 + MP3, fragmented) and Matroska (x264 H.264 + Opus) written…") | – | [✓](https://github.com/GNOME/gnome-shell/blob/29380c2662da/js/dbusServices/screencast/screencastService.js#L32 "Writes MP4 (H.264) or WebM (VP8) through a GStreamer cascade, pipewiresrc to filesink; container and codec…") | – | – |
| Sound in recordings | – | – | [✓](https://github.com/SeaDve/Kooha/blob/247bd2557953/data/resources/ui/window.ui#L93 "Main-window toggle (Ctrl+M, off by default) records the default input device via pulsesrc; no device picker.…") | – | – | – | – |
| Upload to an image host | – | [◐](https://github.com/AlexanderVanhee/Gradia/blob/3ac3d8170233/gradia/ui/preferences/provider_selection_window.py#L39 "Ready-made Imgur (anonymous API upload with Gradia's own client ID) and GoFile.io (anonymous, no account)…") | – | [–](https://github.com/flameshot-org/flameshot/blob/3455978ec0b7/CMakeLists.txt#L87 "In the code, but not reachable: Imgur only: toolbar 'Upload the selection' POSTs the PNG anonymously to…") | – | [✓](https://github.com/shutter-project/shutter/blob/cb78390f546e/share/shutter/resources/system/upload_plugins/upload/Imgur.pm#L46 "Imgur (anonymous with a bundled Client-ID, or OAuth PIN) and Gyazo. Imgur is hidden without Path::Class…") | [✓](https://github.com/ksnip/ksnip/blob/df69d12a08f0/src/backend/config/Config.cpp#L936 "Imgur is the built-in uploader and the default Uploader Type; anonymous with a built-in client ID (or own…") |
| Command line | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/crates/cli/src/main.rs#L135 "`octosnap capture --mode all-in-one\|area\|window\|fullscreen\|previous-area\|self-timer` (default area), with…") | [✓](https://github.com/AlexanderVanhee/Gradia/blob/3ac3d8170233/gradia/gradia.in#L79 "`gradia --screenshot[=INTERACTIVE\|FULL] [--delay=MS]`: portal call in the launcher; INTERACTIVE is GNOME…") | – | [✓](https://github.com/flameshot-org/flameshot/blob/3455978ec0b7/src/main.cpp#L287 "gui (overlay; --region WxH+X+Y preset, --last-region, -s accept on select), full (whole desktop), screen -n N…") | – | [◐](https://github.com/shutter-project/shutter/blob/cb78390f546e/share/shutter/resources/modules/Shutter/App/Options.pm#L53 "-f/-s/-w/-a (and -r in a running instance) end at the xdg portal; GNOME may show its own picker. -m/-t are…") | [◐](https://github.com/ksnip/ksnip/blob/df69d12a08f0/src/backend/commandLine/CommandLine.cpp#L54 "-r -l -f -m -a -u plus -d seconds and -c. On GNOME 41+ only -f is non-interactive; every other mode opens the…") |
| Global shortcuts of its own | [✓](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/extension/src/keybindings.ts#L33 "The GNOME Shell extension registers its capture keys with Main.wm.addKeybinding under Shell.ActionMode.ALL…") | – | – | [⊘](https://github.com/flameshot-org/flameshot/blob/3455978ec0b7/src/core/flameshot.cpp#L98 "Not on GNOME Wayland: Windows/macOS only: QHotkey for 'Capture screen' (Win Meta+Shift+X, mac Ctrl+Shift+X)…") | [✓](https://github.com/GNOME/gnome-shell/blob/29380c2662da/js/ui/screenshot.js#L1835 "The Shell binds four keys itself: Print (screenshot UI), Shift+Print (full screenshot), Alt+Print (focused…") | – | [⊘](https://github.com/ksnip/ksnip/blob/df69d12a08f0/src/gui/globalHotKeys/GlobalHotKeyHandler.cpp#L48 "Not on GNOME Wayland: Alt+Shift+R/L/F/C/A/U global keys grabbed with XGrabKey (X11) or a Windows handler. In…") |
| In other languages | [–](https://github.com/odrakirmusic/OctoSnap/blob/v0.1.7/extension/metadata.json#L8 "In the code, but not reachable: Extension metadata declares gettext domain 'octosnap', but no string is…") | [✓](https://github.com/AlexanderVanhee/Gradia/blob/3ac3d8170233/po/LINGUAS#L2 "gettext plus Blueprint strings; 21 languages built (po/LINGUAS); it.po exists but is not listed; kk, oc and…") | [✓](https://github.com/SeaDve/Kooha/blob/247bd2557953/src/main.rs#L57 "gettext (Rust gettext() plus translatable UI/schema/desktop/metainfo strings); 49 catalogues listed in…") | [✓](https://github.com/flameshot-org/flameshot/blob/3455978ec0b7/src/CMakeLists.txt#L117 "43 languages compiled (49 .ts files present; ar, ars, km, peo, sl, tk not listed). 'auto' follows the locale…") | [✓](https://github.com/GNOME/gnome-shell/blob/29380c2662da/po/meson.build#L1 "gettext via i18n.gettext; po/LINGUAS lists 96 languages, all with a .po. 62 of them translate 'Area…") | [✓](https://github.com/shutter-project/shutter/blob/cb78390f546e/share/shutter/resources/modules/Shutter/App/Common.pm#L67 "gettext (Locale::gettext, domain 'shutter', dir share/locale): po2mo.sh compiles 74 app, 59 plugin and 59…") | [✓](https://github.com/ksnip/ksnip/blob/df69d12a08f0/translations/CMakeLists.txt#L3 "35 ksnip Qt Linguist catalogues compiled and installed (39 .ts in the tree) plus 31 for kImageAnnotator; both…") |
| Also on Windows or macOS | – | – | – | [✓](https://github.com/flameshot-org/flameshot/blob/3455978ec0b7/src/utils/screengrabber.cpp#L309 "Windows: per-screen physical-pixel composite, MSI/ZIP via CPack, flameshot-cli.exe, QHotkey, ms-screenclip…") | – | – | [✓](https://github.com/ksnip/ksnip/blob/df69d12a08f0/src/backend/imageGrabber/WinImageGrabber.cpp#L26 "Windows: area, last area, full, active window, current screen, own overlay, global hotkeys, MSI/exe. macOS…") |

✓ has it on GNOME Wayland · ◐ in part, hover for which part · ⊘ in the code, but not on GNOME Wayland · – not in the code, or not reachable. Each mark links to its code.

The [full comparison](docs/comparison.md) has all 353, area by area, with what only OctoSnap does, where the others go further, and how the code was read.
<!-- /comparison:glance -->

## Installing

OctoSnap is two pieces, and it needs both: **the app**, and **its GNOME Shell extension**, which draws the capture overlay and reads the screen. It is made for GNOME 50 on Wayland; the extension also declares GNOME 48 and 49, which have not been tested.

Neither piece is on Flathub or extensions.gnome.org. Each [release](https://github.com/odrakirmusic/OctoSnap/releases) has both as downloads, and both can be built from this repository. The Flatpak carries the extension and installs it for you, so the extension's own download is for the native app, or for installing it by hand.

### The app, as a Flatpak

From a release:

```bash
flatpak install --user --bundle OctoSnap-0.1.7.flatpak
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
