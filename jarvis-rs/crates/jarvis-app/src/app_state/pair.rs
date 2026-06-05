//! Pair programming: start/poll wiring for the collaborative terminal.
//!
//! Models `social.rs` (presence). `start_pair` constructs the `PairManager`,
//! spins up the room worker on the shared `self.tokio_runtime`, drains
//! `PairManager`'s `event_rx` → `PairFrame` → encrypt → room (in an async
//! bridge task), and routes inbound frames back over a sync mpsc that
//! `poll_pair` drains on the main thread.
//!
//! `start_pair` is idempotent: it is wired once at startup (a no-op while
//! `collab.enabled` is false), and the `pair_start` / `pair_join` IPC handlers
//! seed `pair_session_id` / `pair_member_id` / `pair_host_pane_id` and re-invoke
//! it to bring up the room connection (the same pattern `revoke_mobile_pairing`
//! uses to (re)start the relay client).

#![allow(dead_code)]

use std::sync::Arc;

use jarvis_social::{PairConfig, PairEvent, PairManager};

use super::core::JarvisApp;
use crate::app_state::ws_server::pair_protocol::{PairFrame, MAX_TERM_INPUT_BYTES};
use crate::app_state::ws_server::pair_room_client::{
    run_pair_room_client, PairRoomClientConfig, PairRoomCommand, PairRoomEvent,
};

/// Messages routed from the async room worker → main thread (drained in `poll_pair`).
pub(in crate::app_state) enum PairInbound {
    /// We successfully joined the room as `member_id`.
    RoomReady { session_id: String, member_id: String },
    /// A member joined the room.
    MemberJoined { member_id: String },
    /// A member left the room.
    MemberLeft { member_id: String },
    /// Current member count fan-out.
    MemberCount { count: u32 },
    /// An inbound application frame to apply locally / forward to the panel.
    Frame(PairFrame),
    /// The room connection dropped.
    Disconnected,
    /// A transport or session error.
    Error(String),
}

/// Commands sent from the main thread → async room worker.
pub(in crate::app_state) enum PairCommand {
    /// Host PTY output to fan out to the room (non-blocking `try_send`).
    Output(Vec<u8>),
    /// Send an arbitrary application frame to the room.
    Send(PairFrame),
    /// End the pair session and tear down the room connection.
    Leave,
}

impl JarvisApp {
    /// Start the pair-programming room worker.
    ///
    /// No-op unless `config.collab.enabled` and `config.relay.url` is set, and a
    /// no-op if a worker is already running (`pair_cmd_tx` present). Reuses
    /// `self.tokio_runtime` via `get_or_insert_with` — never spawns a second
    /// runtime.
    pub(in crate::app_state) fn start_pair(&mut self) {
        if !self.config.collab.enabled {
            return;
        }
        if self.config.relay.url.is_empty() {
            tracing::debug!("Pair skipped: no relay.url configured");
            return;
        }
        // Already running — handlers re-invoke start_pair, so guard against
        // spawning a second worker for the same app session.
        if self.pair_cmd_tx.is_some() {
            return;
        }

        // SECURITY (M3): pair sessions are EXPERIMENTAL and UNAUTHENTICATED.
        // The room key gives confidentiality from the relay but NOT per-member
        // authentication: members share a symmetric key and assert their own
        // identity, so impersonation / host-spoofing are possible. Surfaced
        // once here (gated on `collab.enabled`) so this is never silently on.
        tracing::warn!(
            require_signed_join = self.config.collab.require_signed_join,
            "Pair programming is EXPERIMENTAL: sessions are unauthenticated; the \
             room key is confidentiality-only and NOT a security boundary between \
             members (per-member auth is deferred to M3). Do not share secrets."
        );

        let relay_url = self.config.relay.url.clone();
        let session_id = self
            .pair_session_id
            .get_or_insert_with(generate_session_id)
            .clone();
        let member_id = self
            .pair_member_id
            .get_or_insert_with(generate_member_id)
            .clone();

        let pair_config = PairConfig {
            enabled: self.config.collab.enabled,
            max_participants: self.config.collab.max_participants,
            allow_takeover: self.config.collab.allow_takeover,
        };
        let (manager, mut event_rx) = PairManager::new(pair_config);
        let manager = Arc::new(manager);

        // main-thread inbound (poll_pair drains this)
        let (inbound_tx, inbound_rx) = std::sync::mpsc::channel::<PairInbound>();
        // main-thread → worker outbound commands (PTY output, frames, leave)
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<PairCommand>(256);
        // room client → bridge: the room worker emits PairRoomEvent over std mpsc
        let (room_event_tx, room_event_rx) = std::sync::mpsc::channel::<PairRoomEvent>();
        // bridge → room client: outbound PairFrames to encrypt + fan out
        let (room_cmd_tx, room_cmd_rx) = tokio::sync::mpsc::channel::<PairRoomCommand>(256);
        // 32-byte room key, delivered to the room client over a watch channel
        let (key_tx, key_rx) = tokio::sync::watch::channel::<Option<[u8; 32]>>(None);

        // Derive the M1 room-symmetric AES key from the session id (same scheme
        // as chat channels) and hand it to the worker before it starts sending.
        if let Some(ref mut crypto) = self.crypto {
            let handle = crypto.derive_room_key(&session_id);
            match crypto.export_key(handle) {
                Ok(key_bytes) => {
                    let _ = key_tx.send(Some(key_bytes));
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to export derived pair room key");
                }
            }
        } else {
            tracing::warn!("CryptoService unavailable; pair room frames will not be sent");
        }

        let room_config = PairRoomClientConfig {
            relay_url,
            session_id: session_id.clone(),
            member_id: member_id.clone(),
            key_rx,
        };

        let manager_for_bridge = Arc::clone(&manager);
        let room_cmd_tx_for_cmds = room_cmd_tx.clone();

        let rt = self.tokio_runtime.get_or_insert_with(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime for pair client")
        });

