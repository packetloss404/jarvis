# dev/ — Development documentation

Internal design notes and planning for Jarvis. Not user-facing — the user/contributor
docs live in [`../docs/manual/`](../docs/manual/) and the root `README.md` / `ARCHITECTURE.md`.

## Layout

| Path | What it is |
|------|------------|
| [`ROADMAP.md`](ROADMAP.md) | **Active.** Remaining / incomplete work and the forward roadmap. Start here for "what's left." |
| [`plans/`](plans/) | **Active** design docs for features. Currently: [`c2-pair-programming.md`](plans/c2-pair-programming.md) (the collaborative-terminal design + security model; M1–M3 shipped, kept as the design record). |
| [`_archive/`](_archive/) | **Historical.** Superseded analyses and dated plans — kept for the record, not current guidance. |

## What's in `_archive/`

- [`pathforward/`](_archive/pathforward/) — the strategic codebase analyses (Gemini / GPT / MiniMax + a synthesized `finalfindings`) that diagnosed the inherited project and guided the revival. Now historical: the revival they recommended has shipped.
- `jarvis-rs/PLAN_2026-02-27.md` — the **Rust-rewrite blueprint**. This is the plan that actually shipped (its crate map and security model match the current tree); retained because its v2 roadmap (voice chat / screen sharing, Phase 8) and platform/security notes still inform future work.
- The other dated `PLAN_*.md` files target the original macOS Python + Swift/Metal stack (now archived at the `legacy-archive` git tag) and are purely historical.

The original macOS prototype itself is not in the tree — it's preserved at the `legacy-archive` git tag.
