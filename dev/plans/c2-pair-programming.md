# C2 — Collaborative Terminal / Pair Programming — Implementation Spec

Status: **planned** (not yet implemented). Branch target: `revive/collab-ai`.
Produced 2026-06-04 from a read-only design pass. This is the executable blueprint
for the C2 milestone; hand it to an implementation agent.

## 0. Existing world (built vs missing)

- **Coordination logic exists; transport does not.** `crates/jarvis-social/src/pair/manager.rs`
  is a complete `PairManager` (create/join/leave, driver handoff `set_driver` ~:237,
  auto-handoff on driver-leave ~:170-198, `relay_input` driver-gated ~:307-323,
  `broadcast_output`, `update_cursor`, `resize`). Every mutation pushes a `PairEvent`
  into an mpsc channel from `PairManager::new` (~:24-35) that **nothing consumes**. That
  is the integration gap.
- **Feature-gated off.** `pair`/`screen_share`/`voice` are behind `experimental-collab`
  (`jarvis-social/src/lib.rs:8-32`, `jarvis-social/Cargo.toml:10`). `jarvis-app` depends
  on `jarvis-social` with **no features** (`jarvis-app/Cargo.toml:22`), so `PairManager`
  is not even compiled into the app today.
- **Serialization gap.** `PairEvent/PairSession/PairRole/PairParticipant` derive only
  Debug/Clone (`pair/types.rs:10,19,31,53`) — not Serialize/Deserialize.
- **Relay Room is ready** (committed `978be5e`). `jarvis-relay` has N:N `RoomSession`
  (`register_room`, `room_targets_excluding`, reconnect-replace, presence fan-out);
  `connection.rs` wires `room_hello{session_id,member_id}` → `room_ready` →
  `member_joined/left/count`, opaque all-but-sender fan-out, drops `{"type":"ping"}`.
- **Transport precedents to fork:** desktop relay client `ws_server/relay_client.rs`
  (outbound connect, hello, P-256 ECDH, AES-256-GCM `RelayCipher` in `crypto_bridge.rs`);
  chat `LiveStream` JS (`assets/panels/chat/index.html:1830-2058`).
- **Crypto is Rust-side, IPC-exposed** (`webview_bridge/crypto_handlers.rs`:
  init/derive_room_key/derive_shared_key/encrypt/decrypt/sign/verify).
- **PTY tap point:** `webview_bridge/pty_polling.rs:19-47` (`poll_pty_output`) already
  fans each PTY to the local webview + mobile broadcaster — add a third (pair) sink here.
- **Integration template = presence wiring:** `social.rs` (start/poll + dedicated tokio
  runtime + sync mpsc back to main), `presence_handlers.rs`, `ipc_dispatch.rs`
  ALLOWED_IPC_KINDS, `JarvisApp` fields in `core.rs`, poll in `polling.rs`, start in
  `event_handler.rs`.

## 1. Transport decision — relay **Room** (not Broadcast)

Pair is inherently bidirectional. Broadcast (`connection.rs` host→spectators) drops
spectator-originated frames, so a driver can't send keystrokes back. Room's all-but-sender
fan-out is the correct primitive and gives presence (`member_joined/left/count`) for free.

**Outer envelope:** reuse `RelayEnvelope` (`ws_server/relay_protocol.rs:42-56`:
`key_exchange|encrypted|plaintext`) unchanged — relay stays fully opaque.

**Inner payload — new `PairFrame` enum** (new `ws_server/pair_protocol.rs`,
`#[serde(tag="type", rename_all="snake_case")]`, bytes base64):
- `pty_output{data}` — host→room, the shared terminal stream
- `term_input{from,data}` — driver→room→host writes to PTY
- `cursor{from,row,col}` — any→room, ghost cursors
- `resize{cols,rows}` — host→room
- `driver_changed{new_driver,old_driver}` — host→room
- `request_control{from}` — navigator→host
- `snapshot{data,cols,rows,driver}` — host→joining member (mid-session replay)
- `session_meta{host,host_name,cols,rows,allow_takeover}` — host→room on create

**Authority model:** host is source of truth. Only host emits pty_output/resize/
driver_changed/snapshot/session_meta. Navigators emit only term_input/cursor/
request_control. Host re-validates every `term_input` via `relay_input` (driver-gated)
before writing to PTY.

**Keying:** M1 = room-derived symmetric AES key via `crypto` IPC `derive_room_key{session_id}`
(same as chat channels; session_id is the shared secret). M3 = pairwise P-256 ECDH
(`derive_shared_key`).

## 2. Per-file change list

