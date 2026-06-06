# Architecture Overview

This document describes the internal architecture of the Jarvis desktop application, covering the Rust workspace structure, crate responsibilities, dependency graph, application lifecycle, key design patterns, and the relationship to the archived Python/Swift prototype.

---

## 1. What Jarvis Is

Jarvis is a GPU-accelerated desktop environment that combines terminal emulation, agentic AI assistants, plugin panels (including retro arcade games and a music player), live social presence and chat, and a configurable visual effects system into a single tiled window. Users interact with multiple panes simultaneously -- each pane can be a terminal, an AI assistant, a chat room, a plugin (game/draw/music), or a settings panel. All pane content is rendered via embedded WebViews (powered by `wry`), while the window chrome, background effects, and tiling layout are rendered natively through `wgpu`.

The application supports:

- **Tiling window management** with split/zoom/resize/swap operations
- **Embedded terminal emulation** via xterm.js connected to native PTY processes
- **Agentic, multi-provider AI assistant panels** backed by Claude, Gemini, and OpenAI-compatible (OpenAI / MiniMax) streaming APIs, with tool calling and an interactive tool-approval gate
- **Live social presence and chat** carried over the relay's symmetric **Room** protocol (no Supabase / external presence server)
- **E2E encrypted chat and mobile bridging** with ECDSA identity, ECDH key exchange, and AES-256-GCM
- **Mobile pairing** via the standalone WebSocket relay server with QR code provisioning
- **Plugin system** for custom HTML/JS/CSS panels served via the `jarvis://` protocol; the bundled games (Tetris, Asteroids, Pinball, etc.), drawing pad, and music player ship as plugins
- **Experimental pair programming** (a shared terminal over an encrypted relay Room, gated behind `collab.enabled` and the `experimental-collab` feature)
- **TOML-based configuration** with live reload, theme support, and validation

---

## 2. Repository Structure

The repository root groups the **primary Rust app**, the **mobile companion**, docs, and tooling. `jarvis-rs/` is the sole application; the original macOS Python + Swift/Metal prototype is archived at the **`legacy-archive`** git tag and is no longer part of the tracked working tree.

```
jarvis/
  README.md              # Rust-first overview + archive pointer
  ARCHITECTURE.md        # "Where things live" summary
  CHANGELOG.md
  jarvis-rs/             # PRIMARY (and only) application: Rust workspace
    Cargo.toml           # Workspace manifest (10 crate members)
    crates/              # The 10 crates (see Section 3)
    assets/panels/       # Canonical bundled web UI (terminal, chat, settings,
                         #   presence, pair, boot, status_bar, and plugins/)
    testdata/            # Shared wire-protocol JSON fixtures (relay <-> desktop)
    docs/ packaging/ scripts/
  jarvis-mobile/         # React Native companion (thin client)
  dev/                   # Development docs (pathforward analysis, archived plans)
  relay/                 # Relay deployment helpers (Dockerfile, cloudbuild, deploy.sh)
  resources/themes/      # Built-in theme assets
  docs/                  # Website + published manual (this chapter lives here)
```

> The original macOS Python + Swift/Metal prototype (`main.py`, `metal-app/`,
> `requirements.txt`, `scripts/`, and the Python `skills/`, `voice/`,
> `connectors/`, `presence/` packages) is no longer tracked in the working tree.
> Check it out with `git checkout legacy-archive`. (An untracked, gitignored
> `legacy/` scratch directory may exist locally but is not part of the repo and
> does not build.)

---

## 3. Rust Workspace Structure

The workspace at `jarvis-rs/Cargo.toml` defines 10 crates:

| Crate | Type | Description |
|-------|------|-------------|
| `jarvis-app` | Binary (`jarvis`) | Application shell: window, event loop, IPC dispatch, all subsystem coordination |
| `jarvis-common` | Library | Shared types, error hierarchy, action enum, event bus, notification queue |
| `jarvis-config` | Library | TOML configuration loading, validation, theme system, live reload, plugin discovery |
| `jarvis-platform` | Library | OS abstractions: clipboard, paths, crash reports, input processing, keybind registry, crypto identity |
| `jarvis-tiling` | Library | Binary split tree, layout engine, pane management, focus tracking, zoom, stacks (tabs) |
| `jarvis-renderer` | Library | GPU rendering: wgpu context, background shader pipeline, quad renderer, UI chrome, command palette, assistant panel |
| `jarvis-ai` | Library | Multi-provider AI clients (Claude, Gemini, OpenAI-compatible: OpenAI/MiniMax, Whisper); SSE streaming, agentic tool-calling loops with an approval gate, session management, skill routing; push-to-talk voice input (cpal mic capture + WAV encode feeding Whisper) |
| `jarvis-social` | Library | Social features over the relay's symmetric **Room** transport: presence tracking, chat history, channels, identity; experimental voice/screen share/pair programming (feature-gated) |
| `jarvis-webview` | Library | WebView management: wry wrapper, IPC bridge, content provider (`jarvis://` protocol), theme bridge, navigation control |
| `jarvis-relay` | Binary (`jarvis-relay`) | Standalone WebSocket relay server: mobile-desktop session pairing **and** the symmetric Room fan-out used by presence/chat/pair; signed-`room_hello` TOFU slot binding (`room_auth`), rate limiting, stale session reaping |

### 3.1 Crate Details

#### `jarvis-common`

**Path:** `jarvis-rs/crates/jarvis-common/src/lib.rs`

The foundation crate with zero internal-crate dependencies. Defines the shared vocabulary used across all other crates.

Key modules and types:

- `actions::Action` -- Enum of every user-triggerable action (39 variants). Keybinds, command palette, CLI, and IPC all resolve to an `Action`. Defined in `actions/action_enum.rs`. Notable variants include pane management (`NewPane`, `ClosePane`, `SplitHorizontal/Vertical`, `Focus*`, `ZoomPane`, `ResizePane`, `SwapPane`, `ToggleBlankPane`), overlays (`OpenCommandPalette`, `OpenSettings`, `OpenChat`, `OpenPair`, `OpenAssistant`, `CloseOverlay`), navigation (`OpenURL`, `OpenURLPrompt`), terminal/clipboard (`Copy`, `Paste`, `SelectAll`, `Search*`, `Scroll*`, `ClearTerminal`), voice (`PushToTalk`, `ReleasePushToTalk`), mobile pairing (`PairMobile`, `RevokeMobilePairing`), and `ReloadConfig`/`Quit`. There is **no** dedicated `LaunchGame` action -- games are plugins opened via the command palette / `OpenURL`.
- `actions::ResizeDirection` -- Left/Right/Up/Down for resize and swap operations.
- `events::Event` -- Broadcast events: `ConfigReloaded`, `PaneOpened(PaneId)`, `PaneClosed(PaneId)`, `PaneFocused(PaneId)`, `PresenceUpdate { user_id, status }`, `ChatMessage { from, text }`, `Notification(String)`, `Shutdown`, plus a `#[serde(other)] Unknown` catch-all.
- `events::EventBus` -- Wrapper around `tokio::sync::broadcast::Sender<Event>` with publish/subscribe API.
- `types::Rect` -- `{x, y, width, height}` as `f64`. Used for viewport and pane bounds. (Defined in `types/core.rs`.)
- `types::PaneId` -- Newtype wrapper `PaneId(u32)`.
- `types::PaneKind` -- Enum: `Terminal`, `Assistant`, `Chat`, `WebView`, `ExternalApp`.
- `types::AppState` -- Enum: `Starting`, `Running`, `ShuttingDown`.
- `types::Color` -- RGBA color with hex/rgba string parsing (in `types/color.rs`).
- `errors::JarvisError`, `errors::ConfigError`, `errors::PlatformError` -- Error hierarchy using `thiserror`.
- `notifications::Notification` / `NotificationLevel` -- In-app toast with level, title, body, TTL.
- `notifications::NotificationQueue` -- Bounded FIFO queue with auto-eviction of expired notifications.
- `id::new_id`, `id::new_correlation_id`, `id::SessionId` -- UUID v4 generators.

