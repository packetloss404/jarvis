# Jarvis repository architecture

This document explains **what lives where** and **which tree is authoritative** for new work. It is the short map; the [manual](docs/manual/README.md) (under **`docs/`**, with the marketing site) is the long-form reference. **Development-only** notes and analysis live under **`dev/`** (including pathforward).

---

## Primary product: `jarvis-rs/`

The **cross-platform desktop application** is the **`jarvis-rs/`** Cargo workspace (Rust, `wgpu`, `wry`). This is the **developed app**: all new features, UX changes, and bug fixes belong here. (The old macOS Python + Swift/Metal stack is archived at the `legacy-archive` git tag — see below.)

- **Build:** `cd jarvis-rs && cargo build --release`
- **Bundled UI:** [`jarvis-rs/assets/panels/`](jarvis-rs/assets/panels/) — single source of truth for panel HTML/CSS/JS and built-in games.
- **Crates:** app shell, config, platform, tiling, renderer, AI, social, webview, relay binary — see [docs/manual/01-architecture.md](docs/manual/01-architecture.md).

Do not maintain duplicate panel copies at the repository root.

---

## Legacy macOS stack (archived)

The original **macOS-only** experience (Python orchestration + Swift/Metal UI, under the former `legacy/` tree, plus its `scripts/` and packaging assets) has been **removed from the working tree**. It is fully superseded by `jarvis-rs/` and preserved in git history at the **`legacy-archive`** tag:

```bash
git checkout legacy-archive   # browse the legacy stack as it last existed
```

New work happens exclusively in `jarvis-rs/`.

---

## Mobile companion: `jarvis-mobile/`

[`jarvis-mobile/`](jarvis-mobile/) is the **React Native** companion. It should stay a **thin client** (pairing, remote control, chat) and should not absorb desktop complexity moved out of `jarvis-rs/`.

---

## Shared / supporting directories

| Path | Role |
|------|------|
| [`docs/`](docs/) | **Website** and published docs: marketing pages (`docs/index.html`), [technical manual](docs/manual/README.md) |
| [`dev/`](dev/) | **Development documentation** only: pathforward analysis (`dev/pathforward/`), archived planning and plugins notes (`dev/_archive/`), and similar internal write-ups |
| [`relay/`](relay/) | Deployment scripts for relay infrastructure (not the `jarvis-relay` crate source) |
| [`resources/`](resources/) | Built-in theme YAML files under `resources/themes/`, loaded at runtime by `jarvis-rs` |
| [`.github/workflows/`](.github/workflows/) | Rust CI and release workflows |

---

## What “Rust-first” means in practice

1. **Default clone experience:** Read this file and `README.md`, then `cd jarvis-rs`.  
2. **Panel and game HTML:** Edit under `jarvis-rs/assets/panels/` only.  
3. **All product work** lands in `jarvis-rs/`; the old macOS stack is archived at the `legacy-archive` tag and is no longer maintained in-tree.  

---

## Further reading

- [CONTRIBUTING.md](CONTRIBUTING.md) — builds, tests, PR hints  
- [dev/pathforward/finalfindings.md](dev/pathforward/finalfindings.md) — strategic analysis (some paths are historical; see status note at top of that file if present)  
- [docs/manual/02-getting-started.md](docs/manual/02-getting-started.md) — install and build details  
