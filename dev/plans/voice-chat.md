# V2 — Multi-User Voice Chat & Interactive Screen Share — Design & Build Spec

Status: **NOT STARTED — SPEC / DECISION DOC.** This is the v2 feature Dylan
feature-flagged out of v1 (`dev/_archive/jarvis-rs/PLAN_2026-02-27.md`, Phase 8,
"WebRTC immaturity, reduces attack surface"). Produced 2026-06-05 as the design
blueprint and the investment decision record. **This document is a SPEC, not an
implementation.** Unlike the C2 pair-programming feature — which shipped its
M1–M3 entirely inside the existing app + relay — this feature has a **hard
external infrastructure dependency that does not exist yet**: a TURN server
(coturn) must be stood up and paid for before multi-party media can work
reliably. The owner should read §2 and §6 first and decide *whether/when* to
invest before any code is written.

The coordination plane is **already substantially built** behind the
`experimental-collab` feature flag (`VoiceManager`, `ScreenShareManager`,
`VoiceSignal`/`ScreenShareSignal`, see §0). What is **missing** is the entire
**media plane** (WebRTC peer connections, audio capture/codec, HD video
encode) and the **infrastructure** to make it traverse NATs.

---

## 0. Existing world (built vs missing)

The honest starting point. A surprising amount of the *coordination* layer
already exists; **none** of the *media* layer does.

### Built (behind `experimental-collab`, compiled out of the app today)

- **`VoiceManager`** (`jarvis-social/src/voice/manager.rs`) — complete room
  lifecycle: `create_room`/`join_room` (returns existing participant ids so the
  caller can fan out WebRTC offers, ~:142), `leave_current_room`, `set_muted`/
  `set_deafened`/`set_speaking` (VAD-driven), `handle_signal` (relays a
  `VoiceSignal` as a `VoiceEvent::Signal`), `handle_user_offline`. State is a
  single-locked `VoiceState { rooms, user_rooms }` (`voice/types.rs`).
  `VoiceConfig` defaults `enabled=false`, `max_participants=8`.
- **`ScreenShareManager`** (`jarvis-social/src/screen_share/manager.rs`) —
  `start_sharing`/`stop_sharing`/`join_session`/`leave_session`/`set_quality`,
  with a `ShareQuality` preset ladder (Low 720p10 → Ultra native30,
  `screen_share/types.rs`). `join_session` returns the host id so the caller can
  initiate the WebRTC connection. Defaults `enabled=false`, `max_viewers=4`.
- **Signaling types** (`jarvis-social/src/protocol.rs:237-269`, gated):
  `VoiceSignal` and `ScreenShareSignal`, each `Offer{sdp}` / `Answer{sdp}` /
  `IceCandidate{candidate, sdp_mid, sdp_m_line_index}`. **These are exactly the
  SDP/ICE payloads WebRTC signaling needs** — they were written for this.
- **Feature gating** (`jarvis-social/src/lib.rs:8-32`, `Cargo.toml:6-10`):
  `pair`/`screen_share`/`voice` are behind `experimental-collab`; `jarvis-app`
  depends on `jarvis-social` with **no** features, so none of this is compiled
  into the shipping binary. The Cargo comment is blunt: *"no authentication yet
  — do NOT enable in production."*

### Missing (the real work)

- **The entire media plane.** No WebRTC integration anywhere (`webrtc-rs`,
  `libwebrtc`, and WebView-RTC are all absent — confirmed: no `webrtc`/`cpal`/
  `opus` dependency in any `Cargo.toml`). The managers above are pure
  coordination state machines; nothing creates a `RTCPeerConnection`, captures a
  mic, encodes Opus, or moves a single audio packet.
- **No app-layer wiring.** Unlike pair (which has `app_state/pair.rs`, a room
  client, IPC handlers, and a panel), voice/screen-share have **no**
  `app_state/voice.rs`, no IPC kinds, no panel, no `JarvisApp` fields. The
  managers' `event_rx` is consumed by nothing.
