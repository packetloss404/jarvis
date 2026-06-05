# Jarvis Roadmap — remaining & incomplete work

The revival is shipped (see `CHANGELOG.md`). This is the consolidated list of what's
**not** finished — known residuals, deferred hardening, and forward features. Surfaced
by the final code/dev review and the design docs.

## Security hardening

- **Relay member-id slot binding** *(documented residual — denial-only)*. The relay's
  in-room `member_id` is client-asserted; a session-id holder can hijack/flood a slot
  (DoS, **not** impersonation — pair frames are E2E-signed so content can't be forged).
  Basic mitigations landed (member-id length/charset validation + a per-room member cap).
  The full fix — binding the relay slot to a signed identity in `room_hello` — is deferred
  because it's a cross-cutting protocol change touching every Room client (chat, presence,
  pair) plus a redeploy. (`jarvis-relay/src/{connection,session}.rs`.)
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
