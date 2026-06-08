//! Outbound relay Room WebSocket client with auto-reconnect.
//!
//! Mirrors the reconnect/forward structure of the desktop's
//! `jarvis-app .../ws_server/relay_client.rs`, but for the symmetric Room
//! protocol used by presence. The client:
//!
//! 1. Connects to `relay_url`, sends `{"type":"room_hello", session_id, member_id}`.
//! 2. Waits for `room_ready`, surfaces [`RoomEvent::Ready`].
//! 3. Forwards relay control frames as [`RoomEvent`]s and every opaque text
//!    frame as [`RoomEvent::Frame`].
//! 4. Sends outbound opaque frames received on the command channel.
//! 5. On disconnect, emits [`RoomEvent::Disconnected`] and reconnects with
//!    exponential backoff.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use super::protocol::{RoomConfig, RoomControl, RoomEvent};

/// Handle for a relay Room connection. Cloneable senders push opaque frames
/// to the background task; the background task pushes [`RoomEvent`]s back.
pub struct RoomClient {
    /// Outbound opaque text frames to broadcast to the room.
    out_tx: mpsc::Sender<String>,
    /// Disconnect signal for graceful shutdown.
    shutdown_tx: mpsc::Sender<()>,
}

impl RoomClient {
    /// Connect to the relay Room and start the background loop. Returns the
    /// client handle and a receiver of [`RoomEvent`]s.
    ///
    /// If `relay_url` is empty the client is a no-op: the background task is
    /// not spawned and no events are produced (presence is disabled gracefully).
    pub fn connect(config: RoomConfig) -> (Self, mpsc::Receiver<RoomEvent>) {
        let (event_tx, event_rx) = mpsc::channel(256);
        let (out_tx, out_rx) = mpsc::channel(256);
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);

        if config.relay_url.is_empty() {
            debug!("RoomClient: empty relay_url, presence transport disabled");
        } else {
            tokio::spawn(run(config, event_tx, out_rx, shutdown_rx));
        }

        (
            Self {
                out_tx,
                shutdown_tx,
            },
            event_rx,
        )
    }

    /// Queue an opaque text frame for broadcast to the room. Non-blocking.
    pub async fn send(&self, frame: String) {
        let _ = self.out_tx.send(frame).await;
    }

    /// A cloneable sender for queuing outbound opaque frames. Useful for
    /// background tasks (e.g. the presence translator re-announcing on join).
    pub fn frame_sender(&self) -> mpsc::Sender<String> {
        self.out_tx.clone()
    }

    /// Signal the background loop to disconnect and stop reconnecting.
    pub async fn disconnect(&self) {
        let _ = self.shutdown_tx.send(()).await;
    }
}

/// Reconnect loop.
async fn run(
    config: RoomConfig,
    event_tx: mpsc::Sender<RoomEvent>,
    mut out_rx: mpsc::Receiver<String>,
    mut shutdown_rx: mpsc::Receiver<()>,
) {
    let mut backoff = Duration::from_secs(config.reconnect_delay_secs.max(1));
    let max_backoff = Duration::from_secs(config.max_reconnect_delay_secs.max(1));

    loop {
        info!(
            url = %config.relay_url,
            session = %config.session_id,
            member = %config.member_id,
            "Connecting to presence room",
        );

        match tokio::time::timeout(
            Duration::from_secs(15),
            tokio_tungstenite::connect_async(&config.relay_url),
        )
        .await
        {
            Ok(Ok((ws, _))) => {
                backoff = Duration::from_secs(config.reconnect_delay_secs.max(1));
                match session(ws, &config, &event_tx, &mut out_rx, &mut shutdown_rx).await {
                    SessionResult::Shutdown => {
                        info!("Presence room client shutting down");
                        return;
                    }
                    SessionResult::Disconnected(reason) => {
                        warn!(reason = %reason, "Presence room connection lost");
                        let _ = event_tx.send(RoomEvent::Disconnected).await;
                    }
                }
            }
            Ok(Err(e)) => {
                warn!(error = %e, "Failed to connect to presence room");
                let _ = event_tx
                    .send(RoomEvent::RelayError(format!("connect failed: {e}")))
                    .await;
            }
            Err(_) => {
                warn!("Presence room connection timed out after 15s");
                let _ = event_tx
                    .send(RoomEvent::RelayError("connection timed out".into()))
                    .await;
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = shutdown_rx.recv() => return,
        }
        backoff = (backoff * 2).min(max_backoff);
    }
}

enum SessionResult {
    Shutdown,
    Disconnected(String),
}

/// Handle one connected session: hello → ready → forward loop.
async fn session(
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    config: &RoomConfig,
    event_tx: &mpsc::Sender<RoomEvent>,
    out_rx: &mut mpsc::Receiver<String>,
    shutdown_rx: &mut mpsc::Receiver<()>,
) -> SessionResult {
    let (mut sink, mut stream) = ws.split();

    // 1. Send the SIGNED room_hello. The relay REQUIRES a valid signature +
    //    `member_id → pubkey` binding (member-id slot DoS fix); without a signer
    //    the relay rejects us, so this is a hard dependency once cut over.
    let hello = match &config.signer {
        Some(signer) => super::signed_hello::build_signed_room_hello(
            signer,
            &config.session_id,
            &config.member_id,
        ),
        None => {
            // No signer plumbed yet: still emit a hello so the worker stays
            // structurally intact, but the relay will reject it. Logged loudly.
            warn!("Presence room: no room_hello signer; relay will reject this hello");
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

    // 2. Forward loop. `room_ready`, the initial `member_joined` burst, and the
    //    `member_count` all arrive as ordinary control frames and are handled
    //    uniformly below.
    loop {
        tokio::select! {
            // Outbound opaque frames → relay.
            frame = out_rx.recv() => {
                match frame {
                    Some(frame) => {
                        if sink.send(Message::Text(frame.into())).await.is_err() {
                            return SessionResult::Disconnected("send failed".into());
                        }
                    }
                    // The command channel was dropped: the owner is gone.
                    None => return SessionResult::Shutdown,
                }
            }

            // Inbound frames from the relay.
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<RoomControl>(&text) {
                            Ok(RoomControl::RoomReady { session_id }) => {
                                info!(session = %session_id, "Presence room ready");
                                let _ = event_tx.send(RoomEvent::Ready { session_id }).await;
                            }
                            Ok(RoomControl::MemberJoined { member_id }) => {
                                let _ = event_tx.send(RoomEvent::MemberJoined { member_id }).await;
                            }
                            Ok(RoomControl::MemberLeft { member_id }) => {
                                let _ = event_tx.send(RoomEvent::MemberLeft { member_id }).await;
                            }
                            Ok(RoomControl::MemberCount { count }) => {
                                let _ = event_tx.send(RoomEvent::MemberCount { count }).await;
                            }
                            Ok(RoomControl::MemberFrame { member_id, payload }) => {
                                let _ = event_tx
                                    .send(RoomEvent::Frame {
                                        member_id,
                                        text: payload,
                                    })
                                    .await;
                            }
                            Ok(RoomControl::Error { message }) => {
                                warn!(error = %message, "Presence room relay error");
                                let _ = event_tx.send(RoomEvent::RelayError(message)).await;
                            }
                            // Unrecognised control frame — drop silently.
                            Err(_) => {}
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

            // Shutdown.
            _ = shutdown_rx.recv() => {
                let _ = sink.close().await;
                return SessionResult::Shutdown;
            }
        }
    }
}
