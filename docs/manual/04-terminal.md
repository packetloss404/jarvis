# Terminal & Shell Subsystem

This document describes the architecture and behavior of the Jarvis terminal
emulation subsystem: how keystrokes flow from the user to a shell process and
back, how the terminal is configured, and how multiple terminal panes are
managed.

---

## 1. Architecture Overview

The terminal is a split-process design:

```
 +---------------------------+         IPC (JSON)        +-------------------+
 |   WebView (xterm.js)      | <======================> |    Rust Backend    |
 |  - Terminal rendering     |   pty_input / pty_output  | - PTY management  |
 |  - Keyboard capture       |   pty_resize / pty_exit   | - Shell spawning  |
 |  - Selection / clipboard  |   terminal_ready          | - I/O bridging    |
 +---------------------------+                           +-------------------+
         ^                                                        |
         |  DOM events                                            | portable-pty
         v                                                        v
     User's keyboard                                      OS PTY + Shell
```

**Frontend**: xterm.js v5.5.0 running inside a per-pane WebView panel. The
terminal HTML is served from a bundled asset at
`jarvis://localhost/terminal/index.html`. xterm.js handles all rendering,
cursor display, scrollback, selection, and ANSI/VT processing on the
JavaScript side.

**Backend**: The `portable-pty` crate provides cross-platform pseudo-terminal
support. Each terminal pane gets its own PTY master/slave pair with a
dedicated reader thread. The Rust `PtyManager` (keyed by pane ID) owns all
active PTY handles.

**Bridge**: The `jarvis-webview` crate provides a JSON-over-IPC bridge
(`window.jarvis.ipc`) that connects the two halves. Messages flow
bidirectionally through this bridge.

### Key source files

| Component | File |
|---|---|
| PTY bridge module | `jarvis-rs/crates/jarvis-app/src/app_state/pty_bridge/mod.rs` |
| PTY spawn logic | `jarvis-rs/crates/jarvis-app/src/app_state/pty_bridge/spawn.rs` |
| PTY I/O + resize + kill | `jarvis-rs/crates/jarvis-app/src/app_state/pty_bridge/io.rs` |
| PTY types (PtyHandle, PtyManager) | `jarvis-rs/crates/jarvis-app/src/app_state/pty_bridge/types.rs` |
| IPC handlers for PTY messages | `jarvis-rs/crates/jarvis-app/src/app_state/webview_bridge/pty_handlers.rs` |
| PTY output polling + broadcast | `jarvis-rs/crates/jarvis-app/src/app_state/webview_bridge/pty_polling.rs` |
| IPC dispatch (allowlist + routing) | `jarvis-rs/crates/jarvis-app/src/app_state/webview_bridge/ipc_dispatch.rs` |
| Terminal panel HTML/JS | `jarvis-rs/assets/panels/terminal/index.html` |
| IPC protocol + init script | `jarvis-rs/crates/jarvis-webview/src/ipc.rs` |
| Shell config schema | `jarvis-rs/crates/jarvis-config/src/schema/shell.rs` |
| Terminal config schema | `jarvis-rs/crates/jarvis-config/src/schema/terminal.rs` |
| Theme injection for xterm.js | `jarvis-rs/crates/jarvis-app/src/app_state/webview_bridge/theme_handlers.rs` |
| Key encoding for terminal | `jarvis-rs/crates/jarvis-platform/src/input_processor/encoding.rs` |
| Input processor (mode routing) | `jarvis-rs/crates/jarvis-platform/src/input_processor/processor.rs` |
| WebView lifecycle (create/destroy) | `jarvis-rs/crates/jarvis-app/src/app_state/webview_bridge/lifecycle.rs` |

---

## 2. Shell Configuration

Shell settings live in `[shell]` in `jarvis.toml`, defined by
`ShellConfig` in `jarvis-rs/crates/jarvis-config/src/schema/shell.rs`.

```toml
[shell]
program = ""                    # Empty = auto-detect
args = []                       # Extra arguments to pass
working_directory = "~/code"    # Initial cwd (~ is expanded)
login_shell = true              # Pass -l on Unix

[shell.env]
EDITOR = "nvim"                 # Extra env vars injected into shell
```