        // 1. The outbound room client (connect → room_hello → forward loop).
        rt.spawn(async move {
            run_pair_room_client(room_config, room_cmd_rx, room_event_tx).await;
        });

        // 2. Bridge: drain main-thread PairCommands + PairManager events, convert
        //    to PairFrames, and push them to the room client. The host writes
        //    PTY output via PairCommand::Output; PairManager mutations (driver
        //    changes, cursor, resize) arrive as PairEvents.
        let session_id_for_bridge = session_id.clone();
        rt.spawn(async move {
            loop {
                tokio::select! {
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(PairCommand::Output(data)) => {
                                let frame = PairFrame::PtyOutput { data };
                                if room_cmd_tx_for_cmds
                                    .send(PairRoomCommand::Send(frame))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Some(PairCommand::Send(frame)) => {
                                if room_cmd_tx_for_cmds
                                    .send(PairRoomCommand::Send(frame))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Some(PairCommand::Leave) | None => {
                                let _ = room_cmd_tx_for_cmds
                                    .send(PairRoomCommand::Shutdown)
                                    .await;
                                break;
                            }
                        }
                    }
                    event = event_rx.recv() => {
                        match event {
                            Some(ev) => {
                                if let Some(frame) =
                                    pair_event_to_frame(&session_id_for_bridge, ev)
                                {
                                    if room_cmd_tx_for_cmds
                                        .send(PairRoomCommand::Send(frame))
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
            // Keep the manager alive for the bridge's lifetime.
            drop(manager_for_bridge);
        });

        // 3. Forward room client events (std mpsc) → PairInbound (std mpsc) on a
        //    dedicated OS thread, so poll_pair drains a single typed channel.
        std::thread::spawn(move || {
            while let Ok(ev) = room_event_rx.recv() {
                let inbound = match ev {
                    PairRoomEvent::RoomReady {
                        session_id,
                        member_id,
                    } => PairInbound::RoomReady {
                        session_id,
                        member_id,
                    },
                    PairRoomEvent::MemberJoined { member_id } => {
                        PairInbound::MemberJoined { member_id }
                    }
                    PairRoomEvent::MemberLeft { member_id } => {
                        PairInbound::MemberLeft { member_id }
                    }
                    PairRoomEvent::MemberCount { count } => PairInbound::MemberCount { count },
                    PairRoomEvent::Frame(frame) => PairInbound::Frame(frame),
                    PairRoomEvent::Disconnected => PairInbound::Disconnected,
                    PairRoomEvent::Error(msg) => PairInbound::Error(msg),
                };
                if inbound_tx.send(inbound).is_err() {
                    break;
                }
            }
        });

        self.pair_manager = Some(manager);
        self.pair_inbound_rx = Some(inbound_rx);
        self.pair_cmd_tx = Some(cmd_tx);
        self.pair_session_id = Some(session_id.clone());
        self.pair_member_id = Some(member_id.clone());
        self.pair_key_tx = Some(key_tx);

        tracing::info!(member = %member_id, "Pair room client started");
    }

    /// Poll inbound pair events (non-blocking).
    ///
    /// Drains `self.pair_inbound_rx`, applies host-authoritative frames
    /// (driver-gated `term_input` → host PTY; `request_control` → driver
    /// handoff), and forwards `pair_event` IPC to the pair panel.
    pub(in crate::app_state) fn poll_pair(&mut self) {
        // Collect first so we don't hold an immutable borrow of `self` while we
        // mutate `self.ptys` / send IPC.
        let mut events = Vec::new();
        if let Some(ref rx) = self.pair_inbound_rx {
            while let Ok(ev) = rx.try_recv() {
                events.push(ev);
            }
        }
        if events.is_empty() {
            return;
        }

        for ev in events {
            match ev {
                PairInbound::RoomReady {
                    session_id,
                    member_id,
                } => {
                    tracing::info!(member = %member_id, "Pair room ready");
                    // The panel handles `connected` (sets the status dot); the
                    // raw `room_ready` event is dropped by the panel. Emit
                    // `connected` so the dot turns green on first connect AND on
                    // every reconnect (RoomReady fires again after a relay drop).
                    self.send_pair_event(&serde_json::json!({ "event": "connected" }));
                    // Re-send a full status snapshot so a navigator's roster /
                    // host / driver / role re-sync after a reconnect (the relay
                    // does no replay, so without this a reconnected navigator
                    // would keep stale state and a grey dot).
                    if let Some(pane_id) = self.pair_host_pane_id {
                        self.send_pair_status_to_pane(pane_id);
                    }
                    if self.is_pair_host() {
                        // Host: re-announce session meta + roster.
                        self.rebroadcast_session_meta(&session_id);
                    } else {
                        // Navigator: announce our display name to the host so it
                        // registers us (driver gating) and re-broadcasts the
                        // roster with our real name. Re-sent on every reconnect.
                        let name = self.pair_display_name.clone().unwrap_or_default();
                        self.send_pair_frame(PairFrame::Join {
                            from: member_id,
                            name,
                        });
                    }
                }
                PairInbound::MemberJoined { member_id } => {
                    // Host side: register the new member in the PairManager so
                    // `set_driver` / `relay_input` recognise it, then re-broadcast
                    // SessionMeta (+ roster) so this late joiner learns host
                    // name / dims / takeover and everyone sees the roster. The
                    // real display name arrives via the navigator's `Join` frame,
                    // which upgrades the placeholder.
                    if self.is_pair_host() {
                        if let Some(session_id) = self.pair_session_id.clone() {
                            self.host_register_member(&session_id, &member_id, &member_id);
                        }
                    }
                    self.send_pair_event(&serde_json::json!({
                        "event": "member_joined",
                        "member_id": member_id,
                    }));
                }
                PairInbound::MemberLeft { member_id } => {
                    self.send_pair_event(&serde_json::json!({
                        "event": "member_left",
                        "member_id": member_id,
                    }));
                }
                PairInbound::MemberCount { count } => {
                    self.send_pair_event(&serde_json::json!({
                        "event": "member_count",
                        "count": count,
                    }));
                }
                PairInbound::Frame(frame) => self.apply_pair_frame(frame),
                PairInbound::Disconnected => {
                    self.send_pair_event(&serde_json::json!({ "event": "disconnected" }));
                }
                PairInbound::Error(msg) => {
                    tracing::warn!(error = %msg, "Pair room error");
                    self.send_pair_event(&serde_json::json!({
                        "event": "error",
                        "message": msg,
                    }));
                }
            }
            self.needs_redraw = true;
        }
    }

    /// Apply a single inbound `PairFrame` host-authoritatively.
    ///
    /// SECURITY (M3 — NOT YET IMPLEMENTED):
    /// The M1 room key gives confidentiality from the relay but NO per-member
    /// authentication: the room key is a shared symmetric secret and `from`/
    /// `member_id` are self-asserted. The following checks are deferred to M3
    /// (signed-join + host-authority + per-member ECDH) and are MISSING today:
    ///   - Host-authority: host-only frames (`PtyOutput`, `DriverChanged`,
    ///     `Snapshot`, `SessionMeta`) are NOT origin-checked, so any member can
    ///     forge them. M3 must drop host-only frames not actually from the host
    ///     (spec risk #7).
    ///   - `from` authentication: `TermInput`/`RequestControl`/`Cursor`/`Join`
    ///     carry a self-asserted `from`, so a member can impersonate the driver
    ///     and inject keystrokes (spec risk #2). M3 must verify a signed-join /
    ///     per-member key before trusting `from`.
    ///   - The host `member_id` slot can be hijacked (no ownership proof).
    /// Until then `collab.enabled` defaults false and the room key is
    /// documented as confidentiality-only, not a security boundary.
    fn apply_pair_frame(&mut self, frame: PairFrame) {
        let Some(session_id) = self.pair_session_id.clone() else {
            return;
        };
        match frame {
            // Driver keystrokes: the host re-validates via the PairManager
            // (driver-gated) before writing to its PTY.
            PairFrame::TermInput { from, mut data } => {
                let Some(host_pane) = self.pair_host_pane_id else {
                    // Only the host owns the PTY; navigators ignore term_input.
                    return;
                };
                let Some(manager) = self.pair_manager.clone() else {
                    return;
                };
                // Defence in depth: cap inbound keystroke size before any write
                // (a hostile peer can forge a frame straight onto the wire).
                if data.len() > MAX_TERM_INPUT_BYTES {
                    tracing::debug!(len = data.len(), "Truncating oversize inbound pair term_input");
                    data.truncate(MAX_TERM_INPUT_BYTES);
                }
                let allowed = self.tokio_runtime.as_ref().is_some_and(|rt| {
                    rt.block_on(manager.relay_input(&session_id, &from, data.clone()))
                        .is_ok()
                });
                if allowed {
                    if let Err(e) = self.ptys.write_input(host_pane, &data) {
                        tracing::warn!(error = %e, "Pair term_input write failed");
                    }
                } else {
                    tracing::debug!(from = %from, "Rejected pair term_input (not driver)");
                }
            }
            // Navigator announces presence + display name. The host registers
            // it in the PairManager (so driver gating / set_driver recognise the
            // navigator) and re-broadcasts the roster so everyone sees real
            // names. NOTE (M3): `from`/`name` are self-asserted — see the
            // SECURITY block above.
            PairFrame::Join { from, name } => {
                if !self.is_pair_host() {
                    return; // only the host owns the roster
                }
                // host_register_member sanitizes the display name and re-emits
                // the roster to the room (defined in pair_handlers.rs, where the
                // sanitizer + frame helpers live).
                self.host_register_member(&session_id, &from, &name);
            }
            // Navigator asks for the driver seat: host honors via set_driver
            // (which enforces allow_takeover) and broadcasts driver_changed.
            PairFrame::RequestControl { from } => {
                if self.pair_host_pane_id.is_none() {
                    return; // only the host arbitrates control
                }
                let Some(manager) = self.pair_manager.clone() else {
                    return;
                };
                let host_member = self.pair_member_id.clone().unwrap_or_default();
                if let Some(rt) = self.tokio_runtime.as_ref() {
                    match rt.block_on(manager.set_driver(&session_id, &host_member, &from)) {
                        Ok(()) => tracing::info!(new_driver = %from, "Granted pair control"),
                        Err(e) => tracing::debug!(error = %e, "Denied pair control request"),
                    }
                }
            }
            // Cursor / driver / resize / output / meta / snapshot: forward to the
            // panel for rendering. The PairManager bridge handles broadcast on the
            // host side; here we surface remote state to the local panel.
            PairFrame::PtyOutput { data } => {
                // Send the raw bytes as base64 (lossless): PTY output is arbitrary
                // bytes (escape sequences, partial UTF-8), so a lossy String would
                // corrupt the stream. The panel base64-decodes before term.write.
                self.send_pair_event(&serde_json::json!({
                    "event": "pty_output",
                    "data": b64_encode(&data),
                }));
            }
            PairFrame::Cursor { from, row, col } => {
                self.send_pair_event(&serde_json::json!({
                    "event": "cursor",
                    "from": from,
                    "row": row,
                    "col": col,
                }));
            }
            PairFrame::Resize { cols, rows } => {
                self.send_pair_event(&serde_json::json!({
                    "event": "resize",
                    "cols": cols,
                    "rows": rows,
                }));
            }
            PairFrame::DriverChanged {
                new_driver,
                old_driver,
            } => {
                self.send_pair_event(&serde_json::json!({
                    "event": "driver_changed",
                    "new_driver": new_driver,
                    "old_driver": old_driver,
                }));
            }
            PairFrame::Snapshot {
                data,
                cols,
                rows,
                driver,
            } => {
                self.send_pair_event(&serde_json::json!({
                    "event": "snapshot",
                    "data": b64_encode(&data),
                    "cols": cols,
                    "rows": rows,
                    "driver": driver,
                }));
            }
            PairFrame::SessionMeta {
                host,
                host_name,
                cols,
                rows,
                allow_takeover,
                roster,
            } => {
                let roster_json: Vec<serde_json::Value> = roster
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "member_id": r.member_id,
                            "name": r.name,
                            "role": r.role,
                        })
                    })
                    .collect();
                self.send_pair_event(&serde_json::json!({
                    "event": "session_meta",
                    "host": host,
                    "host_name": host_name,
                    "cols": cols,
                    "rows": rows,
                    "allow_takeover": allow_takeover,
                    "roster": roster_json,
                }));
            }
        }
    }

    /// Enqueue host PTY output to fan out to the room (non-blocking `try_send`)
    /// AND mirror it to the host's own pair panel.
    ///
    /// Called from the PTY tap (`pty_polling.rs`) when `pane_id ==
    /// pair_host_pane_id`. Two sinks:
    ///   1. The room worker (for navigators). Drops on a full channel — the room
    ///      stream tolerates loss and a lagging navigator must not stall the
    ///      60Hz PTY hot path.
    ///   2. The host's own pair panel, as a `pair_event{pty_output}`. The relay
    ///      does all-but-sender fan-out, so the host never gets its own frame
    ///      echoed back — without this local mirror the host's pair panel would
    ///      stay blank while it shares its terminal.
    pub(in crate::app_state) fn pair_enqueue_output(&self, data: Vec<u8>) {
        // Sink 2: mirror to the host's own panel so the host sees the shared
        // terminal it is broadcasting.
        self.send_pair_event(&serde_json::json!({
            "event": "pty_output",
            "data": b64_encode(&data),
        }));

        // Sink 1: fan out to the room for navigators.
        if let Some(ref tx) = self.pair_cmd_tx {
            match tx.try_send(PairCommand::Output(data)) {
                Ok(()) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    tracing::trace!("Pair output channel full, dropping PTY chunk");
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    tracing::debug!("Pair output channel closed");
                }
            }
        }
    }

