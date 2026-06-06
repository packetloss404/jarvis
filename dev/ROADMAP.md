# Jarvis Roadmap — remaining & incomplete work

The revival is shipped (see `CHANGELOG.md`). This is the consolidated list of what's
**not** finished — known residuals, deferred hardening, and forward features. Surfaced
by the final code/dev review and the design docs.

## Security hardening

- **Relay member-id slot binding** *(LANDED — residual: first-mover pin on
  non-fingerprinted ids)*. Every Room client now SIGNS its `room_hello` (ECDSA P-256 over
  domain-tagged canonical bytes); the relay verifies the signature, enforces a per-slot
  strictly-monotonic nonce (anti-replay), and TOFU-pins `(session_id, member_id) → pubkey`,
  so a self-asserted `member_id` can no longer squat or evict a slot without the matching
  key. (`jarvis-relay/src/{room_auth,connection,protocol}.rs`.)
  **Residual (first-mover pins):** the pair (`random_alnum`) and presence (raw UUID)
  `member_id` formats embed NO pubkey relationship, so the first valid signer to present
  such an id PINS it for the room's lifetime — a later, differently-keyed claimant is then
  refused, but a malicious *first* mover could pre-pin an id it doesn't "own". For the chat
  clients, whose id is `fingerprint(pubkey).hex + "." + userId`, the relay additionally
  checks the fingerprint prefix against `fingerprint(carried_pubkey)`, closing first-mover
  squatting for those ids. Fully closing it for the non-fingerprinted formats would require
  deriving their `member_id` from the pubkey (changing the pair/presence id formats and the
  `member_id ↔ user_id` linkage rosters/DM-channels depend on) and is deferred. The
  exposure is denial-only — pair frames are E2E-signed, so content can't be forged.
- **Enable collaboration by default** *(DONE)*. `collab.enabled` now defaults to `true`:
  the slot binding above landed + deployed (relay redeployed to Railway 2026-06-05) and a
  live signed-hello smoke test confirmed the relay accepts signed / rejects unsigned + bogus
  hellos. Pair sessions are authenticated (M3 signed frames). Still feature-flagged (set
  `collab.enabled = false` to disable). A full 2-user / multi-machine pair-programming run
  (see Testing) is still pending — the transport + auth are verified, the end-to-end driver/
  navigator flow across two real machines is not yet exercised live.
- **macOS native notifier.** `jarvis-platform/src/notifications.rs` shells out to
  `osascript` (argument-escaped, but) — swap to a native crate (`notify-rust`).

## Testing

- **Real 2-user / multi-machine test** of chat, presence, and pair programming.
  - *Relay transport — VERIFIED (2026-06-05).* A two-distinct-identity test against the
    **live Railway relay** passed 10/10: both signed joins admitted, presence roster +
    `member_count` propagate both ways, bidirectional opaque message fan-out (no member gets
    its own frame), and the slot binding holds — a third key cannot evict an active member's
    slot, and that member stays live. This covers the N:N transport chat/presence/pair all share.
  - *Still pending — the client app flow.* The desktop pair-programming driver/navigator state
    machine (and the chat/presence UIs) across **two real app instances on two machines** is
    not yet exercised live; that's the remaining gap (needs Ian's second machine).

## Features to complete

- **Voice input** *(DONE)*. Push-to-talk STT is wired: hold the PTT key (default `F4`) →
  `cpal` mic capture → `jarvis-ai/src/voice` WAV encode → `whisper.rs` (OpenAI Whisper) →
  the transcript lands in the assistant input textarea for review (never auto-sent).
  Off by default (`[voice].enabled`, needs `OPENAI_API_KEY`). The broader v2 **Voice Chat &
  Screen Sharing** (peer-to-peer audio over the relay, needs TURN/coturn) is still future
  work — see `dev/plans/voice-chat.md`.
- **Chat history persistence.** Chat history is currently ephemeral (client-side, capped,
  wiped on refresh). A lightweight server-side store on the relay is optional future work.
- **Boot splash timing.** `jarvis-app/src/boot/sequence.rs` has unused timing methods —
  either finish wiring the splash progress or trim `BootSequence` to what's used.

## Tooling & maintenance

- **`jarvis --screenshot` flag.** Add a headless capture using the app's own Windows
  Graphics Capture code (the GPU-composited window can't be grabbed by GDI screenshot
  tools) — would make README/marketing screenshots reproducible.
- **Mobile maintenance.** `jarvis-mobile` runs on bleeding-edge Expo/RN; scrub the residual
  cosmetic `supabase` strings (backend is the relay Room now) and keep deps on a supported SDK.
- **Windows identity-key permissions.** Ensure the persistent ECDSA identity key file is
  ACL-restricted on Windows (it is on Unix).

## Done (for reference — see CHANGELOG.md)

Multi-provider AI · agentic tools + approval gate · relay Room backend (chat/presence off
Supabase) · authenticated pair programming (M1–M3) · games→plugins · Windows capture ·
Railway deploy · mobile migration · legacy archived · MIT license · docs/manual rebuilt.