### Fields

| Field | Type | Default | Description |
|---|---|---|---|
| `program` | `String` | `""` (auto-detect) | Shell executable path. Empty string triggers auto-detection. |
| `args` | `Vec<String>` | `[]` | Extra arguments appended to the shell command. |
| `working_directory` | `Option<String>` | `None` (inherit) | Initial working directory. Supports `~` expansion. Falls back to the parent process's cwd if the path does not exist. |
| `env` | `HashMap<String, String>` | `{}` | Additional environment variables injected into the shell. |
| `login_shell` | `bool` | `true` | On Unix, appends `-l` to spawn a login shell (loads `.profile`, `.bash_profile`, etc.). |

### Shell auto-detection

When `program` is empty (the default), the shell is detected at spawn time in
`spawn.rs`:

- **Unix**: Reads `$SHELL`, falls back to `/bin/sh`.
- **Windows**: Reads `$COMSPEC`, falls back to `cmd.exe`.

### Environment sanitization

The PTY spawn logic does **not** inherit the full parent environment. Instead,
it calls `env_clear()` and selectively re-adds only a safe allowlist of
variables to prevent leaking API keys, tokens, or other secrets from the
Jarvis host process into the shell. The allowed variables are:

```
HOME, USER, LOGNAME, SHELL, PATH, TERM, LANG, LC_ALL, LC_CTYPE,
DISPLAY, WAYLAND_DISPLAY, XDG_RUNTIME_DIR, TMPDIR, TMP, TEMP,
USERPROFILE, APPDATA, LOCALAPPDATA, SYSTEMROOT, COMSPEC, HOMEDRIVE, HOMEPATH
```

In addition, `TERM` is always forced to `xterm-256color` regardless of the
inherited value.

Any variables specified in `[shell.env]` are added on top of the inherited
set.

---

## 3. Terminal Configuration

Terminal display settings live in `[terminal]` in `jarvis.toml`, defined by
`TerminalConfig` in `jarvis-rs/crates/jarvis-config/src/schema/terminal.rs`.

```toml
[terminal]
scrollback_lines = 10000
cursor_style = "block"          # "block", "underline", or "beam"
cursor_blink = true
cursor_blink_interval_ms = 500
true_color = true
word_separators = " /\\()\"'-.,:;<>~!@#$%^&*|+=[]{}~?"

[terminal.bell]
visual = true
audio = false
duration_ms = 150

[terminal.mouse]
copy_on_select = false
url_detection = true
click_to_focus = true

[terminal.search]
wrap_around = true
regex = false
case_sensitive = false
```

### Top-level terminal fields

| Field | Type | Default | Description |
|---|---|---|---|
| `scrollback_lines` | `u32` | `10000` | Maximum scrollback buffer lines (valid: 0--100,000). |
| `cursor_style` | `CursorStyle` | `block` | Cursor shape: `block`, `underline`, or `beam`. Note: `beam` maps to xterm.js `"bar"`. |
| `cursor_blink` | `bool` | `true` | Whether the cursor blinks. |
| `cursor_blink_interval_ms` | `u32` | `500` | Blink interval in ms (valid: 100--2000). |
| `true_color` | `bool` | `true` | Enable 24-bit true color support. |
| `word_separators` | `String` | (punctuation set) | Characters treated as word boundaries for double-click selection. |

### Bell sub-config `[terminal.bell]`

| Field | Type | Default | Description |
|---|---|---|---|
| `visual` | `bool` | `true` | Flash the terminal on BEL character. |
| `audio` | `bool` | `false` | Play a sound on BEL character. |
| `duration_ms` | `u32` | `150` | Visual bell flash duration (valid: 50--1000). |

### Mouse sub-config `[terminal.mouse]`

| Field | Type | Default | Description |
|---|---|---|---|
| `copy_on_select` | `bool` | `false` | Automatically copy text to clipboard on mouse selection. |
| `url_detection` | `bool` | `true` | Detect and highlight clickable URLs in terminal output. |
| `click_to_focus` | `bool` | `true` | Focus the terminal pane when clicked. |

