# WebView System & IPC Bridge

This document is the definitive reference for the Jarvis WebView subsystem and the bidirectional IPC (Inter-Process Communication) bridge between Rust and JavaScript.

---

## Table of Contents

1. [System Overview](#system-overview)
2. [The `jarvis://` Custom Protocol](#the-jarvis-custom-protocol)
3. [ContentProvider](#contentprovider)
4. [WebView Creation & Lifecycle](#webview-creation--lifecycle)
5. [The IPC Bridge: JavaScript API](#the-ipc-bridge-javascript-api)
6. [IPC Message Kinds Reference](#ipc-message-kinds-reference)
7. [IPC Allowlist & Security](#ipc-allowlist--security)
8. [Navigation Handler & Allowlist](#navigation-handler--allowlist)
9. [Theme Bridge](#theme-bridge)
10. [Keyboard Shortcut Interception](#keyboard-shortcut-interception)
11. [Command Palette Overlay](#command-palette-overlay)
12. [Clipboard Integration](#clipboard-integration)
13. [WebViewHandle Rust API](#webviewhandle-rust-api)
14. [WebViewRegistry Lifecycle](#webviewregistry-lifecycle)

---

## System Overview

Jarvis embeds web content into tiling panes using the [wry](https://github.com/nickel-org/wry) crate, which provides a cross-platform WebView wrapper (WKWebView on macOS, WebView2 on Windows, WebKitGTK on Linux).

**Key architectural decisions:**

- **One WebView per pane.** Every tiling pane that displays web content (terminal, chat, assistant, settings, presence, games) gets its own `wry::WebView` instance.
- **Child WebViews.** All WebViews are created as children of the main application window via `build_as_child()`, positioned and sized using the tiling layout engine.
- **Custom protocol.** Bundled HTML/JS/CSS assets are served through a `jarvis://` custom protocol rather than a local HTTP server.
- **Bidirectional IPC.** JavaScript sends messages to Rust via `window.ipc.postMessage()`. Rust sends messages to JavaScript via `webview.evaluate_script()`.
- **No raw DOM access from Rust.** All communication between Rust and the web layer happens through typed JSON messages.

### Crate Layout

| Crate | Path | Responsibility |
|-------|------|----------------|
| `jarvis-webview` | `jarvis-rs/crates/jarvis-webview/` | WebView primitives: IPC types, content serving, manager, registry, theme bridge |
| `jarvis-app` (webview_bridge) | `jarvis-rs/crates/jarvis-app/src/app_state/webview_bridge/` | Application-level IPC dispatch, lifecycle, handler implementations |

---

## The `jarvis://` Custom Protocol

### URL Format

All bundled panel assets are loaded via the `jarvis://` scheme:

```
jarvis://localhost/{path-to-asset}
```

Examples:
```
jarvis://localhost/terminal/index.html
jarvis://localhost/chat/index.html
jarvis://localhost/assistant/index.html
jarvis://localhost/settings/index.html
jarvis://localhost/presence/index.html
jarvis://localhost/games/tetris.html
jarvis://localhost/boot/index.html
jarvis://localhost/plugins/{plugin-id}/index.html
```

### Panel URL Mapping

Each `PaneKind` maps to a specific URL:

| PaneKind | URL |
|----------|-----|
| `Terminal` | `jarvis://localhost/terminal/index.html` |
| `Assistant` | `jarvis://localhost/assistant/index.html` |
| `Chat` | `jarvis://localhost/chat/index.html` |
| `WebView` | `jarvis://localhost/terminal/index.html` (fallback) |
| `ExternalApp` | `jarvis://localhost/terminal/index.html` (fallback) |

Additional panels resolved by name (not by `PaneKind`):

| Panel Name | URL |
|------------|-----|
| `settings` | `jarvis://localhost/settings/index.html` |
| `presence` | `jarvis://localhost/presence/index.html` |

### URI Resolution

The custom protocol handler strips the scheme prefix to extract the asset path. It tries multiple prefix patterns to handle platform differences:

1. `jarvis://localhost/{path}`
2. `jarvis://localhost` (bare, no trailing slash)
3. `jarvis:///{path}`
4. `jarvis://{path}`

### CORS

Every response from the custom protocol includes the header:

```
Access-Control-Allow-Origin: jarvis://localhost
```

This allows same-origin requests between assets served from the custom protocol.

### Windows (WebView2) Behavior

On Windows, WebView2 rewrites custom protocols:

```
jarvis://localhost/... --> http://jarvis.localhost/...
```

The navigation allowlist includes `http://jarvis.localhost` to handle this rewriting transparently.

### Error Handling

If an asset is not found, the protocol handler returns:

```
HTTP 404 - "Not Found"
```

A warning is logged via `tracing::warn!` with the requested path.

---

## ContentProvider

**Source:** `jarvis-rs/crates/jarvis-webview/src/content.rs`

The `ContentProvider` resolves `jarvis://` URL paths to file contents and MIME types. It supports four resolution strategies, checked in order:

### 1. In-Memory Overrides

Dynamically generated content registered via `add_override(path, mime, data)`. Overrides take absolute priority over filesystem resolution.

```rust
provider.add_override(
    "panels/chat/index.html",
    "text/html",
    b"<html>override</html>".to_vec(),
);
```

### 2. Plugin Directories

Paths matching `plugins/{plugin_id}/{asset_path}` are resolved from registered plugin directories:

```rust
provider.add_plugin_dir("my-plugin", "/path/to/plugin/dir");
// Resolves: plugins/my-plugin/index.html -> /path/to/plugin/dir/index.html
```

Plugin directories are stored in an `Arc<RwLock<HashMap<String, PathBuf>>>` so they can be shared between the custom protocol closure and the app state for hot-reloading.

Methods:
- `add_plugin_dir(id, path)` -- Register a plugin directory
- `clear_plugin_dirs()` -- Remove all plugin directories
- `plugin_dirs_handle()` -- Get a shared `Arc<RwLock<...>>` handle

### 3. Base Directory (Filesystem)

Falls back to reading from `{base_dir}/{clean_path}` on disk. The base directory is typically the `assets/` folder at the workspace root. This is useful during development to iterate on panel HTML without rebuilding.

### 4. Embedded Assets (Compile-Time)

If the file is not found on disk (or the base directory does not exist), the content provider falls back to assets embedded in the binary at compile time via `include_dir`. The `assets/panels/` directory is statically included in the `jarvis-webview` crate, making the binary fully self-contained. The embedded path strips the `panels/` prefix since the embedded directory is rooted at `assets/panels/`.

```rust
static EMBEDDED_PANELS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/panels");
```

This means the binary works correctly regardless of working directory -- no need to ship the `assets/` folder alongside the executable.

### Security: Directory Traversal Prevention

All filesystem resolution includes canonicalization-based containment checks:

```rust
let canonical_base = std::fs::canonicalize(&self.base_dir).ok()?;
let canonical_file = std::fs::canonicalize(&file_path).ok()?;
if !canonical_file.starts_with(&canonical_base) {
    return None; // Blocked
}
```

This blocks:
- `../../etc/passwd`
- `panels/../../../etc/passwd`
- Symlink-based traversal
- Absolute paths like `/etc/passwd`

Plugin directories have their own independent containment check.

### MIME Type Detection

MIME types are determined by file extension:

| Extension | MIME Type |
|-----------|-----------|
| `.html`, `.htm` | `text/html` |
| `.css` | `text/css` |
| `.js`, `.mjs` | `application/javascript` |
| `.json` | `application/json` |
| `.png` | `image/png` |
| `.jpg`, `.jpeg` | `image/jpeg` |
| `.gif` | `image/gif` |
| `.svg` | `image/svg+xml` |
| `.wasm` | `application/wasm` |
| `.ico` | `image/x-icon` |
| `.woff` | `font/woff` |
| `.woff2` | `font/woff2` |
| `.ttf` | `font/ttf` |
| `.otf` | `font/otf` |
| `.mp3` | `audio/mpeg` |
| `.ogg` | `audio/ogg` |
| `.wav` | `audio/wav` |
| `.mp4` | `video/mp4` |
| `.webm` | `video/webm` |
| `.webp` | `image/webp` |
| `.txt` | `text/plain` |
| `.xml` | `application/xml` |
| (unknown) | `application/octet-stream` |

---

## WebView Creation & Lifecycle

**Source:** `jarvis-rs/crates/jarvis-webview/src/manager/lifecycle.rs`, `jarvis-rs/crates/jarvis-app/src/app_state/webview_bridge/lifecycle.rs`

### WebViewConfig

Every WebView is created from a `WebViewConfig`:

```rust
pub struct WebViewConfig {
    pub url: Option<String>,         // Initial URL (mutually exclusive with html)
    pub html: Option<String>,        // Initial HTML (mutually exclusive with url)
    pub transparent: bool,           // Default: true on macOS, false on Windows/Linux
    pub devtools: bool,              // Default: true in debug, false in release
    pub user_agent: Option<String>,  // Default: "Jarvis/0.1"
    pub clipboard: bool,             // Default: true
    pub autoplay: bool,              // Default: true
}
```

Convenience constructors:
- `WebViewConfig::with_url(url)` -- Load a URL with all other defaults
- `WebViewConfig::with_html(html)` -- Render inline HTML with all other defaults

### Creation Sequence

When `WebViewManager::create()` is called:

1. **Builder initialization** -- `WebViewBuilder::new()` with bounds, transparency, devtools, clipboard, autoplay, and `focused(false)`.

2. **IPC init script injection** -- The `IPC_INIT_SCRIPT` is injected via `with_initialization_script()`. This script runs before any page JavaScript and sets up the `window.jarvis.ipc` bridge.

3. **User agent** -- Set to `"Jarvis/0.1"` by default.

4. **Handler attachment** -- Four handlers are attached to the builder:
   - **IPC handler** (`with_ipc_handler`) -- Validates JSON and pushes `WebViewEvent::IpcMessage` to the event queue.
   - **Page load handler** (`with_on_page_load_handler`) -- Pushes `WebViewEvent::PageLoad` with `Started` or `Finished` state.
   - **Title change handler** (`with_document_title_changed_handler`) -- Pushes `WebViewEvent::TitleChanged`.
   - **Navigation handler** (`with_navigation_handler`) -- Allowlist check; blocks disallowed URLs, pushes `WebViewEvent::NavigationRequested`.

5. **Custom protocol** -- The `jarvis://` protocol is registered via `with_custom_protocol()` if a `ContentProvider` is set.

6. **Initial content** -- Sets URL, HTML, or a blank page.

7. **Build** -- `builder.build_as_child(window)` creates the WebView as a child of the main OS window.

8. **Theme injection** -- After creation, `inject_theme_into_all_webviews()` is called to push CSS variables and xterm theme to the new panel.

### Application-Level Lifecycle

In the application layer (`webview_bridge/lifecycle.rs`):

- `create_webview_for_pane(pane_id)` -- Creates with `PaneKind::Terminal` default.
- `create_webview_for_pane_with_kind(pane_id, kind)` -- Creates with the panel URL for the given kind.
- `create_webview_for_pane_with_url(pane_id, url)` -- Creates with an arbitrary URL.
- `destroy_webview_for_pane(pane_id)` -- Kills the PTY (if any), then destroys the WebView.
- `show_boot_webview()` -- Creates a fullscreen WebView for the boot animation at pane ID 0.
- `handle_boot_complete()` -- Destroys boot WebView, sets up default panel layout.
- `sync_webview_bounds()` -- Recomputes tiling layout and repositions all WebViews.
- `poll_webview_events()` -- Drains the event queue and dispatches each event.

### Boot Sequence

1. App creates a fullscreen boot WebView at pane ID 0 loading `jarvis://localhost/boot/index.html`.
2. The boot page runs its animation.
3. JavaScript sends `boot_complete` IPC message.
4. Rust destroys the boot WebView, creates the default panel layout.

### Coordinate Conversion

**Source:** `jarvis-rs/crates/jarvis-app/src/app_state/webview_bridge/bounds.rs`

Tiling rects (f64 logical coordinates) are converted to wry rects:

```rust
fn tiling_rect_to_wry(rect: &Rect) -> wry::Rect {
    wry::Rect {
        position: wry::dpi::Position::Logical(LogicalPosition::new(rect.x, rect.y)),
        size: wry::dpi::Size::Logical(LogicalSize::new(rect.width, rect.height)),
    }
}
```

The tiling viewport accounts for:
- macOS custom titlebar height (top offset)
- Tab bar height
- Status bar height (bottom offset)

---

## The IPC Bridge: JavaScript API

**Source:** `jarvis-rs/crates/jarvis-webview/src/ipc.rs` (the `IPC_INIT_SCRIPT` constant)

The IPC init script is injected into every WebView before page content loads. It creates the `window.jarvis.ipc` object with the following API:

### `window.jarvis.ipc.send(kind, payload)`

Fire-and-forget message to Rust. The message is JSON-serialized and sent via `window.ipc.postMessage()`.

```javascript
window.jarvis.ipc.send('pty_input', { data: 'ls\n' });
```

Wire format:
```json
{ "kind": "pty_input", "payload": { "data": "ls\n" } }
```

### `window.jarvis.ipc.postMessage(msg)`

Low-level method that JSON-stringifies an arbitrary object and sends it. Used internally by `send()`.

```javascript
window.jarvis.ipc.postMessage({ kind: 'ping', payload: null });
```

### `window.jarvis.ipc.on(kind, callback)`

Register a handler for messages dispatched from Rust to JavaScript.

```javascript
window.jarvis.ipc.on('pty_output', function(payload) {
    xterm.write(payload.data);
});
```

Rust dispatches to these handlers by evaluating:
```javascript
window.jarvis.ipc._dispatch("pty_output", {"data": "..."});
```

### `window.jarvis.ipc.request(kind, payload)`

Request-response pattern for async Rust calls. Returns a `Promise` that resolves with the Rust response or rejects on timeout (10 seconds).

```javascript
window.jarvis.ipc.request('clipboard_paste', {}).then(function(resp) {
    console.log(resp.text);
});
```

**How it works:**

1. JS assigns a unique `_reqId` to the payload and stores resolve/reject callbacks in `_pendingRequests`.
2. The message is sent to Rust via `send()`.
3. Rust processes the request and sends a response back via `send_ipc()` with the same `_reqId` in the payload.
4. The overridden `_dispatch()` function checks for `_reqId` in incoming messages. If found, it resolves or rejects the matching pending Promise.
5. If `payload.error` is set, the Promise rejects. Otherwise, it resolves with `payload.result` (or the full payload if no `result` key).
6. A 10-second timeout auto-rejects abandoned requests.

### `window.jarvis.ipc._dispatch(kind, payload)`

Internal method. Rust calls this (via `evaluate_script`) to push messages to JavaScript. The dispatch function first checks for `_reqId` (request-response pattern), then falls through to registered `on()` handlers.

### `window.jarvis.ipc._handlers`

Internal map of `kind -> callback` registered via `on()`.

### `window.jarvis.ipc._pendingRequests`

Internal map of `reqId -> { resolve, reject }` for pending request-response calls.

---

## IPC Message Kinds Reference

### Messages from JavaScript to Rust (JS -> Rust)

Every message must have a `kind` field that appears in the `ALLOWED_IPC_KINDS` allowlist or it is rejected.

#### Terminal / PTY

| Kind | Payload | Description |
|------|---------|-------------|
| `pty_input` | `{ data: string }` | Write keystroke data to the pane's PTY |
| `pty_resize` | `{ cols: number, rows: number }` | Resize the PTY (sanity-checked: 1-500) |
| `pty_restart` | `{ cols: number, rows: number }` | Kill existing PTY and spawn a new one |
| `terminal_ready` | `{ cols: number, rows: number }` | Terminal panel loaded; spawn a PTY if not already present |

#### Panel Management

| Kind | Payload | Description |
|------|---------|-------------|
| `panel_focus` | `{}` | The user clicked in this panel; switch tiling focus |
| `open_panel` | `{ panel: "terminal"\|"assistant"\|"chat"\|"settings"\|"presence" }` | Open a new panel (splits the focused pane horizontally) |
| `panel_close` | (none) | Close this panel (refused if it's the last pane) |
| `panel_toggle` | `{ panel: string }` | Toggle a panel from the status bar |
| `open_settings` | (none) | Open or focus the settings panel |
| `launch_game` | `{ game: "tetris"\|"asteroids"\|"minesweeper"\|"pinball"\|"doodlejump"\|"draw"\|"subway"\|"videoplayer"\|"emulator" }` | Navigate the current pane to a game; stores original URL for Escape-back |

#### Settings

| Kind | Payload | Description |
|------|---------|-------------|
| `settings_init` | (none) | Settings panel loaded; requests current config, themes list |
| `settings_set_theme` | `{ name: string }` | Switch to a named theme; reloads and re-injects into all panels |
| `settings_update` | `{ path: string, value: any }` | Update a single config field (auto-saves to TOML, broadcasts CSS) |
| `settings_reset_section` | `{ section: string }` | Reset an entire config section to defaults |
| `settings_get_config` | (none) | Request full config JSON |

#### Presence

| Kind | Payload | Description |
|------|---------|-------------|
| `presence_request_users` | (none) | Request current online user list |
| `presence_poke` | `{ target_user_id: string }` | Send a poke to another user (max 64 chars) |

#### Assistant

| Kind | Payload | Description |
|------|---------|-------------|
| `assistant_input` | `{ text: string }` | User input to the AI assistant (max 4096 chars) |
| `assistant_ready` | (none) | Assistant panel loaded and ready for messages |

#### Crypto

| Kind | Payload | Description |
|------|---------|-------------|
| `crypto` | `{ _reqId: number, op: string, params: object }` | Crypto operation (request-response pattern). Operations: `init`, `derive_room_key`, `derive_shared_key`, `encrypt`, `decrypt`, `sign`, `verify` |

#### File I/O

| Kind | Payload | Description |
|------|---------|-------------|
| `read_file` | `{ _reqId: number, path: string }` | Read a local image file, returns base64 data URL (max 5MB, validated by magic bytes) |

#### Clipboard

| Kind | Payload | Description |
|------|---------|-------------|
| `clipboard_copy` | `{ text: string }` | Copy text to the system clipboard |
| `clipboard_paste` | `{ _reqId: number }` | Read from clipboard; returns image (PNG data URL) or text |

#### Window

| Kind | Payload | Description |
|------|---------|-------------|
| `window_drag` | (none) | Initiate OS window drag |
| `open_url` | `{ url: string }` | Open a URL in the default browser |

#### Navigation / Keyboard

| Kind | Payload | Description |
|------|---------|-------------|
| `keybind` | `{ key: string, ctrl: bool, alt: bool, shift: bool, meta: bool }` | Keyboard shortcut forwarded from webview JS (because WKWebView captures Cmd+key before winit) |

#### Command Palette

| Kind | Payload | Description |
|------|---------|-------------|
| `palette_click` | `{ index: number }` | User clicked an item in the command palette overlay |
| `palette_hover` | `{ index: number }` | User hovered over an item in the command palette |
| `palette_dismiss` | `{}` | User clicked the backdrop to dismiss the palette |

#### Status Bar

| Kind | Payload | Description |
|------|---------|-------------|
| `status_bar_init` | (none) | Status bar loaded; requests current app state |

#### System

| Kind | Payload | Description |
|------|---------|-------------|
| `ping` | (none) | IPC round-trip test; Rust responds with `pong` |
| `boot_complete` | (none) | Boot animation finished; destroy boot webview, load panels |
| `debug_event` | `{ type: string, ... }` | Diagnostic event from JS (mousedown, keydown, focus, blur) |

### Messages from Rust to JavaScript (Rust -> JS)

These are dispatched via `handle.send_ipc(kind, payload)` which calls `window.jarvis.ipc._dispatch(kind, payload)`.

| Kind | Payload | Description |
|------|---------|-------------|
| `pong` | `"pong"` | Response to `ping` |
| `pty_output` | `{ data: string }` | PTY output bytes (UTF-8 lossy) for xterm.js |
| `pty_exit` | `{ code: number }` | PTY process exited |
| `presence_users` | `{ users: [{ user_id, display_name, status, activity }] }` | Online user list |
| `presence_update` | `{ status: string }` | Online count status line (e.g., `"[ 3 online ]"`) |
| `presence_notification` | `{ line: string }` | Notification text for presence panel |
| `settings_data` | `{ currentTheme, availableThemes, config }` | Full config JSON for settings panel |
| `settings_saved` | `{ path: string }` | Confirmation that a setting was saved |
| `settings_field_warning` | `{ path: string, message: string }` | Validation warning for a setting field |
| `status_update` | `{ online_count, active_panel, connection }` | App state for status bar |
| `focus_changed` | `{ focused: bool }` | Whether this panel has focus |
| `theme` | `{ xterm: {...}, fontSize, fontFamily, ... }` | xterm.js theme object (dispatched via `generate_xterm_theme_js`) |
| `crypto_response` | `{ _reqId, result: {...} }` or `{ _reqId, error: string }` | Response to a crypto request |
| `read_file_response` | `{ _reqId, data_url: string }` or `{ _reqId, error: string }` | Response to a read_file request |
| `clipboard_paste_response` | `{ _reqId, kind: "image"\|"text", data_url?, text? }` or `{ _reqId, error }` | Response to clipboard_paste request |
| `palette_show` | `{ items, query, selectedIndex, mode, placeholder }` | Show the command palette overlay |
| `palette_update` | `{ items, query, selectedIndex, mode, placeholder }` | Update the command palette display |
| `palette_hide` | (none) | Hide the command palette overlay |

---

## IPC Allowlist & Security

**Source:** `jarvis-rs/crates/jarvis-app/src/app_state/webview_bridge/ipc_dispatch.rs`

### Allowlist

Every IPC message from JavaScript is checked against a static allowlist before dispatch. Messages with an unknown `kind` are rejected and logged as warnings.

The complete allowlist:

```
pty_input, pty_resize, pty_restart, terminal_ready,
panel_focus, presence_request_users, presence_poke,
settings_init, settings_set_theme, settings_update,
settings_reset_section, settings_get_config,
assistant_input, assistant_ready,
open_panel, panel_close, panel_toggle, open_settings,
status_bar_init, launch_game, ping, boot_complete,
crypto, window_drag, keybind, read_file,
clipboard_copy, clipboard_paste, open_url,
palette_click, palette_hover, palette_dismiss,
debug_event
```

The check is case-sensitive and exact-match. `"PTY_INPUT"`, `"pty_input_extra"`, `"pty_input\0"`, `"ping; rm -rf /"` are all rejected.

### JSON Validation

Before the IPC message even reaches the allowlist check, the raw `ipc_handler` in `handlers.rs` validates that the body is valid JSON:

```rust
if serde_json::from_str::<serde_json::Value>(&body).is_err() {
    warn!("IPC message rejected: invalid JSON");
    return;
}
```

### Message Parsing

The body is then parsed as an `IpcMessage`:

```rust
pub struct IpcMessage {
    pub kind: String,
    pub payload: IpcPayload,
}

pub enum IpcPayload {
    Text(String),
    Json(serde_json::Value),
    None,
}
```

If parsing fails, the message is rejected.

### Additional Per-Handler Validation

Individual handlers perform their own validation:
- **Panel names** are checked against `ALLOWED_PANELS`: `["terminal", "assistant", "chat", "settings", "presence"]`
- **Game names** are checked against `ALLOWED_GAMES`: `["tetris", "asteroids", "minesweeper", "pinball", "doodlejump", "draw", "subway", "videoplayer", "emulator"]`
- **Settings paths** are checked against `VALID_PATHS` (100+ whitelisted dotted paths like `"colors.primary"`, `"font.size"`)
- **PTY resize** values are sanity-checked: cols/rows must be 1-500
- **Assistant input** is capped at 4096 characters
- **Presence poke** target_user_id must be non-empty and at most 64 characters
- **Crypto operations** are validated by operation name: `init`, `derive_room_key`, `derive_shared_key`, `encrypt`, `decrypt`, `sign`, `verify`
- **Display names** are sanitized: only alphanumeric, spaces, hyphens; truncated to 20 chars

---

## Navigation Handler & Allowlist

**Source:** `jarvis-rs/crates/jarvis-webview/src/manager/handlers.rs`

The navigation handler controls which URLs a WebView is allowed to load. It uses a two-tier approach:

### Tier 1: HTTP/HTTPS Always Allowed

Any URL starting with `https://` or `http://` is permitted. This allows panels to load CDN resources (e.g., Supabase, xterm.js) and navigate to external games.

### Tier 2: Non-HTTP Scheme Allowlist

For non-HTTP/HTTPS schemes, the URL must match one of the `ALLOWED_NAV_PREFIXES`:

| Prefix | Purpose |
|--------|---------|
| `jarvis://` | Custom protocol for bundled assets |
| `http://jarvis.localhost` | WebView2 (Windows) rewrite of `jarvis://` |
| `about:blank` | Default empty page |

### Blocked Schemes

The following are blocked:
- `file://` -- Prevents reading local filesystem
- `javascript:` -- Prevents JS injection via URL
- `data:` -- Prevents inline content injection
- `ftp://` -- Not needed
- Empty strings, garbage

### Behavior on Block

When a navigation is blocked:
1. A `tracing::warn!` is emitted with the pane ID and URL.
2. The handler returns `false`, preventing the navigation.
3. No `WebViewEvent::NavigationRequested` is emitted for blocked URLs.

---

## Theme Bridge

**Source:** `jarvis-rs/crates/jarvis-webview/src/theme_bridge/`, `jarvis-rs/crates/jarvis-app/src/app_state/webview_bridge/theme_handlers.rs`

The theme bridge converts Jarvis configuration values into CSS custom properties and xterm.js theme objects, then injects them into all active WebViews.

### CSS Variable Injection

The config is mapped to 33 CSS custom properties:

**Colors (12):**
`--color-primary`, `--color-secondary`, `--color-background`, `--color-panel-bg`, `--color-text`, `--color-text-muted`, `--color-border`, `--color-border-focused`, `--color-user-text`, `--color-success`, `--color-warning`, `--color-error`

**Font (6):**
`--font-family`, `--font-size`, `--font-title-size`, `--line-height`, `--font-ui`, `--font-ui-size`

**Layout (7):**
`--border-radius`, `--panel-padding`, `--panel-gap`, `--scrollbar-width`, `--border-width`, `--outer-padding`, `--inactive-opacity`

**Effects (4):**
`--blur-radius`, `--saturate`, `--transition-speed`, `--glow-intensity`

**Other (4):**
`--panel-opacity`, `--titlebar-height`, `--status-bar-height`, `--status-bar-bg`

### Injection Mechanism

CSS variables are injected live (no page reload) via `document.documentElement.style.setProperty()`:

```javascript
(function() {
  var s = document.documentElement.style;
  s.setProperty('--color-primary', '#cba6f7');
  s.setProperty('--font-size', '13px');
  // ...
})();
```

### xterm.js Theme

Terminal panels receive a separate theme object via `window.jarvis.ipc._dispatch('theme', {...})`:

```json
{
  "xterm": {
    "background": "#1e1e2e",
    "foreground": "#cdd6f4",
    "cursor": "#cba6f7",
    "cursorAccent": "#1e1e2e",
    "selectionBackground": "rgba(203, 166, 247, 0.25)",
    "selectionForeground": "#1e1e2e",
    "black": "#45475a", "red": "#f38ba8", "green": "#a6e3a1",
    "yellow": "#f9e2af", "blue": "#89b4fa", "magenta": "#f5c2e7",
    "cyan": "#94e2d5", "white": "#bac2de",
    "brightBlack": "#585b70", "brightRed": "#f38ba8",
    "brightGreen": "#a6e3a1", "brightYellow": "#f9e2af",
    "brightBlue": "#89b4fa", "brightMagenta": "#f5c2e7",
    "brightCyan": "#94e2d5", "brightWhite": "#a6adc8"
  },
  "fontSize": 13,
  "fontFamily": "'Menlo', monospace",
  "lineHeight": 1.6,
  "fontWeight": 400,
  "fontWeightBold": 700,
  "cursorStyle": "block",
  "cursorBlink": true,
  "scrollback": 10000
}
```

The theme is also stored at `window.__jarvis_theme` for panels that need to read it synchronously.

Cursor style mapping: `Block` -> `"block"`, `Underline` -> `"underline"`, `Beam` -> `"bar"` (xterm.js naming convention).

### CSS Value Sanitization

**Source:** `jarvis-rs/crates/jarvis-webview/src/theme_bridge/sanitize.rs`

All CSS values are validated before injection to prevent CSS injection attacks:

**Color validation** (`validate_css_color`):
- Accepts: hex (`#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`) and `rgb()`/`rgba()` with numeric arguments
- Rejects: named colors (`red`, `blue`, `transparent`), anything else

**Font family validation** (`validate_css_font_family`):
- Accepts: alphanumeric characters, spaces, hyphens, underscores, quotes, commas
- Rejects: any other special characters

**Numeric validation** (`validate_css_numeric`):
- Accepts: numbers with optional units (`px`, `em`, `rem`, `%`)
- Rejects: non-numeric strings

**Injection pattern blocking** (applied to all types):
Rejects values containing any of:
```
expression(  url(  javascript:  eval(  import
@import  @charset  behavior:  -moz-binding
;  {  }  <  >
```

Invalid values are silently skipped (with a warning log), never injected.

### Theme Change Flow

1. User sends `settings_set_theme` with `{ name: "mocha" }`.
2. Rust loads the theme overrides from config.
3. Applies overrides to the in-memory `JarvisConfig`.
4. Calls `inject_theme_into_all_webviews()`.
5. For each active WebView: evaluates CSS injection JS and xterm theme JS.
6. All panels update their colors live.

---

## Keyboard Shortcut Interception

**Source:** The `IPC_INIT_SCRIPT` in `jarvis-rs/crates/jarvis-webview/src/ipc.rs`

WKWebView (macOS) captures Cmd+key events before the winit event loop sees them. To make application keybinds work, the init script intercepts keyboard events in the WebView and forwards them to Rust via IPC.

### Overlay Mode

When an overlay (command palette or assistant) is active, ALL non-repeat keydown events are intercepted:

```javascript
if (_overlayActive && !e.repeat) {
    e.preventDefault();
    e.stopPropagation();
    window.jarvis.ipc.send('keybind', {
        key: e.key, ctrl: e.ctrlKey, alt: e.altKey,
        shift: e.shiftKey, meta: e.metaKey
    });
    return;
}
```

The overlay state is controlled by `window.jarvis._setOverlayActive(bool)`.

### Escape Key

Escape is always forwarded to Rust (without `preventDefault`, so terminals still receive it via xterm.js):

```javascript
if (e.key === 'Escape' && !e.repeat) {
    window.jarvis.ipc.send('keybind', {
        key: 'Escape', ctrl: false, alt: false, shift: false, meta: false
    });
    return;
}
```

In Rust, Escape is used for:
- Exiting games (navigates back to the original panel URL)
- Closing overlays

### Meta (Cmd) Key Shortcuts

When `e.metaKey` is pressed:

| Key | Behavior |
|-----|----------|
| `Cmd+C` | Grabs selection from `window._xtermInstance.getSelection()` or `window.getSelection()`, sends `clipboard_copy` IPC |
| `Cmd+V` | Sends `clipboard_paste` IPC request, dispatches result to active element or xterm |
| `Cmd+R` | Passed through to webview (page reload) |
| `Cmd+L` | Passed through to webview |
| `Cmd+Q` | Passed through to webview |
| `Cmd+A` | Passed through to webview (select all) |
| `Cmd+X` | Passed through to webview (cut) |
| `Cmd+Z` | Passed through to webview (undo) |
| All other `Cmd+key` | Forwarded to Rust as `keybind` IPC, with `preventDefault` |

### Keybind Dispatch in Rust

**Source:** `jarvis-rs/crates/jarvis-app/src/app_state/webview_bridge/ipc_dispatch.rs`

When a `keybind` message arrives:

1. If the command palette is open, keys are routed to the palette handler. `Cmd+V` in palette mode pastes clipboard text into the search query.
2. If the assistant overlay is open, keys are routed to the assistant handler.
3. If Escape is pressed and a game is active on this pane, the game exits (navigates back to the original URL).
4. Otherwise, the key combo is looked up in the keybind registry and the corresponding action is dispatched.

### Diagnostic Event Logging

The init script also logs diagnostic events (mousedown, keydown, focus, blur) via `debug_event` IPC messages. `mousedown` additionally sends `panel_focus` to switch tiling focus.

---

## Command Palette Overlay

**Source:** The `IPC_INIT_SCRIPT` in `jarvis-rs/crates/jarvis-webview/src/ipc.rs`

The command palette is a DOM overlay injected by the IPC init script into every WebView. It provides a fuzzy-searchable command launcher similar to VS Code's command palette.

### DOM Structure

```
#_cp_overlay         (fixed fullscreen backdrop with blur)
  #_cp_panel         (centered modal, 480px wide, max 380px tall)
    #_cp_search      (search bar with icon and query)
      .icon          (">" for commands, "url:" for URL input mode)
      #_cp_query     (displayed query text with blinking cursor)
    #_cp_items       (scrollable list of items)
      ._cp_header    (category header, shown when not filtering)
      ._cp_item      (command row: label + keybind)
```

### Styles

Styles are lazily injected into `<head>` on first show. They use CSS custom properties from the theme bridge (e.g., `var(--color-panel-bg, #1e1e2e)`) with fallback defaults.

### Modes

- **Command mode** (`mode !== 'url_input'`): Shows filtered command items.
- **URL input mode** (`mode === 'url_input'`): Shows a hint ("Type a URL and press Enter"), icon changes to "url:".

### IPC Integration

**Rust -> JS:**
- `palette_show` -- Calls `window._showCommandPalette(items, query, selectedIndex, mode, placeholder)`
- `palette_update` -- Calls `window._updateCommandPalette(items, query, selectedIndex, mode, placeholder)`
- `palette_hide` -- Calls `window._hideCommandPalette()`

**JS -> Rust:**
- `palette_click { index }` -- User clicked item at index
- `palette_hover { index }` -- User hovered over item at index
- `palette_dismiss {}` -- User clicked backdrop

### Keyboard Blocking

When the palette is visible, a capturing keydown listener blocks Escape, Enter, ArrowUp, ArrowDown, Backspace, Tab, and printable characters from reaching the underlying page content. These are instead forwarded via the overlay keybind mechanism.

### Item Format

Each item in the `items` array:

```json
{
  "label": "New Terminal",
  "keybind": "Cmd+N",
  "category": "Panels"
}
```

Categories are displayed as section headers when no search query is active.

---

## Clipboard Integration

**Source:** `IPC_INIT_SCRIPT`, `jarvis-rs/crates/jarvis-app/src/app_state/webview_bridge/ipc_dispatch.rs`, `file_handlers.rs`

WKWebView (macOS) blocks direct clipboard access from JavaScript. Jarvis works around this by proxying all clipboard operations through Rust via IPC.

### Copy Flow (Cmd+C)

1. The JS keydown handler intercepts `Cmd+C`.
2. It tries to get selected text from `window._xtermInstance.getSelection()` (xterm.js) first, then `window.getSelection()` (DOM).
3. If text is found, sends `clipboard_copy { text }` IPC to Rust.
4. Rust writes the text to the system clipboard via `jarvis_platform::Clipboard`.

### Paste Flow (Cmd+V)

1. The JS keydown handler intercepts `Cmd+V`, prevents default, and stops propagation.
2. Sends `clipboard_paste` IPC request (request-response pattern).
3. Rust reads the system clipboard:
   - **Image data**: Encodes RGBA pixels as PNG, returns `{ kind: "image", data_url: "data:image/png;base64,..." }`.
   - **Text data**: Returns `{ kind: "text", text: "..." }`.
   - **Empty**: Returns `{ error: "clipboard empty" }`.
4. JS receives the response:
   - **Image**: Dispatches a `jarvis:paste-image` CustomEvent on the document.
   - **Text**: If an input/textarea/contentEditable is focused, inserts via `document.execCommand('insertText')`. If an xterm instance exists, sends `pty_input { data }`.

### Clipboard API Polyfill

The init script also overrides `navigator.clipboard.writeText` to proxy through IPC:

```javascript
navigator.clipboard.writeText = function(text) {
    return new Promise(function(resolve) {
        window.jarvis.ipc.send('clipboard_copy', { text: text });
        resolve();
    });
};
```

This allows games and web apps that use the standard Clipboard API to function correctly.

---

## WebViewHandle Rust API

**Source:** `jarvis-rs/crates/jarvis-webview/src/manager/handle.rs`

`WebViewHandle` wraps a `wry::WebView` instance and provides a safe, typed API:

```rust
pub struct WebViewHandle {
    webview: WebView,       // Underlying wry WebView
    pane_id: u32,           // Pane ID this WebView belongs to
    current_url: String,    // Best-effort URL tracking
    current_title: String,  // Current document title
}
```

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `pane_id()` | `-> u32` | Get the pane ID |
| `current_url()` | `-> &str` | Get the tracked current URL |
| `current_title()` | `-> &str` | Get the tracked current title |
| `load_url(url)` | `-> Result<(), wry::Error>` | Navigate to a URL |
| `load_html(html)` | `-> Result<(), wry::Error>` | Load raw HTML (sets URL to `about:blank`) |
| `evaluate_script(js)` | `-> Result<(), wry::Error>` | Execute arbitrary JavaScript |
| `send_ipc(kind, payload)` | `-> Result<(), wry::Error>` | Send a typed IPC message to JS (calls `_dispatch`) |
| `set_bounds(rect)` | `-> Result<(), wry::Error>` | Reposition/resize within the parent window |
| `set_visible(bool)` | `-> Result<(), wry::Error>` | Show or hide the WebView |
| `focus()` | `-> Result<(), wry::Error>` | Give OS focus to this WebView |
| `focus_parent()` | `-> Result<(), wry::Error>` | Return OS focus to the parent window |
| `open_devtools()` | `()` | Open browser devtools (if enabled) |
| `zoom(scale)` | `-> Result<(), wry::Error>` | Set zoom level |
| `set_title(title)` | `()` | Update the tracked title string |
| `inner()` | `-> &WebView` | Access the underlying wry WebView |

### IPC Dispatch Implementation

`send_ipc` generates a JavaScript snippet and evaluates it:

```rust
pub fn send_ipc(&self, kind: &str, payload: &serde_json::Value) -> Result<(), wry::Error> {
    let script = crate::ipc::js_dispatch_message(kind, payload);
    self.webview.evaluate_script(&script)
}
```

The generated JS:
```javascript
window.jarvis.ipc._dispatch("pty_output", {"data": "ls\nREADME.md"});
```

---

## WebViewRegistry Lifecycle

**Source:** `jarvis-rs/crates/jarvis-webview/src/manager/registry.rs`

The `WebViewRegistry` is the top-level container that manages all active WebView instances, mapping pane IDs to `WebViewHandle`s.

```rust
pub struct WebViewRegistry {
    manager: WebViewManager,
    handles: HashMap<u32, WebViewHandle>,
}
```

### Methods

| Method | Description |
|--------|-------------|
| `new(manager)` | Create a registry wrapping a `WebViewManager` |
| `create(pane_id, window, bounds, config)` | Create a WebView and register it for a pane |
| `get(pane_id)` | Get an immutable handle by pane ID |
| `get_mut(pane_id)` | Get a mutable handle by pane ID |
| `destroy(pane_id)` | Destroy a WebView, emit `Closed` event |
| `active_panes()` | List all pane IDs with active WebViews |
| `drain_events()` | Drain pending events from all WebViews |
| `destroy_all()` | Destroy all WebViews (graceful shutdown) |
| `count()` | Number of active WebViews |

### Event System

The `WebViewManager` maintains an `Arc<Mutex<Vec<WebViewEvent>>>` event queue. Events are pushed by the IPC handler, page load handler, title change handler, and navigation handler. The application event loop calls `drain_events()` to process them.

Event types:

```rust
pub enum WebViewEvent {
    PageLoad { pane_id: u32, state: PageLoadState, url: String },
    TitleChanged { pane_id: u32, title: String },
    IpcMessage { pane_id: u32, body: String },
    NavigationRequested { pane_id: u32, url: String },
    Closed { pane_id: u32 },
}
```

`PageLoadState` has two variants: `Started` and `Finished`.

### Lifecycle Flow

1. **Startup**: `WebViewManager::new()` -> `WebViewRegistry::new(manager)` -> `manager.set_content_provider(provider)`.
2. **Panel creation**: `registry.create(pane_id, window, bounds, config)` -> WebView appears in the window.
3. **Event loop**: `registry.drain_events()` -> dispatch IPC messages, handle page loads.
4. **Resize**: `handle.set_bounds(new_rect)` on window resize or tiling layout change.
5. **Navigation**: `handle.load_url(url)` for game launch, panel switch, etc.
6. **Destruction**: `registry.destroy(pane_id)` when a panel is closed.
7. **Shutdown**: `registry.destroy_all()` during graceful exit.

### Page Load Handling

When a page finishes loading in the focused pane, the handle receives OS focus to ensure the keyboard shortcut forwarder works. Additionally, for "Bros" games (kartbros, basketbros, etc.), an ad-blocker CSS/JS snippet is injected.

---

## Appendix: Data Flow Diagrams

### JS -> Rust Message Flow

```
JavaScript
    |
    v
window.jarvis.ipc.send(kind, payload)
    |
    v
window.ipc.postMessage(JSON.stringify({kind, payload}))
    |
    v
wry IPC handler (validates JSON)
    |
    v
WebViewEvent::IpcMessage { pane_id, body }
    |
    v
Event queue (Arc<Mutex<Vec<WebViewEvent>>>)
    |
    v
poll_webview_events() drains queue
    |
    v
handle_ipc_message(pane_id, body)
    |
    v
IpcMessage::from_json() -- parse kind + payload
    |
    v
is_ipc_kind_allowed() -- allowlist check
    |
    v
match msg.kind -- dispatch to handler
```

### Rust -> JS Message Flow

```
Rust handler
    |
    v
handle.send_ipc(kind, payload)
    |
    v
js_dispatch_message(kind, payload)
    -- generates: window.jarvis.ipc._dispatch("kind", {...})
    |
    v
webview.evaluate_script(js_string)
    |
    v
JavaScript executes in WebView context
    |
    v
_dispatch checks for _reqId (request-response)
    |
    v
If _reqId: resolves/rejects pending Promise
Else: calls registered on() handler
```

### Request-Response Flow

```
JS: jarvis.ipc.request('clipboard_paste', {})
    |
    v
Assigns _reqId=N, stores Promise callbacks
    |
    v
send('clipboard_paste', { _reqId: N })
    |
    v
[... Rust processes, reads clipboard ...]
    |
    v
handle.send_ipc('clipboard_paste_response',
    { _reqId: N, kind: "text", text: "hello" })
    |
    v
_dispatch sees _reqId=N, finds pending request
    |
    v
Resolves Promise with { kind: "text", text: "hello" }
```
