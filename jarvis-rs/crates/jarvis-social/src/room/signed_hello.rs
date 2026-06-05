//! Signed `room_hello` construction (client side).
//!
//! The relay now REQUIRES every Room client to sign its `room_hello` with its
//! ECDSA P-256 identity (member-id slot DoS fix). This module builds the EXACT
//! canonical bytes / payload / wire JSON the relay's `room_auth` verifier
//! expects, so the desktop PRESENCE client (and, by mirroring this spec, the
//! other three Room clients) all produce a byte-identical signed hello.
//!
//! ## Canonical signing bytes (MUST match relay + all four clients)
//! ```text
//! "jarvis-room-hello-v1" ‹0x1F› session_id ‹0x1F› member_id ‹0x1F› pubkey ‹0x1F› nonce(decimal)
//! ```
//! The signature is taken over `base64(canonical_bytes)` (`signed_hello_payload`)
//! so it passes through the project's `&str`-based ECDSA sign/verify surface
//! unchanged, matching the `SignedPairFrame` precedent.
//!
//! ## Signer seam
//! jarvis-social has no crypto stack of its own. The caller (jarvis-app, which
//! owns the `CryptoService` ECDSA identity) injects a [`RoomHelloSigner`]: its
//! `pubkey` plus a closure that signs a payload string and returns the base64
//! P1363 signature — i.e. `CryptoService::sign` / `PairFrameSigner::sign_bytes`.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;

/// Crypto domain separator for the signed `room_hello`. MUST equal
/// `jarvis_relay::protocol::ROOM_HELLO_SIG_DOMAIN` byte-for-byte.
pub const ROOM_HELLO_SIG_DOMAIN: &str = "jarvis-room-hello-v1";

/// Canonical field separator (ASCII Unit Separator). MUST equal the relay's
/// `ROOM_HELLO_SEP`.
pub const ROOM_HELLO_SEP: u8 = 0x1F;

/// Build the canonical signing bytes (see module docs). Kept identical to
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

/// The ASCII string actually signed/verified: `base64(canonical_bytes)`. MUST
/// equal `jarvis_relay::protocol::signed_hello_payload`.
pub fn signed_hello_payload(session_id: &str, member_id: &str, pubkey: &str, nonce: u64) -> String {
    B64.encode(room_hello_canonical_bytes(session_id, member_id, pubkey, nonce))
}

/// Current unix-epoch time in MILLISECONDS — the `nonce` unit the relay
/// range-checks for freshness.
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Injected ECDSA signer for the Room hello. Holds the identity `pubkey` (SPKI
/// DER, base64) and a closure that signs a payload string with that identity,
/// returning a base64 IEEE-P1363 signature — i.e. exactly
/// `jarvis_platform::PairFrameSigner::sign_bytes(payload.as_bytes())` /
/// `CryptoService::sign(payload)`.
///
/// The closure is `Send + Sync` so the room background task can hold it across
/// reconnects.
pub struct RoomHelloSigner {
    /// Signer's ECDSA identity public key, SPKI DER, base64.
    pub pubkey: String,
    sign_fn: Box<dyn Fn(&str) -> String + Send + Sync>,
}

impl RoomHelloSigner {
    /// Construct from a pubkey and a signing closure.
    pub fn new(pubkey: String, sign_fn: Box<dyn Fn(&str) -> String + Send + Sync>) -> Self {
        Self { pubkey, sign_fn }
    }

    /// Produce the base64 signature for a given payload string.
    pub fn sign(&self, payload: &str) -> String {
        (self.sign_fn)(payload)
    }
}

impl std::fmt::Debug for RoomHelloSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoomHelloSigner")
            .field("pubkey", &self.pubkey)
            .finish_non_exhaustive()
    }
}

