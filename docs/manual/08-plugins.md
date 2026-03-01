# Chapter 8: Plugin System

The Jarvis plugin system lets you build custom panels, tools, and web applications that run inside Jarvis with full access to the IPC bridge. Plugins appear in the command palette and load as webview panes -- the same mechanism that powers built-in panels like Settings, Chat, and Games.

This chapter covers everything you need to know to configure, create, and debug plugins.

---

## 8.1 Overview

The plugin system is organized into two tiers, each with different capabilities and complexity:

| Tier | What it is | Where it lives | What it can do |
|------|-----------|----------------|----------------|
| **Bookmark** | A named URL | `config.toml` | Opens any website in a Jarvis pane |
| **Local Plugin** | HTML/JS/CSS folder | `~/.config/jarvis/plugins/` | Full `jarvis://` protocol + IPC bridge |

**Bookmark plugins** are the simplest form: a name, a URL, and an optional category, all declared in your configuration file. They require no files on disk and load any website directly in a Jarvis pane.

**Local plugins** are full HTML/JS/CSS applications that live in a folder on your filesystem. They are served through the `jarvis://` custom protocol, have access to the IPC bridge (`window.jarvis.ipc`), and can communicate bidirectionally with the Rust backend.

Both tiers appear as entries in the command palette. Selecting a plugin navigates the focused pane to the plugin's content. Pressing Escape returns the pane to its previous state.

---

## 8.2 Bookmark Plugins

### 8.2.1 Configuration Format

Bookmark plugins are defined in `config.toml` using the `[[plugins.bookmarks]]` array-of-tables syntax. Each entry supports three fields:

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | string | Yes | `""` | Display name in the command palette |
| `url` | string | Yes | `""` | URL to open (any valid `https://` or `http://` URL) |
| `category` | string | No | `"Plugins"` | Palette category for grouping |

Bookmarks with an empty `name` or empty `url` are silently skipped during palette injection. This behavior is enforced in the `inject_plugin_items` function, which checks:

```rust
for bm in &self.config.plugins.bookmarks {
    if bm.name.is_empty() || bm.url.is_empty() {
        continue;
    }
    // ... create palette entry
}
```

### 8.2.2 Examples

A single bookmark:

```toml
[[plugins.bookmarks]]
name = "Spotify"
url = "https://open.spotify.com"
category = "Music"
```

Multiple bookmarks across categories:

```toml
[[plugins.bookmarks]]
name = "Spotify"
url = "https://open.spotify.com"
category = "Music"

[[plugins.bookmarks]]
name = "GitHub"
url = "https://github.com"
category = "Dev"

[[plugins.bookmarks]]
name = "Figma"
url = "https://figma.com"
category = "Design"

[[plugins.bookmarks]]
name = "Hacker News"
url = "https://news.ycombinator.com"
category = "News"
```

### 8.2.3 Categories

The `category` field controls how the bookmark appears in the command palette. When the palette is open and no search query is active, items are grouped under category headers. Common categories include:

- `"Music"` -- streaming and audio services
- `"Dev"` -- development tools and repositories
- `"Design"` -- design and prototyping tools
- `"Productivity"` -- task management and documentation
- `"News"` -- news aggregators and feeds

If `category` is omitted, the bookmark falls under the default `"Plugins"` category. You can use any string as a category name.

### 8.2.4 How Bookmarks Load

When a user selects a bookmark from the command palette, Jarvis dispatches `Action::OpenURL` with the bookmark's URL. The dispatch handler:

1. Normalizes the URL (auto-prepends `https://` if no scheme is present).
2. Saves the current URL of the focused pane (so Escape can return to it).
3. Navigates the focused pane's webview to the bookmark URL.

The navigation handler permits all `https://` and `http://` URLs, so bookmarks can point to any public website.

---

## 8.3 Local Plugins

### 8.3.1 Folder Structure

Local plugins live in the platform-specific plugins directory:

| OS | Plugins directory |
|----|-------------------|
| macOS | `~/Library/Application Support/jarvis/plugins/` or `~/.config/jarvis/plugins/` |
| Linux | `~/.config/jarvis/plugins/` |
| Windows | `%APPDATA%\jarvis\plugins\` |

The directory is resolved by the `plugins_dir()` function, which uses `dirs::config_dir()` and appends `jarvis/plugins`:

```rust
pub fn plugins_dir() -> Option<PathBuf> {
    let config_dir = dirs::config_dir()?;
    Some(config_dir.join("jarvis").join("plugins"))
}
```

Each plugin occupies its own subfolder. The folder name becomes the plugin's **ID** and is used in URLs:

```
jarvis://localhost/plugins/{folder-name}/{file}
```

A typical plugin folder looks like this:

```
~/.config/jarvis/plugins/
  my-plugin/
    plugin.toml          # Required: manifest file
    index.html           # Entry point (configurable)
    style.css            # Optional: stylesheets
    app.js               # Optional: scripts
    assets/              # Optional: images, fonts, etc.
      icon.png
      sounds/
        click.mp3
```

### 8.3.2 The Manifest: plugin.toml

Every local plugin requires a `plugin.toml` file in its root folder. The manifest is deserialized into an internal `Manifest` struct with these fields:

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | string | No | Folder name | Display name in the command palette |
| `category` | string | No | `"Plugins"` | Palette category for grouping |
| `entry` | string | No | `"index.html"` | Entry point HTML file |

All fields are optional. A completely empty `plugin.toml` is valid -- the plugin will use the folder name as its display name, `"Plugins"` as its category, and `index.html` as its entry point.

**Minimal valid manifest:**

```toml
name = "My Timer"
```

**Full manifest:**

```toml
name = "Project Dashboard"
category = "Productivity"
entry = "app.html"
```

When the `name` field is empty or omitted, the discovery logic falls back to the folder name:

```rust
let name = if m.name.is_empty() {
    id.clone()
} else {
    m.name
};
```

### 8.3.3 Discovery

Plugin discovery is performed by the `discover_local_plugins()` function. This function is called at startup and whenever `ReloadConfig` is dispatched. The discovery algorithm:

1. Reads all entries in the plugins directory using `std::fs::read_dir`.
2. Skips any entry that is not a directory.
3. For each directory, checks for the existence of `plugin.toml`.
4. Reads and parses the manifest file.
5. Constructs a `LocalPlugin` struct with the folder name as `id`, the parsed `name` (or folder name as fallback), `category`, and `entry`.

Directories without a `plugin.toml` are silently ignored. Manifests that fail to parse produce a warning log:

```
Failed to parse plugin manifest (plugin: "my-plugin", error: ...)
```

Manifests that cannot be read from disk also produce a warning:

```
Failed to read plugin manifest (plugin: "my-plugin", error: ...)
```

After discovery, each plugin directory is registered with the `ContentProvider`:

```rust
content_provider.add_plugin_dir("my-plugin", "/path/to/my-plugin");
```

This registration maps the plugin ID to its filesystem path, enabling the custom protocol handler to serve the plugin's files.

### 8.3.4 The LocalPlugin Data Type

The `LocalPlugin` struct, defined in `jarvis-config/src/schema/plugins.rs`, carries the resolved plugin metadata:

```rust
pub struct LocalPlugin {
    pub id: String,       // Folder name, used in URLs
    pub name: String,     // Display name in palette
    pub category: String, // Palette category grouping
    pub entry: String,    // Entry HTML file (default "index.html")
}
```

Local plugins are stored in `PluginsConfig.local`, which is marked `#[serde(skip)]` because local plugins are discovered from the filesystem at runtime, not deserialized from `config.toml`.

