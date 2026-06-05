//! Pair-programming IPC handlers and panel forwarding (M1/M2).
//!
//! Models `presence_handlers` + `chat_stream_handlers`. Panel↔Rust is pure IPC:
//! up = `pair_start`/`pair_join`/`pair_leave`/`pair_input`/`pair_request_control`/
//! `pair_set_driver`/`pair_cursor`/`pair_status`; down = `pair_event`/`pair_status`.
//!
//! The transport (relay Room), the room worker, and the host-authoritative
//! inbound routing live in `app_state::pair` and `ws_server`. These handlers are
//! the panel-facing surface: they seed the session identity, drive the
//! `PairManager` lifecycle (`create_session`/`join_session`/`set_driver`/
//! `relay_input`/`update_cursor`), emit `PairFrame`s over the room worker, and
//! reflect status back to the panel.
//!
//! Authority model (matches the spec): the host is the trust anchor. Only the
//! host has a live `PairManager` session (it calls `create_session`); navigators
//! never call `create_session`, so every `relay_input`/`set_driver` they might
//! attempt fails closed. Navigators emit `term_input`/`cursor`/`request_control`
//! frames; the host re-validates each one in `app_state::pair::apply_pair_frame`.
//!
//! M3 (ENFORCED): every inbound frame is end-to-end ECDSA-signed and verified in
//! `verify_signed_frame` BEFORE `apply_pair_frame` honors it. Under
//! `require_signed_join` (the default) unsigned/unverifiable frames are dropped
//! fail-closed, the host registers a navigator (`join_session`) only after its
//! VERIFIED signed `Join`, host-only frames are accepted only from the pinned
//! host, and `term_input` only from the verified current driver.

#![allow(unused_variables)]

use jarvis_webview::IpcPayload;

use super::presence_handlers::sanitize_display_name;
use crate::app_state::core::JarvisApp;
use crate::app_state::pair::PairCommand;
use crate::app_state::ws_server::pair_protocol::{PairFrame, MAX_TERM_INPUT_BYTES};

