// SPDX-License-Identifier: GPL-3.0-or-later

//! Shortcuts as GSettings stores them: `SYS-04`, `spec/08` §3.
//!
//! Two questions the Preferences dialog has to answer without GTK's help. **Is this
//! accelerator already taken?** -- by another of our own actions, or by GNOME in
//! `org.gnome.shell.keybindings`, `org.gnome.desktop.wm.keybindings` or the media keys,
//! which is what `spec/08` §3's "conflict detection" means. And **what does "Use OctoSnap
//! for the system screenshot keys" write**, so that the same table gives the keys back.
//!
//! Accelerators are compared as *structure*, not text. `<Shift><Super>4` and
//! `<Super><Shift>4` are one binding; `<Primary>a` and `<Control>a` are one binding; GTK's
//! own `gtk_accelerator_name` writes yet another spelling. Anything that compared the
//! strings would miss half the conflicts it exists to find.

use std::collections::BTreeSet;

/// A modifier, in the order `gtk_accelerator_name` writes them -- which is also the
/// canonical order here, so a canonical string is one GTK would have produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Modifier {
    Shift,
    Control,
    Alt,
    Meta,
    Super,
    Hyper,
}

impl Modifier {
    /// Every spelling GSettings files contain. `Primary` is GTK's platform-neutral name
    /// for Control on this platform; `Mod1` and `Mod4` are X11's names for Alt and Super,
    /// and GNOME's own schemas still use them in places.
    fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "shift" => Some(Self::Shift),
            "control" | "ctrl" | "primary" => Some(Self::Control),
            "alt" | "mod1" => Some(Self::Alt),
            "meta" => Some(Self::Meta),
            "super" | "mod4" => Some(Self::Super),
            "hyper" => Some(Self::Hyper),
            _ => None,
        }
    }

    /// The `<Name>` GSettings spelling.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Shift => "<Shift>",
            Self::Control => "<Control>",
            Self::Alt => "<Alt>",
            Self::Meta => "<Meta>",
            Self::Super => "<Super>",
            Self::Hyper => "<Hyper>",
        }
    }

    /// The word on the keycap, as GNOME Settings shows it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Shift => "Shift",
            Self::Control => "Ctrl",
            Self::Alt => "Alt",
            Self::Meta => "Meta",
            Self::Super => "Super",
            Self::Hyper => "Hyper",
        }
    }
}

/// One accelerator, parsed: a set of modifiers and a key name.
///
/// The key keeps GDK's spelling except that a single letter is lower-cased, which is how
/// `gtk_accelerator_name` writes letters and how the schemas mostly do; every other key
/// name (`Print`, `F5`, `Page_Up`) compares case-insensitively.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Accelerator {
    modifiers: BTreeSet<Modifier>,
    key: String,
}

impl Accelerator {
    /// Parses `<Super><Shift>4`. `None` for an empty string, a missing key, or a
    /// modifier this module has never heard of -- an accelerator it cannot read is one it
    /// must not claim to have compared.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let mut modifiers = BTreeSet::new();
        let mut rest = text;
        while let Some(after) = rest.strip_prefix('<') {
            let (name, tail) = after.split_once('>')?;
            modifiers.insert(Modifier::parse(name)?);
            rest = tail;
        }
        if rest.is_empty() || rest.contains('<') || rest.contains('>') || rest.contains(char::is_whitespace) {
            return None;
        }
        let key = if rest.chars().count() == 1 { rest.to_lowercase() } else { rest.to_owned() };
        Some(Self { modifiers, key })
    }

    /// The one spelling: modifiers in GTK's order, then the key.
    #[must_use]
    pub fn canonical(&self) -> String {
        let mut out = String::new();
        for modifier in &self.modifiers {
            out.push_str(modifier.name());
        }
        out.push_str(&self.key);
        out
    }

    /// `Ctrl+Shift+4`, the way GNOME Settings writes a shortcut for a person.
    #[must_use]
    pub fn label(&self) -> String {
        let mut parts: Vec<String> = self.modifiers.iter().map(|m| m.label().to_owned()).collect();
        parts.push(key_label(&self.key));
        parts.join("+")
    }

    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn modifiers(&self) -> impl Iterator<Item = Modifier> + '_ {
        self.modifiers.iter().copied()
    }

    /// Structural equality with case-insensitive key names, so `<Alt>Print` and
    /// `<Mod1>print` are the same binding.
    #[must_use]
    pub fn is(&self, other: &Self) -> bool {
        self.modifiers == other.modifiers && self.key.eq_ignore_ascii_case(&other.key)
    }
}