- **No signaling transport binding.** `VoiceSignal` is defined but never
  serialized onto the relay. There is no `VoiceFrame`/`SignedVoiceFrame` on the
  wire (the pair feature's `SignedPairFrame` is the template — §3).
- **No TURN server.** The relay is an **app-layer message forwarder**, not a
  media relay (§2). Symmetric-NAT peers have **no** media path today.
- **No audio plumbing.** No `cpal` capture/playback, no Opus, no echo
  cancellation / VAD wired to `VoiceManager::set_speaking`.
- **No `VoiceConfig` in `jarvis-config`.** `CollabConfig`
  (`jarvis-config/src/schema/collab.rs`) exists for pair; there is no
  `[voice]` / `[screen_share]` schema section.

### What ALREADY works for read-only screen spectating (and why the new part is narrow)

Read-only screen *spectating* is **already shipped** without WebRTC, via two
existing paths — this materially shrinks what voice-chat must add:

1. **Workspace capture → relay Broadcast → chat LiveStream.**
   `app_state/workspace_capture/` captures the Jarvis window as a **JPEG**
   (`windows.rs` uses **xcap** → Windows Graphics Capture/DXGI; `macos.rs`/
   `linux.rs` analogous), downscaled to 640×360 @ JPEG q35,
   `chat_stream_handlers.rs` pushes a frame every 350 ms as a base64 data URL
   over the relay's **Broadcast** session (host→spectators, `session.rs`
   `BroadcastSession`), and the chat panel renders it as a `LiveStream`. This is
   a low-FPS, low-res, host→viewer slideshow — but it works **today**, with no
   TURN, because it is just relayed application messages.

**So the NEW part is specifically: interactive, HD, low-latency WebRTC media**
— smooth (15–30 fps) video, audio, and bidirectional/peer media. The
JPEG-over-Broadcast path remains the always-available fallback when WebRTC/TURN
is unavailable or disabled (§4 M3, §5).

### Transport precedents to fork (same as pair)

- **Relay Room** (`jarvis-relay/src/session.rs` `RoomSession`,
  `register_room`/`room_targets_excluding`, `MAX_ROOM_MEMBERS=32`;
  `connection.rs` `room_hello`→`room_ready`→`member_joined/left/count`,
  opaque all-but-sender fan-out). This is the natural **signaling** channel.
- **`SignedPairFrame`** (`app_state/ws_server/pair_protocol.rs`) — the
  end-to-end ECDSA signing/anti-replay envelope to **clone** for signed
  signaling.
- **`RelayEnvelope` + `RelayCipher`** (`ws_server/relay_protocol.rs`,
  `crypto_bridge.rs`) — AES-256-GCM opaque outer envelope, unchanged.
- **`app_state/pair.rs`** — the worker pattern (drain `event_rx` → frame →
  sign → encrypt → Room; route inbound back over a sync mpsc; reuse
  `self.tokio_runtime`).

---

## 1. Architecture

Two planes, cleanly separated. **Signaling rides the existing relay; media does
not.**

```
  ┌──────────────────────── SIGNALING PLANE (exists / reuse) ───────────────────────┐
  │  relay Room  (jarvis-relay RoomSession, opaque AES-GCM RelayEnvelope)            │
  │   room_hello → room_ready → member_joined/left/count                            │
  │   carries SignedVoiceFrame { SdpOffer | SdpAnswer | IceCandidate | join | ... }  │
  └─────────────────────────────────────────────────────────────────────────────────┘
              │ SDP/ICE exchange establishes ↓
  ┌──────────────────────── MEDIA PLANE (NEW — WebRTC) ─────────────────────────────┐
  │  RTCPeerConnection (DTLS-SRTP)   peer ⇄ peer  (full mesh, ≤~5 for audio)         │
  │   audio: cpal capture → Opus encode → RTP/SRTP → … → Opus decode → cpal playback│
  │   video: xcap/screen frame → VP8/H264 encode → RTP/SRTP (interactive HD share)  │
  │   NAT traversal: STUN (host/srflx) + **TURN (coturn) relay candidate** ← §2     │
  └─────────────────────────────────────────────────────────────────────────────────┘
```

### 1.1 Media-plane integration choice (the load-bearing decision)

