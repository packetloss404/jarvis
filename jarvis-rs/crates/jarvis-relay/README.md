# jarvis-relay

WebSocket relay for Jarvis: **bridge** sessions (desktop ↔ mobile PTY), **broadcast** sessions (host ↔ spectators), and **room** sessions (symmetric N:N chat/presence). The relay parses only the first frame (hello); after that it forwards **opaque text** between peers.

## Wire protocol (relay layer)

Defined in [`src/protocol.rs`](src/protocol.rs).

**Client → relay (first message only):**

| `type`             | Role      | Purpose                          |
|--------------------|-----------|----------------------------------|
| `desktop_hello`    | Desktop   | Creates / rejoins bridge session |
| `mobile_hello`     | Mobile    | Joins existing bridge session    |
| `host_hello`       | Host      | Broadcast session                |
| `spectator_hello`  | Spectator | Watch broadcast                  |
| `room_hello`       | Member    | Join / create room session; also carries `member_id` |

**Relay → client:**

| `type`               | Meaning                          |
|----------------------|----------------------------------|
| `session_ready`      | Registered; includes `session_id` |
| `peer_connected`     | Bridge peer joined               |
| `peer_disconnected`  | Bridge peer left                 |
| `host_connected`     | (Broadcast) host online          |
| `host_disconnected`  | (Broadcast) host left            |
| `viewer_count`       | Spectator count                  |
| `room_ready`         | (Room) member registered; includes `session_id` |
| `member_joined`      | (Room) a member joined; includes `member_id` |
| `member_left`        | (Room) a member left; includes `member_id` |
| `member_count`       | (Room) current member count      |
| `error`              | Fatal / validation error         |

## Room sessions (N:N)

A **room** is a symmetric session where every member's frames fan out to **all other members** (never echoed back to the sender). Rooms back relay-hosted chat + presence (replacing Supabase).

- The first `room_hello` for a `session_id` **auto-creates** the room (subject to the global session cap); subsequent members join it. `member_id` identifies each participant.
- On join the member receives `room_ready`, then a `member_joined` for each member already present (its initial roster), then a `member_count`. Existing members each receive a `member_joined { member_id }` for the newcomer.
- After registration, **opaque text** frames from a member are forwarded to every other member via all-but-sender fan-out. `{"type":"ping"}` keepalives are dropped, as on bridge links.
- Reconnecting with the same `member_id` **replaces** that member's channel rather than adding a duplicate.
- On disconnect the relay fans out `member_left { member_id }` and an updated `member_count` to the remaining members. When the last member leaves, the room is removed.

## PTY / encryption (opaque to relay)

After registration, clients exchange JSON **envelopes** (`key_exchange`, `plaintext`, `encrypted`) and inner PTY messages. The relay does not parse those.

- **Desktop:** [`jarvis-app` `relay_client.rs`](../jarvis-app/src/app_state/ws_server/relay_client.rs), [`relay_protocol.rs`](../jarvis-app/src/app_state/ws_server/relay_protocol.rs)
- **Mobile:** [`jarvis-mobile/lib/relay-connection.ts`](../../../jarvis-mobile/lib/relay-connection.ts)

## Keepalive

Mobile sends periodic JSON `{"type":"ping"}` on the WebSocket. The relay **does not forward** these over bridge or room links (peers never consumed them); WebSocket **Ping** frames are still answered with **Pong** as usual.

## Conformance

`session_ready` (and future relay control messages) are covered by shared JSON under [`jarvis-rs/testdata/relay/`](../../testdata/relay/); `jarvis-relay` and `jarvis-app` tests deserialize or serialize against the same file.

## Run

```bash
cargo run -p jarvis-relay -- --help
```

Options and defaults are in `src/main.rs` (listen address, TLS, session limits, etc.).
