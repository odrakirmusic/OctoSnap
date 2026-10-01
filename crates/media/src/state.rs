// SPDX-License-Identifier: GPL-3.0-or-later
//! The recorder's phases, named the way `spec/10` §3.1's `SetRecordingState` names them,
//! and what each phase allows. Pure, so the flow, the panel and the CLI agree on one
//! answer to "can this be stopped now?" instead of three.

/// Where a recording is. `Paused` is M9's; a GIF has no pause (D68).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Idle,
    /// The countdown runs over the live screen; nothing is recorded yet.
    Countdown,
    Recording,
    /// Stop was pressed; the encoder is finishing the file.
    Processing,
}

/// What the user can ask of a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Finish the file (`spec/06` §3 Stop).
    Stop,
    /// Discard everything (`spec/06` §3 Trash).
    Trash,
}

impl Phase {
    /// The wire name `SetRecordingState` carries.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Countdown => "countdown",
            Self::Recording => "recording",
            Self::Processing => "processing",
        }
    }

    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "idle" => Self::Idle,
            "countdown" => Self::Countdown,
            "recording" => Self::Recording,
            "processing" => Self::Processing,
            _ => return None,
        })
    }

    /// True while something of ours is on the screen or on the bus for this recording.
    #[must_use]
    pub const fn is_busy(self) -> bool {
        !matches!(self, Self::Idle)
    }

    /// The phase after `command`, or `None` when the phase does not take it: Stop is
    /// only meaningful while frames are being taken, and Trash any time before the
    /// file exists.
    #[must_use]
    pub const fn after(self, command: Command) -> Option<Self> {
        match (self, command) {
            (Self::Recording, Command::Stop) => Some(Self::Processing),
            (Self::Countdown | Self::Recording | Self::Processing, Command::Trash) => {
                Some(Self::Idle)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_only_stops_a_recording() {
        assert_eq!(
            Phase::Recording.after(Command::Stop),
            Some(Phase::Processing)
        );
        assert_eq!(Phase::Idle.after(Command::Stop), None);
        assert_eq!(Phase::Countdown.after(Command::Stop), None);
        assert_eq!(Phase::Processing.after(Command::Stop), None);
    }

    #[test]
    fn trash_discards_anything_that_is_not_finished() {
        assert_eq!(Phase::Countdown.after(Command::Trash), Some(Phase::Idle));
        assert_eq!(Phase::Recording.after(Command::Trash), Some(Phase::Idle));
        assert_eq!(Phase::Processing.after(Command::Trash), Some(Phase::Idle));
        assert_eq!(Phase::Idle.after(Command::Trash), None);
    }

    #[test]
    fn wire_names_round_trip() {
        for phase in [
            Phase::Idle,
            Phase::Countdown,
            Phase::Recording,
            Phase::Processing,
        ] {
            assert_eq!(Phase::from_wire(phase.as_wire()), Some(phase));
        }
        assert_eq!(Phase::from_wire("paused"), None);
        assert!(!Phase::Idle.is_busy());
        assert!(Phase::Processing.is_busy());
    }
}