    /// Forward a `pair_event` IPC payload to the pair panel (the host/joiner pane).
    fn send_pair_event(&self, payload: &serde_json::Value) {
        let Some(pane_id) = self.pair_host_pane_id else {
            return;
        };
        if let Some(ref registry) = self.webviews {
            if let Some(handle) = registry.get(pane_id) {
                if let Err(e) = handle.send_ipc("pair_event", payload) {
                    tracing::warn!(pane_id, error = %e, "Failed to send pair_event IPC");
                }
            }
        }
    }
}

/// Convert a `PairManager` event into the leaner `PairFrame` wire form. Returns
/// `None` for events that carry no on-wire payload (join/leave/end are surfaced
/// via the relay's own member presence frames).
fn pair_event_to_frame(_session_id: &str, ev: PairEvent) -> Option<PairFrame> {
    match ev {
        PairEvent::TerminalOutput { data, .. } => Some(PairFrame::PtyOutput { data }),
        // term_input is navigator→host ONLY. The host writes accepted driver
        // input to its own PTY, and the resulting PTY OUTPUT is what fans out to
        // navigators. Re-broadcasting the input here would echo every keystroke
        // back into the room (and the navigators) — drop it.
        PairEvent::TerminalInput { .. } => None,
        PairEvent::CursorMoved {
            user_id, row, col, ..
        } => Some(PairFrame::Cursor {
            from: user_id,
            row,
            col,
        }),
        PairEvent::Resized { cols, rows, .. } => Some(PairFrame::Resize { cols, rows }),
        PairEvent::DriverChanged {
            new_driver,
            old_driver,
            ..
        } => Some(PairFrame::DriverChanged {
            new_driver,
            old_driver,
        }),
        // Lifecycle events ride the relay's member presence channel, not PairFrame.
        PairEvent::SessionCreated { .. }
        | PairEvent::SessionEnded { .. }
        | PairEvent::UserJoined { .. }
        | PairEvent::UserLeft { .. }
        | PairEvent::Error(_) => None,
    }
}