/// Whether two accelerator strings name one binding. Unreadable strings never match.
#[must_use]
pub fn same(a: &str, b: &str) -> bool {
    match (Accelerator::parse(a), Accelerator::parse(b)) {
        (Some(a), Some(b)) => a.is(&b),
        _ => false,
    }
}

/// A key name as a keycap reads it.
fn key_label(key: &str) -> String {
    match key.to_ascii_lowercase().as_str() {
        "space" => "Space".to_owned(),
        "return" | "kp_enter" => "Enter".to_owned(),
        "escape" => "Esc".to_owned(),
        "print" => "Print".to_owned(),
        "backspace" => "Backspace".to_owned(),
        "delete" => "Delete".to_owned(),
        "tab" => "Tab".to_owned(),
        "up" => "\u{2191}".to_owned(),
        "down" => "\u{2193}".to_owned(),
        "left" => "\u{2190}".to_owned(),
        "right" => "\u{2192}".to_owned(),
        "page_up" => "Page Up".to_owned(),
        "page_down" => "Page Down".to_owned(),
        "home" => "Home".to_owned(),
        "end" => "End".to_owned(),
        "insert" => "Insert".to_owned(),
        _ if key.chars().count() == 1 => key.to_uppercase(),
        _ => key.replace('_', " "),
    }
}

/// Somebody's binding: one key of one schema and the accelerators it holds. The app
/// collects these from GNOME's schemas and from ours; this module only compares them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub schema: String,
    pub key: String,
    pub accelerators: Vec<String>,
}

impl Binding {
    #[must_use]
    pub fn new(schema: &str, key: &str, accelerators: &[&str]) -> Self {
        Self {
            schema: schema.to_owned(),
            key: key.to_owned(),
            accelerators: accelerators.iter().map(|a| (*a).to_owned()).collect(),
        }
    }

    /// Whether this binding holds `candidate`.
    #[must_use]
    pub fn holds(&self, candidate: &Accelerator) -> bool {
        self.accelerators
            .iter()
            .filter_map(|text| Accelerator::parse(text))
            .any(|held| held.is(candidate))
    }

    /// `switch-applications` as "Switch applications": what the recorder shows beside
    /// "already used by". GNOME's own keys have no user-facing names outside GNOME
    /// Settings' translations, so the key's own words are the honest label.
    #[must_use]
    pub fn label(&self) -> String {
        humanize(&self.key)
    }

    /// Which desktop component the schema belongs to, for the same message.
    #[must_use]
    pub fn owner(&self) -> &'static str {
        match self.schema.as_str() {
            GNOME_SHELL_KEYBINDINGS => "GNOME Shell",
            WM_KEYBINDINGS => "the window manager",
            s if s.starts_with(MEDIA_KEYS) => "GNOME's media keys",
            _ => "OctoSnap",
        }
    }
}

/// `switch-applications` → `Switch applications`.
#[must_use]
pub fn humanize(key: &str) -> String {
    let words = key.replace(['-', '_'], " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The bindings other than our own `own_key` that already hold `candidate`.
///
/// Our other keys count -- two OctoSnap actions on one key would fire one of them and
/// look like the other is broken -- and the key being recorded does not, or every
/// re-record of the same accelerator would report a conflict with itself.
#[must_use]
pub fn conflicts<'a>(
    candidate: &str,
    own_schema: &str,
    own_key: &str,
    known: &'a [Binding],
) -> Vec<&'a Binding> {
    let Some(candidate) = Accelerator::parse(candidate) else { return Vec::new() };
    known
        .iter()
        .filter(|binding| !(binding.schema == own_schema && binding.key == own_key))
        .filter(|binding| binding.holds(&candidate))
        .collect()
}

pub const GNOME_SHELL_KEYBINDINGS: &str = "org.gnome.shell.keybindings";
pub const WM_KEYBINDINGS: &str = "org.gnome.desktop.wm.keybindings";
pub const MEDIA_KEYS: &str = "org.gnome.settings-daemon.plugins.media-keys";

/// One GSettings write the onboarding step makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    /// Store these accelerators (an empty list unbinds the key).
    Set { schema: &'static str, key: &'static str, accelerators: Vec<&'static str> },
    /// Back to the schema's own default.
    Reset { schema: &'static str, key: &'static str },
}

/// `spec/08` §3's onboarding: GNOME's key, the OctoSnap action that takes it over, and
/// the accelerator both mean. GNOME 42+ binds `Print` to its screenshot UI,
/// `<Shift>Print` to a full-screen shot and `<Alt>Print` to a window shot;
/// `<Ctrl><Shift><Alt>R` (the recording UI) stays GNOME's until M5 has something to
/// offer for it.
pub const PRINT_KEYS: [(&str, &str, &str); 3] = [
    ("show-screenshot-ui", "all-in-one", "Print"),
    ("screenshot", "capture-area", "<Shift>Print"),
    ("screenshot-window", "capture-window", "<Alt>Print"),
];

