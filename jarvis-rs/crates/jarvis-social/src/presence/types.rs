//! Configuration and event types for the presence client.

use crate::protocol::OnlineUser;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Default global presence room id.
pub const DEFAULT_PRESENCE_ROOM_ID: &str = "jarvis-presence-global";

/// Configuration for the presence client.
///
/// Presence rides the project's own relay Room transport (see [`crate::room`]).
/// If [`relay_url`](Self::relay_url) is empty the presence client no-ops
/// gracefully.
#[derive(Debug, Clone)]
pub struct PresenceConfig {
    /// WebSocket URL of the relay server. Empty disables presence.
    pub relay_url: String,
    /// The global presence room session id every desktop joins.
    pub room_id: String,
    /// Reconnect delay (base) in seconds.
    pub reconnect_delay: u64,
    /// Maximum reconnect delay in seconds.
    pub max_reconnect_delay: u64,
}

impl Default for PresenceConfig {
    fn default() -> Self {
        Self {
            relay_url: String::new(),
            room_id: DEFAULT_PRESENCE_ROOM_ID.to_string(),
            reconnect_delay: 1,
            max_reconnect_delay: 30,
        }
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Events emitted by the presence system for the UI to consume.
#[derive(Debug, Clone)]
pub enum PresenceEvent {
    Connected {
        online_count: u32,
    },
    Disconnected,
    UserOnline(OnlineUser),
    UserOffline {
        user_id: String,
        display_name: String,
    },
    ActivityChanged(OnlineUser),
    GameInvite {
        user_id: String,
        display_name: String,
        game: String,
        code: Option<String>,
    },
    Poked {
        user_id: String,
        display_name: String,
    },
    ChatMessage {
        user_id: String,
        display_name: String,
        channel: String,
        content: String,
    },
    Error(String),
}