---

## 8.4 How Plugins Load

Understanding the complete loading flow helps when debugging or building more advanced plugins:

```
1. Jarvis starts (or ReloadConfig is dispatched)
       |
2. discover_local_plugins() scans ~/.config/jarvis/plugins/
   - Lists subdirectories
   - Reads plugin.toml from each
   - Builds LocalPlugin { id, name, category, entry }
       |
3. Each plugin dir is registered with the ContentProvider
   content_provider.add_plugin_dir("my-plugin", "/path/to/my-plugin")
       |
4. User opens command palette (Cmd+Shift+P / Ctrl+Shift+P)
       |
5. inject_plugin_items() creates palette entries:
   - Bookmarks  -> Action::OpenURL("https://...")
   - Local      -> Action::OpenURL("jarvis://localhost/plugins/{id}/{entry}")
       |
6. User selects a plugin from the palette
       |
7. dispatch(Action::OpenURL(url))
   - The focused pane's webview navigates to the URL
   - The previous URL is saved in game_active (so Escape can go back)
       |
8. WebView requests jarvis://localhost/plugins/my-plugin/index.html
       |
9. Custom protocol handler -> ContentProvider.resolve()
   - Looks up "my-plugin" in plugin_dirs
   - Reads the file from disk
   - Performs containment check (directory traversal protection)
   - Returns 200 with correct MIME type
       |
10. Plugin HTML loads with the IPC bridge already injected
    (window.jarvis.ipc is available immediately)
       |
11. User presses Escape -> navigates back to the previous page
```

The `OpenURL` dispatch handler in `dispatch.rs` implements the navigation and back-navigation tracking:

```rust
Action::OpenURL(ref url) => {
    let normalized = if !url.contains("://") {
        format!("https://{}", url)
    } else {
        url.clone()
    };
    let pane_id = self.tiling.focused_id();
    if let Some(ref mut registry) = self.webviews {
        if let Some(handle) = registry.get_mut(pane_id) {
            let original_url = handle.current_url().to_string();
            if let Err(e) = handle.load_url(&normalized) {
                tracing::warn!(error = %e, url = %normalized, "Failed to open URL");
            } else {
                self.game_active.insert(pane_id, original_url);
            }
        }
    }
}
```

The `game_active` map stores the previous URL for each pane, enabling Escape to restore the original content.

---

## 8.5 Plugin Development Guide

### 8.5.1 Step-by-Step: Creating a Plugin

**Step 1: Create the plugin folder.**

```bash
# Linux/macOS
mkdir -p ~/.config/jarvis/plugins/hello-world

# Windows (PowerShell)
mkdir "$env:APPDATA\jarvis\plugins\hello-world"
```

**Step 2: Create the manifest.**

Create `plugin.toml` in the plugin folder:

```toml
name = "Hello World"
category = "Tools"
```

**Step 3: Create the entry point.**

Create `index.html` in the plugin folder:

```html
<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <style>
    body {
      margin: 0;
      padding: 24px;
      background: transparent;
      color: var(--color-text, #cdd6f4);
      font-family: var(--font-ui, sans-serif);
      font-size: var(--font-ui-size, 13px);
    }
    h1 { color: var(--color-primary, #cba6f7); }
    button {
      background: var(--color-primary, #cba6f7);
      color: var(--color-background, #1e1e2e);
      border: none;
      padding: 8px 16px;
      border-radius: 6px;
      cursor: pointer;
      font-size: 14px;
    }
  </style>
</head>
<body>
  <h1>Hello from a Jarvis Plugin!</h1>
  <p>This is running inside the jarvis:// protocol with full IPC access.</p>
  <button onclick="ping()">Ping Jarvis</button>
  <p id="result"></p>

  <script>
    async function ping() {
      window.jarvis.ipc.send('ping', {});
      window.jarvis.ipc.on('pong', () => {
        document.getElementById('result').textContent = 'Pong received!';
      });
    }
  </script>
</body>
</html>
```

**Step 4: Load the plugin.**

Open the command palette and select "Reload Config". Your plugin appears under the "Tools" category. Select "Hello World" and it loads in the focused pane.

### 8.5.2 Development Workflow

The recommended development workflow is:

1. Edit your plugin files (HTML, JS, CSS) in your editor of choice.
2. Switch to Jarvis and press `Cmd+R` (macOS) or `Ctrl+R` (Windows/Linux) to refresh the webview in-place.
3. Repeat.

Since plugin files are read from disk on every request, changes take effect immediately without restarting Jarvis. No build step is required.

If you add, remove, or rename a plugin folder, or change `plugin.toml`, you need to trigger "Reload Config" from the command palette.

---

## 8.6 The IPC Bridge API

Every webview in Jarvis -- including plugins -- receives the IPC bridge automatically. It is injected as an initialization script (`IPC_INIT_SCRIPT`) that runs before any page scripts. The bridge is available at `window.jarvis.ipc`.

### 8.6.1 Core Methods

**`send(kind, payload)`** -- Fire-and-forget message to Rust.

```javascript
window.jarvis.ipc.send('clipboard_copy', { text: 'Hello from my plugin!' });
```

Internally, `send` serializes `{ kind, payload }` as JSON and posts it via `window.ipc.postMessage`.

**`on(kind, callback)`** -- Register a handler for messages from Rust.

```javascript
window.jarvis.ipc.on('pong', (payload) => {
  console.log('Got pong:', payload);
});
```

You can register one handler per message kind. Calling `on` again for the same kind replaces the previous handler.

**`request(kind, payload)`** -- Send a request and await a response. Returns a `Promise`.

