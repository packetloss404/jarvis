use serde::{Deserialize, Serialize};

/// Configuration for the mobile relay bridge.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RelayConfig {
    /// WebSocket URL of the relay server.
    pub url: String,
    /// Whether to connect to the relay automatically on startup.
    pub auto_connect: bool,
    /// Optional per-deployment secret mixed into PBKDF2 channel key derivation.
    ///
    /// When set, relay chat channel keys include this value in the PBKDF2
    /// password (`"{channel_id}:{channel_secret}"`), making them
    /// non-precomputable from the public channel name alone. Without this
    /// value, channel keys are deterministic from the public channel name
    /// and messages are authenticated but not confidential against observers.
    pub channel_secret: Option<String>,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            url: "wss://jarvis-relay-production-3eb6.up.railway.app/ws".into(),
            auto_connect: false,
            channel_secret: None,
        }
    }
}