impl JarvisApp {
    /// Handle `pair_start` — host creates a new pair session on this pane.
    ///
    /// Seeds `pair_host_pane_id`/`pair_session_id`/`pair_member_id`, spins up the
    /// room worker via `start_pair`, then registers the session in the local
    /// `PairManager` (`create_session`) so the host can re-validate driver input.
    /// Replies with a `pair_status` snapshot (`role: "host"`).
    pub(in crate::app_state) fn handle_pair_start(&mut self, pane_id: u32, payload: &IpcPayload) {
        if !self.config.collab.enabled {
            self.send_pair_event_to_pane(
                pane_id,
                &serde_json::json!({
                    "event": "error",
                    "message": "Pair programming is disabled (collab.enabled = false)",
                }),
            );
            return;
        }
        if self.config.relay.url.is_empty() {
            self.send_pair_event_to_pane(
                pane_id,
                &serde_json::json!({
                    "event": "error",
                    "message": "No relay configured",
                }),
            );
            return;
        }

        let (cols, rows) = extract_dims(payload).unwrap_or((80, 24));
        let display_name = self.resolve_display_name(payload);
        self.pair_display_name = Some(display_name.clone());

        // The pair panel pane is the host's terminal pane and the panel target.
        self.pair_host_pane_id = Some(pane_id);

        // The pair panel renders its own xterm but has no shell behind it. Spawn
        // a PTY for the panel pane so the host has a real terminal to share: its
        // output is tapped in `pty_polling.rs` (fanned to the room + mirrored to
        // the host's own panel), and host keystrokes arrive as `pty_input`.
        if !self.ptys.contains(pane_id) {
            let cwd = self.config.shell.working_directory.as_deref();
            match crate::app_state::pty_bridge::spawn_pty(cols, rows, cwd) {
                Ok(handle) => {
                    self.ptys.insert(pane_id, handle);
                    tracing::info!(pane_id, cols, rows, "Spawned PTY for pair host panel");
                }
                Err(e) => {
                    tracing::error!(pane_id, error = %e, "pair_start: PTY spawn failed");
                    self.send_pair_event_to_pane(
                        pane_id,
                        &serde_json::json!({
                            "event": "error",
                            "message": "Failed to start shared shell",
                        }),
                    );
                    return;
                }
            }
        }

        // Fresh session identity for a brand-new room (the capability secret).
        // start_pair generates these if unset; we clear any stale ids first so a
        // new "Share my terminal" always mints a new room.
        self.teardown_pair_worker();
        self.pair_session_id = None;
        self.pair_member_id = None;

        // Bring up the room worker (constructs the PairManager + connects).
        self.start_pair();

        let (Some(session_id), Some(member_id)) = (
            self.pair_session_id.clone(),
            self.pair_member_id.clone(),
        ) else {
            self.send_pair_event_to_pane(
                pane_id,
                &serde_json::json!({
                    "event": "error",
                    "message": "Failed to start pair session",
                }),
            );
            return;
        };

        // SECURITY (M3 — THE host-authority binding): the host created this
        // session and KNOWS its own identity, so pin the host `(member_id,
        // pubkey)` immediately — before any frame is processed. The shareable
        // invite carries the host pubkey, and a navigator pins from it, so only
        // this identity can ever drive host-only frames (closes the SessionMeta
        // host-claim race). With no crypto the pubkey is empty and the host stays
        // unpinned (host-only frames then fail closed under require_signed_join).
        let host_pubkey = self
            .crypto
            .as_ref()
            .map(|c| c.pubkey_base64.clone())
            .unwrap_or_default();
        self.pair_pin_host(Some(&member_id), &host_pubkey);

        // The shareable invite carries BOTH the session id and the host pubkey
        // (base64url) so the navigator can pin the host before processing frames.
        let invite = crate::app_state::pair::make_invite(&session_id, &host_pubkey);

        // Register the session in the host's PairManager. The host is the initial
        // driver; member_id is the in-room user id (matches relay member id).
        if let (Some(manager), Some(rt)) =
            (self.pair_manager.clone(), self.tokio_runtime.as_ref())
        {
            if let Err(e) = rt.block_on(manager.create_session(
                &session_id,
                &member_id,
                &display_name,
                cols,
                rows,
            )) {
                tracing::warn!(error = %e, "pair_start: create_session failed");
                self.send_pair_event_to_pane(
                    pane_id,
                    &serde_json::json!({ "event": "error", "message": e }),
                );
                return;
            }
        }

        // Announce session metadata + roster to the room so joiners learn host
        // name + dimensions + takeover policy (re-broadcast on each join too).
        self.rebroadcast_session_meta(&session_id);

        tracing::info!(pane_id, member = %member_id, "Pair session hosted");

        // Confirm to the host panel: hides the setup overlay, sets role/badge.
        // `invite` is the shareable code (session_id + host pubkey); the panel
        // shows/copies it. `session_id` is still sent for status correlation.
        self.send_pair_event_to_pane(
            pane_id,
            &serde_json::json!({
                "event": "session_started",
                "session_id": session_id,
                "invite": invite,
                "role": "host",
            }),
        );
        self.send_pair_status_to_pane(pane_id);
        self.needs_redraw = true;
    }

