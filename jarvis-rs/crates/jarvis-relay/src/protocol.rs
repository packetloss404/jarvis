//! Relay-level wire protocol. Only the first message is parsed; everything after
//! is forwarded as opaque text frames.

use serde::{Deserialize, Serialize};

/// Crypto domain separator for the SIGNED `room_hello`. A fixed, versioned tag
/// prepended to the canonical signing bytes so a room-hello signature can never
/// be cross-presented to any other signer that shares the same ECDSA identity
/// key (pair frames use `jarvis-pair-sig-v1`; these spaces are disjoint).
///
/// This EXACT string must be reproduced byte-for-byte by all four Room clients
/// (desktop pair, desktop presence, desktop chat JS, mobile chat JS).
pub const ROOM_HELLO_SIG_DOMAIN: &str = "jarvis-room-hello-v1";

/// Field separator inside the canonical signing bytes (ASCII Unit Separator).
/// Disjoint from base64 / hostnames / member-id charsets, so the delimited
/// fields are unambiguous. Mirrors the pair-frame canonicalization (`0x1F`).
pub const ROOM_HELLO_SEP: u8 = 0x1F;

/// Build the CANONICAL, deterministic byte string a Room member signs in its
/// `room_hello`:
///
/// ```text
/// ROOM_HELLO_SIG_DOMAIN ‹0x1F› session_id ‹0x1F› member_id ‹0x1F› pubkey ‹0x1F› nonce(decimal)
/// ```
///
/// - `ROOM_HELLO_SIG_DOMAIN` — fixed crypto domain separator (see above).
/// - `session_id` — binds the signature to THIS room, so a captured hello can't
///   be replayed into a different session.
/// - `member_id` AND `pubkey` — bound together, so the slot the signature claims
///   (`member_id`) is cryptographically tied to the signing key (`pubkey`); the
///   relay then enforces TOFU `member_id → pubkey` pinning on top.
/// - `nonce` — a freshness value (unix-epoch MILLISECONDS as a decimal u64) the
///   relay range-checks against its own clock, so a hello captured off the wire
///   cannot be replayed outside a short window.
///
/// The four clients and the relay verifier MUST produce identical bytes. The
/// signature is taken over `base64(canonical_bytes)` (see [`signed_hello_payload`])
/// so it passes unchanged through the project's `&str`-based ECDSA sign/verify
/// surface (`CryptoService::{sign,verify}` desktop, Web Crypto mobile), matching
/// the `SignedPairFrame` precedent.
pub fn room_hello_canonical_bytes(
    session_id: &str,
    member_id: &str,
    pubkey: &str,
    nonce: u64,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(
        ROOM_HELLO_SIG_DOMAIN.len() + session_id.len() + member_id.len() + pubkey.len() + 32,
    );
    buf.extend_from_slice(ROOM_HELLO_SIG_DOMAIN.as_bytes());
    buf.push(ROOM_HELLO_SEP);
    buf.extend_from_slice(session_id.as_bytes());
    buf.push(ROOM_HELLO_SEP);
    buf.extend_from_slice(member_id.as_bytes());
    buf.push(ROOM_HELLO_SEP);
    buf.extend_from_slice(pubkey.as_bytes());
    buf.push(ROOM_HELLO_SEP);
    buf.extend_from_slice(nonce.to_string().as_bytes());
    buf
}

/// The exact ASCII string that is actually fed to ECDSA sign/verify: the
/// standard-base64 encoding of [`room_hello_canonical_bytes`]. Both the four
/// clients and the relay sign/verify over THIS string (not the raw bytes), so
/// the canonical bytes round-trip losslessly through the `&str` crypto APIs.
pub fn signed_hello_payload(session_id: &str, member_id: &str, pubkey: &str, nonce: u64) -> String {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    B64.encode(room_hello_canonical_bytes(session_id, member_id, pubkey, nonce))
}

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
        /// Signer's ECDSA P-256 identity public key, SPKI DER, base64. Same
        /// encoding as `CryptoService::pubkey_base64` / Web Crypto
        /// `exportKey('spki')`. The relay verifies `sig` against this key and
        /// TOFU-pins `member_id → pubkey`.
        pubkey: String,
        /// Freshness value: unix-epoch MILLISECONDS as a u64. The relay rejects
        /// a hello whose nonce is outside its accept window (anti-replay).
        nonce: u64,
        /// Base64 IEEE-P1363 (r||s, 64-byte) ECDSA-P256 signature over
        /// `base64(room_hello_canonical_bytes(session_id, member_id, pubkey, nonce))`
        /// — see [`signed_hello_payload`].
        sig: String,
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
