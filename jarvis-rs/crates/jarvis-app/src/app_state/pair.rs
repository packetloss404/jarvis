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

use std::collections::HashMap;
use std::sync::Arc;

use jarvis_social::{PairConfig, PairEvent, PairManager};

use super::core::JarvisApp;
use crate::app_state::ws_server::pair_protocol::{
    FrameRejectReason, FrameVerifyResult, PairFrame, SignedPairFrame, MAX_TERM_INPUT_BYTES,
};
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
    /// An inbound application frame to apply locally / forward to the panel
    /// (LEGACY unsigned path).
    Frame(PairFrame),
    /// M3: an inbound SIGNED frame to verify (`verify_signed_frame`) before
    /// `apply_pair_frame` honors the inner frame.
    SignedFrame(SignedPairFrame),
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

/// M3 authentication state for the active pair session: the member→identity
/// roster (TOFU pubkey pinning), per-sender anti-replay counters, and the
/// pinned host. Lives on the main thread alongside `CryptoService` (the only
/// owner of the identity key), and is consulted by `verify_signed_frame` before
/// `apply_pair_frame` honors any inbound frame.
///
/// M3 (IMPLEMENTED): `verify_signed_frame` performs full inbound
/// authentication — TOFU pubkey binding, ECDSA signature check (fail-closed
/// under `require_signed_join`), per-sender anti-replay, self-`from` binding,
/// and host-authority pinning. Reset on each `start_pair`.
#[derive(Default)]
pub(in crate::app_state) struct PairAuthState {
    /// member_id → pinned ECDSA identity pubkey (SPKI base64). First verified
    /// frame / signed `Join` from a member pins it (TOFU); later frames from the
    /// same member_id MUST match or are dropped (`PubkeyMismatch`). `None` means
    /// the member_id is known but UNPINNED — never store an empty pubkey as an
    /// identity (an empty-pubkey legacy member must not be claimable by anyone).
    pub(in crate::app_state) member_pubkeys: HashMap<String, Option<String>>,
    /// member_id → last-accepted `(epoch, seq)`. A frame is accepted iff its
    /// `epoch` is strictly greater (→ `seq` tracking resets) OR its `epoch`
    /// matches and its `seq` strictly exceeds the stored `seq`. A lower epoch or
    /// a non-increasing seq within the epoch is a replay (`StaleSeq`).
    pub(in crate::app_state) last_seq: HashMap<String, (u64, u64)>,
    /// The pinned host identity `(member_id, pubkey)`. On the HOST this is pinned
    /// to its own identity at `pair_start`; on a NAVIGATOR it is pinned from the
    /// INVITE (which carries the host pubkey) before any frame is processed.
    /// Host-only frames are accepted ONLY from this pinned host; while it is
    /// `None` ALL host-only frames are refused (fail closed).
    pub(in crate::app_state) pinned_host: Option<PinnedHost>,
}

/// The pinned host capability. The `pubkey` is the AUTHORITY ANCHOR: it is bound
/// to the invite (the navigator parses the host pubkey out of the invite and
/// pins it before processing frames; the host pins its own), so only the real
/// host — the one whose pubkey is in the invite the navigator used — can drive
/// host-only frames. This closes the SessionMeta race (a forged SessionMeta from
/// a non-invite pubkey can no longer claim the host slot).
///
/// `member_id` is the host's in-room id. The HOST knows its own up front; the
/// NAVIGATOR learns it from the first host-only frame that verifies against the
/// pinned pubkey (`None` until then) and thereafter requires it to stay
/// consistent. Authority is gated on the PUBKEY, not the relay-assigned slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app_state) struct PinnedHost {
    pub(in crate::app_state) member_id: Option<String>,
    pub(in crate::app_state) pubkey: String,
}

impl PairAuthState {
    /// Pure M3 inbound decision: authenticate a `SignedPairFrame` against this
    /// roster and return whether to apply or drop it. Mutates the roster
    /// (TOFU pin, seq, host pin) only on the success path. `verify` is the
    /// signature predicate over `base64(canonical_bytes)` (injected so the
    /// logic is testable without the main-thread `CryptoService`).
    ///
    /// On `Accept`, the VERIFIED sender is `signed.member_id`, and every
    /// navigator frame's inner `from` has been bound to equal it.
    ///
    /// Steps (see `JarvisApp::verify_signed_frame` for the prose): (1) TOFU
    /// pubkey binding, (2) signature / fail-closed under `require_signed`,
    /// (3) anti-replay seq, (4) self-`from` binding, (5) host authority.
    pub(in crate::app_state) fn verify_signed_frame(
        &mut self,
        session_id: &str,
        require_signed: bool,
        signed: &SignedPairFrame,
        verify: impl Fn(&str, &str, &str) -> bool,
    ) -> FrameVerifyResult {
        // --- (1) identity binding (TOFU pin / match) ---
        // A pinned `Some(pk)` that differs from the carried pubkey is an
        // impersonation attempt. A pinned `None` (unpinned legacy member) is
        // never matched against any pubkey — it stays unclaimable.
        if let Some(Some(pinned)) = self.member_pubkeys.get(&signed.member_id) {
            if *pinned != signed.pubkey {
                tracing::warn!(member = %signed.member_id, "pair: pubkey mismatch (impersonation?)");
                return FrameVerifyResult::Reject(FrameRejectReason::PubkeyMismatch);
            }
        }

        // --- (2) signature check ---
        let signed_ok = !signed.sig.is_empty() && !signed.pubkey.is_empty();
        if signed_ok {
            let bytes = match signed.signing_bytes(session_id) {
                Ok(b) => b,
                Err(_) => return FrameVerifyResult::Reject(FrameRejectReason::BadSignature),
            };
            let msg = base64_of(&bytes);
            if !verify(&msg, &signed.sig, &signed.pubkey) {
                return FrameVerifyResult::Reject(FrameRejectReason::BadSignature);
            }
        } else if require_signed {
            // FAIL-CLOSED: an unsigned frame (no sig/pubkey) cannot be
            // authenticated, so under require_signed_join it is dropped. This is
            // the check that makes require_signed_join real (was a no-op pre-M3).
            tracing::debug!(member = %signed.member_id, "pair: dropped unsigned frame (require_signed_join)");
            return FrameVerifyResult::Reject(FrameRejectReason::UnknownMember);
        }
        // Record the binding only for authenticated frames. Never pin an empty
        // pubkey as an identity (Option<String>; a legacy empty stays unpinned).
        if signed_ok {
            self.member_pubkeys
                .entry(signed.member_id.clone())
                .or_insert_with(|| Some(signed.pubkey.clone()));
        }

        // --- (3) anti-replay via signed per-connection epoch ---
        // Accept iff the (epoch, seq) pair is strictly newer than what we stored
        // for this member: a higher epoch resets seq tracking (a genuine
        // reconnect picks a strictly-greater epoch), the same epoch requires a
        // strictly-greater seq, and a lower epoch (or non-increasing seq within
        // the epoch) is a replay. epoch+seq are both bound into the signature.
        if let Some(&(last_epoch, last_seq)) = self.last_seq.get(&signed.member_id) {
            let newer = signed.epoch > last_epoch
                || (signed.epoch == last_epoch && signed.seq > last_seq);
            if !newer {
                tracing::debug!(
                    member = %signed.member_id,
                    epoch = signed.epoch, seq = signed.seq,
                    last_epoch, last_seq,
                    "pair: stale (epoch, seq)"
                );
                return FrameVerifyResult::Reject(FrameRejectReason::StaleSeq);
            }
        }
        self.last_seq
            .insert(signed.member_id.clone(), (signed.epoch, signed.seq));

        // --- (4) self-asserted `from` binding ---
        // Bind a navigator frame's inner `from` to the verified `member_id` so a
        // member can't sign a frame attributed to someone else (the driver, say).
        // Only enforced for authenticated frames (the unsigned legacy path has no
        // verified identity to bind to).
        if signed_ok {
            match &signed.frame {
                PairFrame::TermInput { from, .. } if *from != signed.member_id => {
                    tracing::warn!(member = %signed.member_id, %from, "pair: term_input `from` != signer");
                    return FrameVerifyResult::Reject(FrameRejectReason::NotDriver);
                }
                PairFrame::Cursor { from, .. }
                | PairFrame::RequestControl { from }
                | PairFrame::Join { from, .. }
                    if *from != signed.member_id =>
                {
                    tracing::warn!(member = %signed.member_id, %from, "pair: frame `from` != signer");
                    return FrameVerifyResult::Reject(FrameRejectReason::PubkeyMismatch);
                }
                _ => {}
            }
        }

        // --- (5) host-authority (bound to the invite capability) ---
        // The host identity is pinned OUT OF BAND (host: at pair_start to its own
        // identity; navigator: from the invite, which carries the host pubkey)
        // BEFORE any frame is processed — NOT first-SessionMeta-wins. So a
        // host-only frame (incl. SessionMeta) is honored ONLY when the verified
        // signer's pubkey AND member_id match the pinned host. While no host is
        // pinned we FAIL CLOSED and refuse every host-only frame. This closes the
        // race where any member could forge a SessionMeta to claim the host slot.
        //
        // Only meaningful for authenticated frames; the legacy unsigned path
        // (`require_signed = false`, `signed_ok = false`) has no verified signer,
        // so it skips this and relies on the operator opting into permissive mode.
        if signed_ok && signed.frame.is_host_only() {
            match self.pinned_host.as_ref() {
                // Authority is anchored on the PUBKEY (from the invite). The
                // verified signer's pubkey must equal the pinned host pubkey...
                Some(host) if host.pubkey == signed.pubkey => {
                    // ...and the host member_id must be consistent: learn it on
                    // first host frame (None), then require it to stay the same.
                    match host.member_id.as_deref() {
                        Some(mid) if mid != signed.member_id => {
                            tracing::warn!(
                                member = %signed.member_id, host = %mid,
                                "pair: host pubkey reused under a different member_id"
                            );
                            return FrameVerifyResult::Reject(FrameRejectReason::NotHost);
                        }
                        Some(_) => { /* consistent — allow */ }
                        None => {
                            // Bind the host's member_id now (first host frame).
                            if let Some(h) = self.pinned_host.as_mut() {
                                h.member_id = Some(signed.member_id.clone());
                            }
                        }
                    }
                }
                Some(_) => {
                    tracing::warn!(
                        member = %signed.member_id,
                        "pair: host-only frame from non-host (pubkey mismatch)"
                    );
                    return FrameVerifyResult::Reject(FrameRejectReason::NotHost);
                }
                None => {
                    tracing::warn!(
                        member = %signed.member_id,
                        "pair: host-only frame but no pinned host (fail closed)"
                    );
                    return FrameVerifyResult::Reject(FrameRejectReason::NotHost);
                }
            }
        }

        FrameVerifyResult::Accept
    }