- **Enable feature:** `jarvis-app/Cargo.toml:22` → `jarvis-social = { workspace = true, features = ["experimental-collab"] }`.
- **Serde derives:** `jarvis-social/src/pair/types.rs` add Serialize/Deserialize to the 4 types (transmit the leaner `PairFrame`, keep `PairEvent` internal).
- **New `app_state/pair.rs`** (model `social.rs`): `start_pair`/`poll_pair`, `PairInbound`/`PairCommand` enums, `Arc<PairManager>`, worker draining `event_rx`→`PairFrame`→encrypt→room and routing inbound frames back over a sync mpsc. Reuse `self.tokio_runtime` via `get_or_insert_with` (do NOT spawn a 2nd runtime).
- **`core.rs`** fields: `pair_manager: Option<Arc<PairManager>>`, `pair_inbound_rx`, `pair_cmd_tx`, `pair_session_id`, `pair_member_id`, `pair_host_pane_id`, `pair_key_tx` (watch). Init in `new()`.
- **Register/wire:** `mod pair;` in `app_state/mod.rs`; `self.poll_pair();` in `polling.rs:18`; `self.start_pair();` in `event_handler.rs:49`.
- **New transport:** `ws_server/pair_protocol.rs` (`PairFrame`) + `ws_server/pair_room_client.rs` (fork `relay_client.rs`: `room_hello` instead of `desktop_hello`; inner payload `PairFrame`; reuse `RelayEnvelope`+`RelayCipher`). Add `RoomHello`/`RoomReady`/`MemberJoined/Left/Count` to `relay_protocol.rs`. Register in `ws_server/mod.rs`.
- **PTY tap:** `pty_polling.rs:40` add a pair sink gated on `pair_host_pane_id == pane_id`, route via `pair_cmd_tx.try_send(PairCommand::Output(bytes))` (non-blocking). Tap `pty_exit` to end session on host shell death.
- **IPC + handlers:** `ipc_dispatch.rs` ALLOWED_IPC_KINDS += pair_start/pair_join/pair_leave/pair_request_control/pair_set_driver/pair_status/pair_cursor + arms + test. New `webview_bridge/pair_handlers.rs` (model presence_handlers + chat_stream_handlers). Register in `webview_bridge/mod.rs`. Reuse `sanitize_display_name`.
- **Crypto reuse:** no new module — M1 `derive_room_key(session_id)` + pass 32-byte key to worker via watch channel (pattern at `relay_polling.rs:33-41`). M3 `derive_shared_key`.
- **Config:** new `jarvis-config/src/schema/collab.rs` `CollabConfig{enabled=false, max_participants=4, allow_takeover=true, require_signed_join=true}`; register in `schema/mod.rs` + field on `JarvisConfig` + default test.

## 3. Shared-terminal panel

New `assets/panels/pair/index.html` (top-level panel, served `jarvis://localhost/pair/index.html`).
**Fork the *terminal* panel for xterm render; fork chat's Crypto/Identity IPC + LiveStream UI
(NOT its WebSocket — keep the relay socket in Rust).** Panel↔Rust is pure IPC
(`pair_start/pair_join/pair_input/pair_request_control/pair_cursor` up; `pair_event/pair_status` down).
- Host: renders its own PTY (tapped in Rust); panel just displays.
- Driver-navigator: `term.onData` → `pair_input` → host validates+writes.
- View-only navigator: keystrokes swallowed; show "Request control".
- Render inbound `pty_output` with `term.write`; on `snapshot` do `term.reset()`+write.
- Top bar: session id/invite code, role badge, participant list w/ ghost-cursor chips,
  Request control / Grant / Leave buttons.
- XSS: names via `textContent`; `pty_output` only to `term.write`, never innerHTML.

## 4. Milestones (smallest slice first)

- **M0 — compile+types (S):** enable feature; add serde derives; add `CollabConfig` (enabled=false).
- **M1 — read-only shared terminal (host streams PTY → navigators over Room):**
  T1.1 `PairFrame` (pty_output/resize/session_meta/snapshot) **M**;
  T1.2 `pair_room_client.rs` fork + room hello/responses + room-key keying **L**;
  T1.3 `pair.rs` start/poll + worker **M**;
  T1.4 core.rs fields + wiring **S**;
  T1.5 PTY tap **M**;
  T1.6 `pair_handlers.rs` (pair_start/pair_status/send_pair_event_to_panel) + IPC **M**;
  T1.7 `assets/panels/pair/index.html` xterm render + start/join **L**.
  Security: no-op unless `collab.enabled`; 32-char high-entropy session_id (capability URL).
- **M2 — driver input + takeover:** PairFrame += term_input/driver_changed/request_control;
  panel role-gates onData + Request control / Grant; handlers route term_input→`relay_input`
  (driver-gated)→`ptys.write_input`. Security: every term_input re-validated; takeover honored
  only if `allow_takeover`.
- **M3 — cursors, resize, mid-join replay, hardened auth:** cursor ghost overlay + resize;
  snapshot/replay on `member_joined` (host keeps ~256KB ring buffer); signed room join
  (`require_signed_join`: host verifies ECDSA sig over session_id via `crypto` sign/verify;
  optional per-member ECDH).

## 5. Top risks + mitigations