```javascript
try {
  const result = await window.jarvis.ipc.request('clipboard_paste', {});
  if (result.kind === 'text') {
    console.log('Clipboard text:', result.text);
  } else if (result.kind === 'image') {
    console.log('Clipboard image:', result.data_url);
  }
} catch (err) {
  console.error('Request failed:', err);
}
```

Under the hood, `request` assigns a unique `_reqId` to the payload, sends the message, and returns a Promise that resolves when Rust sends back a response containing the same `_reqId`. The request has a **10-second timeout** -- if no response arrives, the Promise rejects with an `Error('IPC request timeout')`.

**`postMessage(msg)`** -- Low-level: sends a raw object. Prefer `send()` which wraps your payload with the `{ kind, payload }` structure that the Rust side expects.

### 8.6.2 Checking Bridge Availability

The bridge is available as soon as your script runs. There is no `DOMContentLoaded` race because the initialization script runs before your page's scripts. However, if you want to be defensive:

```javascript
if (window.jarvis && window.jarvis.ipc) {
  // IPC bridge is ready
  window.jarvis.ipc.send('ping', {});
}
```

### 8.6.3 Request-Response Internals

The request-response pattern works as follows:

1. JavaScript calls `request(kind, payload)`.
2. The bridge assigns `payload._reqId = N` (incrementing integer).
3. The message is sent to Rust via `send`.
4. Rust processes the request and sends a response IPC message back to the webview, including `_reqId: N` in the payload.
5. The bridge's augmented `_dispatch` function checks incoming messages for `_reqId`. If found, it resolves (or rejects, if `payload.error` is present) the matching pending Promise.
6. If no `_reqId` match is found, the message is passed to the regular `on` handler.

---

## 8.7 Available IPC Messages

### 8.7.1 Messages You Can Send (JS to Rust)

| Kind | Payload | Behavior |
|------|---------|----------|
| `ping` | `{}` | Health check. Rust replies with `pong`. |
| `clipboard_copy` | `{ text: "..." }` | Copy text to the system clipboard. |
| `clipboard_paste` | `{}` | **Request-response.** Returns clipboard contents as `{ kind: "text", text }` or `{ kind: "image", data_url }`. |
| `open_url` | `{ url: "https://..." }` | Navigate the current pane to a URL. URLs without a scheme get `https://` prepended. |
| `launch_game` | `{ game: "tetris" }` | Launch a built-in game in the current pane. |
| `open_settings` | `{}` | Open the Settings panel. |
| `open_panel` | `{ kind: "terminal" }` | Open a new panel of the given kind. |
| `panel_close` | `{}` | Close the current panel (will not close the last one). |
| `read_file` | `{ path: "..." }` | **Request-response.** Read an image file from disk. Returns `{ data_url }` (base64 data URL) or `{ error }`. Maximum file size: 5 MB. Supports `~/` expansion. Only recognized image formats (PNG, JPEG, GIF, WebP, BMP) are accepted. |
| `pty_input` | `{ data: "..." }` | Send input to the terminal PTY (if this pane has one). |
| `keybind` | `{ key, ctrl, alt, shift, meta }` | Simulate a keybind press. |
| `window_drag` | `{}` | Start dragging the window (for custom titlebars). |
| `debug_event` | `{ type, ...data }` | Log structured data to the Rust `tracing::info!` output. |

### 8.7.2 Messages You Can Receive (Rust to JS)

| Kind | Payload | When |
|------|---------|------|
| `pong` | `"pong"` | After you send `ping`. |
| `palette_show` | `{ items, query, selectedIndex, mode, placeholder }` | Command palette opens (handled automatically by the injected script). |
| `palette_update` | `{ items, query, selectedIndex, mode, placeholder }` | Command palette state changes (handled automatically). |
| `palette_hide` | `{}` | Command palette closes (handled automatically). |

The palette messages are handled by the injected command palette overlay system. You do not need to handle them in your plugin unless you want to customize the palette behavior.

For request-response messages (`clipboard_paste`, `read_file`), the response arrives through the Promise returned by `ipc.request()`, not through `ipc.on()`.

---

## 8.8 Keyboard and Input Handling

### 8.8.1 How Keyboard Events Work

Keyboard events in plugins follow a layered interception model:

1. **Overlay mode.** When the command palette or assistant is open, ALL keyboard input is captured by the overlay. Your plugin receives no key events during this time. The IPC bridge forwards every keystroke to Rust for overlay handling.

2. **Command keys (Cmd/Ctrl+key).** Most Cmd+key combinations are intercepted by the IPC bridge's capture-phase `keydown` listener and forwarded to Rust as `keybind` messages. This is how Cmd+T (new pane), Cmd+W (close pane), and other shortcuts work even when a plugin is focused.

3. **Escape key.** Always forwarded to Rust. If the pane is showing a plugin or other non-terminal content (tracked in `game_active`), Escape navigates back to the previous page.

4. **Normal mode.** All other keyboard input goes to your plugin's webview normally. Standard HTML elements like `<input>`, `<textarea>`, and custom key handlers work as expected.

### 8.8.2 Keys That Pass Through

These Cmd+key combinations are NOT intercepted and reach your plugin normally:

- `Cmd+R` / `Ctrl+R` -- useful for refreshing the plugin during development
- `Cmd+L` / `Ctrl+L`
- `Cmd+Q` / `Ctrl+Q`
- `Cmd+A` / `Ctrl+A` -- select all
- `Cmd+X` / `Ctrl+X` -- cut
- `Cmd+Z` / `Ctrl+Z` -- undo

### 8.8.3 Handling Escape in Your Plugin

If your plugin has modal dialogs or internal states that should close on Escape, handle it before the IPC bridge does:

```javascript
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') {
    if (myModalIsOpen) {
      closeMyModal();
      e.stopPropagation();  // Attempt to prevent bridge from forwarding
      return;
    }
    // Otherwise, Escape navigates back to the previous page
  }
});
```

Note that `e.stopPropagation()` may not prevent the bridge from seeing Escape in all cases, since the bridge uses a capture-phase listener. If you need full Escape control, structure your plugin so that Escape-to-exit is acceptable behavior.

---

## 8.9 Styling and Theming

Plugins run in a transparent webview with the Jarvis theme's CSS variables injected into every page. Using these variables ensures your plugin matches the user's chosen theme automatically.

### 8.9.1 Available CSS Variables

**Colors:**

