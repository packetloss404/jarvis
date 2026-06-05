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
use super::pair_protocol::PairFrame;
use super::relay_protocol::{RelayEnvelope, RoomHello};

/// Commands the `pair.rs` worker sends down into the room client.
pub enum PairRoomCommand {
    /// Send an application frame to the room (all-but-sender fan-out).
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
    /// An inbound application frame arrived from another member.
    Frame(PairFrame),
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

    loop {
        tracing::info!(url = %config.relay_url, "Connecting to pair room...");

        match connect_async(&config.relay_url).await {
            Ok((ws, _)) => {
                backoff = Duration::from_secs(1);
                match room_session(ws, &config, &mut cmd_rx, &event_tx).await {
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
    cmd_rx: &mut mpsc::Receiver<PairRoomCommand>,
    event_tx: &std::sync::mpsc::Sender<PairRoomEvent>,
) -> SessionResult {
    let (mut sink, mut stream) = ws.split();

    // 1. Send room_hello{session_id, member_id}.
    let hello = serde_json::to_string(&RoomHello::new(
        config.session_id.clone(),
        config.member_id.clone(),
    ))
    .unwrap();
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
                        let envelope = match encrypt_frame(c, &frame) {
                            Ok(env) => env,
                            Err(e) => {
                                tracing::warn!(error = %e, "Pair frame encryption failed, dropping");
                                continue;
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
                Ok(plain) => match serde_json::from_slice::<PairFrame>(&plain) {
                    Ok(pair_frame) => {
                        let _ = event_tx.send(PairRoomEvent::Frame(pair_frame));
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Pair room: bad inner PairFrame");
                    }
                },
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
            PairRoomEvent::Frame(PairFrame::PtyOutput { data }) => {
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
}