    /// Pure host-authority predicate used by `apply_pair_frame` as
    /// defense-in-depth: may a host-only frame from `verified_sender` be applied?
    ///
    /// `true` when the frame is not host-only, when there is no verified sender
    /// (legacy permissive path, `require_signed_join = false`), or when the
    /// verified sender's `member_id` IS the pinned host's. `false` (drop) when a
    /// host-only frame has a verified sender that is not the pinned host —
    /// INCLUDING when no host is pinned yet (fail closed: `verify_signed_frame`
    /// already rejects this, and this mirrors it so the invariant is local here).
    pub(in crate::app_state) fn host_authority_allows(
        &self,
        frame: &PairFrame,
        verified_sender: Option<&str>,
    ) -> bool {
        if !frame.is_host_only() {
            return true;
        }
        match verified_sender {
            // Legacy unsigned path (require_signed_join = false): no verified
            // identity to gate on, so permit (the operator opted in).
            None => true,
            // Signed path: a host must be pinned (fail closed when not), and the
            // verified sender must be the host's bound member_id. `verify_signed_
            // frame` binds that member_id (on the first host frame) BEFORE this
            // runs, so by the time a host-only frame reaches apply it is Some.
            Some(sender) => self
                .pinned_host
                .as_ref()
                .is_some_and(|h| h.member_id.as_deref() == Some(sender)),
        }
    }
}

/// Upper bound on the host's mid-join replay ring buffer (~256 KB). A late
/// joiner gets the most recent `MAX_SNAPSHOT_BYTES` of raw PTY output replayed
/// as one `Snapshot` frame, so its terminal starts populated instead of blank.
/// 256 KB comfortably holds several screenfuls of scrollback plus any escape
/// sequences (colors, cursor moves) needed to reconstruct the visible state,
/// while staying small enough that a single `Snapshot` frame is cheap to
/// encrypt + fan out.
pub(in crate::app_state) const MAX_SNAPSHOT_BYTES: usize = 256 * 1024;

/// A bounded ring buffer of the HOST's recent raw PTY output, used to replay a
/// mid-session snapshot to late joiners (see [`MAX_SNAPSHOT_BYTES`]).
///
/// Raw bytes are pushed verbatim (escape sequences included) so a replay is a
/// faithful `term.reset()` + `term.write(snapshot)` of recent terminal state.
/// When the buffer would exceed the cap, the oldest bytes are dropped from the
/// front. Truncating at a byte boundary can clip a multi-byte UTF-8 char or an
/// escape sequence at the very start, but xterm tolerates a leading partial
/// sequence and the live stream immediately corrects it — acceptable for a
/// best-effort "don't start blank" snapshot.
///
/// Empty / unused on navigators (only the host taps its PTY).
#[derive(Default)]
pub(in crate::app_state) struct PairSnapshotBuffer {
    buf: std::collections::VecDeque<u8>,
}

impl PairSnapshotBuffer {
    /// Append a chunk of host PTY output, evicting the oldest bytes to stay
    /// within [`MAX_SNAPSHOT_BYTES`].
    pub(in crate::app_state) fn push(&mut self, data: &[u8]) {
        // A single chunk larger than the cap: keep only its tail.
        if data.len() >= MAX_SNAPSHOT_BYTES {
            self.buf.clear();
            self.buf.extend(&data[data.len() - MAX_SNAPSHOT_BYTES..]);
            return;
        }
        self.buf.extend(data);
        let overflow = self.buf.len().saturating_sub(MAX_SNAPSHOT_BYTES);
        if overflow > 0 {
            self.buf.drain(0..overflow);
        }
    }

    /// Snapshot the buffered bytes into a contiguous `Vec` for a `Snapshot`
    /// frame. Empty when nothing has been buffered yet.
    pub(in crate::app_state) fn snapshot(&self) -> Vec<u8> {
        self.buf.iter().copied().collect()
    }

    /// Clear the buffer (called on each `start_pair`).
    pub(in crate::app_state) fn clear(&mut self) {
        self.buf.clear();
    }
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

        // SECURITY (M3): pair frames are END-TO-END SIGNED with each app's ECDSA
        // identity. With `require_signed_join` (the default) every inbound frame
        // must carry a valid signature binding member_id↔pubkey: impersonation,
        // driver-spoofing, and host-only stream-spoofing are rejected, and the
        // room key is confidentiality-only. The feature is still EXPERIMENTAL and
        // off by default; surfaced once here (gated on `collab.enabled`) so it is
        // never silently on. With `require_signed_join = false` the legacy
        // permissive (unsigned, self-asserted `from`) path is used.
        if self.config.collab.require_signed_join {
            tracing::info!("Pair programming (EXPERIMENTAL): end-to-end signed frames ENFORCED (require_signed_join).");
        } else {
            tracing::warn!(
                "Pair programming (EXPERIMENTAL): require_signed_join is FALSE — frames are \
                 unsigned and `from` is self-asserted (no per-member authentication). \
                 The room key is confidentiality-only, NOT a security boundary."
            );
        }

        // M3: start each session with a clean auth roster (TOFU pubkeys, seq
        // counters, pinned host). Idempotent re-invocations from the handlers
        // re-enter here only when no worker is running, so this never clobbers
        // a live session's roster.
        self.reset_pair_auth();
        // Start each session with an empty replay buffer (a brand-new shared
        // terminal has no history to snapshot to late joiners) and a zero output
        // offset (the snapshot-dedup monotonic byte counter).
        self.pair_snapshot_buf.clear();
        self.pair_output_offset = 0;

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