#### `jarvis-config`

**Path:** `jarvis-rs/crates/jarvis-config/src/lib.rs`
**Depends on:** `jarvis-common`

Manages the entire configuration lifecycle.

Key modules:

- `schema` -- Sub-modules defining every config section as `#[derive(Serialize, Deserialize, Default)]` structs. Root type: `schema::JarvisConfig` (27 sections): theme, colors, font, terminal, shell, window, effects, layout, opacity, background, visualizer, startup, voice, assistant, keybinds, panels, livechat, presence, performance, updates, logging, advanced, auto_open, status_bar, relay, plugins, collab. (There is no `games` section -- games are plugins.) `CONFIG_SCHEMA_VERSION` is `1`.
- `toml_loader` -- Loads `config.toml` from the OS config directory (`dirs::config_dir()/jarvis/config.toml`). Creates a default config file if none exists.
- `toml_loader::plugins` -- Discovers local plugin directories from `{config_dir}/jarvis/plugins/`.
- `toml_writer` -- Serializes config back to TOML for the settings panel.
- `theme` -- Built-in themes (`BUILT_IN_THEMES`) and `ThemeOverrides` that selectively override config fields. Themes are applied after loading.
- `validation` -- Validates color formats, font sizes, opacity ranges, layout constraints, background settings, visualizer parameters.
- `watcher::ConfigWatcher` -- File system watcher (via `notify` crate) for live config reload.
- `reload::ReloadManager` -- Debounced reload coordination.
- `colors` -- Color parsing utilities.
- `keybinds` -- Keybind string normalization.

The main entry point is `load_config()` which: loads TOML -> applies theme -> discovers plugins -> validates -> returns `JarvisConfig`.

#### `jarvis-platform`

**Path:** `jarvis-rs/crates/jarvis-platform/src/lib.rs`
**Depends on:** `jarvis-common`, `jarvis-config`

OS-level abstractions and platform services.

Key modules:

- `input::KeybindRegistry` -- `HashMap<KeyCombo, Action>` built from `KeybindConfig`. Supports lookup by key combination and reverse lookup (action -> display string). Defined in `input/registry.rs`.
- `input::KeyCombo` -- Normalized key combination (ctrl, alt, shift, super + key name). Defined in `input/key_combo.rs`.
- `input_processor::InputProcessor` -- Stateful processor that sits between winit keyboard events and the rest of the app. Checks keybind registry first, then falls back to terminal byte encoding. Tracks current `InputMode` (Terminal, CommandPalette, Assistant, Settings). Defined in `input_processor/processor.rs`.
- `input_processor::InputResult` -- Return type: `Action(Action)`, `TerminalInput(Vec<u8>)`, or `Consumed`.
- `input_processor::InputMode` -- Enum: `Terminal`, `CommandPalette`, `Assistant`, `Settings`.
- `input_processor::encoding` -- Encodes key names to terminal escape sequences (VT100/xterm).
- `keymap` -- Parses keybind config strings (e.g., `"Cmd+T"`) into `KeyBind` structs. Handles platform normalization (Cmd on macOS = Ctrl elsewhere).
- `clipboard::Clipboard` -- Cross-platform clipboard access via `arboard`.
- `paths` -- Resolves OS-standard directories: `config_dir()`, `data_dir()`, `cache_dir()`, `log_dir()`, `crash_report_dir()`, `identity_file()`. `ensure_dirs()` creates them.
- `crash_report` -- Writes structured crash reports on panic: backtrace, platform info, sanitized paths. Defined in `crash_report/mod.rs`.
- `crypto::CryptoService` -- Full cryptographic identity service:
  - Persistent ECDSA P-256 signing key (for message authentication)
  - Persistent ECDH P-256 key pair (for key exchange)
  - AES-256-GCM symmetric encryption/decryption
  - PBKDF2-HMAC-SHA256 room key derivation
  - ECDH shared key derivation
  - Key handle store (opaque u32 references to in-memory keys)
  - Identity persistence to JSON file with PKCS#8 DER encoding
  - Fingerprint: first 8 bytes of SHA-256 of the ECDSA public key SPKI DER
  - `PairFrameSigner` -- a cheap clone of the signing key, handed to the social/pair subsystem to sign/verify pair-programming frames without sharing the full service
- `winit_keys::normalize_winit_key` -- Normalizes winit key names to canonical form.
- `mouse` -- Mouse event helpers.
- `notifications` -- OS-native notification support.

#### `jarvis-tiling`

**Path:** `jarvis-rs/crates/jarvis-tiling/src/lib.rs`
**Depends on:** `jarvis-common`

A pure-logic tiling window manager with no platform dependencies.

Key types:

- `tree::SplitNode` -- Recursive binary tree enum:
  ```rust
  enum SplitNode {
      Leaf { pane_id: u32 },
      Split { direction: Direction, ratio: f64, first: Box<SplitNode>, second: Box<SplitNode> },
  }
  ```
  Supports: `split_at`, `remove_pane`, `swap_panes`, `adjust_ratio`, `adjust_ratio_between`, `find_neighbor`, `next_pane`/`prev_pane` (wrapping), `collect_pane_ids` (DFS order).

- `tree::Direction` -- `Horizontal` (left/right split) or `Vertical` (top/bottom split).

- `manager::TilingManager` -- Central coordinator:
  - `tree: SplitNode` -- The root split tree
  - `panes: HashMap<u32, Pane>` -- Registry of all panes
  - `stacks: HashMap<u32, PaneStack>` -- Tabbed pane stacks at leaf positions
  - `focused: u32` -- Currently focused pane ID
  - `zoomed: Option<u32>` -- Zoomed pane (fills entire viewport)
  - `layout_engine: LayoutEngine` -- Layout computation config
  - `next_id: u32` -- Auto-incrementing pane ID counter

  Operations: `split`, `close_focused`, `close_pane`, `focus_next`, `focus_prev`, `focus_direction`, `focus_pane`, `zoom_toggle`, `resize`, `swap`, `push_to_stack`, `cycle_stack_next`, `split_with` (custom kind/title), `compute_layout`, `execute(TilingCommand)`.