| Variable | Default Value | Purpose |
|----------|--------------|---------|
| `--color-primary` | `#cba6f7` | Primary accent color |
| `--color-secondary` | `#f5c2e7` | Secondary accent color |
| `--color-background` | `#1e1e2e` | Base background color |
| `--color-panel-bg` | `rgba(30,30,46,0.88)` | Panel background with transparency |
| `--color-text` | `#cdd6f4` | Primary text color |
| `--color-text-muted` | `#6c7086` | Muted/secondary text color |
| `--color-border` | `#181825` | Border color |
| `--color-border-focused` | `rgba(203,166,247,0.15)` | Focused border color |
| `--color-success` | `#a6e3a1` | Success state color |
| `--color-warning` | `#f9e2af` | Warning state color |
| `--color-error` | `#f38ba8` | Error state color |

**Fonts:**

| Variable | Default Value | Purpose |
|----------|--------------|---------|
| `--font-family` | `Menlo` | Monospace font family |
| `--font-size` | `13px` | Monospace font size |
| `--font-ui` | `-apple-system, BlinkMacSystemFont, 'Inter', 'Segoe UI', sans-serif` | UI font family |
| `--font-ui-size` | `13px` | UI font size |
| `--font-title-size` | `14px` | Title font size |
| `--line-height` | `1.6` | Line height |

**Layout:**

| Variable | Default Value | Purpose |
|----------|--------------|---------|
| `--border-radius` | `8px` | Border radius |

These variables update automatically when the user changes themes or reloads config. The theme injection is handled by `inject_theme_into_all_webviews()`, which is called during `ReloadConfig`.

### 8.9.2 Transparent Backgrounds

The webview background is transparent by default. Set your `body` background to `transparent` or use the theme variables:

```css
body {
  background: transparent;           /* Fully transparent */
  /* or */
  background: var(--color-panel-bg); /* Match the panel glass effect */
}
```

### 8.9.3 Starter Template

This CSS template provides a solid starting point for any plugin:

```css
* { box-sizing: border-box; margin: 0; padding: 0; }
body {
  background: var(--color-panel-bg, rgba(30,30,46,0.88));
  color: var(--color-text, #cdd6f4);
  font-family: var(--font-ui, sans-serif);
  font-size: var(--font-ui-size, 13px);
  line-height: var(--line-height, 1.6);
  padding: 16px;
  overflow-y: auto;
}
h1, h2, h3 {
  color: var(--color-primary, #cba6f7);
  margin-bottom: 8px;
}
button {
  background: var(--color-primary, #cba6f7);
  color: var(--color-background, #1e1e2e);
  border: none;
  padding: 6px 14px;
  border-radius: calc(var(--border-radius, 8px) / 2);
  cursor: pointer;
  font-family: inherit;
  font-size: inherit;
}
button:hover { opacity: 0.85; }
input, textarea {
  background: var(--color-background, #1e1e2e);
  color: var(--color-text, #cdd6f4);
  border: 1px solid var(--color-border, #181825);
  padding: 6px 10px;
  border-radius: calc(var(--border-radius, 8px) / 2);
  font-family: inherit;
  font-size: inherit;
}
input:focus, textarea:focus {
  outline: none;
  border-color: var(--color-primary, #cba6f7);
}
```

Always provide fallback values in your `var()` declarations (e.g., `var(--color-primary, #cba6f7)`) so the plugin renders correctly even if theme injection has not yet occurred.

### 8.9.4 Detecting Theme Changes Programmatically

If your plugin uses a canvas or other non-CSS rendering, you can watch for theme changes:

```javascript
const observer = new MutationObserver(() => {
  const primary = getComputedStyle(document.documentElement)
    .getPropertyValue('--color-primary').trim();
  updateMyCanvas(primary);
});
observer.observe(document.documentElement, {
  attributes: true,
  attributeFilter: ['style']
});
```

---

## 8.10 Asset Loading and MIME Types

### 8.10.1 Referencing Assets

Your plugin can include any static assets. Reference them with relative paths in your HTML:

```html
<!-- These resolve via jarvis://localhost/plugins/my-plugin/... -->
<link rel="stylesheet" href="style.css">
<script src="app.js"></script>
<img src="assets/logo.png">
<audio src="assets/notification.mp3"></audio>
```

Relative paths work because the browser resolves them against the current URL (`jarvis://localhost/plugins/my-plugin/index.html`).

### 8.10.2 Supported MIME Types

The `ContentProvider` determines MIME types by file extension using the `mime_from_extension` function. The complete mapping:

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
| `.webp` | `image/webp` |
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
| `.txt` | `text/plain` |
| `.xml` | `application/xml` |
| (other) | `application/octet-stream` |

Files with unrecognized extensions are served as `application/octet-stream`, which may not render correctly in the webview.

### 8.10.3 External Resources

Plugins can load resources from the web. The navigation handler permits all `https://` and `http://` URLs:

```html
<!-- CDN libraries work fine -->
<script src="https://cdn.jsdelivr.net/npm/chart.js"></script>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Inter">
```

Standard browser CORS rules apply to `fetch()` and `XMLHttpRequest` calls.

### 8.10.4 WebAssembly

`.wasm` files are served with the correct `application/wasm` MIME type, enabling you to load WebAssembly modules:

```javascript
const response = await fetch('jarvis://localhost/plugins/my-plugin/engine.wasm');
const { instance } = await WebAssembly.instantiateStreaming(response);
```

---

## 8.11 Security Model

### 8.11.1 Directory Traversal Protection

Plugin assets are sandboxed to their own folder. The `ContentProvider` enforces this by canonicalizing both the plugin's base path and the requested file path, then verifying the file path starts with the base path:

```rust
fn resolve_plugin_asset(&self, plugin_id: &str, asset_path: &str)
    -> Option<(Cow<'_, str>, Cow<'_, [u8]>)>
{
    let plugin_base = { /* look up plugin directory */ };
    let file_path = plugin_base.join(asset_path);

    // Containment check: canonicalize both to prevent traversal
    let canonical_base = std::fs::canonicalize(&plugin_base).ok()?;
    let canonical_file = std::fs::canonicalize(&file_path).ok()?;
    if !canonical_file.starts_with(&canonical_base) {
        return None;  // Blocked: path escapes plugin directory
    }

    // ... read and serve file
}
```

This prevents all forms of directory traversal:

```
jarvis://localhost/plugins/my-plugin/../../etc/passwd             -> 404
jarvis://localhost/plugins/my-plugin/../other-plugin/secret.json  -> 404
```

Symlinks that escape the plugin directory are also blocked because `std::fs::canonicalize` resolves symlinks to their real paths before the containment check.

### 8.11.2 Navigation Allowlist

The webview navigation handler enforces a strict allowlist for non-HTTP(S) schemes. Only these URL patterns are permitted:

| Prefix | Purpose |
|--------|---------|
| `jarvis://` | Custom protocol for bundled and plugin assets |
| `http://jarvis.localhost` | WebView2 (Windows) rewrite of `jarvis://` |
| `about:blank` | Default empty page |
| `https://` | All HTTPS URLs (permitted by the handler directly) |
| `http://` | All HTTP URLs (permitted by the handler directly) |

The following are explicitly blocked:

- `file://` -- direct filesystem access
- `javascript:` -- script execution via URL
- `data:` -- data URLs for navigation
- `ftp://` -- FTP protocol
- Any other unrecognized scheme

### 8.11.3 IPC Allowlist

All IPC messages from JavaScript are validated against a strict allowlist before processing. The complete list of accepted `kind` values:

```
pty_input          pty_resize        pty_restart       terminal_ready
panel_focus        presence_request_users              presence_poke
settings_init      settings_set_theme                  settings_update
settings_reset_section               settings_get_config
assistant_input    assistant_ready
open_panel         panel_close       panel_toggle      open_settings
status_bar_init    launch_game
ping               boot_complete     crypto            window_drag
keybind            read_file
clipboard_copy     clipboard_paste   open_url
palette_click      palette_hover     palette_dismiss
debug_event
```

Any message with a `kind` not in this list is **silently dropped** with a warning log:

```
IPC message rejected: unknown kind (pane_id: 1, kind: "exec")
```

The allowlist check is case-sensitive. `"PTY_INPUT"` is rejected. Injection attempts like `"ping; rm -rf /"` or `"<script>alert(1)</script>"` are also rejected.

Additionally, the IPC handler validates that the raw message body is valid JSON before forwarding it to the dispatch layer.

### 8.11.4 What Plugins CAN Do

- Read and write the system clipboard (via `clipboard_copy` and `clipboard_paste` IPC messages)
- Navigate the current pane to any URL
- Make HTTP requests to any origin (standard browser CORS rules apply)
- Load any `https://` resource
- Read image files from disk (via the `read_file` IPC message, limited to 5 MB, image formats only)
- Send input to the terminal PTY
- Open and close panels
- Use localStorage for persistence (scoped to `jarvis://localhost` origin)
- Start window drag operations
- Log debug events to the Rust log

### 8.11.5 What Plugins CANNOT Do

- Access the `file://` protocol directly
- Execute `javascript:` URLs
- Use `data:` URLs for navigation
- Access other plugins' files via path traversal
- Bypass the IPC allowlist (unknown message kinds are rejected)
- Read arbitrary files from disk (only recognized image formats via `read_file`)
- Execute arbitrary system commands

---

## 8.12 Hot Reload

### 8.12.1 Bookmark Plugins

Edit your `config.toml`, then trigger "Reload Config" from the command palette. Bookmark changes take effect immediately the next time the palette is opened.

### 8.12.2 Local Plugin Files (HTML/JS/CSS)

Since files are read from disk on every request by the `ContentProvider`, changes to your plugin's source files take effect the next time the plugin is loaded. You can:

1. Press Escape to go back to the terminal, then re-open the plugin from the palette.
2. Press `Cmd+R` / `Ctrl+R` to refresh the webview in-place (this key is not intercepted by Jarvis).

Option 2 is the fastest workflow during development.

### 8.12.3 Adding or Removing Plugins

Create or delete the plugin folder under `~/.config/jarvis/plugins/`, then trigger "Reload Config" from the command palette. The discovery runs again, the `ContentProvider`'s plugin directory map is cleared and rebuilt, and the palette updates accordingly.

The reload logic in `dispatch.rs`:

```rust
Action::ReloadConfig => {
    // ...
    if let Some(ref dirs_handle) = self.plugin_dirs {
        if let Ok(mut dirs) = dirs_handle.write() {
            dirs.clear();
            if let Some(plugins_base) = plugins_dir() {
                for lp in &c.plugins.local {
                    dirs.insert(lp.id.clone(), plugins_base.join(&lp.id));
                }
            }
        }
    }
    // ...
}
```

### 8.12.4 Editing plugin.toml

Changes to the manifest (name, category, entry) require a "Reload Config" to take effect in the palette, since the manifest is only read during discovery.

---

## 8.13 Debugging Plugins

### 8.13.1 DevTools

In debug builds (`cargo build` without `--release`), DevTools are enabled for all webviews. The `WebViewConfig` defaults to `devtools: cfg!(debug_assertions)`. Right-click inside your plugin and select "Inspect Element" to open the developer tools.

In release builds, DevTools are disabled by default. Enable them by setting:

```toml
[advanced.developer]
inspector_enabled = true
```

### 8.13.2 Console Logging

Standard `console.log`, `console.warn`, and `console.error` work in DevTools:

```javascript
console.log('Plugin loaded, IPC available:', !!window.jarvis?.ipc);
```

### 8.13.3 Debug Events

Send structured debug events to the Rust log:

```javascript
window.jarvis.ipc.send('debug_event', {
  type: 'my_plugin_state',
  data: { count: 42, status: 'running' }
});
```

These appear in the Jarvis log output as `tracing::info!` entries:

```
[JS] webview event (pane_id: 1, event: {"type":"my_plugin_state","data":{"count":42,"status":"running"}})
```

### 8.13.4 Ping/Pong Test

Verify the IPC bridge is working:

```javascript
window.jarvis.ipc.on('pong', () => console.log('IPC bridge works!'));
window.jarvis.ipc.send('ping', {});
```

### 8.13.5 Built-in Diagnostic Logging

The IPC bridge automatically sends diagnostic events for certain DOM events. During development, you can observe these in the Rust log:

- `mousedown` events (with coordinates and target element)
- `keydown` events (with key, code, and modifier state)
- `focus` and `blur` events

These are sent as `debug_event` IPC messages by the initialization script.

---

## 8.14 Vibe Coding a Plugin with AI

The plugin system is designed to be AI-friendly: single-file HTML plugins with an injected bridge API and CSS variables. Here is how to build plugins with Claude, ChatGPT, Cursor, or any AI coding assistant.

### 8.14.1 The Prompt Template

Give your AI this context and ask it to build what you need:

```
I'm building a plugin for Jarvis, a terminal/desktop app with a webview plugin system.

Plugin structure:
- Folder at ~/.config/jarvis/plugins/my-plugin/
- plugin.toml with: name, category, entry (defaults to index.html)
- HTML/JS/CSS files served via jarvis:// protocol

The IPC bridge (available as window.jarvis.ipc):
- send(kind, payload) -- fire-and-forget message to Rust
- on(kind, callback) -- listen for messages from Rust
- request(kind, payload) -- returns Promise for request-response

Useful IPC messages:
- clipboard_copy: { text } -- copy to clipboard
- clipboard_paste: {} -- request clipboard contents
- open_url: { url } -- navigate to URL
- ping: {} -- health check (replies with 'pong')
- read_file: { path } -- read an image file from disk

CSS variables available from the Jarvis theme:
--color-primary, --color-background, --color-panel-bg, --color-text,
--color-text-muted, --color-border, --color-success, --color-warning,
--color-error, --font-family, --font-ui, --font-size, --border-radius

The webview background is transparent. Use var(--color-panel-bg) for a glass effect.

Build me: [DESCRIBE WHAT YOU WANT]

Requirements:
- Single index.html file (inline CSS and JS is fine)
- Use the CSS variables so it matches the Jarvis theme
- Use window.jarvis.ipc for any system interaction
```

### 8.14.2 Example AI Prompts

**Sticky notes plugin:**

```
Build me a sticky notes plugin. Notes should be stored in localStorage.
I want to create, edit, and delete notes. Each note should have a
title and body. Use a grid layout. Match the Jarvis dark theme.
```

**JSON formatter:**

```
Build me a JSON formatter/viewer plugin. Paste or type JSON on the left,
see formatted output on the right. Add syntax highlighting. Include a
"Copy" button that uses the clipboard IPC. Handle invalid JSON gracefully.
```

**System dashboard:**

```
Build me a system dashboard plugin that shows the current time,
a stopwatch, and a countdown timer. Add keyboard shortcuts
(S for stopwatch start/stop, R for reset). Use large monospace numbers.
```

### 8.14.3 Tips for AI-Assisted Development

1. **Start with one file.** Tell the AI to put everything in `index.html` with inline `<style>` and `<script>`. Split into separate files later if needed.

2. **Share the CSS variables list.** AI tools produce much better-looking results when they know the exact variable names and their defaults.

3. **Mention `window.jarvis.ipc` explicitly.** This tells the AI how to interact with the system -- otherwise it might try to use `fetch()` to a nonexistent API server.

4. **Iterate fast.** Save the file, `Cmd+R` to refresh, repeat. No build step needed.

5. **Use localStorage for persistence.** Plugins do not have a database, but `localStorage` persists across loads. The storage is scoped to the `jarvis://localhost` origin, so all plugins share the same localStorage namespace. Prefix your keys with your plugin ID to avoid collisions:

```javascript
// Good: namespaced key
localStorage.setItem('my-timer:sessions', JSON.stringify(sessions));

// Bad: might collide with other plugins
localStorage.setItem('sessions', JSON.stringify(sessions));
```

---

## 8.15 Complete Example: Pomodoro Timer

This example demonstrates a full plugin with timer logic, multiple states, and localStorage persistence.

### plugin.toml

```toml
name = "Pomodoro Timer"
category = "Productivity"
```

### index.html

```html
<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <style>
    * { box-sizing: border-box; margin: 0; padding: 0; }
    body {
      background: var(--color-panel-bg, rgba(30,30,46,0.88));
      color: var(--color-text, #cdd6f4);
      font-family: var(--font-ui, sans-serif);
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
      height: 100vh;
      gap: 24px;
    }
    .timer {
      font-family: var(--font-family, 'Menlo');
      font-size: 72px;
      font-weight: bold;
      color: var(--color-primary, #cba6f7);
    }
    .timer.break { color: var(--color-success, #a6e3a1); }
    .timer.warning { color: var(--color-warning, #f9e2af); }
    .label {
      font-size: 16px;
      color: var(--color-text-muted, #6c7086);
      text-transform: uppercase;
      letter-spacing: 2px;
    }
    .controls { display: flex; gap: 12px; }
    button {
      background: var(--color-primary, #cba6f7);
      color: var(--color-background, #1e1e2e);
      border: none;
      padding: 10px 24px;
      border-radius: calc(var(--border-radius, 8px) / 2);
      cursor: pointer;
      font-size: 14px;
      font-family: inherit;
      font-weight: 600;
    }
    button:hover { opacity: 0.85; }
    button.secondary {
      background: var(--color-border, #181825);
      color: var(--color-text, #cdd6f4);
    }
    .sessions {
      font-size: 13px;
      color: var(--color-text-muted, #6c7086);
    }
  </style>
</head>
<body>
  <div class="label" id="label">Focus Time</div>
  <div class="timer" id="timer">25:00</div>
  <div class="controls">
    <button id="startBtn" onclick="toggleTimer()">Start</button>
    <button class="secondary" onclick="resetTimer()">Reset</button>
    <button class="secondary" onclick="skipPhase()">Skip</button>
  </div>
  <div class="sessions" id="sessions">Sessions: 0</div>

  <script>
    const WORK_MINUTES = 25;
    const BREAK_MINUTES = 5;
    const LONG_BREAK_MINUTES = 15;

    let seconds = WORK_MINUTES * 60;
    let running = false;
    let interval = null;
    let isBreak = false;
    let sessionCount = parseInt(localStorage.getItem('pomodoro:sessions') || '0');

    const timerEl = document.getElementById('timer');
    const labelEl = document.getElementById('label');
    const sessionsEl = document.getElementById('sessions');
    const startBtn = document.getElementById('startBtn');

    sessionsEl.textContent = `Sessions: ${sessionCount}`;

    function formatTime(s) {
      const m = Math.floor(s / 60);
      const sec = s % 60;
      return `${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}`;
    }

    function updateDisplay() {
      timerEl.textContent = formatTime(seconds);
      timerEl.className = 'timer' +
        (isBreak ? ' break' : '') +
        (!isBreak && seconds <= 60 ? ' warning' : '');
    }

    function toggleTimer() {
      if (running) {
        clearInterval(interval);
        running = false;
        startBtn.textContent = 'Resume';
      } else {
        running = true;
        startBtn.textContent = 'Pause';
        interval = setInterval(() => {
          seconds--;
          if (seconds <= 0) {
            clearInterval(interval);
            running = false;
            if (!isBreak) {
              sessionCount++;
              localStorage.setItem('pomodoro:sessions', String(sessionCount));
              sessionsEl.textContent = `Sessions: ${sessionCount}`;
            }
            switchPhase();
          }
          updateDisplay();
        }, 1000);
      }
    }

    function switchPhase() {
      isBreak = !isBreak;
      if (isBreak) {
        const isLong = sessionCount % 4 === 0;
        seconds = (isLong ? LONG_BREAK_MINUTES : BREAK_MINUTES) * 60;
        labelEl.textContent = isLong ? 'Long Break' : 'Short Break';
      } else {
        seconds = WORK_MINUTES * 60;
        labelEl.textContent = 'Focus Time';
      }
      startBtn.textContent = 'Start';
      updateDisplay();
    }

    function resetTimer() {
      clearInterval(interval);
      running = false;
      isBreak = false;
      seconds = WORK_MINUTES * 60;
      labelEl.textContent = 'Focus Time';
      startBtn.textContent = 'Start';
      updateDisplay();
    }

    function skipPhase() {
      clearInterval(interval);
      running = false;
      seconds = 0;
      if (!isBreak) {
        sessionCount++;
        localStorage.setItem('pomodoro:sessions', String(sessionCount));
        sessionsEl.textContent = `Sessions: ${sessionCount}`;
      }
      switchPhase();
    }

    updateDisplay();
  </script>
</body>
</html>
```

