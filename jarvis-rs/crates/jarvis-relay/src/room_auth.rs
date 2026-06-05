//! Signed `room_hello` verification: bind a Room slot to a cryptographic
//! identity so a self-asserted `member_id` alone can no longer squat or evict a
//! slot (the member-id slot DoS).
//!
//! ## What this enforces
//! Every Room client signs its `room_hello` with its existing ECDSA P-256
//! identity. Before a member is admitted / registered / replaced, the relay:
//!
//! 1. Verifies the ECDSA signature over the canonical bytes
//!    ([`crate::protocol::signed_hello_payload`]) against the carried `pubkey`.
//! 2. Checks `nonce` freshness against the relay clock (anti-replay window).
//! 3. Enforces the `member_id → pubkey` binding via **relay-side TOFU**: the
//!    first signed join for a `(session_id, member_id)` PINS its pubkey; any
//!    later join/reconnect/replace for that `member_id` MUST present the same
//!    pubkey (and a valid signature).
//!
//! ## Binding choice: TOFU pinning (not self-authenticating member_id)
//!
//! The four Room clients derive `member_id` heterogeneously and NONE of them is
//! a pure function of the ECDSA pubkey:
//!
//!   - desktop PAIR    → `random_alnum(16)` (no pubkey relationship at all);
//!   - desktop PRESENCE→ `identity.user_id` (a UUIDv4, no pubkey relationship);
//!   - desktop chat JS → `fingerprint(pubkey).hex + "." + userId` (UUID tail);
//!   - mobile chat JS  → same `fingerprint.hex + "." + userId`.
//!
//! Making `member_id == fingerprint(pubkey)` would change all four formats AND
//! sever the `member_id ↔ user_id` linkage that presence rosters, DM channel
//! names, and the pair-frame `from` fields depend on — a large blast radius.
//! TOFU pinning binds the slot to the key while leaving every `member_id`
//! format untouched (zero client-format blast radius), so it is the chosen
//! mechanism. (The `fingerprint.` prefix on the chat ids still makes squatting
//! a *specific* id require the matching key, and the pin closes the rest.)
//!
//! ## Fingerprinted ids: pre-pin fingerprint check (closes first-mover squat)
//!
//! For the chat clients whose `member_id` is `fingerprint(pubkey).hex + "." +
//! userId`, the relay additionally requires the leading fingerprint segment to
//! equal `fingerprint(carried_pubkey)` (sha256(SPKI)[..8] hex). This closes the
//! first-mover squat on those ids: an attacker cannot be the first to pin a
//! fingerprinted id without holding the key whose fingerprint the id embeds.
//! The non-fingerprinted formats (pair `random_alnum`, presence raw UUID) carry
//! no key relationship, so they retain a RESIDUAL first-mover-pins risk: the
//! first valid signer to present such an id pins it (any later, differently-keyed
//! claimant is then refused). See `dev/ROADMAP.md`.
//!
//! ## Replay / monotonic nonce
//!
//! Each pinned slot also stores the highest `nonce` it has accepted. A hello is
//! rejected unless its `nonce` is STRICTLY GREATER than the last accepted nonce
//! for that slot, so a captured hello replayed within the freshness window can
//! no longer evict the live connection holding the slot. A genuine reconnect
//! uses a fresh (higher unix-millis) nonce and is accepted.
//!
//! ## Verify / commit split (no orphan pins)
//!
//! Verification is a pure read ([`RoomAuthStore::verify`]); the pin + nonce
//! high-water are mutated only by [`RoomAuthStore::commit`], which the
//! connection handler calls ONLY after the slot is actually registered. Any
//! early return after verify (capacity, `ensure_session`, register, or the
//! first `send` failing) leaves NO pin / nonce state behind.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::RwLock;

use crate::protocol::signed_hello_payload;

/// How far a `room_hello` nonce (unix-epoch millis) may deviate from the relay
/// clock, in milliseconds, in EITHER direction. This is a SANITY freshness
/// bound only — the real anti-replay guarantee is the per-slot STRICTLY
/// MONOTONIC nonce (see [`RoomAuthStore`]). Kept tight to limit how far in the
/// past/future a first (slot-pinning) hello may be dated while still tolerating
/// modest client/relay clock skew.
pub const NONCE_WINDOW_MS: u64 = 30_000; // ±30 seconds

