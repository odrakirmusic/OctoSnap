// SPDX-License-Identifier: GPL-3.0-or-later

//! The Quick Access Overlay's stylesheet.
//!
//! OctoSnap's own artwork and its own numbers. `spec/04` §1 is explicit that the card's
//! *structure* is what the reference established -- a bare thumbnail at rest, six controls
//! under a scrim on hover -- and that the expression is ours: "Reproduce the *structure*,
//! not the artwork."
//!
//! Kept as a string rather than a GResource because it is loaded once into a
//! `CssProvider` at display scope, and a resource bundle for one stylesheet would add a
//! build step to change a colour.

use octosnap_core::qao;

/// The card's corner radius, `spec/04` §1's "≈ 10 pt".
pub const RADIUS: i32 = 10;

/// Built rather than a literal, so the shadow can never be drawn wider than the
/// transparent band the window reserves for it. Getting that wrong does not look like a
/// CSS bug: the shadow is simply cut off along a straight line, which reads as a
/// rendering glitch in the compositor.
#[must_use]
pub fn sheet() -> String {
    // The shadow has to fit inside `SHADOW_MARGIN` on every side. A `box-shadow` with
    // blur `b` and vertical offset `dy` reaches `b + dy` below and `b - dy` above, so the
    // budget is `blur + |offset| <= SHADOW_MARGIN`.
    let blur = qao::SHADOW_MARGIN - 4;
    let offset = 4;
    debug_assert!(blur + offset <= qao::SHADOW_MARGIN);

    format!(
        "
/* The window itself paints nothing: the shadow band around the card has to be genuinely
   transparent, or the card would sit on a visible rectangle. */
window.octosnap-card,
window.octosnap-card > * {{
    background: none;
    background-color: transparent;
    box-shadow: none;
}}

.octosnap-card-body {{
    border-radius: {RADIUS}px;
    background-color: black;
    /* `spec/04` §1: 'a thin dark hairline, and a soft drop shadow'. The hairline keeps a
       pale screenshot from bleeding into a pale desktop. */
    box-shadow: 0 {offset}px {blur}px rgba(0, 0, 0, 0.45);
    outline: 1px solid rgba(0, 0, 0, 0.55);
    outline-offset: -1px;
}}

/* `spec/04` §3: 'Focus the card for keyboard shortcuts (no visible change beyond a
   subtle ring)'.

   An explicit class, not `:focus-within`. GTK gives every window's first focusable widget
   the focus as soon as it is built, whether or not that window is the active one, so
   `:focus-within` matched on *every* card at once -- a stack of six all wearing the accent
   ring, each announcing a keyboard focus only one of them could have. `:not(:backdrop)`
   was the obvious correction and does not work either: these windows are never activated
   in the ordinary way, and GTK never puts them in the backdrop state, so the ring stayed
   on. Measured both, on 2026-09-08.

   The app does not have to infer this. A card holds the keyboard exactly when the
   extension has lent it one (`FocusWindow`), which is a fact the overlay already knows
   and can simply state. `docs/decisions.md` D35. */
.octosnap-card-body.octosnap-focused {{
    outline: 2px solid alpha(@accent_color, 0.9);
}}

.octosnap-scrim {{
    border-radius: {RADIUS}px;
    background-color: rgba(0, 0, 0, 0.55);
    padding: 6px;
}}

/* The four corner controls. Round, small, and quiet until they are under the pointer --
   `spec/04` §1 puts them in corners precisely so they cannot be hit by accident. */
.octosnap-corner {{
    padding: 0;
    border-radius: 11px;
    background-color: rgba(0, 0, 0, 0.38);
    color: #ffffff;
    border: none;
    box-shadow: none;
}}

.octosnap-corner:hover {{
    background-color: rgba(255, 255, 255, 0.22);
}}

.octosnap-corner:active {{
    background-color: rgba(255, 255, 255, 0.32);
}}

.octosnap-corner.destructive:hover {{
    background-color: alpha(@destructive_color, 0.85);
}}

/* `spec/04` §1: 'the two actions people take constantly (copy, save) get large text
   targets in the middle where the pointer already is'. Text, not icons, and wide. */
.octosnap-pill {{
    min-width: 0;
    min-height: 0;
    padding: 0 10px;
    border-radius: 11px;
    font-size: 0.85em;
    background-color: rgba(255, 255, 255, 0.16);
    color: #ffffff;
    font-weight: 600;
    border: none;
    box-shadow: none;
}}

.octosnap-pill:hover {{
    background-color: rgba(255, 255, 255, 0.30);
}}

.octosnap-pill:active {{
    background-color: rgba(255, 255, 255, 0.40);
}}

.octosnap-pill.destructive:hover {{
    background-color: alpha(@destructive_color, 0.85);
}}

/* `spec/04` §6's 'thin progress hairline along the bottom edge', kept subtle enough to
   read as a countdown rather than as a loading bar. */
.octosnap-hairline {{
    background-color: rgba(255, 255, 255, 0.55);
    border-radius: 1px;
}}

/* `spec/04` §1: video cards carry 'a small badge reading the duration and file size
   together'. M5 fills it in; the style is here so the card is not restyled later. */
.octosnap-badge {{
    padding: 1px 6px;
    border-radius: 6px;
    background-color: rgba(0, 0, 0, 0.65);
    color: #ffffff;
    font-size: 0.8em;
}}

/* `spec/06` §3's controls pill: a small dark rounded bar with the timer, Stop and Trash,
   placed outside the recorded area. The window itself is transparent so only the bar
   shows. */
.octosnap-recorder {{
    background-color: transparent;
}}

.octosnap-recorder > box {{
    background-color: rgba(30, 30, 30, 0.92);
    border-radius: 12px;
    color: #ffffff;
    padding: 6px 10px;
}}

.octosnap-recorder button {{
    min-width: 24px;
    min-height: 24px;
    padding: 2px;
    border-radius: 8px;
    color: #ffffff;
}}

.octosnap-rec-dot {{
    color: #e01b24;
}}

.octosnap-rec-time {{
    font-feature-settings: \"tnum\";
    font-weight: bold;
}}

/* Stop is what the pill is for, so it is filled at rest where Discard is flat; Discard
   only turns red under the pointer. */
.octosnap-rec-stop {{
    background-color: alpha(#ffffff, 0.18);
}}

.octosnap-rec-stop:hover {{
    background-color: alpha(#ffffff, 0.3);
}}

.octosnap-rec-trash:hover {{
    background-color: alpha(@destructive_color, 0.85);
}}

/* `spec/07` §1.1's scrolling-capture pill: the preview strip, the size, the hint and the
   controls in one dark bar outside the selection (D74). Before Start the strip's place
   holds the two choices instead (D152). The room inside the dark shape is padding: it
   was margins until 2026-09-27, which are outside the background, so the strip and the
   buttons met the pill's edge. */
.octosnap-scroller {{
    background-color: transparent;
}}

.octosnap-scroller > box {{
    background-color: rgba(30, 30, 30, 0.92);
    border-radius: 12px;
    color: #ffffff;
    padding: 10px;
}}

.octosnap-scroller button {{
    min-width: 24px;
    min-height: 24px;
    padding: 2px 6px;
    border-radius: 8px;
    color: #ffffff;
}}

/* Start, and Done in its place: wide enough to be the one to press. */
.octosnap-scroller button.octosnap-scroll-primary {{
    min-width: 72px;
}}

.octosnap-scroll-title {{
    font-weight: bold;
}}

/* The directions as tiles, an arrow above a word, as the overlay's toolbar shows its
   modes. Room either side of the longest word, which met its lit tile's edges without. */
.octosnap-scroll-ways > toggle {{
    padding: 6px;
}}

/* Help's popover: a panel of the pill's width in the pill's own dark, under the pill or
   over it (D152). libadwaita draws a popover's contents from these two. Opaque, since it
   can open over the page. The padding is the room `GUIDE_FRAME` counts, with libadwaita's
   1px border. */
popover.octosnap-scroll-guide {{
    --popover-bg-color: rgb(30, 30, 30);
    --popover-fg-color: #ffffff;
}}

popover.octosnap-scroll-guide > contents {{
    padding: 12px;
    border-radius: 12px;
}}

.octosnap-scroll-preview {{
    background-color: rgba(0, 0, 0, 0.45);
    border-radius: 6px;
}}

.octosnap-scroll-size {{
    font-feature-settings: \"tnum\";
    font-weight: bold;
    font-size: 0.9em;
}}

.octosnap-scroll-long {{
    color: #f5c211;
}}

.octosnap-scroll-hint {{
    font-size: 0.85em;
    opacity: 0.75;
}}

.octosnap-scroll-cancel:hover {{
    background-color: alpha(@destructive_color, 0.85);
}}

/* `spec/07` §2.1's reading indicator (D96): the same dark pill as the other two, with no
   buttons on it, sitting over the area a text capture is still reading. */
.octosnap-reader {{
    background-color: transparent;
}}

.octosnap-reader > box {{
    background-color: rgba(30, 30, 30, 0.92);
    border-radius: 12px;
    color: #ffffff;
    padding: 8px 16px 8px 14px;
}}

.octosnap-reading {{
    font-size: 0.95em;
}}
"
    )
}