Three options (the PLAN's Phase 8 step 1 listed all three). Recommendation:
**start with Option C (WebView/WebRTC) for the smallest working slice, design
the signaling so Option A (`webrtc-rs`) can replace it later without touching
the relay or the signed-frame protocol.**

| Option | What | Pros | Cons | Verdict |
|--------|------|------|------|---------|
| **A — `webrtc-rs`** (pure-Rust pion port) | Native `RTCPeerConnection` in Rust; pair Opus via `audiopus`/`opus`, capture via `cpal`. | Single binary, no JS, full control of media; signaling stays in Rust next to `SignedVoiceFrame`. | `webrtc-rs` is the *least* mature of the three (echo cancellation, device hotplug, codec breadth are weak — see §5); large dep tree; we own all the AEC/jitter pain. | **Target end-state**, M2+. |
| **B — `libwebrtc` FFI** | Bind Google's libwebrtc. | Battle-tested media + built-in AEC/NS/AGC. | Enormous native build, cross-platform packaging nightmare, FFI surface; contradicts "single Rust binary." | **Rejected** (ops cost). |
| **C — WebView/WebRTC** | Run the `RTCPeerConnection` **inside the wry WebView** (WebKit/WebKitGTK/WebView2 all ship a real WebRTC stack with mature AEC/NS/AGC). Rust only relays signaling between the WebView's JS and the relay Room. | Fastest to a working call; **free, mature echo cancellation/NS/AGC and Opus**; getUserMedia handles mic capture + device UI; reuses the panel-IPC pattern we already have. | Media lives in the WebView (must keep identity/keys in Rust, §3); screen *capture* of native panes still needs the Rust `xcap` path piped in as a track; per-platform WebView WebRTC quirks. | **Recommended first slice (M1).** |

**Why C first:** the single hardest part of voice (echo cancellation, §5) is
*solved for free* by the WebView's WebRTC stack, and getUserMedia gives us mic
permission UX and device selection without writing `cpal` device code. We get a
working 1:1 call fastest, prove the **signaling + TURN** infrastructure (the
genuinely new/risky parts), and only then decide whether the native
`webrtc-rs` end-state is worth it. The relay and `SignedVoiceFrame` protocol are
**identical** either way, so the migration is media-plane-only.

### 1.2 Signaling over the relay Room (reuse, do not rebuild)

The relay Room is the natural signaling channel and is **already proven** by
pair programming. Signaling messages are tiny, bursty, and need exactly the
Room's opaque all-but-sender fan-out + free presence (`member_joined/left`).

- **Outer envelope:** reuse `RelayEnvelope` + `RelayCipher` (AES-256-GCM,
  room-derived key from `session_id` via `derive_room_key`) **unchanged** — the
  relay stays fully opaque, exactly as for pair.
- **Inner payload:** a new `VoiceFrame` enum (new
  `ws_server/voice_protocol.rs`, modeled on `pair_protocol.rs`,
  `#[serde(tag="type", rename_all="snake_case", deny_unknown_fields)]`):
  - `join{display_name, pubkey}` — announce presence + identity into the room.
  - `sdp_offer{to, sdp}` / `sdp_answer{to, sdp}` — directed (the room fans out
    to all; recipients filter on `to`, exactly like the existing `poke` pattern
    in `PresenceFrame`).
  - `ice_candidate{to, candidate, sdp_mid, sdp_m_line_index}`.
  - `mute{muted}` / `deafen{deafened}` / `speaking{speaking}` — UI state
    (drives `VoiceManager::set_*`).
  - `leave{}`.
  - **(screen share, M4)** `share_start{quality, source_label}` /
    `share_stop{}` reuse the same enum (or a sibling `ScreenShareFrame`); the
    SDP/ICE variants are shared since the media negotiation is identical.
  - The existing `VoiceSignal`/`ScreenShareSignal` (`protocol.rs`) become the
    inner SDP/ICE payload carried by these frames — **they already match.**
- **Topology:** **full-mesh** for audio (each member ⇄ each member, N·(N−1)/2
  peer connections). Fine to ~5 participants; beyond that, an SFU is required
  (§5, explicitly out of scope — deferred). Screen share is **1 host →
  N viewers** (a star, like Broadcast), which is cheaper than mesh.

### 1.3 Audio pipeline (Option A / native path; Option C gets this free)

- **Capture/playback:** `cpal` (cross-platform; the PLAN's chosen crate) →
  default input device → 48 kHz mono frames.
- **Codec:** **Opus** (`audiopus`/`opus`), 20 ms frames, ~24–32 kbps, FEC + DTX
  (discontinuous transmission so silence sends nothing).
- **VAD:** energy/threshold (or WebRTC VAD) → drives
  `VoiceManager::set_speaking` → `speaking` frame → roster "talking" ring.
- **Echo cancellation (AEC):** the hard part (§5). Native path needs `webrtc-rs`
  AEC or `speexdsp`; if unavailable, **enforce push-to-talk (PTT)** — the PLAN's
  fallback ("Enforce PTT-only if native AEC unavailable"). Option C inherits the
  WebView's AEC and sidesteps this entirely for M1.

### 1.4 Interactive / HD screen share (the genuinely new media)

Read-only spectating already works (the JPEG/Broadcast path, §0). The new part
is **smooth HD**:
- **Source:** reuse `workspace_capture` (`xcap` on Windows; the existing crop/
  monitor-pick logic in `windows.rs`) but instead of JPEG-per-350 ms, feed raw
  RGBA frames into a **WebRTC video track** (VP8/VP9/H264 encode, 15–30 fps,
  `ShareQuality` ladder already defines the resolution/fps targets).
- **In Option C**, the captured frames are handed to the WebView as a
  `MediaStreamTrack` (canvas/`captureStream` or an inserted track); in Option A,
  encoded directly via `webrtc-rs` video tracks.
- **Window-only default** (§3 security): share a *specific* source, never the
  whole desktop by default; persistent "SHARING" indicator.

---

## 2. Infrastructure — the TURN server (the hard dependency)

**This is the section to read before deciding to build.** Everything else can
be written in-repo; this cannot.

### 2.1 Why the relay is NOT enough

The Jarvis relay (`jarvis-relay`, Railway-hosted, see `docs/manual/09-networking.md`)
is an **app-layer WebSocket message forwarder**. It moves small opaque JSON/
ciphertext frames between room members and **never inspects payloads**. It is
**not** a media relay and must not become one:

- It speaks WebSocket text frames, not RTP/SRTP/UDP.
- It is sized and priced for tiny presence/chat/signaling messages (a few
  KB/s/room), not for **continuous audio (~32 kbps/peer) and HD video
  (1–5 Mbps)**. Routing media through it would blow its bandwidth budget and
  add a latency hop it was never designed for.
- WebRTC media is **peer-to-peer**: it wants a direct UDP path, falling back to
  a TURN relay only when NAT/firewall blocks the direct path.

So WebRTC needs two server roles the relay does **not** fill:

1. **STUN** — lets a peer discover its public (server-reflexive) address. Cheap,
   stateless, can use free public STUN (`stun.l.google.com:19302`) for
   experiments. **Not sufficient alone.**
2. **TURN** — a **media relay** for the ~10–25% of peer pairs that are behind
   symmetric NATs / restrictive firewalls and cannot connect directly. TURN
   actually carries the audio/video bytes when P2P fails. **Without TURN, a
   meaningful fraction of calls silently fail to connect** (one-way or no audio)
   — and there is no graceful in-band fallback because the relay can't carry
   media either.

**Conclusion: TURN (coturn) is a mandatory, standalone piece of infrastructure
that must be deployed and paid for. It is the single thing that turns this from
"writable in the repo" into "requires standing up a server."**

### 2.2 What to deploy: coturn

`coturn` is the de-facto open-source STUN/TURN server.

- **Deployment options:**
  - **(a) Alongside the Railway relay** — a second Railway service (or the same
    project) running the `coturn` Docker image. Simplest ops continuity (same
    dashboard/billing as the relay). **Caveat:** TURN needs **UDP** and a wide
    **relay port range** (typically 49152–65535); confirm the PaaS allows UDP +
    arbitrary port ranges. Railway/Heroku-class PaaS are frequently **TCP/HTTP-
    only**, which makes them a **poor fit for TURN** — likely forcing TURN-over-
    TCP/TLS (port 443) only, which works but is slower and loses UDP. **Verify
    before committing to (a).**
  - **(b) A dedicated small VPS** (Hetzner/DigitalOcean/Fly.io with UDP) running
    `coturn` directly — the **recommended** path because TURN wants raw UDP, a
    public IP, and a port range that PaaS platforms resist. A $5–10/mo VPS
    handles a small community.
- **Config essentials:** `lt-cred-mech` with **short-lived, HMAC-signed
  credentials** (NOT static username/password — see §3), `realm`, `min-port`/
  `max-port` relay range, `fingerprint`, TLS cert for `turns:` (TURN over TLS/443
  for restrictive networks), `no-loopback-peers`, `no-multicast-peers`,
  `denied-peer-ip` for internal ranges (SSRF hardening).
- **Client config plumbing:** desktop needs `iceServers` (STUN + TURN URL +
  ephemeral credential). Add to `jarvis-config` (§schema below). The desktop
  fetches/derives a **time-limited TURN credential** (HMAC of `username =
  expiry:userid` with a shared secret) — coturn's standard REST-credential
  scheme — rather than shipping a static secret.

### 2.3 Cost & ops implications (be explicit)

- **Bandwidth is the cost driver.** TURN-relayed media is **double bandwidth**
  (in + out through the server). Audio is cheap (~32 kbps/peer); **HD screen
  share is expensive** (1–5 Mbps/viewer). A handful of simultaneous HD shares
  through TURN can saturate a small VPS NIC and run up egress bills.
- **Only relayed traffic costs** — most calls connect P2P (STUN only) and never
  touch TURN. TURN cost ∝ (fraction of NAT-blocked pairs) × (media bitrate) ×
  (minutes). For a tiny community this is plausibly a few dollars/month; it does
  **not** scale free.
- **Ops burden:** a long-lived server to patch, monitor, cert-rotate (TLS), and
  firewall (TURN is an SSRF/relay-abuse target — lock down peer IP ranges and
  require auth, §3). This is **net-new operational surface** the project does
  not have today.
- **Managed alternative:** Twilio/Cloudflare/metered.ca/Xirsys sell TURN as a
  service (per-GB). Removes ops burden, adds per-GB billing and a third-party
  dependency. **Reasonable for a first slice** to avoid running coturn while
  validating demand — then self-host coturn if it sticks.

**Decision gate:** Do not start M1 until the owner has chosen (self-host coturn
on a VPS) **or** (managed TURN) **or** (accept that ~10–25% of calls won't
connect and ship STUN-only as an explicit "best-effort, same-network/open-NAT"
experiment). The third is the only $0 path and should be labeled as such.

---

## 3. Security

Map onto the project's existing identity/signing model — the same model the C2
pair feature hardened to M3. The PLAN's **9-point Phase-8 checklist** is the
spec; each point is mapped to a concrete mechanism below.

### 3.1 Reuse the pair signed-frame model for signaling

Signaling is a **bearer-secret Room** (anyone who knows `session_id` can connect
to the relay room): the room key is **confidentiality-only**, and the real
boundary is a **per-member ECDSA signature on every frame** — identical to
`SignedPairFrame` (`pair_protocol.rs`). Clone it as **`SignedVoiceFrame`**:

- Canonical, domain-separated signing bytes with a **fresh domain tag**
  (`jarvis-voice-sig-v1` — distinct from `jarvis-pair-sig-v1` so a voice
  signature can never be cross-presented to the pair/relay/theme signers that
  share the one ECDSA identity key).
- Bind `session_id | member_id | pubkey | frame_type | epoch | seq | frame_json`;
  `deny_unknown_fields`; per-member `(epoch, seq)` anti-replay; TOFU
  `member_id → pubkey` pinning; reuse the crypto IPC
  (`webview_bridge/crypto_handlers.rs` sign/verify) and the per-app identity key
  (`jarvis-social/src/identity.rs`).
- **Invite = capability + identity pin**, exactly like pair:
  `base64url(session_id ":" host_pubkey)` so a joiner pins the host out-of-band.

### 3.2 The PLAN's 9-point Phase-8 checklist → mechanism

1. **[P0] Authenticate WebRTC signaling.** ✅ `SignedVoiceFrame` (§3.1): every
   SDP offer/answer/ICE candidate is signed; an unsigned/unverifiable signaling
   frame is dropped fail-closed. Only verified room members can exchange SDP.
2. **[P0] Screen-sharing safety.** Default to **window/source-specific** sharing
   (never whole-desktop by default); **persistent "SHARING" indicator** in the
   panel + status bar; **warn before sharing a terminal pane** (secrets leak
   risk — Jarvis *is* a terminal); option to exclude panes. The
   `workspace_capture` crop logic already supports a bounded region.
3. **[P1] DTLS fingerprint verification.** Carry the **DTLS fingerprint inside
   the signed SDP** (it already lives in the SDP); since the SDP is covered by
   the ECDSA signature and the signer's pubkey is identity-pinned, the
   fingerprint is bound to the verified peer identity — defeats MITM at the
   media layer.
4. **[P1] Mic-active visual indicator.** Always-visible mic indicator whenever
   capture is live; **log every capture start/stop** (`tracing`, at info).
5. **[P2] Zero frame buffers.** Wipe screen-capture buffers after encode;
   consider `mlock`/disable core dumps during a share. (Best-effort; document
   limits — full guarantees are hard in a GC'd WebView path.)
6. **[P2] DTLS-SRTP on all media.** WebRTC mandates DTLS-SRTP; **never** accept
   an unencrypted negotiation; alert on negotiation failure. (Free in both
   Option A and Option C.)
7. **[P3] Opus frame validation.** Validate frame size/duration before decode;
   panic-safe decoder (Option A only; Option C's WebView handles this).
8. **[P3] Echo cancellation / PTT.** Native AEC where available; **enforce PTT
   if AEC is unavailable** (§5). Option C inherits WebView AEC.
9. **[P3] Call-recording consent.** No recording in scope; if ever added,
   require all-party notification.

### 3.3 Inherited residual

Same as pair: the relay assigns the in-room **slot** from a self-asserted
`member_id` (unsigned relay layer), so a `session_id`-knower can churn/deny a
**slot** (denial only, never content — every honored frame is signed). For voice
this means an attacker who knows the room id can **disrupt signaling** (deny a
call setup) but **cannot inject media or impersonate a peer** (DTLS fingerprint
+ signed SDP bind the media to the identity). Fully closing it needs the relay
to bind slots to identities (relay protocol change + redeploy) — out of scope,
documented.

---

## 4. Milestones (smallest working slice first)

Each milestone names files, an effort tag (S/M/L), and the **infra** it needs.
Default everything **off** (`voice.enabled=false`).

### M0 — Wire the coordination plane + config (no media yet)  · infra: none

Make the already-built managers reachable and configurable.
- Enable the feature in the app: `jarvis-app/Cargo.toml` →
  `jarvis-social = { features = ["experimental-collab"] }` **(S)**.
- New `jarvis-config/src/schema/voice.rs`: `VoiceConfig { enabled=false,
  max_participants=5, push_to_talk=true, ice_servers=[], turn_secret=None }` +
  `ScreenShareConfig`; register in `schema/mod.rs` + field on `JarvisConfig` +
  default test (model `schema/collab.rs`) **(M)**.
- New `app_state/voice.rs` (model `app_state/pair.rs`): `start_voice`/
  `poll_voice`, `Arc<VoiceManager>`, worker draining `VoiceEvent` → signaling
  frame, reusing `self.tokio_runtime`; `JarvisApp` fields in `core.rs`; register
  in `app_state/mod.rs`, `polling.rs`, `event_handler.rs` **(M)**.
- New `ws_server/voice_protocol.rs` (`VoiceFrame` + `SignedVoiceFrame`, fork
  `pair_protocol.rs`) **(M)**.
- Outcome: rooms can be created/joined and signaling frames flow + verify over
  the relay — **but no audio**. Testable with two instances exchanging
  `join`/`speaking` frames.

### M1 — 1:1 audio call (WebView/WebRTC, Option C)  · infra: STUN (free) — **TURN strongly recommended**

The smallest *useful* slice.
- New `ws_server/voice_room_client.rs` (fork `pair_room_client.rs`: `room_hello`,
  inner `SignedVoiceFrame`, sign-once seam) **(L)**.
- New `assets/panels/voice/index.html` — a WebView panel running
  `RTCPeerConnection` + getUserMedia; **all signaling via panel↔Rust IPC** (Rust
  owns the relay socket + identity/keys; the WebView never holds the identity
  key). IPC kinds `voice_start`/`voice_join`/`voice_leave`/`voice_signal_out`
  down, `voice_signal_in`/`voice_status` up **(L)**.
- IPC + handlers: `ipc_dispatch.rs` ALLOWED_IPC_KINDS += the voice kinds + arms;
  new `webview_bridge/voice_handlers.rs` (model `pair_handlers.rs`) **(M)**.
- `iceServers` from config; AEC/NS/Opus inherited from the WebView **(free)**.
- Mic indicator + capture start/stop logging (checklist #4) **(S)**.
- Outcome: two users hear each other if a direct or TURN path exists. **Without
  TURN, symmetric-NAT pairs fail — this is the milestone that forces the §2
  decision.**

### M2 — Multi-party audio (full mesh ≤5)  · infra: TURN required

- `VoiceManager::join_room` already returns existing participant ids; the panel
  opens a peer connection to each → mesh. Roster UI (chips, speaking ring,
  mute/deafen) driven by `mute`/`deafen`/`speaking` frames **(M)**.
- PTT path (checklist #8) when AEC weak; `max_participants` cap enforced **(S)**.
- Outcome: a small voice room. Mesh CPU/bandwidth makes ~5 the ceiling (SFU =
  future, §5).

### M3 — Optional native media (webrtc-rs, Option A)  · infra: same TURN

- New `jarvis-social` deps behind feature: `webrtc`, `cpal`, `opus` **(L)**.
- Replace the WebView media plane with native `RTCPeerConnection` + `cpal`
  capture/playback + Opus; **signaling, relay, and `SignedVoiceFrame` unchanged**
  **(L)**. Gated behind a config/build flag; WebView path stays as the default/
  fallback. **Only pursue if the single-binary/no-WebView goal justifies the AEC
  and maturity cost (§5).**

### M4 — Interactive HD screen share  · infra: TURN (HD = the expensive case)

- Feed `workspace_capture` (`xcap`) frames into a WebRTC **video track**
  (Option C: `MediaStreamTrack` into the WebView; Option A: native video track),
  host→viewers star topology, `ShareQuality` ladder **(L)**.
- Security: window-only default, "SHARING" indicator, terminal-pane warning
  (checklist #2) **(M)**.
- **Keep the JPEG-over-Broadcast LiveStream (§0) as the always-on fallback** when
  WebRTC/TURN is unavailable **(S)**.

### M5 — Quality & polish  · infra: none new

- Better VAD, jitter buffer tuning, device selection UI, network-quality
  indicator, graceful TURN-failure → "couldn't connect" UX (not silent failure)
  **(M)**.

---

## 5. Risks (and what Dylan's deferral got right)

1. **WebRTC-in-Rust maturity (`webrtc-rs`).** The PLAN deferred voice for
   exactly this ("WebRTC immaturity"). `webrtc-rs` lacks robust echo
   cancellation, AGC, and device hotplug that libwebrtc/browsers have spent a
   decade on. **Mitigation:** Option C (WebView WebRTC) sidesteps it for M1–M2;
   treat native (M3) as optional. **Dylan was right** that betting v1 on
   `webrtc-rs` would have been a schedule and quality risk.
2. **TURN cost & ops (the real blocker).** Net-new server to run, secure
   (SSRF/relay-abuse target), cert-rotate, and pay egress for — and HD share
   makes egress nontrivial. **Mitigation:** start with managed TURN or
   STUN-only-experiment; self-host coturn only once demand is proven.
   **Dylan was right** that this is infrastructure the project didn't want to
   take on for v1.
3. **Audio AEC complexity.** Echo cancellation is the perennially hard part of
   voice; getting it wrong = unusable feedback. **Mitigation:** lean on WebView
   AEC; enforce PTT as the guaranteed-safe fallback (checklist #8). This single
   risk is the strongest argument for Option C over Option A.
4. **Cross-platform capture.** Mic capture (`cpal`) and screen capture (`xcap`)
   have per-OS quirks (Wayland screen-capture restrictions, macOS TCC
   mic/screen permission prompts, Windows Graphics Capture nuances). The
   `workspace_capture` path already absorbs *some* of this for screens; audio is
   new. **Mitigation:** getUserMedia (Option C) handles mic permission UX per
   platform for free.
5. **Mesh scaling ceiling.** Full-mesh audio is O(N²) connections/bandwidth;
   ~5 participants is the practical cap. A real multi-user room needs an **SFU**
   (selective forwarding unit) — a *much* larger media-server investment than
   TURN. **Explicitly out of scope.** "Multi-user" here means small rooms.
6. **Security surface returns.** The PLAN feature-flagged voice partly to
   "reduce attack surface." Re-enabling adds mic/screen capture, a media plane,
   and a public TURN server. **Mitigation:** the §3 signed-signaling model +
   default-off + window-only-share + indicators keep it bounded — but it is
   genuinely more surface than chat.

**Net: Dylan's deferral was correct for v1.** None of these risks have
*disappeared*; what has improved is that (a) the coordination plane and
signaling types are already written, and (b) Option C lets us avoid the two
worst risks (`webrtc-rs` maturity + AEC) for the first slice. The one thing the
deferral was *right about that remains fully unaddressed* is the **TURN
infrastructure** — that decision is still entirely ahead of us and gates
everything.

---

## 6. Decision summary (for the owner)

- **Build cost in-repo is moderate**, not huge: the managers, signaling types,
  and the entire pair-feature transport/signing template already exist to fork.
  M0–M2 is mostly *wiring + one WebView panel*.
- **The blocker is infrastructure, not code:** a **TURN server (coturn)** must be
  stood up and paid for, or a managed TURN service subscribed to. Without it,
  ~10–25% of calls won't connect and there is **no in-band fallback** (the relay
  cannot carry media).
- **Recommended path if you proceed:** (1) pick managed TURN (or a $5–10/mo UDP
  VPS running coturn) — §2.3 decision gate; (2) build M0→M1 with Option C
  (WebView WebRTC) to get a working 1:1 call with free AEC; (3) reassess before
  M3 (native `webrtc-rs`) and M4 (HD share egress cost).
- **$0 path exists but is honest about its limit:** STUN-only, labeled
  "best-effort / open-NAT only" — useful for same-network or well-behaved-NAT
  testing, not a reliable product.

---

## Source Map

| Concern | Source |
|---------|--------|
| Voice room state machine (built) | `jarvis-rs/crates/jarvis-social/src/voice/{manager,types}.rs` |
| Screen-share session state (built) | `jarvis-rs/crates/jarvis-social/src/screen_share/{manager,types}.rs` |
| SDP/ICE signaling types (built) | `jarvis-rs/crates/jarvis-social/src/protocol.rs` (`VoiceSignal`/`ScreenShareSignal`) |
| Feature gate | `jarvis-rs/crates/jarvis-social/src/lib.rs`, `Cargo.toml` (`experimental-collab`) |
| Relay Room (signaling transport) | `jarvis-rs/crates/jarvis-relay/src/{session,connection}.rs` |
| Signed-frame template to fork | `jarvis-rs/crates/jarvis-app/src/app_state/ws_server/pair_protocol.rs` |
| Room-client worker template | `jarvis-rs/crates/jarvis-app/src/app_state/{pair.rs, ws_server/pair_room_client.rs}` |
| Screen capture (xcap/JPEG, reuse for HD source) | `jarvis-rs/crates/jarvis-app/src/app_state/workspace_capture/` |
| Existing JPEG-over-Broadcast LiveStream (fallback) | `jarvis-rs/crates/jarvis-app/src/app_state/webview_bridge/chat_stream_handlers.rs` |
| Crypto / identity (reuse for signing) | `webview_bridge/crypto_handlers.rs`, `jarvis-social/src/identity.rs` |
| Config schema template | `jarvis-rs/crates/jarvis-config/src/schema/collab.rs` |
| Original roadmap (Phase 8 + 9-pt checklist) | `dev/_archive/jarvis-rs/PLAN_2026-02-27.md` |
| Pair design record (security model precedent) | `dev/plans/c2-pair-programming.md` |
| Relay/networking transport context | `docs/manual/09-networking.md`, `docs/manual/12-collaboration.md` |