/// Base64-encode raw bytes for delivery to the panel (`pty_output`/`snapshot`).
/// PTY output is arbitrary bytes; base64 keeps the stream lossless across the
/// JSON IPC boundary (the panel base64-decodes before `term.write`).
fn b64_encode(data: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    B64.encode(data)
}

/// Generate a 32-char high-entropy session id (the room's capability secret).
fn generate_session_id() -> String {
    random_alnum(32)
}

/// Generate a 16-char stable member id for this app within the room.
fn generate_member_id() -> String {
    random_alnum(16)
}

fn random_alnum(len: usize) -> String {
    (0..len)
        .map(|_| {
            let idx = rand::random::<u8>() % 36;
            if idx < 10 {
                (b'0' + idx) as char
            } else {
                (b'a' + idx - 10) as char
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_social::PairRole;

    /// term_input must NOT be re-broadcast by the host: the host writes accepted
    /// driver input to its PTY and the resulting PTY OUTPUT is what reaches
    /// navigators. Echoing the input would double the keystroke on every screen.
    #[test]
    fn terminal_input_event_is_not_rebroadcast() {
        let ev = PairEvent::TerminalInput {
            session_id: "sid".into(),
            from_user: "m2".into(),
            data: b"ls\n".to_vec(),
        };
        assert!(
            pair_event_to_frame("sid", ev).is_none(),
            "term_input must never be mapped to an outbound frame (no echo loop)"
        );
    }

    /// Terminal OUTPUT, by contrast, IS fanned out (the host→navigator path).
    #[test]
    fn terminal_output_event_maps_to_pty_output() {
        let ev = PairEvent::TerminalOutput {
            session_id: "sid".into(),
            data: vec![0x1b, b'[', b'2', b'J'],
        };
        match pair_event_to_frame("sid", ev) {
            Some(PairFrame::PtyOutput { data }) => assert_eq!(data, vec![0x1b, b'[', b'2', b'J']),
            other => panic!("expected pty_output, got {other:?}"),
        }
    }

    /// A driver change still rides a frame (host-authoritative state).
    #[test]
    fn driver_changed_event_maps_to_frame() {
        let ev = PairEvent::DriverChanged {
            session_id: "sid".into(),
            new_driver: "m2".into(),
            old_driver: "m1".into(),
        };
        assert!(matches!(
            pair_event_to_frame("sid", ev),
            Some(PairFrame::DriverChanged { .. })
        ));
    }

    /// Lifecycle events ride the relay presence channel, not a PairFrame.
    #[test]
    fn lifecycle_events_carry_no_frame() {
        let _ = PairRole::Navigator; // keep the import meaningful
        assert!(pair_event_to_frame(
            "sid",
            PairEvent::UserJoined {
                session_id: "sid".into(),
                user_id: "m2".into(),
                display_name: "Nav".into(),
                role: PairRole::Navigator,
            }
        )
        .is_none());
        assert!(pair_event_to_frame("sid", PairEvent::SessionEnded { session_id: "sid".into() }).is_none());
    }
}