---

## 8.16 Complete Example: Markdown Previewer

This example demonstrates a split-pane editor using a CDN library (marked.js) and IPC clipboard integration.

### plugin.toml

```toml
name = "Markdown Preview"
category = "Tools"
```

### index.html

```html
<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <script src="https://cdn.jsdelivr.net/npm/marked/marked.min.js"></script>
  <style>
    * { box-sizing: border-box; margin: 0; padding: 0; }
    body {
      background: var(--color-panel-bg, rgba(30,30,46,0.88));
      color: var(--color-text, #cdd6f4);
      font-family: var(--font-ui, sans-serif);
      font-size: var(--font-ui-size, 13px);
      height: 100vh;
      display: flex;
      flex-direction: column;
    }
    .toolbar {
      display: flex;
      gap: 8px;
      padding: 8px 12px;
      border-bottom: 1px solid var(--color-border, #181825);
      align-items: center;
    }
    .toolbar span {
      color: var(--color-text-muted, #6c7086);
      font-size: 12px;
    }
    button {
      background: var(--color-primary, #cba6f7);
      color: var(--color-background, #1e1e2e);
      border: none;
      padding: 4px 12px;
      border-radius: 4px;
      cursor: pointer;
      font-size: 12px;
      font-family: inherit;
    }
    button:hover { opacity: 0.85; }
    .panes {
      display: flex;
      flex: 1;
      overflow: hidden;
    }
    textarea {
      flex: 1;
      background: var(--color-background, #1e1e2e);
      color: var(--color-text, #cdd6f4);
      border: none;
      border-right: 1px solid var(--color-border, #181825);
      padding: 16px;
      font-family: var(--font-family, 'Menlo');
      font-size: var(--font-size, 13px);
      line-height: var(--line-height, 1.6);
      resize: none;
      outline: none;
    }
    .preview {
      flex: 1;
      padding: 16px;
      overflow-y: auto;
      line-height: 1.7;
    }
    .preview h1, .preview h2, .preview h3 {
      color: var(--color-primary, #cba6f7);
      margin: 16px 0 8px;
    }
    .preview h1 { font-size: 24px; }
    .preview h2 { font-size: 20px; }
    .preview h3 { font-size: 16px; }
    .preview p { margin: 8px 0; }
    .preview code {
      background: var(--color-background, #1e1e2e);
      padding: 2px 6px;
      border-radius: 3px;
      font-family: var(--font-family, 'Menlo');
      font-size: 0.9em;
    }
    .preview pre {
      background: var(--color-background, #1e1e2e);
      padding: 12px;
      border-radius: 6px;
      overflow-x: auto;
      margin: 12px 0;
    }
    .preview pre code { padding: 0; }
    .preview a { color: var(--color-primary, #cba6f7); }
    .preview blockquote {
      border-left: 3px solid var(--color-primary, #cba6f7);
      padding-left: 12px;
      color: var(--color-text-muted, #6c7086);
      margin: 8px 0;
    }
    .preview ul, .preview ol { padding-left: 24px; margin: 8px 0; }
    .preview li { margin: 4px 0; }
  </style>
</head>
<body>
  <div class="toolbar">
    <span>Markdown Preview</span>
    <button onclick="copyHtml()">Copy HTML</button>
    <button onclick="pasteFromClipboard()">Paste</button>
  </div>
  <div class="panes">
    <textarea id="editor" placeholder="Type or paste Markdown here..."></textarea>
    <div class="preview" id="preview"></div>
  </div>

  <script>
    const editor = document.getElementById('editor');
    const preview = document.getElementById('preview');

    // Load saved content
    editor.value = localStorage.getItem('markdown:content')
      || '# Hello\n\nStart typing **Markdown** here...';

    function render() {
      preview.innerHTML = marked.parse(editor.value);
      localStorage.setItem('markdown:content', editor.value);
    }

    editor.addEventListener('input', render);
    render();

    function copyHtml() {
      const html = marked.parse(editor.value);
      window.jarvis.ipc.send('clipboard_copy', { text: html });
    }

    async function pasteFromClipboard() {
      try {
        const result = await window.jarvis.ipc.request('clipboard_paste', {});
        if (result && result.text) {
          const start = editor.selectionStart;
          const end = editor.selectionEnd;
          editor.value = editor.value.slice(0, start)
            + result.text + editor.value.slice(end);
          editor.selectionStart = editor.selectionEnd = start + result.text.length;
          render();
        }
      } catch (e) {
        console.error('Paste failed:', e);
      }
    }
  </script>
</body>
</html>
```

---

## 8.17 Cookbook: Common Patterns

### 8.17.1 Persistent Data Storage

```javascript
// Save
localStorage.setItem('my-plugin:data', JSON.stringify(myData));

// Load
const myData = JSON.parse(localStorage.getItem('my-plugin:data') || '{}');
```

Always namespace your keys with your plugin ID. All plugins share the same `jarvis://localhost` localStorage origin.

### 8.17.2 Clipboard Operations

```javascript
// Copy text to clipboard
function copyToClipboard(text) {
  window.jarvis.ipc.send('clipboard_copy', { text });
}

// Read from clipboard (async)
async function readClipboard() {
  const result = await window.jarvis.ipc.request('clipboard_paste', {});
  return result?.text || '';
}
```

### 8.17.3 URL Navigation

```javascript
function openUrl(url) {
  window.jarvis.ipc.send('open_url', { url });
}
```

### 8.17.4 Reading Image Files

```javascript
async function readImageFile(path) {
  const result = await window.jarvis.ipc.request('read_file', { path });
  // result.data_url contains a base64 data URL (e.g., "data:image/png;base64,...")
  return result;
}
```