        // M3: hand the worker a detached, Send-able ECDSA signer so it can wrap
        // every outbound PairFrame in a SignedPairFrame at the point of send.
        // `None` (no CryptoService) → legacy unsigned wire form.
        let signer = self.crypto.as_ref().map(|c| c.pair_frame_signer());

        let room_config = PairRoomClientConfig {
            relay_url,
            session_id: session_id.clone(),
            member_id: member_id.clone(),
            key_rx,
            signer,
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
                                // Unused by the default host path (pair_enqueue_output
                                // now sends a fully-framed PtyOutput with an offset via
                                // PairCommand::Send); kept for completeness. No offset
                                // here (0 → no dedup), so the default path is the one
                                // that carries dedup offsets.
                                let frame = PairFrame::PtyOutput { data, offset: 0 };
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
                    PairRoomEvent::SignedFrame(signed) => PairInbound::SignedFrame(signed),
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
                        // M3: announce our ECDSA identity pubkey so the host can
                        // pin `member_id → pubkey` (TOFU). Empty if crypto is
                        // unavailable (legacy unsigned path).
                        let pubkey = self
                            .crypto
                            .as_ref()
                            .map(|c| c.pubkey_base64.clone())
                            .unwrap_or_default();
                        self.send_pair_frame(PairFrame::Join {
                            from: member_id,
                            name,
                            pubkey,
                        });
                    }
                }
                PairInbound::MemberJoined { member_id } => {
                    // Host side: a relay-level `member_joined` carries only an
                    // unauthenticated member_id slot. Under `require_signed_join`
                    // (the default) we DO NOT register it in the PairManager yet —
                    // registration (which makes a member a `set_driver`/
                    // `relay_input` candidate) waits for the member's VERIFIED
                    // signed `Join` frame (handled in `apply_pair_frame`, gated by
                    // `verify_signed_frame`). This is the signed-join enforcement:
                    // an unsigned / un-joined relay peer never enters the roster.
                    // When `require_signed_join` is false we keep the legacy
                    // behaviour (register immediately from the relay event).
                    if self.is_pair_host() {
                        if let Some(session_id) = self.pair_session_id.clone() {
                            if !self.config.collab.require_signed_join {
                                self.host_register_member(&session_id, &member_id, &member_id);
                            }
                            // M3 mid-join replay: send the late joiner a snapshot
                            // of recent terminal state so its xterm starts
                            // populated instead of blank (terminal state only — it
                            // grants no authority, so it is safe to send before the
                            // signed Join). The relay does no replay, so without
                            // this a member joining mid-task would see nothing until
                            // the next PTY output. The snapshot is addressed to
                            // `member_id` so existing navigators ignore it.
                            self.send_pair_snapshot(&session_id, &member_id);
                        }
                    }
                    self.send_pair_event(&serde_json::json!({
                        "event": "member_joined",
                        "member_id": member_id,
                    }));
                }
                PairInbound::MemberLeft { member_id } => {
                    // Prune the departed member's identity / anti-replay roster
                    // entries (keeps a long session from accumulating stale state;
                    // never prunes the pinned host).
                    self.pair_prune_member(&member_id);
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
                PairInbound::Frame(frame) => {
                    // LEGACY unsigned path (a bare PairFrame on the wire). Under
                    // `require_signed_join` (the default) it carries no verifiable
                    // identity, so it is DROPPED fail-closed — this is what makes
                    // the flag real for the unsigned wire form (the signed path is
                    // gated in `verify_signed_frame`). Only when the operator has
                    // explicitly opted into the permissive legacy mode
                    // (`require_signed_join = false`) is it applied, with `None`
                    // as the verified sender (no authenticated identity).
                    if self.config.collab.require_signed_join {
                        tracing::debug!(
                            frame_type = frame.frame_type(),
                            "Pair: dropped unsigned legacy frame (require_signed_join)"
                        );
                    } else {
                        self.apply_pair_frame(frame, None);
                    }
                }
                PairInbound::SignedFrame(signed) => {
                    // M3: fully authenticate the envelope (identity binding +
                    // signature + anti-replay + self-`from` binding + host
                    // authority) BEFORE honoring the inner frame. On Accept,
                    // `signed.member_id` is the VERIFIED sender, and every
                    // navigator frame's inner `from` has been bound to equal it,
                    // so apply_pair_frame's PairManager driver-gating now gates on
                    // an authenticated identity. A reject DROPS the frame here so
                    // it never reaches apply_pair_frame. The verified sender is
                    // threaded into apply_pair_frame for defense-in-depth
                    // host/driver authority re-checks.
                    match self.verify_signed_frame(&signed) {
                        FrameVerifyResult::Accept => {
                            let sender = signed.member_id.clone();
                            self.apply_pair_frame(signed.frame, Some(&sender));
                        }
                        FrameVerifyResult::Reject(reason) => {
                            tracing::debug!(
                                member = %signed.member_id,
                                ?reason,
                                "Pair: rejected signed frame"
                            );
                        }
                    }
                }
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

    // ========================================================================
    // M3 SIGNING / VERIFICATION SEAMS (IMPLEMENTED)
    // ========================================================================

    /// INBOUND SEAM: verify a `SignedPairFrame` against the auth roster before
    /// `apply_pair_frame` honors it. On `Accept`, the VERIFIED sender is
    /// `signed.member_id` — the signature proved possession of the private key
    /// for the pubkey pinned to that member_id, so the caller may trust
    /// `signed.member_id` and pass it to `apply_pair_frame` for the driver check.
    ///
    /// Enforced, in order (any failure → `Reject`, the frame is DROPPED):
    ///   1. identity binding: `member_id → pubkey` TOFU — pin on first signed
    ///      frame, then every later frame from that member_id MUST carry the same
    ///      pubkey (`PubkeyMismatch` on a swap = impersonation attempt).
    ///   2. signature valid over the canonical bytes vs the carried pubkey
    ///      (`BadSignature`). Under `require_signed_join`, an UNSIGNED frame
    ///      (empty sig/pubkey) is rejected fail-closed (`UnknownMember`).
    ///   3. anti-replay: strictly-increasing per-sender `seq`, modulo the
    ///      reconnect reset window (`StaleSeq`).
    ///   4. self-asserted-`from` binding: a navigator frame's inner `from` must
    ///      equal the verified `member_id` — a member cannot sign a frame that
    ///      claims to originate from someone else (`NotDriver` for `term_input`,
    ///      `PubkeyMismatch` for the others).
    ///   5. host-authority: the host identity `(member_id, pubkey)` is pinned
    ///      OUT OF BAND (host: at `pair_start`; navigator: from the INVITE, which
    ///      carries the host pubkey) before any frame is processed. Host-only
    ///      frames (`pty_output`/`resize`/`driver_changed`/`snapshot`/
    ///      `session_meta`) are accepted ONLY when the verified signer's pubkey
    ///      AND member_id match the pinned host; while no host is pinned ALL
    ///      host-only frames are refused (`NotHost`, fail closed).
    ///
    /// The final `term_input` DRIVER check (verified sender == current driver)
    /// stays in `apply_pair_frame`, enforced by `PairManager::relay_input`; step
    /// 4 above guarantees the `from` it gates on is the verified identity.
    pub(in crate::app_state) fn verify_signed_frame(
        &mut self,
        signed: &SignedPairFrame,
    ) -> FrameVerifyResult {
        let Some(session_id) = self.pair_session_id.clone() else {
            return FrameVerifyResult::Reject(FrameRejectReason::UnknownMember);
        };
        let require_signed = self.config.collab.require_signed_join;
        // Thread the signature check through a closure so the decision logic is a
        // pure function of `PairAuthState` (unit-testable without a JarvisApp).
        // `crypto` is the only verifier; absent crypto, every signature "fails"
        // (the require_signed_join fail-closed path then drops it).
        let crypto = self.crypto.as_ref();
        let verify = |msg_b64: &str, sig: &str, pk: &str| -> bool {
            crypto
                .map(|c| c.verify(msg_b64, sig, pk).unwrap_or(false))
                .unwrap_or(false)
        };
        self.pair_auth
            .verify_signed_frame(&session_id, require_signed, signed, verify)
    }

    /// Host-side identity pin from a signed `Join` (TOFU). Records
    /// `member_id → pubkey` so later frames from this member must match. No-op
    /// for an empty pubkey (legacy unsigned Join).
    pub(in crate::app_state) fn pair_pin_member_identity(&mut self, member_id: &str, pubkey: &str) {
        if pubkey.is_empty() {
            return;
        }
        match self.pair_auth.member_pubkeys.get(member_id) {
            Some(Some(existing)) if existing != pubkey => {
                tracing::warn!(member = %member_id, "pair: Join pubkey conflicts with pinned identity");
            }
            _ => {
                self.pair_auth
                    .member_pubkeys
                    .insert(member_id.to_string(), Some(pubkey.to_string()));
            }
        }
    }

    /// Reset the M3 auth roster (called on `start_pair` so each session begins
    /// with a clean TOFU table and seq counters).
    pub(in crate::app_state) fn reset_pair_auth(&mut self) {
        self.pair_auth = PairAuthState::default();
    }

    /// Pin the host identity for this session. THE critical host-authority
    /// binding, anchored on the host PUBKEY: the HOST calls this at `pair_start`
    /// with its own `(member_id, pubkey)` (it created the session and knows who
    /// it is); the NAVIGATOR calls it from the invite (host pubkey, `member_id =
    /// None`) BEFORE any frame is processed. Thereafter `verify_signed_frame`
    /// accepts host-only frames ONLY from the pinned host pubkey (binding the
    /// host member_id on the first such frame). No-op for an empty pubkey (the
    /// host stays unpinned → host-only frames fail closed under
    /// `require_signed_join`).
    pub(in crate::app_state) fn pair_pin_host(
        &mut self,
        member_id: Option<&str>,
        pubkey: &str,
    ) {
        if pubkey.is_empty() {
            return; // never pin an empty/unknown host (stays fail-closed)
        }
        self.pair_auth.pinned_host = Some(super::pair::PinnedHost {
            member_id: member_id.filter(|m| !m.is_empty()).map(|m| m.to_string()),
            pubkey: pubkey.to_string(),
        });
        if let Some(mid) = member_id.filter(|m| !m.is_empty()) {
            self.pair_pin_member_identity(mid, pubkey);
        }
    }

    /// Prune a departed member's identity / anti-replay roster entries (called on
    /// `member_left`) so a long-lived session does not accumulate stale state.
    /// Never prunes the pinned host (its capability outlives a transient drop).
    pub(in crate::app_state) fn pair_prune_member(&mut self, member_id: &str) {
        let is_host = self
            .pair_auth
            .pinned_host
            .as_ref()
            .is_some_and(|h| h.member_id.as_deref() == Some(member_id));
        if is_host {
            return;
        }
        self.pair_auth.member_pubkeys.remove(member_id);
        self.pair_auth.last_seq.remove(member_id);
    }

    /// Apply a single inbound `PairFrame`, host-authoritatively.
    ///
    /// SECURITY (M3 — ENFORCED):
    /// Pair frames are END-TO-END SIGNED with each app's ECDSA identity, verified
    /// in [`Self::verify_signed_frame`] BEFORE this is called. `verified_sender`
    /// is `Some(member_id)` for an authenticated signed frame (the signature
    /// proved possession of the private key pinned to that member_id) and `None`
    /// only on the legacy permissive path (`require_signed_join = false`).
    ///
    /// This applies the authority model as DEFENSE-IN-DEPTH on top of
    /// `verify_signed_frame` (which already drops host-only frames not from the
    /// pinned host and binds each navigator frame's inner `from` to the verified
    /// signer):
    ///   - Host-only frames (`pty_output`/`resize`/`driver_changed`/`snapshot`/
    ///     `session_meta`) are honored ONLY when `verified_sender` equals the
    ///     pinned host — else DROPPED + debug-logged (stream-spoof defense).
    ///   - `term_input` is honored ONLY when `verified_sender` equals the current
    ///     `driver_user_id` (read from the host's `PairManager`) AND passes the
    ///     `PairManager::relay_input` driver-gate, which ALSO still gates.
    ///   - `request_control`/`cursor`/`join` are accepted from any verified
    ///     member.
    ///
    /// RESIDUAL (denial only): the relay now BINDS each Room slot to a key — every
    /// member signs its `room_hello` and the relay TOFU-pins `(session_id,
    /// member_id) -> pubkey` and enforces a strictly-monotonic nonce (see
    /// `jarvis-relay/src/room_auth.rs`), so a peer who merely knows the
    /// `session_id` can no longer churn, replace, or evict another member's pinned
    /// slot (wrong key -> rejected; replayed/older nonce -> rejected). What remains
    /// is FIRST-MOVER pinning: the pair `member_id` is a random token with no
    /// pubkey relationship, so a malicious party who learns a `session_id` could
    /// race to pin a member_id before its legitimate owner connects — disrupting
    /// the join for DENIAL only. It cannot forge CONTENT: every honored frame is
    /// end-to-end signed, so the host identity (pinned from the invite pubkey) and
    /// each member identity are non-forgeable — an attacker can only deny/disrupt,
    /// never impersonate. Fully closing first-mover pinning would require deriving
    /// the pair `member_id` from the pubkey (an id-format change); tracked in
    /// `dev/plans/c2-pair-programming.md` §6.
    fn apply_pair_frame(&mut self, frame: PairFrame, verified_sender: Option<&str>) {
        let Some(session_id) = self.pair_session_id.clone() else {
            return;
        };

        // Host-authority defense-in-depth: for a host-only frame, require that
        // the verified signer is the pinned host. `verify_signed_frame` already
        // enforces this for signed frames, but re-checking here keeps the
        // invariant local to where the frame is honored (and covers any future
        // caller). On the legacy unsigned path (`verified_sender == None`) there
        // is no identity to check, so it passes (that path only runs when
        // `require_signed_join = false`).
        if !self.pair_auth.host_authority_allows(&frame, verified_sender) {
            tracing::debug!(
                sender = verified_sender,
                host = self
                    .pair_auth
                    .pinned_host
                    .as_ref()
                    .and_then(|h| h.member_id.as_deref()),
                frame_type = frame.frame_type(),
                "Pair: dropped host-only frame from non-host (apply)"
            );
            return;
        }

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
                // M3 driver authority (defense-in-depth): the verified signer must
                // be the current driver. `verify_signed_frame` already bound
                // `from == verified_sender`, and `relay_input` below ALSO gates on
                // the driver, but we re-read the authoritative driver here so a
                // non-driver's keystrokes are dropped even if `relay_input`'s
                // gating ever changes. Skipped on the legacy unsigned path.
                if let Some(sender) = verified_sender {
                    let driver = self
                        .pair_manager
                        .clone()
                        .zip(self.tokio_runtime.as_ref())
                        .and_then(|(m, rt)| rt.block_on(m.get_session(&session_id)))
                        .map(|s| s.driver_user_id);
                    if let Some(driver) = driver {
                        if sender != driver {
                            tracing::debug!(
                                sender = %sender,
                                driver = %driver,
                                "Pair: dropped term_input from non-driver (apply)"
                            );
                            return;
                        }
                    }
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
            // names. M3: under `require_signed_join` this frame only reaches here
            // after `verify_signed_frame` authenticated the sender and bound
            // `from == verified_sender`, so the registration is of a verified
            // identity (not a self-asserted claim).
            PairFrame::Join { from, name, pubkey } => {
                if !self.is_pair_host() {
                    return; // only the host owns the roster
                }
                // Pin `from → pubkey` in the member roster (TOFU) so every later
                // frame from `from` must verify against this pubkey. `pubkey` is
                // carried here AND bound to the outer SignedPairFrame signature,
                // so it is the sender's attested identity, not a self-asserted
                // claim.
                self.pair_pin_member_identity(&from, &pubkey);
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
            PairFrame::PtyOutput { data, offset } => {
                // Send the raw bytes as base64 (lossless): PTY output is arbitrary
                // bytes (escape sequences, partial UTF-8), so a lossy String would
                // corrupt the stream. The panel base64-decodes before term.write.
                // `offset` lets the joiner dedup chunks the join snapshot covered.
                self.send_pair_event(&serde_json::json!({
                    "event": "pty_output",
                    "data": b64_encode(&data),
                    "offset": offset,
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
                target,
                offset,
            } => {
                self.send_pair_event(&serde_json::json!({
                    "event": "snapshot",
                    "data": b64_encode(&data),
                    "cols": cols,
                    "rows": rows,
                    "driver": driver,
                    "target": target,
                    "offset": offset,
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
    pub(in crate::app_state) fn pair_enqueue_output(&mut self, data: Vec<u8>) {
        // Sink 3 (M3 mid-join replay): record into the bounded ring buffer so a
        // late joiner can be sent a `Snapshot` of recent terminal state instead
        // of a blank screen. Pushed BEFORE fan-out so the snapshot includes the
        // chunk we are about to broadcast.
        self.pair_snapshot_buf.push(&data);

        // Advance the monotonic output offset (snapshot-dedup byte counter) by
        // this chunk; the resulting offset (cumulative bytes INCLUDING this
        // chunk) is stamped onto the frame and is what a late joiner dedups
        // against.
        self.pair_output_offset = self
            .pair_output_offset
            .saturating_add(data.len() as u64);
        let offset = self.pair_output_offset;

        // Sink 2: mirror to the host's own panel so the host sees the shared
        // terminal it is broadcasting. (The host is never a late joiner, so the
        // offset is irrelevant here, but we send it for symmetry.)
        self.send_pair_event(&serde_json::json!({
            "event": "pty_output",
            "data": b64_encode(&data),
            "offset": offset,
        }));

        // Sink 1: fan out to the room for navigators, stamped with the offset so
        // a late joiner can dedup against the snapshot. Goes through the SINGLE
        // outbound signing seam (PairCommand::Send → worker sign_frame).
        if let Some(ref tx) = self.pair_cmd_tx {
            let frame = PairFrame::PtyOutput { data, offset };
            match tx.try_send(PairCommand::Send(frame)) {
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

    /// Host-only: send a mid-join `Snapshot` of recent terminal state addressed
    /// to a single late joiner.
    ///
    /// Replays the bounded [`PairSnapshotBuffer`] so the joiner's xterm does
    /// `term.reset()` + `term.write(snapshot)` and starts populated. The frame
    /// carries `target = joiner` so existing navigators (the relay fans it to
    /// every member) ignore it instead of resetting their own view. No-op when
    /// the buffer is empty (nothing to replay yet) or we are not the host.
    fn send_pair_snapshot(&self, session_id: &str, target: &str) {
        let data = self.pair_snapshot_buf.snapshot();
        if data.is_empty() {
            return;
        }
        // Read authoritative dims + current driver from the host's PairManager.
        let session = match (self.pair_manager.clone(), self.tokio_runtime.as_ref()) {
            (Some(manager), Some(rt)) => rt.block_on(manager.get_session(session_id)),
            _ => None,
        };
        let Some(session) = session else {
            return; // not the host / no live session
        };
        self.send_pair_frame(PairFrame::Snapshot {
            data,
            cols: session.cols,
            rows: session.rows,
            driver: session.driver_user_id.clone(),
            target: target.to_string(),
            // The snapshot covers all host PTY output up to the current offset;
            // the joiner drops live pty_output chunks at/below this (dedup).
            offset: self.pair_output_offset,
        });
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
        PairEvent::TerminalOutput { data, .. } => Some(PairFrame::PtyOutput { data, offset: 0 }),
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

/// Base64 of the canonical signing bytes so they can pass through
/// `CryptoService::sign`/`verify`, which take `&str`. Both sign and verify use
/// this exact mapping, so it is signature-transparent (the underlying ECDSA
/// still authenticates the original canonical bytes 1:1).
fn base64_of(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    B64.encode(bytes)
}

/// Build the shareable INVITE string for a host session: a base64url (no pad)
/// of `session_id ":" host_pubkey`. The navigator parses both fields out of the
/// invite and pins the host identity from the pubkey BEFORE processing any
/// frame, so only the real host (whose pubkey is in the invite) can drive
/// host-only frames — closing the SessionMeta host-claim race.
///
/// `session_id` is alnum and `:` is the single delimiter; the host pubkey
/// (base64 SPKI) may itself contain `+`/`/`/`=`, so the JOIN-side split is on
/// the FIRST `:` only.
pub(in crate::app_state) fn make_invite(session_id: &str, host_pubkey: &str) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
    use base64::Engine;
    let raw = format!("{session_id}:{host_pubkey}");
    B64URL.encode(raw.as_bytes())
}

/// Parse an invite string back into `(session_id, host_pubkey)`. Tolerates a
/// bare session id (no `:`) for backward/manual entry — then `host_pubkey` is
/// empty (the navigator stays unpinned and host-only frames fail closed, which
/// is the safe default). Returns `None` only if the input is neither valid
/// base64url nor a plausible bare session id.
pub(in crate::app_state) fn parse_invite(invite: &str) -> Option<(String, String)> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
    use base64::Engine;
    let invite = invite.trim();
    if invite.is_empty() {
        return None;
    }
    // Try base64url(session_id:host_pubkey) first.
    if let Ok(bytes) = B64URL.decode(invite.as_bytes()) {
        if let Ok(s) = String::from_utf8(bytes) {
            if let Some((sid, pk)) = s.split_once(':') {
                if !sid.is_empty() {
                    return Some((sid.to_string(), pk.to_string()));
                }
            }
        }
    }
    // Fall back to a bare session id (legacy/manual): no host pubkey to pin.
    Some((invite.to_string(), String::new()))
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
            Some(PairFrame::PtyOutput { data, .. }) => assert_eq!(data, vec![0x1b, b'[', b'2', b'J']),
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

    // ----- M3 mid-join replay ring buffer -----

    /// Under the cap, the snapshot is the exact concatenation of pushed chunks
    /// in order (so a replay faithfully reconstructs recent terminal state).
    #[test]
    fn snapshot_buffer_preserves_bytes_under_cap() {
        let mut b = PairSnapshotBuffer::default();
        assert!(b.snapshot().is_empty(), "empty buffer => empty snapshot");
        b.push(b"hello ");
        b.push(&[0x1b, b'[', b'2', b'J']); // an escape sequence, kept verbatim
        b.push(b"world");
        assert_eq!(b.snapshot(), b"hello \x1b[2Jworld");
    }

    /// Once the buffer exceeds the cap it keeps only the most recent
    /// `MAX_SNAPSHOT_BYTES` bytes (oldest evicted from the front).
    #[test]
    fn snapshot_buffer_evicts_oldest_over_cap() {
        let mut b = PairSnapshotBuffer::default();
        // Fill exactly to the cap with 'a', then push one screenful of 'b'.
        b.push(&vec![b'a'; MAX_SNAPSHOT_BYTES]);
        b.push(&vec![b'b'; 100]);
        let snap = b.snapshot();
        assert_eq!(snap.len(), MAX_SNAPSHOT_BYTES, "never grows past the cap");
        // The tail is the newest bytes; the head lost 100 'a's.
        assert_eq!(&snap[snap.len() - 100..], &vec![b'b'; 100][..]);
        assert_eq!(snap[0], b'a');
    }

    /// A single chunk larger than the cap is truncated to its tail (the most
    /// recent bytes), never panicking on the underflow path.
    #[test]
    fn snapshot_buffer_handles_oversize_single_chunk() {
        let mut b = PairSnapshotBuffer::default();
        b.push(b"stale");
        let mut big = vec![b'x'; MAX_SNAPSHOT_BYTES + 50];
        // Tag the last byte so we can prove the TAIL is kept.
        *big.last_mut().unwrap() = b'Z';
        b.push(&big);
        let snap = b.snapshot();
        assert_eq!(snap.len(), MAX_SNAPSHOT_BYTES);
        assert_eq!(*snap.last().unwrap(), b'Z', "keeps the newest bytes");
    }

    /// `clear` empties the buffer (called on each `start_pair`).
    #[test]
    fn snapshot_buffer_clear_empties() {
        let mut b = PairSnapshotBuffer::default();
        b.push(b"data");
        b.clear();
        assert!(b.snapshot().is_empty());
    }

    /// The `Snapshot` frame carries a `target` so existing navigators ignore a
    /// late-joiner snapshot (round-trips the new field on the wire).
    #[test]
    fn snapshot_frame_carries_target() {
        let frame = PairFrame::Snapshot {
            data: b"screen".to_vec(),
            cols: 80,
            rows: 24,
            driver: "m1".into(),
            target: "m2".into(),
            offset: 0,
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("\"target\":\"m2\""));
        match serde_json::from_str::<PairFrame>(&json).unwrap() {
            PairFrame::Snapshot { target, .. } => assert_eq!(target, "m2"),
            _ => panic!("expected snapshot"),
        }
        // Wire-compat: a pre-targeting snapshot (no `target`) still parses to "".
        let legacy = r#"{"type":"snapshot","data":"","cols":80,"rows":24,"driver":"m1"}"#;
        match serde_json::from_str::<PairFrame>(legacy).unwrap() {
            PairFrame::Snapshot { target, .. } => assert!(target.is_empty()),
            _ => panic!("expected snapshot"),
        }
    }

    // ====================================================================
    // M3: PairAuthState inbound verification decision logic
    // ====================================================================
    //
    // These drive the pure `PairAuthState::verify_signed_frame` with REAL ECDSA
    // signatures (via `CryptoService`) and a real verify closure, so identity
    // binding, signature, anti-replay, self-`from` binding, and host authority
    // are all exercised end-to-end without needing a full `JarvisApp`.

    use jarvis_platform::CryptoService;

    const TSID: &str = "verifySession0123456789abcdef0123";

    /// Build a SignedPairFrame exactly as the worker's `sign_frame` does, so the
    /// canonical bytes + base64 mapping match `verify_signed_frame`. Uses epoch=1.
    fn make_signed(
        crypto: &CryptoService,
        member_id: &str,
        seq: u64,
        frame: PairFrame,
    ) -> SignedPairFrame {
        make_signed_ep(crypto, member_id, 1, seq, frame)
    }

    /// Like `make_signed` but with an explicit `epoch` (for anti-replay tests).
    fn make_signed_ep(
        crypto: &CryptoService,
        member_id: &str,
        epoch: u64,
        seq: u64,
        frame: PairFrame,
    ) -> SignedPairFrame {
        let pubkey = crypto.pubkey_base64.clone();
        let canonical = SignedPairFrame::canonical_signing_bytes(
            TSID, member_id, &pubkey, &frame, epoch, seq,
        )
        .unwrap();
        let sig = crypto.sign(&base64_of(&canonical)).unwrap();
        SignedPairFrame {
            member_id: member_id.into(),
            pubkey,
            epoch,
            seq,
            sig,
            frame,
        }
    }

    /// Pin `host_member`'s identity as the session host (the out-of-band binding
    /// the host does at pair_start / the navigator does from the invite).
    fn pin_host(auth: &mut PairAuthState, host_member: &str, host_pubkey: &str) {
        auth.pinned_host = Some(PinnedHost {
            member_id: Some(host_member.to_string()),
            pubkey: host_pubkey.to_string(),
        });
        auth.member_pubkeys
            .insert(host_member.to_string(), Some(host_pubkey.to_string()));
    }

    /// The verify closure a receiver uses: a fresh `CryptoService` as a pure
    /// verifier (verification needs only the public key carried in the frame).
    fn verifier() -> impl Fn(&str, &str, &str) -> bool {
        let c = CryptoService::generate().unwrap();
        move |msg, sig, pk| c.verify(msg, sig, pk).unwrap_or(false)
    }

    /// A correctly signed frame is accepted, and its identity is pinned (TOFU).
    #[test]
    fn auth_good_signature_accepts_and_pins() {
        let sender = CryptoService::generate().unwrap();
        let mut auth = PairAuthState::default();
        let signed = make_signed(&sender, "m2", 1, PairFrame::Cursor { from: "m2".into(), row: 1, col: 1 });
        assert_eq!(
            auth.verify_signed_frame(TSID, true, &signed, verifier()),
            FrameVerifyResult::Accept
        );
        assert_eq!(auth.member_pubkeys.get("m2"), Some(&Some(sender.pubkey_base64.clone())));
        assert_eq!(auth.last_seq.get("m2"), Some(&(1, 1)));
    }

    /// A tampered inner frame (sig made over different bytes) is rejected.
    #[test]
    fn auth_tampered_frame_rejected() {
        let sender = CryptoService::generate().unwrap();
        let mut auth = PairAuthState::default();
        let mut signed = make_signed(&sender, "m2", 1, PairFrame::TermInput { from: "m2".into(), data: b"ls\n".to_vec() });
        signed.frame = PairFrame::TermInput { from: "m2".into(), data: b"rm -rf /\n".to_vec() };
        assert_eq!(
            auth.verify_signed_frame(TSID, true, &signed, verifier()),
            FrameVerifyResult::Reject(FrameRejectReason::BadSignature)
        );
    }

    /// A sig presented under a different pubkey than the one it was made with
    /// fails the signature check (wrong-key).
    #[test]
    fn auth_wrong_key_rejected() {
        let sender = CryptoService::generate().unwrap();
        let attacker = CryptoService::generate().unwrap();
        let mut auth = PairAuthState::default();
        let mut signed = make_signed(&sender, "m2", 1, PairFrame::Cursor { from: "m2".into(), row: 0, col: 0 });
        signed.pubkey = attacker.pubkey_base64.clone();
        assert_eq!(
            auth.verify_signed_frame(TSID, true, &signed, verifier()),
            FrameVerifyResult::Reject(FrameRejectReason::BadSignature)
        );
    }

    /// Once a member_id is pinned to a pubkey, a later frame from that member_id
    /// carrying a DIFFERENT pubkey is rejected (impersonation / TOFU violation).
    #[test]
    fn auth_member_pubkey_mismatch_rejected() {
        let real = CryptoService::generate().unwrap();
        let imposter = CryptoService::generate().unwrap();
        let mut auth = PairAuthState::default();
        // Pin "m2" → real key.
        let first = make_signed(&real, "m2", 1, PairFrame::RequestControl { from: "m2".into() });
        assert_eq!(auth.verify_signed_frame(TSID, true, &first, verifier()), FrameVerifyResult::Accept);
        // Imposter signs validly with its OWN key but claims member_id "m2".
        let forged = make_signed(&imposter, "m2", 2, PairFrame::RequestControl { from: "m2".into() });
        assert_eq!(
            auth.verify_signed_frame(TSID, true, &forged, verifier()),
            FrameVerifyResult::Reject(FrameRejectReason::PubkeyMismatch)
        );
    }

    /// A replayed frame (seq not strictly increasing) is rejected as stale.
    #[test]
    fn auth_replayed_seq_rejected() {
        let sender = CryptoService::generate().unwrap();
        let mut auth = PairAuthState::default();
        let f1 = make_signed(&sender, "m2", 1, PairFrame::Cursor { from: "m2".into(), row: 0, col: 0 });
        let f2 = make_signed(&sender, "m2", 2, PairFrame::Cursor { from: "m2".into(), row: 0, col: 1 });
        assert_eq!(auth.verify_signed_frame(TSID, true, &f1, verifier()), FrameVerifyResult::Accept);
        assert_eq!(auth.verify_signed_frame(TSID, true, &f2, verifier()), FrameVerifyResult::Accept);
        // Replay f1 (seq=1, ≤ last=2, not far enough below to look like reconnect).
        let replay = make_signed(&sender, "m2", 1, PairFrame::Cursor { from: "m2".into(), row: 0, col: 0 });
        assert_eq!(
            auth.verify_signed_frame(TSID, true, &replay, verifier()),
            FrameVerifyResult::Reject(FrameRejectReason::StaleSeq)
        );
    }

    /// Under require_signed_join, an unsigned frame (no sig/pubkey) is dropped
    /// fail-closed — proving require_signed_join is no longer a no-op.
    #[test]
    fn auth_unsigned_frame_failclosed_under_require_signed() {
        let mut auth = PairAuthState::default();
        let unsigned = SignedPairFrame {
            member_id: "m2".into(),
            pubkey: String::new(),
            epoch: 1,
            seq: 1,
            sig: String::new(),
            frame: PairFrame::TermInput { from: "m2".into(), data: b"x".to_vec() },
        };
        assert_eq!(
            auth.verify_signed_frame(TSID, true, &unsigned, verifier()),
            FrameVerifyResult::Reject(FrameRejectReason::UnknownMember)
        );
        // With require_signed_join OFF, the legacy permissive path accepts it.
        let mut auth2 = PairAuthState::default();
        assert_eq!(
            auth2.verify_signed_frame(TSID, false, &unsigned, verifier()),
            FrameVerifyResult::Accept
        );
    }

    /// A navigator that signs a frame but puts SOMEONE ELSE's id in `from`
    /// (impersonating the driver) is rejected — the inner `from` is bound to the
    /// verified signer.
    #[test]
    fn auth_forged_from_rejected() {
        let sender = CryptoService::generate().unwrap();
        let mut auth = PairAuthState::default();
        // member "m3" signs a term_input claiming from="m2" (the driver).
        let signed = make_signed(&sender, "m3", 1, PairFrame::TermInput { from: "m2".into(), data: b"ls\n".to_vec() });
        assert_eq!(
            auth.verify_signed_frame(TSID, true, &signed, verifier()),
            FrameVerifyResult::Reject(FrameRejectReason::NotDriver)
        );
    }

    /// Host authority (THE critical fix): the host is pinned OUT OF BAND (from
    /// the invite / pair_start), NOT first-SessionMeta-wins. A SessionMeta or
    /// host-only frame whose verified signer pubkey != the pinned host pubkey is
    /// rejected — so a member can no longer race a forged SessionMeta to claim
    /// the host slot.
    #[test]
    fn auth_host_only_frame_from_non_host_rejected() {
        let host = CryptoService::generate().unwrap();
        let attacker = CryptoService::generate().unwrap();
        let mut auth = PairAuthState::default();
        // Host pinned from the invite (pubkey is the authority anchor).
        pin_host(&mut auth, "host1", &host.pubkey_base64);
        // Attacker "evil" forges a host-only SessionMeta (validly signed by its
        // OWN key) trying to become the host — rejected (pubkey != pinned).
        let forged_meta = make_signed(&attacker, "evil", 1, PairFrame::SessionMeta {
            host: "evil".into(),
            host_name: "Evil".into(),
            cols: 80,
            rows: 24,
            allow_takeover: true,
            roster: vec![],
        });
        assert_eq!(
            auth.verify_signed_frame(TSID, true, &forged_meta, verifier()),
            FrameVerifyResult::Reject(FrameRejectReason::NotHost)
        );
        // Attacker also can't forge a host-only pty_output.
        let spoof = make_signed(&attacker, "evil", 2, PairFrame::PtyOutput { data: b"fake".to_vec(), offset: 0 });
        assert_eq!(
            auth.verify_signed_frame(TSID, true, &spoof, verifier()),
            FrameVerifyResult::Reject(FrameRejectReason::NotHost)
        );
    }

    /// Fail-closed: while NO host is pinned, every host-only frame is refused
    /// (even a validly-signed one) — there is no host to authorize it.
    #[test]
    fn auth_host_only_frame_failclosed_when_unpinned() {
        let someone = CryptoService::generate().unwrap();
        let mut auth = PairAuthState::default();
        let out = make_signed(&someone, "m9", 1, PairFrame::PtyOutput { data: b"x".to_vec(), offset: 0 });
        assert_eq!(
            auth.verify_signed_frame(TSID, true, &out, verifier()),
            FrameVerifyResult::Reject(FrameRejectReason::NotHost),
            "host-only frame must fail closed with no pinned host"
        );
    }

    /// The genuine host (matching the pinned pubkey) drives host-only frames, and
    /// its member_id is learned/bound on the first such frame.
    #[test]
    fn auth_host_only_frame_from_host_accepted() {
        let host = CryptoService::generate().unwrap();
        let mut auth = PairAuthState::default();
        // Pin the host by PUBKEY only (navigator-from-invite case: member_id None).
        auth.pinned_host = Some(PinnedHost { member_id: None, pubkey: host.pubkey_base64.clone() });
        let meta = make_signed(&host, "host1", 1, PairFrame::SessionMeta {
            host: "host1".into(),
            host_name: "Host".into(),
            cols: 80,
            rows: 24,
            allow_takeover: true,
            roster: vec![],
        });
        assert_eq!(auth.verify_signed_frame(TSID, true, &meta, verifier()), FrameVerifyResult::Accept);
        // member_id bound on first host frame.
        assert_eq!(auth.pinned_host.as_ref().unwrap().member_id.as_deref(), Some("host1"));
        let out = make_signed(&host, "host1", 2, PairFrame::PtyOutput { data: b"hi".to_vec(), offset: 0 });
        assert_eq!(auth.verify_signed_frame(TSID, true, &out, verifier()), FrameVerifyResult::Accept);
    }

    /// Anti-replay via epoch: a fresh, strictly-greater epoch resets seq tracking
    /// (a genuine reconnect), so a low seq under a higher epoch is accepted.
    #[test]
    fn auth_epoch_bump_resets_seq() {
        let sender = CryptoService::generate().unwrap();
        let mut auth = PairAuthState::default();
        // Pin last_seq high under epoch 1.
        let f = make_signed_ep(&sender, "m2", 1, 20, PairFrame::Cursor { from: "m2".into(), row: 0, col: 0 });
        assert_eq!(auth.verify_signed_frame(TSID, true, &f, verifier()), FrameVerifyResult::Accept);
        // After reconnect the peer bumps epoch to 2 and restarts seq at 1 → accepted.
        let after = make_signed_ep(&sender, "m2", 2, 1, PairFrame::Cursor { from: "m2".into(), row: 0, col: 1 });
        assert_eq!(auth.verify_signed_frame(TSID, true, &after, verifier()), FrameVerifyResult::Accept);
        assert_eq!(auth.last_seq.get("m2"), Some(&(2, 1)));
    }

    /// Anti-replay via epoch: a LOWER epoch (a replayed old-connection frame) is
    /// rejected even with a high seq.
    #[test]
    fn auth_lower_epoch_rejected() {
        let sender = CryptoService::generate().unwrap();
        let mut auth = PairAuthState::default();
        let f = make_signed_ep(&sender, "m2", 5, 1, PairFrame::Cursor { from: "m2".into(), row: 0, col: 0 });
        assert_eq!(auth.verify_signed_frame(TSID, true, &f, verifier()), FrameVerifyResult::Accept);
        // Replay from epoch 4 (older connection), seq high — still rejected.
        let old = make_signed_ep(&sender, "m2", 4, 99, PairFrame::Cursor { from: "m2".into(), row: 0, col: 1 });
        assert_eq!(
            auth.verify_signed_frame(TSID, true, &old, verifier()),
            FrameVerifyResult::Reject(FrameRejectReason::StaleSeq)
        );
    }

    /// Anti-replay within an epoch: a replayed (non-increasing) seq under the
    /// same epoch is rejected.
    #[test]
    fn auth_replay_within_epoch_rejected() {
        let sender = CryptoService::generate().unwrap();
        let mut auth = PairAuthState::default();
        let f1 = make_signed_ep(&sender, "m2", 3, 1, PairFrame::Cursor { from: "m2".into(), row: 0, col: 0 });
        let f2 = make_signed_ep(&sender, "m2", 3, 2, PairFrame::Cursor { from: "m2".into(), row: 0, col: 1 });
        assert_eq!(auth.verify_signed_frame(TSID, true, &f1, verifier()), FrameVerifyResult::Accept);
        assert_eq!(auth.verify_signed_frame(TSID, true, &f2, verifier()), FrameVerifyResult::Accept);
        // Replay f1 (epoch 3, seq 1 ≤ stored seq 2) → stale.
        let replay = make_signed_ep(&sender, "m2", 3, 1, PairFrame::Cursor { from: "m2".into(), row: 0, col: 0 });
        assert_eq!(
            auth.verify_signed_frame(TSID, true, &replay, verifier()),
            FrameVerifyResult::Reject(FrameRejectReason::StaleSeq)
        );
    }

    /// member_id-bound signature: a frame signed as member_id "m2" but RELABELED
    /// to "m3" (to dodge m3's seq tracker) fails the signature check, because
    /// member_id is now covered by the canonical signing bytes.
    #[test]
    fn auth_relabeled_member_id_rejected() {
        let sender = CryptoService::generate().unwrap();
        let mut auth = PairAuthState::default();
        let mut signed = make_signed(&sender, "m2", 1, PairFrame::Cursor { from: "m2".into(), row: 0, col: 0 });
        // Relabel the envelope to a different member_id, keeping the sig.
        signed.member_id = "m3".into();
        assert_eq!(
            auth.verify_signed_frame(TSID, true, &signed, verifier()),
            FrameVerifyResult::Reject(FrameRejectReason::BadSignature),
            "relabeling member_id must break the signature"
        );
    }

    /// Domain separation: a signature whose canonical bytes were built WITHOUT
    /// the pair-sig domain tag (as a relay/pairing/theme signer would produce)
    /// does not verify as a pair frame — the tag prevents cross-presentation.
    #[test]
    fn auth_signature_without_domain_tag_rejected() {
        use base64::engine::general_purpose::STANDARD as B64;
        use base64::Engine;
        let sender = CryptoService::generate().unwrap();
        let mut auth = PairAuthState::default();
        let frame = PairFrame::Cursor { from: "m2".into(), row: 0, col: 0 };
        // Forge canonical bytes WITHOUT the domain tag (legacy layout), sign them.
        let frame_json = serde_json::to_vec(&frame).unwrap();
        let mut buf = Vec::new();
        buf.extend_from_slice(TSID.as_bytes());
        buf.push(0x1F);
        buf.extend_from_slice(frame.frame_type().as_bytes());
        buf.push(0x1F);
        buf.extend_from_slice(b"1");
        buf.push(0x1F);
        buf.extend_from_slice(&frame_json);
        let sig = sender.sign(&B64.encode(&buf)).unwrap();
        let signed = SignedPairFrame {
            member_id: "m2".into(),
            pubkey: sender.pubkey_base64.clone(),
            epoch: 1,
            seq: 1,
            sig,
            frame,
        };
        assert_eq!(
            auth.verify_signed_frame(TSID, true, &signed, verifier()),
            FrameVerifyResult::Reject(FrameRejectReason::BadSignature),
            "a non-domain-separated signature must not verify as a pair frame"
        );
    }

    /// Invite roundtrip: `make_invite` → `parse_invite` recovers both the session
    /// id and the host pubkey (even when the pubkey contains base64 `+`/`/`/`=`).
    #[test]
    fn invite_roundtrips_session_and_pubkey() {
        let sid = "abcDEF0123456789abcDEF0123456789";
        let pk = "MFkwEwYHKoZIzj0CAQYI+abc/def=="; // base64-ish with +, /, =
        let invite = make_invite(sid, pk);
        let (got_sid, got_pk) = parse_invite(&invite).unwrap();
        assert_eq!(got_sid, sid);
        assert_eq!(got_pk, pk);
    }

    /// A bare session id (no host pubkey, manual entry) parses with an empty
    /// pubkey — the navigator stays unpinned and host-only frames fail closed.
    #[test]
    fn invite_bare_session_id_has_empty_pubkey() {
        let (sid, pk) = parse_invite("plainSessionId0123").unwrap();
        assert_eq!(sid, "plainSessionId0123");
        assert!(pk.is_empty());
    }

    // ====================================================================
    // M3 host-authority + signed-join: apply-layer enforcement
    // ====================================================================
    //
    // These exercise the host-authority predicate `apply_pair_frame` uses as
    // defense-in-depth, plus the signed-join fail-closed rule, at the pure
    // `PairAuthState` level (no full JarvisApp needed).

    fn host_only_frame() -> PairFrame {
        PairFrame::PtyOutput { data: b"out".to_vec(), offset: 0 }
    }
    fn nav_frame() -> PairFrame {
        PairFrame::Cursor { from: "m2".into(), row: 1, col: 1 }
    }

    /// Host-authority (apply layer): once a host is pinned, a host-only frame
    /// whose verified sender is NOT the pinned host is dropped.
    #[test]
    fn apply_host_only_frame_from_non_host_dropped() {
        let mut auth = PairAuthState::default();
        auth.pinned_host = Some(PinnedHost { member_id: Some("host1".into()), pubkey: "pk".into() });
        // pty_output (host-only) from a verified non-host member is rejected.
        assert!(
            !auth.host_authority_allows(&host_only_frame(), Some("evil")),
            "host-only frame from non-host must be dropped at apply layer"
        );
        // ...but from the pinned host it is allowed.
        assert!(auth.host_authority_allows(&host_only_frame(), Some("host1")));
    }

    /// Host-authority (apply layer): navigator-origin frames are never gated by
    /// the host check, the legacy unsigned path (no verified sender) passes (it
    /// only runs when require_signed_join = false), and a host-only frame with a
    /// verified sender but NO pinned host fails closed.
    #[test]
    fn apply_host_authority_allows_nav_and_legacy() {
        let mut auth = PairAuthState::default();
        auth.pinned_host = Some(PinnedHost { member_id: Some("host1".into()), pubkey: "pk".into() });
        // A navigator frame from a non-host verified member is fine.
        assert!(auth.host_authority_allows(&nav_frame(), Some("m2")));
        // Legacy unsigned path: no verified sender => host-only frame passes.
        assert!(auth.host_authority_allows(&host_only_frame(), None));
        // Fail closed: a verified host-only frame with no pinned host is dropped.
        let fresh = PairAuthState::default();
        assert!(!fresh.host_authority_allows(&host_only_frame(), Some("anyone")));
    }

    /// term_input from a non-driver is rejected: a member who is NOT the driver
    /// signs a (correctly self-`from`-bound) term_input, and the verify layer
    /// drops it. (`verify_signed_frame` binds `from == signer`; the driver check
    /// itself is `apply_pair_frame` reading the PairManager driver — here we
    /// assert the upstream binding that a forged driver id is already rejected.)
    #[test]
    fn term_input_forged_driver_id_rejected() {
        let nondriver = CryptoService::generate().unwrap();
        let mut auth = PairAuthState::default();
        // "m3" (not the driver) signs term_input but claims from="driver".
        let forged = make_signed(
            &nondriver,
            "m3",
            1,
            PairFrame::TermInput { from: "driver".into(), data: b"evil\n".to_vec() },
        );
        assert_eq!(
            auth.verify_signed_frame(TSID, true, &forged, verifier()),
            FrameVerifyResult::Reject(FrameRejectReason::NotDriver),
            "a member cannot sign term_input attributed to the driver"
        );
    }

    /// Signed-join: an UNSIGNED join (no sig/pubkey) is rejected fail-closed
    /// under require_signed_join, so the host never registers an unverified
    /// member. With require_signed_join off, the legacy path accepts it.
    #[test]
    fn unsigned_join_rejected_when_require_signed() {
        let unsigned_join = SignedPairFrame {
            member_id: "m2".into(),
            pubkey: String::new(),
            epoch: 1,
            seq: 1,
            sig: String::new(),
            frame: PairFrame::Join {
                from: "m2".into(),
                name: "Nav".into(),
                pubkey: String::new(),
            },
        };
        let mut auth = PairAuthState::default();
        assert_eq!(
            auth.verify_signed_frame(TSID, true, &unsigned_join, verifier()),
            FrameVerifyResult::Reject(FrameRejectReason::UnknownMember),
            "unsigned join must be rejected under require_signed_join"
        );
        // The member was NOT pinned (no registration of an unverified identity).
        assert!(auth.member_pubkeys.get("m2").is_none());
        // Legacy permissive path accepts the unsigned join.
        let mut auth_off = PairAuthState::default();
        assert_eq!(
            auth_off.verify_signed_frame(TSID, false, &unsigned_join, verifier()),
            FrameVerifyResult::Accept
        );
    }

    /// A genuine signed join from the navigator is accepted and pins its
    /// identity (so subsequent frames from that member_id must match).
    #[test]
    fn signed_join_accepted_and_pins_identity() {
        let nav = CryptoService::generate().unwrap();
        let mut auth = PairAuthState::default();
        let join = make_signed(
            &nav,
            "m2",
            1,
            PairFrame::Join { from: "m2".into(), name: "Nav".into(), pubkey: nav.pubkey_base64.clone() },
        );
        assert_eq!(auth.verify_signed_frame(TSID, true, &join, verifier()), FrameVerifyResult::Accept);
        assert_eq!(auth.member_pubkeys.get("m2"), Some(&Some(nav.pubkey_base64.clone())));
    }
}
