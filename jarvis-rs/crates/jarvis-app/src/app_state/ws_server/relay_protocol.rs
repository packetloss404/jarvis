//! Relay-level envelope messages — separate from the inner PTY protocol.

use serde::{Deserialize, Serialize};

/// First message desktop sends to identify itself.
#[derive(Debug, Serialize)]
pub struct DesktopHello {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub session_id: String,
}

impl DesktopHello {
    pub fn new(session_id: String) -> Self {
        Self {
            msg_type: "desktop_hello",
            session_id,
        }
    }
}

/// First message a pair-Room member sends to join an N:N relay room.
///
/// The relay's `connection.rs` wires `room_hello{...}` → `room_ready` and fans
/// out `member_joined`/`member_left`/`member_count`. The relay now REQUIRES the
/// hello to be SIGNED (member-id slot DoS fix): it verifies `sig` (base64 P1363
/// ECDSA-P256) over the canonical bytes against `pubkey`, range-checks `nonce`
/// (unix millis) for freshness, and TOFU-pins `member_id → pubkey`. Build this
/// via [`RoomHello::signed`], which mirrors the relay's
/// `jarvis_relay::protocol::signed_hello_payload` byte-for-byte.
#[derive(Debug, Serialize)]
pub struct RoomHello {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub session_id: String,
    pub member_id: String,
    /// Signer's ECDSA identity public key, SPKI DER, base64
    /// (`CryptoService::pubkey_base64`).
    pub pubkey: String,
    /// Freshness nonce: unix-epoch MILLISECONDS.
    pub nonce: u64,
    /// Base64 IEEE-P1363 ECDSA-P256 signature over the canonical payload.
    pub sig: String,
}

/// Crypto domain separator for the signed `room_hello`. MUST equal
/// `jarvis_relay::protocol::ROOM_HELLO_SIG_DOMAIN`.
pub const ROOM_HELLO_SIG_DOMAIN: &str = "jarvis-room-hello-v1";
/// Canonical field separator (ASCII Unit Separator). MUST equal the relay's
/// `ROOM_HELLO_SEP`.
pub const ROOM_HELLO_SEP: u8 = 0x1F;

impl RoomHello {
    /// Build a SIGNED `room_hello` using the app's ECDSA identity signer.
    ///
    /// Computes a fresh `nonce` (unix millis), the canonical signing bytes (see
    /// module-level docs / the relay's `room_hello_canonical_bytes`), signs
    /// `base64(canonical_bytes)` with the [`jarvis_platform::PairFrameSigner`]
    /// (`sign_bytes` → base64 P1363, exactly what `CryptoService::verify`
    /// accepts), and assembles the wire struct.
    pub fn signed(
        session_id: String,
        member_id: String,
        signer: &jarvis_platform::PairFrameSigner,
    ) -> Self {
        let pubkey = signer.pubkey_base64.clone();
        let nonce = now_millis();
        let payload = signed_hello_payload(&session_id, &member_id, &pubkey, nonce);
        let sig = signer.sign_bytes(payload.as_bytes());
        Self {
            msg_type: "room_hello",
            session_id,
            member_id,
            pubkey,
            nonce,
            sig,
        }
    }
}

/// Canonical signing bytes for the `room_hello` — identical to the relay's
/// `jarvis_relay::protocol::room_hello_canonical_bytes`.
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

/// `base64(canonical_bytes)` — the exact string fed to ECDSA sign/verify. MUST
/// equal `jarvis_relay::protocol::signed_hello_payload`.
pub fn signed_hello_payload(session_id: &str, member_id: &str, pubkey: &str, nonce: u64) -> String {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    B64.encode(room_hello_canonical_bytes(session_id, member_id, pubkey, nonce))
}

/// Current unix-epoch MILLISECONDS (the `nonce` unit).
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Messages the relay sends back to us.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum RelayResponse {
    #[serde(rename = "session_ready")]
    SessionReady { session_id: String },

    #[serde(rename = "peer_connected")]
    PeerConnected,

    #[serde(rename = "peer_disconnected")]
    PeerDisconnected,

    #[serde(rename = "error")]
    Error { message: String },
}

/// Envelope for messages forwarded through the relay.
/// Carries either an `encrypted` (E2E ciphertext via [`RelayEnvelope::Encrypted`]),
/// `key_exchange`, or `plaintext` (pre-encryption setup) variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RelayEnvelope {
    /// Key exchange: send our DH public key to the peer.
    #[serde(rename = "key_exchange")]
    KeyExchange { dh_pubkey: String },

    /// Encrypted payload — inner PTY protocol message.
    #[serde(rename = "encrypted")]
    Encrypted { iv: String, ct: String },

    /// Plaintext PTY protocol message (used before encryption is set up).
    #[serde(rename = "plaintext")]
    Plaintext { payload: String },
}

#[cfg(test)]
mod wire_conformance_tests {
    use super::RelayResponse;

    const SESSION_READY_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/relay/session_ready.json"
    ));

    #[test]
    fn session_ready_deserializes_shared_fixture() {
        let r: RelayResponse = serde_json::from_str(SESSION_READY_FIXTURE.trim()).unwrap();
        match r {
            RelayResponse::SessionReady { session_id } => assert_eq!(session_id, "test-sid"),
            _ => panic!("expected SessionReady"),
        }
    }
}

#[cfg(test)]
mod signed_room_hello_tests {
    use super::{room_hello_canonical_bytes, signed_hello_payload, RoomHello};
    use jarvis_platform::CryptoService;

    /// The canonical bytes are domain-tagged and 0x1F-delimited exactly as the
    /// relay's `jarvis_relay::protocol::room_hello_canonical_bytes` builds them.
    #[test]
    fn canonical_bytes_match_spec() {
        let bytes = room_hello_canonical_bytes("sid", "m1", "pk", 42);
        assert_eq!(bytes, b"jarvis-room-hello-v1\x1Fsid\x1Fm1\x1Fpk\x1F42");
    }

    /// A signed hello produced by `RoomHello::signed` carries a signature that
    /// verifies against the carried pubkey over the SAME payload the relay
    /// recomputes — i.e. this client and the relay verifier agree byte-for-byte.
    #[test]
    fn signed_hello_verifies_against_carried_pubkey() {
        let crypto = CryptoService::generate().unwrap();
        let signer = crypto.pair_frame_signer();

        let hello = RoomHello::signed("sessionXYZ".into(), "member-7".into(), &signer);
        assert_eq!(hello.msg_type, "room_hello");
        assert_eq!(hello.pubkey, crypto.pubkey_base64);

        // Recompute the payload the relay would verify and check the signature.
        let payload =
            signed_hello_payload(&hello.session_id, &hello.member_id, &hello.pubkey, hello.nonce);
        assert!(
            crypto.verify(&payload, &hello.sig, &hello.pubkey).unwrap(),
            "RoomHello::signed sig must verify over the canonical payload"
        );
    }
}