    /// Handle `pair_join` — join an existing session by id / invite code.
    ///
    /// Seeds `pair_session_id` from the payload (so `start_pair` reuses it),
    /// joins as a view-only navigator, and connects the room worker. The joiner
    /// does NOT call `create_session`, so it never becomes a local authority.
    pub(in crate::app_state) fn handle_pair_join(&mut self, pane_id: u32, payload: &IpcPayload) {
        if !self.config.collab.enabled {
            self.send_pair_event_to_pane(
                pane_id,
                &serde_json::json!({
                    "event": "error",
                    "message": "Pair programming is disabled (collab.enabled = false)",
                }),
            );
            return;
        }
        if self.config.relay.url.is_empty() {
            self.send_pair_event_to_pane(
                pane_id,
                &serde_json::json!({ "event": "error", "message": "No relay configured" }),
            );
            return;
        }

        // The panel sends the INVITE (base64url of session_id + host pubkey) as
        // `invite_code`/`session_id`. Parse out BOTH the session id and the host
        // pubkey so we can pin the host identity from the invite (the critical
        // host-authority binding) before any frame is processed.
        let raw_invite = match extract_string(payload, "invite_code")
            .or_else(|| extract_string(payload, "session_id"))
        {
            Some(s) if !s.is_empty() && s.len() <= 512 => s,
            _ => {
                self.send_pair_event_to_pane(
                    pane_id,
                    &serde_json::json!({ "event": "error", "message": "Enter an invite code." }),
                );
                return;
            }
        };
        let (session_id, host_pubkey) =
            match crate::app_state::pair::parse_invite(&raw_invite) {
                Some((sid, pk)) if !sid.is_empty() && sid.len() <= 128 => (sid, pk),
                _ => {
                    self.send_pair_event_to_pane(
                        pane_id,
                        &serde_json::json!({ "event": "error", "message": "Invalid invite code." }),
                    );
                    return;
                }
            };

        let display_name = self.resolve_display_name(payload);
        self.pair_display_name = Some(display_name.clone());

        // This pane is the panel target. (For a navigator there is no host PTY,
        // but `pair_host_pane_id` doubles as the panel pane; the host-only write
        // paths fail closed because the joiner's PairManager has no session.)
        self.pair_host_pane_id = Some(pane_id);

        // Seed the session id BEFORE start_pair so the worker joins the right
        // room and derives the matching room key. Fresh member id each join.
        self.teardown_pair_worker();
        self.pair_session_id = Some(session_id.clone());
        self.pair_member_id = None;

        self.start_pair();

        // SECURITY (M3 — THE host-authority binding, navigator side): pin the
        // host pubkey FROM THE INVITE now (after start_pair, which reset the auth
        // roster) — before any frame is processed. The host member_id is learned
        // from the first host-only frame that verifies against this pubkey.
        // Without a host pubkey in the invite (bare/manual code) the navigator
        // stays unpinned and host-only frames fail closed (the safe default).
        if !host_pubkey.is_empty() {
            self.pair_pin_host(None, &host_pubkey);
        }

        let member_id = self.pair_member_id.clone().unwrap_or_default();

        // The session_id is the room's CAPABILITY SECRET — never log it at info.
        // Log only a truncated fingerprint at debug for correlation.
        tracing::info!(pane_id, "Pair session join requested");
        tracing::debug!(pane_id, sid_fp = %session_id_fingerprint(&session_id), "Pair join");

        // Confirm to the panel: join confirmed, view-only until driver_changed.
        self.send_pair_event_to_pane(
            pane_id,
            &serde_json::json!({
                "event": "session_started",
                "session_id": session_id,
                "role": "view",
            }),
        );
        self.send_pair_status_to_pane(pane_id);
        self.needs_redraw = true;
    }

    /// Handle `pair_leave` — leave / end the current pair session.
    pub(in crate::app_state) fn handle_pair_leave(&mut self, pane_id: u32, payload: &IpcPayload) {
        // Tell the host's PairManager we left (ends the session if we are host).
        if let (Some(manager), Some(rt), Some(member_id)) = (
            self.pair_manager.clone(),
            self.tokio_runtime.as_ref(),
            self.pair_member_id.clone(),
        ) {
            rt.block_on(manager.leave_session(&member_id));
        }

        self.teardown_pair_worker();
        self.pair_session_id = None;
        self.pair_member_id = None;
        self.pair_display_name = None;
        let panel_pane = self.pair_host_pane_id.take();

        // Kill the shared shell we spawned for the host panel (no-op for a
        // navigator, whose panel pane has no PTY).
        if let Some(p) = panel_pane {
            if self.ptys.contains(p) {
                self.ptys.kill_and_remove(p);
            }
            self.send_pair_event_to_pane(
                p,
                &serde_json::json!({ "event": "session_ended" }),
            );
        }
        self.needs_redraw = true;
    }

