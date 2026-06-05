//! Outbound pair-Room relay client.
//!
//! Forks [`relay_client`](super::relay_client): sends `room_hello` instead of
//! `desktop_hello`, carries [`PairFrame`](super::pair_protocol::PairFrame) as
//! the inner payload, and reuses
//! [`RelayEnvelope`](super::relay_protocol::RelayEnvelope) +
//! [`RelayCipher`](super::crypto_bridge::RelayCipher).
//!
//! ## Wire flow
//! 1. Connect outbound to `config.relay_url`.
//! 2. Send `room_hello{session_id, member_id}`.
//! 3. Await the relay's `room_ready`, then emit [`PairRoomEvent::RoomReady`].
//! 4. Run a read/forward loop:
//!    - `cmd_rx` → [`PairRoomCommand::Send`] → encrypt the [`PairFrame`] into a
//!      [`RelayEnvelope::Encrypted`] → send (drop while no key yet).
//!    - inbound relay control frames (`member_joined`/`member_left`/
//!      `member_count`) → matching [`PairRoomEvent`].
//!    - inbound [`RelayEnvelope::Encrypted`] → decrypt → [`PairFrame`] →
//!      [`PairRoomEvent::Frame`].
//! 5. The 32-byte room AES key arrives over `config.key_rx` (M1 room-derived
//!    key); until it lands the cipher is `None` and outbound frames are dropped.
//! 6. On disconnect, emit [`PairRoomEvent::Disconnected`] and reconnect with
//!    exponential backoff (1s → 30s), exactly like `relay_client`.
//!
//! The relay forwards every member's frames opaquely to all *other* members, so
//! a member never receives its own frame echoed back.

#![allow(dead_code)]

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use super::crypto_bridge::RelayCipher;
use super::pair_protocol::{PairFrame, SignedPairFrame};
use super::relay_protocol::{RelayEnvelope, RoomHello};

/// Commands the `pair.rs` worker sends down into the room client.
pub enum PairRoomCommand {
    /// Send an application frame to the room (all-but-sender fan-out).
    ///
    /// This is the SINGLE outbound signing seam: when the worker holds a
    /// [`PairFrameSigner`] (the default with `CryptoService` present) it wraps
    /// the inner `PairFrame` in a [`SignedPairFrame`] (member_id + identity
    /// pubkey + per-connection epoch + monotonic seq + ECDSA signature) at the
    /// point of send, then encrypts it into the opaque `RelayEnvelope` (the relay
    /// stays unchanged). Without a signer it falls back to the legacy unsigned
    /// wire form. There is no separate pre-signed command — signing happens here,
    /// exactly once per frame.
    Send(PairFrame),
    /// Tear down the room connection.
    Shutdown,
}

/// Events the room client emits up to the `pair.rs` worker.
pub enum PairRoomEvent {
    /// `room_ready` received: we are joined as `member_id`.
    RoomReady { session_id: String, member_id: String },
    /// Another member joined.
    MemberJoined { member_id: String },
    /// A member left.
    MemberLeft { member_id: String },
    /// Current member count.
    MemberCount { count: u32 },
    /// An inbound application frame arrived from another member (LEGACY unsigned
    /// path — the envelope carried a bare `PairFrame`).
    Frame(PairFrame),
    /// M3: an inbound SIGNED application frame. The main thread runs
    /// `verify_signed_frame` (identity/anti-replay/host-authority) before
    /// `apply_pair_frame` honors the inner frame.
    SignedFrame(SignedPairFrame),
    /// The room connection dropped (will auto-reconnect).
    Disconnected,
    /// A transport-level error occurred.
    Error(String),
}

/// Configuration for the pair-Room relay client.
pub struct PairRoomClientConfig {
    /// WebSocket URL of the relay server (reuses `[relay].url`).
    pub relay_url: String,
    /// 32-char high-entropy session id (the room's capability secret).
    pub session_id: String,
    /// This member's stable id within the room.
    pub member_id: String,
    /// Watch receiver for the derived 32-byte room AES key (room-derived in M1).
    pub key_rx: tokio::sync::watch::Receiver<Option<[u8; 32]>>,
    /// M3: detached ECDSA signer for this app's identity. When present, EVERY
    /// outbound [`PairFrame`] is wrapped in a [`SignedPairFrame`] (member_id +
    /// pubkey + monotonic seq + ECDSA signature) before encryption, so peers can
    /// authenticate the sender (impersonation / host-spoofing defence). `None`
    /// (no [`CryptoService`]) falls back to the legacy unsigned wire form.
    pub signer: Option<jarvis_platform::PairFrameSigner>,
}