### Search sub-config `[terminal.search]`

| Field | Type | Default | Description |
|---|---|---|---|
| `wrap_around` | `bool` | `true` | Wrap search at scrollback boundaries. |
| `regex` | `bool` | `false` | Enable regex search by default. |
| `case_sensitive` | `bool` | `false` | Case-sensitive search by default. |

### Theme integration

Terminal settings (`cursor_style`, `cursor_blink`, `scrollback_lines`) plus
font settings and color palette are bundled into a single `theme` IPC message
and dispatched to xterm.js when:

1. The terminal WebView first loads (via `__jarvis_theme` global).
2. The theme changes at runtime (via the `theme` IPC dispatch).

The mapping from config to xterm.js properties happens in
`config_to_xterm_theme()` in `theme_handlers.rs`. Notably, the config
`CursorStyle::Beam` maps to xterm.js `"bar"`.

The xterm.js theme object includes a full 16-color ANSI palette
(Catppuccin Mocha by default), plus `background`, `foreground`, `cursor`,
`cursorAccent`, `selectionBackground`, and `selectionForeground`.

---

## 4. PTY Lifecycle

### 4.1 Creation

PTYs are created in two scenarios:

1. **`terminal_ready` IPC message**: When a terminal panel's JavaScript
   finishes initializing, it sends `terminal_ready` with `{ cols, rows }`.
   The Rust handler spawns a PTY and inserts it into the `PtyManager`.

2. **`pty_restart` IPC message**: The user clicks "Restart Shell" in the
   exit overlay. The Rust handler kills the existing PTY, removes it, then
   spawns a fresh one.

The spawn sequence (`spawn_pty()` in `spawn.rs`):

```
1. native_pty_system()           -- get OS PTY system
2. openpty(PtySize{cols,rows})   -- create master/slave pair
3. build_shell_command(shell)    -- configure shell with sanitized env
4. slave.spawn_command(cmd)      -- fork shell into slave side
5. drop(slave)                   -- only master is needed
6. master.take_writer()          -- get writer for input
7. master.try_clone_reader()     -- get reader for output
8. thread::spawn(pty-reader)     -- background thread reads output
9. Return PtyHandle              -- caller inserts into PtyManager
```

### 4.2 Default dimensions

If the IPC message does not include valid `cols`/`rows`, the PTY falls back
to the defaults:

- `DEFAULT_COLS` = 80
- `DEFAULT_ROWS` = 24

Size validation rejects values of 0 or greater than 500 for either dimension.

### 4.3 The PtyHandle

Each PTY is represented by a `PtyHandle` struct:

```rust
pub struct PtyHandle {
    writer: Box<dyn Write + Send>,           // input to PTY
    output_rx: mpsc::Receiver<Vec<u8>>,      // output from reader thread
    child: Box<dyn Child + Send + Sync>,     // shell process handle
    master: Box<dyn MasterPty + Send>,       // for resize operations
    size: PtySize,                           // current cols x rows
}
```

### 4.4 The PtyManager

`PtyManager` is a `HashMap<u32, PtyHandle>` keyed by pane ID. It provides
convenience methods:

- `write_input(pane_id, data)` -- forward keystrokes to a pane's PTY.
- `resize(pane_id, cols, rows)` -- resize a pane's PTY.
- `kill_and_remove(pane_id)` -- kill the shell and remove the handle.
- `kill_all()` -- used during shutdown.
- `drain_all_output()` -- collect output from every PTY (non-blocking).
- `check_finished()` -- find PTYs whose shell has exited.

### 4.5 Resize

Resize is triggered when:

- xterm.js fires `term.onResize()` (after the `FitAddon` recalculates
  dimensions due to a container resize).
- The JavaScript sends `pty_resize` with `{ cols, rows }`.
- The Rust handler calls `self.ptys.resize(pane_id, cols, rows)`.
- `PtyHandle::resize()` calls `master.resize(PtySize{...})` to notify the
  OS PTY of the new window size.