    /// Handle `pair_input` — driver/navigator keystrokes.
    ///
    /// The local panel only sends this when its role is `driver`. We emit a
    /// `term_input` `PairFrame` to the room; the host re-validates it via
    /// `relay_input` (driver-gated) before writing to its PTY. The raw xterm
    /// string is converted to bytes here (the panel sends a raw string, not
    /// base64 — base64 happens inside `PairFrame::TermInput` on the wire).
    pub(in crate::app_state) fn handle_pair_input(&mut self, pane_id: u32, payload: &IpcPayload) {
        let data = match extract_string(payload, "data") {
            Some(s) => s,
            None => return,
        };
        let Some(from) = self.pair_member_id.clone() else {
            return;
        };
        let mut bytes = data.into_bytes();
        // Cheap hardening: a single keystroke frame should be tiny. Drop
        // anything oversize rather than fan a multi-KB paste-bomb to the host.
        if bytes.len() > MAX_TERM_INPUT_BYTES {
            tracing::debug!(len = bytes.len(), "Dropping oversize pair term_input");
            bytes.truncate(MAX_TERM_INPUT_BYTES);
        }
        self.send_pair_frame(PairFrame::TermInput { from, data: bytes });
    }

    /// Handle `pair_request_control` — navigator asks for the driver seat.
    ///
    /// Emits a `request_control` frame to the room. The host arbitrates in
    /// `apply_pair_frame` (honoring `allow_takeover`) and broadcasts
    /// `driver_changed`. If we are ourselves the host, there is nothing to ask.
    pub(in crate::app_state) fn handle_pair_request_control(
        &mut self,
        pane_id: u32,
        payload: &IpcPayload,
    ) {
        let Some(from) = self.pair_member_id.clone() else {
            return;
        };
        self.send_pair_frame(PairFrame::RequestControl { from });
    }

    /// Handle `pair_set_driver` — host grants/changes the driver seat.
    ///
    /// Only meaningful on the host (which owns the live `PairManager` session).
    /// `set_driver` enforces `allow_takeover` and emits a `DriverChanged`
    /// `PairEvent`, which the bridge fans out to the room as a
    /// `driver_changed` frame.
    pub(in crate::app_state) fn handle_pair_set_driver(
        &mut self,
        pane_id: u32,
        payload: &IpcPayload,
    ) {
        let new_driver = match extract_string(payload, "user_id") {
            Some(s) if !s.is_empty() && s.len() <= 64 => s,
            _ => return,
        };
        let Some(session_id) = self.pair_session_id.clone() else {
            return;
        };
        let Some(requester) = self.pair_member_id.clone() else {
            return;
        };
        if let (Some(manager), Some(rt)) =
            (self.pair_manager.clone(), self.tokio_runtime.as_ref())
        {
            match rt.block_on(manager.set_driver(&session_id, &requester, &new_driver)) {
                Ok(()) => tracing::info!(%new_driver, "Pair driver granted"),
                Err(e) => tracing::debug!(error = %e, "pair_set_driver rejected"),
            }
        }
    }

    /// Handle `pair_cursor` — local cursor moved; broadcast as a ghost cursor.
    pub(in crate::app_state) fn handle_pair_cursor(&mut self, pane_id: u32, payload: &IpcPayload) {
        let (row, col) = match extract_row_col(payload) {
            Some(rc) => rc,
            None => return,
        };
        let Some(from) = self.pair_member_id.clone() else {
            return;
        };
        self.send_pair_frame(PairFrame::Cursor { from, row, col });
    }

    /// Handle `pair_status` — panel requests the current session status snapshot.
    pub(in crate::app_state) fn handle_pair_status(&mut self, pane_id: u32, payload: &IpcPayload) {
        self.send_pair_status_to_pane(pane_id);
    }

    /// End the pair session when the host's shared PTY exits.
    ///
    /// Called from the PTY-exit path (`pty_polling.rs`) when the finished pane is
    /// the pair host pane. Drives `leave_session` (host leaving ends the session
    /// in the `PairManager`), tears the worker down, and notifies the panel.
    pub(in crate::app_state) fn end_pair_on_host_exit(&mut self) {
        if let (Some(manager), Some(rt), Some(member_id)) = (
            self.pair_manager.clone(),
            self.tokio_runtime.as_ref(),
            self.pair_member_id.clone(),
        ) {
            rt.block_on(manager.leave_session(&member_id));
        }
        let panel_pane = self.pair_host_pane_id.take();
        self.teardown_pair_worker();
        self.pair_session_id = None;
        self.pair_member_id = None;
        self.pair_display_name = None;
        if let Some(p) = panel_pane {
            self.send_pair_event_to_pane(
                p,
                &serde_json::json!({ "event": "session_ended", "reason": "host terminal closed" }),
            );
        }
        self.needs_redraw = true;
    }
}

