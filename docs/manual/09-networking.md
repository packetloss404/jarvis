# 09 -- Networking, Social, and Communication

This chapter covers Jarvis's networked features: the live chat system, online
presence, the mobile relay bridge, the cryptographic identity layer, and the
AI assistant panel.  Every subsystem is described from both the user-facing
perspective and the implementation perspective so that contributors can
navigate the codebase confidently.

---

## Table of Contents

1. [Live Chat System](#live-chat-system)
2. [Presence System](#presence-system)
3. [Relay System and Mobile Bridge](#relay-system-and-mobile-bridge)
4. [Mobile Device Pairing](#mobile-device-pairing)
5. [Crypto Service and Identity](#crypto-service-and-identity)
6. [AI Assistant Panel](#ai-assistant-panel)
7. [Chat Panel Features](#chat-panel-features)
8. [Network Configuration Reference](#network-configuration-reference)
9. [Security Model](#security-model)

---

## Live Chat System

### Overview

Jarvis ships a multi-channel live chat panel that lets users communicate in
real time.  The chat is backed by **Supabase Realtime** (Phoenix Channels v1
protocol over WebSocket) and runs entirely inside a WebView panel loaded from
`jarvis://localhost/chat/index.html`.

### Architecture

```
 Chat WebView (HTML/JS)
   |
   |-- Supabase JS SDK (CDN, SRI-verified)
   |      |
   |      +-- wss://<project>.supabase.co/realtime/v1/websocket
   |
   |-- jarvis.ipc.request('crypto', ...)   (Rust-side CryptoService)
   |
   +-- AutoMod  (client-side moderation, JS)
```

All chat messages are broadcast via Supabase Realtime channels.  The chat
panel connects directly from the WebView; the Rust backend does **not**
proxy chat traffic.  Crypto operations (encryption, signing, key derivation)
are delegated to the Rust `CryptoService` through IPC so that WebView
JavaScript never handles raw key material.

### Channels

Seven channels are pre-configured:

| Supabase Channel ID          | Display Name |
|------------------------------|-------------|
| `jarvis-livechat`            | `# general` |
| `jarvis-livechat-discord`    | `# discord` |
| `jarvis-livechat-showoff`    | `# showoff` |
| `jarvis-livechat-help`       | `# help`    |
| `jarvis-livechat-random`     | `# random`  |
| `jarvis-livechat-games`      | `# games`   |
| `jarvis-livechat-memes`      | `# memes`   |

The `general` channel is the **primary** channel: it carries Supabase
Presence state (online user tracking).  All channels are subscribed at
startup so that messages arriving on background channels are buffered and
unread counts are tracked.

Channel switching is instant because every channel is pre-subscribed.
Source: `jarvis-rs/assets/panels/chat/index.html`, `CONFIG.CHANNELS`.

### Channels (Backend)

On the Rust side, `jarvis-social` provides a `ChannelManager` struct
(`jarvis-rs/crates/jarvis-social/src/channels.rs`) that maintains an
in-memory set of channels with member tracking.  Two channels are created
by default: `general` and `games`.  Users are auto-joined, and the manager
supports join, leave, leave-all, member listing, and per-user channel queries.

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
   `Identity.sign()` (IPC to Rust).
5. The payload is broadcast on the active Supabase channel:
   ```json
   {
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
6. The message is stored in the local channel history and rendered
   immediately with `verifyStatus: 'self'`.

### Message Flow (Receive)

1. A broadcast arrives on the Supabase channel subscription.
2. If `payload.userId === this.userId`, the message is dropped (self-echo
   prevention).
3. AutoMod filters run: keyword check on text and nickname, spam check
   (repeated character ratio, repeated identical messages), and per-user
   rate limiting.
4. ECDSA signature verification is performed via `Identity.verify()` (IPC).
5. The TOFU Trust Store checks whether the sender's fingerprint matches
   previously-seen identity for that nickname.
6. A verification badge is attached:
   - Checkmark (verified): signature valid, TOFU trusted or new.
   - Warning (key-changed): signature valid but fingerprint differs from
     previously recorded -- possible impersonation.
   - Cross (invalid): signature verification failed.
   - Question mark (unverified): no signature present.
7. The message is rendered and stored in the channel history.

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

Messages support emoji reactions.  Reactions are broadcast as a separate
`reaction` event (unencrypted) with `{ msgId, emoji, userId, action }`.
A picker of 16 emoji is shown on hover.  Reactions are tracked per message
in the local history and persisted across channel switches.

### Direct Messages (DMs)

Users can open an end-to-end encrypted DM from the online users dropdown.
The flow:

1. User clicks a peer's "DM" button in the dropdown.
2. An ECDH shared key is derived: `Identity.deriveSharedKey(otherDhPubkey)`.
3. A deterministic DM channel name is computed from both fingerprints
   (sorted, concatenated with `jarvis-dm-` prefix).
4. A new Supabase channel subscription is created for the DM.
5. Outbound DM messages are encrypted with AES-256-GCM using the ECDH-derived
   key via `Crypto.encrypt()`.  The payload contains `iv` and `ct` (base64)
   instead of `text`.
6. Inbound DM messages are decrypted via `Crypto.decrypt()`.
7. Signatures are computed over `id|userId|nick|ts|iv|ct` (the ciphertext
   components, not plaintext) to prevent chosen-plaintext attacks on the
   signature oracle.

### Reconnection

If the primary channel drops, an exponential backoff reconnect strategy
engages:

| Parameter       | Value  |
|-----------------|--------|
| Base delay      | 2 s    |
| Max delay       | 30 s   |
| Max attempts    | 8      |
| Jitter          | 75-125% |

After max attempts, the user is prompted to reload.

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

Source: `jarvis-rs/crates/jarvis-social/src/presence/`

The `PresenceClient` connects to Supabase Realtime via the reusable
`RealtimeClient`.  On start:

1. A `RealtimeClient` is created with the configured `project_ref` and
   `api_key`.
2. The `jarvis-presence` channel is joined with presence tracking enabled
   (keyed by `user_id`).
3. The client tracks its presence with a payload containing:
   `user_id`, `display_name`, `status`, `activity`, `online_at`.
4. A background `event_translator` task converts low-level
   `RealtimeEvent`s into high-level `PresenceEvent`s.

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
| `Connected`        | Channel joined; includes initial `online_count`. |
| `Disconnected`     | Connection lost; user list cleared. |
| `UserOnline`       | Another user joined. |
| `UserOffline`      | Another user left. |
| `ActivityChanged`  | A user's status or activity text changed. |
| `GameInvite`       | A user broadcast a game invitation. |
| `Poked`            | Someone poked this user (targeted by `target_user_id`). |
| `ChatMessage`      | A chat message arrived on the presence channel. |
| `Error`            | Connection or protocol error. |

### Broadcast Events

Four broadcast event types ride on the `jarvis-presence` channel:

| Event Name         | Payload Type             |
|--------------------|--------------------------|
| `activity_update`  | `ActivityUpdatePayload`  |
| `game_invite`      | `GameInvitePayload`      |
| `poke`             | `PokePayload`            |
| `chat_message`     | `ChatMessagePayload`     |

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
`display_name` defaults to the OS `USER`/`USERNAME` environment variable.
Identities support optional Supabase Auth JWTs for authenticated connections.
The access token is never serialized (`#[serde(skip)]`) and redacted in
`Debug` output.

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

The Supabase Realtime connection sends Phoenix heartbeat messages at a
configurable interval:

| Parameter                 | Default |
|---------------------------|---------|
| `heartbeat_interval_secs` | 25 s    |
| `reconnect_delay_secs`    | 1 s     |
| `max_reconnect_delay_secs`| 30 s    |

Reconnection uses exponential backoff: `delay = min(delay * 2, max_delay)`.
On reconnect, previously-joined channels are automatically re-joined and
presence is re-tracked.

---

## Relay System and Mobile Bridge

### Overview

The relay system enables a mobile phone to connect to a desktop Jarvis
instance and interact with its terminal sessions.  It consists of three
components:

1. **`jarvis-relay`** -- a standalone WebSocket relay server (binary crate).
2. **Relay client** -- outbound WebSocket connection from the desktop app.
3. **Mobile client** -- connects from the phone to the relay.

The relay is a thin message forwarder that never inspects payload content.
All PTY data is end-to-end encrypted between the desktop and mobile
endpoints.

### Relay Server (`jarvis-relay`)

Source: `jarvis-rs/crates/jarvis-relay/src/`

The relay is a standalone Tokio application that:

- Listens for TCP connections on a configurable port (default 8080).
- Accepts WebSocket upgrades.
- Pairs connections by session ID (one desktop + one mobile per session).
- Forwards text frames bidirectionally between paired peers.

#### Hello Protocol

The first message from each client identifies its role:

```json
// Desktop
{"type": "desktop_hello", "session_id": "abc123..."}

// Mobile
{"type": "mobile_hello", "session_id": "abc123..."}
```

Desktop clients **create** sessions; mobile clients **join** existing ones.
After hello, all subsequent text frames are forwarded opaquely to the peer.

#### Session Lifecycle

```
Desktop connects  -->  Session created (desktop_tx = Some)
                       Relay sends: session_ready
Mobile connects   -->  mobile_tx = Some
                       Relay sends: peer_connected to both
Mobile disconnects ->  mobile_tx = None
                       Relay sends: peer_disconnected to desktop
Desktop disconnects -> Session removed if both sides gone
```

#### Session Store

`SessionStore` (`session.rs`) maps session IDs to `Session` structs
containing optional `mpsc::Sender<String>` handles for each role.  A stale
session reaper runs every 60 seconds and removes sessions older than
`session_ttl` (default 300 seconds) that have no mobile peer.

#### Rate Limiting

`RateLimiter` (`rate_limit.rs`) enforces:

| Parameter                  | Default | Description |
|----------------------------|---------|-------------|
| `max_connections_per_ip`   | 10      | Concurrent WebSocket connections per IP |
| `max_connect_rate_per_ip`  | 20      | New connections per IP per rate window |
| `rate_window_secs`         | 60      | Sliding window for rate counting |
| `max_total_sessions`       | 1000    | Global session cap |
| `max_session_id_len`       | 64      | Maximum session ID length in bytes |

Rate-limited connections are rejected before the WebSocket handshake
completes.  Stale rate-limit entries are pruned on each connection attempt.

#### CLI Arguments

```
jarvis-relay [OPTIONS]

Options:
  -p, --port <PORT>                    Port to listen on [default: 8080]
      --session-ttl <SECONDS>          Max stale session age [default: 300]
      --max-connections-per-ip <N>     Per-IP concurrent limit [default: 10]
      --max-sessions <N>              Global session cap [default: 1000]
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

### Supabase SDK (SRI)

The Supabase JavaScript SDK is loaded from a CDN with Subresource
Integrity verification:

```html
<script
  src="https://cdn.jsdelivr.net/npm/@supabase/supabase-js@2.97.0/dist/umd/supabase.min.js"
  integrity="sha384-1+ItoWbWcmVSm+Y+dJaUt4SEWNA21/jxef+Z0TSHHVy/dEUxEUEnZ1bHn6GT5hj+"
  crossorigin="anonymous">
</script>
```

SRI ensures the loaded script has not been tampered with.

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

## Network Configuration Reference

### Presence Configuration

Source: `jarvis-rs/crates/jarvis-config/src/schema/social.rs`

```toml
[presence]
enabled = true
server_url = ""         # Supabase project ref
heartbeat_interval = 30 # seconds
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

```toml
[relay]
url = "wss://jarvis-relay-363598788638.us-central1.run.app/ws"
auto_connect = false
```

### Supabase Realtime Connection

| Parameter                          | Value |
|------------------------------------|-------|
| WebSocket URL pattern              | `wss://<project_ref>.supabase.co/realtime/v1/websocket?apikey=<key>&vsn=1.0.0` |
| Heartbeat interval                 | 25 seconds |
| Reconnect base delay               | 1 second |
| Max reconnect delay                | 30 seconds |
| WebSocket connect timeout          | 15 seconds |
| Channel join / broadcast ack       | enabled |
| Self-send on broadcasts            | disabled |

### Relay Server Defaults

| Parameter                  | Default |
|----------------------------|---------|
| Port                       | 8080    |
| Session TTL (no mobile)    | 300 s   |
| Max connections per IP     | 10      |
| Max total sessions         | 1000    |
| Max session ID length      | 64 bytes |
| Hello timeout              | 10 s    |
| Rate window                | 60 s    |
| Max connect rate per IP    | 20      |

---

## Security Model

### End-to-End Encryption

Three encryption contexts exist in Jarvis:

1. **Livechat channel messages** -- encrypted with AES-256-GCM using a
   room key derived via PBKDF2 from the channel name.  Since the room name
   is known to all participants, this provides protection against relay
   snooping but not against other channel members.

2. **Direct messages (DMs)** -- encrypted with AES-256-GCM using a shared
   key derived via ECDH between the two participants.  Only the two
   endpoints can decrypt.

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

### SRI for External Dependencies

The Supabase JavaScript SDK loaded in the chat panel is protected with a
`sha384` Subresource Integrity hash and `crossorigin="anonymous"`, ensuring
the script has not been tampered with in transit or at the CDN.

### Rate Limiting (Relay)

The relay server implements per-IP connection limits and global session
caps to prevent resource exhaustion.  Connection rate limiting uses a
sliding window, and stale sessions are reaped automatically.

### Experimental Collab Features

Behind the `experimental-collab` feature flag, the `jarvis-social` crate
includes additional modules for:

- **Pair programming** (`pair/`) -- collaborative terminal sessions with
  driver/navigator roles, remote cursor tracking, and terminal
  input/output relay.
- **Voice chat** (`voice/`) -- WebRTC voice rooms with SDP offer/answer
  and ICE candidate signaling.
- **Screen sharing** (`screen_share/`) -- WebRTC screen sharing with
  configurable quality and SDP/ICE signaling.

These features use the same Supabase Realtime transport for WebRTC
signaling (SDP offers/answers and ICE candidates).