A `ResizeObserver` on the `#terminal-container` div ensures the xterm.js
`FitAddon` is triggered whenever the container's dimensions change (e.g.,
window resize, pane layout change).

### 4.6 Shutdown and cleanup

On application exit, `shutdown()` calls `ptys.kill_all()`, which iterates
every active PTY, kills the child process, waits for the exit code, and
removes the handle. This runs before webview destruction.

When a single pane is closed via `panel_close`, the lifecycle handler calls
`destroy_webview_for_pane(pane_id)`, which first kills the PTY via
`ptys.kill_and_remove(pane_id)` and then destroys the WebView.

---

## 5. IPC Protocol

All IPC messages are JSON objects with the shape:

```json
{ "kind": "<message_type>", "payload": <any> }
```

### 5.1 JS-to-Rust messages (terminal-related)

| Kind | Payload | When sent | Handler |
|---|---|---|---|
| `terminal_ready` | `{ cols, rows }` | xterm.js initialized | `handle_terminal_ready()` -- spawns a new PTY |
| `pty_input` | `{ data: "<string>" }` | User types in terminal | `handle_pty_input()` -- writes bytes to PTY |
| `pty_resize` | `{ cols, rows }` | Terminal container resized | `handle_pty_resize()` -- resizes PTY |
| `pty_restart` | `{ cols, rows }` | "Restart Shell" button clicked | `handle_pty_restart()` -- kills old PTY, spawns new |
| `panel_focus` | `{}` | Mouse click in any panel | Sets tiling focus to this pane |
| `clipboard_copy` | `{ text }` | Cmd+C with selection | Writes text to system clipboard |
| `clipboard_paste` | `{ _reqId }` | Cmd+V in terminal | Returns clipboard text/image via response |
| `keybind` | `{ key, ctrl, alt, shift, meta }` | Cmd+key or Escape pressed | Routes through keybind registry |
| `debug_event` | (various) | Diagnostic logging | Logged at INFO level |

### 5.2 Rust-to-JS messages (terminal-related)

| Kind | Payload | When sent | Effect |
|---|---|---|---|
| `pty_output` | `{ data: "<string>" }` | PTY has output bytes | `term.write(data)` in xterm.js |
| `pty_exit` | `{ code: <number> }` | Shell process exited | Shows "Process exited" overlay |
| `theme` | (xterm theme + font settings) | Theme change or initial load | Updates xterm.js theme, font, cursor |
| `focus_changed` | `{ focused: bool }` | Pane focus changes | `term.focus()` or blur |
| `clipboard_paste_response` | `{ _reqId, kind, text/data_url }` | Response to clipboard_paste | Inserts text via pty_input or dispatches paste-image event |

### 5.3 IPC allowlist

All incoming IPC message kinds are validated against a compile-time
allowlist in `ipc_dispatch.rs`. Any message with an unrecognized `kind` is
rejected and logged. This prevents arbitrary message injection from the
WebView context.

### 5.4 IPC bridge initialization

Every WebView has `IPC_INIT_SCRIPT` injected at creation. This script
establishes:

- `window.jarvis.ipc.send(kind, payload)` -- fire-and-forget messages.
- `window.jarvis.ipc.request(kind, payload)` -- Promise-based
  request/response (used for clipboard paste). Has a 10-second timeout.
- `window.jarvis.ipc.on(kind, callback)` -- register a handler for
  Rust-to-JS messages.
- `window.jarvis.ipc._dispatch(kind, payload)` -- internal dispatch with
  request-response resolution.

The bridge also installs global event listeners for `mousedown` (sends
`panel_focus`), `keydown` (diagnostic logging + shortcut forwarding), and
`focus`/`blur` (diagnostic logging).

---

## 6. Data Flow: Keystroke to Terminal Output

### 6.1 Input path (keystroke to shell)

There are two parallel input paths depending on the platform:

**Path A: via winit (native key events)**

