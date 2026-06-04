# Contributing to Jarvis

Thank you for helping improve Jarvis. This repository intentionally holds **two** desktop stories; knowing which one you are changing keeps reviews small and CI meaningful.

## Where to work

| Area | Path | When to touch it |
|------|------|-------------------|
| **Primary product (default)** | [`jarvis-rs/`](jarvis-rs/) | All new features, bug fixes, and UX work for the cross-platform app. |
| **Mobile companion** | [`jarvis-mobile/`](jarvis-mobile/) | React Native client; keep it a thin companion relative to the desktop app. |
| **Website & manual** | [`docs/`](docs/) | Marketing site, technical manual — publishable / contributor-facing. |
| **Dev docs** | [`dev/`](dev/) | Internal development notes, pathforward analysis (`dev/pathforward/`), archived plans and plugins notes (`dev/_archive/`). |

> The original macOS Python + Swift/Metal stack has been removed from the working tree and archived at the **`legacy-archive`** git tag. There is no in-tree legacy code to maintain.

Read **[ARCHITECTURE.md](ARCHITECTURE.md)** for a full map of directories and boundaries.

## Building and testing the Rust app

```bash
cd jarvis-rs
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo build --release
```

Optional: compile the GitHub release checker module with `cargo test -p jarvis-app --features updater` (the `updater` feature is off by default until it is wired into the UI).

Release-style binaries land under `jarvis-rs/target/release/` (`jarvis` or `jarvis.exe`).

## Manual HTML (`docs/manual/`)

The single-file manual (`jarvis-manual.html`) is **not** tracked in git. Generate it from the Markdown chapters:

```bash
cd docs/manual
python build_html.py
```

## Pull requests

- Prefer small, focused PRs with a clear primary target.
- If you change paths or entrypoints, update **README.md**, **ARCHITECTURE.md**, and the **getting started** manual chapter so newcomers are not misled.
- Rust changes should pass `fmt`, `clippy` (`-D warnings`), and tests locally when possible.

## Code of conduct

Be constructive and assume good intent. For security-sensitive issues, disclose privately per repository security policy if one is published.
