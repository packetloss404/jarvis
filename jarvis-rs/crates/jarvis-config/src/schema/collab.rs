//! Collaborative terminal / pair-programming configuration types.
//!
//! Pair programming rides the project's relay Room transport (the same
//! transport as presence and chat). It is **disabled by default** and has
//! limited authentication today — see the C2 spec. Do not enable in
//! production until M3 signed-join hardening lands.

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
    /// **NOT YET ENFORCED (M3 placeholder).** Intended to require an
    /// ECDSA-signed join over the session id so the host can authenticate each
    /// member. Today this flag is read but no signature is verified: pair
    /// sessions have NO per-member authentication (shared symmetric room key +
    /// self-asserted `from`/`member_id`), so a member can impersonate the
    /// driver, forge host-only frames, or hijack the host slot. The room key
    /// provides confidentiality from the relay ONLY — it is not a security
    /// boundary between members. Until M3 (signed-join + host-authority +
    /// per-member ECDH) lands, keep `enabled = false` outside experiments.
    pub require_signed_join: bool,
}

impl Default for CollabConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_participants: 4,
            allow_takeover: true,
            require_signed_join: true,
        }
    }
}