- `layout::LayoutEngine` -- Computes `Vec<(u32, Rect)>` from a `SplitNode` tree and viewport rect. Configurable `gap`, `outer_padding`, `min_pane_size`.

- `layout::borders` -- Computes resize borders between panes for drag-to-resize.

- `pane::Pane` -- `{ id: PaneId, kind: PaneKind, title: String }`.

- `stack::PaneStack` -- Tab stack at a leaf position. Supports push, cycle, active tracking.

- `commands::TilingCommand` -- Command enum: `SplitHorizontal`, `SplitVertical`, `Close`, `Resize(Direction, i32)`, `Swap(Direction)`, `FocusNext`, `FocusPrev`, `FocusDirection(Direction)`, `Zoom`.

- `platform::WindowManager` -- Trait for OS-level window tiling (macOS/X11/Wayland/Windows/noop implementations).

#### `jarvis-renderer`

**Path:** `jarvis-rs/crates/jarvis-renderer/src/lib.rs`
**Depends on:** `jarvis-common`, `jarvis-config`, `jarvis-platform`

GPU rendering via `wgpu` (cross-platform Vulkan/Metal/DX12).

Key types:

- `gpu::GpuContext` -- Holds the wgpu `Instance`, `Adapter`, `Device`, `Queue`, `Surface`, and `SurfaceConfiguration`. Created asynchronously from a `winit::Window`.

- `render_state::RenderState` -- Core rendering state:
  - `gpu: GpuContext`
  - `quad: QuadRenderer` -- Renders colored rectangles for UI chrome
  - `bg_pipeline: BackgroundPipeline` -- Full-screen hex grid background shader
  - `uniforms: GpuUniforms` -- Shared GPU uniform buffer (viewport size, time, config values)
  - `clear_color: wgpu::Color`

  Per-frame render order: (1) clear + hex grid background, (2) UI chrome quads (tab bar, status bar, active tab highlight).

- `quad::QuadRenderer` -- Instanced quad renderer. `QuadInstance { rect: [f32; 4], color: [f32; 4] }`. Uploads instance data each frame and draws with a single draw call.

- `background::BackgroundPipeline` -- Full-screen shader pipeline for the animated hex grid background.

- `ui::UiChrome` -- Layout computation for tab bar, status bar, and pane borders. Built from `LayoutConfig`.
  - `TabBar` -- `{ tabs: Vec<Tab>, height: f32 }`
  - `Tab` -- `{ pane_id: u32, title: String, is_active: bool }`
  - `StatusBar` -- `{ height: f32, bg_color: [f32; 4] }`
  - `PaneBorder` -- Border rendering data

- `command_palette::CommandPalette` -- Fuzzy command search with `PaletteItem` entries. Modes: `Commands` (default), `UrlInput`. Supports type-ahead filtering, arrow key selection, confirm/dismiss.

- `assistant_panel::AssistantPanel` -- Chat UI model with `ChatMessage { role: ChatRole, content: String }` history.

- `boot_screen` -- Boot splash screen text rendering.
- `effects` -- Post-processing effect types.
- `perf::FrameTimer` -- Frame timing measurement.

#### `jarvis-ai`

**Path:** `jarvis-rs/crates/jarvis-ai/src/lib.rs`
**Depends on:** `jarvis-common`

Multi-provider, agentic AI clients with a unified interface.

Key types:

- `AiClient` trait -- Async trait with `send_message` and `send_message_streaming` methods. Implemented by every provider client.
- `Message { role: Role, content: String, blocks: Vec<ContentBlock> }` -- Chat message. Plain-text messages carry `content`; agentic turns carry structured `blocks`. Role: `User`, `Assistant`, `System`, `Tool`.
- `ContentBlock` -- `Text`, `ToolUse { id, name, input }`, `ToolResult { tool_use_id, content, is_error }` -- mirrors Claude's content-block model so tool-use / tool-result turns round-trip.
- `ToolDefinition { name, description, parameters }` -- Function/tool schema for tool calling.
- `AiResponse { content, tool_calls: Vec<ToolCall>, usage: TokenUsage }` -- Response with optional tool invocations.
- `AiError` -- `ApiError`, `RateLimited`, `NetworkError`, `ParseError`, `Timeout`.

Providers:

