// SPDX-License-Identifier: GPL-3.0-or-later

//! `SYS-02`, `spec/08` §1's "Launch at login": an XDG autostart entry.
//!
//! The app is a D-Bus service that starts on its first call, so "launch at login" is not
//! what makes OctoSnap *work* -- the extension is loaded by the shell, and a shortcut
//! activates the app whether or not it is running. What the entry buys is the first
//! capture landing inside `spec/00` §9's budget instead of paying a cold start, and the
//! history janitor running at login rather than on first use.
//!
//! The entry is a file in `$XDG_CONFIG_HOME/autostart/`, which is what the desktop reads
//! and what GNOME Settings' own "Startup Applications" lists. Writing one is the whole
//! implementation; this module only says what to write and reads back whether it is on.

use std::path::{Path, PathBuf};

/// The entry's file name -- the app id, as `gnome-session` matches it.
pub const FILE_NAME: &str = "io.github.odrakirmusic.OctoSnap.desktop";

/// `<config home>/autostart/<file>`.
#[must_use]
pub fn path(config_home: &Path) -> PathBuf {
    config_home.join("autostart").join(FILE_NAME)
}

/// What the service is started under, wherever it is started from outside a Flatpak: two
/// malloc arenas rather than one per thread that ever allocated, and a fixed mmap threshold
/// rather than one that rises to the largest buffer freed (`docs/decisions.md` D128). The
/// D-Bus service file says the same (`data/meson.build`); glibc reads both once, at exec, so
/// they have to be in the command and cannot be set by the service itself.
pub const LAUNCH_ENV: &str = "/usr/bin/env MALLOC_ARENA_MAX=2 MALLOC_MMAP_THRESHOLD_=131072";

/// The file's contents, starting `exec` under [`LAUNCH_ENV`]. `NoDisplay`, because this
/// is not the launcher -- `data/io.github.odrakirmusic.OctoSnap.desktop.in` is -- and an
/// entry that showed in the app grid would be a second OctoSnap.
#[must_use]
pub fn entry(exec: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=OctoSnap\n\
         Comment=Starts the OctoSnap service at login\n\
         Exec={LAUNCH_ENV} {exec}\n\
         Icon=io.github.odrakirmusic.OctoSnap\n\
         Terminal=false\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n\
         OnlyShowIn=GNOME;\n",
        exec = quote_exec(exec)
    )
}

/// An existing entry with its `Exec` started under [`LAUNCH_ENV`], or `None` when it
/// already is or has no `Exec`. An entry written before D128 starts the service without
/// it, and a service started at login is never started by the D-Bus service file, so this
/// is how such a login gets the malloc settings. The binary the entry names is kept: this
/// changes how the service starts, not which build it is.
#[must_use]
pub fn refresh(contents: &str) -> Option<String> {
    let mut out = String::with_capacity(contents.len() + LAUNCH_ENV.len() + 1);
    let mut changed = false;
    let mut seen_exec = false;
    for line in contents.split_inclusive('\n') {
        let (text, end) = line.strip_suffix('\n').map_or((line, ""), |text| (text, "\n"));
        match text.strip_prefix("Exec=") {
            Some(command) if !seen_exec => {
                seen_exec = true;
                let wanted = format!("Exec={LAUNCH_ENV} {}", without_launch_env(command.trim()));
                changed |= wanted != text;
                out.push_str(&wanted);
                out.push_str(end);
            }
            _ => out.push_str(line),
        }
    }
    changed.then_some(out)
}

/// `command` without the `env` and malloc settings an earlier build put in front of it.
/// Anything else given to `env` stays, and ends up after [`LAUNCH_ENV`]'s own.
fn without_launch_env(command: &str) -> &str {
    let Some(mut rest) = command.strip_prefix("/usr/bin/env ") else {
        return command;
    };
    loop {
        rest = rest.trim_start();
        match rest.split_once(' ') {
            Some((word, after)) if word.starts_with("MALLOC_") && word.contains('=') => rest = after,
            _ => return rest,
        }
    }
}

/// Whether an existing entry is on. A file the user has disabled from GNOME Settings
/// gets `X-GNOME-Autostart-enabled=false` or `Hidden=true` rather than being deleted, so
/// a present file is not the same as an enabled one.
#[must_use]
pub fn is_enabled(contents: &str) -> bool {
    let value = |key: &str| {
        contents
            .lines()
            .map(str::trim)
            .find_map(|line| line.strip_prefix(key).and_then(|rest| rest.strip_prefix('=')))
            .map(|v| v.trim().to_ascii_lowercase())
    };
    if value("Hidden").as_deref() == Some("true") {
        return false;
    }
    value("X-GNOME-Autostart-enabled").as_deref() != Some("false")
}