/// "Use OctoSnap for the system screenshot keys": GNOME's three keys are unbound and
/// ours take their accelerators. GNOME's first, so there is no instant in which two
/// bindings hold one key and the shell refuses ours.
#[must_use]
pub fn take_over_print_keys(our_schema: &'static str) -> Vec<Write> {
    let mut writes: Vec<Write> = PRINT_KEYS
        .iter()
        .map(|(gnome, _, _)| Write::Set { schema: GNOME_SHELL_KEYBINDINGS, key: gnome, accelerators: Vec::new() })
        .collect();
    writes.extend(PRINT_KEYS.iter().map(|(_, ours, accel)| Write::Set {
        schema: our_schema,
        key: ours,
        accelerators: vec![*accel],
    }));
    writes
}

/// The exact reverse: ours back to their defaults first, so the Print keys are free
/// when GNOME's come back, then GNOME's three reset to what GNOME ships.
#[must_use]
pub fn give_back_print_keys(our_schema: &'static str) -> Vec<Write> {
    let mut writes: Vec<Write> = PRINT_KEYS
        .iter()
        .map(|(_, ours, _)| Write::Reset { schema: our_schema, key: ours })
        .collect();
    writes.extend(
        PRINT_KEYS.iter().map(|(gnome, _, _)| Write::Reset { schema: GNOME_SHELL_KEYBINDINGS, key: gnome }),
    );
    writes
}

