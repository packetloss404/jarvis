# Jarvis

A multiplayer vibe coding experience — games, chats, and vibes.

![Jarvis](screenshot.png)

Jarvis is a shared desktop environment where multiple AI assistants, retro arcade games, live chat, and a reactive visual shell coexist on one screen. This repository is organized so **one codebase is obviously “the product”** and everything else is supporting or historical.

---

## TL;DR: what to open first

| If you want to… | Go here |
|-----------------|--------|
| **Build the app most people should run** | [`jarvis-rs/`](jarvis-rs/) — cross-platform Rust desktop (Windows, macOS, Linux). |
| **Understand the folder layout** | [`ARCHITECTURE.md`](ARCHITECTURE.md) |
| **Contribute code or run tests** | [`CONTRIBUTING.md`](CONTRIBUTING.md) |
| **Work on the mobile companion** | [`jarvis-mobile/`](jarvis-mobile/) |

**Developed app:** the **`jarvis-rs`** workspace (wgpu + embedded WebViews) is where all feature work belongs. The original macOS Python + Swift/Metal prototype has been removed from the working tree; it is preserved at the **`legacy-archive`** git tag for historical reference.

---

## Primary application — `jarvis-rs/` (Rust)

This is the **default** development target: one binary, embedded panel assets, relay/mobile pairing, terminal, chat, games, and assistants.

```bash
cd jarvis-rs
cargo build --release
# Output: jarvis-rs/target/release/jarvis   (or jarvis.exe on Windows)
cargo test --workspace
```

Panel HTML/CSS/JS is canonical under **`jarvis-rs/assets/panels/`** (bundled via `include_dir` at compile time). Do not add parallel copies at the repository root.

**Relay server** (optional, for mobile / pairing):

```bash
cd jarvis-rs
cargo build --release --bin jarvis-relay
```

Further detail: **[docs/manual/README.md](docs/manual/README.md)** (full technical manual).

---

## Repository layout (high level)

```
jarvis/
  jarvis-rs/           # PRIMARY: Rust desktop app (develop here)
    testdata/          # Shared wire-protocol JSON fixtures (relay ↔ desktop tests)
  jarvis-mobile/       # React Native companion (thin client)
  docs/                # Website + published manual; built HTML is gitignored
  dev/                 # Development docs only (pathforward analysis, etc.)
    pathforward/       # Strategic / model-sourced codebase write-ups
    _archive/          # Dated internal plans (kept for history; not the live manual)
  relay/               # Deployment helpers (separate from app crates)
  resources/           # Built-in theme assets (resources/themes/, loaded by jarvis-rs)
```

> The original macOS Python + Swift/Metal prototype (formerly `legacy/`, with its
> `scripts/` and packaging assets) has been removed from the working tree. It is
> preserved in history at the **`legacy-archive`** git tag.

See **[ARCHITECTURE.md](ARCHITECTURE.md)** for intent, boundaries, and “where does this feature live?”

---

## Documentation

| Doc | Purpose |
|-----|---------|
| [ARCHITECTURE.md](ARCHITECTURE.md) | Repo map, primary app, mobile |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Builds, tests, PR hints |
| [docs/manual/README.md](docs/manual/README.md) | Full Jarvis technical manual (lives under **`docs/`** with the site) |
| [dev/_archive/plugins/plugins.md](dev/_archive/plugins/plugins.md) | Plugin system (Rust app; archived notes) |
| [dev/pathforward/finalfindings.md](dev/pathforward/finalfindings.md) | Strategic codebase analysis (under **`dev/`**; see also **`dev/_archive/`** for older plans) |
| [CHANGELOG.md](CHANGELOG.md) | High-level history from git (themes over time) |

---

## License

See repository license file (if present) or package metadata in `jarvis-rs/Cargo.toml`.
