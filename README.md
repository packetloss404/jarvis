# Jarvis

A programmable, GPU-rendered tiling desktop shell for vibe coding — an agentic
multi-provider AI assistant, self-hosted encrypted chat & presence, collaborative
terminals, and a `jarvis://` plugin system, all in one cross-platform Rust binary.

![Jarvis](screenshot.png)

Jarvis is a tiling window environment that hosts embedded WebView panels — a
terminal, an AI assistant, live chat, presence, games — over a wgpu-rendered
shell. Everything load-bearing lives in the **`jarvis-rs/`** Rust workspace.

---

## What's implemented

- **Tiling shell + terminal** — wgpu (Vulkan/Metal/DX12) renderer, binary-split
  tiling window manager, PTY-backed terminal (xterm.js), command palette, and a
  `jarvis://` plugin protocol with embedded first-party plugins (games, music).
- **Agentic AI assistant** — multi-provider (**Claude · OpenAI GPT · Google
  Gemini · MiniMax**, switchable in-panel). Read-only filesystem tools by default;
  **write/exec tools are opt-in and gated behind a fail-closed human approval
  gate** (no-shell argv execution, sandbox-jailed paths, per-call approval).
- **Live chat + presence** — end-to-end encrypted (ECDSA identities, AES-GCM),
  running over Jarvis's **own relay** (the `jarvis-relay` crate) — no third-party
  backend. Deployable anywhere (a Railway/Docker config is included).
- **Collaborative terminal / pair programming** *(experimental, off by default)* —
  share a terminal over the relay with driver/navigator roles; sessions are
  **authenticated with signed frames** (see `collab.enabled` in config).
- **Mobile companion** — `jarvis-mobile/` (React Native / Expo): pair to the
  desktop for a remote terminal, the same relay chat, and a `claude.ai` view.
- **Cross-platform** — Windows, macOS, Linux (incl. Windows workspace screen
  capture for live sharing).

---

## Build & run — `jarvis-rs/` (Rust)

```bash
cd jarvis-rs
cargo run                    # debug
cargo build --release        # release → target/release/jarvis(.exe)
cargo test --workspace       # full test suite
```

Panel HTML/CSS/JS is canonical under **`jarvis-rs/assets/panels/`** (bundled via
`include_dir` at compile time). The **relay server** builds separately:

```bash
cargo build --release --bin jarvis-relay   # then deploy (see relay/, railway.json)
```

Configuration lives in the OS config dir (`<config>/jarvis/config.toml`). AI
provider keys are read from the environment (`OPENAI_API_KEY`, `GEMINI_API_KEY` /
`GOOGLE_API_KEY`, `MINIMAX_API_KEY`; Claude via `claude auth login` or
`CLAUDE_CODE_OAUTH_TOKEN`). The relay URL defaults to the project's deployment and
is overridable in config.

Full detail: **[docs/manual/README.md](docs/manual/README.md)**.

---

## Repository layout

```
jarvis/
  jarvis-rs/           # PRIMARY: Rust desktop app + relay (develop here)
    crates/            # app, renderer, tiling, webview, ai, social, relay, config, platform, common
    assets/panels/     # embedded panel + plugin HTML/CSS/JS
    testdata/          # shared wire-protocol fixtures (relay ↔ desktop)
  jarvis-mobile/       # React Native / Expo companion (thin client)
  relay/               # relay deployment helpers (Dockerfile, etc.)
  railway.json         # relay deploy config (Railway)
  resources/themes/    # built-in theme assets (loaded by jarvis-rs)
  docs/                # technical manual + site
  dev/                 # development notes, plans, analysis
```

> The original macOS Python + Swift/Metal prototype (formerly `legacy/`) has been
> removed from the working tree; it is preserved at the **`legacy-archive`** git tag
> (`git checkout legacy-archive` to run it).

See **[ARCHITECTURE.md](ARCHITECTURE.md)** for boundaries and "where does this feature live?"

---

## Documentation

| Doc | Purpose |
|-----|---------|
| [ARCHITECTURE.md](ARCHITECTURE.md) | Repo map, crate boundaries |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Builds, tests, PR hints |
| [docs/manual/README.md](docs/manual/README.md) | Full technical manual |
| [dev/plans/c2-pair-programming.md](dev/plans/c2-pair-programming.md) | Collaborative-terminal design + security |
| [CHANGELOG.md](CHANGELOG.md) | High-level history |

---

## License

MIT — see [LICENSE](LICENSE).
