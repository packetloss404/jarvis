//! Inner payload protocol for pair-programming Room sessions.
//!
//! `PairFrame` is the application-level message carried *inside* the opaque
//! [`RelayEnvelope`](super::relay_protocol::RelayEnvelope) — the relay never
//! sees these contents. Host is the source of truth: only the host emits
//! `pty_output`/`resize`/`driver_changed`/`snapshot`/`session_meta`;
//! navigators emit only `term_input`/`cursor`/`request_control`.
//!
//! Wire format: `#[serde(tag = "type", rename_all = "snake_case")]`. Raw byte
//! payloads (terminal data) are base64-encoded strings on the wire.

use serde::{Deserialize, Serialize};

/// Maximum bytes accepted for a single `term_input` keystroke frame. A real
/// keystroke (even a bracketed paste chunk) is tiny; this caps a hostile or
/// buggy navigator from fanning a multi-KB paste-bomb into the host PTY.
pub const MAX_TERM_INPUT_BYTES: usize = 4096;

/// Application-level pair-session message carried inside a `RelayEnvelope`.
///
/// `#[serde(deny_unknown_fields)]` (per variant) is a SECURITY requirement: the
/// signature covers the exact `serde_json` serialization of this frame, so a
/// peer must not be able to append or reorder fields on the wire and keep a
/// valid signature — any unexpected field fails deserialization and the frame is
/// dropped before verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PairFrame {
    /// Host → room: a chunk of the shared terminal's PTY output stream.
    ///
    /// `offset` is the host's MONOTONIC cumulative byte offset at the END of this
    /// chunk (total host PTY bytes emitted so far, including this chunk). A late
    /// joiner replays the join `Snapshot` (which carries the offset it captured
    /// up to) and then DROPS any live `pty_output` whose `offset <= snapshot
    /// offset`, deduping the overlap so the terminal isn't garbled by writes the
    /// snapshot already contains. `#[serde(default)]` keeps wire-compat (a 0
    /// offset disables dedup — the legacy behaviour).
    PtyOutput {
        #[serde(with = "b64")]
        data: Vec<u8>,
        #[serde(default)]
        offset: u64,
    },

    /// Driver → room → host: keystrokes to write into the host PTY.
    TermInput {
        from: String,
        #[serde(with = "b64")]
        data: Vec<u8>,
    },

    /// Any → room: a remote (ghost) cursor position update.
    Cursor { from: String, row: u16, col: u16 },

    /// Host → room: the shared terminal was resized.
    Resize { cols: u16, rows: u16 },

    /// Host → room: the driver seat changed hands.
    DriverChanged {
        new_driver: String,
        old_driver: String,
    },

    /// Navigator → host: ask to take the driver seat.
    RequestControl { from: String },

    /// Navigator → host: announce presence + display name on room join.
    ///
    /// The relay's `member_joined` carries only the opaque hex member id, so a
    /// navigator emits this once it is room-ready to hand the host its
    /// `display_name`. The host calls `join_session` with it (so driver gating
    /// recognises the navigator) and re-broadcasts `SessionMeta` with the
    /// updated roster.
    ///
    /// M3 identity binding: `pubkey` is the navigator's ECDSA identity public
    /// key (SPKI DER, base64). The host pins `from → pubkey` (TOFU) on the first
    /// signed `Join` and thereafter REQUIRES every frame from `from` to verify
    /// against this pubkey. The enclosing [`SignedPairFrame`] also carries this
    /// pubkey and binds it to the signature, so a forged `Join` cannot register
    /// a pubkey the sender does not actually hold the private key for.
    ///
    /// `#[serde(default)]` keeps wire-compat with pre-M3 `Join` frames (empty
    /// pubkey → treated as unauthenticated under the legacy unsigned path).
    Join {
        from: String,
        name: String,
        #[serde(default)]
        pubkey: String,
    },

    /// Host → joining member: a mid-session replay snapshot of terminal state.
    ///
    /// The relay fans every frame out to ALL other members (no per-member
    /// addressing), so a snapshot meant for one late joiner would also reach
    /// existing navigators and force a disruptive `term.reset()` on them. To
    /// avoid that, `target` names the member the snapshot is for: a member
    /// applies it only when `target` is empty (legacy broadcast) or equals its
    /// own `member_id`; everyone else ignores it. `#[serde(default)]` keeps
    /// wire-compat with pre-targeting snapshots.
    Snapshot {
        #[serde(with = "b64")]
        data: Vec<u8>,
        cols: u16,
        rows: u16,
        driver: String,
        #[serde(default)]
        target: String,
        /// The host's monotonic byte offset captured by this snapshot (total host
        /// PTY bytes emitted at the moment it was taken). The joiner replays the
        /// snapshot, then drops live `pty_output` chunks whose `offset <= this`
        /// (dedup). `#[serde(default)]` keeps wire-compat with pre-offset
        /// snapshots (0 → no dedup).
        #[serde(default)]
        offset: u64,
    },

    /// Host → room on create AND on every join: session metadata for joiners.
    ///
    /// Re-broadcast on each `member_joined`/`Join` so late joiners (the relay
    /// does all-but-sender fan-out with no replay) still learn the host name,
    /// dimensions, takeover policy, and the current roster of real display
    /// names — without it a late joiner would render raw hex member ids.
    SessionMeta {
        host: String,
        host_name: String,
        cols: u16,
        rows: u16,
        allow_takeover: bool,
        /// Current participants (host + navigators) with sanitized display
        /// names and roles, so navigators render names instead of member ids.
        #[serde(default)]
        roster: Vec<RosterEntry>,
    },
}