- `claude::ClaudeClient` / `ClaudeConfig` -- Claude API client with SSE streaming. Modules: `claude/client.rs`, `claude/api.rs`, `claude/config.rs`.
- `gemini::GeminiClient` / `GeminiConfig` -- Gemini (Google Generative Language API) client.
- `openai::OpenAiClient` / `OpenAiConfig` -- One parameterized client speaking the OpenAI Chat Completions wire format, used for both the `OpenAi` and `MiniMax` providers.
- `whisper::WhisperClient` / `WhisperConfig` -- OpenAI Whisper transcription client.
- `voice::VoiceRecorder` / `voice::encode_wav_mono` (`voice/mod.rs`) -- Microphone capture for push-to-talk voice input. Opens the default input device via `cpal`, buffers samples while the PTT key is held, mixes them down to mono, and on `stop()` encodes a complete 16-bit PCM WAV byte vector (at the device's native sample rate) ready to hand to `WhisperClient::transcribe`. The `cpal` stream is `!Send`, so the recorder lives on the app's main/UI thread. On Linux this requires the ALSA dev headers (`libasound2-dev`) at build time.
- `router::SkillRouter` -- Routes user intents to the appropriate provider/skill. `Provider` enum is `Claude | OpenAi | MiniMax | Gemini` (default `Claude`); `Skill { name, provider, system_prompt }`. Tool conversion helpers (`to_claude_tool`, `to_gemini_tool`, `to_openai_tool`) adapt the shared `ToolDefinition` to each provider's schema.

Agentic session and tools:

- `session::Session` -- Manages multi-turn conversation state with automatic tool-call loops. Modules: `session/chat.rs`, `session/manager.rs`, `session/types.rs`.
- `session::ToolExecutor` / `ToolOutcome` / `ToolEvent` / `ToolEventCallback` -- Pluggable tool execution with per-call event reporting.
- **Approval gate**: `ApprovalGate` / `ApprovalRequest` / `ApprovalDecision` / `ApprovalReceiver`. Tools in `APPROVAL_REQUIRED_TOOLS` (`write_file`, `run_command`) pause the agent loop until the user approves or denies via IPC, with an `APPROVAL_TIMEOUT` of 120s.
- `tools` -- Tool definitions (`tools/definitions.rs`: `builtin_tools()`, `read_only_tools()`) and executors. `ReadOnlyToolExecutor` runs only safe tools; `WriteExecToolExecutor` additionally runs `WRITE_EXEC_TOOLS` (`write_file`, `run_command`, with a `RUN_COMMAND_TIMEOUT` of 30s) and is only invoked after approval. Sandboxing in `tools/sandbox.rs`.
- `streaming` -- SSE stream parsing utilities.
- `token_tracker::TokenTracker` -- Tracks cumulative token usage across providers.

#### `jarvis-social`

**Path:** `jarvis-rs/crates/jarvis-social/src/lib.rs`
**Depends on:** `jarvis-common`

Social features: presence, chat, identity, and experimental collaboration -- all carried over the relay's symmetric **Room** transport (the old Supabase/Phoenix `realtime` transport has been removed).

Key types:

- `room::RoomClient` / `RoomConfig` / `RoomEvent` / `RoomControl` -- A thin, transport-only WebSocket client that speaks the relay's symmetric **Room** protocol (`room_hello` -> `room_ready` + `member_joined`* + `member_count`, then opaque text fan-out via `RoomClient::send`). It knows nothing about presence semantics; it surfaces control frames (`MemberJoined`/`MemberLeft`/`MemberCount`) and every opaque text frame as a `RoomEvent::Frame`. This module **replaces** the former `realtime::RealtimeClient`.
- `presence` (module: `presence/client.rs`, `event_translator.rs`, `helpers.rs`, `types.rs`) -- `PresenceClient` layers presence semantics on top of `RoomClient`: it sends `PresenceFrame`s and translates inbound `RoomEvent`s into `PresenceEvent`s. Returns events via channel.
- `presence::PresenceEvent` -- `Connected { online_count }`, `Disconnected`, `UserOnline`, `UserOffline`, `ActivityChanged`, `GameInvite`, `Poked`, `ChatMessage`, `Error`, and more.
- `presence::PresenceConfig` -- Relay/room URL, identity, heartbeat interval.
- `chat::ChatHistory` / `ChatHistoryConfig` / `ChatMessage` -- Chat message storage.
- `channels::Channel` / `ChannelManager` -- Chat channel management.
- `identity::Identity` -- User identity (user_id, display_name) generation.
- `protocol` -- Wire protocol types: `OnlineUser`, `UserStatus`, `PresenceFrame`, `PresencePayload`, `ChatMessagePayload`, `GameInvitePayload`, `PokePayload`, `ActivityUpdatePayload`.

Feature-gated experimental modules (behind the `experimental-collab` feature flag):

- `pair` -- Pair programming sessions over a Room (`PairManager`, `PairSession`, `PairRole`, `PairConfig`, `PairEvent`). Frames are signed via `PairFrameSigner` and exchanged as opaque encrypted text over the relay Room.
- `voice` -- Voice chat rooms (`VoiceManager`, `VoiceRoom`, `VoiceConfig`, `VoiceEvent`); `VoiceSignal` in `protocol`.
- `screen_share` -- Screen sharing (`ScreenShareManager`, `ScreenShareConfig`, `ShareQuality`, `ScreenShareEvent`); `ScreenShareSignal` in `protocol`.

#### `jarvis-webview`

**Path:** `jarvis-rs/crates/jarvis-webview/src/lib.rs`
**Depends on:** `jarvis-common`

WebView lifecycle management and IPC bridge via `wry`.

Key types:

- `manager::WebViewManager` -- Creates, tracks, and destroys `wry::WebView` instances. Maintains an event sink (`Arc<Mutex<Vec<WebViewEvent>>>`) for the main event loop to drain.
- `manager::WebViewRegistry` -- Higher-level wrapper mapping `pane_id: u32` -> `WebViewHandle`. Methods: `create`, `get`, `get_mut`, `destroy`, `destroy_all`, `active_panes`, `drain_events`, `count`.
- `manager::WebViewHandle` -- Wrapper around a `wry::WebView` with convenience methods: `evaluate_script`, `load_url`, `send_ipc`, `set_bounds`, `current_url`.
- `manager::WebViewConfig` -- Configuration for creating a WebView (URL, transparent, IPC handler).
- `manager::handlers` -- Navigation allowlisting (`ALLOWED_NAV_PREFIXES`, `is_navigation_allowed`).
- `content::ContentProvider` -- Serves local files via the `jarvis://` custom protocol:
  - Resolves paths relative to a base directory (typically `assets/panels/`)
  - Supports in-memory overrides for dynamically generated content
  - Plugin directory resolution (`plugins/{id}/...`) with containment checks to prevent directory traversal
  - MIME type inference from file extensions (HTML, CSS, JS, images, fonts, audio, video, WASM)
  - Shared plugin directory handle via `Arc<RwLock<HashMap<String, PathBuf>>>` for config reload
- `ipc::IpcMessage { kind: String, payload: IpcPayload }` -- Typed IPC message from JavaScript.
- `ipc::IpcPayload` -- `Text(String)`, `Json(Value)`, or `None`.
- `ipc::IPC_INIT_SCRIPT` -- JavaScript initialization script injected into every WebView. Sets up:
  - `window.jarvis.ipc` bridge with `send()`, `on()`, `_dispatch()`, `request()` (Promise-based)
  - Keyboard shortcut forwarder (intercepts Cmd+key before WebView consumes them)
  - Clipboard API polyfill (WKWebView blocks `navigator.clipboard`)
  - Command palette overlay system (DOM injection, keyboard handler, IPC integration)
  - Diagnostic event logging
- `events::WebViewEvent` -- `Closed { pane_id }`, `PageLoadState`, etc.
- `theme_bridge` -- Generates CSS custom properties from config theme and injects into WebViews.

#### `jarvis-relay`

**Path:** `jarvis-rs/crates/jarvis-relay/src/main.rs`
**Depends on:** (no internal crates -- standalone binary)

A standalone WebSocket relay server (CLI flags: `--port` default 8080, `--session-ttl`, `--max-connections-per-ip`, `--max-sessions`). The connecting client's `Role` (`Desktop`/`Mobile`/`Host`/`Spectator`/`Member`) maps to one of three `SessionKind`s (`Bridge`/`Broadcast`/`Room`), all keyed by a session ID and forwarding opaque text after the first message:

- **Bridge (1:1 pairing)** -- `Desktop`/`Mobile` roles via `desktop_hello` / `mobile_hello`, with `peer_connected` / `peer_disconnected` notifications. Used for mobile<->desktop bridging.
- **Broadcast (1:N)** -- `Host`/`Spectator` roles via `host_hello` / `spectator_hello`, with `host_connected` / `viewer_count`. Used for workspace/chat streaming to viewers.
- **Room (N:N symmetric)** -- `Member` role via `room_hello { session_id, member_id, pubkey, nonce, sig }` -> `room_ready` + `member_joined`* + `member_count`, then symmetric opaque fan-out. This is the transport `jarvis-social` (presence/chat/pair) rides on.

Modules:

- `connection` -- Handles individual WebSocket connections, parses the first `RelayHello`, verifies/commits the signed `room_hello` (for Room joins) via `room_auth`, and routes the connection into the appropriate session kind.
- `protocol` -- `RelayHello` / `RelayResponse` wire enums (only the first frame is parsed; the rest is opaque text), plus `signed_hello_payload` (the canonical bytes a `room_hello` signs). Wire conformance is pinned against JSON fixtures in `jarvis-rs/testdata/relay/`.
- `room_auth::RoomAuthStore` -- Signed-`room_hello` verification that binds each Room slot to a cryptographic identity (defeats the member-id slot-squat/eviction DoS). Each `room_hello` is signed with the client's ECDSA P-256 identity; before a member is admitted the relay verifies the signature over the canonical payload, checks nonce freshness/monotonicity (anti-replay), and enforces a per-`(session_id, member_id)` pubkey binding via **relay-side TOFU** (first signed join pins the key; later joins must present the same key). Verification (`verify`) is a pure read; the pin + nonce high-water mark are mutated only by `commit`, called after the slot is actually registered.
- `session::SessionStore` -- Manages active `Bridge`/`Broadcast`/`Room` sessions, supports stale session reaping.
- `rate_limit::RateLimiter` -- Per-IP connection limiting and total session caps.

The relay never inspects message payloads -- all PTY/pair data is E2E encrypted between endpoints using the `CryptoService` from `jarvis-platform`.

#### `jarvis-app`

**Path:** `jarvis-rs/crates/jarvis-app/src/main.rs`
**Depends on:** all other crates (except `jarvis-relay`)

The main application binary. Wires everything together.

Entry point (`main.rs`):
1. `load_dotenv()` -- Loads `.env` file (searched at the project root, the `jarvis-rs/` workspace root, then `./`)
2. `install_panic_hook()` -- Installs crash report writer
3. `cli::parse()` -- Parses CLI arguments (`-e/--execute`, `-d/--directory`, `--config`, `--log-level`)
4. Initializes `tracing_subscriber` logging
5. `jarvis_config::load_config()` -- Loads and validates config (falls back to defaults on error)
6. `jarvis_platform::paths::ensure_dirs()` -- Creates platform directories
7. Applies `--directory` (changes the working dir) if given
8. `KeybindRegistry::from_config()` -- Builds keybind registry
9. Creates `winit::EventLoop` and `JarvisApp::new(config, registry)`
10. `event_loop.run_app(&mut app)` -- Enters the event loop

(An optional `updater` module is compiled in behind the `updater` cargo feature.)

**`app_state` module** (sub-modules in `app_state/mod.rs`):

- `core.rs` -- `JarvisApp` struct definition with all fields
- `event_handler.rs` -- `impl ApplicationHandler for JarvisApp` (winit event loop integration)
- `init.rs` -- Window creation, GPU renderer initialization, WebView subsystem setup, crypto identity loading
- `dispatch.rs` -- Action dispatch: routes `Action` enum variants to subsystem calls
- `shutdown.rs` -- Ordered shutdown: PTYs -> WebViews -> presence -> relay -> tokio runtime -> GPU
- `polling.rs` -- Polling at ~60Hz (every `POLL_INTERVAL` = 16ms) of: presence, pair, assistant, webview events, PTY output, chat stream, mobile commands, relay events, menu events
- `terminal.rs` -- Terminal-pane setup / shell command helpers
- `pty_bridge/` -- PTY process management via `portable-pty` (`spawn.rs`, `io.rs`, `types.rs`). Spawns shell processes, manages reader threads, bridges I/O between xterm.js and PTY
- `webview_bridge/` -- 16 sub-modules handling all WebView-related operations:
  - `ipc_dispatch.rs` -- IPC message validation (allowlist of 51 permitted kinds) and routing
  - `lifecycle.rs` -- WebView creation/destruction per pane
  - `bounds.rs` -- Coordinate conversion and bounds synchronization
  - `pty_handlers.rs` -- PTY input/resize/restart IPC handlers
  - `pty_polling.rs` -- Polls PTY output and forwards to WebView
  - `presence_handlers.rs` -- Presence user list and poke forwarding
  - `settings_handlers.rs` -- Settings panel IPC (get/set/update/reset config, theme changes)
  - `assistant_handlers.rs` -- AI assistant input/output forwarding, provider selection, tool approve/deny
  - `crypto_handlers.rs` -- Crypto operations proxied from WebView JS
  - `file_handlers.rs` -- File read operations for WebView
  - `theme_handlers.rs` -- Theme CSS injection into all WebViews
  - `status_bar_handlers.rs` -- Status bar initialization
  - `chat_stream_handlers.rs` -- Workspace streaming to viewers/mobile chat (start/stop/status)
  - `emulator_handlers.rs` -- ROM file listing and loading for the emulator plugin (scans ~/ROMs; NES/SNES/GB/GBA/N64/Genesis/etc.)
  - `music_handlers.rs` -- Music library init/scan/search/set-dir for the music-player plugin
  - `pair_handlers.rs` -- Pair-programming IPC (`pair_start/join/leave/input/request_control/set_driver/cursor/status`)
- `assistant.rs` / `assistant_task.rs` -- AI assistant panel state and the background agentic task (streaming + tool loop + approval gate)
- `social.rs` -- Presence client lifecycle and event polling
- `pair.rs` -- Pair-programming lifecycle: builds the `PairManager`, bridges its events to/from the relay Room, polled by `poll_pair` (idempotent; no-op while `collab.enabled` is false)
- `music_library.rs` -- Local music library scanning/search (metadata via `lofty`) for the music-player plugin
- `palette.rs` -- Command palette keyboard handler; injects plugin items into the palette
- `blanking.rs` -- Pane blanking: covers individual panes with a black overlay and blocks input. State tracked in `blanked_panes`
- `workspace_capture/` -- Cross-platform workspace frame capture for live streaming. Native implementations on macOS and Linux; stub on Windows
- `resize_drag.rs` -- Mouse drag-to-resize pane borders
- `title.rs` -- Dynamic window title updates
- `ui_state.rs` -- UI chrome state updates (tab bar, status bar)
- `menu.rs` -- Native menu bar (via `muda`)
- `ws_server/` -- Mobile relay bridge and pair-room WebSocket clients (`relay_client.rs`, `relay_polling.rs`, `relay_protocol.rs`, `pairing.rs`, `pair_room_client.rs`, `pair_protocol.rs`, `mobile_polling.rs`, `broadcast.rs`, `crypto_bridge.rs`, `startup.rs`)
- `types.rs` -- Internal types: `AssistantEvent`, `PresenceCommand`, `POLL_INTERVAL` (16ms / ~60Hz)

---

## 4. Dependency Graph

```
jarvis-common        (foundation -- no internal deps)
    |
    +-- jarvis-config        (depends on: jarvis-common)
    |
    +-- jarvis-tiling        (depends on: jarvis-common)
    |
    +-- jarvis-ai            (depends on: jarvis-common)
    |
    +-- jarvis-social        (depends on: jarvis-common)
    |
    +-- jarvis-webview       (depends on: jarvis-common)
    |
    +-- jarvis-platform      (depends on: jarvis-common, jarvis-config)
    |
    +-- jarvis-renderer      (depends on: jarvis-common, jarvis-config, jarvis-platform)
    |
    +-- jarvis-app           (depends on: ALL of the above)
    |
    jarvis-relay             (standalone -- no internal deps)
```

Textual summary:

- `jarvis-common` is depended on by every other library crate.
- `jarvis-config` depends only on `jarvis-common`.
- `jarvis-tiling`, `jarvis-ai`, `jarvis-social`, `jarvis-webview` each depend only on `jarvis-common`.
- `jarvis-platform` depends on `jarvis-common` and `jarvis-config`.
- `jarvis-renderer` depends on `jarvis-common`, `jarvis-config`, and `jarvis-platform`.
- `jarvis-app` depends on all library crates, and enables `jarvis-social`'s `experimental-collab` feature (so the pair/voice/screen-share modules are compiled in). `jarvis-app` does **not** depend on `jarvis-relay`.
- `jarvis-relay` has no internal dependencies (standalone server binary). It is wire-compatible with `jarvis-social`'s `RoomClient` and the desktop relay client, verified by shared JSON fixtures in `jarvis-rs/testdata/relay/`.

---

## 5. Application Lifecycle

### 5.1 Startup Sequence

```
main()
  |
  +-- load_dotenv()                    Load .env (project root / jarvis-rs / cwd)
  +-- install_panic_hook()             Register crash report writer
  +-- cli::parse()                     Parse -e/--execute, -d/--directory, --config, --log-level
  +-- tracing_subscriber::init()       Initialize structured logging
  +-- jarvis_config::load_config()     Load TOML -> apply theme -> discover plugins -> validate
  +-- jarvis_platform::ensure_dirs()   Create ~/.config/jarvis, ~/.local/share/jarvis, etc.
  +-- (set working dir from --directory)
  +-- KeybindRegistry::from_config()   Build keybind lookup table
  +-- EventLoop::new()                 Create winit event loop
  +-- JarvisApp::new(config, registry) Construct app state (no window yet)
  +-- event_loop.run_app(&mut app)     Enter event loop
        |
        +-- ApplicationHandler::resumed()
              |
              +-- initialize_window()
              |     +-- Create winit window (1280x800, transparent on macOS only)
              |     +-- Load window icon from embedded PNG
              |     +-- RenderState::new() (async GPU init via pollster)
              |     +-- BootSequence::new()
              |     +-- initialize_webviews()
              |     |     +-- ContentProvider::new(assets/panels)
              |     |     +-- Register plugin directories
              |     |     +-- WebViewManager::new() + WebViewRegistry::new()
              |     +-- CryptoService load/generate identity
              |     +-- initialize_menu() (native menu bar via muda)
              |
              +-- show_boot_webview() OR setup_default_layout()
              +-- start_presence()           Connect to the relay Room (presence)
              +-- start_relay_client()       Connect to the mobile relay (pairing)
              +-- start_pair()               Wire pair-programming (no-op unless collab.enabled)
              +-- update_window_title()
              +-- request_redraw()
```

### 5.2 Event Loop

The event loop is driven by `winit`'s `ApplicationHandler` trait. `JarvisApp` implements:

- **`resumed()`** -- Called once when the window system is ready. Performs all initialization.
- **`window_event()`** -- Handles:
  - `CloseRequested` -- Triggers shutdown
  - `Resized` -- Reconfigures GPU surface, re-syncs WebView bounds
  - `ScaleFactorChanged` -- Handles DPI changes
  - `CursorMoved` -- Updates cursor icon near resize borders, handles drag-to-resize
  - `MouseInput` -- Starts/stops border drag resize, focuses panes on click, initiates window drag in titlebar zone, forwards clicks to WebView overlays
  - `ModifiersChanged` -- Tracks Ctrl/Alt/Shift/Super state
  - `KeyboardInput` -- Routes through `handle_keyboard_input()` (see Section 7)
  - `RedrawRequested` -- Calls `update_chrome()` + `render_frame()`
- **`about_to_wait()`** -- Called when the event queue is empty. Runs `poll_and_schedule()` which:
  1. Every `POLL_INTERVAL` (16ms, ~60Hz): polls presence, pair, assistant, webview events, PTY output, chat stream, mobile commands, relay events, menu events
  2. If `needs_redraw`: requests redraw and sets `ControlFlow::Poll`
  3. Otherwise: sets `ControlFlow::WaitUntil(now + 16ms)` for power-efficient waiting

### 5.3 Shutdown Sequence

Shutdown is triggered by `Action::Quit` or `WindowEvent::CloseRequested`.

Order (defined in `shutdown.rs`):

1. **Kill all PTY child processes** (`ptys.kill_all()`)
2. **Destroy all WebView panels** (`webviews.destroy_all()`)
3. **Disconnect presence** (drop senders, clear user list)
4. **Shut down mobile relay bridge** (send shutdown signal, clear state)
5. **Shut down tokio runtime** (`rt.shutdown_timeout(2s)` -- cancels background tasks)
6. **Release GPU resources** (`render_state = None`)

Shutdown is idempotent -- calling it twice does not panic.

---

## 6. Key Design Patterns

### 6.1 Action Dispatch

All user interactions resolve to an `Action` enum variant. The `dispatch()` method on `JarvisApp` (`dispatch.rs`) is the single routing point that maps actions to subsystem calls:

```
User Input -> InputProcessor -> Action -> dispatch() -> Subsystem
```

This decouples input handling from business logic. The same `Action::NewPane` can be triggered by a keybind, command palette selection, IPC message, or CLI argument.

### 6.2 Event Bus (Broadcast)

The `EventBus` (in `jarvis-common`) uses `tokio::sync::broadcast` for pub/sub event distribution. Events like `ConfigReloaded`, `PaneOpened`, `PaneFocused`, and `Shutdown` are published centrally and received by any subscriber. This enables loose coupling between subsystems.

### 6.3 Tiling Manager (Binary Split Tree)

The tiling system uses a recursive `SplitNode` binary tree. Each internal node has a direction (horizontal/vertical) and a ratio (0.0-1.0). Leaf nodes hold pane IDs. The `LayoutEngine` traverses this tree with a viewport rect to compute pixel positions for each pane.

This structure supports arbitrary nesting: e.g., `[A | (B / C)]` is a horizontal split where the right side is vertically split.

### 6.4 WebView Registry

WebViews are managed through a `WebViewRegistry` that maps `pane_id -> WebViewHandle`. When the tiling layout changes, `sync_webview_bounds()` recomputes positions and calls `set_bounds()` on each WebView handle. This keeps the WebView positions synchronized with the tiling tree.

### 6.5 IPC Bridge (Rust <-> JavaScript)

Bidirectional communication between Rust and WebView JavaScript:

- **JS -> Rust**: JavaScript calls `window.jarvis.ipc.send(kind, payload)`, which triggers `window.ipc.postMessage(JSON.stringify({kind, payload}))`. The WebView's IPC handler parses this and routes to `handle_ipc_message()`.
- **Rust -> JS**: Rust calls `handle.evaluate_script("...")` or `handle.send_ipc(kind, payload)` which generates `window.jarvis.ipc._dispatch(kind, payload)`.
- **Request/Response**: `window.jarvis.ipc.request(kind, payload)` returns a Promise. Rust responds with a payload containing `_reqId` to resolve the matching pending request.

All IPC messages are validated against an allowlist of 51 permitted `kind` strings (`ALLOWED_IPC_KINDS` in `webview_bridge/ipc_dispatch.rs`). Unknown kinds are rejected and logged.

### 6.6 PTY Bridge

Each terminal pane gets its own PTY process (via `portable-pty`):

- **Input flow**: xterm.js keypress -> IPC `pty_input` -> Rust writes to PTY writer
- **Output flow**: PTY reader thread -> channel -> `poll_pty_output()` -> IPC `pty_output` -> xterm.js `.write()`

PTY processes are spawned with the configured shell program (or platform default) and environment variables.

### 6.7 Custom Protocol (`jarvis://`)

The `ContentProvider` registers a `jarvis://` custom protocol with `wry`. When a WebView requests `jarvis://localhost/terminal/index.html`, the content provider resolves it to `{assets_dir}/panels/terminal/index.html` and returns the file with the correct MIME type.

Panel assets are embedded in the binary at compile time via `include_dir`, so the binary is fully self-contained. The content provider checks the filesystem first (for development and plugin overrides), then falls back to embedded assets. This avoids the need for a local HTTP server and enables in-memory overrides, plugin directory resolution, and embedded fallback, all with security containment (canonicalization-based traversal prevention).

### 6.8 Sync/Async Bridge

The main event loop runs synchronously on the main thread (required by winit). Async operations (the relay Room for presence/pair, AI streaming, the mobile relay client) run on dedicated `tokio::runtime::Runtime`s (multi-thread, 1 worker thread each). Communication uses `std::sync::mpsc` channels (sync -> async) and `tokio::sync::mpsc` channels (async -> sync), drained at ~60Hz (every 16ms) by the main thread.

### 6.9 Crypto Identity

Each Jarvis installation has a persistent cryptographic identity (ECDSA + ECDH P-256 key pairs stored in `~/.local/share/jarvis/identity.json`). All crypto operations (signing, verification, key derivation, encryption, decryption) run in Rust via `jarvis-platform::CryptoService`. WebView JavaScript never touches `crypto.subtle` -- it proxies all operations through IPC, avoiding macOS Keychain prompts and ensuring consistent behavior.

### 6.10 Config Hot Reload

When `Action::ReloadConfig` is dispatched:

1. `jarvis_config::load_config()` reloads from disk
2. `KeybindRegistry` is rebuilt
3. `UiChrome` is reconfigured
4. Plugin directories are re-registered
5. Theme CSS is re-injected into all WebViews
6. `Event::ConfigReloaded` is published on the event bus

---

## 7. Data Flow: Keyboard Input to Action Dispatch

The complete flow of a keyboard event from physical keypress to subsystem effect:

```
Physical key press
    |
    v
[winit] WindowEvent::KeyboardInput { event: KeyEvent }
    |
    v
[event_handler.rs] JarvisApp::handle_keyboard_input()
    |
    +-- Extract logical_key and state (press/release)
    +-- Normalize key name via normalize_winit_key()
    |
    +-- [Gate 1] If command palette is open:
    |     Route to handle_palette_key() -> handles typing, arrows, Enter, Escape
    |     Return early if consumed
    |
    +-- [Gate 2] If assistant panel is open:
    |     Route to handle_assistant_key() -> handles typing, Enter, Escape
    |     Return early if consumed
    |
    +-- Build Modifiers { ctrl, alt, shift, super_key } from self.modifiers
    |
    v
[input_processor/processor.rs] InputProcessor::process_key()
    |
    +-- Build KeyCombo from modifiers + key name
    |
    +-- [Key release] Check for PushToTalk release -> return Action::ReleasePushToTalk
    |   Otherwise return Consumed
    |
    +-- [Key press] Check KeybindRegistry::lookup(combo)
    |     Match found -> return InputResult::Action(action)
    |
    +-- [Terminal mode only] Encode key for terminal via encode_key_for_terminal()
    |     Non-empty bytes -> return InputResult::TerminalInput(bytes)
    |     Empty bytes -> return InputResult::Consumed
    |
    v
[event_handler.rs] Match on InputResult
    |
    +-- Action(action) -> dispatch(action)
    |       |
    |       v
    |   [dispatch.rs] JarvisApp::dispatch(action)
    |       |
    |       +-- Match action variant:
    |           NewPane       -> tiling.split() + create_webview_for_pane()
    |           ClosePane     -> tiling.close_focused() + destroy_webview_for_pane()
    |           FocusPane(n)  -> tiling.focus_pane(n) + notify_focus_changed()
    |           ZoomPane      -> tiling.execute(TilingCommand::Zoom) + sync_webview_bounds()
    |           ResizePane    -> tiling.execute(TilingCommand::Resize) + sync_webview_bounds()
    |           ToggleFullscreen -> window.set_fullscreen()
    |           ToggleBlankPane  -> toggle_blank_for_focused_pane()
    |           OpenCommandPalette -> create CommandPalette (+ inject plugin items),
    |                                 set InputMode::CommandPalette
    |           OpenAssistant -> toggle assistant panel, set InputMode::Assistant
    |           OpenChat / OpenSettings / OpenPair -> open the corresponding panel
    |           Copy          -> evaluate_script() to grab selection from xterm.js
    |           Paste         -> Clipboard::get_text() + evaluate_script() to inject
    |           OpenURL(url)  -> load_url(normalized_url); a jarvis://localhost/plugins/<id>/
    |                            URL may request an opaque webview (e.g. WebGL games)
    |           PairMobile    -> show pairing QR (relay pairing session)
    |           ReloadConfig  -> reload config, rebuild registry, re-inject themes
    |           Quit          -> publish Shutdown event, call shutdown()
    |           ...etc

    Note: there is no LaunchGame action. The bundled games are plugins; they are
    launched from the command palette (plugin items) or via OpenURL, which loads
    jarvis://localhost/plugins/<game>/index.html.
    |
    +-- TerminalInput(bytes) -> (handled natively by xterm.js in focused WebView)
    |
    +-- Consumed -> no-op
```

Note: Most terminal typing never reaches this path. xterm.js in the focused WebView intercepts keystrokes directly and sends `pty_input` IPC messages to Rust. The winit keyboard handler primarily processes modifier-key combinations (keybinds) and overlay interactions.

### 7.1 WebView-Originated Keybinds

On macOS, WKWebView captures Cmd+key before winit sees them. The IPC init script intercepts these in JavaScript and forwards them via IPC:

```
[xterm.js / WebView] document keydown event
    |
    +-- Cmd+key detected (or Escape, or overlay-active keys)
    |
    v
window.jarvis.ipc.send('keybind', { key, ctrl, alt, shift, meta })
    |
    v
[ipc_dispatch.rs] handle_ipc_message(pane_id, body)
    |
    +-- Parse IpcMessage, validate kind against allowlist
    |
    v
[ipc_dispatch.rs] handle_keybind_from_webview(pane_id, payload)
    |
    +-- [Gate 1] Command palette open -> route to handle_palette_key()
    +-- [Gate 2] Assistant open -> route to handle_assistant_key()
    +-- [Gate 3] Escape with active game -> navigate back to original URL
    +-- Build KeyCombo, lookup in registry -> dispatch(action)
```

---

## 8. Archived Python/Swift Prototype vs Rust Rewrite

The original macOS Python + Swift/Metal prototype is no longer in the working tree. It has been archived and preserved in history at the **`legacy-archive`** git tag; check it out with `git checkout legacy-archive`. The mapping below is kept as historical context for how the current Rust crates trace back to that prototype.

### 8.1 Archived Stack (Python + Swift)

The original Jarvis was built with:

- **`main.py`** -- Python entry point handling mic capture, Metal bridge, and event loop
- **`metal-app/`** -- Swift/Metal frontend rendering a 3D reactive orb, hex grid, and chat panels
- **`skills/`** -- Python AI skill system with Gemini routing, Claude Agent SDK integration, code tools, domain hunting, paper matching, stream moderation
- **`voice/`** -- Python audio capture (push-to-talk) and Whisper transcription
- **`connectors/`** -- Python service integrations (Claude proxy, token tracker, SQLite reader, HTTP client)

The archived stack is macOS-only (depends on Metal and AppKit) and requires Python 3.10+ and Swift 5.9+.

### 8.2 Rust Rewrite

The Rust rewrite (`jarvis-rs/`) replaces the entire stack with a cross-platform architecture. The original components below live only at the `legacy-archive` tag:

| Archived Component | Rust Replacement |
|-----------------|------------------|
| `metal-app/` (Swift/Metal) | `jarvis-renderer` (wgpu -- Vulkan/Metal/DX12) |
| `main.py` event loop | `jarvis-app` (winit event loop) |
| Metal 3D orb | wgpu background pipeline (hex grid shader) |
| Swift chat panels | `jarvis-webview` (wry WebViews with HTML/JS/CSS) |
| `skills/router.py` | `jarvis-ai::router::SkillRouter` |
| `skills/claude_code.py` | `jarvis-ai::claude::ClaudeClient` |
| `voice/audio.py` | `jarvis-ai::voice::VoiceRecorder` (cpal mic capture) + `jarvis-ai::whisper::WhisperClient` (transcription) |
| `connectors/token_tracker.py` | `jarvis-ai::token_tracker::TokenTracker` |
| None (new) | `jarvis-ai` multi-provider clients (OpenAI / MiniMax / Gemini) + agentic tool loop with approval gate |
| None (new) | `jarvis-tiling` (binary split tree tiling manager) |
| None (new) | `jarvis-config` (TOML config with validation + live reload) |
| None (new) | `jarvis-platform::CryptoService` (E2E encryption) |
| None (new) | `jarvis-social` (presence + chat over the relay Room transport) |
| None (new) | `jarvis-relay` (mobile bridge + Room/broadcast server) |
| None (new) | Plugin system (`jarvis://` protocol + plugin directories; games, draw, music) |
| None (new) | Experimental pair programming over an encrypted relay Room |

### 8.3 What Exists Where

- The Rust workspace is the only stack; it is self-contained in `jarvis-rs/`
- There are two binaries: `jarvis` (from `jarvis-app`) and `jarvis-relay` (from `jarvis-relay`)
- Bundled web assets (HTML/JS/CSS for panels and the game/draw/music plugins) live in `jarvis-rs/assets/panels/` (with plugins under `assets/panels/plugins/`)
- Shared relay <-> desktop wire fixtures live in `jarvis-rs/testdata/relay/`
- The archived prototype's Python entry point (`main.py`), Swift app (`metal-app/`), and supporting code are no longer tracked in the tree -- they are preserved at the `legacy-archive` git tag (`git checkout legacy-archive` to inspect them)

---

## 9. Configuration Architecture

The configuration system is layered:

1. **Schema** (`jarvis-config::schema::JarvisConfig`) -- 27 strongly-typed config sections, all with `#[serde(default)]`
2. **TOML Loading** -- Reads from `{config_dir}/jarvis/config.toml`, creates default if missing
3. **Theme Application** -- Loads the named theme (skipped for the default `jarvis-dark`), selectively overrides config fields
4. **Plugin Discovery** -- Scans `{config_dir}/jarvis/plugins/` for local plugin directories into `config.plugins.local`
5. **Validation** -- Checks color formats, numeric ranges, enum values, cross-field constraints
6. **Live Reload** -- File watcher triggers `Action::ReloadConfig` which re-runs the pipeline

Config sections (27): theme, colors, font, terminal, shell, window, effects, layout, opacity, background, visualizer, startup, voice, assistant, keybinds, panels, livechat, presence, performance, updates, logging, advanced, auto_open, status_bar, relay, plugins, collab.

---

## 10. Rendering Architecture

The rendering system uses a hybrid approach:

- **GPU-rendered**: Background (hex grid shader), UI chrome (tab bar, status bar, active tab highlight) via `wgpu` quad instancing
- **WebView-rendered**: All pane content (terminals, chat, assistant, games, settings, plugins) via `wry` WebViews positioned over the GPU surface

Per-frame render order:

1. `update_chrome()` -- Update tab bar and status bar state from tiling manager
2. `prepare_chrome_quads()` -- Generate `QuadInstance` data and upload to GPU
3. `render_background()`:
   - Pass 1: Clear with configured color + render hex grid background shader
   - Pass 2: Render UI chrome quads (tab bar background, active tab highlight, status bar)
4. WebViews are composited by the OS window system on top of the GPU surface

The `GpuContext` manages the wgpu device/queue/surface lifecycle. The `BackgroundPipeline` has its own uniform buffer updated each frame with time, viewport size, and config parameters. The `QuadRenderer` uses instanced rendering for efficient multi-quad drawing.
