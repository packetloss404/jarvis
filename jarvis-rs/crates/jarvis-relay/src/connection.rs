//! Per-connection handler: classify, register, then forward messages.

use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::protocol::{RelayHello, RelayResponse};
use crate::rate_limit::RateLimiter;
use crate::room_auth::RoomAuthStore;
use crate::session::{Role, SessionKind, SessionStore};

/// The signed-`room_hello` credential carried alongside a Room member's hello:
/// `(pubkey, nonce, sig)`. The relay verifies this and TOFU-pins
/// `member_id → pubkey` before admitting the slot. Only Room hellos carry it.
type RoomCredential = (String, u64, String);

/// Handle a single WebSocket connection.
pub async fn handle_connection(
    ws: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    addr: SocketAddr,
    store: SessionStore,
    room_auth: RoomAuthStore,
    limiter: &RateLimiter,
) {
    let (mut sink, mut stream) = ws.split();

    // 1. Read the hello message to identify this client. Room hellos also carry
    //    a signed credential (`pubkey`, `nonce`, `sig`) in `credential`.
    let (session_id, role, member_id, credential) = match read_hello(&mut stream, addr).await {
        Some(v) => v,
        None => return,
    };

    // Validate session ID: non-empty, bounded length, conservative charset.
    if !is_valid_member_id(&session_id, limiter.max_session_id_len()) {
        let _ = send_response(
            &mut sink,
            &RelayResponse::Error {
                message: "invalid session ID".into(),
            },
        )
        .await;
        return;
    }

    // Validate the room member_id like the session_id: non-empty, bounded
    // length, and a conservative charset. Only Room hellos carry one.
    if let Some(mid) = member_id.as_deref() {
        if !is_valid_member_id(mid, limiter.max_session_id_len()) {
            let _ = send_response(
                &mut sink,
                &RelayResponse::Error {
                    message: "invalid member ID".into(),
                },
            )
            .await;
            return;
        }
    }

    // Bridge sessions follow the existing desktop/mobile flow.
    match role.session_kind() {
        SessionKind::Bridge => match role {
            Role::Desktop => {
                // Enforce global session cap.
                if store.count().await >= limiter.max_total_sessions() {
                    // Check if this is a reconnect (session already exists).
                    if !store.exists(&session_id).await {
                        let _ = send_response(
                            &mut sink,
                            &RelayResponse::Error {
                                message: "server at capacity".into(),
                            },
                        )
                        .await;
                        return;
                    }
                }

                match store.ensure_session(&session_id, SessionKind::Bridge).await {
                    Ok(true) => {}
                    Ok(false) => {
                        // Session already exists -- desktop is reconnecting. Allow it by
                        // unregistering the old desktop first.
                        store.unregister(&session_id, Role::Desktop).await;
                        let _ = store.ensure_session(&session_id, SessionKind::Bridge).await;
                    }
                    Err(_) => {
                        let _ = send_response(
                            &mut sink,
                            &RelayResponse::Error {
                                message: "session kind mismatch".into(),
                            },
                        )
                        .await;
                        return;
                    }
                }
            }
            Role::Mobile => {
                // Verify session exists (created by desktop)
                if !store.exists(&session_id).await {
                    let _ = send_response(
                        &mut sink,
                        &RelayResponse::Error {
                            message: "session not found".into(),
                        },
                    )
                    .await;
                    return;
                }
            }
            _ => {}
        },
        SessionKind::Broadcast => {
            // Enforce the global session cap. Allow joining an existing session
            // (e.g. a spectator connecting after the host created it), but
            // reject creation of a brand-new broadcast session when the server
            // is at capacity.
            if !store.exists(&session_id).await
                && store.count().await >= limiter.max_total_sessions()
            {
                tracing::warn!("Total session cap reached, rejecting broadcast session");
                let _ = send_response(
                    &mut sink,
                    &RelayResponse::Error {
                        message: "server at capacity".into(),
                    },
                )
                .await;
                return;
            }

            if let Err(e) = store
                .ensure_session(&session_id, SessionKind::Broadcast)
                .await
            {
                let _ = send_response(&mut sink, &RelayResponse::Error { message: e.into() }).await;
                return;
            }

            if matches!(role, Role::Host) {
                store.unregister(&session_id, Role::Host).await;
            }
        }
        SessionKind::Room => {
            // SIGNED room_hello gate: verify the ECDSA signature over the
            // canonical bytes against the carried pubkey, check nonce freshness,
            // and enforce the TOFU `member_id → pubkey` binding BEFORE the slot
            // can be created/joined/replaced. This is the member-id slot DoS
            // fix: a self-asserted member_id can no longer squat/evict a slot
            // without the matching identity key. A forged hello is refused here,
            // before `ensure_session` can even auto-create the room.
            let mid = member_id.as_deref().unwrap_or_default();
            match &credential {
                Some((pubkey, nonce, sig)) => {
                    // PURE verify only — the TOFU pin / nonce high-water is
                    // committed LATER, after `register_room` succeeds, so an
                    // early return (capacity / ensure_session / register / first
                    // send) below never leaves an orphan pin or advances the
                    // nonce. See `RoomAuthStore::{verify, commit}`.
                    if let Err(reason) =
                        room_auth.verify(&session_id, mid, pubkey, *nonce, sig).await
                    {
                        tracing::warn!(
                            peer = %addr,
                            reason = ?reason,
                            "Rejected signed room_hello",
                        );
                        let _ = send_response(
                            &mut sink,
                            &RelayResponse::Error {
                                message: reason.message().into(),
                            },
                        )
                        .await;
                        return;
                    }
                }
                None => {
                    // BREAKING cutover: an unsigned room_hello is no longer
                    // admitted. All four clients sign; the relay requires it.
                    tracing::warn!(peer = %addr, "Rejected unsigned room_hello");
                    let _ = send_response(
                        &mut sink,
                        &RelayResponse::Error {
                            message: "signed room hello required".into(),
                        },
                    )
                    .await;
                    return;
                }
            }

            // Enforce the global session cap when a member would create a new
            // room. Joining an existing room never trips the cap.
            if !store.exists(&session_id).await
                && store.count().await >= limiter.max_total_sessions()
            {
                let _ = send_response(
                    &mut sink,
                    &RelayResponse::Error {
                        message: "server at capacity".into(),
                    },
                )
                .await;
                return;
            }

            // Auto-create on first member, like Broadcast does.
            if let Err(e) = store.ensure_session(&session_id, SessionKind::Room).await {
                let _ = send_response(&mut sink, &RelayResponse::Error { message: e.into() }).await;
                return;
            }
        }
    }

    // 2. Create our receive channel and register.
    let (tx, mut rx) = mpsc::channel::<String>(256);
    // Keep a clone of our own sender so room cleanup can prove the stored slot
    // still belongs to THIS connection (see `unregister_member`).
    let my_tx = tx.clone();
    let peer_tx = match role.session_kind() {
        SessionKind::Bridge => match store.register_bridge(&session_id, role, tx).await {
            Ok(peer) => peer,
            Err(e) => {
                let _ = send_response(
                    &mut sink,
                    &RelayResponse::Error {
                        message: e.to_string(),
                    },
                )
                .await;
                return;
            }
        },
        SessionKind::Broadcast => match store.register_broadcast(&session_id, role, tx).await {
            Ok(registration) => {
                if matches!(role, Role::Spectator) && registration.host_connected {
                    let _ = send_response(&mut sink, &RelayResponse::HostConnected).await;
                }
                None
            }
            Err(e) => {
                let _ = send_response(
                    &mut sink,
                    &RelayResponse::Error {
                        message: e.to_string(),
                    },
                )
                .await;
                return;
            }
        },
        SessionKind::Room => {
            // `member_id` is always Some for Room hellos (see read_hello).
            let mid = member_id.clone().unwrap_or_default();
            if let Err(e) = store.register_room(&session_id, mid.clone(), tx).await {
                let _ = send_response(
                    &mut sink,
                    &RelayResponse::Error {
                        message: e.to_string(),
                    },
                )
                .await;
                return;
            }
            // The slot is now actually registered — COMMIT the TOFU pin + nonce
            // high-water. `credential` is always Some for an admitted Room hello
            // (an unsigned/forged one returned above). The commit re-checks the
            // pubkey + monotonic nonce under the write lock; if a concurrent
            // hello raced the pin, tear our just-registered slot back down so we
            // never hold a slot we couldn't pin.
            if let Some((pubkey, nonce, _sig)) = &credential {
                if let Err(reason) = room_auth
                    .commit(&session_id, &mid, pubkey, *nonce)
                    .await
                {
                    tracing::warn!(
                        peer = %addr,
                        reason = ?reason,
                        "Signed room_hello lost pin commit race; refusing slot",
                    );
                    if store.unregister_member(&session_id, &mid, &my_tx).await {
                        room_auth.forget_session(&session_id).await;
                    }
                    let _ = send_response(
                        &mut sink,
                        &RelayResponse::Error {
                            message: reason.message().into(),
                        },
                    )
                    .await;
                    return;
                }
            }
            None
        }
    };

    // For Room (pair) sessions the session_id is the room's CAPABILITY SECRET,
    // so log only a truncated fingerprint; other roles keep the full id.
    tracing::info!(
        peer = %addr,
        session = %log_session(&session_id, role),
        role = ?role,
        "Client registered"
    );

    // 3. Send the ready response (room_ready for Room, session_ready otherwise).
    let ready = if role == Role::Member {
        RelayResponse::RoomReady {
            session_id: session_id.clone(),
        }
    } else {
        RelayResponse::SessionReady {
            session_id: session_id.clone(),
        }
    };
    if send_response(&mut sink, &ready).await.is_err() {
        if role == Role::Member {
            store
                .unregister_member(
                    &session_id,
                    member_id.as_deref().unwrap_or_default(),
                    &my_tx,
                )
                .await;
        } else {
            store.unregister(&session_id, role).await;
        }
        return;
    }

    // 4. Notify peers.
    match role.session_kind() {
        SessionKind::Bridge => {
            if let Some(ref peer) = peer_tx {
                let _ = send_response(&mut sink, &RelayResponse::PeerConnected).await;
                let json = serde_json::to_string(&RelayResponse::PeerConnected).unwrap();
                let _ = peer.send(json).await;
            }
        }
        SessionKind::Broadcast => {
            if matches!(role, Role::Host) {
                let json = serde_json::to_string(&RelayResponse::HostConnected).unwrap();
                for peer in store.spectator_targets(&session_id).await {
                    let _ = peer.send(json.clone()).await;
                }
            }
            notify_viewer_count(&store, &session_id).await;
        }
        SessionKind::Room => {
            let me = member_id.clone().unwrap_or_default();

            // Tell every other member that this member joined.
            let joined = serde_json::to_string(&RelayResponse::MemberJoined {
                member_id: me.clone(),
            })
            .unwrap();
            for peer in store.room_targets_excluding(&session_id, &me).await {
                let _ = peer.send(joined.clone()).await;
            }

            // Send the joining member the current roster (every OTHER member),
            // so it can build its local presence list.
            for other in store.room_member_ids(&session_id).await {
                if other == me {
                    continue;
                }
                let _ = send_response(
                    &mut sink,
                    &RelayResponse::MemberJoined { member_id: other },
                )
                .await;
            }

            // Followed by the current member count.
            let _ = send_response(
                &mut sink,
                &RelayResponse::MemberCount {
                    count: store.room_member_count(&session_id).await,
                },
            )
            .await;
        }
    }

    // 5. Forwarding loop.
    loop {
        tokio::select! {
            // Messages from our receive channel -> send to this client's WebSocket
            Some(msg) = rx.recv() => {
                if sink.send(Message::Text(msg.into())).await.is_err() {
                    break;
                }
            }

            // Messages from this client's WebSocket -> forward to peer
            frame = stream.next() => {
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        if role.session_kind() == SessionKind::Bridge {
                            if let Some(peer) = store.get_peer_tx(&session_id, role).await {
                                // Mobile sends `{"type":"ping"}` on the WebSocket for keepalive; desktop
                                // does not consume it as a relay envelope — drop here to avoid noise.
                                if !is_relay_keepalive_ping(&text) {
                                    if peer.send(text.to_string()).await.is_err() {
                                        tracing::debug!(session = %session_id, "Peer channel closed");
                                    }
                                }
                            }
                        } else if role == Role::Member {
                            // Fan out to every OTHER member; a member never receives
                            // its own frame. Drop keepalive pings like the bridge does.
                            if !is_relay_keepalive_ping(&text) {
                                let me = member_id.as_deref().unwrap_or_default();
                                // Wrap the raw payload in a MemberFrame envelope so
                                // receivers know the relay-authenticated sender
                                // member_id without trusting any self-asserted field
                                // inside the payload itself.
                                let envelope = serde_json::to_string(
                                    &RelayResponse::MemberFrame {
                                        member_id: me.to_string(),
                                        payload: text.to_string(),
                                    },
                                )
                                .unwrap_or_default();
                                for peer in store.room_targets_excluding(&session_id, me).await {
                                    if peer.send(envelope.clone()).await.is_err() {
                                        tracing::debug!(session = %session_id, "Member channel closed");
                                    }
                                }
                            }
                        } else if matches!(role, Role::Host) {
                            for peer in store.spectator_targets(&session_id).await {
                                if peer.send(text.to_string()).await.is_err() {
                                    tracing::debug!(session = %session_id, "Spectator channel closed");
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = sink.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        tracing::debug!(peer = %addr, error = %e, "WS error");
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    // 6. Cleanup.
    tracing::info!(
        peer = %addr,
        session = %log_session(&session_id, role),
        role = ?role,
        "Client disconnected"
    );

    drop(rx);

    match role.session_kind() {
        SessionKind::Bridge => {
            store.unregister(&session_id, role).await;
            if let Some(peer) = store.get_peer_tx(&session_id, role).await {
                let json = serde_json::to_string(&RelayResponse::PeerDisconnected).unwrap();
                let _ = peer.send(json).await;
            }
        }
        SessionKind::Broadcast => {
            store.unregister(&session_id, role).await;
            if matches!(role, Role::Host) {
                let json = serde_json::to_string(&RelayResponse::HostDisconnected).unwrap();
                for peer in store.spectator_targets(&session_id).await {
                    let _ = peer.send(json.clone()).await;
                }
            }
            notify_viewer_count(&store, &session_id).await;
        }
        SessionKind::Room => {
            let me = member_id.as_deref().unwrap_or_default();
            // `unregister_member` returns true when it removed the now-empty
            // room; drop that room's TOFU pins so the pin map tracks live
            // sessions (the same `(session_id, member_id)` can later be re-pinned
            // by a fresh signed join).
            if store.unregister_member(&session_id, me, &my_tx).await {
                room_auth.forget_session(&session_id).await;
            }

            // Fan out the departure + an updated count to whoever is left.
            let left = serde_json::to_string(&RelayResponse::MemberLeft {
                member_id: me.to_string(),
            })
            .unwrap();
            let count = serde_json::to_string(&RelayResponse::MemberCount {
                count: store.room_member_count(&session_id).await,
            })
            .unwrap();
            for peer in store.room_targets_excluding(&session_id, me).await {
                let _ = peer.send(left.clone()).await;
                let _ = peer.send(count.clone()).await;
            }
        }
    }
}

/// Validate a room `member_id`: non-empty, at most `max_len` bytes, and made up
/// only of a conservative charset (alphanumeric plus `-`, `_`, `.`). This mirrors
/// the bound applied to `session_id` and rejects control characters / oversized
/// ids before they enter the room roster.
fn is_valid_member_id(member_id: &str, max_len: usize) -> bool {
    !member_id.is_empty()
        && member_id.len() <= max_len
        && member_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn is_relay_keepalive_ping(text: &str) -> bool {
    // Cheap heuristic: the keepalive ping is a small JSON object containing
    // the string "ping". Avoid a full serde_json parse on every forwarded
    // frame — this is called in the hot forwarding loop.
    text.len() < 64 && text.contains("\"ping\"")
}

/// Read and parse the first message as a RelayHello.
async fn read_hello(
    stream: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    >,
    addr: SocketAddr,
) -> Option<(String, Role, Option<String>, Option<RoomCredential>)> {
    // Wait up to 10 seconds for the hello message.
    let frame = tokio::time::timeout(std::time::Duration::from_secs(10), stream.next()).await;

    match frame {
        Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<RelayHello>(&text) {
            Ok(RelayHello::DesktopHello { session_id }) => {
                Some((session_id, Role::Desktop, None, None))
            }
            Ok(RelayHello::MobileHello { session_id }) => {
                Some((session_id, Role::Mobile, None, None))
            }
            Ok(RelayHello::HostHello { session_id }) => Some((session_id, Role::Host, None, None)),
            Ok(RelayHello::SpectatorHello { session_id }) => {
                Some((session_id, Role::Spectator, None, None))
            }
            Ok(RelayHello::RoomHello {
                session_id,
                member_id,
                pubkey,
                nonce,
                sig,
            }) => Some((
                session_id,
                Role::Member,
                Some(member_id),
                Some((pubkey, nonce, sig)),
            )),
            Err(e) => {
                tracing::warn!(peer = %addr, error = %e, "Invalid hello message");
                None
            }
        },
        Ok(Some(Ok(_))) => {
            tracing::warn!(peer = %addr, "Expected text hello, got binary");
            None
        }
        Ok(Some(Err(e))) => {
            tracing::warn!(peer = %addr, error = %e, "WS error during hello");
            None
        }
        Ok(None) => {
            tracing::debug!(peer = %addr, "Connection closed before hello");
            None
        }
        Err(_) => {
            tracing::warn!(peer = %addr, "Hello timeout (10s)");
            None
        }
    }
}

/// Render a session id for logging. For `Role::Member` (pair Room) the
/// session_id is the room's capability secret, so emit only a non-reversible
/// fingerprint (first 6 chars + length). Other roles keep the full id.
fn log_session(session_id: &str, role: Role) -> String {
    if role == Role::Member {
        let prefix: String = session_id.chars().take(6).collect();
        format!("{prefix}…(len={})", session_id.len())
    } else {
        session_id.to_string()
    }
}

/// Send a RelayResponse as a JSON text frame.
async fn send_response(
    sink: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        Message,
    >,
    response: &RelayResponse,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    let json = serde_json::to_string(response).unwrap();
    sink.send(Message::Text(json.into())).await
}

async fn notify_viewer_count(store: &SessionStore, session_id: &str) {
    let json = serde_json::to_string(&RelayResponse::ViewerCount {
        count: store.viewer_count(session_id).await,
    })
    .unwrap();

    for peer in store.broadcast_targets(session_id).await {
        let _ = peer.send(json.clone()).await;
    }
}

#[cfg(test)]
mod member_id_tests {
    use super::is_valid_member_id;

    #[test]
    fn accepts_reasonable_ids() {
        assert!(is_valid_member_id("abc123", 64));
        assert!(is_valid_member_id("host-1_a.b", 64));
        assert!(is_valid_member_id("A", 64));
    }

    #[test]
    fn rejects_empty_or_oversized() {
        assert!(!is_valid_member_id("", 64));
        assert!(!is_valid_member_id(&"a".repeat(65), 64));
    }

    #[test]
    fn rejects_bad_charset() {
        assert!(!is_valid_member_id("has space", 64));
        assert!(!is_valid_member_id("new\nline", 64));
        assert!(!is_valid_member_id("emoji😀", 64));
        assert!(!is_valid_member_id("semi;colon", 64));
    }
}