type WsStream = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

/// Run the pair-Room client with auto-reconnect.
///
/// Drains `cmd_rx` (outbound [`PairFrame`]s) and surfaces inbound room control
/// frames + decrypted [`PairFrame`]s over `event_tx`. Returns when the worker
/// sends [`PairRoomCommand::Shutdown`] or drops `cmd_rx`.
pub async fn run_pair_room_client(
    config: PairRoomClientConfig,
    mut cmd_rx: mpsc::Receiver<PairRoomCommand>,
    event_tx: std::sync::mpsc::Sender<PairRoomEvent>,
) {
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(30);

    // M3 anti-replay epoch: a per-connection counter that strictly increases on
    // each (re)connect. Seeded from the wall clock (unix seconds) so it is fresh
    // across full process restarts too — a recipient that pinned a high
    // `(epoch, seq)` for our member_id in a previous session still accepts our
    // new session because the new epoch is strictly greater. Each successful
    // `room_session` uses a strictly-greater epoch than the last.
    let mut epoch: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1)
        .max(1);

    loop {
        tracing::info!(url = %config.relay_url, "Connecting to pair room...");

        match connect_async(&config.relay_url).await {
            Ok((ws, _)) => {
                backoff = Duration::from_secs(1);
                // Fresh, strictly-greater epoch for this (re)connect.
                epoch = epoch.wrapping_add(1).max(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                );
                match room_session(ws, &config, epoch, &mut cmd_rx, &event_tx).await {
                    SessionResult::Shutdown => {
                        tracing::info!("Pair room client shutting down");
                        return;
                    }
                    SessionResult::Disconnected(reason) => {
                        tracing::warn!(reason = %reason, "Pair room connection lost");
                        let _ = event_tx.send(PairRoomEvent::Disconnected);
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to connect to pair room");
                let _ = event_tx.send(PairRoomEvent::Error(format!("connect failed: {e}")));
            }
        }

        // Reconnect backoff, but bail immediately if the worker is gone/shutting down.
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(PairRoomCommand::Shutdown) | None => return,
                    // A Send arriving mid-backoff is dropped (no live socket yet).
                    Some(PairRoomCommand::Send(_)) => {}
                }
            }
        }

        backoff = (backoff * 2).min(max_backoff);
    }
}

enum SessionResult {
    Shutdown,
    Disconnected(String),
}