/// Why a signed `room_hello` was rejected. Enumerated so the rules are testable
/// and greppable in relay logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomHelloRejectReason {
    /// `pubkey` / `sig` were not valid base64 / DER / signature encodings.
    MalformedCrypto,
    /// ECDSA signature did not verify against the carried `pubkey` over the
    /// canonical bytes.
    BadSignature,
    /// `nonce` is outside the sanity freshness window (too old or too far in the
    /// future), OR it is not strictly greater than the last nonce accepted for
    /// this slot — i.e. a replayed/older hello. Covers both the stale and the
    /// replay cases (kept a single coarse reason; logs carry the detail).
    StaleNonce,
    /// This `(session_id, member_id)` is already pinned to a DIFFERENT pubkey
    /// (slot squat / eviction attempt by a non-owner of the key).
    PubkeyMismatch,
    /// The `member_id` embeds a pubkey fingerprint prefix (`<fp>.<userId>`) that
    /// does NOT match `fingerprint(carried_pubkey)` — a first-mover squat on a
    /// fingerprinted id by a holder of a different key.
    FingerprintMismatch,
}

impl RoomHelloRejectReason {
    /// A short, client-facing message for the relay `error` response. Kept
    /// deliberately coarse so it does not leak which check failed beyond what an
    /// attacker can already infer.
    pub fn message(&self) -> &'static str {
        match self {
            RoomHelloRejectReason::MalformedCrypto
            | RoomHelloRejectReason::BadSignature
            | RoomHelloRejectReason::StaleNonce => "invalid room hello signature",
            RoomHelloRejectReason::PubkeyMismatch
            | RoomHelloRejectReason::FingerprintMismatch => {
                "member id bound to a different identity"
            }
        }
    }
}

/// What a verified `room_hello` is bound to: its pubkey plus the highest nonce
/// accepted so far for the slot (the per-slot monotonic anti-replay high-water).
#[derive(Clone, Debug)]
struct Pin {
    pubkey: String,
    last_nonce: u64,
}

/// Relay-side TOFU pin store: `(session_id, member_id) -> {pubkey, last_nonce}`.
/// The first valid signed join pins the pubkey and the nonce high-water; later
/// joins for the same slot must present the SAME pubkey AND a STRICTLY GREATER
/// nonce.
///
/// Cloneable handle over shared state (like [`crate::session::SessionStore`]).
/// Pins are pruned when a room empties (see [`RoomAuthStore::forget_session`]),
/// so the map does not grow without bound.
///
/// Callers use a two-phase protocol: [`verify`](Self::verify) (pure, no
/// mutation) BEFORE attempting to register the slot, then [`commit`](Self::commit)
/// AFTER the slot is actually registered. This keeps a forged/early-rejected
/// hello from leaving an orphan pin or advancing the nonce high-water.
#[derive(Clone, Default)]
pub struct RoomAuthStore {
    pins: Arc<RwLock<HashMap<String, HashMap<String, Pin>>>>,
}

