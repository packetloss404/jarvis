# Input System & Command Palette

This document covers the complete input pipeline in Jarvis: how keyboard and
mouse events travel from the operating system through winit, get processed by
the `InputProcessor`, matched against the `KeybindRegistry`, resolved to
`Action` variants, and dispatched to subsystems.  It also covers the command
palette -- a searchable overlay that exposes every action to the user.

---

## Table of Contents

1. [Input Modes](#input-modes)
2. [The Action Enum](#the-action-enum)
3. [Keybind System](#keybind-system)
4. [Default Keybinds Table](#default-keybinds-table)
5. [Input Flow: End-to-End](#input-flow-end-to-end)
6. [The Command Palette](#the-command-palette)
7. [Palette Categories](#palette-categories)
8. [Plugin Items in the Palette](#plugin-items-in-the-palette)
9. [Full Dispatch Table](#full-dispatch-table)
10. [Mouse Input Handling](#mouse-input-handling)
11. [Modifier Key Tracking](#modifier-key-tracking)
12. [Terminal Key Encoding](#terminal-key-encoding)

---

## Input Modes

The `InputProcessor` maintains an `InputMode` that determines how key events
are routed.  There are four modes:

| Mode              | Description                                                                 |
|-------------------|-----------------------------------------------------------------------------|
| `Terminal`        | Default mode.  Keybinds are checked first; unmatched keys are encoded as terminal bytes and sent to the PTY via xterm.js. |
| `CommandPalette`  | The command palette overlay is open.  All keys are routed to the palette's filter/selection handler.  Only keybind lookups still fire (so the user can dismiss the palette with its keybind). |
| `Assistant`       | The AI assistant panel is open.  Keys are routed to the assistant's text input and scroll handler. |
| `Settings`        | The settings UI is open.  Keys are consumed (not forwarded to the terminal). |

Mode transitions happen when overlays are opened or closed:

- `OpenCommandPalette` sets mode to `CommandPalette`.
- `OpenAssistant` toggles between `Assistant` and `Terminal`.
- `CloseOverlay` returns mode to `Terminal`.
- `OpenSettings` sets mode to `Settings`.

**Source**: `jarvis-rs/crates/jarvis-platform/src/input_processor/types.rs`

---

## The Action Enum

Every user-triggerable operation in Jarvis is represented by a variant of
`Action`.  Keybinds, the command palette, CLI commands, and IPC messages all
resolve to an `Action` before being dispatched.

**Source**: `jarvis-rs/crates/jarvis-common/src/actions/action_enum.rs`

### Complete Action Reference

| Variant | Label | Category | Description |
|---------|-------|----------|-------------|
| `NewPane` | New Pane | Panes | Create a new terminal pane (auto-splits in the optimal direction). Respects `max_panels` config limit. |
| `ClosePane` | Close Pane | Panes | Close the currently focused pane and destroy its webview. |
| `SplitHorizontal` | Split Horizontal | Panes | Split the focused pane horizontally (side-by-side). |
| `SplitVertical` | Split Vertical | Panes | Split the focused pane vertically (top-bottom). |
| `FocusPane(n)` | Focus Pane _n_ | Panes | Focus pane by 1-based index (1 through 5 have explicit labels). |
| `FocusNextPane` | Focus Next Pane | Panes | Cycle focus to the next pane. |
| `FocusPrevPane` | Focus Previous Pane | Panes | Cycle focus to the previous pane. |
| `ZoomPane` | Zoom Pane | Panes | Toggle zoom (maximize) on the focused pane. |
| `ResizePane { direction, delta }` | Resize Pane | Panes | Resize the focused pane in the given direction by `delta` pixels. |
| `SwapPane(direction)` | Swap Pane | Panes | Swap the focused pane with its neighbor in the given direction. |
| `ToggleFullscreen` | Toggle Fullscreen | Window | Toggle borderless fullscreen on the application window. |
| `Quit` | Quit | Window | Publish a `Shutdown` event, clean up, and exit the application. |
| `OpenCommandPalette` | Command Palette | Apps | Open the command palette overlay. |
| `OpenSettings` | Open Settings | Apps | Open the settings panel in a new horizontal split. |
| `OpenChat` | Open Chat | Apps | Open the chat panel in a new horizontal split. |
| `CloseOverlay` | Close Overlay | Apps | Close whichever overlay is open (assistant or palette) and return to `Terminal` mode. |
| `OpenURLPrompt` | Open URL | Apps | Sentinel action -- never dispatched.  Intercepted by the palette to enter URL input mode. |
| `OpenAssistant` | Open Assistant | Apps | Toggle the AI assistant panel (opens or closes). |
| `PushToTalk` | Push to Talk | System | Begin push-to-talk voice capture. On key _release_, emits `ReleasePushToTalk`. |
| `ReleasePushToTalk` | Release Push to Talk | System | End push-to-talk voice capture. Synthesized by `InputProcessor` on key release. |
| `ScrollUp(n)` | Scroll Up | Terminal | Scroll the terminal up by `n` lines. Handled by the webview. |
| `ScrollDown(n)` | Scroll Down | Terminal | Scroll the terminal down by `n` lines. Handled by the webview. |
| `ScrollToTop` | Scroll to Top | Terminal | Scroll to the top of terminal history. |
| `ScrollToBottom` | Scroll to Bottom | Terminal | Scroll to the bottom of terminal output. |
| `Copy` | Copy | Terminal | Copy the current selection from xterm.js or the DOM to the system clipboard. |
| `Paste` | Paste | Terminal | Read from the system clipboard and paste into the focused pane. Handles both input elements and terminal PTY input. |
| `SelectAll` | Select All | Terminal | Select all text in the focused pane. |
| `SearchOpen` | Find | Terminal | Open the terminal search bar. |
| `SearchClose` | Close Find | Terminal | Close the terminal search bar. |
| `SearchNext` | Find Next | Terminal | Jump to the next search match. |
| `SearchPrev` | Find Previous | Terminal | Jump to the previous search match. |
| `ClearTerminal` | Clear Terminal | Terminal | Clear the terminal scrollback via `xterm.clear()`. |
| `LaunchGame(name)` | Play _Name_ | Games | Navigate the focused pane's webview to `jarvis://localhost/games/{name}.html`. Stores the original URL for Escape-to-exit. |
| `OpenURL(url)` | (context-dependent) | Games / Web | Navigate the focused pane to the given URL. Auto-prepends `https://` if no scheme is present. Game URLs (kartbros, basketbros, etc.) are categorized as "Games"; everything else is "Web". |
| `PairMobile` | Pair Mobile Device | System | Display a pairing code for mobile device connection. |
| `RevokeMobilePairing` | Revoke Mobile Pairing | System | Revoke the current mobile pairing and regenerate the session ID. |
| `ReloadConfig` | Reload Config | System | Re-read the TOML config, rebuild the keybind registry, re-register plugin directories, and re-inject the theme into all webviews. |
| `None` | None | System | No-op sentinel. Never dispatched. |

### ResizeDirection

Used by `ResizePane` and `SwapPane`:

| Variant | Meaning |
|---------|---------|
| `Left` | Resize or swap leftward |
| `Right` | Resize or swap rightward |
| `Up` | Resize or swap upward |
| `Down` | Resize or swap downward |

**Source**: `jarvis-rs/crates/jarvis-common/src/actions/mod.rs`

---

## Keybind System

### Architecture Overview

The keybind system is spread across three crates:

| Crate | Responsibility |
|-------|---------------|
| `jarvis-config` | Defines `KeybindConfig` (the schema) and validation (`keybinds.rs`). |
| `jarvis-platform` | Parses keybind strings (`keymap/`), converts winit events to `KeyCombo`, and maps combos to `Action` via `KeybindRegistry` (`input/`). |
| `jarvis-app` | Owns the `InputProcessor` and calls `process_key()` from the event handler. |

### Configuration (`KeybindConfig`)

Keybinds are configured in a TOML file under the `[keybinds]` section.  Each
field is a string in `"Modifier+Key"` format.

**Source**: `jarvis-rs/crates/jarvis-config/src/schema/keybind_config.rs`

```toml
[keybinds]
push_to_talk = "Option+Period"
open_assistant = "Cmd+G"
new_panel = "Cmd+T"
close_panel = "Escape+Escape"
toggle_fullscreen = "Cmd+F"
open_settings = "Cmd+,"
open_chat = "Cmd+J"
focus_panel_1 = "Cmd+1"
focus_panel_2 = "Cmd+2"
focus_panel_3 = "Cmd+3"
focus_panel_4 = "Cmd+4"
focus_panel_5 = "Cmd+5"
cycle_panels = "Tab"
cycle_panels_reverse = "Shift+Tab"
split_vertical = "Cmd+D"
split_horizontal = "Cmd+Shift+D"
close_pane = "Cmd+W"
command_palette = "Cmd+Shift+P"
copy = "Cmd+C"
paste = "Cmd+V"
```

### Modifier Names (Case-Insensitive)

The parser recognizes the following modifier tokens:

| Token(s) | Resolved Modifier | Notes |
|----------|-------------------|-------|
| `Ctrl`, `Control` | `Ctrl` | |
| `Alt` | `Alt` | |
| `Option`, `Opt` | `Alt` | macOS name for the Alt key |
| `Cmd`, `Command` | `Super` on macOS, `Ctrl` on Windows/Linux | Cross-platform convenience |
| `Super`, `Win`, `Meta` | `Super` | Always maps to the platform super key |
| `Shift` | `Shift` | |

**Source**: `jarvis-rs/crates/jarvis-platform/src/keymap/parse.rs`

### Key Name Normalization

The last token in a keybind string is the key.  It is normalized by the parser:

| Input Token | Normalized Key |
|-------------|---------------|
| `Period` | `.` |
| `Comma` | `,` |
| `Slash` | `/` |
| `Backslash` | `\` |
| `Space` | `Space` |
| `Enter`, `Return` | `Enter` |
| `Escape`, `Esc` | `Escape` |
| `Tab` | `Tab` |
| `Backspace` | `Backspace` |
| `Delete`, `Del` | `Delete` |
| `Up`, `Down`, `Left`, `Right` | As-is (capitalized) |
| `Home`, `End` | As-is |
| `PageUp`, `PageDown` | As-is |
| Single character (e.g. `g`) | Uppercased (`G`) |
| Multi-character (e.g. `F1`) | Title-cased |

### Winit Key Normalization

When winit delivers a key event, the raw key name is normalized to match the
keybind system's canonical names:

| Winit Name | Normalized |
|------------|-----------|
| `ArrowUp` / `ArrowDown` / `ArrowLeft` / `ArrowRight` | `Up` / `Down` / `Left` / `Right` |
| `" "` (space character) | `Space` |
| Single lowercase letter | Uppercased |
| `F1` through `F12` | Pass-through |
| Punctuation (`.`, `,`, `/`, etc.) | Pass-through |

**Source**: `jarvis-rs/crates/jarvis-platform/src/winit_keys.rs`

### KeyCombo

`KeyCombo` is the internal representation used for O(1) HashMap lookups.
Modifiers are stored as a bitmask rather than a sorted vector:

| Bit | Modifier |
|-----|----------|
| `0b0001` (1) | Ctrl |
| `0b0010` (2) | Alt |
| `0b0100` (4) | Shift |
| `0b1000` (8) | Super |

A `KeyCombo` can be constructed from either:
- A parsed `KeyBind` (config path): `KeyCombo::from_keybind(&kb)`
- Raw winit booleans (event path): `KeyCombo::from_winit(ctrl, alt, shift, super_key, key)`

Both paths produce identical `KeyCombo` values for the same logical key
combination, ensuring correct HashMap equality.

**Source**: `jarvis-rs/crates/jarvis-platform/src/input/key_combo.rs`

### KeybindRegistry

The registry is a `HashMap<KeyCombo, Action>`.  It is built from
`KeybindConfig` at startup and rebuilt on `ReloadConfig`.

```
KeybindConfig (TOML strings)
    |
    v  parse_keybind()
KeyBind { modifiers, key }
    |
    v  KeyCombo::from_keybind()
KeyCombo { mods: u8, key: String }
    |
    v  HashMap::insert()
KeybindRegistry { bindings: HashMap<KeyCombo, Action> }
```

The registry supports:
- **Forward lookup**: `lookup(&KeyCombo) -> Option<&Action>` -- used by `InputProcessor`.
- **Reverse lookup**: `keybind_for_action(&Action) -> Option<String>` -- used by the command palette to display keybind hints.
- **Iteration**: `all_bindings()` -- exposes the full map for debug/display.

**Source**: `jarvis-rs/crates/jarvis-platform/src/input/registry.rs`

### Validation

The `keybinds.rs` module in `jarvis-config` provides:
- `all_keybinds(config)` -- returns 18 `(name, binding)` pairs for inspection.
- `validate_no_duplicates(config)` -- ensures no two actions share the same key combination.

**Source**: `jarvis-rs/crates/jarvis-config/src/keybinds.rs`

---

## Default Keybinds Table

The table below shows every default keybind.  The **Config Key** column is the
field name in `[keybinds]`.  The **Binding** column uses the `Cmd` abstraction
(Super on macOS, Ctrl on Windows/Linux).

| Config Key | Binding | Action | Label |
|------------|---------|--------|-------|
| `push_to_talk` | `Option+.` | `PushToTalk` | Push to Talk |
| `open_assistant` | `Cmd+G` | `OpenAssistant` | Open Assistant |
| `new_panel` | `Cmd+T` | `NewPane` | New Pane |
| `close_panel` | `Escape+Escape` | `ClosePane` | Close Pane |
| `toggle_fullscreen` | `Cmd+F` | `ToggleFullscreen` | Toggle Fullscreen |
| `open_settings` | `Cmd+,` | `OpenSettings` | Open Settings |
| `open_chat` | `Cmd+J` | `OpenChat` | Open Chat |
| `focus_panel_1` | `Cmd+1` | `FocusPane(1)` | Focus Pane 1 |
| `focus_panel_2` | `Cmd+2` | `FocusPane(2)` | Focus Pane 2 |
| `focus_panel_3` | `Cmd+3` | `FocusPane(3)` | Focus Pane 3 |
| `focus_panel_4` | `Cmd+4` | `FocusPane(4)` | Focus Pane 4 |
| `focus_panel_5` | `Cmd+5` | `FocusPane(5)` | Focus Pane 5 |
| `cycle_panels` | `Tab` | `FocusNextPane` | Focus Next Pane |
| `cycle_panels_reverse` | `Shift+Tab` | `FocusPrevPane` | Focus Previous Pane |
| `split_vertical` | `Cmd+D` | `SplitVertical` | Split Vertical |
| `split_horizontal` | `Cmd+Shift+D` | `SplitHorizontal` | Split Horizontal |
| `close_pane` | `Cmd+W` | `ClosePane` | Close Pane |
| `command_palette` | `Cmd+Shift+P` | `OpenCommandPalette` | Command Palette |
| `copy` | `Cmd+C` | `Copy` | Copy |
| `paste` | `Cmd+V` | `Paste` | Paste |

> **Note:** Both `close_panel` (`Escape+Escape`) and `close_pane` (`Cmd+W`)
> resolve to `Action::ClosePane`.  The registry stores 20 mapping entries but
> two share the same action, yielding 19 unique `KeyCombo` keys (since
> `Escape+Escape` is parsed as a single `KeyCombo` with the key `Escape` and
> no modifiers -- it maps to the second `Escape` token as the key).

### Platform Display

Keybind display strings are platform-aware:

| Platform | Modifier Glyphs | Separator | Super Key Name |
|----------|----------------|-----------|----------------|
| macOS | `⌘` `⌃` `⌥` `⇧` | None (concatenated) | `⌘` |
| Windows | `Ctrl` `Alt` `Shift` `Win` | `+` | `Win` |
| Linux | `Ctrl` `Alt` `Shift` `Super` | `+` | `Super` |

Special key display on macOS: `Enter` = `↩`, `Backspace` = `⌫`, `Delete` = `⌦`,
`Escape` = `⎋`, `Tab` = `⇥`, `Space` = `␣`, arrow keys = `↑↓←→`.

**Source**: `jarvis-rs/crates/jarvis-platform/src/keymap/display.rs`

---

## Input Flow: End-to-End

The complete path of a keyboard event through the system:

```
winit WindowEvent::KeyboardInput { event: KeyEvent }
    |
    v
JarvisApp::handle_keyboard_input()          [event_handler.rs]
    |
    |-- Extract logical_key (Named or Character)
    |-- Normalize via normalize_winit_key()   [winit_keys.rs]
    |
    |-- [1] If command palette is open:
    |       handle_palette_key() -> true?  => DONE (consumed)
    |
    |-- [2] If assistant is open:
    |       handle_assistant_key() -> true? => DONE (consumed)
    |
    |-- [3] Build Modifiers from winit ModifiersState
    |
    |-- [4] InputProcessor::process_key()     [processor.rs]
    |       |
    |       |-- Build KeyCombo::from_winit()
    |       |
    |       |-- If key release:
    |       |     If combo matches PushToTalk => Action(ReleasePushToTalk)
    |       |     Otherwise => Consumed
    |       |
    |       |-- If key press:
    |       |     Check KeybindRegistry::lookup()
    |       |       Match found => Action(action)
    |       |       No match:
    |       |         If mode != Terminal => Consumed
    |       |         If mode == Terminal => encode_key_for_terminal()
    |       |           Non-empty bytes => TerminalInput(bytes)
    |       |           Empty => Consumed
    |
    v
InputResult:
    Action(action)      => JarvisApp::dispatch(action)
    TerminalInput(bytes) => (rarely fires; xterm.js handles typing natively)
    Consumed             => no-op
```

### Key Points

1. **Overlay priority**: The command palette and assistant get first crack at
   key events before the `InputProcessor` runs.  This ensures typed characters
   go into the palette filter or assistant input, not the terminal.

2. **Push-to-talk release**: The `InputProcessor` has special handling for key
   _releases_.  If the released key combo matches `PushToTalk` in the registry,
   it synthesizes `ReleasePushToTalk`.  All other releases are consumed.

3. **Terminal bypass**: In practice, terminal keystrokes rarely flow through
   `InputResult::TerminalInput` because the webview hosting xterm.js intercepts
   keyboard events at the DOM level before winit sees them.  The encoding path
   exists for correctness and for cases where the webview does not capture input.

4. **Modifier-only presses**: Keys like bare `Ctrl` or `Shift` do not produce a
   recognized key name and are consumed.

---

## The Command Palette

The command palette is a searchable overlay listing all available actions.  It
is implemented across two crates:

| Crate | Module | Responsibility |
|-------|--------|----------------|
| `jarvis-renderer` | `command_palette/` | Core data model: `CommandPalette`, `PaletteItem`, `PaletteMode`. Filtering, selection, and confirmation logic. |
| `jarvis-app` | `app_state/palette.rs` | Key handling, webview IPC bridge, plugin injection, overlay state management. |

### Opening the Palette

1. User presses the `command_palette` keybind (default `Cmd+Shift+P`).
2. The registry resolves to `Action::OpenCommandPalette`.
3. `dispatch()` creates a new `CommandPalette` from the registry, injects
   plugin items, sets `InputMode::CommandPalette`, and sends `palette_show`
   IPC to the focused webview.

### PaletteMode

The palette operates in one of two modes:

| Mode | Behavior |
|------|----------|
| `ActionSelect` | Default.  The user types to filter the action list.  Up/Down/Tab navigate.  Enter confirms the selected action. |
| `UrlInput` | The user types a URL.  No filtering occurs (the item list is empty).  Enter confirms and dispatches `OpenURL(typed_text)`.  Escape returns to `ActionSelect`. |

The `UrlInput` mode is entered when the user selects `OpenURLPrompt` in
`ActionSelect` mode.  `OpenURLPrompt` is a sentinel action that is _never_
dispatched to the app -- the palette key handler intercepts it and calls
`enter_url_mode()` instead.

### PaletteItem

Each item in the palette has:

```rust
pub struct PaletteItem {
    pub action: Action,           // The action this item triggers
    pub label: String,            // Human-readable display label
    pub keybind_display: Option<String>,  // e.g. "⌘T" or "Ctrl+T"
    pub category: String,         // Grouping category
}
```

### Filtering

Filtering is case-insensitive substring matching on the label:

```
query = "split"
  => matches "Split Horizontal", "Split Vertical"
  => does NOT match "New Pane"
```

When the query is empty, all items are shown.  On each character typed (or
deleted), the filter re-runs and the selection index resets to 0.

In `UrlInput` mode, typed characters are appended to the query but no filtering
occurs (the list stays empty).  Character case is preserved for URL input but
lowercased for action filtering.

### Key Handling in the Palette

| Key | ActionSelect Mode | UrlInput Mode |
|-----|-------------------|---------------|
| `Escape` | Close palette (`CloseOverlay`) | Return to `ActionSelect` (rebuilds palette) |
| `Enter` | Confirm selection (dispatch action; or enter URL mode if `OpenURLPrompt`) | Confirm URL (dispatch `OpenURL`) |
| `Up` | Select previous item (wraps) | -- |
| `Down` | Select next item (wraps) | -- |
| `Tab` | Select next item (wraps) | -- |
| `Backspace` | Delete last character from query | Delete last character from URL |
| Printable char | Append (lowercased) to query, re-filter | Append (as-is) to URL |

### Webview IPC Bridge

The palette state is sent to the focused pane's webview via IPC messages:

| IPC Message | When Sent |
|-------------|-----------|
| `palette_show` | When the palette first opens (includes full item list, query, selected index, mode). |
| `palette_update` | After every keystroke that changes the palette state. |
| `palette_hide` | When the palette closes. |

The IPC payload is a JSON object:

```json
{
    "items": [
        { "label": "New Pane", "keybind": "⌘T", "category": "Panes" },
        { "label": "Split Horizontal", "keybind": "⌘⇧D", "category": "Panes" }
    ],
    "query": "split",
    "selectedIndex": 0,
    "mode": "action_select",
    "placeholder": ""
}
```

For `UrlInput` mode, the `mode` field is `"url_input"` and `placeholder` is
`"Type a URL and press Enter"`.

### Overlay State Notification

When any overlay (palette or assistant) opens or closes, all webviews are
notified via `window.jarvis._setOverlayActive(true/false)`.  This tells the
JavaScript keybind interceptor to forward keys like `Cmd+V` to Rust instead of
handling them in the DOM.

**Source**: `jarvis-rs/crates/jarvis-app/src/app_state/palette.rs`

---

## Palette Categories

Actions are grouped into categories for visual organization.  The category is
determined by the `Action::category()` method:

| Category | Actions |
|----------|---------|
| **Panes** | `NewPane`, `ClosePane`, `SplitHorizontal`, `SplitVertical`, `FocusPane(_)`, `FocusNextPane`, `FocusPrevPane`, `ZoomPane`, `SwapPane(_)`, `ResizePane { .. }` |
| **Window** | `ToggleFullscreen`, `Quit` |
| **Apps** | `OpenSettings`, `OpenAssistant`, `OpenChat`, `OpenURLPrompt`, `OpenCommandPalette`, `CloseOverlay` |
| **Terminal** | `Copy`, `Paste`, `SelectAll`, `SearchOpen`, `SearchClose`, `SearchNext`, `SearchPrev`, `ScrollUp(_)`, `ScrollDown(_)`, `ScrollToTop`, `ScrollToBottom`, `ClearTerminal` |
| **Games** | `LaunchGame(_)`, `OpenURL(_)` (when URL matches game domains: kartbros, basketbros, footballbros, soccerbros, wrestlebros, baseballbros, lichess) |
| **Web** | `OpenURL(_)` (non-game URLs) |
| **System** | `PairMobile`, `RevokeMobilePairing`, `ReloadConfig`, `PushToTalk`, `ReleasePushToTalk`, `None` |

Plugin items (bookmarks and local plugins) use whatever category is specified
in their configuration.  The default category for bookmarks is `"Plugins"`.

**Source**: `jarvis-rs/crates/jarvis-common/src/actions/dispatch.rs`

---

## Plugin Items in the Palette

When the command palette opens, plugin items are injected via
`inject_plugin_items()`.  There are two sources:

### Bookmark Plugins

Defined in the TOML config under `[[plugins.bookmarks]]`:

```toml
[[plugins.bookmarks]]
name = "My Dashboard"
url = "https://example.com/dashboard"
category = "Tools"
```

Each bookmark becomes a `PaletteItem` with `action = OpenURL(url)` and no
keybind display.

**Source**: `jarvis-rs/crates/jarvis-config/src/schema/plugins.rs`

### Local Plugins

Discovered from the filesystem and registered at startup.  Each local plugin
has an `id`, `name`, `category`, and `entry` HTML file.

The palette item's action URL is constructed as:
```
jarvis://localhost/plugins/{id}/{entry}
```

For example, a plugin with `id = "calculator"` and `entry = "index.html"`
would produce `jarvis://localhost/plugins/calculator/index.html`.

### Injection Mechanics

`inject_plugin_items()` builds a `Vec<PaletteItem>` from both sources and
calls `CommandPalette::add_items()`, which appends the items to the internal
list and re-runs the filter.  Items with empty `name` or `url` fields are
skipped.

**Source**: `jarvis-rs/crates/jarvis-app/src/app_state/palette.rs`

---

## Full Dispatch Table

The `dispatch()` method in `JarvisApp` routes each `Action` to the appropriate
subsystem.  The table below documents every case:

| Action | Subsystem | Behavior |
|--------|-----------|----------|
| `NewPane` | Tiling + WebView | Check `max_panels` limit. Auto-pick split direction from content rect. Split, create webview, sync bounds. |
| `ClosePane` | Tiling + WebView | Close focused pane, destroy its webview, sync bounds. |
| `SplitHorizontal` | Tiling + WebView | Execute `TilingCommand::SplitHorizontal`, create webview for new pane, sync bounds. |
| `SplitVertical` | Tiling + WebView | Execute `TilingCommand::SplitVertical`, create webview for new pane, sync bounds. |
| `FocusPane(n)` | Tiling | Focus pane by ID, notify focus change. |
| `FocusNextPane` | Tiling | Execute `TilingCommand::FocusNext`, notify focus change. |
| `FocusPrevPane` | Tiling | Execute `TilingCommand::FocusPrev`, notify focus change. |
| `ZoomPane` | Tiling | Execute `TilingCommand::Zoom`, sync webview bounds. |
| `ResizePane { direction, delta }` | Tiling | Map `ResizeDirection` to `Direction` and signed delta. Execute `TilingCommand::Resize`. Sync bounds. |
| `SwapPane(direction)` | Tiling | Map direction, execute `TilingCommand::Swap`. Sync bounds. |
| `ToggleFullscreen` | Window (winit) | Toggle between borderless fullscreen and windowed mode. |
| `Quit` | Event Bus + Shutdown | Publish `Event::Shutdown`, call `shutdown()`, set `should_exit = true`. |
| `OpenCommandPalette` | Palette + Input | Create `CommandPalette`, inject plugins, set `CommandPalette` mode, send `palette_show` IPC, notify overlay state. |
| `OpenAssistant` | Assistant + Input | Toggle: if open, close and return to `Terminal` mode. If closed, create `AssistantPanel`, set `Assistant` mode, start runtime. |
| `CloseOverlay` | Palette / Assistant | Close whichever is open. If assistant, clear panel. If palette, send `palette_hide`. Return to `Terminal` mode. |
| `OpenSettings` | Tiling + WebView | Set `Settings` mode. Split horizontally with `PaneKind::WebView`, load `jarvis://localhost/settings/index.html`. |
| `OpenChat` | Tiling + WebView | Split horizontally with `PaneKind::Chat`, load `jarvis://localhost/chat/index.html`. |
| `Copy` | WebView (JS eval) | Evaluate JS in the focused webview to grab the xterm.js selection (or DOM selection) and send it back via `clipboard_copy` IPC. |
| `Paste` | Clipboard + WebView | Read system clipboard via `jarvis_platform::Clipboard`. Evaluate JS to paste into the focused element (input/textarea) or send via `pty_input` IPC for terminal paste. |
| `LaunchGame(name)` | WebView | Store the pane's current URL in `game_active`. Navigate to `jarvis://localhost/games/{name}.html`. |
| `OpenURL(url)` | WebView | Normalize URL (prepend `https://` if missing). Store current URL in `game_active`. Navigate to the URL. |
| `PairMobile` | Mobile | Call `show_pair_code()`. |
| `RevokeMobilePairing` | Mobile | Call `revoke_mobile_pairing()`. Log confirmation. |
| `ReloadConfig` | Config + Registry + Chrome | Reload TOML config. Rebuild `KeybindRegistry`. Rebuild `UiChrome`. Re-register plugin directories. Re-inject theme into all webviews. Publish `Event::ConfigReloaded`. On error, push a `Notification::error`. |
| `ClearTerminal` | WebView (JS eval) | Evaluate `window._xtermInstance.clear()` in the focused webview. |
| `ScrollUp(_)` / `ScrollDown(_)` | WebView | Logged as debug; handled natively by the webview's xterm.js. |
| All other / `None` | -- | Logged as debug ("unhandled action"). |

After every dispatch, `update_window_title()` is called to reflect any state
changes in the window title.

**Source**: `jarvis-rs/crates/jarvis-app/src/app_state/dispatch.rs`

---

## Mouse Input Handling

Mouse events are handled by `JarvisApp` in the winit event handler.

### Cursor Movement (`CursorMoved`)

`handle_cursor_moved(x, y)` serves two purposes:

1. **During drag**: If `drag_state` is active, compute the ratio delta from the
   cursor position and adjust the split ratio between the two panes straddling
   the dragged border via `tree.adjust_ratio_between()`.  On macOS, vertical
   deltas are negated to account for AppKit's upward-increasing Y axis.  After
   adjustment, sync webview bounds and request redraw.

2. **Idle hover**: When not dragging, compute all inter-pane borders, check if
   the cursor is near one, and set the cursor icon accordingly:
   - Horizontal border: `CursorIcon::ColResize`
   - Vertical border: `CursorIcon::RowResize`
   - No border: `CursorIcon::Default`

### Mouse Button (`MouseInput`)

Only `MouseButton::Left` is handled.

**On press:**

1. **Overlay forwarding**: If the command palette or assistant is open, the
   click is forwarded into the focused pane's webview via
   `document.elementFromPoint()` + `.click()`.  This allows clicking palette
   items rendered in the DOM overlay.

2. **Border drag**: If the click lands on a resize border, start a `DragState`
   recording the border and the starting position.

3. **Tab bar click**: If the click is in the titlebar area (top N pixels), check
   if it lands on a tab.  If so, focus that pane.

4. **Window drag**: If the click is in the titlebar but not on a tab, or in the
   chrome gap area between panes, initiate a window drag (`w.drag_window()`).

5. **Pane focus**: If the click is inside a pane, focus that pane and give
   native focus to its webview for keyboard input.

**On release:**

If a drag is active, clear `drag_state` and reset the cursor to `Default`.

### Modifier State (`ModifiersChanged`)

The `ModifiersState` from winit is stored on `self.modifiers` and read when
building `Modifiers` for `InputProcessor::process_key()`.

**Source**: `jarvis-rs/crates/jarvis-app/src/app_state/event_handler.rs`

---

## Modifier Key Tracking

Modifier state is tracked via the `WindowEvent::ModifiersChanged` event from
winit.  The raw `ModifiersState` is stored on `JarvisApp.modifiers` and
queried each time a `KeyboardInput` event fires:

```rust
let mods = Modifiers {
    ctrl: self.modifiers.control_key(),
    alt: self.modifiers.alt_key(),
    shift: self.modifiers.shift_key(),
    super_key: self.modifiers.super_key(),
};
```

These booleans are passed to `InputProcessor::process_key()`, which constructs
a `KeyCombo::from_winit()` with the modifier bitmask for registry lookup.

The `Modifiers` struct is a simple bundle:

```rust
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
}
```

There is no debouncing or synthetic modifier tracking -- winit delivers
accurate modifier state per-event.

**Source**: `jarvis-rs/crates/jarvis-platform/src/input_processor/types.rs`

---

## Terminal Key Encoding

When a key press does not match any keybind and the input mode is `Terminal`,
the `InputProcessor` encodes the key into terminal escape sequences via
`encode_key_for_terminal()`.

### Encoding Table

| Key | No Modifier | Ctrl | Alt |
|-----|-------------|------|-----|
| `Enter` | `\r` | `\r` | `ESC \r` |
| `Backspace` | `\x7f` (DEL) | `\x7f` | `ESC \x7f` |
| `Tab` | `\t` | `\t` | `\t` |
| `Escape` | `\x1b` | `\x1b` | `\x1b` |
| `Space` | `" "` | `" "` | `ESC " "` |
| `Delete` | `ESC[3~` | -- | -- |
| `Insert` | `ESC[2~` | -- | -- |
| `Up` | `ESC[A` | -- | -- |
| `Down` | `ESC[B` | -- | -- |
| `Right` | `ESC[C` | -- | -- |
| `Left` | `ESC[D` | -- | -- |
| `Home` | `ESC[H` | -- | -- |
| `End` | `ESC[F` | -- | -- |
| `PageUp` | `ESC[5~` | -- | -- |
| `PageDown` | `ESC[6~` | -- | -- |
| `F1`-`F4` | `ESC O P/Q/R/S` | -- | -- |
| `F5`-`F12` | `ESC[15~` through `ESC[24~` | -- | -- |
| `A`-`Z` (letter) | Letter byte | Ctrl byte (1-26) | `ESC` + letter byte |
| `[` | `[` | `ESC` (0x1b) | -- |
| `\` | `\` | 0x1c (FS) | -- |
| `]` | `]` | 0x1d (GS) | -- |
| Other single char | Char byte | -- | `ESC` + char byte |

### Bracketed Paste

The `InputProcessor` supports bracketed paste mode.  When enabled
(`set_bracketed_paste(true)`), pasted text is wrapped in DCS markers:

```
\x1b[200~  <pasted text>  \x1b[201~
```

This allows terminal applications to distinguish pasted text from typed input.

**Source**: `jarvis-rs/crates/jarvis-platform/src/input_processor/encoding.rs`

---

## Palette Actions List

The following actions appear in the command palette (in order). This list is
returned by `Action::palette_actions()`:

| # | Action | Label | Category |
|---|--------|-------|----------|
| 1 | `NewPane` | New Pane | Panes |
| 2 | `ClosePane` | Close Pane | Panes |
| 3 | `SplitHorizontal` | Split Horizontal | Panes |
| 4 | `SplitVertical` | Split Vertical | Panes |
| 5 | `ToggleFullscreen` | Toggle Fullscreen | Window |
| 6 | `OpenSettings` | Open Settings | Apps |
| 7 | `OpenChat` | Open Chat | Apps |
| 8 | `OpenURLPrompt` | Open URL | Apps |
| 9 | `Copy` | Copy | Terminal |
| 10 | `Paste` | Paste | Terminal |
| 11 | `SelectAll` | Select All | Terminal |
| 12 | `ScrollToTop` | Scroll to Top | Terminal |
| 13 | `ScrollToBottom` | Scroll to Bottom | Terminal |
| 14 | `ClearTerminal` | Clear Terminal | Terminal |
| 15 | `LaunchGame("tetris")` | Play Tetris | Games |
| 16 | `LaunchGame("asteroids")` | Play Asteroids | Games |
| 17 | `LaunchGame("minesweeper")` | Play Minesweeper | Games |
| 18 | `LaunchGame("pinball")` | Play Pinball | Games |
| 19 | `LaunchGame("doodlejump")` | Play Doodle Jump | Games |
| 20 | `LaunchGame("draw")` | Open Draw | Games |
| 21 | `LaunchGame("subway")` | Play Subway Surfers | Games |
| 22 | `OpenURL("https://kartbros.io")` | Play KartBros | Games |
| 23 | `OpenURL("https://basketbros.io")` | Play Basket Bros | Games |
| 24 | `OpenURL("https://footballbros.io")` | Play Football Bros | Games |
| 25 | `OpenURL("https://soccerbros.gg")` | Play Soccer Bros | Games |
| 26 | `OpenURL("https://wrestlebros.io")` | Play Wrestle Bros | Games |
| 27 | `OpenURL("https://baseballbros.io")` | Play Baseball Bros | Games |
| 28 | `OpenURL("https://lichess.org")` | Play Lichess | Games |
| 29 | `OpenURL("https://monkeytype.com")` | Open Monkeytype | Web |
| 30 | `OpenURL("https://excalidraw.com")` | Open Excalidraw | Web |
| 31 | `OpenURL("https://www.desmos.com/calculator")` | Open Desmos | Web |
| 32 | `OpenURL("https://news.ycombinator.com")` | Open Hacker News | Web |
| 33 | `OpenURL("https://open.spotify.com")` | Open Spotify | Web |
| 34 | `PairMobile` | Pair Mobile Device | System |
| 35 | `RevokeMobilePairing` | Revoke Mobile Pairing | System |
| 36 | `ReloadConfig` | Reload Config | System |
| 37 | `Quit` | Quit | Window |

In addition, bookmark and local plugin items are appended dynamically at
runtime (see [Plugin Items in the Palette](#plugin-items-in-the-palette)).

**Source**: `jarvis-rs/crates/jarvis-common/src/actions/dispatch.rs`
