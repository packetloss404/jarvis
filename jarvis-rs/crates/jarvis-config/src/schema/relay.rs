use serde::{Deserialize, Serialize};

/// Configuration for the mobile relay bridge.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RelayConfig {
    /// WebSocket URL of the relay server.
    pub url: String,
    /// Whether to connect to the relay automatically on startup.
    pub auto_connect: bool,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            url: "wss://jarvis-relay-production-3eb6.up.railway.app/ws".into(),
            auto_connect: false,
        }
    }
}