// =============================================================================
// PANEL FORWARDING
// =============================================================================

impl JarvisApp {
    /// Forward an inbound `pair_event` payload to a specific pane.
    ///
    /// This is the explicit panel-forwarding entry point named in the contract
    /// (`send_pair_event_to_panel`). The poll loop's host-authoritative routing
    /// uses the private `send_pair_event` in `app_state::pair`; handlers use this
    /// pane-explicit form for immediate replies (start/join/leave confirmations).
    #[allow(dead_code)]
    pub(in crate::app_state) fn send_pair_event_to_panel(&self, payload: &serde_json::Value) {
        if let Some(pane_id) = self.pair_host_pane_id {
            self.send_pair_event_to_pane(pane_id, payload);
        }
    }

    /// Send a `pair_event` IPC message to a specific pane.
    fn send_pair_event_to_pane(&self, pane_id: u32, payload: &serde_json::Value) {
        if let Some(ref registry) = self.webviews {
            if let Some(handle) = registry.get(pane_id) {
                if let Err(e) = handle.send_ipc("pair_event", payload) {
                    tracing::warn!(pane_id, error = %e, "Failed to send pair_event IPC");
                }
            }
        }
    }

    /// Build and send a full `pair_status` snapshot to a pane.
    ///
    /// On the host (live `PairManager` session) this reflects the authoritative
    /// session state (participants, driver, dimensions). On a navigator (no local
    /// session yet) it sends the identity + connection-pending shell, which the
    /// panel merges; subsequent `pair_event`s (`session_meta`, `member_*`,
    /// `driver_changed`) fill in the rest.
    pub(in crate::app_state) fn send_pair_status_to_pane(&self, pane_id: u32) {
        let member_id = self.pair_member_id.clone().unwrap_or_default();
        let session_id = self.pair_session_id.clone().unwrap_or_default();

        // Try to read the authoritative session from the local PairManager.
        let session = match (self.pair_manager.clone(), self.tokio_runtime.as_ref()) {
            (Some(manager), Some(rt)) if !session_id.is_empty() => {
                rt.block_on(manager.get_session(&session_id))
            }
            _ => None,
        };

        let connected = self.pair_cmd_tx.is_some();

        let payload = if let Some(session) = session {
            // Host view: full authoritative snapshot.
            let role = if session.driver_user_id == member_id {
                if session.host_user_id == member_id {
                    "host"
                } else {
                    "driver"
                }
            } else {
                "view"
            };
            // The shareable invite (session id + host pubkey). Only meaningful
            // for the actual host (who is the host_user_id); for a navigator on
            // this authoritative branch it would be wrong, but a navigator never
            // owns a live PairManager session, so this branch is the host's.
            let invite = if session.host_user_id == member_id {
                let host_pubkey = self
                    .crypto
                    .as_ref()
                    .map(|c| c.pubkey_base64.clone())
                    .unwrap_or_default();
                crate::app_state::pair::make_invite(&session.session_id, &host_pubkey)
            } else {
                session.session_id.clone()
            };
            let participants: Vec<serde_json::Value> = session
                .participants
                .values()
                .map(|p| {
                    serde_json::json!({
                        "user_id": p.user_id,
                        "display_name": sanitize_display_name(&p.display_name),
                        "role": pair_role_str(p.role),
                    })
                })
                .collect();
            serde_json::json!({
                "session_id": session.session_id,
                "invite": invite,
                "member_id": member_id,
                "role": role,
                "host_user_id": session.host_user_id,
                "driver_user_id": session.driver_user_id,
                "allow_takeover": session.allow_takeover,
                "connected": connected,
                "cols": session.cols,
                "rows": session.rows,
                "participants": participants,
            })
        } else if session_id.is_empty() {
            // No session at all — empty shell so the panel shows the setup UI.
            serde_json::json!({
                "session_id": "",
                "member_id": member_id,
                "connected": false,
            })
        } else {
            // Navigator: identity + connection state. Role stays view-only until
            // a `driver_changed` frame promotes us. participants/host are filled
            // in incrementally by `session_meta` / `member_joined` events.
            serde_json::json!({
                "session_id": session_id,
                "member_id": member_id,
                "role": "view",
                "connected": connected,
            })
        };

        if let Some(ref registry) = self.webviews {
            if let Some(handle) = registry.get(pane_id) {
                if let Err(e) = handle.send_ipc("pair_status", &payload) {
                    tracing::warn!(pane_id, error = %e, "Failed to send pair_status IPC");
                }
            }
        }
    }