impl RoomAuthStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// PURE verification of a signed `room_hello` — performs NO mutation.
    ///
    /// The connection handler calls this BEFORE attempting to admit / register
    /// the slot. On `Ok(())` the hello is cryptographically valid AND consistent
    /// with the current pin (right key, strictly-newer nonce, fingerprint match);
    /// the caller may proceed to register and then [`commit`](Self::commit). On
    /// `Err` the connection must be refused with the reason's
    /// [`message`](RoomHelloRejectReason::message), and nothing is left behind.
    pub async fn verify(
        &self,
        session_id: &str,
        member_id: &str,
        pubkey: &str,
        nonce: u64,
        sig: &str,
    ) -> Result<(), RoomHelloRejectReason> {
        // 1. Sanity freshness: reject a nonce outside ±NONCE_WINDOW_MS of our
        //    clock. (The real anti-replay is the monotonic check in step 4.)
        if !nonce_is_fresh(nonce, now_millis()) {
            return Err(RoomHelloRejectReason::StaleNonce);
        }

        // 2. Signature: ECDSA-P256 over the canonical payload, against pubkey.
        let payload = signed_hello_payload(session_id, member_id, pubkey, nonce);
        match verify_ecdsa_p256(&payload, sig, pubkey) {
            Ok(true) => {}
            Ok(false) => return Err(RoomHelloRejectReason::BadSignature),
            Err(()) => return Err(RoomHelloRejectReason::MalformedCrypto),
        }

        // 3. Fingerprinted-id binding: if member_id is `<fp>.<userId>`, the
        //    leading fp segment must equal fingerprint(pubkey). Closes the
        //    first-mover squat for the chat clients' id format.
        if let Some(prefix) = fingerprint_prefix(member_id) {
            match fingerprint_of_pubkey(pubkey) {
                Some(fp) if fp == prefix => {}
                Some(_) => return Err(RoomHelloRejectReason::FingerprintMismatch),
                None => return Err(RoomHelloRejectReason::MalformedCrypto),
            }
        }

        // 4. TOFU + monotonic-nonce check against the current pin (read-only).
        let map = self.pins.read().await;
        if let Some(pin) = map.get(session_id).and_then(|room| room.get(member_id)) {
            if pin.pubkey != pubkey {
                return Err(RoomHelloRejectReason::PubkeyMismatch);
            }
            // Strictly monotonic: a replay of the same (or older) nonce is a
            // replay and must NOT be able to evict the live slot.
            if nonce <= pin.last_nonce {
                return Err(RoomHelloRejectReason::StaleNonce);
            }
        }
        Ok(())
    }

    /// COMMIT the pin + nonce high-water for a slot the caller has just
    /// successfully registered. Idempotently inserts (first join) or updates
    /// (reconnect) the `(pubkey, last_nonce)` binding.
    ///
    /// Re-checks the same invariants under the write lock so two concurrent
    /// valid hellos for the same slot cannot race a replay through: a commit
    /// whose pubkey no longer matches, or whose nonce is not strictly newer than
    /// what is now stored, is refused (the caller should then tear its slot down).
    /// Only ever called AFTER `verify` succeeded and the slot registered.
    pub async fn commit(
        &self,
        session_id: &str,
        member_id: &str,
        pubkey: &str,
        nonce: u64,
    ) -> Result<(), RoomHelloRejectReason> {
        let mut map = self.pins.write().await;
        let room = map.entry(session_id.to_string()).or_default();
        match room.get_mut(member_id) {
            Some(pin) => {
                if pin.pubkey != pubkey {
                    return Err(RoomHelloRejectReason::PubkeyMismatch);
                }
                if nonce <= pin.last_nonce {
                    return Err(RoomHelloRejectReason::StaleNonce);
                }
                pin.last_nonce = nonce;
            }
            None => {
                room.insert(
                    member_id.to_string(),
                    Pin {
                        pubkey: pubkey.to_string(),
                        last_nonce: nonce,
                    },
                );
            }
        }
        Ok(())
    }

    /// Drop all pins for a session once its room is gone, so the pin map tracks
    /// the live session set. Safe to call unconditionally on room teardown.
    pub async fn forget_session(&self, session_id: &str) {
        self.pins.write().await.remove(session_id);
    }

    /// The pubkey currently pinned for a slot, if any (test/inspection helper).
    #[cfg(test)]
    pub async fn pinned(&self, session_id: &str, member_id: &str) -> Option<String> {
        self.pins
            .read()
            .await
            .get(session_id)
            .and_then(|room| room.get(member_id))
            .map(|pin| pin.pubkey.clone())
    }

    /// The last-accepted nonce for a slot, if any (test/inspection helper).
    #[cfg(test)]
    pub async fn last_nonce(&self, session_id: &str, member_id: &str) -> Option<u64> {
        self.pins
            .read()
            .await
            .get(session_id)
            .and_then(|room| room.get(member_id))
            .map(|pin| pin.last_nonce)
    }
}

/// If `member_id` has the fingerprinted form `<fp>.<rest>` where `<fp>` is a
/// 16-char lowercase-hex pubkey fingerprint (the chat clients' format), return
/// that fingerprint prefix. Other formats (pair `random_alnum`, presence raw
/// UUID — UUIDs contain `-`, not a leading 16-hex `.` segment) return `None` and
/// are left to plain TOFU. The check is intentionally narrow so it only fires
/// for ids that actually embed a fingerprint.
fn fingerprint_prefix(member_id: &str) -> Option<&str> {
    let (head, _rest) = member_id.split_once('.')?;
    let is_fp = head.len() == 16 && head.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
    if is_fp {
        Some(head)
    } else {
        None
    }
}