Note: `read_file` currently only supports image files (PNG, JPEG, GIF, WebP, BMP) with a maximum size of 5 MB.

### 8.17.5 Opening Panels

```javascript
// Open a new terminal pane
window.jarvis.ipc.send('open_panel', { kind: 'terminal' });

// Close the current panel
window.jarvis.ipc.send('panel_close', {});
```

### 8.17.6 Full-Screen Canvas Plugin

```html
<style>
  body { margin: 0; overflow: hidden; }
  canvas { display: block; }
</style>
<canvas id="canvas"></canvas>
<script>
  const canvas = document.getElementById('canvas');
  const ctx = canvas.getContext('2d');

  function resize() {
    canvas.width = window.innerWidth;
    canvas.height = window.innerHeight;
  }
  window.addEventListener('resize', resize);
  resize();

  function draw() {
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    // Your rendering logic here
    requestAnimationFrame(draw);
  }
  draw();
</script>
```

### 8.17.7 Inter-Plugin Communication

Plugins share the same `localStorage` namespace. You can use this as a simple message bus:

```javascript
// Plugin A: send a message
localStorage.setItem('plugin-bus:message', JSON.stringify({
  from: 'plugin-a',
  type: 'data-updated',
  timestamp: Date.now()
}));

// Plugin B: listen for messages
window.addEventListener('storage', (e) => {
  if (e.key === 'plugin-bus:message') {
    const msg = JSON.parse(e.newValue);
    console.log('Got message from', msg.from, msg.type);
  }
});
```

Note: `storage` events only fire in other windows/tabs, not the one that made the change. Since plugins in different panes run in different webviews, this works for cross-pane communication.

---

## 8.18 Troubleshooting

### Plugin does not appear in the palette

1. **Check the folder location.** Verify the plugin is in the correct platform-specific directory (see Section 8.3.1).
2. **Check that `plugin.toml` exists** in the plugin's folder root. Directories without a manifest are silently skipped.
3. **Check for TOML syntax errors.** Make sure your manifest parses correctly. Look for `"Failed to parse plugin manifest"` warnings in the Jarvis log.
4. **Reload config.** Open the palette and select "Reload Config". Discovery only runs at startup and on config reload.
5. **Check for read errors.** Look for `"Failed to read plugin manifest"` warnings in the log (file permission issues, etc.).

### Plugin loads but shows a blank page

1. **Check the entry point.** Does `index.html` (or your custom entry from `plugin.toml`) exist in the plugin folder?
2. **Open DevTools** (right-click then "Inspect", or enable via `advanced.developer.inspector_enabled`) and check the console for errors.
3. **Check the URL.** The plugin loads at `jarvis://localhost/plugins/{folder-name}/{entry}`. Make sure the folder name and entry file match.
4. **On Windows, check the WebView2 rewrite.** Windows WebView2 rewrites `jarvis://localhost/...` to `http://jarvis.localhost/...`. Both forms are permitted by the navigation handler.

### IPC bridge is not available

1. **Check `window.jarvis`.** It should exist even before your script runs.
2. **Make sure you are loading via `jarvis://`**, not `file://` or `http://`. The IPC bridge is only injected for webviews created by Jarvis.

### CSS variables not working

1. **Use fallback values.** Always write `var(--color-primary, #cba6f7)` with a fallback. The second value is used if the variable is not set.
2. **Check in DevTools** that the `:root` styles are being injected. Theme injection happens via `inject_theme_into_all_webviews()`.

### Escape key closes my plugin unexpectedly

Pressing Escape when a plugin is loaded navigates back to the previous page. This is by design (same behavior as exiting a game). If you need Escape inside your plugin, capture it at the document level with `stopPropagation`, but be aware the capture-phase IPC listener may still see it. See Section 8.8.3 for details.

### Plugin assets return 404

1. **Check relative paths.** Assets should be relative to your HTML file: `<img src="icon.png">` not `<img src="/icon.png">`. Absolute paths resolve against `jarvis://localhost/`, not your plugin directory.
2. **Check file extensions.** Unknown extensions are served as `application/octet-stream` which may not render correctly.
3. **No directory traversal.** `../` paths outside your plugin folder will return 404 due to the containment check.

### Request-response times out

The `ipc.request()` method has a 10-second timeout. If your request consistently times out:

1. Verify the message `kind` is in the IPC allowlist (Section 8.11.3).
2. Check the Rust logs for any error processing the request.
3. Ensure you are using `request()` for message kinds that support request-response (`clipboard_paste`, `read_file`). Fire-and-forget messages like `clipboard_copy` do not send a response.

### localStorage data lost or shared unexpectedly

All plugins share the same `jarvis://localhost` localStorage origin. If you do not namespace your keys, one plugin may overwrite another's data. Always prefix keys with your plugin ID:

```javascript
localStorage.setItem('my-plugin:key', value);  // Good
localStorage.setItem('key', value);             // Bad -- may collide
```

---

## 8.19 Reference: Full IPC Message Table

### JS to Rust (send)

| Kind | Payload | Notes |
|------|---------|-------|
| `ping` | `{}` | Rust replies with `pong` |
| `clipboard_copy` | `{ text: string }` | Copy to system clipboard |
| `clipboard_paste` | `{}` | Request-response. Returns `{ kind, text?, data_url? }` |
| `open_url` | `{ url: string }` | Navigate current pane. Auto-prepends `https://` if no scheme. |
| `launch_game` | `{ game: string }` | Launch built-in game |
| `open_panel` | `{ kind: string }` | Open new panel |
| `panel_close` | `{}` | Close current panel |
| `open_settings` | `{}` | Open settings panel |
| `pty_input` | `{ data: string }` | Send to terminal |
| `pty_resize` | `{ cols, rows }` | Resize terminal |
| `read_file` | `{ path: string }` | Request-response. Read image file from disk (max 5 MB). Returns `{ data_url }`. |
| `keybind` | `{ key, ctrl, alt, shift, meta }` | Simulate key combo |
| `window_drag` | `{}` | Start window drag |
| `debug_event` | `{ type, ...data }` | Log to Rust |
| `panel_focus` | `{}` | Auto-sent on mousedown by the initialization script |

### Rust to JS (on)

| Kind | Payload | Notes |
|------|---------|-------|
| `pong` | `"pong"` | Reply to `ping` |
| `palette_show` | `{ items, query, selectedIndex, mode, placeholder }` | Auto-handled by injected script |
| `palette_update` | `{ items, query, selectedIndex, mode, placeholder }` | Auto-handled by injected script |
| `palette_hide` | `{}` | Auto-handled by injected script |
