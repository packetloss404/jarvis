# 09 -- Networking, Social, and Communication

This chapter covers Jarvis's networked features: the WebSocket **relay** that
underpins everything, the live chat system, online presence, the mobile relay
bridge, the cryptographic identity layer, and the AI assistant panel.  Every
subsystem is described from both the user-facing perspective and the
implementation perspective so that contributors can navigate the codebase
confidently.

A single piece of infrastructure ties the networked features together: the
**`jarvis-relay`** WebSocket server.  Chat, presence, the mobile bridge,
screen-share broadcast, and pair programming all speak to it.  There is **no
Supabase** in Jarvis any more -- the relay's symmetric **Room** sessions
replaced Supabase Realtime for chat and presence.

---

## Table of Contents

1. [Relay Server (`jarvis-relay`)](#relay-server-jarvis-relay)
2. [Live Chat System](#live-chat-system)
3. [Presence System](#presence-system)
4. [Mobile Relay Bridge](#mobile-relay-bridge)
5. [Mobile Device Pairing](#mobile-device-pairing)
6. [Crypto Service and Identity](#crypto-service-and-identity)
7. [AI Assistant Panel](#ai-assistant-panel)
8. [Chat Panel Features](#chat-panel-features)
9. [Workspace Streaming](#workspace-streaming)
10. [Retro Game Emulator](#retro-game-emulator)
11. [Network Configuration Reference](#network-configuration-reference)
12. [Security Model](#security-model)

---

## Relay Server (`jarvis-relay`)

### Overview

Source: `jarvis-rs/crates/jarvis-relay/src/`

`jarvis-relay` is a standalone Tokio WebSocket server (binary crate).  It is a
thin message forwarder: it parses **only the first frame** of each connection
(the *hello*), registers the client in a session, and from then on forwards
**opaque text frames** between peers without ever inspecting payload content.
All application-level encryption is end-to-end between the endpoints; the relay
sees only ciphertext (for the bridge) or already-built application frames (for
chat/presence).

### Three Session Kinds

The relay multiplexes three different topologies onto one server, chosen by the
client's hello message (`session.rs`, `Role::session_kind`):

| Kind        | Roles                | Topology | Used by |
|-------------|----------------------|----------|---------|
| **Bridge**  | `Desktop`, `Mobile`  | 1:1      | Mobile relay bridge (desktop terminal on a phone) |
| **Broadcast** | `Host`, `Spectator` | 1:N      | Host-to-spectators screen-share |
| **Room**    | `Member`             | N:N (symmetric) | Chat, presence, pair programming |

A session is identified by its `session_id`.  Within a session, every client
gets an `mpsc::Sender<String>` handle in the `SessionStore`; forwarding is just
pushing the incoming text frame onto the right peer senders.

### Hello Protocol (Client to Relay)

The first message identifies the client's role.  Defined in `protocol.rs`
(`RelayHello`):

```json
// Bridge
{"type": "desktop_hello",   "session_id": "abc123..."}
{"type": "mobile_hello",    "session_id": "abc123..."}

// Broadcast
{"type": "host_hello",      "session_id": "abc123..."}
{"type": "spectator_hello", "session_id": "abc123..."}

// Room — SIGNED (note the extra member_id + signed credential)
{
  "type": "room_hello",
  "session_id": "abc123...",
  "member_id": "m1",
  "pubkey": "<base64 ECDSA P-256 SPKI DER>",
  "nonce": 1700000000000,
  "sig": "<base64 IEEE-P1363 ECDSA-P256 signature>"
}
```

If no valid hello arrives within **10 seconds**, the connection is dropped.
Session IDs are validated against `max_session_id_len` (default 64 bytes) and
must be non-empty.  Room `member_id`s are validated the same way: non-empty, at
most `max_session_id_len` bytes, and restricted to a conservative charset
(alphanumeric plus `-`, `_`, `.`).

> **Room hellos must now be SIGNED.**  Unlike the four single-field hellos,
> `room_hello` carries a cryptographic credential — `pubkey`, `nonce`, and
> `sig` — that the relay verifies *before* the member is admitted.  The old
> unsigned `{"type":"room_hello","session_id":..,"member_id":..}` form is
> **rejected** (a breaking cutover; all four Room clients sign).  See
> [Signed `room_hello` (Room Slot Binding)](#signed-room_hello-room-slot-binding)
> for the full signing scheme and the relay-side checks.

### Control Responses (Relay to Client)

Defined in `protocol.rs` (`RelayResponse`):

| `type`               | Session kind | Meaning |
|----------------------|--------------|---------|
| `session_ready`      | Bridge/Broadcast | Registered; carries `session_id` |
| `peer_connected`     | Bridge       | The other side joined |
| `peer_disconnected`  | Bridge       | The other side left |
| `host_connected`     | Broadcast    | Host came online |
| `host_disconnected`  | Broadcast    | Host left |
| `viewer_count`       | Broadcast    | Spectator count changed |
| `room_ready`         | Room         | Member registered; carries `session_id` |
| `member_joined`      | Room         | A member joined; carries `member_id` |
| `member_left`        | Room         | A member left; carries `member_id` |
| `member_count`       | Room         | Current member count |
| `error`              | any          | Fatal / validation error |

### Bridge Sessions (1:1)

Desktop clients **create** a bridge session; mobile clients **join** an
existing one.  The desktop is sent `session_ready`; when the mobile joins, both
sides receive `peer_connected`.  After that, each text frame from one side is
forwarded verbatim to the other.  A desktop reconnecting with the same
`session_id` replaces the old desktop registration.  See
[Mobile Relay Bridge](#mobile-relay-bridge).

### Broadcast Sessions (1:N)

A `host_hello` auto-creates the session; `spectator_hello` clients join it.
The host's frames fan out to all spectators; the relay also emits `viewer_count`
to everyone as spectators come and go, and `host_connected` /
`host_disconnected` to spectators.  This backs host-to-spectator screen-share
(covered with the collaboration features).

### Room Sessions (N:N, symmetric)

Rooms are the newest and most general session kind.  They back relay-hosted
**chat**, **presence**, and **pair programming**.

- The **first** `room_hello` for a `session_id` **auto-creates** the room
  (subject to the global session cap); subsequent members join it.  Each
  participant is identified by its `member_id`.
- On join the member receives `room_ready`, then **one `member_joined` per
  member already present** (its initial roster), then a `member_count`.  Every
  existing member receives a `member_joined { member_id }` for the newcomer.
- After registration, an **opaque text frame** from a member is forwarded to
  **every other member** (all-but-sender fan-out) -- a member never receives
  its own frames.
- Reconnecting with the **same `member_id` replaces** that member's channel
  rather than adding a duplicate.
- On disconnect the relay fans out `member_left { member_id }` and an updated
  `member_count` to the remaining members.  When the **last** member leaves,
  the room is removed.
- `{"type":"ping"}` keepalive frames are **dropped** (not forwarded), exactly
  as on bridge links.

Because a pair-programming room's `session_id` is itself a capability secret,
the relay logs only a truncated, non-reversible fingerprint of that id (first
6 chars + length) for `Member` connections.

### Signed `room_hello` (Room Slot Binding)

Source: `jarvis-rs/crates/jarvis-relay/src/protocol.rs`,
`jarvis-rs/crates/jarvis-relay/src/room_auth.rs`,
`jarvis-rs/crates/jarvis-relay/src/connection.rs`

A Room slot is identified by a self-asserted `member_id`.  Without
authentication, anyone could send a `room_hello` claiming any `member_id` and
either squat a not-yet-taken slot or evict the live connection holding it (a
member-id slot DoS).  To close this, **every Room hello is signed** with the
client's ECDSA P-256 identity key, and the relay verifies and *pins* the
signature to the slot.  This is a **security boundary**: a self-asserted
`member_id` can no longer squat or evict a slot without the matching private
key.

#### What the client signs

The client builds a canonical, deterministic byte string
(`room_hello_canonical_bytes`):

```text
"jarvis-room-hello-v1" 0x1F session_id 0x1F member_id 0x1F pubkey 0x1F nonce(decimal)
```

- `"jarvis-room-hello-v1"` — a fixed crypto **domain separator**, so a
  room-hello signature can never be cross-presented to another signer that
  shares the same ECDSA identity (pair frames use the disjoint
  `jarvis-pair-sig-v1`).
- `0x1F` — the ASCII Unit Separator delimiting the fields (disjoint from
  base64 / hostnames / member-id charsets, so the fields are unambiguous).
- `session_id` — binds the signature to **this** room, so a captured hello
  can't be replayed into a different session.
- `member_id` **and** `pubkey` — bound together, so the slot the signature
  claims is cryptographically tied to the signing key.
- `nonce` — a freshness value: **unix-epoch MILLISECONDS** as a decimal `u64`.

The string actually fed to ECDSA sign/verify is **`base64(canonical_bytes)`**
(`signed_hello_payload`).  Encoding the canonical bytes as base64 first lets the
signature round-trip losslessly through the project's `&str`-based ECDSA
sign/verify surface (`CryptoService::{sign,verify}` on desktop, Web Crypto on
mobile), matching the `SignedPairFrame` precedent.  The wire `sig` is the
base64 IEEE-P1363 (r‖s, 64-byte) ECDSA-P256 signature over that payload, and
`pubkey` is the signer's ECDSA SPKI-DER identity key, base64.

A fixed golden vector pins the encoding across all clients and the relay:
`signed_hello_payload("sid","m1","pk",42)` ==
`"amFydmlzLXJvb20taGVsbG8tdjEfc2lkH20xH3BrHzQy"`.  The Rust relay test, the
desktop chat JS, and the mobile chat JS all assert this same constant, so any
future separator/domain drift fails the build on at least one side.

#### How the relay verifies (before admission)

When a `room_hello` arrives, the connection handler (Room arm of
`connection.rs`) calls `RoomAuthStore::verify` **before** `ensure_session` can
even auto-create the room.  `verify` is a pure read and performs, in order:

1. **Nonce sanity freshness.**  The `nonce` must be within **±30 s**
   (`NONCE_WINDOW_MS`) of the relay clock, in either direction.  This is only a
   sanity bound — the real anti-replay guarantee is the monotonic check below.
2. **Signature.**  The ECDSA-P256 signature must verify against the carried
   `pubkey` over `signed_hello_payload(...)`.  (Mirrors
   `CryptoService::verify` exactly: same curve, SPKI pubkey encoding, P1363
   signature encoding, SHA-256 prehash.)
3. **Fingerprint-prefix check (chat-style ids only).**  If `member_id` has the
   form `<fp>.<userId>` where `<fp>` is a 16-char lowercase-hex pubkey
   fingerprint (the chat clients' id format), the leading `<fp>` segment must
   equal `fingerprint(carried_pubkey)` (first 8 bytes of `SHA-256(SPKI DER)`,
   lowercase hex, no `:` separators).  This closes the *first-mover squat* on
   those ids — an attacker can't be the first to pin a fingerprinted id without
   holding the key whose fingerprint the id embeds.
4. **TOFU pin + monotonic nonce.**  If `(session_id, member_id)` is already
   pinned, the carried `pubkey` must equal the pinned key (else
   `PubkeyMismatch`), and the `nonce` must be **strictly greater** than the
   last nonce accepted for that slot (else `StaleNonce`).

A rejected hello is answered with a coarse `error` response (`"invalid room
hello signature"` or `"member id bound to a different identity"`) and the
connection is dropped.  An **unsigned** `room_hello` (no credential at all) is
rejected outright with `"signed room hello required"`.

#### Verify / commit split (no orphan pins)

The pin and the nonce high-water are mutated only by `RoomAuthStore::commit`,
which the handler calls **only after the slot is actually registered**
(`register_room` succeeded).  `commit` re-checks the same pubkey + monotonic
nonce invariants under the write lock, so two concurrent valid hellos for the
same slot can't race a replay through; if the commit loses that race the
just-registered slot is torn back down.  Any early return after `verify`
(capacity, `ensure_session`, register, or the first `send` failing) therefore
leaves **no** pin and **no** advanced nonce behind.  When a room empties, its
pins are dropped (`forget_session`), so the same slot can later be re-pinned by
a fresh signed join.

#### Binding model: TOFU pinning (not self-authenticating ids)

The four Room clients derive `member_id` heterogeneously and none of them is a
pure function of the pubkey:

| Client | `member_id` |
|--------|-------------|
| Desktop **pair** | `random_alnum(16)` |
| Desktop **presence** | `identity.user_id` (a UUIDv4) |
| Desktop **chat** (JS) | `fingerprint(pubkey).hex + "." + userId` |
| Mobile **chat** (JS) | `fingerprint(pubkey).hex + "." + userId` |

Rather than force every id to be `fingerprint(pubkey)` (which would sever the
`member_id ↔ user_id` linkage that presence rosters, DM channel names, and the
pair-frame `from` fields all depend on), the relay binds the slot to the key
via **first-signed-join-pins** TOFU.  The fingerprinted chat ids additionally
get the prefix check above; the non-fingerprinted formats (pair `random_alnum`,
presence raw UUID) carry no key relationship and so retain a residual
first-mover-pins property — the first valid signer to present such an id pins
it, and any later differently-keyed claimant is refused.

#### Client signers

- Desktop presence (and the shared seam):
  `jarvis-rs/crates/jarvis-social/src/room/signed_hello.rs`
  (`RoomHelloSigner` / `build_signed_room_hello`).  `jarvis-social` has no
  crypto stack of its own; `jarvis-app` injects the ECDSA pubkey + a
  `sign(payload)` closure backed by `CryptoService`.
- Desktop chat: the `RoomHelloSig` builder in
  `jarvis-rs/assets/panels/chat/index.html` (signs via the Rust `crypto` IPC).
- Mobile chat: the `RoomHelloSig` builder in
  `jarvis-mobile/lib/jarvis-chat-html.ts` (signs via Web Crypto).

All three reproduce the canonical bytes / base64 payload byte-for-byte so a
signature any of them produces verifies at the relay.

### Session Store and Reaping

`SessionStore` (`session.rs`) maps session IDs to a `Session` enum
(`Bridge` / `Broadcast` / `Room`).  A background reaper runs every 60 seconds
and removes sessions older than `session_ttl` (default 300 s) that have no
live peers (no mobile for bridge, no host/spectators for broadcast, no members
for room).

### Rate Limiting

`RateLimiter` (`rate_limit.rs`) enforces:

| Parameter                  | Default | Description |
|----------------------------|---------|-------------|
| `max_connections_per_ip`   | 10      | Concurrent WebSocket connections per IP |
| `max_connect_rate_per_ip`  | 20      | New connections per IP per rate window |
| `rate_window_secs`         | 60      | Sliding window for rate counting |
| `max_total_sessions`       | 1000    | Global session cap |
| `max_session_id_len`       | 64      | Maximum session ID length in bytes |

Rate-limited connections are rejected before the WebSocket handshake completes.
The global session cap is checked when a hello would **create** a new session
(desktop bridge, or first room member); joining an existing session never trips
it.

### CLI Arguments and `$PORT`

Source: `jarvis-rs/crates/jarvis-relay/src/main.rs`

```
jarvis-relay [OPTIONS]

Options:
  -p, --port <PORT>                Port to listen on [default: 8080]
      --session-ttl <SECONDS>      Max stale session age [default: 300]
      --max-connections-per-ip <N> Per-IP concurrent limit [default: 10]
      --max-sessions <N>           Global session cap [default: 1000]
```

The relay binds `0.0.0.0:<port>`.  If the environment variable **`$PORT`** is
set (Railway, Cloud Run, Heroku, etc.) it **overrides** the `--port` default,
so the same binary deploys anywhere with no flags.

### Deployment

The relay is a single self-contained binary that can be deployed anywhere.  A
container build and a Railway configuration are included in the repo root:

- **`relay/Dockerfile`** -- a multi-stage build.  Stage 1 (`rust:1.83-slim`)
  compiles `jarvis-relay` from the workspace (it first stubs every crate's
  source to pre-cache dependencies, then builds the real relay).  Stage 2
  (`debian:bookworm-slim`) copies just the `jarvis-relay` binary, sets
  `ENV PORT=8080`, exposes `8080`, and runs `jarvis-relay --port 8080`
  (the binary still honors `$PORT` if the platform overrides it).
- **`railway.json`** -- uses the `DOCKERFILE` builder pointing at
  `relay/Dockerfile`, with `restartPolicyType: ON_FAILURE` and up to 10
  retries.

The **default relay URL** shipped in the desktop config points at the Railway
deployment (see [Relay Configuration](#relay-configuration)).

### Wire Conformance

The relay's control messages are pinned by shared JSON fixtures under
`jarvis-rs/testdata/relay/` (e.g. `session_ready.json`, `room_ready.json`).
Both `jarvis-relay` and the desktop client (`jarvis-app`) test against the same
files so the two sides cannot drift.

---

## Live Chat System

### Overview

Jarvis ships a multi-channel live chat panel that lets users communicate in
real time.  The chat is backed by the **relay's Room sessions** -- there is no
Supabase and no third-party SDK any more.  The panel runs inside a WebView
loaded from `jarvis://localhost/chat/index.html`.

Each chat channel (and each DM) maps to **one relay Room** whose `session_id`
is the channel ID.  The relay fans out any opaque application frame a member
sends to all *other* members of that Room.  Messages are end-to-end protected
by the same crypto as before (ECDSA signatures on every frame; AES-256-GCM for
DMs).

### Architecture

```
 Chat WebView (HTML/JS)
   |
   |-- RoomConnection (one per channel/DM)
   |      |
   |      +-- WebSocket --> jarvis-relay (Room session, session_id == channelId)
   |
   |-- jarvis.ipc.request('crypto', ...)         (Rust-side CryptoService)
   |-- jarvis.ipc.request('chat_stream_control', {action:'status'})
   |        (fetches the relay URL from Rust)
   |
   +-- AutoMod  (client-side moderation, JS)
```

The chat panel connects **directly** from the WebView to the relay; the Rust
backend does not proxy chat traffic.  Crypto operations (encryption, signing,
key derivation) are delegated to the Rust `CryptoService` through IPC so that
WebView JavaScript never handles raw key material.  The relay URL itself is
obtained from Rust via the `chat_stream_control` `status` IPC.

### `RoomConnection` (Transport)

Source: `jarvis-rs/assets/panels/chat/index.html`, `RoomConnection`

`RoomConnection` is a small WebSocket wrapper that speaks the relay's Room
protocol.  Per channel it:

1. Opens a WebSocket and, on open, builds and sends a **signed** `room_hello`
   via the `RoomHelloSig` builder — `{"type":"room_hello", "session_id":
   channelId, "member_id": <fingerprint.userId>, "pubkey": ..., "nonce": ...,
   "sig": ...}`.  Signing is async (it calls `Identity.sign`, which on desktop
   is the Rust `crypto` IPC and on mobile is Web Crypto), so the hello is built
   in a Promise and only sent if the socket is still current.  The relay
   rejects an unsigned or forged hello — see
   [Signed `room_hello`](#signed-room_hello-room-slot-binding).
2. Waits for `room_ready`, then handles `member_joined` / `member_left` /
   `member_count` / `error` control frames (distinguished by their `type`
   field).
3. Surfaces every other frame -- the **application frames** tagged with a `k`
   field (`k:"message"`, `k:"reaction"`, or `k:"presence"`) -- to the chat app
   via an `onAppFrame` handler.
4. Sends a `{"type":"ping"}` keepalive every 25 s (which the relay drops).
5. Reconnects with exponential backoff (base 2 s, max 30 s, 8 attempts, 75-125%
   jitter); after the cap it prompts the user to reload.

### Channels

Seven channels are pre-configured (`CONFIG.CHANNELS`):

| Room session ID (`session_id`) | Display Name |
|--------------------------------|-------------|
| `jarvis-livechat`              | `# general` |
| `jarvis-livechat-discord`      | `# discord` |
| `jarvis-livechat-showoff`      | `# showoff` |
| `jarvis-livechat-help`         | `# help`    |
| `jarvis-livechat-random`       | `# random`  |
| `jarvis-livechat-games`        | `# games`   |
| `jarvis-livechat-memes`        | `# memes`   |

The `general` channel (`jarvis-livechat`, the default) is the **primary** Room:
it carries chat presence (the in-channel online roster).  A separate
`RoomConnection` is opened for **every** channel at startup, so messages
arriving on background channels are buffered and unread counts are tracked, and
channel switching is instant.
Source: `jarvis-rs/assets/panels/chat/index.html`, `CONFIG.CHANNELS`.

### In-Channel Presence Roster

The chat panel maintains its own per-channel online roster on the **primary**
Room, independent of the global presence system:

- On `room_ready` (and on each `member_joined`) the panel broadcasts a
  `{"k":"presence", ...}` frame announcing its `memberId`, `userId`, `nick`,
  ECDSA `pubkey`, `fingerprint`, ECDH `dhPubkey`, and `online_at`.
- Received presence frames populate a `memberId -> {nick, fingerprint, pubkey,
  dhPubkey, userId}` map, which drives the user-count display and the
  online-users dropdown (the source of the DH key used to start a DM).
- `member_left` drops the member from the roster; `member_count` refreshes the
  count.

### Chat History

`ChatHistory` (`jarvis-rs/crates/jarvis-social/src/chat.rs`) is a bounded
in-memory ring buffer keyed by channel name.  Configuration:

| Parameter                  | Default |
|----------------------------|---------|
| `max_messages_per_channel` | 500     |

When the buffer is full, the oldest message is evicted on push.  The WebView
maintains its own parallel buffer (also capped at 500 messages per channel)
and its DOM is capped at 600 nodes to prevent unbounded growth.

### Message Flow (Send)

1. User types a message and presses Enter.
2. Sender-side **AutoMod** checks run: keyword filter, spam detection, rate
   limit.
3. Emoji shortcodes (`:smile:`, `:fire:`, etc.) are replaced with Unicode
   emoji.
4. The message is signed with the user's ECDSA P-256 key via
   `Identity.sign()` (IPC to Rust).  The signature is computed over the
   canonical string `id|userId|nick|ts|text`.
5. The application frame is sent on the active channel's `RoomConnection` via
   `sendFrame()`.  The relay fans it out to all other members of that Room:
   ```json
   {
     "k": "message",
     "id": "<uuid>",
     "userId": "<uuid>",
     "nick": "alice",
     "ts": 1700000000000,
     "text": "hello world",
     "sig": "<base64 ECDSA signature>",
     "pubkey": "<base64 SPKI DER>",
     "fingerprint": "aa:bb:cc:dd:ee:ff:00:11"
   }
   ```
6. There is no hard per-frame limit on the relay, but the panel rejects frames
   larger than ~240 KB (oversized images) before sending.
7. The message is stored in the local channel history and rendered
   immediately with `verifyStatus: 'self'`.

Channel messages carry the message text in the `text` field and are
authenticated by the ECDSA signature.  (DMs differ -- see below -- carrying
AES-GCM `iv`/`ct` instead.)

### Message Flow (Receive)

1. An application frame (`k:"message"`) arrives via the channel's
   `RoomConnection` `onAppFrame` handler.  (The relay never echoes a member's
   own frames back, so self-echo cannot occur; the handler also drops any frame
   whose `userId` equals the local user as a belt-and-braces check.)
2. AutoMod filters run: keyword check on text and nickname, spam check
   (repeated character ratio, repeated identical messages), and per-user
   rate limiting.
3. ECDSA signature verification is performed via `Identity.verify()` (IPC)
   over the same canonical string `id|userId|nick|ts|text`.
4. The TOFU Trust Store checks whether the sender's fingerprint matches
   previously-seen identity for that nickname.
5. A verification badge is attached:
   - Checkmark (verified): signature valid, TOFU trusted or new.
   - Warning (key-changed): signature valid but fingerprint differs from
     previously recorded -- possible impersonation.
   - Cross (invalid): signature verification failed.
   - Question mark (unverified): no signature present.
6. The message is rendered and stored in the channel history.

### Image Messages

Users can paste images (Ctrl+V) or drag-and-drop image files into the chat
input.  Images are compressed via an HTML canvas to JPEG at 0.5 quality,
max width 300px, producing a `data:image/jpeg;base64,...` data URL.

Limits:

| Parameter         | Value     |
|-------------------|-----------|
| `MAX_IMAGE_LEN`   | 150,000 chars (~100 KB base64) |
| `IMAGE_MAX_WIDTH` | 300 px    |
| `IMAGE_QUALITY`   | 0.5       |

Images are sent as the `text` field of a normal message payload and rendered
as `<img>` elements with a click-to-lightbox viewer.

### Reactions

Messages support emoji reactions.  Reactions are sent as a separate frame
(`k:"reaction"`) on the same Room, with `{ msgId, emoji, userId, action }`.
A picker of 16 emoji is shown on hover.  Reactions are tracked per message
in the local history and persisted across channel switches.

### Direct Messages (DMs)

Users can open an end-to-end encrypted DM from the online users dropdown.
The flow:

1. User clicks a peer's "DM" button in the dropdown.
2. An ECDH shared key is derived: `Identity.deriveSharedKey(otherDhPubkey)`,
   using the peer's `dhPubkey` learned from its presence frame.
3. A deterministic DM channel name is computed from both fingerprints
   (sorted, concatenated with `jarvis-dm-` prefix).
4. A new `RoomConnection` is opened for the DM (its `session_id` is that
   deterministic channel name), so both peers meet in the same one-frame-fanned
   Room.
5. Outbound DM messages are encrypted with AES-256-GCM using the ECDH-derived
   key via `Crypto.encrypt()`.  The `k:"message"` frame carries `iv` and `ct`
   (base64) instead of `text`.
6. Inbound DM messages are decrypted via `Crypto.decrypt()`.
7. Signatures are computed over `id|userId|nick|ts|iv|ct` (the ciphertext
   components, not plaintext) to prevent chosen-plaintext attacks on the
   signature oracle.

The relay carries the DM ciphertext opaquely, exactly like any other Room
frame -- it cannot read DM contents.

### Reconnection

`RoomConnection` reconnects each channel independently with exponential
backoff:

| Parameter       | Value  |
|-----------------|--------|
| Base delay      | 2 s    |
| Max delay       | 30 s   |
| Max attempts    | 8      |
| Jitter          | 75-125% |

After max attempts on the primary channel, the user is prompted to reload.

---

## Presence System

### Overview

The presence system tracks which Jarvis users are online, their current
status, and activity.  It is implemented across two layers:

1. **`jarvis-social` crate** -- Rust-side presence client and event
   translator.
2. **Presence panel WebView** -- lightweight HTML panel that displays
   the user list and accepts poke interactions.

### Rust-Side Presence Client

Source: `jarvis-rs/crates/jarvis-social/src/presence/` and
`jarvis-rs/crates/jarvis-social/src/room/`

Presence now rides the relay's **Room** transport (the old Supabase/Phoenix
`realtime` client was deleted).  Every desktop joins a single, well-known
**global presence Room** named `jarvis-presence-global`
(`DEFAULT_PRESENCE_ROOM_ID`).

The transport itself is `room::RoomClient` -- a generic, presence-agnostic
WebSocket client (mirroring the desktop bridge's `relay_client.rs`):

1. Connects to the relay (`RoomConfig.relay_url`, taken from `[relay].url`),
   sends a **signed** `room_hello` (built by `signed_hello.rs`'s
   `build_signed_room_hello` from an injected `RoomHelloSigner`) for
   `session_id: "jarvis-presence-global", member_id: <user_id>`, and waits for
   `room_ready`.  The presence `member_id` is the desktop's stable `user_id` (a
   UUID), left untouched by the TOFU binding; the relay still requires the
   signature.  See [Signed `room_hello`](#signed-room_hello-room-slot-binding).
2. Surfaces relay control frames (`member_joined` / `member_left` /
   `member_count`) and every opaque text frame as `RoomEvent`s, and accepts
   opaque text frames to broadcast via `RoomClient::send`.
3. Auto-reconnects with exponential backoff (1 s base, 30 s max, 15 s connect
   timeout).  If `relay_url` is empty the client is a graceful no-op (presence
   disabled).

`PresenceClient` wraps `RoomClient` and maps presence semantics onto opaque
Room frames (`PresenceFrame`, serialized as JSON).  Its public surface
(`start` / `update_activity` / `send_invite` / `send_poke` / `send_chat` /
`online_users` / `disconnect`) is unchanged from the old Supabase client, so
the app layer needed no changes.  A background `event_translator` task converts
`RoomEvent`s into high-level `PresenceEvent`s and maintains the online-user
roster:

- On `Ready` and on each `member_joined`, the client (re-)announces its own
  `presence` frame so peers learn its roster entry.
- `member_left` removes the member (keyed by `user_id == member_id`) and emits
  `UserOffline`.
- Incoming `presence` / `activity_update` frames insert/update roster entries
  and emit `UserOnline` / `ActivityChanged`; `poke` frames are surfaced only
  when targeted at this user; `game_invite` and `chat_message` frames are
  surfaced as their respective events.  Frames whose `user_id` is our own are
  ignored.

### Presence Frame Types

Presence application frames are tagged with a `type` field
(`jarvis-social/src/protocol.rs`, `PresenceFrame`):

| `type`             | Payload |
|--------------------|---------|
| `presence`         | `OnlineUser { user_id, display_name, status, activity }` |
| `activity_update`  | `ActivityUpdatePayload` |
| `poke`             | `PokePayload { user_id, display_name, target_user_id }` |
| `game_invite`      | `GameInvitePayload` |
| `chat_message`     | `ChatMessagePayload` |

These are distinct from the relay's own `member_joined` / `member_left` /
`member_count` control frames, which the relay generates itself.

### User Status Values

```rust
pub enum UserStatus {
    Online,       // Default
    Idle,
    InGame,
    InSkill,
    DoNotDisturb,
    Away,
}
```

### Presence Events

| Event              | Description |
|--------------------|-------------|
| `Connected`        | Room joined (`room_ready`); includes initial `online_count`. |
| `Disconnected`     | Connection lost; user list cleared. |
| `UserOnline`       | Another user joined / first presence frame seen. |
| `UserOffline`      | Another user left (`member_left`). |
| `ActivityChanged`  | A user's status or activity text changed. |
| `GameInvite`       | A user broadcast a game invitation. |
| `Poked`            | Someone poked this user (targeted by `target_user_id`). |
| `ChatMessage`      | A chat message arrived on the presence Room. |
| `Error`            | Connection or protocol error. |

### App Integration

Source: `jarvis-rs/crates/jarvis-app/src/app_state/social.rs`

The `JarvisApp` struct runs the presence client on a dedicated Tokio runtime
(1 worker thread).  A `std::sync::mpsc` channel bridges async events to the
synchronous main thread.  `poll_presence()` drains events on every frame:

- Updates the in-memory `online_users` list.
- Sends updates to presence panel webviews via `evaluate_script`.
- Generates desktop notifications for pokes.
- Dispatches `PresenceCommand`s (poke, activity update) to the async task.

### Identity

Source: `jarvis-rs/crates/jarvis-social/src/identity.rs`

Each Jarvis instance generates a UUID-based identity on startup.  The
`display_name` defaults to the OS `USER`/`USERNAME` environment variable.  The
`user_id` doubles as the presence Room's `member_id`.

### Presence Panel (WebView)

Source: `jarvis-rs/assets/panels/presence/index.html`

The presence panel displays:

- Connection status indicator (dot: green = connected, red = error, gray =
  offline).
- Online user count (clickable to toggle user list dropdown).
- User list with: avatar (deterministic color from name hash), display name,
  status dot (color-coded by `UserStatus`), activity text.
- Poke button per user (sends `presence_poke` IPC to Rust, which sends a
  targeted broadcast).
- Notification feed (max 10 lines, auto-scrolling).

IPC messages received from Rust:
- `presence_update` -- status text update.
- `presence_users` -- full user list array.
- `presence_notification` -- single notification line.

IPC messages sent to Rust:
- `presence_request_users` -- requests fresh user list.
- `presence_poke` -- poke a user by `target_user_id`.

### Heartbeat and Reconnect

The presence `RoomClient` sends the relay's `{"type":"ping"}` keepalive and
reconnects with exponential backoff:

| Parameter                 | Default |
|---------------------------|---------|
| `reconnect_delay_secs`    | 1 s     |
| `max_reconnect_delay_secs`| 30 s    |
| Connect timeout           | 15 s    |

Reconnection uses exponential backoff: `delay = min(delay * 2, max_delay)`.  On
reconnect, the client re-sends a freshly-signed `room_hello` (a new unix-millis
`nonce`, strictly greater than the last, so the relay's monotonic check accepts
the genuine reconnect against the same pinned key), re-announces its own
presence, and re-seeds its roster from the `member_joined` burst the relay
replays.

---

## Mobile Relay Bridge

### Overview

The mobile bridge enables a phone to connect to a desktop Jarvis instance and
interact with its terminal sessions.  It is a **Bridge** session on the relay
(see [Relay Server](#relay-server-jarvis-relay) for the relay internals,
hello/response protocol, session store, rate limiting, CLI, and deployment).
It consists of:

1. **Relay client** -- outbound WebSocket connection from the desktop app.
2. **Mobile client** -- connects from the phone to the relay
   (`jarvis-mobile/lib/relay-connection.ts`).

The relay is a thin message forwarder that never inspects payload content.  All
PTY data is end-to-end encrypted between the desktop and mobile endpoints.

#### Bridge Session Lifecycle

```
Desktop connects  -->  Bridge session created (desktop_tx = Some)
                       Relay sends: session_ready
Mobile connects   -->  mobile_tx = Some
                       Relay sends: peer_connected to both
Mobile disconnects ->  mobile_tx = None
                       Relay sends: peer_disconnected to desktop
Desktop disconnects -> Session removed if both sides gone
```

### Desktop Relay Client

Source: `jarvis-rs/crates/jarvis-app/src/app_state/ws_server/`

The desktop side connects **outbound** to the relay server.

#### Startup

1. A session ID is loaded from disk (`~/.config/jarvis/relay_session_id`)
   or generated as a 32-character alphanumeric string and persisted.
2. A `MobileBroadcaster` (tokio `broadcast::channel`, capacity 256) is
   created for PTY output fan-out.
3. The relay client task connects to the configured relay URL, sends
   `desktop_hello`, and waits for `session_ready`.
4. Auto-reconnect with exponential backoff (1s base, 30s max).

#### Message Flow (Desktop to Mobile)

```
PTY output
  --> poll_pty_output() on main thread
  --> MobileBroadcaster.send(ServerMessage::PtyOutput)
  --> relay client task receives broadcast
  --> RelayCipher.encrypt_server_message()  (AES-256-GCM)
  --> RelayEnvelope::Encrypted { iv, ct }
  --> relay WebSocket --> relay server --> mobile client
```

#### Message Flow (Mobile to Desktop)

```
Mobile input arrives at relay server --> forwarded to desktop WebSocket
  --> relay client task receives frame
  --> Parse as RelayEnvelope
  --> If Encrypted: RelayCipher.decrypt_client_message()
  --> ClientCommand::PtyInput or PtyResize
  --> std::sync::mpsc to main thread
  --> poll_mobile_commands() writes to PTY
```

#### Wire Protocol (Inner)

Source: `jarvis-rs/crates/jarvis-app/src/app_state/ws_server/protocol.rs`

Messages between desktop and mobile (inside the relay envelope):

**Server (Desktop) to Client (Mobile):**
| Type         | Fields                          |
|-------------|----------------------------------|
| `pty_output` | `pane_id: u32`, `data: String`  |
| `pty_exit`   | `pane_id: u32`, `code: u32`     |
| `pane_list`  | `panes: [PaneInfo]`, `focused_id` |

**Client (Mobile) to Server (Desktop):**
| Type         | Fields                          |
|-------------|----------------------------------|
| `pty_input`  | `pane_id: u32`, `data: String`  |
| `pty_resize` | `pane_id: u32`, `cols: u16`, `rows: u16` |
| `ping`       | *(empty)*                       |

#### Relay Envelope

Source: `relay_protocol.rs`

```rust
pub enum RelayEnvelope {
    KeyExchange { dh_pubkey: String },
    Encrypted { iv: String, ct: String },
    Plaintext { payload: String },
}
```

- `KeyExchange` -- carries the DH public key (SPKI DER, base64) for ECDH.
- `Encrypted` -- AES-256-GCM ciphertext with base64-encoded IV and
  ciphertext.
- `Plaintext` -- only accepted before encryption is established; rejected
  after a cipher is active to prevent downgrade attacks.

### Mobile Client

Source: `jarvis-mobile/lib/relay-connection.ts`

The phone side is a TypeScript `RelayConnection` (React Native / Expo).  Given a
scanned pairing string, it:

1. Parses the pairing URL into `relayUrl`, `sessionId`, and the desktop's
   `dhPubkey`.
2. Opens a WebSocket to `relayUrl` and sends `mobile_hello`.
3. On `peer_connected`, runs the ECDH key exchange: it builds a `RelayCipher`
   from the desktop's DH pubkey and sends its own ephemeral pubkey back in a
   `key_exchange` envelope, then marks the connection encrypted.
4. Decrypts `encrypted` envelopes (rejecting `plaintext` once a cipher exists,
   for downgrade protection) and dispatches the inner `pty_output` /
   `pty_exit` / `pane_list` messages to the terminal UI; sends `pty_input` /
   `pty_resize` as encrypted envelopes.
5. Sends a `{"type":"ping"}` keepalive every 15 s, and auto-reconnects with
   exponential backoff (1 s base, 30 s max).  On `peer_disconnected` it clears
   its cipher and waits for the desktop to return.

---

## Mobile Device Pairing

### QR Code Flow

Source: `jarvis-rs/crates/jarvis-app/src/app_state/ws_server/pairing.rs`

The pairing process is triggered by the `PairMobile` action (available from
the command palette).

1. The desktop must have an active relay session (session ID available).
2. A pairing URL is constructed:
   ```
   jarvis://pair?relay=<relay_url>&session=<session_id>&dhpub=<url_encoded_dh_pubkey>
   ```
3. The URL is encoded as a QR code using Unicode half-block characters
   (upper/lower half blocks for two-row compression).
4. The QR code is displayed in the focused terminal pane via PTY output,
   along with the raw URL for manual entry.

### Key Exchange

After the mobile client scans the QR code and connects through the relay:

1. The relay sends `peer_connected` to both sides.
2. The desktop sends its ECDH public key to mobile via a `KeyExchange`
   envelope.
3. The mobile sends its ECDH public key back.
4. The desktop derives a shared AES-256 key:
   `CryptoService::derive_shared_key(mobile_dh_pubkey)`.
5. The raw 32-byte key is exported via `CryptoService::export_key()` and
   sent to the relay client task via a `tokio::sync::watch` channel.
6. The relay client creates a `RelayCipher` with the shared key.
7. From this point, all messages are encrypted.
8. The QR code pane is cleared and replaced with a "Mobile paired
   (encrypted)" confirmation message.

### Revocation

The `RevokeMobilePairing` action:

1. Shuts down the existing relay client connection.
2. Clears the `MobileBroadcaster`, command receivers, event receivers, key
   state, and peer-connected flag.
3. Deletes the persisted session ID file from disk.
4. Restarts the relay client with a freshly generated session ID.

This effectively invalidates the old pairing -- the mobile device can no
longer connect because the session ID has changed.  The cipher is also
cleared, requiring a full new key exchange.

### Security Note

When a `PeerDisconnected` message arrives from the relay, the desktop
intentionally does **not** clear the cipher.  A malicious relay could send
fake `PeerDisconnected` to force a plaintext downgrade.  The cipher is only
cleared on explicit revoke/re-pair.

---

## Crypto Service and Identity

### Overview

Source: `jarvis-rs/crates/jarvis-platform/src/crypto.rs`

The `CryptoService` is the central cryptographic engine for Jarvis.  It
manages persistent identity keys and ephemeral session keys.  The WebView
JavaScript never directly handles cryptographic keys -- all operations are
proxied through IPC to Rust.

### Identity Keys

Each Jarvis installation generates and persists two P-256 (secp256r1) key
pairs:

1. **ECDSA signing key** -- used for message signing and verification.
2. **ECDH key** -- used for Diffie-Hellman key agreement (DMs, mobile
   pairing).

Keys are stored in PKCS#8 DER format, base64-encoded, in a JSON identity
file:

```json
{
  "version": 1,
  "ecdsa_pkcs8_b64": "...",
  "ecdh_pkcs8_b64": "..."
}
```

On Unix, the identity file is written with mode `0600` (owner read/write
only).

### Public Key Export

Both public keys are exported as SPKI DER, base64-encoded, and made
available as:

- `pubkey_base64` -- ECDSA verifying key.
- `dh_pubkey_base64` -- ECDH public key.

### Fingerprint

The fingerprint is the first 8 bytes of `SHA-256(ECDSA SPKI DER)`,
formatted as colon-separated hex:

```
aa:bb:cc:dd:ee:ff:00:11
```

This provides a human-readable identity summary for TOFU verification.

### Key Store

The `CryptoService` maintains an in-memory key store mapping opaque `u32`
handles to 32-byte AES-256 key values.  Handles are monotonically
increasing.  Key material never crosses the IPC boundary -- only handles
are shared with the WebView.

### Symmetric Key Derivation

Two derivation methods:

1. **Room key (PBKDF2):**
   `derive_room_key(room_name)` derives an AES-256 key using
   PBKDF2-HMAC-SHA256 with salt `jarvis-livechat-salt-v1` and 10,000
   iterations.  Deterministic: same room name always produces the same key.

2. **Shared key (ECDH):**
   `derive_shared_key(other_dh_spki_b64)` performs elliptic-curve
   Diffie-Hellman with another party's public key.  The raw shared secret
   is hashed with SHA-256 to produce the AES-256 key.

### Encryption / Decryption

All symmetric encryption uses **AES-256-GCM** with:

- 12-byte random nonce (IV).
- Output: `(iv_base64, ciphertext_base64)`.

### Signing / Verification

- `sign(data)` signs UTF-8 data with ECDSA-SHA256-P256 and returns a
  base64-encoded IEEE P1363 signature (r||s, 64 bytes).
- `verify(data, sig_b64, pubkey_b64)` verifies against an SPKI-encoded
  public key.

### IPC Operations

The chat panel communicates with the `CryptoService` via
`window.jarvis.ipc.request('crypto', { op, params })`:

| Operation          | Parameters                      | Returns |
|--------------------|--------------------------------|---------|
| `init`             | *(none)*                       | `fingerprint`, `pubkey`, `dhPubkey` |
| `derive_room_key`  | `room: string`                 | `keyHandle: number` |
| `derive_shared_key`| `dhPubkey: string`             | `keyHandle: number` |
| `encrypt`          | `plaintext`, `keyHandle`       | `iv`, `ct` |
| `decrypt`          | `iv`, `ct`, `keyHandle`        | `plaintext` |
| `sign`             | `data: string`                 | `signature: string` |
| `verify`           | `data`, `signature`, `pubkey`  | `valid: boolean` |

---

## AI Assistant Panel

### Overview

The AI assistant is an overlay panel that provides a conversational
interface to Claude (or other AI providers).

### Architecture

```
User types message
  --> handle_assistant_key("Enter")
  --> panel.take_input()
  --> send to assistant_tx (std::sync::mpsc)
  --> assistant_task (async, Tokio runtime)
      --> ClaudeClient.send_message_streaming()
      --> on_chunk callback sends AssistantEvent::StreamChunk
  --> poll_assistant() on main thread
      --> panel.append_streaming_chunk()
      --> send_assistant_ipc("assistant_chunk", ...)
```

Source:
- `jarvis-rs/crates/jarvis-app/src/app_state/assistant.rs`
- `jarvis-rs/crates/jarvis-app/src/app_state/assistant_task.rs`

### Assistant Task

The assistant task runs on the shared Tokio runtime (lazily created, 1
worker thread).  It:

1. Loads `ClaudeConfig` from environment variables (`ANTHROPIC_API_KEY`,
   etc.).
2. Creates a `Session` with a system prompt:
   *"You are Jarvis, an AI assistant embedded in a terminal emulator.
   Be concise and helpful. Use plain text, not markdown."*
3. Enters a receive loop on `user_rx`.
4. For each message, calls `session.chat_streaming()` which streams SSE
   chunks back via the `on_chunk` callback.
5. Sends `AssistantEvent::Done` when the response completes, or
   `AssistantEvent::Error` on failure.

### Event Types

| Event          | Description |
|----------------|-------------|
| `Initialized`  | Runtime ready, includes model name. |
| `StreamChunk`  | Partial text from streaming response. |
| `Done`         | Full response complete. |
| `Error`        | API or network error. |

### IPC Messages

The assistant panel webview receives:

| IPC Kind            | Payload |
|---------------------|---------|
| `assistant_config`  | `{ model_name }` |
| `assistant_chunk`   | `{ text }` |
| `assistant_output`  | `{ text }` (full accumulated response) |
| `assistant_error`   | `{ message }` |

### AI Engine (`jarvis-ai`)

Source: `jarvis-rs/crates/jarvis-ai/src/`

The AI engine provides:

- **`AiClient` trait** -- common interface with `send_message` and
  `send_message_streaming` methods.
- **`ClaudeClient`** -- Anthropic Claude API client with SSE streaming.
- **`GeminiClient`** -- Google Gemini API client.
- **`WhisperClient`** -- OpenAI Whisper speech-to-text client.
- **`SkillRouter`** -- dispatches requests to the appropriate provider
  based on skill name.  Default provider is Claude.
- **`Session`** -- manages conversation history, system prompts, tool
  definitions, and tool-call loops (up to 10 rounds).
- **`TokenTracker`** -- accumulates input/output token usage.

### Tool Calling

The `Session` supports tool calling.  `ToolDefinition`s are registered with
name, description, and JSON Schema parameters.  When the AI returns a
`ToolCall`, the session executes it via the registered `ToolExecutor`
callback and feeds the result back for the next round.  Maximum tool-call
loop iterations: 10.

---

## Chat Panel Features

### No External SDK

The chat panel has **no third-party SDK dependency**.  The former Supabase
JavaScript SDK (loaded from a CDN with Subresource Integrity) has been removed;
the transport is now the plain-WebSocket `RoomConnection` against
`jarvis-relay`.  This eliminates the CDN supply-chain surface entirely.

### AutoMod

Source: `jarvis-rs/assets/panels/chat/index.html`, class `AutoMod`

Client-side auto-moderation runs on both outgoing and incoming messages.

**Keyword filter:**
A set of banned words is checked against message text using pre-compiled
word-boundary regexes (`\b<word>\b`, case-insensitive).  Words can be
added or removed at runtime via `addBanWord()` / `removeBanWord()`.

**Rate limiting:**
A sliding-window rate limiter tracks message timestamps per user:

| Parameter            | Default |
|----------------------|---------|
| `RATE_LIMIT_COUNT`   | 5       |
| `RATE_LIMIT_WINDOW`  | 10,000 ms |

Both sender-side and receiver-side rate limits are enforced.

**Spam detection:**
- Repeated character ratio: if >80% of a message (>5 chars) is the same
  character, it is blocked (`spam_repeated_chars`).
- Repeated message history: if the last 3 messages from a user are
  identical, the message is blocked (`spam_repeated_message`).

**Cleanup:**
A periodic cleanup runs every 60 seconds to prune stale rate-limit
entries and cap spam history to 200 users.

### AutoMod (Backend Configuration)

Source: `jarvis-rs/crates/jarvis-config/src/schema/livechat.rs`

```toml
[livechat.automod]
enabled = true
filter_profanity = true
rate_limit = 5
max_message_length = 500
spam_detection = true
```

### Nickname System

Nicknames are configurable per the `NicknameConfig`:

| Parameter      | Default  |
|----------------|----------|
| `min_length`   | 1        |
| `max_length`   | 20       |
| `pattern`      | `^[a-zA-Z0-9_\- ]+$` |
| `persist`      | true     |
| `allow_change` | true     |

Nicknames are persisted to `localStorage` and subjected to keyword
filtering before being accepted.

### TOFU Trust Store

The chat panel implements a Trust On First Use (TOFU) model for identity
verification.  When a nickname-fingerprint pair is first seen, it is
recorded in `localStorage` under `jarvis-chat-tofu`.  On subsequent
messages:

- If the fingerprint matches: `trusted` (green checkmark).
- If the fingerprint differs: `changed` (amber warning + system message).
- If no fingerprint is present: `unverified` (gray question mark).

### XSS Prevention

All user-generated content (nicknames, message text, system messages) is
rendered using `element.textContent = value` rather than `innerHTML`,
preventing XSS injection.

---

## Workspace Streaming

**Source:** `jarvis-rs/crates/jarvis-app/src/app_state/webview_bridge/chat_stream_handlers.rs`, `jarvis-rs/crates/jarvis-app/src/app_state/workspace_capture/`

Jarvis supports live workspace streaming to the chat panel, allowing mobile-connected users to see your desktop workspace in real time.

### Architecture

The chat panel sends a `chat_stream_control` IPC message with an action field:

| Action     | Effect |
|------------|--------|
| `"start"`  | Begins capturing workspace frames on a background thread |
| `"stop"`   | Stops the capture thread |
| `"status"` | Returns whether streaming is currently active |

When streaming is active, a background thread captures workspace frames at a fixed interval (`CHAT_STREAM_FRAME_INTERVAL = 350ms`) and sends them back to the chat WebView as `chat_stream_host_frame` IPC messages containing a JPEG data URL.

### Platform Support

The workspace capture module (`app_state/workspace_capture/`) has per-platform implementations:

| Platform | Status |
|----------|--------|
| macOS    | Native implementation (CoreGraphics) |
| Linux    | Native implementation |
| Windows  | Stub (returns error -- implementation pending) |

---

## Retro Game Emulator

**Source:** `jarvis-rs/crates/jarvis-app/src/app_state/webview_bridge/emulator_handlers.rs`, `jarvis-rs/assets/panels/games/emulator.html`

Jarvis includes a retro game emulator panel that can load ROM files from the user's system.

### IPC Messages

| Kind                 | Direction | Description |
|----------------------|-----------|-------------|
| `emulator_list_roms` | JS -> Rust | Scans `~/ROMs` directory for ROM files and returns a list |
| `emulator_load_rom`  | JS -> Rust | Reads a ROM file and returns its contents as base64-encoded data |

### Supported ROM Formats

The emulator scans for files with these extensions: `.nes`, `.smc`, `.sfc`, `.gb`, `.gbc`, `.gba`, `.nds`, `.n64`, `.z64`, `.v64`, `.bin`, `.cue`, `.iso`, `.md`, `.smd`, `.sms`, `.gg`, `.a26`, `.zip`, `.7z`.

Maximum ROM size: **64 MB**.

### WebView Transparency

The emulator panel uses WebGL for rendering, which requires an opaque (non-transparent) WebView. When navigating to the emulator, the existing transparent WebView is destroyed and recreated with `transparent: false`. When exiting the emulator, the WebView is recreated with the default transparency setting.

---

## Network Configuration Reference

### Presence Configuration

Source: `jarvis-rs/crates/jarvis-config/src/schema/social.rs`

Presence no longer has its own connection settings -- it reuses the relay URL
from `[relay].url` and joins the global room named by `room_id`:

```toml
[presence]
# Session id of the global presence room every desktop joins.
room_id = "jarvis-presence-global"
```

### Livechat Configuration

Source: `jarvis-rs/crates/jarvis-config/src/schema/livechat.rs`

```toml
[livechat]
enabled = true
server_port = 19847
connection_timeout = 10  # seconds

[livechat.nickname]
default = ""
persist = true
allow_change = true

[livechat.nickname.validation]
min_length = 1
max_length = 20
pattern = "^[a-zA-Z0-9_\\- ]+$"

[livechat.automod]
enabled = true
filter_profanity = true
rate_limit = 5
max_message_length = 500
spam_detection = true
```

### Relay Configuration

Source: `jarvis-rs/crates/jarvis-config/src/schema/relay.rs`

This single URL is shared by the mobile bridge, presence, and chat.  The
default points at the Railway deployment of `jarvis-relay`:

```toml
[relay]
url = "wss://jarvis-relay-production-3eb6.up.railway.app/ws"
auto_connect = false
```

### Relay Room / Presence Client Defaults

These apply to the desktop's presence `RoomClient` and (mirrored in JS) the
chat `RoomConnection`:

| Parameter                          | Value |
|------------------------------------|-------|
| Keepalive ping                     | `{"type":"ping"}` (dropped by relay) |
| Reconnect base delay               | 1 second |
| Max reconnect delay                | 30 seconds |
| WebSocket connect timeout          | 15 seconds |
| Own-frame echo                     | never (relay fans out all-but-sender) |

### Relay Server Defaults

| Parameter                  | Default |
|----------------------------|---------|
| Port                       | 8080 (overridable by `$PORT`) |
| Session TTL (no live peers)| 300 s   |
| Max connections per IP     | 10      |
| Max total sessions         | 1000    |
| Max session ID length      | 64 bytes |
| Hello timeout              | 10 s    |
| Rate window                | 60 s    |
| Max connect rate per IP    | 20      |

---

## Security Model

### Message Authentication and Encryption

Jarvis's networked messaging has three protection contexts:

1. **Livechat channel messages** -- carried as opaque application frames over
   the relay Room and **signed** with the sender's ECDSA-P256 key.  The message
   text rides in the `text` field.  Authentication (not confidentiality) is the
   guarantee here: the relay forwards frames it cannot forge a valid signature
   for, and receivers verify every signature.  (The `CryptoService` also exposes
   a PBKDF2-derived per-channel room key via `derive_room_key`, used for
   background channel-key caching.)

2. **Direct messages (DMs)** -- end-to-end encrypted with AES-256-GCM using a
   shared key derived via ECDH between the two participants.  The frame carries
   `iv`/`ct` instead of `text`; only the two endpoints can decrypt, and the
   relay sees only ciphertext.

3. **Mobile relay bridge** -- all PTY data is encrypted with AES-256-GCM
   using a shared key derived via ECDH between desktop and mobile.  The
   relay server sees only opaque ciphertext.

### Key Material Isolation

- Private keys (ECDSA, ECDH) live exclusively in the Rust `CryptoService`.
- The WebView receives only opaque key **handles** (u32 integers).
- Encryption/decryption/signing operations are performed Rust-side via IPC.
- The identity file is written with restricted permissions (`0600` on Unix).

### Downgrade Protection

- After a cipher is established on the relay connection, plaintext
  `RelayEnvelope::Plaintext` messages are rejected.
- `PeerDisconnected` from the relay does **not** clear the cipher --
  preventing a relay-in-the-middle from forcing a plaintext downgrade.
- Cipher is only cleared on explicit pairing revocation.

### Identity Verification (TOFU)

The Trust On First Use model records nickname-to-fingerprint bindings.
Key changes trigger visible warnings in the chat UI.  Signature verification
uses the ECDSA public key embedded in each message, checked against the
TOFU store.

### Message Signing

Every chat message includes:
- An ECDSA-P256-SHA256 signature over the canonical string
  `id|userId|nick|ts|text` (or `id|userId|nick|ts|iv|ct` for DMs).
- The sender's SPKI-encoded public key and fingerprint.

Receivers verify signatures Rust-side and display verification status
badges.

### Automod as Defense-in-Depth

Client-side automod provides keyword filtering, rate limiting, and spam
detection on both outgoing and incoming messages.  This is not a security
boundary (clients could be modified), but it protects well-behaved users
from spam and abuse in the shared chat environment.

### No External Script Dependencies

The chat panel no longer loads any third-party JavaScript SDK from a CDN (the
Supabase SDK is gone), removing that supply-chain surface entirely.  The
transport is a plain WebSocket to the project's own relay.

### Rate Limiting (Relay)

The relay server implements per-IP connection limits and global session
caps to prevent resource exhaustion.  Connection rate limiting uses a
sliding window, and stale sessions are reaped automatically.

### Signed Room Slot Binding (Relay)

Room membership is a security boundary, not just a label.  Every `room_hello`
is signed with the client's ECDSA P-256 identity, and the relay verifies the
signature, enforces nonce freshness (±30 s sanity window) **and** a per-slot
strictly-monotonic nonce (anti-replay), and TOFU-pins
`(session_id, member_id) → pubkey` — the first signed join pins the key; later
joins for that slot need the same key **and** a strictly-greater nonce.  For
chat-style `<fingerprint>.<userId>` ids the relay additionally checks that the
fingerprint prefix equals `fingerprint(pubkey)`.  Verification happens *before*
admission; the pin is committed only *after* the slot registers (no orphan
pins).  Consequently a self-asserted `member_id` can no longer squat or evict a
slot without the matching private key, and a captured hello replayed within the
freshness window cannot evict the live connection holding the slot.  See
[Signed `room_hello`](#signed-room_hello-room-slot-binding).

### Collaboration Features

The `jarvis-social` crate also includes collaboration modules (pair
programming, and experimental voice / screen share).  **Pair programming**
rides the relay's symmetric **Room** sessions (the same N:N transport that
backs chat and presence), over an encrypted Room keyed by a capability secret.
These features -- their roles, cursor tracking, and signaling -- are documented
in detail in the collaboration chapter; this chapter only covers the relay
Room transport they share.