    /// True iff this app is the pair *host* with authority: it owns a live
    /// `PairManager` session keyed by the current `pair_session_id`. (We can't
    /// just check `pair_host_pane_id`, which a navigator also sets as its panel
    /// pane — only the host calls `create_session`.) M3 cleanup: split the
    /// panel-pane role from host-authority into a dedicated flag.
    pub(in crate::app_state) fn is_pair_host(&self) -> bool {
        let Some(session_id) = self.pair_session_id.clone() else {
            return false;
        };
        match (self.pair_manager.clone(), self.tokio_runtime.as_ref()) {
            (Some(manager), Some(rt)) => {
                rt.block_on(manager.get_session(&session_id)).is_some()
            }
            _ => false,
        }
    }

    /// Host-only: register (or refresh) a navigator in the `PairManager` roster
    /// and re-broadcast `SessionMeta` so every member sees the updated roster.
    ///
    /// `join_session` is idempotent enough for our use: it first drops any prior
    /// membership for `member_id`, then inserts as a Navigator. Re-calling it
    /// when the `Join` frame upgrades a placeholder name simply refreshes the
    /// display name. The host's own member_id is never registered this way
    /// (it is already in the session as host/driver).
    pub(in crate::app_state) fn host_register_member(
        &self,
        session_id: &str,
        member_id: &str,
        display_name: &str,
    ) {
        let host_member = self.pair_member_id.clone().unwrap_or_default();
        if member_id == host_member {
            return; // never re-register the host as a navigator
        }
        let name = sanitize_display_name(display_name);
        if let (Some(manager), Some(rt)) =
            (self.pair_manager.clone(), self.tokio_runtime.as_ref())
        {
            if let Err(e) =
                rt.block_on(manager.join_session(session_id, member_id, &name))
            {
                tracing::debug!(error = %e, "pair host_register_member: join_session failed");
            }
        }
        self.rebroadcast_session_meta(session_id);
    }

    /// Host-only: re-emit `SessionMeta` (host name, dims, takeover policy, and
    /// the full roster of real display names) to the room AND the local panel,
    /// so late joiners and reconnecting members re-sync (the relay does no
    /// replay).
    pub(in crate::app_state) fn rebroadcast_session_meta(&self, session_id: &str) {
        let session = match (self.pair_manager.clone(), self.tokio_runtime.as_ref()) {
            (Some(manager), Some(rt)) => rt.block_on(manager.get_session(session_id)),
            _ => None,
        };
        let Some(session) = session else {
            return; // not the host / no live session
        };

        let roster: Vec<crate::app_state::ws_server::pair_protocol::RosterEntry> = session
            .participants
            .values()
            .map(|p| {
                let role = if p.user_id == session.host_user_id {
                    "host"
                } else if p.user_id == session.driver_user_id {
                    "driver"
                } else {
                    "view"
                };
                crate::app_state::ws_server::pair_protocol::RosterEntry {
                    member_id: p.user_id.clone(),
                    name: sanitize_display_name(&p.display_name),
                    role: role.to_string(),
                }
            })
            .collect();

        let frame = PairFrame::SessionMeta {
            host: session.host_user_id.clone(),
            host_name: sanitize_display_name(&session.host_display_name),
            cols: session.cols,
            rows: session.rows,
            allow_takeover: session.allow_takeover,
            roster,
        };
        // To the room (navigators).
        self.send_pair_frame(frame);

        // To our own panel (the host renders its own roster too). We emit the
        // panel event directly rather than re-entering apply_pair_frame (which
        // takes &mut self).
        let roster_json: Vec<serde_json::Value> = session
            .participants
            .values()
            .map(|p| {
                let role = if p.user_id == session.host_user_id {
                    "host"
                } else if p.user_id == session.driver_user_id {
                    "driver"
                } else {
                    "view"
                };
                serde_json::json!({
                    "member_id": p.user_id,
                    "name": sanitize_display_name(&p.display_name),
                    "role": role,
                })
            })
            .collect();
        if let Some(pane_id) = self.pair_host_pane_id {
            self.send_pair_event_to_pane(
                pane_id,
                &serde_json::json!({
                    "event": "session_meta",
                    "host": session.host_user_id,
                    "host_name": sanitize_display_name(&session.host_display_name),
                    "cols": session.cols,
                    "rows": session.rows,
                    "allow_takeover": session.allow_takeover,
                    "roster": roster_json,
                }),
            );
        }
    }