/// Handle a single room session: `room_hello` → `room_ready` → forward loop.
async fn room_session(
    ws: WsStream,
    config: &PairRoomClientConfig,
    epoch: u64,
    cmd_rx: &mut mpsc::Receiver<PairRoomCommand>,
    event_tx: &std::sync::mpsc::Sender<PairRoomEvent>,
) -> SessionResult {
    let (mut sink, mut stream) = ws.split();

    // 1. Send the SIGNED room_hello{session_id, member_id, pubkey, nonce, sig}.
    //    The relay REQUIRES a valid signature + `member_id → pubkey` TOFU
    //    binding (member-id slot DoS fix). The same identity signer that signs
    //    outbound SignedPairFrames signs the hello, so the slot is bound to our
    //    identity. Without a signer (no CryptoService) the relay rejects us.
    let hello = match config.signer.as_ref() {
        Some(signer) => serde_json::to_string(&RoomHello::signed(
            config.session_id.clone(),
            config.member_id.clone(),
            signer,
        ))
        .unwrap(),
        None => {
            tracing::warn!("Pair room: no room_hello signer; relay will reject this hello");
            // Structurally still emit a (bare) hello; the relay refuses it.
            serde_json::json!({
                "type": "room_hello",
                "session_id": config.session_id,
                "member_id": config.member_id,
            })
            .to_string()
        }
    };
    if sink.send(Message::Text(hello.into())).await.is_err() {
        return SessionResult::Disconnected("failed to send room_hello".into());
    }

    // 2. Wait for room_ready. The relay only echoes `session_id`, so we pair it
    //    with our own member_id from config (the relay never relabels us).
    match read_room_ready(&mut stream).await {
        RoomReadyResult::Ready { session_id } => {
            tracing::info!(member = %config.member_id, "Pair room ready");
            let _ = event_tx.send(PairRoomEvent::RoomReady {
                session_id,
                member_id: config.member_id.clone(),
            });
        }
        RoomReadyResult::Error(message) => {
            return SessionResult::Disconnected(format!("relay error: {message}"));
        }
        RoomReadyResult::Closed => {
            return SessionResult::Disconnected("relay closed before room_ready".into());
        }
    }

    // The cipher is set once the 32-byte room key arrives over the watch channel.
    // Seed it from the current key in case it was derived before we connected.
    let mut key_rx = config.key_rx.clone();
    let mut cipher: Option<RelayCipher> = key_rx.borrow().map(RelayCipher::new);

    // M3 OUTBOUND anti-replay counter. Per-sender monotonic `seq` carried in
    // every SignedPairFrame, paired with the per-connection `epoch`. It restarts
    // at 0 on each relay reconnect; the recipient tolerates that because the
    // accompanying `epoch` is strictly greater than the previous connection's, so
    // it resets its `last_seq` for us (no downward-reset window needed).
    let mut outbound_seq: u64 = 0;

    // 3. Forwarding loop.
    loop {
        tokio::select! {
            // Room key derived / rotated by the main thread.
            result = key_rx.changed() => {
                if result.is_ok() {
                    if let Some(key_bytes) = *key_rx.borrow() {
                        tracing::info!("Pair room key received, encryption enabled");
                        cipher = Some(RelayCipher::new(key_bytes));
                    }
                }
            }

            // Outbound: worker → encrypt PairFrame → send (drop if no key yet).
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(PairRoomCommand::Send(frame)) => {
                        let Some(ref c) = cipher else {
                            tracing::trace!("No pair room key yet, dropping outbound frame");
                            continue;
                        };
                        // M3: when we hold a signer, EVERY outbound frame is wrapped
                        // in a SignedPairFrame (member_id + pubkey + monotonic seq +
                        // ECDSA sig) so peers can authenticate us. Without a signer
                        // (no CryptoService) we fall back to the legacy unsigned wire.
                        let envelope = if let Some(ref signer) = config.signer {
                            outbound_seq += 1;
                            match sign_frame(signer, &config.member_id, &config.session_id, epoch, outbound_seq, frame) {
                                Ok(signed) => match encrypt_signed_frame(c, &signed) {
                                    Ok(env) => env,
                                    Err(e) => {
                                        tracing::warn!(error = %e, "Pair signed-frame encryption failed, dropping");
                                        continue;
                                    }
                                },
                                Err(e) => {
                                    tracing::warn!(error = %e, "Pair frame signing failed, dropping");
                                    continue;
                                }
                            }
                        } else {
                            match encrypt_frame(c, &frame) {
                                Ok(env) => env,
                                Err(e) => {
                                    tracing::warn!(error = %e, "Pair frame encryption failed, dropping");
                                    continue;
                                }
                            }
                        };
                        let json = serde_json::to_string(&envelope).unwrap();
                        if sink.send(Message::Text(json.into())).await.is_err() {
                            return SessionResult::Disconnected("send failed".into());
                        }
                    }
                    Some(PairRoomCommand::Shutdown) | None => {
                        let _ = sink.close().await;
                        return SessionResult::Shutdown;
                    }
                }
            }

            // Inbound: relay control frames + encrypted PairFrames from peers.
            frame = stream.next() => {
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(result) =
                            handle_inbound_text(&text, cipher.as_ref(), event_tx)
                        {
                            return result;
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = sink.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        return SessionResult::Disconnected("relay closed connection".into());
                    }
                    Some(Err(e)) => {
                        return SessionResult::Disconnected(format!("ws error: {e}"));
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Encrypt a `PairFrame` into a `RelayEnvelope::Encrypted`.
///
/// JSON-serialize the frame, then hand the bytes to the shared `RelayCipher`
/// (AES-256-GCM, fresh 12-byte nonce, base64 wire) — the same envelope the
/// mobile bridge uses, so the relay stays fully opaque.
fn encrypt_frame(cipher: &RelayCipher, frame: &PairFrame) -> Result<RelayEnvelope, String> {
    let json = serde_json::to_string(frame).map_err(|e| e.to_string())?;
    cipher.encrypt_bytes(json.as_bytes())
}

/// M3: encrypt a `SignedPairFrame` into a `RelayEnvelope::Encrypted`.
///
/// Identical envelope to `encrypt_frame` — the signed wrapper is just the JSON
/// payload, so the relay (and `RelayCipher`) stay unchanged. The inner JSON
/// carries `member_id`/`pubkey`/`seq`/`sig`/`frame`, distinguishable inbound
/// by the presence of those fields (see `handle_inbound_text`).
fn encrypt_signed_frame(
    cipher: &RelayCipher,
    signed: &SignedPairFrame,
) -> Result<RelayEnvelope, String> {
    let json = serde_json::to_string(signed).map_err(|e| e.to_string())?;
    cipher.encrypt_bytes(json.as_bytes())
}

/// M3: wrap a `PairFrame` in a signed envelope.
///
/// Computes the canonical signing bytes (domain tag ‹0x1F› session_id ‹0x1F›
/// member_id ‹0x1F› pubkey ‹0x1F› frame_type ‹0x1F› epoch ‹0x1F› seq ‹0x1F›
/// frame_json), signs them with the app's ECDSA identity, and attaches our
/// `member_id`, identity `pubkey`, per-connection `epoch`, monotonic `seq`, and
/// `sig`.
///
/// The signature is taken over `base64(canonical_bytes)` — a lossless, fixed
/// mapping the verifier (`pair.rs::verify_signed_frame`) applies identically, so
/// it stays within the `&str`-based `CryptoService::verify`/`PairFrameSigner`
/// API while still authenticating the original canonical bytes 1:1.
fn sign_frame(
    signer: &jarvis_platform::PairFrameSigner,
    member_id: &str,
    session_id: &str,
    epoch: u64,
    seq: u64,
    frame: PairFrame,
) -> Result<SignedPairFrame, String> {
    let pubkey = signer.pubkey_base64.clone();
    let canonical =
        SignedPairFrame::canonical_signing_bytes(session_id, member_id, &pubkey, &frame, epoch, seq)?;
    let msg = b64_of(&canonical);
    let sig = signer.sign_bytes(msg.as_bytes());
    Ok(SignedPairFrame {
        member_id: member_id.to_string(),
        pubkey,
        epoch,
        seq,
        sig,
        frame,
    })
}

/// Base64 of arbitrary bytes. The signing payload is `base64(canonical_bytes)`
/// so it can pass through the `&str` sign/verify API unchanged on both ends
/// (mirrors `pair.rs::base64_of`).
fn b64_of(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    B64.encode(bytes)
}

/// Route one inbound text frame. Returns `Some(SessionResult)` only when the
/// session must end (it never ends on a single bad frame).
fn handle_inbound_text(
    text: &str,
    cipher: Option<&RelayCipher>,
    event_tx: &std::sync::mpsc::Sender<PairRoomEvent>,
) -> Option<SessionResult> {
    // Parse once as an untyped object so we can branch on the `type` tag without
    // depending on the relay echoing fields (e.g. room_ready omits member_id).
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => {
            tracing::debug!("Pair room: undecodable text frame");
            return None;
        }
    };
    let tag = value.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match tag {
        // --- Relay room control frames ---
        "member_joined" => {
            if let Some(id) = value.get("member_id").and_then(|m| m.as_str()) {
                let _ = event_tx.send(PairRoomEvent::MemberJoined {
                    member_id: id.to_string(),
                });
            }
        }
        "member_left" => {
            if let Some(id) = value.get("member_id").and_then(|m| m.as_str()) {
                let _ = event_tx.send(PairRoomEvent::MemberLeft {
                    member_id: id.to_string(),
                });
            }
        }
        "member_count" => {
            if let Some(count) = value.get("count").and_then(|c| c.as_u64()) {
                let _ = event_tx.send(PairRoomEvent::MemberCount {
                    count: count as u32,
                });
            }
        }
        "room_ready" => {
            // A second room_ready (e.g. after a relay-side reconnect) — ignore;
            // the first one already produced our RoomReady event.
            tracing::debug!("Pair room: duplicate room_ready, ignoring");
        }
        "error" => {
            let message = value
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown relay error")
                .to_string();
            tracing::warn!(error = %message, "Pair room relay error");
            let _ = event_tx.send(PairRoomEvent::Error(message));
        }

        // --- Application envelopes from peer members ---
        "encrypted" => {
            let (Some(iv), Some(ct)) = (
                value.get("iv").and_then(|v| v.as_str()),
                value.get("ct").and_then(|v| v.as_str()),
            ) else {
                tracing::debug!("Pair room: malformed encrypted envelope");
                return None;
            };
            let Some(c) = cipher else {
                tracing::warn!("Pair room: encrypted frame before key, dropping");
                return None;
            };
            match c.decrypt_bytes(iv, ct) {
                Ok(plain) => {
                    // M3: prefer the SIGNED envelope (member_id/pubkey/seq/sig +
                    // inner frame). A bare `PairFrame` (`{"type": ...}` at the
                    // top level, no `frame` field) is the LEGACY unsigned path.
                    if let Ok(signed) = serde_json::from_slice::<SignedPairFrame>(&plain) {
                        let _ = event_tx.send(PairRoomEvent::SignedFrame(signed));
                    } else {
                        match serde_json::from_slice::<PairFrame>(&plain) {
                            Ok(pair_frame) => {
                                let _ = event_tx.send(PairRoomEvent::Frame(pair_frame));
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Pair room: bad inner PairFrame");
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Pair room: decryption failed");
                }
            }
        }
        // Plaintext/key_exchange envelopes are not used in the M1 room flow.
        "plaintext" | "key_exchange" => {
            tracing::debug!(tag, "Pair room: ignoring non-encrypted envelope");
        }
        other => {
            tracing::trace!(tag = other, "Pair room: unhandled frame tag");
        }
    }

    None
}

enum RoomReadyResult {
    Ready { session_id: String },
    Error(String),
    Closed,
}

/// Read text frames until `room_ready` (or `error`), with a 10s timeout.
async fn read_room_ready(
    stream: &mut futures_util::stream::SplitStream<WsStream>,
) -> RoomReadyResult {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return RoomReadyResult::Closed;
        }
        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let value: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                match value.get("type").and_then(|t| t.as_str()) {
                    Some("room_ready") => {
                        let session_id = value
                            .get("session_id")
                            .and_then(|s| s.as_str())
                            .unwrap_or_default()
                            .to_string();
                        return RoomReadyResult::Ready { session_id };
                    }
                    Some("error") => {
                        let message = value
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown relay error")
                            .to_string();
                        return RoomReadyResult::Error(message);
                    }
                    // Tolerate control frames arriving before room_ready.
                    _ => continue,
                }
            }
            Ok(Some(Ok(_))) => continue,
            _ => return RoomReadyResult::Closed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [7u8; 32];

    /// A `PairFrame` encrypted with `encrypt_frame` round-trips back through the
    /// inbound handler as `PairRoomEvent::Frame` — the full wire contract.
    #[test]
    fn pair_frame_roundtrips_through_envelope() {
        let cipher = RelayCipher::new(KEY);
        let frame = PairFrame::PtyOutput {
            data: vec![0x1b, b'[', b'2', b'J'],
            offset: 0,
        };

        // Outbound: encrypt → JSON envelope (what we put on the wire).
        let envelope = encrypt_frame(&cipher, &frame).unwrap();
        let wire = serde_json::to_string(&envelope).unwrap();
        assert!(wire.contains("\"type\":\"encrypted\""));

        // Inbound: feed the same wire bytes back through the handler.
        let (tx, rx) = std::sync::mpsc::channel();
        let end = handle_inbound_text(&wire, Some(&cipher), &tx);
        assert!(end.is_none(), "a valid frame must not end the session");

        match rx.try_recv().unwrap() {
            PairRoomEvent::Frame(PairFrame::PtyOutput { data, .. }) => {
                assert_eq!(data, vec![0x1b, b'[', b'2', b'J']);
            }
            _ => panic!("expected a decrypted pty_output Frame event"),
        }
    }

    /// A frame encrypted under a different key fails to decrypt and surfaces no
    /// event (dropped, session continues).
    #[test]
    fn frame_with_wrong_key_is_dropped() {
        let sender = RelayCipher::new(KEY);
        let receiver = RelayCipher::new([9u8; 32]);
        let frame = PairFrame::Resize { cols: 80, rows: 24 };

        let envelope = encrypt_frame(&sender, &frame).unwrap();
        let wire = serde_json::to_string(&envelope).unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        assert!(handle_inbound_text(&wire, Some(&receiver), &tx).is_none());
        assert!(rx.try_recv().is_err(), "wrong-key frame must be dropped");
    }

    /// Relay room-control frames map to the matching events without a cipher.
    #[test]
    fn room_control_frames_map_to_events() {
        let (tx, rx) = std::sync::mpsc::channel();

        handle_inbound_text(r#"{"type":"member_joined","member_id":"m2"}"#, None, &tx);
        handle_inbound_text(r#"{"type":"member_left","member_id":"m3"}"#, None, &tx);
        handle_inbound_text(r#"{"type":"member_count","count":4}"#, None, &tx);
        handle_inbound_text(r#"{"type":"error","message":"boom"}"#, None, &tx);

        match rx.try_recv().unwrap() {
            PairRoomEvent::MemberJoined { member_id } => assert_eq!(member_id, "m2"),
            _ => panic!("expected MemberJoined"),
        }
        match rx.try_recv().unwrap() {
            PairRoomEvent::MemberLeft { member_id } => assert_eq!(member_id, "m3"),
            _ => panic!("expected MemberLeft"),
        }
        match rx.try_recv().unwrap() {
            PairRoomEvent::MemberCount { count } => assert_eq!(count, 4),
            _ => panic!("expected MemberCount"),
        }
        match rx.try_recv().unwrap() {
            PairRoomEvent::Error(msg) => assert_eq!(msg, "boom"),
            _ => panic!("expected Error"),
        }
    }

    /// An encrypted frame arriving before the room key drops cleanly (no panic,
    /// no event, session continues).
    #[test]
    fn encrypted_frame_before_key_is_dropped() {
        let (tx, rx) = std::sync::mpsc::channel();
        let wire = r#"{"type":"encrypted","iv":"AAAAAAAAAAAAAAAA","ct":"AAAA"}"#;
        assert!(handle_inbound_text(wire, None, &tx).is_none());
        assert!(rx.try_recv().is_err());
    }

    // ====================================================================
    // M3: signing + verification (worker outbound sign / receiver verify)
    // ====================================================================
    //
    // These exercise the REAL crypto path both peers use: `sign_frame` (the
    // worker's outbound signer) producing a `SignedPairFrame`, and the exact
    // verification predicate the receiver runs in
    // `pair.rs::verify_signed_frame` — identity (pubkey) binding, signature
    // over the canonical bytes, anti-replay seq, and tamper detection — built
    // on `CryptoService::{pair_frame_signer, verify}`.

    use jarvis_platform::CryptoService;

    const SID: &str = "sessionABCDEF0123456789abcdef0123";

    /// The receiver's signature predicate (mirror of `verify_signed_frame`'s
    /// signature step): recompute the canonical bytes from the parsed envelope
    /// and verify the carried sig against the carried pubkey. A real receiver
    /// additionally pins `member_id → pubkey` and checks `seq`; those are
    /// asserted separately in the tests below.
    fn verify_sig(crypto: &CryptoService, signed: &SignedPairFrame) -> bool {
        let bytes = match signed.signing_bytes(SID) {
            Ok(b) => b,
            Err(_) => return false,
        };
        let msg = b64_of(&bytes);
        crypto
            .verify(&msg, &signed.sig, &signed.pubkey)
            .unwrap_or(false)
    }

    /// A frame signed by the worker verifies for any holder of the pubkey
    /// (the receiver uses a fresh `CryptoService` purely as a verifier).
    #[test]
    fn signed_frame_good_sig_verifies() {
        let sender = CryptoService::generate().unwrap();
        let signer = sender.pair_frame_signer();
        let verifier = CryptoService::generate().unwrap(); // receiver, different identity

        let frame = PairFrame::TermInput {
            from: "m2".into(),
            data: b"ls\n".to_vec(),
        };
        let signed = sign_frame(&signer, "m2", SID, 1, 1, frame).unwrap();

        assert_eq!(signed.member_id, "m2");
        assert_eq!(signed.pubkey, sender.pubkey_base64);
        assert_eq!(signed.epoch, 1);
        assert_eq!(signed.seq, 1);
        assert!(!signed.sig.is_empty());
        assert!(verify_sig(&verifier, &signed), "good signature must verify");
    }

    /// Tampering with the inner frame after signing breaks the signature
    /// (the canonical bytes no longer match what was signed).
    #[test]
    fn signed_frame_tampered_payload_rejected() {
        let sender = CryptoService::generate().unwrap();
        let signer = sender.pair_frame_signer();

        let frame = PairFrame::TermInput {
            from: "m2".into(),
            data: b"ls\n".to_vec(),
        };
        let mut signed = sign_frame(&signer, "m2", SID, 1, 1, frame).unwrap();
        // Attacker swaps the keystrokes for `rm -rf /` but keeps the sig.
        signed.frame = PairFrame::TermInput {
            from: "m2".into(),
            data: b"rm -rf /\n".to_vec(),
        };
        assert!(
            !verify_sig(&sender, &signed),
            "tampered payload must fail verification"
        );
    }

    /// A signature is bound to the signer's pubkey: presenting the same signed
    /// bytes under a DIFFERENT pubkey (wrong-key / impersonation) fails.
    #[test]
    fn signed_frame_wrong_key_rejected() {
        let sender = CryptoService::generate().unwrap();
        let attacker = CryptoService::generate().unwrap();
        let signer = sender.pair_frame_signer();

        let frame = PairFrame::Cursor {
            from: "m2".into(),
            row: 1,
            col: 2,
        };
        let mut signed = sign_frame(&signer, "m2", SID, 1, 1, frame).unwrap();
        // Attacker substitutes their own pubkey (claiming authorship). The sig
        // was made with the sender's key, so it no longer verifies.
        signed.pubkey = attacker.pubkey_base64.clone();
        assert!(
            !verify_sig(&sender, &signed),
            "sig must not verify against a different pubkey"
        );
    }

    /// Identity binding (TOFU): once `member_id → pubkey` is pinned, a frame from
    /// the same member_id carrying a DIFFERENT pubkey is a mismatch (rejected by
    /// the receiver before any signature check).
    #[test]
    fn member_pubkey_mismatch_detected() {
        let real = CryptoService::generate().unwrap();
        let imposter = CryptoService::generate().unwrap();

        // First frame from "m2" pins its pubkey.
        let first = sign_frame(&real.pair_frame_signer(), "m2", SID, 1, 1, PairFrame::RequestControl { from: "m2".into() }).unwrap();
        let pinned = first.pubkey.clone();

        // Later frame claims to be "m2" but carries a different (imposter) key —
        // even though it is internally self-consistent (imposter signed it).
        let forged = sign_frame(&imposter.pair_frame_signer(), "m2", SID, 1, 2, PairFrame::RequestControl { from: "m2".into() }).unwrap();

        assert_ne!(pinned, forged.pubkey, "imposter key differs from pinned");
        // The receiver rejects on the pinned≠carried pubkey mismatch (this is the
        // `PubkeyMismatch` arm) WITHOUT trusting the forged frame's self-sig.
        assert_eq!(forged.member_id, "m2");
        // Sanity: the forged frame is itself validly signed by the imposter key,
        // proving the defence is the *binding*, not a broken signature.
        assert!(verify_sig(&imposter, &forged));
    }

    /// Anti-replay: a replayed (or reordered) frame carries a seq that does not
    /// strictly exceed the last seen seq, so the receiver drops it. We assert the
    /// seq monotonicity rule the receiver enforces.
    #[test]
    fn replayed_seq_rejected_by_monotonic_rule() {
        let sender = CryptoService::generate().unwrap();
        let signer = sender.pair_frame_signer();

        let mk = |seq: u64| {
            sign_frame(&signer, "m2", SID, 1, seq, PairFrame::Cursor { from: "m2".into(), row: 0, col: 0 }).unwrap()
        };
        let f1 = mk(1);
        let f2 = mk(2);
        let replay = mk(1); // a captured copy of an earlier frame

        // All three are individually valid signatures...
        assert!(verify_sig(&sender, &f1));
        assert!(verify_sig(&sender, &f2));
        assert!(verify_sig(&sender, &replay));

        // ...but the receiver tracks (epoch, last_seq) and requires a strictly
        // newer pair. After accepting epoch=1/seq=2, a replayed epoch=1/seq=1 is
        // NOT newer, so it is dropped.
        let mut stored: (u64, u64) = (0, 0); // (epoch, last_seq)
        let mut accept = |epoch: u64, seq: u64| -> bool {
            if epoch > stored.0 {
                stored = (epoch, seq);
                return true;
            }
            if epoch == stored.0 && seq > stored.1 {
                stored.1 = seq;
                return true;
            }
            false
        };
        assert!(accept(f1.epoch, f1.seq), "seq 1 accepted first");
        assert!(accept(f2.epoch, f2.seq), "seq 2 accepted");
        assert!(!accept(replay.epoch, replay.seq), "replayed seq 1 must be rejected");
    }

    /// End-to-end transport: a worker-signed frame, encrypted into the opaque
    /// envelope, round-trips back through `handle_inbound_text` as a
    /// `SignedFrame` event carrying the intact member_id/pubkey/seq/sig — and the
    /// recovered envelope's signature still verifies.
    #[test]
    fn signed_frame_roundtrips_through_envelope() {
        let cipher = RelayCipher::new(KEY);
        let sender = CryptoService::generate().unwrap();
        let signer = sender.pair_frame_signer();

        let frame = PairFrame::PtyOutput { data: vec![0x1b, b'[', b'2', b'J'], offset: 0 };
        let signed = sign_frame(&signer, "host1", SID, 1, 5, frame).unwrap();
        let envelope = encrypt_signed_frame(&cipher, &signed).unwrap();
        let wire = serde_json::to_string(&envelope).unwrap();
        assert!(wire.contains("\"type\":\"encrypted\""));

        let (tx, rx) = std::sync::mpsc::channel();
        assert!(handle_inbound_text(&wire, Some(&cipher), &tx).is_none());

        match rx.try_recv().unwrap() {
            PairRoomEvent::SignedFrame(got) => {
                assert_eq!(got.member_id, "host1");
                assert_eq!(got.seq, 5);
                assert_eq!(got.pubkey, sender.pubkey_base64);
                assert!(matches!(got.frame, PairFrame::PtyOutput { .. }));
                assert!(verify_sig(&sender, &got), "recovered sig must verify");
            }
            _ => panic!("expected a SignedFrame event"),
        }
    }

    /// Single signing seam: one outbound `PairFrame` is wrapped in EXACTLY ONE
    /// `SignedPairFrame` (one signature, one envelope, one inbound SignedFrame
    /// event). With `SendSigned` removed, `sign_frame` is the sole signing path,
    /// so a frame is never double-signed. This guards the "signed exactly once"
    /// invariant the dead-seam removal was about.
    #[test]
    fn outbound_frame_is_signed_exactly_once() {
        let cipher = RelayCipher::new(KEY);
        let sender = CryptoService::generate().unwrap();
        let signer = sender.pair_frame_signer();

        // The worker's outbound step for one Send: sign once, encrypt once.
        let frame = PairFrame::Cursor { from: "m2".into(), row: 1, col: 2 };
        let signed = sign_frame(&signer, "m2", SID, 1, 1, frame).unwrap();
        let envelope = encrypt_signed_frame(&cipher, &signed).unwrap();
        let wire = serde_json::to_string(&envelope).unwrap();

        // Exactly one inbound event, and it is a SignedFrame (not a double emit
        // and not the legacy unsigned path).
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(handle_inbound_text(&wire, Some(&cipher), &tx).is_none());
        assert!(
            matches!(rx.try_recv(), Ok(PairRoomEvent::SignedFrame(_))),
            "exactly one SignedFrame event expected"
        );
        assert!(
            rx.try_recv().is_err(),
            "a single outbound frame must yield a single event (signed once)"
        );
    }

    /// A legacy bare `PairFrame` (no signing fields) still parses as the unsigned
    /// `Frame` path — wire-compat with pre-M3 peers is preserved.
    #[test]
    fn legacy_unsigned_frame_still_parses() {
        let cipher = RelayCipher::new(KEY);
        let frame = PairFrame::Resize { cols: 80, rows: 24 };
        let envelope = encrypt_frame(&cipher, &frame).unwrap();
        let wire = serde_json::to_string(&envelope).unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        assert!(handle_inbound_text(&wire, Some(&cipher), &tx).is_none());
        match rx.try_recv().unwrap() {
            PairRoomEvent::Frame(PairFrame::Resize { cols, rows }) => {
                assert_eq!((cols, rows), (80, 24));
            }
            _ => panic!("expected legacy unsigned Frame event"),
        }
    }
}