impl PairFrame {
    /// The stable snake_case type tag for this frame (matches the serde `tag`).
    ///
    /// Used as a domain separator inside the canonical signing bytes so a
    /// signature over one frame type can never be replayed as another type.
    pub fn frame_type(&self) -> &'static str {
        match self {
            PairFrame::PtyOutput { .. } => "pty_output",
            PairFrame::TermInput { .. } => "term_input",
            PairFrame::Cursor { .. } => "cursor",
            PairFrame::Resize { .. } => "resize",
            PairFrame::DriverChanged { .. } => "driver_changed",
            PairFrame::RequestControl { .. } => "request_control",
            PairFrame::Join { .. } => "join",
            PairFrame::Snapshot { .. } => "snapshot",
            PairFrame::SessionMeta { .. } => "session_meta",
        }
    }

    /// True iff this frame type is **host-authoritative**: it may only be
    /// honored when the verified signer is the pinned host (see
    /// [`SignedPairFrame`] host-authority rule). Navigator-origin frames
    /// (`term_input`/`cursor`/`request_control`/`join`) return false.
    pub fn is_host_only(&self) -> bool {
        matches!(
            self,
            PairFrame::PtyOutput { .. }
                | PairFrame::Resize { .. }
                | PairFrame::DriverChanged { .. }
                | PairFrame::Snapshot { .. }
                | PairFrame::SessionMeta { .. }
        )
    }
}

// ============================================================================
// M3: END-TO-END SIGNED FRAME ENVELOPE
// ============================================================================
//
// THREAT MODEL (closed by this envelope):
//   1. Impersonation — the inner `PairFrame` carries a self-asserted `from`,
//      and ALL members share one symmetric room key (PBKDF2 of session_id), so
//      any member could forge a frame as the driver and inject keystrokes.
//   2. Stream-spoofing — host-only frames were applied from ANY member with no
//      origin check.
//   3. `require_signed_join` was a no-op.
//
// FIX: every `PairFrame` is wrapped in a `SignedPairFrame` carrying the
// sender's stable `member_id`, its ECDSA identity public key (SPKI base64),
// a per-sender monotonic `seq` (anti-replay), and an ECDSA P-256 signature
// over a CANONICAL byte string. The signed envelope is what gets encrypted
// into the `RelayEnvelope` (confidentiality from the relay is unchanged; the
// signature is the NEW authenticity layer). The relay stays opaque and
// UNCHANGED — verification is purely client-side.

