//! Collaborative terminal / pair-programming configuration types.
//!
//! Pair programming rides the project's relay Room transport (the same
//! transport as presence and chat). It is **enabled by default** as of the
//! signed-`room_hello` slot binding (the relay binds each Room slot to its key).
//! As of the M3 hardening it uses END-TO-END SIGNED FRAMES (per-app ECDSA identity) so
//! each member is authenticated and `require_signed_join` is now ENFORCED — the
//! shared room key is confidentiality-only, and the signatures are the security
//! boundary between members. See the C2 spec.

use serde::{Deserialize, Serialize};

/// Configuration for collaborative terminal / pair-programming sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CollabConfig {
    /// Master toggle. When false, all pair IPC and transport are no-ops.
    pub enabled: bool,
    /// Maximum participants per session (including the host).
    pub max_participants: usize,
    /// Whether navigators may request/take the driver seat.
    pub allow_takeover: bool,
    /// **ENFORCED (M3).** When true (the default), every inbound pair frame
    /// MUST carry a valid end-to-end ECDSA signature binding the sender's
    /// `member_id` to its identity public key (see `SignedPairFrame`). Unsigned
    /// or unverifiable frames are dropped fail-closed: an unsigned join never
    /// registers a member, an unsigned host-only frame (`pty_output`/
    /// `driver_changed`/`session_meta`/…) is rejected, and a forged `from`
    /// (impersonating the driver) is rejected. With this set the shared room key
    /// is confidentiality-only and the signatures are the security boundary
    /// between members. When false, the legacy permissive M1/M2 path is used
    /// (unsigned frames accepted, self-asserted `from`) — experiments only.
    pub require_signed_join: bool,
}

impl Default for CollabConfig {
    fn default() -> Self {
        Self {
            // Enabled by default as of the signed-`room_hello` slot binding +
            // M3 signed pair frames: members are authenticated and the relay
            // binds each Room slot to its key, so the experimental gate is
            // lifted. Still feature-flagged here so it can be turned off.
            enabled: true,
            max_participants: 4,
            allow_takeover: true,
            require_signed_join: true,
        }
    }
}
