# 12 -- Collaborative Terminal / Pair Programming

This chapter covers Jarvis's collaborative terminal -- an **experimental**
pair-programming feature that lets a host share one terminal with remote
participants over the relay, with `driver`/`navigator` roles, live ghost
cursors, and end-to-end signed frames.  It is described from both the
user-facing perspective and the implementation perspective, with particular
attention to the security model, which is the most load-bearing part of the
design.

> **Experimental and off by default.** The entire feature is gated on
> `collab.enabled`, which defaults to **`false`**.  With it unset, every pair
> IPC handler and the transport worker are no-ops.  The Rust code lives behind
> the `experimental-collab` feature flag of the `jarvis-social` crate.  Treat
> this chapter as documentation of a feature you must deliberately opt into.

The authoritative design and security specification is
`dev/plans/c2-pair-programming.md`.  This chapter reflects the implemented M3
state.

---

## Table of Contents

1. [What It Is](#what-it-is)
2. [Transport](#transport)
3. [Frame Protocol](#frame-protocol)
4. [Security Model](#security-model)
5. [Configuration Reference](#configuration-reference)
6. [Usage](#usage)
7. [Source Map](#source-map)

---

## What It Is

The collaborative terminal lets one user (the **host**) share a live shell
session with one or more remote **navigators** over the same relay used by the
mobile bridge.  Within a session:

- The **host** owns a real PTY.  Its terminal output is streamed to everyone;
  its keystrokes are handled locally.  The host is the single source of truth
  ("authority anchor") for the session.
- Exactly one participant holds the **driver** seat at any time.  The driver's
  keystrokes flow back to the host's PTY.  The host is the initial driver.
- Every other participant is a **navigator** (`view`-only).  Navigators see the
  shared terminal and can move their (ghost) cursor and **request control**, but
  their keystrokes are swallowed locally until they are granted the driver seat.

Roles map directly to the `PairRole` enum in
`jarvis-social/src/pair/types.rs` (`Driver` / `Navigator`); the host is a
`Driver` whose `user_id` also equals the session's `host_user_id`.  The panel
renders three role strings: `host`, `driver`, and `view`.

Each Jarvis instance may be in **one** pair session at a time.  A session ends
when the host leaves or the host's shared shell exits.

### Coordination vs. transport

The collaboration logic is split across two layers:

1. **`jarvis-social` (`pair/manager.rs`)** -- a transport-agnostic
   `PairManager` that owns session state (participants, the current driver,
   takeover policy).  It enforces the role rules: `create_session`,
   `join_session`, `set_driver` (driver handoff, with auto-handoff to the host
   when the driver leaves), and the **driver-gated** `relay_input` (input is
   accepted only from the current driver).  Every mutation emits a `PairEvent`.
2. **`jarvis-app` (`app_state/pair.rs`)** -- the worker that wires the
   `PairManager` to the relay Room: it drains `PairEvent`s, converts them to
   wire frames, signs + encrypts them, fans them out, and routes inbound frames
   back through verification into the local PTY / panel.

---

## Transport

Pair sessions ride the relay's symmetric **Room** -- the same N:N transport
described in the networking chapter ([09 -- Networking & Social](09-networking.md),
"Relay System and Mobile Bridge").  The mobile bridge uses the relay's 1:1
desktop/mobile session; pair programming uses the relay's Room, whose
all-but-sender fan-out is the correct primitive for a bidirectional,
many-participant session (a unidirectional broadcast would drop the driver's
keystrokes).

Pairing inherits the relay's properties:

- The relay is a thin, **opaque** message forwarder.  It never inspects payload
  content; pair frames are end-to-end encrypted between members.
- Members join a Room with `room_hello{session_id, member_id}` and receive
  `room_ready`, then `member_joined` / `member_left` / `member_count` presence
  fan-out for free.
- Every member's frame is forwarded to **all other** members (the sender never
  receives its own frame echoed back).

### Outer envelope

Pair frames reuse the unchanged `RelayEnvelope`
(`ws_server/relay_protocol.rs`: `KeyExchange | Encrypted | Plaintext`) and the
`RelayCipher` (AES-256-GCM with a 12-byte random nonce, base64 on the wire) --
the same envelope the mobile bridge uses.  The room key is the 32-byte AES key
derived from the `session_id` via the crypto service's `derive_room_key`
(PBKDF2; identical scheme to chat channels).  The key is handed to the room
client over a `tokio::sync::watch` channel; until it lands, outbound frames are
dropped.  The relay therefore sees only opaque ciphertext.

### Data flow

```
 HOST                                       NAVIGATOR
 ----                                       ---------
 PTY output
   -> pty_polling.rs tap (host pane only)
   -> pair_enqueue_output()
        -> PairSnapshotBuffer (ring, ~256 KB)   <- mid-join replay
        -> mirror to host's own panel
        -> PairFrame::PtyOutput{data, offset}
             -> sign -> encrypt -> Room  ------>  decrypt -> verify
                                                  -> pair_event{pty_output}
                                                  -> term.write(xterm.js)

                                                  driver keystroke
                                                  term.onData -> pair_input IPC
                                                  <- PairFrame::TermInput
   verify (signer == driver)                 <----  sign -> encrypt -> Room
   -> PairManager::relay_input (driver gate)
   -> ptys.write_input(host_pane)
```

The host's PTY output is tapped in `app_state/pty_polling.rs`, gated so the tap
fires only for the pane registered as `pair_host_pane_id`.  The tap uses a
non-blocking `try_send` and drops chunks on a full channel: the 60 Hz PTY hot
path must never stall on a lagging navigator.

Navigators render the stream with **xterm.js** (`assets/panels/pair/index.html`,
forked from the terminal panel).  PTY output is arbitrary bytes (escape
sequences, partial UTF-8), so `data` is base64 on the wire and is written only
via `term.write` -- **never** `innerHTML`.

### Mid-join snapshot replay

The relay does no replay, so a member joining mid-task would otherwise see a
blank terminal until the next output.  The host keeps a bounded
`PairSnapshotBuffer` ring of its recent raw PTY output
(`MAX_SNAPSHOT_BYTES = 256 KB`).  On each `member_joined`, the host sends that
member a `Snapshot` frame (`term.reset()` + `term.write(snapshot)` on the
joiner) so its terminal starts populated.

Because the relay has no per-member addressing, a snapshot would also reach
existing navigators and force a disruptive reset.  Two mechanisms avoid that:

- **`target`** -- the snapshot names the member it is for; everyone else
  ignores it.
- **`offset`** -- the host stamps every `PtyOutput` chunk with a monotonic
  cumulative byte offset.  The snapshot carries the offset it captured up to; the
  joiner then **dedups** by dropping any live `pty_output` whose `offset <=` the
  snapshot offset, so the overlap is not written twice.

### Ghost cursors

Host and driver report their viewport-relative cursor position
(`pair_cursor` IPC, throttled to ~20 Hz) as a `Cursor` frame.  Receivers render
each remote member as a colored **ghost caret** in an overlay layer
(`#ghost-layer`), with a deterministic per-member color and a name label drawn
via the CSS `attr()` function (never `innerHTML`).  Ghost carets are hidden while
the viewer is scrolled back into scrollback, where viewport-relative geometry no
longer lines up.  View-only navigators do not emit a cursor (their local caret
merely tracks the host's output).

---

## Frame Protocol

The inner application payload is the `PairFrame` enum
(`ws_server/pair_protocol.rs`, `#[serde(tag = "type", rename_all =
"snake_case", deny_unknown_fields)]`).  Byte payloads are base64.

| Frame             | Direction              | Purpose |
|-------------------|------------------------|---------|
| `pty_output`      | host -> room           | A chunk of the shared terminal stream (+ monotonic `offset`). |
| `term_input`      | driver -> room -> host | Driver keystrokes to write into the host PTY (`from`, `data`). |
| `cursor`          | any -> room            | Ghost-cursor position (`from`, `row`, `col`). |
| `resize`          | host -> room           | Shared terminal resized (`cols`, `rows`). |
| `driver_changed`  | host -> room           | Driver seat changed hands (`new_driver`, `old_driver`). |
| `request_control` | navigator -> host      | Ask for the driver seat (`from`). |
| `join`            | navigator -> host      | Announce presence + display name + identity `pubkey`. |
| `snapshot`        | host -> joiner         | Mid-session replay (`data`, `cols`, `rows`, `driver`, `target`, `offset`). |
| `session_meta`    | host -> room           | Host name, dimensions, takeover policy, full roster (re-broadcast on every join). |

**Authority model.** The host is the source of truth.  Only the host emits the
**host-only** frames (`pty_output` / `resize` / `driver_changed` / `snapshot` /
`session_meta`; see `PairFrame::is_host_only`).  Navigators emit only
`term_input` / `cursor` / `request_control` / `join`.  The host re-validates
every `term_input` through `PairManager::relay_input` (driver-gated) before
writing it to the PTY.

---

## Security Model

This is the most important section of the chapter.  Pair programming runs over a
**bearer-secret** Room: anyone who knows the `session_id` can connect to the
relay's room.  The room key (PBKDF2 of the `session_id`) is therefore
**confidentiality-only** -- it hides traffic from the relay but is shared by all
members, so it is *not* a boundary between members.  The real security boundary
is a **per-member ECDSA signature** on every frame.

### Signed frames

Every `PairFrame` is wrapped in a `SignedPairFrame`
(`ws_server/pair_protocol.rs`) *before* it is encrypted into the
`RelayEnvelope`.  The signed envelope carries:

```text
SignedPairFrame {
  member_id,   // sender's stable in-room id
  pubkey,      // sender's ECDSA identity public key (SPKI DER, base64)
  epoch,       // per-connection epoch (strictly greater on each reconnect)
  seq,         // per-sender monotonic counter within an epoch
  sig,         // base64 IEEE-P1363 ECDSA-P256 signature
  frame        // the inner PairFrame
}
```

The signature is taken over a **canonical, domain-separated** byte string
(`SignedPairFrame::canonical_signing_bytes`), delimited by `0x1F`:

```text
"jarvis-pair-sig-v1" | session_id | member_id | pubkey | frame_type | epoch | seq | frame_json
```

Each component is load-bearing:

- **`jarvis-pair-sig-v1`** is a fixed crypto domain tag.  The app's single ECDSA
  identity key is *also* used by the relay, pairing, and theme signers; the tag
  guarantees a pair signature can never be cross-presented to (or accepted by)
  one of those other signers.
- **`session_id`** binds a signature to its room (no cross-session replay).
- **`member_id` + `pubkey`** are bound in, so a frame cannot be relabeled to a
  different `member_id` (to dodge the per-member seq tracker) or have its claimed
  pubkey swapped.
- **`frame_type`** is a per-type separator (a signature over one type cannot be
  reinterpreted as another).
- **`epoch` + `seq`** are the anti-replay counters.
- **`frame_json`** is the exact `serde_json` serialization of the inner frame.
  `#[serde(deny_unknown_fields)]` on both `SignedPairFrame` and `PairFrame`
  ensures a peer cannot append or reorder fields and keep a valid signature.

Frames are signed **exactly once**, in the worker's single outbound seam
(`pair_room_client::sign_frame`).  Verification is purely client-side; the relay
stays opaque and unchanged.

### Inbound verification

Every inbound `SignedPairFrame` is checked by
`PairAuthState::verify_signed_frame` (`app_state/pair.rs`) **before**
`apply_pair_frame` honors the inner frame.  The checks, in order -- any failure
drops the frame:

1. **Identity binding (TOFU).** The first verified frame from a `member_id` pins
   `member_id -> pubkey` in the roster.  A later frame from that id carrying a
   different pubkey is dropped (`PubkeyMismatch` -- impersonation).  An empty
   pubkey is never pinned as an identity, so a legacy-empty member is
   unclaimable.
2. **Signature.** ECDSA-P256 verification against the carried pubkey.  Under
   `require_signed_join` (the default), an **unsigned** frame cannot be
   authenticated and is dropped **fail-closed** -- this is the check that makes
   `require_signed_join` real.
3. **Anti-replay (epoch + seq).** A frame is accepted iff its `(epoch, seq)` is
   strictly newer than the last accepted pair for that member: a higher epoch
   resets seq tracking (a genuine reconnect picks a strictly-greater epoch); the
   same epoch requires a strictly-greater seq.  A lower epoch or non-increasing
   seq is a replay (`StaleSeq`).  The epoch is seeded from the wall clock so it
   survives full process restarts; there is no downward-reset window (which would
   itself be a replay vector).
4. **Self-`from` binding.** A navigator frame's inner `from` must equal the
   verified `member_id` -- a member cannot sign a frame attributed to someone
   else (e.g. forging a keystroke "from" the driver).
5. **Host authority.** Host-only frames are honored only from the pinned host
   (see below).

The final **driver** check for `term_input` (verified signer == current
`driver_user_id`) is enforced in `apply_pair_frame` and re-validated by
`PairManager::relay_input`, so a non-driver's keystrokes are dropped even though
step 4 already bound `from` to the signer.

### Host identity pinned from the invite (the critical fix)

The host's identity is pinned **out of band**, before any frame is processed --
not "first `SessionMeta` wins" (which would let any member forge a
`SessionMeta` to claim the host slot):

- The **host** pins its own `(member_id, pubkey)` at `pair_start`.
- The **navigator** pins the host **pubkey from the invite**.  The shareable
  invite is `base64url(session_id ":" host_pubkey)`
  (`pair::make_invite` / `parse_invite`).

Thereafter, host-only frames are honored **only** when the verified signer's
pubkey matches the pinned host pubkey (the host's `member_id` is learned and
locked on the first such frame).  While no host is pinned, all host-only frames
**fail closed**.  A navigator that joins with a bare/manual session id (no host
pubkey in the invite) stays unpinned, and host-only frames fail closed -- the
safe default.

### `require_signed_join`

When `require_signed_join` is `true` (the default) the above is **enforced**:

- A relay `member_joined` does **not** register a member in the `PairManager`;
  registration waits for that member's *verified* signed `Join` frame.  An
  unsigned / un-joined relay peer never enters the roster (so it can never be a
  `set_driver` / `relay_input` candidate).
- A bare (unsigned) `PairFrame` on the wire is dropped fail-closed.
- Host-only and driver frames are gated as above.

When `require_signed_join` is `false`, the legacy permissive M1/M2 path is used
(unsigned frames accepted, self-asserted `from`).  A `warn!` is logged at
startup; this is for experiments only and the room key is not a security
boundary in that mode.

### Documented residual: relay member-slot DoS

There is **one** documented residual risk, called out in the `RESIDUAL` block
atop `apply_pair_frame` and in the spec.

The relay assigns a member's in-room **slot** from a **self-asserted**
`member_id` and reconnect-replaces it, and that relay layer is **unsigned**.  A
peer who knows the `session_id` can therefore claim or churn a `member_id` slot
-- disrupting or hijacking the **slot** for **denial only**.

It **cannot forge content**: every honored frame is end-to-end signed, so the
host identity (pinned from the invite pubkey) and every member identity are
non-forgeable.  An attacker can deny or disrupt, but never impersonate.

Fully closing the slot-DoS would require the **relay** to bind slots to
identities (a relay protocol change + redeploy).  The relay is intentionally
left unchanged for this slice.  Defense-in-depth limits the blast radius: the
feature defaults off, the `session_id` is a high-entropy capability secret never
logged at info (only a `first6…(len=N)` fingerprint at debug), the relay
enforces a global session cap, and `term_input` is length-capped
(`MAX_TERM_INPUT_BYTES = 4096`) on both send and host.

A possible future improvement is optional per-member ECDH, giving each member a
distinct key (confidentiality *between* members, not just from the relay).

---

## Configuration Reference

Source: `jarvis-rs/crates/jarvis-config/src/schema/collab.rs`

```toml
[collab]
enabled = false             # master toggle; when false all pair IPC + transport are no-ops
max_participants = 4        # max participants per session, including the host
allow_takeover = true       # may navigators request / take the driver seat
require_signed_join = true  # ENFORCED: every inbound frame must carry a valid E2E signature
```

| Field                | Default | Meaning |
|----------------------|---------|---------|
| `enabled`            | `false` | Master toggle.  The whole feature is off unless this is set. |
| `max_participants`   | `4`     | Maximum participants per session (host included).  Enforced by `PairManager::join_session`. |
| `allow_takeover`     | `true`  | Whether navigators may request / be granted the driver seat (`PairManager::set_driver`). |
| `require_signed_join`| `true`  | When true, every inbound pair frame must carry a valid ECDSA signature binding `member_id` to its identity pubkey; unsigned/unverifiable frames are dropped fail-closed.  When false, the legacy permissive (unsigned, self-asserted `from`) path is used -- experiments only. |

Pairing also requires a relay URL.  It reuses the same `[relay].url`
(networking chapter, "Relay Configuration"); with no relay URL, `start_pair` is
a no-op.

---

## Usage

### Opening the panel

Open the **Open Pair Programming** action from the command palette
(`Action::OpenPair`).  Jarvis opens a new split running the pair panel at
`jarvis://localhost/pair/index.html`.  The panel shows a setup overlay with two
choices: **Share my terminal** (host) or **Join** an invite code.

### Hosting (invite flow)

1. Click **Share my terminal**.  The panel loads the local identity and sends
   `pair_start`.
2. Rust spawns a real PTY behind the panel (the shared shell), creates the
   session in the `PairManager` (host = initial driver), pins the host identity,
   and brings up the room worker.
3. The panel shows the **invite code** -- `base64url(session_id:host_pubkey)` --
   in the top bar.  Click it to copy.  Share it over a trusted channel: it is a
   bearer secret that grants room access **and** pins your host identity.
4. The host's terminal output streams to navigators as they join; a roster of
   participant chips (with ghost-cursor colors) appears in the top bar.

### Joining

1. Paste the invite code into the **Join** field and submit.  The panel sends
   `pair_join`.
2. Rust parses the invite, pins the host pubkey **from the invite** (before any
   frame is processed), and connects the room worker as a `view`-only navigator.
   The navigator never calls `create_session`, so it is never a local authority.
3. The navigator sends a signed `Join` (with its display name and identity
   pubkey); the host registers it and re-broadcasts the roster.  A mid-join
   snapshot populates the navigator's terminal.

### Requesting and granting control

- A **view-only** navigator clicks **Request control**, sending
  `request_control`.  The host arbitrates via `set_driver`, which honors
  `allow_takeover`.  If granted, the host broadcasts `driver_changed` and the
  navigator's role becomes `driver`; its keystrokes now flow to the host PTY.
- The **host** may grant the seat directly from the participant dropdown
  (the **Grant** button), sending `pair_set_driver`.
- If the current driver leaves, the `PairManager` auto-hands control back to the
  host.

### Leaving / ending

- **Leave** ends the session for the local participant (`pair_leave`).  If the
  **host** leaves -- or the host's shared shell exits (tapped via `pty_exit`) --
  the session ends for everyone (`leave_session` on the host removes the session).

### Panel IPC surface

Panel-to-Rust kinds (allow-listed in `ipc_dispatch.rs`):
`pair_start`, `pair_join`, `pair_leave`, `pair_input`, `pair_request_control`,
`pair_set_driver`, `pair_cursor`, `pair_status`.

Rust-to-panel kinds: `pair_status` (full session snapshot) and `pair_event`
(tagged incremental events: `pty_output`, `snapshot`, `resize`, `session_meta`,
`driver_changed`, `member_joined` / `member_left`, `cursor`, `connected` /
`disconnected` / `error`, `session_started` / `session_ended`).

All participant names rendered in the panel use `textContent`, and PTY output
goes only to `term.write` -- never `innerHTML` (XSS prevention).

---

## Source Map

| Concern | Source |
|---------|--------|
| Roles, session state, events, `PairConfig` | `jarvis-rs/crates/jarvis-social/src/pair/types.rs` |
| Session manager (driver gate, takeover, handoff) | `jarvis-rs/crates/jarvis-social/src/pair/manager.rs` |
| App worker, verification, snapshot buffer, invite | `jarvis-rs/crates/jarvis-app/src/app_state/pair.rs` |
| Inner frame + signed-frame protocol | `jarvis-rs/crates/jarvis-app/src/app_state/ws_server/pair_protocol.rs` |
| Outbound Room client (sign + encrypt + reconnect) | `jarvis-rs/crates/jarvis-app/src/app_state/ws_server/pair_room_client.rs` |
| IPC handlers + panel forwarding | `jarvis-rs/crates/jarvis-app/src/app_state/webview_bridge/pair_handlers.rs` |
| Config schema | `jarvis-rs/crates/jarvis-config/src/schema/collab.rs` |
| Pair panel (xterm, ghost cursors, role gating) | `jarvis-rs/assets/panels/pair/index.html` |
| Design + security spec | `dev/plans/c2-pair-programming.md` |

See also [09 -- Networking & Social](09-networking.md) for the relay, the
`RelayEnvelope` / `RelayCipher`, and the crypto service this feature builds on.