/// Field separator inside the canonical signing byte string. `0x1F` (ASCII
/// Unit Separator) cannot appear in an alnum `session_id`/`member_id`, in a
/// base64 `pubkey`, in a snake_case `frame_type`, or in the decimal
/// `epoch`/`seq`, so the signed fields are unambiguously delimited without
/// length-prefixing.
const CANON_SEP: u8 = 0x1F;

/// CRYPTO DOMAIN SEPARATOR: a fixed scheme tag prefixed to every pair-frame
/// signing input. Because the app's single ECDSA identity key is ALSO used by
/// the relay / pairing / theme signers, this tag guarantees a pair-frame
/// signature can never be cross-presented to (or accepted by) one of those
/// other signers — they sign over different, non-prefixed byte strings. Bump
/// the `v1` suffix if the canonical layout below ever changes.
pub(crate) const PAIR_SIG_DOMAIN: &str = "jarvis-pair-sig-v1";

/// The signed envelope wrapping an inner [`PairFrame`].
///
/// Wire shape (carried *inside* the encrypted `RelayEnvelope`):
/// ```text
/// SignedPairFrame {
///   member_id: String,   // sender's stable in-room id (also the `from` of inner navigator frames)
///   pubkey:    String,   // sender's ECDSA identity public key, SPKI DER, base64 (CryptoService::pubkey_base64)
///   epoch:     u64,      // per-connection epoch: strictly-greater on each (re)connect (anti-replay)
///   seq:       u64,      // per-sender monotonic counter within an epoch (anti-replay)
///   sig:       String,   // base64 IEEE-P1363 ECDSA-P256 signature over canonical_signing_bytes()
///   frame:     PairFrame // the existing inner application frame
/// }
/// ```
///
/// ANTI-REPLAY: `member_id` AND `pubkey` are bound into the signature (so a
/// frame cannot be relabeled to a different `member_id` to dodge the per-member
/// seq tracker), and a signed per-connection `epoch` lets a recipient accept a
/// fresh counter on reconnect WITHOUT a downward-reset window: it accepts iff
/// `epoch > stored.epoch` (resetting `last_seq`) OR `epoch == stored.epoch &&
/// seq > stored.last_seq`; a lower epoch or a non-increasing seq is rejected.
///
/// IDENTITY BINDING (TOFU within a session): the first verified frame from a
/// `member_id` pins `member_id → pubkey` in the recipient's roster; every later
/// frame from that `member_id` MUST verify against the pinned pubkey or be
/// dropped. The `Join` frame additionally carries the pubkey at the application
/// layer (see [`PairFrame::Join`]) so the host can pin it explicitly on join.
///
/// HOST AUTHORITY: the host's `member_id`/`pubkey` is pinned first-host-wins
/// from the first verified `SessionMeta`. Host-only frames
/// (`pty_output`/`resize`/`driver_changed`/`snapshot`/`session_meta`,
/// see [`PairFrame::is_host_only`]) are accepted ONLY when the verified signer
/// equals the pinned host; `term_input` is accepted ONLY when the verified
/// signer equals the current `driver_user_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedPairFrame {
    /// Sender's stable in-room member id.
    pub member_id: String,
    /// Sender's ECDSA identity public key (SPKI DER, base64) — same encoding as
    /// [`jarvis_platform::CryptoService::pubkey_base64`] and what
    /// `CryptoService::verify` expects for `pubkey_b64`.
    pub pubkey: String,
    /// Per-connection epoch: a peer picks a fresh, strictly-greater value on
    /// each (re)connect. Bound into the signature; lets the recipient reset its
    /// per-member `seq` tracker on a genuine reconnect without a replay-friendly
    /// downward-reset window.
    pub epoch: u64,
    /// Per-sender monotonic anti-replay counter within an epoch.
    pub seq: u64,
    /// Base64 IEEE-P1363 ECDSA-P256 signature over
    /// [`SignedPairFrame::canonical_signing_bytes`].
    pub sig: String,
    /// The inner application frame.
    pub frame: PairFrame,
}

