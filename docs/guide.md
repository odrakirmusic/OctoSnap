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
12. [From a terminal](#from-a-terminal)
13. [Changing the shortcuts](#changing-the-shortcuts)
14. [Every shortcut](#every-shortcut)

## Starting a capture

### The menu in the top bar

OctoSnap's icon in the top bar opens a menu with every kind of capture: All-In-One,
Capture Area, Capture Previous Area, Capture Fullscreen, Capture Window, Self-Timer and
Record GIF. After them come History…, Hide Desktop Icons (shown only when the desktop has
icons), the Desktop Pets switch and Settings…. Each item shows its shortcut when it has one.
When a newer release of the Flatpak is out, **Update OctoSnap** comes before Settings… and
installs it, as Settings → General → Updates does. OctoSnap starts the new release once none
of its windows is open, and the next logout finishes the extension.

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

### The app's icon

Right-click OctoSnap's icon in the dash or the app grid for All-In-One, Capture Area,
Capture Fullscreen, Capture Window and History. History opens the strip, and leaves it open
when it is up already.

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

Files opens pictures in it too: OctoSnap is one of Open With's choices for PNG, JPEG, WebP
and GIF files. Choose several and each opens in an editor of its own, a GIF in the GIF
editor.

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
| M | Magnifier |
| C | Counter |
| B | Pixelate |
| X | Crop |
| Shift+X | Cut Out |
| G | Shows or hides the Background panel |
| F9 | Shows or hides the Layers panel |
| 1 to 6 | The size, for the tool and for what is selected |
| [ and ] | Less or more of what the options row sets by degrees: how strongly Pixelate hides, how dark Spotlight makes the rest, or how much the Magnifier enlarges |

### Drawing

| Key or gesture | What it does |
|---|---|
| Shift, while drawing an arrow or a line | Keeps it to 45° steps |
| Shift, while drawing a rectangle, an ellipse, a spotlight, a pixelation or a text box | Makes a square or a circle |
| Alt, while drawing those | Draws from the centre out |
| Shift, with the pencil or the highlighter | Draws a straight line |
| Ctrl, with the highlighter | Stops it keeping to the line of text |
| The pencil held still for half a second, the button still down | Makes the stroke the line, rectangle or ellipse it was meant to be, with that tool's handles. Anything else stays a stroke |
| A click, with Counter or Text | Places one |
| A drag, with Counter | Places a badge with a pointer to where you let go |
| A drag, with Magnifier | Puts the lens where you let go, joined to the spot you pressed on. A click puts it over the spot |

A drag too short to be meant is dropped. The eyedropper on the options row picks a colour
from anywhere on the screen, and Esc puts it away.

With a pen, the pencil's line is as wide as you press, from a quarter of its width at the
lightest touch to nearly twice it pressed hard. A mouse or a finger draws it one width.

### Pointers and badges

A badge or a label can point at something. Drag out of a badge as you place it, or
right-click a badge or a label and choose Add Pointer, then drag the tip: it is the ninth
handle. Remove Pointer takes it away.

Badges keep counting. Delete one and the ones after it close the gap. A new number, given
with a double-click or Edit Number in its menu, moves the badge in the count: give the 4
of four badges a 2, and the old 2 and 3 become 3 and 4. The counter's starting number moves
them all. Each is one step to undo.

### The magnifier

A magnifier is a lens showing a spot of the picture enlarged, joined to it by a line. Drag
from the spot to where the lens should be, or click to put the lens over the spot and drag
it off later. The options row sets how much it enlarges, 1.5 to 4 times. The lens shows the
picture as it is, redactions and all, and none of the marks. Drag the lens to move it, and
the spot's ring, or its handle, to choose what it shows.

### Cut Out

Shift+X, then drag down over the part of the picture to remove: the band across the picture
goes, and what was below it moves up to close the gap. Drag across instead to remove a band
down the picture. Marks on the band go with it. Ctrl+Z puts it all back.

### Text

Click for a box that grows with its text, or drag for one that wraps at the width you drag.
Enter finishes, Shift+Enter starts a new line, and Esc finishes too, keeping the text.
Double-click a piece of text to change it, and a counter to change its number. The
options row's alignment menu lines a label's lines up on the left, in the centre or on the
right.

### Redact Sensitive Info

**Redact Sensitive Info**, on the Pixelate tool's options row, or in the canvas menu with
nothing selected, reads the picture and covers what should not be shared: e-mail
addresses, phone numbers, IBANs and card numbers, IP addresses, keys and tokens, and
passwords, in a link or after a label such as `Password:`. Each is covered in the Pixelate
tool's style, all of them one step to undo, and they are left selected, so a wrong one is a
Delete away. Names, postal addresses and faces are not among them: look over the rest
yourself. A second press covers only what is not covered already.

### Text in the picture

**Select Text in the Picture**, the last button of the group at the top left, or
Ctrl+Shift+T, reads the picture and lets you select its words:

| Key or gesture | What it does |
|---|---|
| Drag across the words | Selects them, in reading order |
| Ctrl+A | Selects every word |
| Ctrl+C | Copies the words selected, or the picture when there are none |
| A click on a link or an e-mail address | Opens it. The canvas menu has Open Link or Write an E-mail |
| Esc | Lets go of the selection, and then of the words |

Only the words a saved copy shows take part. A word under a redaction, a filled box, a
pasted picture, a badge, a label with a box or a magnifier's lens, outside the crop or a
background's frame, or in what a spotlight at its darkest makes black, is not marked,
selected or copied, and a link under one does not open.

Any tool, a crop, the button or Ctrl+Shift+T again puts the words away. Both of these read
the picture on this computer, with the language pack [text
recognition](#text-recognition) uses. Without one, their message has a Language Packs
button, which opens Settings on the packs.

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

### Layers

F9, or the Layers button at the bottom, opens a list of every mark, the one on top first.

| Key or gesture | What it does |
|---|---|
| Click a row | Selects the mark. Ctrl+click adds one more, and Shift+click the rows between |
| Drag a row onto another | Moves the mark to that place |
| Alt+Up, Alt+Down | Moves the selected mark one place up or down |
| The eye | Hides the mark, or shows it again. A hidden mark is kept, and is in no export |
| The padlock | Locks the mark, or unlocks it. A locked mark cannot be selected, here or on the picture |

A mark moves only among its own kind: badges are always over shapes, and shapes over
spotlights and redactions. A redaction cannot be hidden, so what it covers never shows.

Dropping an image from another app into the middle of the picture adds it as an object.
Dropping it at one of the four edges places it beside the picture. On a background, the
background grows around both pictures.

### Crop

Press X, then drag the handles, move the rectangle, or drag a new one. Enter or the Crop
button applies it, and Esc, Cancel or another tool leaves it. The crop's own toolbar can
keep the proportions and snap to the picture's edges and to objects. A crop dragged past
the picture's edge makes room there, in the colour of the picture's border.

### Background

Press G for the Background panel, on the left: a gradient, a wallpaper, the picture
blurred or a plain colour behind the picture, with a margin, round corners and a shadow.
The background is always at the back. What you draw shows over it, on the margin as well
as on the picture. After a crop, the background goes around the crop, and around the room
a crop past the picture made too, with whatever you put there. None, at the top of the
panel, takes the background away again, margin, corners and shadow too, and gives the
picture back its own size.

The Presets menu saves a set of these by name. Presets → Default Preset chooses one for
every new screenshot. Its Add to New Screenshots item, like Settings → Annotate → Add the
default background, turns that off and on again. Shift as you confirm a capture leaves it
off that one.

### Looking around

| Key or gesture | What it does |
|---|---|
| Ctrl+0 | Fits the picture to the window |
| Ctrl+1 | 100 % |
| Ctrl+Plus, Ctrl+Minus | Zoom in, zoom out |
| Ctrl+scroll | Zooms in or out at the pointer |
| Scroll, Shift+scroll | Moves up and down, or sideways |
| Middle-drag, or Space held while dragging | Moves the view |
| Two fingers pinched, on a touchscreen or a touchpad | Zoom in or out between them |
| Two fingers moved together, on a touchscreen | Moves the view. A second finger stops what the first had started |
| F11 | Fullscreen |

### Finishing

| Key or button | What it does |
|---|---|
| Ctrl+S, Save | Saves it. The editor stays open |
| Ctrl+Shift+S, Save as… | Asks where to save it. A project (`.octosnap`) keeps the marks editable. An SVG or a PDF keeps them sharp at any size, over the picture, with the redactions part of the picture |
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

The Flatpak brings ONNX Runtime, the library the packs run on. The app built natively uses
the system's, and when it has none, a read says which package to install. **How to Install**
opens the README's list, and its way for a distribution with no package. The next read after the
install finds it.

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
| Typing, or Ctrl+F | Searches, by what the captures say (below) |
| Delete | Deletes the selected captures from the history, without asking. A file you saved elsewhere stays |
| Right-click | The menu: Restore, Annotate, Pin to the Screen, Copy, Save As… and Delete from History |
| The wheel | Scrolls the strip |
| Esc | Ends the search, and then closes the strip |
| The shortcut again | Closes it |

The chips at the top show only screenshots, videos, GIFs or projects. A capture is a
project when the editor closed on what its project file holds, and opening it from here
opens the project, its marks still editable. The strip's menu has Select All and Clear
History…, which asks first. Settings → Advanced → Capture history chooses how
long closed captures are kept, and whether saved ones stay in the history as well.

A search finds a capture by what it says, by its app and by its window's title: every word
you type has to be in one of them, whatever the case. The newest match is selected, so
Enter brings it back. OctoSnap reads each capture on this computer to know what it says, with
the language pack text recognition uses: a new one within seconds, and older ones while the
computer is idle and not saving power. Recordings and GIFs are not read. Settings → Advanced
→ Capture history → Find captures by their text turns the reading off, and deletes what it
read.

The Activities overview's search finds captures the same way. Choosing one brings it back
as a card, and choosing OctoSnap itself there opens the strip on the same search. GNOME's
Settings → Search chooses whether OctoSnap's results are shown there.

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

### Pets of your own

A pet of your own comes in a **pet pack**: a folder holding a `sheet.png` with the pet's
frames and a `pet.json` that says what the pet is and what each frame shows. Put the folder
in `~/.local/share/octosnap/pets/`, which **Open Pets Folder** in Settings → Pets opens,
making it the first time. OctoSnap reads a pack a second after it arrives or changes, and
its pet gets a card on the Pets page after the five. A pack it cannot read is listed under
**Your own pets**, with what to change. For everyone on a computer, an administrator can put
a pack in `/usr/local/share/octosnap/pets/` or `/usr/share/octosnap/pets/`, read when you
log in.

The folder's name is what OctoSnap calls the pet: up to 32 lower-case letters, digits and
hyphens. A pack is only pictures and words. It runs nothing, it cannot replace one of the
five, and updating OctoSnap leaves it where it is. Its pet does everything the five do, on
the floor, the pins and the desk, and is never in a capture.

[Mochi the cat](../data/pets/packs/mochi/pet.json) is a whole pack to learn from: its
`pet.json` is a model to copy, and [its sheet](../data/pets/packs/mochi/sheet.png) shows how
the cells are laid out. Mochi's pictures are GPL-3.0-or-later, as OctoSnap is, so a pet
drawn over them is a changed copy of Mochi; one drawn on an empty sheet takes nothing from
it.

**The sheet** holds the frames in cells of the pet's size, one image pixel to the pet's
pixel, numbered from 0 at the top left along each row and then down. A frame is 8 to 64
pixels each way; the sheet is at most 512 × 512 pixels, 1 MiB and 255 colours, and not
interlaced. A pixel is either solid or clear: one at least half opaque is solid, and any
other clear, so the pet stays sharp at every size and scale.

**pet.json** has `"format": 1`, and gives the pet its `name` and `species` ("Mochi" and
"cat" make "Mochi the cat"), its frame `size` as `[width, height]`, how it gets about (`gait`: `waddle`, `hop` or
`glide`), `floats` to fall slowly as Omni does, and a list of `frames`, each a `cell` of
the sheet and what it shows:

| Field | What the frame shows |
|---|---|
| (none) | The pet at rest. Every pack needs one |
| `mood` | Its face: `blink`, `half`, `closed`, `sleep`, `happy`, `squeeze`, `wide` or `dizzy` |
| `step` | Walking: `1` with its left foot up, `3` with its right |
| `arm` | `up`, `wave`, `wave2` or `flap` |
| `look`, `lookY` | Looking left (`-1`) or right (`1`), up (`-1`) or down (`1`) |
| `squash` | Squashed flat (`1`) or stretched tall (`-1`) |
| `view`, `face` | `"view": "top"` is the pet seen from above, on the desk, facing `down` (towards you), `up`, `left` or `right` |
| `name` | A frame only one of its own tricks shows |

Fields go together: `{ "cell": 9, "mood": "happy", "step": 1 }` is a happy pet with its left
foot up. For each moment OctoSnap shows the frame that matches the most and contradicts
nothing, and the nearest drawn one stands in for the rest, so a pack with a single frame
already walks, jumps and reacts, and each frame added makes it more lively. A missing
`left` facing is the `right` one mirrored. A pet that has no frame seen from above is drawn
from the side on the desk too, and one not drawn squashed is squashed by whole rows of
pixels.

`tricks` lists what it does by itself: `wave`, `flap`, `wiggle`, `roll` (with square
frames), `ink` (vanishing in a puff), or a trick of its own from `acts`, made of its frames:

```json
"tricks": ["groom", "wave"],
"acts": {
  "groom": {
    "times": [2, 4],
    "steps": [
      { "frame": "groom-1", "ms": 260 },
      { "frame": "groom-2", "ms": 220 }
    ]
  }
}
```

Each step holds a frame, by its `name`, or a `mood`, `arm`, `look` or `step`, for `ms`
milliseconds, and can `lift` the pet a few pixels off the ground or float up an `emote`:
`heart`, `note`, `spark`, `star`, `bang`, `what` or `zzz`. The steps play a number of times
between the two `times`, a little faster or slower each time. Mochi's
[pet.json](../data/pets/packs/mochi/pet.json) uses most of this.

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

## From a terminal

`octosnap` drives the app from a terminal or a script. In the Flatpak it is `flatpak run
io.github.odrakirmusic.OctoSnap`, followed by the same words. `octosnap --help` lists the
commands, and built natively each has a man page (`man octosnap-capture`) and completions
for bash, zsh and fish.

| Command | What it does |
|---|---|
| `octosnap capture` | Capture Area. `--mode` chooses another kind of capture, `--rect X,Y,W,H` takes that area without the overlay, and `--action` what happens to it |
| `octosnap capture -o shot.png` | Writes the capture to `shot.png`, and does nothing else with it. `-o -` writes it to standard output, for a pipe |
| `octosnap annotate FILE` | Opens a picture or a project in the editor. `pin FILE` pins it, and `add-overlay FILE` makes it a card. `-` for the file reads a picture from standard input |
| `octosnap text FILE` | Reads the text in a picture, and copies it |
| `octosnap record` | Records a GIF |

The Flatpak writes only into Pictures and Videos, so there `-o -` and the shell's `>` put
the picture anywhere else: `flatpak run io.github.odrakirmusic.OctoSnap capture -o - >
shot.png`.

Settings → Advanced → Allow the command line and octosnap:// links, when it is off, makes
`octosnap capture` and the links refuse. No menu has Quit: `gapplication action
io.github.odrakirmusic.OctoSnap quit` stops the app.

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

Ctrl+? in any of OctoSnap's windows lists every key it answers, with a search field, as do
Keyboard Shortcuts in the editor's canvas menu and the button on Settings → Shortcuts. The
shortcuts that work anywhere are these, in the groups Settings shows them in.

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