1. **Mid-join blank terminal** → M3 snapshot/replay (host ring buffer + `snapshot` on member_joined).
2. **No-auth room joins** (feature literally says "no auth yet") → layered: M1 high-entropy
   session_id (don't log at info), M2 driver gating, M3 `require_signed_join` ECDSA verify,
   default `enabled=false`, relay global session cap.
3. **PTY-tap perf** (60Hz hot path) → gate on `pair_host_pane_id.is_some() && pane_id==host`;
   `try_send` (drop on full); bounded relay channel drops for lagging navigators.
4. **Two tokio runtimes** → `start_pair` MUST reuse `self.tokio_runtime` via get_or_insert_with.
5. **PairEvent isn't ideal wire type** (redundant session_id + raw byte arrays) → transmit `PairFrame`.
6. **Driver/host-leave races** → drive all lifecycle through `PairManager`; on host `pty_exit`
   call `leave_session(host)` for a clean end.
7. **Relay treats payloads opaque** → host is trust anchor; re-validate term_input/request_control;
   ignore pty_output/driver_changed/snapshot claimed by non-host members.

## 6. Security status (M3 ENFORCED)

`collab.enabled` still defaults **false** and the feature remains
**experimental**, but the M3 hardening is now **implemented and enforced**.
Every pair frame is an **end-to-end ECDSA-signed `SignedPairFrame`** (per-app
identity key), verified client-side in
`app_state::pair::PairAuthState::verify_signed_frame` BEFORE any frame is
honored. With `require_signed_join` (the default) the room key is
**confidentiality-only** and the **signatures are the security boundary between
members**.

What is now ENFORCED:

- **Per-member authentication (TOFU).** First verified frame from a `member_id`
  pins `member_id → pubkey`; a later frame from that id with a different pubkey
  is dropped (`PubkeyMismatch`). The pin is `Option<String>` — an empty pubkey
  is never stored as an identity, so a legacy-empty member is unclaimable.
- **member_id + pubkey bound into the signature.** The canonical signing bytes
  now include the `member_id` and `pubkey` (plus a fixed crypto domain tag
  `jarvis-pair-sig-v1`, the `session_id`, `frame_type`, `epoch`, `seq`, and the
  inner frame JSON). A frame can no longer be relabeled to a different
  `member_id` to dodge the per-member seq tracker, and the pair signature can
  never be cross-presented to the relay/pairing/theme signers that share the
  identity key.
- **Driver-spoofing rejected.** A navigator frame's inner `from` is bound to the
  verified signer (`term_input` from a non-signer `from` is dropped); the
  `term_input` driver-gate is re-checked against the host's `PairManager`.
- **Host-authority bound to the INVITE capability (the critical fix).** The host
  identity is pinned **out of band** — the HOST pins its own `(member_id,
  pubkey)` at `pair_start`; the NAVIGATOR pins the host **pubkey from the
  invite** (the shareable invite is `base64url(session_id ":" host_pubkey)`)
  before any frame is processed. Host-only frames
  (`pty_output`/`resize`/`driver_changed`/`snapshot`/`session_meta`) are honored
  ONLY when the verified signer's pubkey matches the pinned host pubkey; while no
  host is pinned they **fail closed**. This closes the prior
  first-`SessionMeta`-wins race where any member could forge a `SessionMeta` to
  claim the host slot.
- **Anti-replay via a signed per-connection `epoch`.** Each (re)connect picks a
  strictly-greater `epoch`; recipients track per-member `(epoch, last_seq)` and
  accept iff `epoch > stored.epoch` (reset seq) OR `epoch == stored.epoch && seq
  > stored.last_seq`. A lower epoch or non-increasing seq is dropped. The old
  `RECONNECT_SEQ_RESET_WINDOW` downward-reset window (a replay vector) is
  **removed**, and the `seq==1` reconnect-drop edge is gone.
- **`deny_unknown_fields`** on `SignedPairFrame` and `PairFrame` so a peer can't
  append/reorder fields and keep a valid signature.
- **Single signing seam.** Outbound frames are signed exactly once in the
  worker (`pair_room_client::sign_frame`); the dead main-thread
  `sign_pair_frame` + `PairRoomCommand::SendSigned` + the duplicate
  `outbound_seq` were removed.

Other mitigations retained: `collab.enabled` defaults false (a `warn!` fires if
`require_signed_join` is off); the `session_id` is a capability secret never
logged at info (only a `first6…(len=N)` fingerprint at debug); `term_input` is
length-capped (`MAX_TERM_INPUT_BYTES = 4096`) on send and on the host.

### RESIDUAL (deferred — needs a relay change + redeploy)

**Relay member_id slot DoS.** The relay assigns the in-room slot from a
**self-asserted** `member_id` and **reconnect-replaces** it, and that layer is
unsigned. A peer who knows the `session_id` can therefore claim or churn a
member_id slot — disrupting or hijacking the SLOT for **DENIAL only**. It cannot
forge CONTENT: signed frames make the host identity (pinned from the invite
pubkey) and every member identity non-forgeable, so the attacker can deny/disrupt
but never impersonate. Fully closing this requires the **relay** to bind slots to
identities (a relay protocol change + redeploy). The relay (`jarvis-relay`) is
intentionally **left UNCHANGED** for this M3 slice. See the `RESIDUAL` note in
the `// SECURITY (M3 — ENFORCED)` block atop
`app_state::pair::apply_pair_frame`.

Possible future work: optional per-member ECDH so each member has a distinct key
rather than the shared room key (confidentiality between members, not just from
the relay).
