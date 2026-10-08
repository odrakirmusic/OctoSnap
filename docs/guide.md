# Using OctoSnap

This guide covers every way to take a capture, what can be done with one afterwards, and
the keys and clicks along the way. How to install OctoSnap, and what to do when something
goes wrong, is in the [README](../README.md).

Two things to know first:

- **The global shortcuts all use Ctrl+Alt+Super**, so that no application has them already.
  Nine are bound when OctoSnap is installed. The rest are Disabled until you give them a
  key in Settings → Shortcuts. OctoSnap can also take over the Print Screen keys.
- **A card or a pin takes keys only while the pointer is over it.** Pointing at one lends it
  the keyboard, and moving away gives it back, so a card never takes the keyboard from what
  you were typing into.

## Contents

1. [Starting a capture](#starting-a-capture)
2. [Choosing what to capture](#choosing-what-to-capture)
3. [The card](#the-card)
4. [The editor](#the-editor)
5. [Pinned screenshots](#pinned-screenshots)
6. [Scrolling capture](#scrolling-capture)
7. [Text recognition](#text-recognition)
8. [Recording a GIF](#recording-a-gif)
9. [The history](#the-history)
10. [Desktop pets](#desktop-pets)
11. [Without the mouse](#without-the-mouse)
12. [Changing the shortcuts](#changing-the-shortcuts)
13. [Every shortcut](#every-shortcut)

## Starting a capture

### The menu in the top bar

OctoSnap's icon in the top bar opens a menu with every kind of capture: All-In-One,
Capture Area, Capture Previous Area, Capture Fullscreen, Capture Window, Self-Timer and
Record GIF. After them come History…, Hide Desktop Icons (shown only when the desktop has
icons), the Desktop Pets switch and Settings…. Each item shows its shortcut when it has one.

While something is under way, the item that ends it comes first. That is **Cancel
Countdown** during a countdown, **Start Scrolling Capture** or **Finish Scrolling Capture**
while the scrolling pill is up, and **Stop Recording** during a recording, when the icon is
red and shows the time.

GNOME's Ctrl+Alt+Tab reaches the top bar from the keyboard. Settings → General → Shell
hides the icon, if you would rather use only the keys.

### The shortcuts

| Shortcut | What it does |
|---|---|
| Ctrl+Alt+Super+Space | **All-In-One**: the last area again, with a toolbar to adjust it and choose what to do |
| Ctrl+Alt+Super+A | **Capture Area**: drag over what you want, and it is taken when you let go |
| Ctrl+Alt+Super+W | **Capture Window**: choose a window |
| Ctrl+Alt+Super+F | **Capture Fullscreen**: the screen the pointer is on, at once |
| Ctrl+Alt+Super+R | **Capture Previous Area**: the last area again, at once |
| Ctrl+Alt+Super+T | **Self-Timer**: choose an area, and it is taken when a countdown ends |
| Ctrl+Alt+Super+Z | **Restore Recently Closed**: brings back the card you closed last |
| Ctrl+Alt+Super+H | **Open the Capture History**. Press it again to close it |
| Ctrl+Alt+Super+D | **Hide or Show Desktop Icons** |

[Every shortcut](#every-shortcut) lists the others, which include Scrolling Capture,
Capture Text and Record a GIF. A capture shortcut does nothing while another capture's
overlay or countdown is up.

### The Print Screen keys

GNOME's own screenshot tool has Print, Shift+Print and Alt+Print. Settings → Shortcuts →
System screenshot keys → **Use Them for OctoSnap** gives them to OctoSnap:

| Key | GNOME's | OctoSnap's |
|---|---|---|
| Print | The screenshot tool | All-In-One |
| Shift+Print | A shot of the whole screen | Capture Area |
| Alt+Print | A shot of the window | Capture Window |

These three keys then replace the Ctrl+Alt+Super keys of those three actions. **Give Them
Back to GNOME** undoes both halves. GNOME's screen recorder key, Ctrl+Shift+Alt+R, stays
GNOME's either way. The welcome window has the same button.

## Choosing what to capture

### Capture Area

Drag over what you want. It is taken the moment you let go.

| Key or gesture | What it does |
|---|---|
| Shift, while dragging | Keeps the proportions the selection has when you press it. Let go of Shift to change them again |
| Shift, held as the drag starts | Makes a square |
| Alt, while dragging | Draws from the centre out |
| Space, held while dragging | Moves the selection instead of sizing it. Let go to size it again |
| Ctrl, held | Shows the crosshair and magnifier (see below) |
| Enter | Takes the selection as it is, even halfway through a drag |
| Esc, a right-click, or a click without a drag | Cancels |

The same selection starts Self-Timer, Scrolling Capture, Capture Text and the "Capture Area
and …" shortcuts. OctoSnap's own cards leave the corner while you choose, and come back
when you have.

### All-In-One

All-In-One opens on the area you took last, with handles to adjust it and a toolbar under
it. Nothing is taken until you confirm, which makes it the place to get an area exactly
right, or to take the same one again with a change.

| Key or gesture | What it does |
|---|---|
| Drag outside the selection | Starts a new one |
| Drag inside the selection | Moves it |
| Drag a handle | Resizes it. Shift keeps the proportions, and Alt resizes it about its centre |
| Click the selection | Takes it. In Record mode it does not, so that a stray click cannot start a recording, and Enter starts instead |
| Click outside it, or right-click | Clears it. A right-click with nothing selected closes the overlay |
| Arrows, Shift+arrows | Move the selection by 1 or 10 pixels |
| Ctrl+arrows, Ctrl+Shift+arrows | Resize it by 1 or 10 pixels, from the bottom-right corner |
| Tab, Shift+Tab | The next or the previous mode on the toolbar |
| An arrow or Tab with nothing selected | Selects the middle of the screen the pointer is on, half its width and half its height, to start from |
| Enter | Confirms, in the mode the toolbar shows |
| Esc | Goes back one step: it closes an open list, leaves a size field, clears the selection, and then closes the overlay |

The toolbar:

| On the toolbar | What it does |
|---|---|
| Area | Takes the selection |
| Fullscreen | Takes the whole screen the pointer is on. Moving on to another mode without changing it gives your area back |
| Window | Opens the window chooser (see [Capture Window](#capture-window)) |
| Scrolling | Starts a [scrolling capture](#scrolling-capture) of the selection |
| Timer | Counts down, then takes the selection |
| Text | Reads the [text](#text-recognition) in the selection |
| Record | Records the selection as a [GIF](#recording-a-gif). The frame turns red, and the toolbar shows the recording's options |
| W and H | Type an exact size, in the pixels of the image it will make. The top-left corner stays put, and Enter sets the size without taking anything |
| The lock | Keeps the proportions the selection has now, on every drag |
| The ratio list | Free, 1:1, 4:3, 3:2, 16:9 or 16:10 |

Enter with nothing selected takes the whole screen in Fullscreen mode, opens the chooser in
Window mode and records the whole screen in Record mode. In the other modes it waits for a
selection.

The area All-In-One opens on, and the one Capture Previous Area takes, is the last one you
drew. A window or a whole screen taken from All-In-One does not replace it.

### Capture Window

Point at a window to highlight it, with its name, and click to take it. From the keyboard,
Tab and Shift+Tab go through the windows from the topmost down, and Enter takes the
highlighted one. The pointer takes over again as soon as it moves onto another window.
Esc, a right-click, or a click where there is no window cancels.

Settings → Screenshots → Window captures can put a background and padding behind every
window you take, and keep or leave off the window's shadow and rounded corners.

### Capture Fullscreen and Capture Previous Area

Capture Fullscreen takes the screen the pointer is on, with no overlay at all.

Capture Previous Area takes the last area again, also with no overlay, so a before and
after is two presses of one key. When there is no area to repeat yet, it opens a selection
instead.

### Self-Timer

Choose an area, and it is taken when the countdown ends. That leaves time to open a menu or
hover over something first: clicks and keys go to the windows underneath while it counts.

Esc cancels it. When Esc belongs to something else, the countdown says so, and the
top-bar menu's **Cancel Countdown** cancels it instead. The length is Settings →
Screenshots → Capture → Self-timer, 5 seconds unless you change it.

### Keys held as you confirm

Whatever confirms a capture, be it Enter, a click on the selection, the end of a drag or a
click on a window:

- **Ctrl** copies it to the clipboard, as well as whatever After Capture does.
- **Shift** leaves off any background that Settings would add to it.

### The crosshair and magnifier

Hold Ctrl while choosing an area, and a crosshair and a magnified view of the pixels under
the pointer appear, for lining up on one pixel. They never show over the toolbar. Settings
→ Screenshots → Capture → Crosshair and magnifier can show them always or never instead,
and Magnify pixels under the crosshair turns the magnified view on or off.

The same group chooses whether to include the pointer in captures, whether to freeze the
screen while you select so that video and animations hold still, and whether to hide the
desktop icons while capturing.

## The card

Each capture lands in a card at the edge of the screen, and is also copied to the
clipboard. Settings → General → After Capture chooses what happens instead, in any
combination: show the card, copy, save to the export location, open in the editor, pin to
the screen. Settings → Quick Access chooses where the cards go and how big they are.

Point at a card to see its buttons: Copy and Save in the middle, Close at the top left, Pin
at the top right and Annotate at the bottom left. A GIF's card has Trim for Annotate and no
Pin. Once the capture is saved, Save becomes Trash.

| Key or gesture | What it does |
|---|---|
| Ctrl+C | Copies it, and the card closes |
| Ctrl+Alt+C | Copies it, and the card stays. Holding Alt as you click Copy does the same |
| Ctrl+S | Saves it to the export location. With Ask where to save on, it asks first |
| Ctrl+E, or a double-click | Opens it in the [editor](#the-editor), or a GIF in the GIF editor |
| Space | Opens it in the desktop's image viewer, or a GIF in the GIF editor, playing |
| Delete | Moves it to the Trash. The notification has Undo |
| Esc, Ctrl+W, or a middle-click | Closes the card |
| Right-click | The menu: Annotate, Pin to the Screen, Copy Text, Save As…, Move to Trash, Copy and Close. Save As… always asks where |
| Drag it into an app | Drops the file there, and the card closes. Hold Alt as you drop to keep it |
| Scroll down over it | Tucks every card away until the next capture |

The keys can be turned off: Settings → Quick Access → Keyboard shortcuts on cards. The same
page has Close after dragging out, Ask where to save, and Close cards automatically, which
says Never until you choose a time. A card the pointer is over never closes on its own.

A save is a PNG unless Settings → Screenshots → File format says JPEG or WebP. JPEG makes
the smallest files, at the quality set below it, and puts white wherever the capture is
see-through, as a window's shadow is. WebP is lossless and keeps the transparency, in files
smaller than PNG's. Save As writes the format its name ends in, so `shot.webp` is a WebP
whatever the setting, and a name with no format in it gets the setting's. A capture taller
or wider than the format can hold, 65,535 pixels for JPEG and 16,384 for WebP, is saved as
a PNG.

A closed card is not gone. Restore Recently Closed brings back the last one, and the
[history](#the-history) keeps the rest.

## The editor

The editor opens from a card, from After Capture, from the history, or with the shortcuts
Annotate the Last Capture and Annotate the Clipboard's Image. The tools' keys work without
Ctrl, except while you are typing text.

Annotate the Clipboard's Image opens what you copied last, in whichever application you
copied it: a picture, or an image file copied in Files, which keeps its name. A GIF opens
in the GIF editor. Its shortcut has no key until you give it one in Settings, and
`octosnap open-from-clipboard` does the same from a terminal.

### Tools

| Key | Tool |
|---|---|
| V | Select |
| A | Arrow |
| L | Line |
| R | Rectangle. Shift+R draws a filled one |
| E | Ellipse |
| T | Text |
| P | Pencil |
| H | Highlighter |
| S | Spotlight |
| C | Counter |
| B | Pixelate |
| X | Crop |
| G | Shows or hides the Background panel |
| 1 to 6 | The size, for the tool and for what is selected |
| [ and ] | Less or more of the intensity on the options row: how strongly Pixelate hides, or how dark Spotlight makes the rest |

### Drawing

| Key or gesture | What it does |
|---|---|
| Shift, while drawing an arrow or a line | Keeps it to 45° steps |
| Shift, while drawing a rectangle, an ellipse, a spotlight, a pixelation or a text box | Makes a square or a circle |
| Alt, while drawing those | Draws from the centre out |
| Shift, with the pencil or the highlighter | Draws a straight line |
| Ctrl, with the highlighter | Stops it keeping to the line of text |
| A click, with Counter or Text | Places one |

A drag too short to be meant is dropped. The eyedropper on the options row picks a colour
from anywhere on the screen, and Esc puts it away.

### Text

Click for a box that grows with its text, or drag for one that wraps at the width you drag.
Enter finishes, Shift+Enter starts a new line, and Esc finishes too, keeping the text.
Double-click a piece of text to change it, and a counter to change its number. The
options row's alignment menu lines a label's lines up on the left, in the centre or on the
right.

### Selecting and arranging

| Key or gesture | What it does |
|---|---|
| Press on an object | Selects it and moves it, with any tool but Pencil and Highlighter |
| Drag a handle | Resizes it. Shift keeps the proportions |
| Shift+click | Adds it to the selection, or takes it out |
| Drag on the picture, with Select | Selects what the band touches. With Shift, adds it |
| Alt+drag an object | Moves a copy of it |
| Shift, while moving | Keeps the move to one direction |
| Arrows, Shift+arrows | Move the selection by 1 or 10 pixels |
| Ctrl+A | Selects everything |
| Ctrl+], Ctrl+[ | Bring to Front, Send to Back |
| Ctrl+D | Duplicates |
| Delete, Backspace | Deletes |
| Ctrl+C, Ctrl+X, Ctrl+V | Copy, cut and paste what is selected. With nothing selected, Ctrl+C copies the whole picture, and Ctrl+V pastes a picture from the clipboard |
| Ctrl+Z | Undo |
| Ctrl+Shift+Z, Ctrl+Y | Redo |
| Esc | Leaves what you are in the middle of: a crop, some text, the eyedropper, and then the selection. It never closes the editor |
| Right-click, Shift+F10, or the Menu key | The menu for what is selected, or for the object under the pointer |
| Right-click → Lock | Locks what is selected: a click goes through it to what is behind, until you right-click it and choose Unlock. Unlock All, with nothing selected, unlocks everything |

A picture you paste or drop onto the picture is picked up by its edge. Inside it, you draw
as you do anywhere else.

Dropping an image from another app into the middle of the picture adds it as an object.
Dropping it at one of the four edges places it beside the picture.

### Background

G opens the Background panel: a gradient, a wallpaper, a blur of the picture or a colour
behind it, with padding, shadow and corners. Its Presets menu saves a set of these by name.
Presets → Default Preset chooses one for every new screenshot. Its Add to New Screenshots
item, like Settings → Annotate → Add the default background, turns that off and on again.
Shift as you confirm a capture leaves it off that one.

### Crop

Press X, then drag the handles, move the rectangle, or drag a new one. Enter or the Crop
button applies it, and Esc, Cancel or another tool leaves it. The crop's own toolbar can
keep the proportions and snap to the picture's edges and to objects.

### Background

Press G for the Background panel, on the left: a gradient, a wallpaper, the picture
blurred or a plain colour behind the picture, with a margin, round corners and a shadow.
The background is always at the back. What you draw shows over it, on the margin as well
as on the picture. None, at the top of the panel, takes the background away again, margin,
corners and shadow too, and gives the picture back its own size.

### Looking around

| Key or gesture | What it does |
|---|---|
| Ctrl+0 | Fits the picture to the window |
| Ctrl+1 | 100 % |
| Ctrl+Plus, Ctrl+Minus | Zoom in, zoom out |
| Ctrl+scroll | Zooms in or out at the pointer |
| Scroll, Shift+scroll | Moves up and down, or sideways |
| Middle-drag, or Space held while dragging | Moves the view |
| F11 | Fullscreen |

### Finishing

| Key or button | What it does |
|---|---|
| Ctrl+S, Save | Saves it. The editor stays open |
| Ctrl+Shift+S, Save as… | Asks where to save it. A project (`.octosnap`) keeps the marks editable |
| Copy | Copies the picture. Its menu has Copy Text |
| Drag me | Drag the picture into another app |
| Ctrl+Shift+P, Pin | Pins it to the screen and closes the editor |
| Ctrl+Shift+Alt+P, or Alt and a click on Pin | Pins it and keeps the editor open |
| Ctrl+P | Prints it |
| Ctrl+W, or the window's × | Closes the editor. The capture goes back to its card |
| Ctrl+Shift+W, Final Close | Closes it for good: no card, and the capture goes to the history |

## Pinned screenshots

A pin keeps a capture above your windows, to copy from or compare against. Pin one from a
card, from the editor or from the history. After Capture's Pin to the screen pins every
capture where it was taken.

| Key or gesture | What it does |
|---|---|
| Drag | Moves it |
| Arrows, Shift+arrows | Move it by 1 or 10 pixels |
| Scroll, or [ and ] | Fainter or more solid, from 10 % to 100 % |
| Shift+scroll, or - and + | Smaller or larger, from 25 % to 400 % |
| 0 | Its actual size |
| Double-click | Switches between its actual size and fitting the screen |
| Ctrl+C, Ctrl+S | Copy, save |
| Ctrl+E | Opens it in the editor. The pin stays |
| Ctrl+L, or the lock | Locks it: clicks go through it to the window underneath, except on the unlock handle at its top right |
| Esc, Ctrl+W, or a middle-click | Closes it, and it becomes a card again |
| Drag me, at the bottom left | Drags the picture into another app. The pin closes and the capture goes to the history, unless you hold Alt as you drop |
| Right-click | The menu: Copy, Save, Annotate, Copy Text, Lock, Opacity, Actual Size, Close and Close All Pinned |

The keys work while the pointer is over the pin. A locked pin takes the pointer only on its
unlock handle, so to unlock it from the keyboard, point there and press Ctrl+L. Settings →
Screenshots → Pinned screenshots chooses rounded corners, a shadow and a thin border.

## Scrolling capture

A scrolling capture stitches a page that does not fit on the screen into one tall image.

1. Press the Scrolling Capture shortcut (give it a key first), or choose Scrolling in
   All-In-One.
2. Select the part of the window that scrolls. A pill appears beside it, and what you
   selected is outlined in blue.
3. If the content scrolls another way than down, choose Up, Right or Left on the pill.
4. Press **Start**, and scroll the content with the wheel or the touchpad. Each time it
   stops moving, the new part is added to what you have.
5. Press **Done**.

Sideways content scrolls with Shift and the wheel.

The pill never takes the keyboard, because the page needs it. So without the pointer, press
the Scrolling Capture shortcut again for Start and once more for Done. Or choose **Start
Scrolling Capture** and then **Finish Scrolling Capture** at the top of the top-bar menu.
The page's own keys scroll it too. The arrow keys work better than Page Down, which can move
the page further than the selection is tall and leave no overlap to join the parts on.

Before Start, × cancels. Once the capture runs, × turns red first, and a second press within
three seconds throws the capture away. The pill's Help button explains all of this in brief.

## Text recognition

**Capture Text** reads the text in an area and copies it, keeping the line breaks. **Capture
Text as One Line** joins each paragraph into one line. Both start out Disabled. Give them
keys in Settings → Shortcuts, or use All-In-One's Text mode. The Copy Text of a card, a pin
or the editor reads a capture you already have. A text capture makes no shutter sound and
does not fly to the corner, because no card comes for it: the sound is the one that says the
text was copied.

Reading needs a language pack, which OctoSnap downloads once, when you choose one (about
13 MB for the first). The first time you read text without one, Settings opens on the
language packs, with the pack for your language ready to install. Press Install (or Enter)
and the text you captured is read as soon as the pack is in. Settings says when it is
copied.

The notification that says the text was copied has **Show**, which opens the text in a
window:

| Key or gesture | What it does |
|---|---|
| Ctrl+F | Find |
| Enter, Shift+Enter | The next or the previous match |
| Ctrl+Shift+C, Copy | Copies all of it |
| Ctrl+C | Copies what is selected |
| A click on a link | Opens it |
| Esc | Closes the find bar, and then the window |

A QR code's address has an Open button. Settings → Advanced → Language packs installs and
removes the packs. Text recognition, above it, chooses the language and says whether to keep
line breaks and whether to detect links.

## Recording a GIF

1. Press the Record a GIF shortcut (give it a key first), choose Record GIF from the top-bar
   menu, or choose Record in All-In-One. The overlay is red.
2. Choose the area as in All-In-One, or press Enter with nothing selected to record the whole
   screen.
3. Enter starts a countdown, then the recording. The top bar's icon turns red and shows the
   time.
4. Stop it with Stop on the pill, with Stop Recording in the top-bar menu, or with the Stop
   the Recording shortcut. Discard asks before it throws the recording away.

The countdown is Settings → Recording → Countdown, 3 seconds unless you change it. The same
page, and the toolbar in Record mode, choose the frame rate, the largest width and the
quality.

The GIF lands in a card. Trim, Ctrl+E or Space opens it in the GIF editor:

| Key or gesture | What it does |
|---|---|
| Space | Play, pause |
| Left, Right | The previous or the next frame |
| Home, End | The start, the end |
| I | Starts the GIF at this frame |
| O | Ends the GIF after this frame |
| Drag on the filmstrip | Moves the start or end mark near the pointer, or scrubs |
| Ctrl+S, Ctrl+Shift+S, Ctrl+C | Save, Save as…, Copy |
| Esc, Ctrl+W | Closes the editor, and leaves a card with the trim in it |
| Ctrl+Shift+W, Final Close | Closes it for good: no card, and the GIF goes to the history |

A GIF is copied as its file, the way Files copies one, so that its frames go with it: paste
it into a chat, a web page or a folder and it arrives animated. A program that takes only
pictures, not files, has nothing to paste from it. A screenshot is copied as its picture.

## The history

**Open the Capture History** (Ctrl+Alt+Super+H), or History… in the top-bar menu, opens a
strip of what you have captured, with the newest selected.

| Key or gesture | What it does |
|---|---|
| Left, Right | The previous or the next capture |
| Click, Ctrl+click | Selects one, or adds it to the selection or takes it out |
| Ctrl+A | Selects everything |
| Enter, or Restore | Brings the selected captures back as cards |
| Double-click | Opens it in the editor |
| Ctrl+C | Copies it, when one is selected |
| Delete, Backspace | Deletes the selected captures from the history, without asking. A file you saved elsewhere stays |
| Right-click | The menu: Restore, Annotate, Pin to the Screen, Copy, Save As… and Delete from History |
| The wheel | Scrolls the strip |
| Esc, or the shortcut again | Closes it |

The chips at the top show only screenshots, videos, GIFs or projects. A capture is a
project when the editor closed on what its project file holds, and opening it from here
opens the project, its marks still editable. The strip's menu has Select All and Clear
History…, which asks first. Settings → Advanced → Capture history chooses how
long closed captures are kept, and whether saved ones stay in the history as well.

## Desktop pets

OctoSnap can keep pixel-art pets on your desktop: Omni the octopus, Pengu the penguin, Hops
the frog, Morel the mushroom and Spud the potato. They walk along the bottom of the screen,
sit on pinned screenshots and react to what you capture. Let them go anywhere and the rest
of the screen is their desk: a pet put down away from the bottom stays there, seen from
above, and wanders about it. No screenshot or recording ever has one in it.

Turn them on with the **Desktop Pets** switch in the top-bar menu, at the end of the welcome
window, or in Settings → Pets. **Hide or Show the Desktop Pets** is a shortcut you can give a
key.

| To | Do this |
|---|---|
| Move a pet | Press and hold it, and drag. Let go to drop it, or let go while moving to throw it. Allowed anywhere, a pet let go away from the bottom stays where you put it, and a thrown one slides |
| Pat a pet | Click it |
| Capture from a pet | Right-click it for its menu: All-In-One's modes, then History, Settings and Hide pets |
| Put them all away | Hide pets in a pet's menu, the Desktop Pets switch, or the shortcut |

In a pet's menu the arrows or Tab move between the buttons, Enter chooses and Esc closes.
A mode opens All-In-One on it. Capture Fullscreen captures the screen the pet is on.

They react to a screenshot, to the capture flying to its card (a frog it lands beside may
lick it), to each second of the self-timer, to text recognition, a scrolling capture, a
copy, a recording that stops and a capture that fails.

They keep out of the way:

- They hide in the Activities overview, behind system dialogs, on a screen with a
  fullscreen window, and while you choose what to capture.
- Before a recording starts, a pet in the area walks out of it, or pops out in a puff when
  there is no time. On the desk it may go up or down to get out.
- While another app shares or records your screen they hide, unless you turn off Settings →
  Pets → Hide while the screen is shared.

Settings → Pets chooses which pets come out and how big they are: six sizes, from Tiny to
Huge, each sharp at any display scale. Smol, between Tiny and Small, is drawn one and a half
screen pixels to the pixel of art where it has to be, as at 100 %: some of its pixels are
then a little wider than others. It also sets how lively they are, from Silent, which does
nothing until you do something to it, and Zen, which mostly sits and only now and then
does a trick, to Lively. And whether they move on their own or stay
where you put them, whether they keep to the bottom of the screen or can be anywhere on it,
and where a pet's menu opens: above it, below it or around it. They come back where you
left them after you log out. With GNOME's Reduce Animation on, they only blink and look
around between reactions.

## Without the mouse

Every capture can be taken from the keyboard:

| To capture | Press |
|---|---|
| An area | All-In-One. The arrows move the area and Ctrl+arrows resize it, and Enter takes it. Capture Previous Area takes the last one again |
| A window | Capture Window, then Tab to the window and Enter |
| The screen | Capture Fullscreen |
| After a countdown | All-In-One, Tab to Timer, Enter |
| A scrolling page | All-In-One, Tab to Scrolling, Enter. Then the Scrolling Capture shortcut, or the top-bar menu, for Start and Done |
| Text | All-In-One, Tab to Text, Enter |
| A GIF | Record a GIF, the arrows, Enter. Stop the Recording, or the top-bar menu, ends it |

With nothing selected, the first arrow or Tab in All-In-One starts from the middle of the
screen. The top-bar menu, which GNOME's Ctrl+Alt+Tab reaches, has every capture too.

## Changing the shortcuts

In Settings → Shortcuts, click an action's key, and press the new one:

- Esc cancels, and the old key stays.
- Backspace or Delete, or the button beside the key, removes it. The row then says Disabled.
- A key that OctoSnap or GNOME already uses is refused, and the row says what has it.
- Without Ctrl, Alt or Super, a key that types something is refused, and so are the keys
  that move around: the arrows, Home, End, Page Up, Page Down, Tab, Enter and Menu. Pressing
  one in any other app would take a capture instead. The function keys, Print and the media
  keys are fine on their own.

A change works at once, with nothing to restart.

## Every shortcut

In the groups Settings shows them in.

| Settings group | Action | Key |
|---|---|---|
| Screenshots | All-In-One | Ctrl+Alt+Super+Space |
| | Capture Area | Ctrl+Alt+Super+A |
| | Capture Window | Ctrl+Alt+Super+W |
| | Capture Fullscreen | Ctrl+Alt+Super+F |
| | Capture Previous Area | Ctrl+Alt+Super+R |
| | Self-Timer | Ctrl+Alt+Super+T |
| Capture Area and … | … Copy to Clipboard | Disabled |
| | … Save | Disabled |
| | … Annotate | Disabled |
| | … Pin to the Screen | Disabled |
| Recording and Scrolling | Record a GIF | Disabled |
| | Stop the Recording | Disabled |
| | Scrolling Capture | Disabled |
| Text Recognition | Capture Text | Disabled |
| | Capture Text as One Line | Disabled |
| Overlays and Pins | Hide or Show the Cards | Disabled |
| | Save All Cards | Disabled |
| | Close All Cards | Disabled |
| | Restore Recently Closed | Ctrl+Alt+Super+Z |
| | Hide or Show Pinned Screenshots | Disabled |
| | Close All Pinned Screenshots | Disabled |
| The Last Capture | Copy the Last Capture | Disabled |
| | Save the Last Capture | Disabled |
| | Annotate the Last Capture | Disabled |
| | Annotate the Clipboard's Image | Disabled |
| Other | Open the Capture History | Ctrl+Alt+Super+H |
| | Hide or Show Desktop Icons | Ctrl+Alt+Super+D |
| | Hide or Show the Desktop Pets | Disabled |
| | Open OctoSnap Settings | Disabled |

A "Capture Area and …" shortcut does only what its name says, in place of After Capture.
Turn on Settings → Screenshots → Capture → "Capture Area and …" shortcuts add to the After
Capture actions, and it does both.

With the scrolling pill up, the Scrolling Capture shortcut presses the pill's Start, and
then Done.
