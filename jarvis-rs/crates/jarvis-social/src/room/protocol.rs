//! Relay Room wire types and the transport-level config/event enums.

use std::sync::Arc;

use serde::Deserialize;

use super::signed_hello::RoomHelloSigner;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for a relay Room connection.
#[derive(Debug, Clone)]
pub struct RoomConfig {
    /// WebSocket URL of the relay server (e.g. `wss://relay.example/ws`).
    pub relay_url: String,
    /// Room session id. Every member joining the same id shares the room.
    pub session_id: String,
    /// This member's stable id (`member_id` in the `room_hello`).
    pub member_id: String,
    /// Reconnect base delay in seconds.
    pub reconnect_delay_secs: u64,
    /// Maximum reconnect delay in seconds.
    pub max_reconnect_delay_secs: u64,
    /// ECDSA signer for the SIGNED `room_hello` (member-id slot DoS fix). The
    /// relay now REQUIRES a signed hello; the owner (jarvis-app, which holds the
    /// `CryptoService` identity) injects this. `None` means the hello is sent
    /// unsigned and the relay will reject it — a deliberate hard failure until
    /// the signer is plumbed.
    pub signer: Option<Arc<RoomHelloSigner>>,
}

impl Default for RoomConfig {
    fn default() -> Self {
        Self {
            relay_url: String::new(),
            session_id: String::new(),
            member_id: String::new(),
            reconnect_delay_secs: 1,
            max_reconnect_delay_secs: 30,
            signer: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Relay control frames (relay → client)
// ---------------------------------------------------------------------------

/// Control frames the relay emits on a Room session. Mirrors
/// `jarvis_relay::protocol::RelayResponse`'s room variants. Any text frame
/// that does not deserialize as one of these is treated as an opaque member
/// frame and surfaced via [`RoomEvent::Frame`].
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum RoomControl {
    #[serde(rename = "room_ready")]
    RoomReady { session_id: String },

    #[serde(rename = "member_joined")]
    MemberJoined { member_id: String },

    #[serde(rename = "member_left")]
    MemberLeft { member_id: String },

    #[serde(rename = "member_count")]
    MemberCount { count: usize },

    #[serde(rename = "error")]
    Error { message: String },
}

// ---------------------------------------------------------------------------
// Events surfaced to the owner of a RoomClient
// ---------------------------------------------------------------------------

/// Events emitted by [`RoomClient`].
#[derive(Debug, Clone)]
pub enum RoomEvent {
    /// The relay accepted our `room_hello`; we are registered in the room.
    Ready { session_id: String },
    /// Another member joined (also emitted once per existing member right
    /// after `room_ready`, forming the initial roster).
    MemberJoined { member_id: String },
    /// A member left.
    MemberLeft { member_id: String },
    /// Current member count reported by the relay.
    MemberCount { count: usize },
    /// An opaque text frame from another member (application payload).
    Frame(String),
    /// The relay reported a fatal/validation error for this session.
    RelayError(String),
    /// The WebSocket connection was lost (a reconnect attempt will follow).
    Disconnected,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_room_ready() {
        let v: RoomControl =
            serde_json::from_str(r#"{"type":"room_ready","session_id":"s1"}"#).unwrap();
        assert_eq!(
            v,
            RoomControl::RoomReady {
                session_id: "s1".into()
            }
        );
    }

    #[test]
    fn parses_member_frames() {
        assert_eq!(
            serde_json::from_str::<RoomControl>(r#"{"type":"member_joined","member_id":"m1"}"#)
                .unwrap(),
            RoomControl::MemberJoined {
                member_id: "m1".into()
            }
        );
        assert_eq!(
            serde_json::from_str::<RoomControl>(r#"{"type":"member_left","member_id":"m1"}"#)
                .unwrap(),
            RoomControl::MemberLeft {
                member_id: "m1".into()
            }
        );
        assert_eq!(
            serde_json::from_str::<RoomControl>(r#"{"type":"member_count","count":3}"#).unwrap(),
            RoomControl::MemberCount { count: 3 }
        );
    }

    #[test]
    fn opaque_presence_frame_is_not_control() {
        // A presence application frame must NOT parse as a control frame, so
        // the client forwards it as RoomEvent::Frame.
        let r = serde_json::from_str::<RoomControl>(
            r#"{"type":"presence","user":{"user_id":"u"}}"#,
        );
        assert!(r.is_err());
    }
}
