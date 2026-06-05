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
- **Enable collaboration by default.** `collab.enabled` stays `false` until the slot
  binding above lands and a real multi-user test passes. Pair sessions are authenticated
  (M3), but the feature is experimental.
- **macOS native notifier.** `jarvis-platform/src/notifications.rs` shells out to
  `osascript` (argument-escaped, but) — swap to a native crate (`notify-rust`).

## Testing

- **Real 2-user / multi-machine test** of chat, presence, and pair programming (everything
  is single-instance + unit-tested today; two distinct identities need two machines).

## Features to complete

- **Voice input.** `jarvis-ai/src/whisper.rs` (Whisper STT client) is present but unwired —
  needs mic capture (e.g. `cpal`) + push-to-talk. Part of the broader v2 **Voice Chat &
  Screen Sharing** roadmap (`dev/_archive/jarvis-rs/PLAN_2026-02-27.md` Phase 8, feature-flagged).
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