impl SignedPairFrame {
    /// Build the CANONICAL, deterministic byte string a member signs over:
    ///
    /// ```text
    /// PAIR_SIG_DOMAIN ‹0x1F› session_id ‹0x1F› member_id ‹0x1F› pubkey ‹0x1F›
    ///   frame_type ‹0x1F› epoch(decimal) ‹0x1F› seq(decimal) ‹0x1F› frame_json
    /// ```
    ///
    /// - `PAIR_SIG_DOMAIN` is a fixed crypto domain separator so a pair-frame
    ///   signature can never be cross-presented to the relay/pairing/theme
    ///   signers that share this identity key.
    /// - `session_id` binds the signature to this room (a sig can't be replayed
    ///   into another session).
    /// - `member_id` AND `pubkey` are bound in, so a frame cannot be relabeled
    ///   to a different `member_id` (to dodge the per-member seq tracker) or have
    ///   its claimed pubkey swapped while keeping a valid signature.
    /// - `frame_type` is a per-type domain separator (a sig over one type can't
    ///   be reinterpreted as another).
    /// - `epoch` is the per-connection epoch and `seq` the per-sender monotonic
    ///   counter within it (anti-replay).
    /// - `frame_json` is the exact `serde_json` serialization of the inner
    ///   `PairFrame` — deterministic for a fixed struct, and the SAME bytes the
    ///   recipient re-serializes to verify.
    ///
    /// Standalone (not a method on `self`) so the SENDER can compute it before a
    /// signature exists and the RECIEVER can recompute it from the parsed
    /// envelope.
    #[allow(clippy::too_many_arguments)]
    pub fn canonical_signing_bytes(
        session_id: &str,
        member_id: &str,
        pubkey: &str,
        frame: &PairFrame,
        epoch: u64,
        seq: u64,
    ) -> Result<Vec<u8>, String> {
        let frame_json = serde_json::to_vec(frame).map_err(|e| e.to_string())?;
        let mut buf = Vec::with_capacity(
            PAIR_SIG_DOMAIN.len() + session_id.len() + member_id.len() + pubkey.len()
                + frame_json.len()
                + 48,
        );
        buf.extend_from_slice(PAIR_SIG_DOMAIN.as_bytes());
        buf.push(CANON_SEP);
        buf.extend_from_slice(session_id.as_bytes());
        buf.push(CANON_SEP);
        buf.extend_from_slice(member_id.as_bytes());
        buf.push(CANON_SEP);
        buf.extend_from_slice(pubkey.as_bytes());
        buf.push(CANON_SEP);
        buf.extend_from_slice(frame.frame_type().as_bytes());
        buf.push(CANON_SEP);
        buf.extend_from_slice(epoch.to_string().as_bytes());
        buf.push(CANON_SEP);
        buf.extend_from_slice(seq.to_string().as_bytes());
        buf.push(CANON_SEP);
        buf.extend_from_slice(&frame_json);
        Ok(buf)
    }

    /// Recompute this envelope's canonical signing bytes for verification (uses
    /// the carried `member_id`/`pubkey`/`epoch`/`seq` + inner `frame` against the
    /// supplied `session_id`).
    pub fn signing_bytes(&self, session_id: &str) -> Result<Vec<u8>, String> {
        Self::canonical_signing_bytes(
            session_id,
            &self.member_id,
            &self.pubkey,
            &self.frame,
            self.epoch,
            self.seq,
        )
    }
}

/// Outcome of verifying an inbound [`SignedPairFrame`] against the session
/// roster (identity binding + anti-replay + host-authority). Returned by the
/// implemented verification seam (`PairAuthState::verify_signed_frame`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameVerifyResult {
    /// Signature valid, identity binding consistent, seq increasing, and (for
    /// host-only / driver frames) the authority rule holds. The frame may be
    /// applied.
    Accept,
    /// Reject with a reason (logged at debug; the frame is dropped).
    Reject(FrameRejectReason),
}