```
User presses key
  --> winit KeyEvent
    --> InputProcessor.process_key()
      --> KeybindRegistry.lookup() -- check for app shortcut
        [match] --> dispatch Action (e.g., NewPanel, ToggleFullscreen)
        [no match, Terminal mode] --> encode_key_for_terminal()
          --> InputResult::TerminalInput(bytes)
            --> ptys.write_input(focused_pane_id, bytes)
              --> PtyHandle.write_input() -- write_all + flush to PTY
```

**Path B: via WebView IPC (xterm.js captures the key)**

```
User presses key in WebView
  --> xterm.js term.onData(data)
    --> window.jarvis.ipc.send('pty_input', { data })
      --> IPC handler parses JSON
        --> handle_pty_input(pane_id, payload)
          --> ptys.write_input(pane_id, data.as_bytes())
            --> PtyHandle.write_input() -- write_all + flush to PTY
```

Path B is the primary path when the terminal WebView has focus. xterm.js
handles the key translation (including special keys, modifiers, and terminal
escape sequences) and sends the already-encoded data string via IPC.

Path A is used as a fallback when winit captures keys before the WebView
(less common on macOS with WKWebView, more common on other platforms).

### 6.2 Output path (shell output to screen)

```
Shell writes to PTY slave
  --> OS PTY mechanism delivers bytes to master reader
    --> pty-reader thread: reader.read(&mut buf)
      --> mpsc::channel tx.send(buf[..n].to_vec())

[Every 8ms in poll loop]
  --> poll_pty_output()
    --> ptys.drain_all_output()
      --> PtyHandle.drain_output() -- try_recv in loop, up to 64KB
        --> For each (pane_id, data):
          --> String::from_utf8_lossy(data)
          --> webview.send_ipc("pty_output", { data: text })
            --> JS: window.jarvis.ipc._dispatch("pty_output", payload)
              --> term.write(payload.data)
                --> xterm.js renders to canvas
```

### 6.3 Output throttling

- The reader thread reads in 8 KB chunks (`PTY_READ_CHUNK = 8192`).
- `drain_output()` accumulates chunks up to 64 KB per frame
  (`PTY_MAX_OUTPUT_PER_FRAME = 65536`), then truncates.
- The main poll loop runs at ~125 Hz (`POLL_INTERVAL = 8ms`), so the
  effective maximum throughput is roughly 64 KB * 125 = 8 MB/s.

### 6.4 Shell exit detection

```
Shell exits
  --> pty-reader thread gets Ok(0) (EOF) or Err
    --> reader thread exits, channel disconnects

[In poll loop]
  --> ptys.check_finished()
    --> try_recv returns Disconnected
      --> pane_id added to "finished" list
        --> ptys.kill_and_remove(pane_id)
          --> webview.send_ipc("pty_exit", { code })
            --> JS shows exit overlay with exit code
```

---

## 7. Copy/Paste Behavior

### 7.1 Copy (Cmd+C / Ctrl+Shift+C)

The IPC init script intercepts `Cmd+C` in the WebView `keydown` handler:

1. Checks `window._xtermInstance.getSelection()` for selected text in the
   terminal.
2. Falls back to `window.getSelection().toString()` for DOM selection.
3. If text is found, sends `clipboard_copy` IPC with `{ text }`.
4. The Rust handler writes the text to the system clipboard via
   `jarvis_platform::Clipboard`.

### 7.2 Paste (Cmd+V / Ctrl+Shift+V)

Paste uses the request/response IPC pattern:

1. IPC init script intercepts `Cmd+V`, calls
   `window.jarvis.ipc.request('clipboard_paste', {})`.
2. The Rust handler reads the system clipboard:
   - Tries image first (`cb.get_image()`) -- encodes as PNG base64 data URL.
   - Falls back to text (`cb.get_text()`).
3. Returns the result as a response with `_reqId`.
4. The JS promise resolves:
   - **Text in terminal**: If no focused input element exists but
     `_xtermInstance` is available, sends `pty_input` with the clipboard
     text (so the text enters the PTY as if typed).
   - **Text in input field**: Uses `document.execCommand('insertText')`.
   - **Image**: Dispatches a `jarvis:paste-image` custom DOM event.

