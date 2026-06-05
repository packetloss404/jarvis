//! Protocol types for the Jarvis presence/social system.
//!
//! These types define the application-level payloads exchanged between
//! desktops. Presence rides over the relay's symmetric **Room** transport
//! (see [`crate::room`]); each payload is serialized as an opaque text frame
//! tagged by [`PresenceFrame`]. The relay forwards these frames verbatim to
//! every other room member.
//!
//! Voice, screen share, and pair programming types are gated behind
//! the `experimental-collab` feature flag.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Broadcast event names
// ---------------------------------------------------------------------------

/// Event names used as the `type` tag of presence room frames.
pub mod events {
    pub const ACTIVITY_UPDATE: &str = "activity_update";
    pub const GAME_INVITE: &str = "game_invite";
    pub const POKE: &str = "poke";
    pub const CHAT_MESSAGE: &str = "chat_message";
    /// A member announcing (or re-announcing) its own roster entry.
    pub const PRESENCE: &str = "presence";
}

// ---------------------------------------------------------------------------
// Presence room frames
// ---------------------------------------------------------------------------

/// Opaque application frames exchanged over the presence relay Room.
///
/// Each variant serializes to JSON tagged by `type` (matching the [`events`]
/// constants), e.g. `{"type":"presence", ...}`. These are distinct from the
/// relay's own control frames (`room_ready` / `member_joined` / `member_left`
/// / `member_count`), so a member can tell control traffic from application
/// traffic by attempting to parse a `PresenceFrame`.
///
/// `poke` carries a `target_user_id`; because the relay fan-out is
/// broadcast-to-all, every member receives every poke and filters by target.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PresenceFrame {
    /// A member announcing its current roster entry. Sent on join, on
    /// `member_joined` (so newcomers learn the roster), and on activity change.
    #[serde(rename = "presence")]
    Presence { user: OnlineUser },
    /// An activity/status change. Functionally a roster update plus a UI event.
    #[serde(rename = "activity_update")]
    ActivityUpdate(ActivityUpdatePayload),
    /// A directed poke (filtered by `target_user_id`).
    #[serde(rename = "poke")]
    Poke(PokePayload),
    /// A game invite broadcast to the room.
    #[serde(rename = "game_invite")]
    GameInvite(GameInvitePayload),
    /// A chat message broadcast to a logical channel.
    #[serde(rename = "chat_message")]
    ChatMessage(ChatMessagePayload),
}

// ---------------------------------------------------------------------------
// Broadcast payloads
// ---------------------------------------------------------------------------

/// Payload for activity update broadcasts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityUpdatePayload {
    pub user_id: String,
    pub display_name: String,
    pub status: UserStatus,
    pub activity: Option<String>,
}

/// Payload for game invite broadcasts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameInvitePayload {
    pub user_id: String,
    pub display_name: String,
    pub game: String,
    pub code: Option<String>,
}

/// Payload for poke broadcasts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PokePayload {
    pub user_id: String,
    pub display_name: String,
    pub target_user_id: String,
}

/// Payload for chat message broadcasts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessagePayload {
    pub user_id: String,
    pub display_name: String,
    pub channel: String,
    pub content: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

/// Presence payload tracked in the relay Room for each user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresencePayload {
    pub user_id: String,
    pub display_name: String,
    pub status: UserStatus,
    pub activity: Option<String>,
    pub online_at: String,
}

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

/// User presence status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserStatus {
    #[default]
    Online,
    Idle,
    InGame,
    InSkill,
    DoNotDisturb,
    Away,
}

/// Information about an online user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlineUser {
    pub user_id: String,
    pub display_name: String,
    pub status: UserStatus,
    pub activity: Option<String>,
}

// ---------------------------------------------------------------------------
// Presence frame tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod presence_frame_tests {
    use super::*;

    /// Every presence frame must round-trip through JSON, and the wire `type`
    /// tag must match the [`events`] constants so the relay fan-out stays opaque
    /// while clients can still discriminate.
    fn roundtrip(frame: PresenceFrame, expected_type: &str) {
        let json = serde_json::to_string(&frame).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["type"], expected_type, "wire tag for {json}");
        // Re-parse and re-serialize: the canonical JSON must be stable.
        let back: PresenceFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    #[test]
    fn presence_frames_roundtrip_with_expected_tags() {
        roundtrip(
            PresenceFrame::Presence {
                user: OnlineUser {
                    user_id: "u1".into(),
                    display_name: "Ada".into(),
                    status: UserStatus::Online,
                    activity: Some("in terminal".into()),
                },
            },
            events::PRESENCE,
        );
        roundtrip(
            PresenceFrame::ActivityUpdate(ActivityUpdatePayload {
                user_id: "u1".into(),
                display_name: "Ada".into(),
                status: UserStatus::InGame,
                activity: Some("chess".into()),
            }),
            events::ACTIVITY_UPDATE,
        );
        roundtrip(
            PresenceFrame::Poke(PokePayload {
                user_id: "u1".into(),
                display_name: "Ada".into(),
                target_user_id: "u2".into(),
            }),
            events::POKE,
        );
        roundtrip(
            PresenceFrame::GameInvite(GameInvitePayload {
                user_id: "u1".into(),
                display_name: "Ada".into(),
                game: "tetris".into(),
                code: None,
            }),
            events::GAME_INVITE,
        );
        roundtrip(
            PresenceFrame::ChatMessage(ChatMessagePayload {
                user_id: "u1".into(),
                display_name: "Ada".into(),
                channel: "general".into(),
                content: "hi".into(),
                timestamp: "0".into(),
                reply_to: None,
            }),
            events::CHAT_MESSAGE,
        );
    }

    #[test]
    fn presence_frame_does_not_collide_with_relay_control_tags() {
        // Relay control frames use tags like "member_joined"/"room_ready".
        // None of our presence frame tags may equal those, or the RoomClient
        // would misclassify application traffic as control traffic.
        for tag in [
            events::PRESENCE,
            events::ACTIVITY_UPDATE,
            events::POKE,
            events::GAME_INVITE,
            events::CHAT_MESSAGE,
        ] {
            assert!(!matches!(
                tag,
                "room_ready" | "member_joined" | "member_left" | "member_count" | "error"
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// WebRTC signaling types (experimental-collab only)
// ---------------------------------------------------------------------------

/// WebRTC signaling messages for voice chat.
#[cfg(feature = "experimental-collab")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VoiceSignal {
    /// SDP offer to establish a peer connection.
    Offer { sdp: String },
    /// SDP answer in response to an offer.
    Answer { sdp: String },
    /// ICE candidate for NAT traversal.
    IceCandidate {
        candidate: String,
        sdp_mid: Option<String>,
        sdp_m_line_index: Option<u32>,
    },
}

/// WebRTC signaling messages for screen sharing.
#[cfg(feature = "experimental-collab")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScreenShareSignal {
    /// SDP offer from the host to a viewer.
    Offer { sdp: String },
    /// SDP answer from a viewer to the host.
    Answer { sdp: String },
    /// ICE candidate.
    IceCandidate {
        candidate: String,
        sdp_mid: Option<String>,
        sdp_m_line_index: Option<u32>,
    },
}
