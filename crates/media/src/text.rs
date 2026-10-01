// SPDX-License-Identifier: GPL-3.0-or-later
//! The words a recording shows: the card's badge and the controls' clock.

/// `spec/04`'s badge, verified as `7s 337 KB`: the duration and the file size together.
#[must_use]
pub fn badge(duration_ms: u64, bytes: u64) -> String {
    format!("{} {}", duration_label(duration_ms), size_label(bytes))
}

/// `7s` under a minute, `1:05` under an hour, `1:02:03` beyond, to the nearest second.
#[must_use]
pub fn duration_label(duration_ms: u64) -> String {
    let seconds = (duration_ms + 500) / 1000;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    clock(seconds, false)
}

/// The controls' clock: `00:42`, always two digits of minutes, hours only when needed.
#[must_use]
pub fn elapsed_label(elapsed_ms: u64) -> String {
    clock(elapsed_ms / 1000, true)
}

fn clock(seconds: u64, pad_minutes: bool) -> String {
    let (h, m, s) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else if pad_minutes {
        format!("{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// `337 KB`, `3.1 MB`: decimal units, the way GNOME Files and the verified badge show them.
#[must_use]
pub fn size_label(bytes: u64) -> String {
    const KB: f64 = 1_000.0;
    const MB: f64 = 1_000_000.0;
    const GB: f64 = 1_000_000_000.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < MB {
        format!("{} KB", (b / KB).round() as u64)
    } else if b < GB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{:.2} GB", b / GB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_badge_reads_like_the_verified_one() {
        assert_eq!(badge(7_200, 337_000), "7s 337 KB");
        assert_eq!(badge(10_000, 3_140_000), "10s 3.1 MB");
        assert_eq!(badge(65_000, 12_345), "1:05 12 KB");
        assert_eq!(badge(3_723_000, 2_500_000_000), "1:02:03 2.50 GB");
        assert_eq!(badge(400, 999), "0s 999 B");
    }

    #[test]
    fn the_clock_pads_minutes_and_grows_hours() {
        assert_eq!(elapsed_label(0), "00:00");
        assert_eq!(elapsed_label(42_900), "00:42");
        assert_eq!(elapsed_label(65_000), "01:05");
        assert_eq!(elapsed_label(3_600_000), "1:00:00");
    }
}