### 7.3 Bracketed paste

The `InputProcessor` supports bracketed paste mode. When enabled (via
`set_bracketed_paste(true)`), pasted text is wrapped in escape sequences:

```
ESC[200~ <pasted text> ESC[201~
```

This allows shells like zsh/fish to distinguish pasted text from typed input.

---

## 8. Keyboard Handling in Terminal Mode

### 8.1 Input modes

The `InputProcessor` operates in one of four modes:

| Mode | Key routing |
|---|---|
| `Terminal` | Keybinds checked first; unmatched keys encoded for PTY. |
| `CommandPalette` | All keys routed to palette filter. |
| `Settings` | Keys consumed (not sent to terminal). |
| `Assistant` | Keys routed to assistant input. |

### 8.2 Key encoding

When a key is not matched by any keybind and the mode is `Terminal`, it is
encoded via `encode_key_for_terminal()` in `encoding.rs`. The encoder handles:

- **Editing keys**: Enter (CR), Backspace (DEL/0x7F), Tab (HT), Escape (ESC),
  Space, Delete (CSI 3~), Insert (CSI 2~).
- **Arrow keys**: Up/Down/Left/Right as CSI A/B/C/D.
- **Navigation**: Home (CSI H), End (CSI F), PageUp (CSI 5~), PageDown
  (CSI 6~).
- **Function keys**: F1--F12 as SS3 or CSI sequences.
- **Ctrl+letter**: Maps to control codes (Ctrl+A = 0x01, ..., Ctrl+Z = 0x1A).
- **Ctrl+[**: ESC (0x1B).
- **Ctrl+\\**: 0x1C.
- **Ctrl+]**: 0x1D.
- **Alt prefix**: Prepends ESC (0x1B) before the character for Alt+key.
- **Printable characters**: Passed through as UTF-8 bytes.

### 8.3 WKWebView shortcut forwarding

On macOS, WKWebView captures `Cmd+key` before winit sees them. The IPC init
script intercepts these in a `keydown` listener and forwards them to Rust via
the `keybind` IPC message. The Rust handler looks them up in the
`KeybindRegistry` and dispatches the matching action.

When an overlay (command palette or assistant) is active, **all** non-repeat
key events are forwarded to Rust via `keybind`, with `preventDefault()` to
block them from reaching xterm.js.

Certain Cmd+key combinations are explicitly **not** forwarded and are left to
the WebView's native handling: Cmd+R, Cmd+L, Cmd+Q, Cmd+A, Cmd+X, Cmd+Z.

---

## 9. Multi-Terminal Support (One PTY per Pane)

### 9.1 Pane/PTY mapping

Every tiling pane that hosts a terminal gets its own independent PTY. The
`PtyManager` maps `pane_id: u32` to `PtyHandle`. This means:

- Each pane runs a separate shell process.
- Each pane has its own scrollback buffer (in xterm.js).
- Resize is per-pane -- each PTY tracks its own `cols x rows`.
- Focus determines which pane receives keyboard input.

### 9.2 Creating new terminal panes

When a new pane is added to the tiling layout:

1. `create_webview_for_pane(pane_id)` creates a WebView loading
   `jarvis://localhost/terminal/index.html`.
2. xterm.js initializes, computes its dimensions via FitAddon, and sends
   `terminal_ready` with `{ cols, rows }`.
3. The Rust handler spawns a new PTY and inserts it into the `PtyManager`.

### 9.3 Closing terminal panes

When a pane is closed:

1. The PTY is killed first (`ptys.kill_and_remove(pane_id)`).
2. The WebView is then destroyed (`registry.destroy(pane_id)`).
3. The tiling layout is updated and remaining pane bounds are synced.

Closing the last pane is refused -- at least one pane always remains.

### 9.4 Focus management

- Clicking inside a WebView sends `panel_focus` IPC, which calls
  `tiling.focus_pane(pane_id)`.
- When focus changes, the Rust side sends `focus_changed` IPC with
  `{ focused: true/false }` to the affected panes.
