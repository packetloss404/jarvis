//! Social and presence configuration types.

use serde::{Deserialize, Serialize};

/// Presence system configuration.
///
/// Presence rides the project's relay Room transport; it reuses the relay URL
/// from `[relay].url` and joins the global room named by [`room_id`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PresenceConfig {
    pub enabled: bool,
    /// Session id of the global presence room every desktop joins.
    pub room_id: String,
}

/// Default global presence room id (kept in sync with
/// `jarvis_social::presence::DEFAULT_PRESENCE_ROOM_ID`).
pub const DEFAULT_PRESENCE_ROOM_ID: &str = "jarvis-presence-global";

impl Default for PresenceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            room_id: DEFAULT_PRESENCE_ROOM_ID.to_string(),
        }
    }
}