/// Why a [`SignedPairFrame`] was rejected. Enumerated so the host-authority and
/// anti-replay rules are testable and the reasons are greppable in logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameRejectReason {
    /// ECDSA signature did not verify against the carried pubkey.
    BadSignature,
    /// `member_id` is already pinned to a DIFFERENT pubkey (TOFU violation /
    /// impersonation attempt).
    PubkeyMismatch,
    /// Anti-replay failure: the `(epoch, seq)` pair was not strictly newer than
    /// what was last seen for this member — i.e. a lower epoch, or the same epoch
    /// with a non-increasing seq (replay / reorder).
    StaleSeq,
    /// A host-only frame whose verified signer is not the pinned host.
    NotHost,
    /// A `term_input` whose verified signer is not the current driver.
    NotDriver,
    /// Frame from a member_id that has not announced an identity (no `Join`
    /// pin yet) while `require_signed_join` is set.
    UnknownMember,
}

/// A signing capability: produces a base64 ECDSA-P256 signature over a message.
///
/// Implemented over [`jarvis_platform::CryptoService`] on the MAIN thread (the
/// only owner of the private identity key). The pair worker never holds the
/// secret key; outbound frames are signed on the main thread before they are
/// handed to the worker (or via a `Send`-able boxed signer if a later refactor
/// moves signing into the worker). The trait keeps that seam swappable.
pub trait PairSigner {
    /// The signer's identity public key (SPKI DER, base64).
    fn identity_pubkey(&self) -> String;
    /// Sign `msg`, returning a base64 IEEE-P1363 signature.
    fn sign_bytes(&self, msg: &[u8]) -> Result<String, String>;
}

/// A verification capability: checks a base64 ECDSA-P256 signature.
pub trait PairVerifier {
    /// Verify `sig_b64` over `msg` against the SPKI-base64 `pubkey_b64`.
    fn verify_bytes(&self, msg: &[u8], sig_b64: &str, pubkey_b64: &str) -> bool;
}

/// A single participant entry carried in [`PairFrame::SessionMeta`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RosterEntry {
    pub member_id: String,
    pub name: String,
    /// `"host"` | `"driver"` | `"view"`.
    pub role: String,
}

/// serde helper: encode `Vec<u8>` as a base64 string on the wire.
mod b64 {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&B64.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        B64.decode(s.as_bytes()).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_output_roundtrips_with_base64() {
        let frame = PairFrame::PtyOutput {
            data: vec![0x1b, b'[', b'2', b'J'],
            offset: 0,
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("\"type\":\"pty_output\""));
        // base64 of the escape sequence, not raw bytes.
        assert!(json.contains("\"data\":\"G1sySg==\""));
        let back: PairFrame = serde_json::from_str(&json).unwrap();
        match back {
            PairFrame::PtyOutput { data, .. } => assert_eq!(data, vec![0x1b, b'[', b'2', b'J']),
            _ => panic!("expected pty_output"),
        }
    }

    #[test]
    fn signed_frame_wraps_inner_frame_on_wire() {
        let signed = SignedPairFrame {
            member_id: "m2".into(),
            pubkey: "PUBKEY_B64".into(),
            epoch: 3,
            seq: 7,
            sig: "SIG_B64".into(),
            frame: PairFrame::TermInput {
                from: "m2".into(),
                data: b"ls\n".to_vec(),
            },
        };
        let json = serde_json::to_string(&signed).unwrap();
        // Carries the auth fields + the nested inner frame tag.
        assert!(json.contains("\"member_id\":\"m2\""));
        assert!(json.contains("\"epoch\":3"));
        assert!(json.contains("\"seq\":7"));
        assert!(json.contains("\"type\":\"term_input\""));
        let back: SignedPairFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(back.member_id, "m2");
        assert_eq!(back.epoch, 3);
        assert_eq!(back.seq, 7);
        assert!(matches!(back.frame, PairFrame::TermInput { .. }));
    }