- The terminal panel responds by calling `term.focus()` on the xterm.js
  instance when focused.
- On page load completion in the focused pane, native WebView focus is
  re-granted via `handle.focus()`.

---

## 10. Terminal-Specific Features

### 10.1 Process exit overlay

When the shell exits, xterm.js receives `pty_exit` with the exit code. The
terminal panel displays an overlay:

```
    +-------------------------------+
    |                               |
    |   Process exited (code 0)     |
    |   [ Restart Shell ]           |
    |                               |
    +-------------------------------+
```

Clicking "Restart Shell" clears the terminal (`term.clear()`), hides the
overlay, and sends `pty_restart` to spawn a new shell.

### 10.2 Terminal clear

`term.clear()` is called on restart. The xterm.js scrollback and viewport
are flushed. This is a JS-side operation; the PTY is not involved.

### 10.3 Scrollback

Scrollback depth is configured via `terminal.scrollback_lines` (default:
10,000 lines). This value is sent to xterm.js in the `theme` IPC payload and
applied via `term.options.scrollback`. The xterm.js instance manages the
scrollback buffer entirely in the browser.

### 10.4 Fit addon (auto-sizing)

The xterm.js `FitAddon` (v0.10.0) automatically calculates the terminal
grid dimensions to fill the container. It is triggered by:

- A `ResizeObserver` on `#terminal-container` (fires on any container resize).
- Theme updates that change font size or line height (explicit
  `fitAddon.fit()` calls).

When the addon recalculates, xterm.js fires `term.onResize()`, which sends
`pty_resize` to synchronize the PTY.

### 10.5 URL detection

When `terminal.mouse.url_detection` is `true` (default), xterm.js detects
URLs in terminal output and renders them as clickable links. This is handled
by xterm.js's built-in link detection (via the `allowProposedApi: true`
terminal option).

### 10.6 Mobile relay

PTY output is also broadcast to connected mobile clients via the WebSocket
relay bridge. The `poll_pty_output()` function sends `ServerMessage::PtyOutput`
to the `mobile_broadcaster` for each pane that has new output, and
`ServerMessage::PtyExit` when a shell exits. This allows remote terminal
viewing from the Jarvis mobile companion app.

---

## 11. xterm.js Terminal Instance Details

The terminal is instantiated in `index.html` with these defaults (overridden
by theme IPC):

```javascript
var term = new Terminal({
    cursorBlink: true,
    cursorStyle: 'block',
    scrollback: 10000,
    fontSize: 13,
    fontFamily: "'Menlo', monospace",
    lineHeight: 1.6,
    fontWeight: '400',
    fontWeightBold: '700',
    allowProposedApi: true,
    theme: AYU    // Ayu Mirage palette as initial default
});
```

The `FitAddon` is loaded and the terminal is opened into `#terminal-container`.
The xterm instance is also stored as `window._xtermInstance` so the IPC init
script can access it for selection reading during copy operations.

### CDN dependencies

| Library | Version | Purpose |
|---|---|---|
| `@xterm/xterm` | 5.5.0 | Terminal emulation and rendering |
| `@xterm/addon-fit` | 0.10.0 | Auto-sizing to container |

Both are loaded from jsDelivr CDN with SRI integrity hashes.

---

## 12. Security Considerations

### Environment isolation

The PTY environment is sanitized to prevent secret leakage. API keys, tokens,
and passwords from the Jarvis process are not inherited by the shell. Only
a curated allowlist of safe variables is forwarded.

### IPC allowlist

All IPC message kinds are validated against a compile-time allowlist. Unknown
message types are rejected. The allowlist includes exactly the expected
message types, and is case-sensitive.

### Navigation allowlist

WebView navigation is restricted to `jarvis://`, `about:blank`, and the
WebView2 Windows rewrite `http://jarvis.localhost`. HTTPS URLs are permitted.
Schemes like `file://`, `javascript:`, and `data:` are blocked.

### Size validation

PTY resize payloads are bounds-checked: cols and rows must each be between 1
and 500 (inclusive). Payloads with zero or oversized dimensions are silently
rejected.