    /// Send a `PairFrame` to the room via the worker (non-blocking `try_send`).
    pub(in crate::app_state) fn send_pair_frame(&self, frame: PairFrame) {
        if let Some(ref tx) = self.pair_cmd_tx {
            if let Err(e) = tx.try_send(PairCommand::Send(frame)) {
                tracing::debug!(error = %e, "Failed to enqueue pair frame");
            }
        }
    }

    /// Tear down the running room worker (drop the command/inbound channels and
    /// key sender), leaving session/member ids for the caller to manage. The
    /// worker tasks observe their channels closing and exit.
    fn teardown_pair_worker(&mut self) {
        self.pair_cmd_tx = None;
        self.pair_inbound_rx = None;
        self.pair_manager = None;
        self.pair_key_tx = None;
    }

    /// Resolve the display name for this app: prefer a sanitized panel-supplied
    /// name, else fall back to the OS user name (same scheme as presence).
    fn resolve_display_name(&self, payload: &IpcPayload) -> String {
        if let Some(name) = extract_string(payload, "display_name") {
            let sanitized = sanitize_display_name(&name);
            if sanitized != "Unknown" {
                return sanitized;
            }
        }
        let host = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "jarvis-user".to_string());
        sanitize_display_name(&host)
    }
}

// =============================================================================
// PAYLOAD HELPERS
// =============================================================================

/// A non-reversible fingerprint of a capability-secret session id, safe to log
/// at debug for correlation: the first 6 chars plus the total length. Never log
/// the full session id (it is the room's bearer secret).
fn session_id_fingerprint(session_id: &str) -> String {
    let prefix: String = session_id.chars().take(6).collect();
    format!("{prefix}…(len={})", session_id.len())
}

fn extract_string(payload: &IpcPayload, field: &str) -> Option<String> {
    match payload {
        IpcPayload::Json(obj) => obj.get(field)?.as_str().map(|s| s.to_string()),
        _ => None,
    }
}

fn extract_dims(payload: &IpcPayload) -> Option<(u16, u16)> {
    match payload {
        IpcPayload::Json(obj) => {
            let cols = obj.get("cols")?.as_u64()? as u16;
            let rows = obj.get("rows")?.as_u64()? as u16;
            if cols == 0 || rows == 0 || cols > 500 || rows > 500 {
                return None;
            }
            Some((cols, rows))
        }
        _ => None,
    }
}

fn extract_row_col(payload: &IpcPayload) -> Option<(u16, u16)> {
    match payload {
        IpcPayload::Json(obj) => {
            let row = obj.get("row")?.as_u64()?.min(u16::MAX as u64) as u16;
            let col = obj.get("col")?.as_u64()?.min(u16::MAX as u64) as u16;
            Some((row, col))
        }
        _ => None,
    }
}

/// Map the internal `PairRole` to the panel's `"host"|"driver"|"view"` strings.
/// Note: host-vs-driver is resolved by the caller (a host is also the driver in
/// the role enum); here `Driver` maps to `"driver"`, `Navigator` to `"view"`.
fn pair_role_str(role: jarvis_social::PairRole) -> &'static str {
    match role {
        jarvis_social::PairRole::Driver => "driver",
        jarvis_social::PairRole::Navigator => "view",
    }
}