    #[test]
    fn canonical_bytes_bind_all_signed_fields() {
        let frame = PairFrame::Resize { cols: 80, rows: 24 };
        let cb = |sess: &str, mid: &str, pk: &str, f: &PairFrame, ep: u64, sq: u64| {
            SignedPairFrame::canonical_signing_bytes(sess, mid, pk, f, ep, sq).unwrap()
        };
        let a = cb("sessA", "m1", "pkA", &frame, 1, 1);
        // Any of session, member_id, pubkey, frame, epoch, or seq => different bytes.
        assert_ne!(a, cb("sessB", "m1", "pkA", &frame, 1, 1), "session bound");
        assert_ne!(a, cb("sessA", "m2", "pkA", &frame, 1, 1), "member_id bound");
        assert_ne!(a, cb("sessA", "m1", "pkB", &frame, 1, 1), "pubkey bound");
        assert_ne!(a, cb("sessA", "m1", "pkA", &frame, 2, 1), "epoch bound");
        assert_ne!(a, cb("sessA", "m1", "pkA", &frame, 1, 2), "seq bound");
        assert_ne!(
            a,
            cb("sessA", "m1", "pkA", &PairFrame::Resize { cols: 81, rows: 24 }, 1, 1),
            "frame bound"
        );
        // Deterministic: same inputs => identical bytes (sender == receiver).
        assert_eq!(a, cb("sessA", "m1", "pkA", &frame, 1, 1));
    }

    /// The canonical bytes begin with the crypto domain separator, so a pair
    /// signature can never collide with the relay/pairing/theme signers that
    /// share the identity key (they never prefix this tag).
    #[test]
    fn canonical_bytes_have_domain_separator() {
        let frame = PairFrame::Resize { cols: 80, rows: 24 };
        let bytes =
            SignedPairFrame::canonical_signing_bytes("sessA", "m1", "pkA", &frame, 1, 1).unwrap();
        assert!(
            bytes.starts_with(PAIR_SIG_DOMAIN.as_bytes()),
            "canonical bytes must start with the pair-sig domain tag"
        );
    }

    /// deny_unknown_fields: a SignedPairFrame (or inner frame) with an extra
    /// field fails to deserialize, so an attacker can't append a field and keep
    /// a valid signature.
    #[test]
    fn deny_unknown_fields_rejects_extra_fields() {
        // Extra field on the envelope.
        let env = r#"{"member_id":"m2","pubkey":"p","epoch":1,"seq":1,"sig":"s",
            "frame":{"type":"resize","cols":80,"rows":24},"evil":"x"}"#;
        assert!(
            serde_json::from_str::<SignedPairFrame>(env).is_err(),
            "extra envelope field must be rejected"
        );
        // Extra field on the inner frame.
        let inner = r#"{"type":"resize","cols":80,"rows":24,"evil":"x"}"#;
        assert!(
            serde_json::from_str::<PairFrame>(inner).is_err(),
            "extra inner-frame field must be rejected"
        );
    }

    #[test]
    fn host_only_classification() {
        assert!(PairFrame::PtyOutput { data: vec![], offset: 0 }.is_host_only());
        assert!(PairFrame::SessionMeta {
            host: "h".into(),
            host_name: "H".into(),
            cols: 80,
            rows: 24,
            allow_takeover: true,
            roster: vec![],
        }
        .is_host_only());
        assert!(!PairFrame::TermInput { from: "m".into(), data: vec![] }.is_host_only());
        assert!(!PairFrame::Join {
            from: "m".into(),
            name: "N".into(),
            pubkey: "P".into(),
        }
        .is_host_only());
    }

    #[test]
    fn legacy_join_without_pubkey_deserializes() {
        // Wire-compat: a pre-M3 Join frame (no `pubkey`) still parses.
        let json = r#"{"type":"join","from":"m2","name":"Nav"}"#;
        let frame: PairFrame = serde_json::from_str(json).unwrap();
        match frame {
            PairFrame::Join { from, name, pubkey } => {
                assert_eq!(from, "m2");
                assert_eq!(name, "Nav");
                assert!(pubkey.is_empty());
            }
            _ => panic!("expected join"),
        }
    }

    #[test]
    fn session_meta_tag_is_snake_case() {
        let frame = PairFrame::SessionMeta {
            host: "u1".into(),
            host_name: "Host".into(),
            cols: 80,
            rows: 24,
            allow_takeover: true,
            roster: vec![RosterEntry {
                member_id: "u1".into(),
                name: "Host".into(),
                role: "host".into(),
            }],
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("\"type\":\"session_meta\""));
        assert!(json.contains("\"roster\""));
    }
}