/// Whether the take-over is in force: every OctoSnap action holds its Print key and no
/// GNOME key still does. The dialog reads this to show which of the two buttons applies.
pub fn print_keys_taken(
    ours: impl Fn(&str) -> Vec<String>,
    gnome: impl Fn(&str) -> Vec<String>,
) -> bool {
    PRINT_KEYS.iter().all(|(gnome_key, our_key, accel)| {
        let held = ours(our_key).iter().any(|a| same(a, accel));
        let free = !gnome(gnome_key).iter().any(|a| same(a, accel));
        held && free
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn spellings_of_one_binding_parse_to_one_structure() {
        let a = Accelerator::parse("<Super><Shift>4").expect("parses");
        let b = Accelerator::parse("<Shift><Super>4").expect("parses");
        assert_eq!(a, b);
        assert_eq!(a.canonical(), "<Shift><Super>4");
        assert!(same("<Primary>a", "<Control>A"));
        assert!(same("<Mod1>F4", "<Alt>F4"));
        assert!(same("<Mod4>Print", "<Super>print"));
        assert!(!same("<Control>a", "<Control><Shift>a"));
        assert!(!same("<Control>a", "<Control>b"));
    }

    #[test]
    fn what_cannot_be_read_is_not_an_accelerator() {
        assert_eq!(Accelerator::parse(""), None);
        assert_eq!(Accelerator::parse("<Control>"), None, "a modifier with no key");
        assert_eq!(Accelerator::parse("<Bogus>a"), None, "an unknown modifier");
        assert_eq!(Accelerator::parse("<Control a"), None, "an unclosed modifier");
        assert_eq!(Accelerator::parse("<Control>a b"), None, "two keys");
        assert!(!same("", ""), "two empty strings are no binding, not one");
        assert!(!same("<Control>a", ""));
    }

    #[test]
    fn a_bare_key_and_a_letter_normalise() {
        let print = Accelerator::parse("Print").expect("parses");
        assert!(print.modifiers().next().is_none());
        assert_eq!(print.key(), "Print");
        assert_eq!(Accelerator::parse("<Control>Q").expect("parses").canonical(), "<Control>q");
        assert_eq!(Accelerator::parse("<Super>Page_Up").expect("parses").key(), "Page_Up");
    }

    #[test]
    fn labels_read_like_gnome_settings() {
        let label = |text: &str| Accelerator::parse(text).expect("parses").label();
        assert_eq!(label("<Control><Shift>4"), "Shift+Ctrl+4");
        assert_eq!(label("<Super>Print"), "Super+Print");
        assert_eq!(label("<Alt>Return"), "Alt+Enter");
        assert_eq!(label("<Control>a"), "Ctrl+A");
        assert_eq!(label("<Super>Page_Up"), "Super+Page Up");
        assert_eq!(label("<Shift>Up"), "Shift+\u{2191}");
        assert_eq!(label("F5"), "F5");
    }

    fn known() -> Vec<Binding> {
        vec![
            Binding::new(GNOME_SHELL_KEYBINDINGS, "screenshot-window", &["<Alt>Print"]),
            Binding::new(GNOME_SHELL_KEYBINDINGS, "show-screenshot-ui", &["Print"]),
            Binding::new(WM_KEYBINDINGS, "switch-applications", &["<Super>Tab", "<Alt>Tab"]),
            Binding::new("org.gnome.shell.extensions.octosnap", "capture-area", &["<Super><Shift>4"]),
            Binding::new("org.gnome.shell.extensions.octosnap", "capture-window", &["<Super><Shift>5"]),
        ]
    }

    #[test]
    fn a_conflict_is_found_across_schemas_and_spellings() {
        let known = known();
        let found = conflicts("<Mod1>print", "org.gnome.shell.extensions.octosnap", "all-in-one", &known);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].key, "screenshot-window");
        assert_eq!(found[0].label(), "Screenshot window");
        assert_eq!(found[0].owner(), "GNOME Shell");

        let found = conflicts("<Alt>Tab", "org.gnome.shell.extensions.octosnap", "all-in-one", &known);
        assert_eq!(found[0].key, "switch-applications");
        assert_eq!(found[0].label(), "Switch applications");
        assert_eq!(found[0].owner(), "the window manager");
    }

    #[test]
    fn our_other_actions_conflict_and_the_one_being_recorded_does_not() {
        let ours = "org.gnome.shell.extensions.octosnap";
        let known = known();
        let found = conflicts("<Shift><Super>4", ours, "capture-window", &known);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].key, "capture-area");
        assert_eq!(found[0].owner(), "OctoSnap");
        assert!(conflicts("<Shift><Super>4", ours, "capture-area", &known).is_empty());
        assert!(conflicts("<Super>F9", ours, "capture-area", &known).is_empty());
        assert!(conflicts("", ours, "capture-area", &known).is_empty(), "nothing to compare");
    }

    #[test]
    fn the_take_over_clears_gnome_first_and_the_give_back_frees_ours_first() {
        let ours = "org.gnome.shell.extensions.octosnap";
        let writes = take_over_print_keys(ours);
        assert_eq!(writes.len(), 6);
        assert_eq!(
            writes[0],
            Write::Set { schema: GNOME_SHELL_KEYBINDINGS, key: "show-screenshot-ui", accelerators: vec![] }
        );
        assert_eq!(writes[3], Write::Set { schema: ours, key: "all-in-one", accelerators: vec!["Print"] });
        assert_eq!(writes[4], Write::Set { schema: ours, key: "capture-area", accelerators: vec!["<Shift>Print"] });
        assert_eq!(writes[5], Write::Set { schema: ours, key: "capture-window", accelerators: vec!["<Alt>Print"] });

        let back = give_back_print_keys(ours);
        assert_eq!(back.len(), 6);
        assert_eq!(back[0], Write::Reset { schema: ours, key: "all-in-one" });
        assert_eq!(back[5], Write::Reset { schema: GNOME_SHELL_KEYBINDINGS, key: "screenshot-window" });
    }

    #[test]
    fn the_take_over_is_in_force_only_when_every_key_moved() {
        let taken_ours = |key: &str| -> Vec<String> {
            match key {
                "all-in-one" => vec!["Print".to_owned()],
                "capture-area" => vec!["<Shift>Print".to_owned()],
                "capture-window" => vec!["<Alt>Print".to_owned()],
                _ => vec![],
            }
        };
        let cleared = |_: &str| -> Vec<String> { vec![] };
        assert!(print_keys_taken(taken_ours, cleared));

        // This machine on 2026-09-12: two GNOME keys cleared by hand, one still bound,
        // and All-In-One on Ctrl+Print. Not in force.
        let machine_ours = |key: &str| -> Vec<String> {
            match key {
                "all-in-one" => vec!["<Control>Print".to_owned()],
                _ => vec![],
            }
        };
        let machine_gnome = |key: &str| -> Vec<String> {
            match key {
                "screenshot-window" => vec!["<Alt>Print".to_owned()],
                _ => vec![],
            }
        };
        assert!(!print_keys_taken(machine_ours, machine_gnome));
        // Ours hold the keys but GNOME still does too: the shell would refuse ours.
        let gnome_default = |key: &str| -> Vec<String> {
            match key {
                "show-screenshot-ui" => vec!["Print".to_owned()],
                "screenshot" => vec!["<Shift>Print".to_owned()],
                "screenshot-window" => vec!["<Alt>Print".to_owned()],
                _ => vec![],
            }
        };
        assert!(!print_keys_taken(taken_ours, gnome_default));
    }

    #[test]
    fn humanize_capitalises_the_first_word_only() {
        assert_eq!(humanize("switch-applications"), "Switch applications");
        assert_eq!(humanize("show_screenshot_ui"), "Show screenshot ui");
        assert_eq!(humanize(""), "");
    }
}