/// The fully-built, signed `room_hello` wire JSON string. Computes a fresh
/// `nonce` (unix millis), the canonical payload, and the signature, then
/// serializes the relay's `RoomHello` shape:
/// `{type, session_id, member_id, pubkey, nonce, sig}`.
pub fn build_signed_room_hello(
    signer: &RoomHelloSigner,
    session_id: &str,
    member_id: &str,
) -> String {
    let nonce = now_millis();
    let payload = signed_hello_payload(session_id, member_id, &signer.pubkey, nonce);
    let sig = signer.sign(&payload);
    serde_json::json!({
        "type": "room_hello",
        "session_id": session_id,
        "member_id": member_id,
        "pubkey": signer.pubkey,
        "nonce": nonce,
        "sig": sig,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_bytes_are_separator_delimited() {
        let bytes = room_hello_canonical_bytes("sid", "m1", "pk", 42);
        let expected = b"jarvis-room-hello-v1\x1Fsid\x1Fm1\x1Fpk\x1F42";
        assert_eq!(bytes, expected);
    }

    #[test]
    fn payload_is_base64_of_canonical() {
        let p = signed_hello_payload("sid", "m1", "pk", 42);
        let decoded = B64.decode(p).unwrap();
        assert_eq!(decoded, room_hello_canonical_bytes("sid", "m1", "pk", 42));
    }

    #[test]
    fn build_signed_room_hello_has_all_wire_fields() {
        let signer = RoomHelloSigner::new(
            "PUBKEY".into(),
            Box::new(|_payload: &str| "SIGNATURE".to_string()),
        );
        let wire = build_signed_room_hello(&signer, "sid", "m1");
        let v: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(v["type"], "room_hello");
        assert_eq!(v["session_id"], "sid");
        assert_eq!(v["member_id"], "m1");
        assert_eq!(v["pubkey"], "PUBKEY");
        assert_eq!(v["sig"], "SIGNATURE");
        assert!(v["nonce"].is_u64());
    }

    /// END-TO-END (real ECDSA): the presence client signs `room_hello` with the
    /// app's actual `CryptoService` identity (via the injected `RoomHelloSigner`),
    /// and the produced `sig` verifies against the carried `pubkey` over the SAME
    /// canonical payload the relay's `room_auth` recomputes — i.e. this client and
    /// the relay verifier agree byte-for-byte. This is the slot-DoS-fix contract.
    #[test]
    fn presence_signed_hello_verifies_against_same_canonical_bytes() {
        use jarvis_platform::CryptoService;

        // The app owns the identity; it wires the signer exactly as
        // `app_state/social.rs` does: pubkey + a `sign(payload)` closure backed by
        // the real ECDSA `PairFrameSigner`.
        let crypto = CryptoService::generate().unwrap();
        let pfs = crypto.pair_frame_signer();
        let signer = RoomHelloSigner::new(
            pfs.pubkey_base64.clone(),
            Box::new(move |payload: &str| pfs.sign_bytes(payload.as_bytes())),
        );

        // The presence member_id is the desktop's stable user id (a UUID) — left
        // untouched by the TOFU binding the foundation chose.
        let session_id = "presence-room-session-0123456789ab";
        let member_id = "2f1c8e64-9a3b-4d77-8e21-0a1b2c3d4e5f"; // user_id (UUID)

        let wire = build_signed_room_hello(&signer, session_id, member_id);
        let v: serde_json::Value = serde_json::from_str(&wire).unwrap();

        // Wire shape: {type, session_id, member_id, pubkey, nonce, sig}.
        assert_eq!(v["type"], "room_hello");
        assert_eq!(v["session_id"], session_id);
        assert_eq!(v["member_id"], member_id);
        assert_eq!(v["pubkey"], crypto.pubkey_base64);
        let nonce = v["nonce"].as_u64().expect("nonce is unix-millis u64");
        let sig = v["sig"].as_str().expect("sig present").to_string();

        // Recompute EXACTLY what the relay's room_auth verifies: base64 of the
        // domain-tagged, 0x1F-delimited canonical bytes.
        let payload = signed_hello_payload(session_id, member_id, &crypto.pubkey_base64, nonce);
        // Canonical bytes are reproducible from the same inputs.
        assert_eq!(
            B64.decode(&payload).unwrap(),
            room_hello_canonical_bytes(session_id, member_id, &crypto.pubkey_base64, nonce),
        );
        // The real ECDSA signature verifies against the carried pubkey.
        assert!(
            crypto.verify(&payload, &sig, &crypto.pubkey_base64).unwrap(),
            "presence room_hello sig must verify over the canonical payload"
        );

        // Negative: a one-byte tamper of the payload (different member_id) must
        // NOT verify under the same sig — binds the slot to (session,member,key).
        let tampered = signed_hello_payload(session_id, "someone-else", &crypto.pubkey_base64, nonce);
        assert!(
            !crypto.verify(&tampered, &sig, &crypto.pubkey_base64).unwrap(),
            "sig must not verify over a different member_id"
        );
    }
}