/// The desktop entry specification's `Exec` quoting: a path with a space, a quote or a
/// backslash goes in double quotes with those characters escaped.
fn quote_exec(exec: &Path) -> String {
    let text = exec.to_string_lossy();
    if text.chars().any(|c| c.is_whitespace() || matches!(c, '"' | '\\' | '\'' | '$' | '`')) {
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"").replace('$', "\\$").replace('`', "\\`");
        format!("\"{escaped}\"")
    } else {
        text.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_lives_under_autostart_and_starts_the_service_hidden() {
        assert_eq!(
            path(Path::new("/home/u/.config")),
            PathBuf::from("/home/u/.config/autostart/io.github.odrakirmusic.OctoSnap.desktop")
        );
        let text = entry(Path::new("/usr/bin/octosnap-app"));
        assert!(text.starts_with("[Desktop Entry]\n"));
        assert!(
            text.contains(
                "\nExec=/usr/bin/env MALLOC_ARENA_MAX=2 MALLOC_MMAP_THRESHOLD_=131072 /usr/bin/octosnap-app\n"
            ),
            "{text}"
        );
        assert!(text.contains("\nNoDisplay=true\n"));
        assert!(text.contains("\nX-GNOME-Autostart-enabled=true\n"));
        assert!(is_enabled(&text));
    }

    #[test]
    fn a_path_with_a_space_is_quoted() {
        let text = entry(Path::new("/home/u/my apps/octosnap-app"));
        assert!(text.contains("=131072 \"/home/u/my apps/octosnap-app\"\n"), "{text}");
        let text = entry(Path::new("/opt/a\"b/octosnap-app"));
        assert!(text.contains("=131072 \"/opt/a\\\"b/octosnap-app\"\n"), "{text}");
    }

    #[test]
    fn an_entry_gnome_settings_switched_off_reads_as_off() {
        let off = "[Desktop Entry]\nType=Application\nX-GNOME-Autostart-enabled=false\n";
        assert!(!is_enabled(off));
        let hidden = "[Desktop Entry]\nHidden=true\n";
        assert!(!is_enabled(hidden));
        let plain = "[Desktop Entry]\nType=Application\nExec=x\n";
        assert!(is_enabled(plain), "no key at all means on, as the specification reads it");
    }

    #[test]
    fn an_entry_from_before_d128_is_started_under_the_launch_env_and_keeps_its_binary() {
        let old = "[Desktop Entry]\nType=Application\nExec=/home/u/OctoSnap/target/release/octosnap-app\n\
                   X-GNOME-Autostart-Delay=5\n";
        let new = format!(
            "[Desktop Entry]\nType=Application\nExec={LAUNCH_ENV} /home/u/OctoSnap/target/release/octosnap-app\n\
             X-GNOME-Autostart-Delay=5\n"
        );
        assert_eq!(refresh(old), Some(new.clone()));
        assert_eq!(refresh(&new), None, "a refreshed entry is left alone");
        assert_eq!(refresh(&entry(Path::new("/usr/bin/octosnap-app"))), None);
    }

    #[test]
    fn an_earlier_launch_env_is_replaced_rather_than_stacked() {
        let arenas_only = "[Desktop Entry]\nExec=/usr/bin/env MALLOC_ARENA_MAX=2 /usr/bin/octosnap-app";
        assert_eq!(
            refresh(arenas_only).as_deref(),
            Some(format!("[Desktop Entry]\nExec={LAUNCH_ENV} /usr/bin/octosnap-app").as_str()),
            "an entry without a final newline still has none"
        );
        let quoted = "Exec=/usr/bin/env MALLOC_ARENA_MAX=2 \"/home/u/my apps/octosnap-app\"\n";
        assert_eq!(
            refresh(quoted),
            Some(format!("Exec={LAUNCH_ENV} \"/home/u/my apps/octosnap-app\"\n"))
        );
        let own_env = "Exec=/usr/bin/env GDK_BACKEND=wayland /usr/bin/octosnap-app\n";
        assert_eq!(
            refresh(own_env),
            Some(format!("Exec={LAUNCH_ENV} GDK_BACKEND=wayland /usr/bin/octosnap-app\n"))
        );
        assert_eq!(refresh("[Desktop Entry]\nType=Application\n"), None, "nothing to start");
    }
}
