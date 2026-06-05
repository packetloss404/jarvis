//! Relay-level wire protocol. Only the first message is parsed; everything after
//! is forwarded as opaque text frames.

use serde::{Deserialize, Serialize};

/// First message a client sends to identify itself.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum RelayHello {
    #[serde(rename = "desktop_hello")]
    DesktopHello { session_id: String },

    #[serde(rename = "mobile_hello")]
    MobileHello { session_id: String },

    #[serde(rename = "host_hello")]
    HostHello { session_id: String },

    #[serde(rename = "spectator_hello")]
    SpectatorHello { session_id: String },

    #[serde(rename = "room_hello")]
    RoomHello {
        session_id: String,
        member_id: String,
    },
}

/// Messages the relay sends back to clients.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum RelayResponse {
    #[serde(rename = "session_ready")]
    SessionReady { session_id: String },

    #[serde(rename = "peer_connected")]
    PeerConnected,

    #[serde(rename = "peer_disconnected")]
    PeerDisconnected,

    #[serde(rename = "host_connected")]
    HostConnected,

    #[serde(rename = "host_disconnected")]
    HostDisconnected,

    #[serde(rename = "viewer_count")]
    ViewerCount { count: usize },

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

#[cfg(test)]
mod wire_conformance_tests {
    use super::RelayResponse;

    const SESSION_READY_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/relay/session_ready.json"
    ));

    const ROOM_READY_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/relay/room_ready.json"
    ));

    /// Shared JSON in `jarvis-rs/testdata/relay/` must match what the desktop client deserializes.
    #[test]
    fn session_ready_json_matches_fixture() {
        let msg = RelayResponse::SessionReady {
            session_id: "test-sid".to_string(),
        };
        let v = serde_json::to_value(&msg).unwrap();
        let expected: serde_json::Value =
            serde_json::from_str(SESSION_READY_FIXTURE.trim()).unwrap();
        assert_eq!(v, expected);
    }

    #[test]
    fn room_ready_json_matches_fixture() {
        let msg = RelayResponse::RoomReady {
            session_id: "test-sid".to_string(),
        };
        let v = serde_json::to_value(&msg).unwrap();
        let expected: serde_json::Value =
            serde_json::from_str(ROOM_READY_FIXTURE.trim()).unwrap();
        assert_eq!(v, expected);
    }

    #[test]
    fn room_member_messages_serialize_with_expected_tags() {
        assert_eq!(
            serde_json::to_value(&RelayResponse::MemberJoined {
                member_id: "m1".to_string(),
            })
            .unwrap(),
            serde_json::json!({"type": "member_joined", "member_id": "m1"})
        );
        assert_eq!(
            serde_json::to_value(&RelayResponse::MemberLeft {
                member_id: "m1".to_string(),
            })
            .unwrap(),
            serde_json::json!({"type": "member_left", "member_id": "m1"})
        );
        assert_eq!(
            serde_json::to_value(&RelayResponse::MemberCount { count: 3 }).unwrap(),
            serde_json::json!({"type": "member_count", "count": 3})
        );
    }
}