/// Compute the chat-client fingerprint of a base64 SPKI-DER pubkey: the first 8
/// bytes of `SHA-256(spki_der)` as lowercase hex (16 chars, NO `:` separators —
/// the chat `member_id` strips them). Mirrors
/// `jarvis_platform::crypto::compute_fingerprint` (then `.replace(/:/g,'')`).
/// `None` if the pubkey is not valid base64.
fn fingerprint_of_pubkey(pubkey_b64: &str) -> Option<String> {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    use sha2::{Digest, Sha256};

    let spki_der = B64.decode(pubkey_b64).ok()?;
    let hash = Sha256::digest(spki_der);
    let mut s = String::with_capacity(16);
    for b in &hash[..8] {
        s.push_str(&format!("{b:02x}"));
    }
    Some(s)
}

/// Current unix-epoch time in MILLISECONDS (the `nonce` unit).
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// True if `nonce` (unix millis) is within ±[`NONCE_WINDOW_MS`] of `now`.
/// Split out (pure) so the window logic is unit-testable without a clock.
fn nonce_is_fresh(nonce: u64, now: u64) -> bool {
    nonce.abs_diff(now) <= NONCE_WINDOW_MS
}

/// Verify a base64 IEEE-P1363 ECDSA-P256 signature over `payload` (an ASCII
/// string) against a base64 SPKI-DER `pubkey`. Mirrors
/// `jarvis_platform::CryptoService::verify` exactly (same curve, same SPKI
/// pubkey encoding, same P1363 signature encoding, same SHA-256 prehash) so a
/// signature any of the four clients produces verifies here byte-for-byte.
///
/// `Ok(true)` valid, `Ok(false)` well-formed but wrong, `Err(())` malformed
/// encoding (mapped to [`RoomHelloRejectReason::MalformedCrypto`]).
fn verify_ecdsa_p256(payload: &str, sig_b64: &str, pubkey_b64: &str) -> Result<bool, ()> {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    use p256::ecdsa::signature::Verifier;
    use p256::ecdsa::{Signature, VerifyingKey};
    use p256::pkcs8::DecodePublicKey;
    use p256::PublicKey;

    let spki_der = B64.decode(pubkey_b64).map_err(|_| ())?;
    let pub_key = PublicKey::from_public_key_der(&spki_der).map_err(|_| ())?;
    let verifying_key = VerifyingKey::from(&pub_key);
    let sig_bytes = B64.decode(sig_b64).map_err(|_| ())?;
    let sig = Signature::from_slice(&sig_bytes).map_err(|_| ())?;
    Ok(verifying_key.verify(payload.as_bytes(), &sig).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    use p256::ecdsa::{signature::Signer, Signature, SigningKey};
    use p256::pkcs8::EncodePublicKey;

    /// A throwaway ECDSA identity used to forge / sign test helloes the same way
    /// the real clients do (sign over `signed_hello_payload`).
    struct TestIdentity {
        key: SigningKey,
        pubkey_b64: String,
    }

    impl TestIdentity {
        fn new() -> Self {
            let key = SigningKey::random(&mut rand::thread_rng());
            let pubkey_b64 = B64.encode(
                key.verifying_key()
                    .to_public_key_der()
                    .unwrap()
                    .as_bytes(),
            );
            Self { key, pubkey_b64 }
        }

        fn sign_hello(&self, session_id: &str, member_id: &str, nonce: u64) -> String {
            let payload = signed_hello_payload(session_id, member_id, &self.pubkey_b64, nonce);
            let sig: Signature = self.key.sign(payload.as_bytes());
            B64.encode(sig.to_bytes())
        }

        /// The fingerprinted chat-client member_id for this identity:
        /// `fingerprint(pubkey).hex + "." + user`.
        fn chat_member_id(&self, user: &str) -> String {
            format!("{}.{}", fingerprint_of_pubkey(&self.pubkey_b64).unwrap(), user)
        }
    }

    /// Mirror the connection-handler flow: PURE verify, then (only on success)
    /// commit the pin + nonce high-water. Most tests exercise this combined
    /// path; the verify/commit split is tested explicitly in the ordering tests.
    async fn verify_then_commit(
        store: &RoomAuthStore,
        session_id: &str,
        member_id: &str,
        pubkey: &str,
        nonce: u64,
        sig: &str,
    ) -> Result<(), RoomHelloRejectReason> {
        store
            .verify(session_id, member_id, pubkey, nonce, sig)
            .await?;
        store.commit(session_id, member_id, pubkey, nonce).await
    }

    #[test]
    fn nonce_window_accepts_within_and_rejects_outside() {
        let now = 1_000_000_000_000u64;
        assert!(nonce_is_fresh(now, now));
        assert!(nonce_is_fresh(now + NONCE_WINDOW_MS, now));
        assert!(nonce_is_fresh(now - NONCE_WINDOW_MS, now));
        assert!(!nonce_is_fresh(now + NONCE_WINDOW_MS + 1, now));
        assert!(!nonce_is_fresh(now - NONCE_WINDOW_MS - 1, now));
    }

    #[tokio::test]
    async fn first_signed_join_pins_and_verifies() {
        let store = RoomAuthStore::new();
        let id = TestIdentity::new();
        let nonce = now_millis();
        let sig = id.sign_hello("sid", "m1", nonce);

        assert_eq!(
            verify_then_commit(&store, "sid", "m1", &id.pubkey_b64, nonce, &sig)
                .await,
            Ok(())
        );
        assert_eq!(store.pinned("sid", "m1").await.as_deref(), Some(id.pubkey_b64.as_str()));
    }

    #[tokio::test]
    async fn same_key_reconnect_accepted() {
        let store = RoomAuthStore::new();
        let id = TestIdentity::new();

        let n1 = now_millis();
        let s1 = id.sign_hello("sid", "m1", n1);
        verify_then_commit(&store, "sid", "m1", &id.pubkey_b64, n1, &s1).await.unwrap();

        let n2 = now_millis() + 1;
        let s2 = id.sign_hello("sid", "m1", n2);
        assert_eq!(
            verify_then_commit(&store, "sid", "m1", &id.pubkey_b64, n2, &s2).await,
            Ok(())
        );
    }

    #[tokio::test]
    async fn squatter_with_different_key_rejected() {
        let store = RoomAuthStore::new();
        let owner = TestIdentity::new();
        let squatter = TestIdentity::new();

        let n1 = now_millis();
        let s1 = owner.sign_hello("sid", "m1", n1);
        verify_then_commit(&store, "sid", "m1", &owner.pubkey_b64, n1, &s1).await.unwrap();

        // The squatter signs a perfectly valid hello WITH ITS OWN key for the
        // same member_id — internally consistent, but the slot is pinned to the
        // owner's key, so the relay rejects on the binding (not the signature).
        let n2 = now_millis() + 1;
        let s2 = squatter.sign_hello("sid", "m1", n2);
        assert_eq!(
            verify_then_commit(&store, "sid", "m1", &squatter.pubkey_b64, n2, &s2).await,
            Err(RoomHelloRejectReason::PubkeyMismatch)
        );
    }

    #[tokio::test]
    async fn bad_signature_rejected() {
        let store = RoomAuthStore::new();
        let id = TestIdentity::new();
        let nonce = now_millis();
        // Sign the WRONG member_id, then claim "m1".
        let sig = id.sign_hello("sid", "other", nonce);
        assert_eq!(
            verify_then_commit(&store, "sid", "m1", &id.pubkey_b64, nonce, &sig).await,
            Err(RoomHelloRejectReason::BadSignature)
        );
    }

    #[tokio::test]
    async fn presented_pubkey_not_signing_key_rejected() {
        // Attacker presents the VICTIM's pubkey (to try to pin/match the
        // victim's slot) but signs with its OWN key (it has no victim private
        // key). The payload binds the presented `pubkey`, so the signature is
        // over the victim's pubkey string yet produced by the wrong key — it
        // must fail signature verification BEFORE any pinning happens.
        let store = RoomAuthStore::new();
        let victim = TestIdentity::new();
        let attacker = TestIdentity::new();
        let nonce = now_millis();
        // Sign the payload that carries the VICTIM's pubkey, but with the
        // attacker's signing key.
        let payload = signed_hello_payload("sid", "m1", &victim.pubkey_b64, nonce);
        let forged: Signature = attacker.key.sign(payload.as_bytes());
        let forged_b64 = B64.encode(forged.to_bytes());
        assert_eq!(
            verify_then_commit(&store, "sid", "m1", &victim.pubkey_b64, nonce, &forged_b64)
                .await,
            Err(RoomHelloRejectReason::BadSignature)
        );
        // And nothing was pinned for the slot.
        assert!(store.pinned("sid", "m1").await.is_none());
    }

    #[tokio::test]
    async fn reconnect_with_different_key_rejected() {
        // Distinct from the squat case: the LEGITIMATE owner joins and pins,
        // genuinely reconnects with the SAME key (accepted), and then a holder
        // of a DIFFERENT key tries to reconnect/replace the same slot. The
        // replacement must be refused — a slot, once pinned, can only be
        // re-taken by the private key that first claimed it.
        let store = RoomAuthStore::new();
        let owner = TestIdentity::new();
        let other = TestIdentity::new();

        let n1 = now_millis();
        let s1 = owner.sign_hello("sid", "m1", n1);
        verify_then_commit(&store, "sid", "m1", &owner.pubkey_b64, n1, &s1)
            .await
            .unwrap();

        // Genuine reconnect with the same key still works.
        let n2 = now_millis() + 1;
        let s2 = owner.sign_hello("sid", "m1", n2);
        verify_then_commit(&store, "sid", "m1", &owner.pubkey_b64, n2, &s2)
            .await
            .unwrap();

        // A different key cannot replace the slot on reconnect.
        let n3 = now_millis() + 2;
        let s3 = other.sign_hello("sid", "m1", n3);
        assert_eq!(
            verify_then_commit(&store, "sid", "m1", &other.pubkey_b64, n3, &s3)
                .await,
            Err(RoomHelloRejectReason::PubkeyMismatch)
        );
        // The pin still belongs to the owner.
        assert_eq!(
            store.pinned("sid", "m1").await.as_deref(),
            Some(owner.pubkey_b64.as_str())
        );
    }

    #[tokio::test]
    async fn different_members_in_same_session_pin_independently() {
        // Two distinct members in the same room pin their own keys; one cannot
        // affect the other's binding.
        let store = RoomAuthStore::new();
        let a = TestIdentity::new();
        let b = TestIdentity::new();
        let na = now_millis();
        let sa = a.sign_hello("sid", "a", na);
        let nb = now_millis() + 1;
        let sb = b.sign_hello("sid", "b", nb);
        verify_then_commit(&store, "sid", "a", &a.pubkey_b64, na, &sa).await.unwrap();
        verify_then_commit(&store, "sid", "b", &b.pubkey_b64, nb, &sb).await.unwrap();
        assert_eq!(store.pinned("sid", "a").await.as_deref(), Some(a.pubkey_b64.as_str()));
        assert_eq!(store.pinned("sid", "b").await.as_deref(), Some(b.pubkey_b64.as_str()));
    }

    #[tokio::test]
    async fn stale_nonce_rejected() {
        let store = RoomAuthStore::new();
        let id = TestIdentity::new();
        let stale = now_millis() - NONCE_WINDOW_MS - 5_000;
        let sig = id.sign_hello("sid", "m1", stale);
        assert_eq!(
            verify_then_commit(&store, "sid", "m1", &id.pubkey_b64, stale, &sig).await,
            Err(RoomHelloRejectReason::StaleNonce)
        );
    }

    #[tokio::test]
    async fn malformed_pubkey_rejected() {
        let store = RoomAuthStore::new();
        let nonce = now_millis();
        assert_eq!(
            verify_then_commit(&store, "sid", "m1", "not-base64-!!", nonce, "AAAA")
                .await,
            Err(RoomHelloRejectReason::MalformedCrypto)
        );
    }

    #[tokio::test]
    async fn forget_session_drops_pins() {
        let store = RoomAuthStore::new();
        let id = TestIdentity::new();
        let nonce = now_millis();
        let sig = id.sign_hello("sid", "m1", nonce);
        verify_then_commit(&store, "sid", "m1", &id.pubkey_b64, nonce, &sig).await.unwrap();
        store.forget_session("sid").await;
        assert!(store.pinned("sid", "m1").await.is_none());
    }

    // -----------------------------------------------------------------------
    // Monotonic-nonce anti-replay (HIGH finding #3)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn replayed_same_nonce_rejected() {
        // A captured, perfectly valid hello replayed with the SAME nonce must be
        // refused once the slot is pinned — it can no longer evict the live slot.
        let store = RoomAuthStore::new();
        let id = TestIdentity::new();
        let n1 = now_millis();
        let s1 = id.sign_hello("sid", "m1", n1);
        verify_then_commit(&store, "sid", "m1", &id.pubkey_b64, n1, &s1)
            .await
            .unwrap();

        // Exact replay (same nonce, same sig) — rejected by the monotonic check.
        assert_eq!(
            store.verify("sid", "m1", &id.pubkey_b64, n1, &s1).await,
            Err(RoomHelloRejectReason::StaleNonce)
        );
        // The high-water is unchanged.
        assert_eq!(store.last_nonce("sid", "m1").await, Some(n1));
    }

    #[tokio::test]
    async fn older_nonce_rejected() {
        // A replay dated EARLIER than the last accepted nonce (still inside the
        // freshness window) is likewise refused.
        let store = RoomAuthStore::new();
        let id = TestIdentity::new();
        let n1 = now_millis();
        let s1 = id.sign_hello("sid", "m1", n1);
        verify_then_commit(&store, "sid", "m1", &id.pubkey_b64, n1, &s1)
            .await
            .unwrap();

        let older = n1 - 5; // still fresh, but <= last_nonce
        let s_old = id.sign_hello("sid", "m1", older);
        assert_eq!(
            store.verify("sid", "m1", &id.pubkey_b64, older, &s_old).await,
            Err(RoomHelloRejectReason::StaleNonce)
        );
        assert_eq!(store.last_nonce("sid", "m1").await, Some(n1));
    }

    #[tokio::test]
    async fn strictly_newer_reconnect_accepted() {
        // A genuine reconnect uses a fresh, strictly-greater nonce and is
        // accepted, advancing the high-water.
        let store = RoomAuthStore::new();
        let id = TestIdentity::new();
        let n1 = now_millis();
        let s1 = id.sign_hello("sid", "m1", n1);
        verify_then_commit(&store, "sid", "m1", &id.pubkey_b64, n1, &s1)
            .await
            .unwrap();

        let n2 = n1 + 1; // strictly newer
        let s2 = id.sign_hello("sid", "m1", n2);
        assert_eq!(
            verify_then_commit(&store, "sid", "m1", &id.pubkey_b64, n2, &s2).await,
            Ok(())
        );
        assert_eq!(store.last_nonce("sid", "m1").await, Some(n2));
    }

    // -----------------------------------------------------------------------
    // Verify / commit ordering (MEDIUM finding #4)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn verify_alone_leaves_no_pin() {
        // PURE verify must not mutate: a valid hello that is verified but never
        // committed (e.g. the connection is then rejected on capacity) leaves NO
        // pin and NO nonce high-water behind.
        let store = RoomAuthStore::new();
        let id = TestIdentity::new();
        let nonce = now_millis();
        let sig = id.sign_hello("sid", "m1", nonce);
        store
            .verify("sid", "m1", &id.pubkey_b64, nonce, &sig)
            .await
            .unwrap();
        assert!(store.pinned("sid", "m1").await.is_none());
        assert!(store.last_nonce("sid", "m1").await.is_none());
    }

    #[tokio::test]
    async fn commit_advances_high_water_only_after_register() {
        // The first commit pins; a later commit with a strictly-newer nonce
        // advances the high-water; a stale-nonce commit is refused under the
        // write lock (the concurrent-race guard).
        let store = RoomAuthStore::new();
        let id = TestIdentity::new();
        let n1 = now_millis();
        store.commit("sid", "m1", &id.pubkey_b64, n1).await.unwrap();
        assert_eq!(store.last_nonce("sid", "m1").await, Some(n1));

        // Stale commit (same nonce) refused — models a lost pin-commit race.
        assert_eq!(
            store.commit("sid", "m1", &id.pubkey_b64, n1).await,
            Err(RoomHelloRejectReason::StaleNonce)
        );
        // Different-key commit refused too.
        let other = TestIdentity::new();
        assert_eq!(
            store.commit("sid", "m1", &other.pubkey_b64, n1 + 1).await,
            Err(RoomHelloRejectReason::PubkeyMismatch)
        );
        // Newer same-key commit advances.
        store.commit("sid", "m1", &id.pubkey_b64, n1 + 1).await.unwrap();
        assert_eq!(store.last_nonce("sid", "m1").await, Some(n1 + 1));
    }

    // -----------------------------------------------------------------------
    // Fingerprinted-id binding (LOW finding #5)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn fingerprinted_member_id_matching_key_accepted() {
        // The chat clients' id is `fingerprint(pubkey).hex + "." + userId`. A
        // hello whose member_id embeds the fingerprint of its OWN key is fine.
        let store = RoomAuthStore::new();
        let id = TestIdentity::new();
        let mid = id.chat_member_id("2f1c8e64-9a3b-4d77-8e21-0a1b2c3d4e5f");
        let nonce = now_millis();
        let sig = id.sign_hello("sid", &mid, nonce);
        assert_eq!(
            verify_then_commit(&store, "sid", &mid, &id.pubkey_b64, nonce, &sig).await,
            Ok(())
        );
    }

    #[tokio::test]
    async fn fingerprinted_member_id_wrong_key_rejected_pre_pin() {
        // First-mover squat: an attacker is the FIRST to present a fingerprinted
        // id, but signs with a DIFFERENT key than the fingerprint embeds. Even
        // with no prior pin, the fingerprint check refuses it.
        let store = RoomAuthStore::new();
        let victim = TestIdentity::new();
        let attacker = TestIdentity::new();
        // member_id embeds the VICTIM's fingerprint...
        let mid = victim.chat_member_id("2f1c8e64-9a3b-4d77-8e21-0a1b2c3d4e5f");
        let nonce = now_millis();
        // ...but the hello is signed (validly, internally consistent) by the
        // ATTACKER's key, carrying the attacker's pubkey.
        let sig = attacker.sign_hello("sid", &mid, nonce);
        assert_eq!(
            store.verify("sid", &mid, &attacker.pubkey_b64, nonce, &sig).await,
            Err(RoomHelloRejectReason::FingerprintMismatch)
        );
        assert!(store.pinned("sid", &mid).await.is_none());
    }

    #[test]
    fn fingerprint_prefix_recognizes_only_fingerprinted_ids() {
        // 16 lowercase hex + ".rest" → fingerprinted.
        assert_eq!(
            fingerprint_prefix("0123456789abcdef.user"),
            Some("0123456789abcdef")
        );
        // Pair random_alnum (no dot) → not fingerprinted.
        assert_eq!(fingerprint_prefix("a1B2c3D4e5F6g7H8"), None);
        // Presence raw UUID (dots? no — UUIDs use '-') → not fingerprinted.
        assert_eq!(fingerprint_prefix("2f1c8e64-9a3b-4d77-8e21-0a1b2c3d4e5f"), None);
        // Wrong-length / uppercase hex head → not treated as a fingerprint.
        assert_eq!(fingerprint_prefix("0123456789ABCDEF.user"), None);
        assert_eq!(fingerprint_prefix("0123.user"), None);
    }

    // -----------------------------------------------------------------------
    // Golden-vector conformance (LOW finding #6) — shared with the JS clients.
    // -----------------------------------------------------------------------

    /// FIXED VECTOR: `signed_hello_payload("sid","m1","pk",42)` must equal this
    /// exact base64 string. The JS conformance tests (mobile + desktop) assert
    /// `btoa(canonical("sid","m1","pk",42))` equals the SAME constant, so any
    /// future SEP / domain drift (e.g. the `'\\x1F'` mobile bug) fails the build
    /// on at least one side.
    const GOLDEN_PAYLOAD_SID_M1_PK_42: &str =
        "amFydmlzLXJvb20taGVsbG8tdjEfc2lkH20xH3BrHzQy";

    #[test]
    fn golden_signed_hello_payload_vector() {
        assert_eq!(
            signed_hello_payload("sid", "m1", "pk", 42),
            GOLDEN_PAYLOAD_SID_M1_PK_42
        );
    }
}
