// SPDX-License-Identifier: GPL-3.0-or-later

use thiserror::Error;

/// `spec/10` §8: "No panics across the D-Bus boundary; all handlers return typed errors."
/// This is the app-side half of that rule.
#[derive(Debug, Error)]
pub enum BridgeError {
    /// The extension is not installed, not enabled, or the shell restarted. This is the
    /// signal to enter degraded mode and show the setup page (`spec/10` §2), not a crash.
    #[error("the OctoSnap shell extension is not available: {0}")]
    Unavailable(String),

    /// The extension answered, but with a protocol major this build cannot speak.
    #[error("protocol mismatch: extension speaks {found}, this build speaks {expected}")]
    ProtocolMismatch { expected: u32, found: u32 },

    /// The extension is there but does not implement this member.
    ///
    /// Distinct from `Unavailable` on purpose. An absent extension means degraded mode;
    /// a *present* extension missing a method means the two halves are different
    /// versions, which is `spec/10` §2's protocol-mismatch case and needs a different
    /// message. Conflating them produced "the extension is not available" for an
    /// extension that was plainly running, which cost real debugging time.
    #[error("the shell extension does not implement {method}; the two halves are different versions")]
    Unsupported { method: &'static str },

    /// A reply arrived with a shape the contract does not allow.
    #[error("malformed reply from {method}: {detail}")]
    MalformedReply { method: &'static str, detail: String },

    #[error("D-Bus call failed: {0}")]
    Dbus(#[from] glib::Error),
}

impl BridgeError {
    /// True when the right response is to fall back to degraded mode rather than to
    /// report a fault to the user.
    #[must_use]
    pub const fn is_extension_missing(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }

    /// True when the halves disagree about the protocol, so the feature should be
    /// refused rather than the whole extension written off (`spec/10` §2).
    #[must_use]
    pub const fn is_version_mismatch(&self) -> bool {
        matches!(self, Self::Unsupported { .. } | Self::ProtocolMismatch { .. })
    }
}
