# Configuration Reference

Jarvis uses a single TOML configuration file with sensible defaults for every field.
You only need to override the values you want to change -- missing fields are
automatically filled with their defaults.

The configuration schema version is **1** (`CONFIG_SCHEMA_VERSION = 1`).

---

## Table of Contents

- [Config File Location](#config-file-location)
- [How Configuration Loading Works](#how-configuration-loading-works)
- [Section Reference](#section-reference)
  - [\[theme\]](#theme)
  - [\[colors\]](#colors)
  - [\[font\]](#font)
  - [\[terminal\]](#terminal)
  - [\[shell\]](#shell)
  - [\[window\]](#window)
  - [\[layout\]](#layout)
  - [\[opacity\]](#opacity)
  - [\[background\]](#background)
  - [\[effects\]](#effects)
  - [\[visualizer\]](#visualizer)
  - [\[startup\]](#startup)
  - [\[voice\]](#voice)
  - [\[keybinds\]](#keybinds)
  - [\[panels\]](#panels)
  - [\[games\]](#games)
  - [\[livechat\]](#livechat)
  - [\[presence\]](#presence)
  - [\[performance\]](#performance)
  - [\[updates\]](#updates)
  - [\[logging\]](#logging)
  - [\[advanced\]](#advanced)
  - [\[auto\_open\]](#auto_open)
  - [\[status\_bar\]](#status_bar)
  - [\[relay\]](#relay)
  - [\[plugins\]](#plugins)
- [Theme System](#theme-system)
  - [Built-in Themes](#built-in-themes)
  - [Custom Themes](#custom-themes)
  - [Theme File Format](#theme-file-format)
  - [Theme Search Paths](#theme-search-paths)
- [Config Validation Rules](#config-validation-rules)
- [Hot Reload](#hot-reload)
  - [How It Works](#how-it-works)
  - [What Triggers a Reload](#what-triggers-a-reload)
  - [Debouncing](#debouncing)
- [Config Saving](#config-saving)
- [Example Configurations](#example-configurations)

---

## Config File Location

Jarvis resolves its configuration file using the platform's standard config
directory (via the `dirs` crate), under the `jarvis/` subfolder:

| Platform | Path |
|----------|------|
| **macOS** | `~/Library/Application Support/jarvis/config.toml` |
| **Linux** | `~/.config/jarvis/config.toml` |
| **Windows** | `C:\Users\<USER>\AppData\Roaming\jarvis\config.toml` |

If the file does not exist when Jarvis starts, it creates a default
`config.toml` populated with commented-out documentation for every section.

---

## How Configuration Loading Works

1. **Load TOML** -- The `config.toml` file is read and deserialized. All
   fields use `#[serde(default)]`, so a partial config (or even an empty
   file) works correctly.
2. **Apply theme** -- If `theme.name` is anything other than `"jarvis-dark"`,
   the theme file is located and its overrides are merged into the config.
3. **Discover plugins** -- The `~/.config/jarvis/plugins/` directory is
   scanned for local plugin folders containing a `plugin.toml` manifest.
4. **Validate** -- All numeric ranges, keybind uniqueness, and other
   constraints are checked. If validation fails, an error is returned.

---

## Section Reference

### \[theme\]

Theme selection. Set `name` to a built-in theme name or a path to a custom
theme file (YAML or TOML).

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | `"jarvis-dark"` | Built-in theme name or path to custom theme file |

```toml
[theme]
name = "dracula"
```

---

### \[colors\]

Full color palette. All values are CSS color strings (hex `#RRGGBB` or
`rgba(r,g,b,a)` format). The defaults use the Catppuccin Mocha palette.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `primary` | string | `"#cba6f7"` | Primary accent color (Mauve) |
| `secondary` | string | `"#f5c2e7"` | Secondary accent color (Pink) |
| `background` | string | `"#1e1e2e"` | Main background color (Base) |
| `panel_bg` | string | `"rgba(30,30,46,0.88)"` | Panel background with alpha |
| `text` | string | `"#cdd6f4"` | Primary text color |
| `text_muted` | string | `"#6c7086"` | Muted/secondary text color (Overlay0) |
| `border` | string | `"#181825"` | Border color (Mantle) |
| `border_focused` | string | `"rgba(203,166,247,0.15)"` | Focused panel border glow |
| `user_text` | string | `"rgba(137,180,250,0.85)"` | User message text color (Blue) |
| `tool_read` | string | `"rgba(137,180,250,0.9)"` | Read tool indicator (Blue) |
| `tool_edit` | string | `"rgba(249,226,175,0.9)"` | Edit tool indicator (Yellow) |
| `tool_write` | string | `"rgba(250,179,135,0.9)"` | Write tool indicator (Peach) |
| `tool_run` | string | `"rgba(166,227,161,0.9)"` | Run tool indicator (Green) |
| `tool_search` | string | `"rgba(203,166,247,0.9)"` | Search tool indicator (Mauve) |
| `success` | string | `"#a6e3a1"` | Success state color (Green) |
| `warning` | string | `"#f9e2af"` | Warning state color (Yellow) |
| `error` | string | `"#f38ba8"` | Error state color (Red) |

```toml
[colors]
primary = "#ff6b6b"
background = "#0d1117"
text = "#e6edf3"
```

---

### \[font\]

Typography configuration for both terminal/code text and UI elements.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `family` | string | `"Menlo"` | -- | Terminal/code font family |
| `size` | u32 | `13` | 8--32 | Font size in points |
| `title_size` | u32 | `14` | 8--48 | Title font size in points |
| `line_height` | f64 | `1.6` | 1.0--3.0 | Line height multiplier |
| `bold_family` | string? | `null` | -- | Override font family for bold text |
| `italic_family` | string? | `null` | -- | Override font family for italic text |
| `nerd_font` | bool | `true` | -- | Enable Nerd Font glyph rendering |
| `ligatures` | bool | `false` | -- | Enable font ligatures (e.g. `->`, `=>`) |
| `fallback_families` | string[] | `[]` | -- | Fallback fonts tried when glyphs are missing |
| `font_weight` | u32 | `400` | 100--900 | Font weight for normal text |
| `bold_weight` | u32 | `700` | 100--900 | Font weight for bold text |
| `ui_family` | string | `"-apple-system, BlinkMacSystemFont, 'Inter', 'Segoe UI', sans-serif"` | -- | UI font family (headers, labels, buttons) |
| `ui_size` | u32 | `13` | 10--24 | UI font size in points |

```toml
[font]
family = "JetBrains Mono"
size = 14
ligatures = true
nerd_font = true
fallback_families = ["Symbols Nerd Font Mono", "Apple Color Emoji"]
font_weight = 300
```

---

### \[terminal\]

Terminal emulator settings: scrollback, cursor, bell, mouse, and search.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `scrollback_lines` | u32 | `10000` | 0--100,000 | Number of scrollback lines |
| `cursor_style` | enum | `"block"` | `block`, `underline`, `beam` | Cursor visual style |
| `cursor_blink` | bool | `true` | -- | Enable cursor blinking |
| `cursor_blink_interval_ms` | u32 | `500` | 100--2000 | Blink interval in milliseconds |
| `word_separators` | string | `` /\()\"'-.,:;<>~!@#$%^&*\|+=[]{}~?`` | -- | Word boundary characters for double-click selection |
| `true_color` | bool | `true` | -- | Enable 24-bit true color |

#### \[terminal.bell\]

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `visual` | bool | `true` | -- | Enable visual bell flash |
| `audio` | bool | `false` | -- | Enable audio bell |
| `duration_ms` | u32 | `150` | 50--1000 | Visual bell flash duration in ms |

#### \[terminal.mouse\]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `copy_on_select` | bool | `false` | Copy selection to clipboard automatically |
| `url_detection` | bool | `true` | Detect and highlight clickable URLs |
| `click_to_focus` | bool | `true` | Click on a pane to give it focus |

#### \[terminal.search\]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `wrap_around` | bool | `true` | Wrap search to beginning when reaching end |
| `regex` | bool | `false` | Use regex patterns in search |
| `case_sensitive` | bool | `false` | Case-sensitive search by default |

```toml
[terminal]
scrollback_lines = 50000
cursor_style = "beam"
cursor_blink = false

[terminal.bell]
audio = true

[terminal.mouse]
copy_on_select = true
```

---

### \[shell\]

Shell process configuration. Controls which shell to launch.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `program` | string | `""` (auto-detect from `$SHELL`) | Shell program path |
| `args` | string[] | `[]` | Extra arguments passed to the shell |
| `working_directory` | string? | `null` (inherit from parent) | Initial working directory |
| `env` | map | `{}` | Extra environment variables |
| `login_shell` | bool | `true` | Launch as login shell (prepend `-` to argv[0]) |

```toml
[shell]
program = "/bin/zsh"
args = ["-l"]
login_shell = true

[shell.env]
TERM = "xterm-256color"
EDITOR = "nvim"
```

---

### \[window\]

Window appearance and behavior.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `decorations` | enum | `"full"` | `full`, `none`, `transparent` | Window decoration style |
| `opacity` | f64 | `1.0` | 0.0--1.0 | Window-level opacity |
| `blur` | bool | `false` | -- | Enable macOS vibrancy / background blur |
| `startup_mode` | enum | `"windowed"` | `windowed`, `maximized`, `fullscreen` | Window startup mode |
| `title` | string | `"Jarvis"` | -- | Static window title |
| `dynamic_title` | bool | `true` | -- | Update title bar with shell-reported title |
| `titlebar_height` | u32 | `38` | -- | Custom titlebar height in pixels (macOS). 0 = system default |

#### \[window.padding\]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `top` | u32 | `0` | Top padding in pixels |
| `right` | u32 | `0` | Right padding in pixels |
| `bottom` | u32 | `0` | Bottom padding in pixels |
| `left` | u32 | `0` | Left padding in pixels |

```toml
[window]
decorations = "transparent"
opacity = 0.9
blur = true
startup_mode = "maximized"
title = "My Terminal"

[window.padding]
top = 4
bottom = 4
left = 8
right = 8
```

---

### \[layout\]

Panel layout geometry.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `panel_gap` | u32 | `6` | 1--20 | Gap between panels in pixels |
| `border_radius` | u32 | `8` | 0--20 | Border radius in pixels |
| `padding` | u32 | `10` | 0--40 | Padding inside panels in pixels |
| `max_panels` | u32 | `5` | 1--10 | Maximum number of panels |
| `default_panel_width` | f64 | `0.72` | 0.3--1.0 | Default panel width as fraction of screen |
| `scrollbar_width` | u32 | `3` | 1--10 | Scrollbar width in pixels |
| `border_width` | f64 | `0.0` | 0.0--3.0 | Panel border width in pixels |
| `outer_padding` | u32 | `0` | 0--40 | Screen-edge padding in pixels |
| `inactive_opacity` | f64 | `1.0` | 0.0--1.0 | Opacity for unfocused panels |

```toml
[layout]
panel_gap = 8
border_radius = 12
padding = 12
max_panels = 3
default_panel_width = 0.5
border_width = 1.0
outer_padding = 10
```

---

### \[opacity\]

Transparency settings. All values are floats in the range 0.0 (fully
transparent) to 1.0 (fully opaque).

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `background` | f64 | `1.0` | 0.0--1.0 | Background layer opacity |
| `panel` | f64 | `0.85` | 0.0--1.0 | Panel opacity |
| `orb` | f64 | `1.0` | 0.0--1.0 | Orb visualizer opacity |
| `hex_grid` | f64 | `0.8` | 0.0--1.0 | Hex grid background opacity |
| `hud` | f64 | `1.0` | 0.0--1.0 | HUD overlay opacity |

```toml
[opacity]
background = 0.95
panel = 0.9
hex_grid = 0.5
```

---

### \[background\]

Background display system. Supports multiple modes with per-mode settings.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | enum | `"hex_grid"` | Background mode: `hex_grid`, `solid`, `image`, `video`, `gradient`, `none` |
| `solid_color` | string | `"#000000"` | Solid color when `mode = "solid"` |

#### \[background.hex\_grid\]

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `color` | string | `"#00d4ff"` | -- | Hex grid line color |
| `opacity` | f64 | `0.08` | 0.0--1.0 | Hex grid opacity |
| `animation_speed` | f64 | `1.0` | 0.0--5.0 | Animation speed multiplier |
| `glow_intensity` | f64 | `0.5` | 0.0--1.0 | Grid line glow intensity |

#### \[background.image\]

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `path` | string | `""` | -- | Path to background image |
| `fit` | enum | `"cover"` | `cover`, `contain`, `fill`, `tile` | Image fit mode |
| `blur` | u32 | `0` | 0--50 | Blur radius for background image |
| `opacity` | f64 | `1.0` | 0.0--1.0 | Background image opacity |

#### \[background.video\]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `path` | string | `""` | Path to background video |
| `loop` | bool | `true` | Loop the video |
| `muted` | bool | `true` | Mute the video |
| `fit` | enum | `"cover"` | Video fit: `cover`, `contain`, `fill` |

#### \[background.gradient\]

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `type` | enum | `"radial"` | `linear`, `radial` | Gradient type |
| `colors` | string[] | `["#000000", "#0a1520"]` | -- | Gradient color stops |
| `angle` | u32 | `180` | 0--360 | Gradient angle (for linear gradients) |

```toml
[background]
mode = "gradient"

[background.gradient]
type = "linear"
colors = ["#0d1117", "#161b22", "#0d1117"]
angle = 135
```

---

### \[effects\]

Post-processing visual effects. Set `enabled = false` to disable all effects
at once regardless of individual sub-effect settings.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `enabled` | bool | `true` | -- | Master toggle for all effects |
| `inactive_pane_dim` | bool | `true` | -- | Dim inactive (unfocused) panes |
| `dim_opacity` | f32 | `0.6` | 0.0--1.0 | Opacity multiplier for inactive panes |
| `crt_curvature` | bool | `false` | -- | CRT barrel distortion (currently no-op) |
| `blur_radius` | u32 | `12` | 0--40 | Backdrop blur radius for glassmorphic panels |
| `saturate` | f64 | `1.1` | 0.0--2.0 | Backdrop saturate multiplier |
| `transition_speed` | u32 | `150` | 0--500 | CSS transition speed in milliseconds |

#### \[effects.scanlines\]

CRT-style scanline overlay.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `enabled` | bool | `true` | -- | Enable scanlines |
| `intensity` | f32 | `0.08` | 0.0--1.0 | Scanline darkness intensity |

#### \[effects.vignette\]

Screen-edge darkening.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `enabled` | bool | `true` | -- | Enable vignette |
| `intensity` | f32 | `1.2` | 0.0--3.0 | Vignette strength |

#### \[effects.bloom\]

Light bleed / bloom effect.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `enabled` | bool | `true` | -- | Enable bloom |
| `intensity` | f32 | `0.9` | 0.0--3.0 | Bloom brightness multiplier |
| `passes` | u32 | `2` | 1--5 | Number of blur passes (more = smoother) |

#### \[effects.glow\]

Glow effect around the active pane.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `enabled` | bool | `true` | -- | Enable glow |
| `color` | string | `"#cba6f7"` | -- | Glow color (hex) |
| `width` | f32 | `2.0` | 0.0--10.0 | Glow width in pixels |
| `intensity` | f64 | `0.0` | 0.0--1.0 | Focus glow intensity for CSS box-shadow |

#### \[effects.flicker\]

Brightness flicker effect.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `enabled` | bool | `true` | -- | Enable flicker |
| `amplitude` | f32 | `0.004` | 0.0--0.05 | Flicker amplitude |

```toml
[effects]
enabled = true
inactive_pane_dim = false
blur_radius = 20
saturate = 1.3

[effects.scanlines]
intensity = 0.12

[effects.bloom]
intensity = 1.5
passes = 3

[effects.glow]
color = "#ff6b00"
width = 4.0
intensity = 0.3

[effects.flicker]
enabled = false
```

---

### \[visualizer\]

The central visualizer system. Supports orb, image, video, particle, and
waveform modes, each with their own sub-configuration. Per-state overrides
let the visualizer react to what Jarvis is doing.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `enabled` | bool | `true` | -- | Enable the visualizer |
| `type` | enum | `"orb"` | `orb`, `image`, `video`, `particle`, `waveform`, `none` | Visualizer type |
| `position_x` | f64 | `0.0` | -1.0--1.0 | Horizontal position offset |
| `position_y` | f64 | `0.0` | -1.0--1.0 | Vertical position offset |
| `scale` | f64 | `1.0` | 0.1--3.0 | Size scale multiplier |
| `anchor` | enum | `"center"` | `center`, `top-left`, `top-right`, `bottom-left`, `bottom-right` | Anchor position |
| `react_to_audio` | bool | `true` | -- | React to audio input levels |
| `react_to_state` | bool | `true` | -- | React to Jarvis state changes |

#### \[visualizer.orb\]

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `color` | string | `"#00d4ff"` | -- | Primary orb color |
| `secondary_color` | string | `"#0088aa"` | -- | Secondary orb color |
| `intensity_base` | f64 | `1.0` | 0.0--3.0 | Base glow intensity |
| `bloom_intensity` | f64 | `1.0` | 0.0--3.0 | Bloom intensity |
| `rotation_speed` | f64 | `1.0` | 0.0--5.0 | Rotation speed multiplier |
| `mesh_detail` | enum | `"high"` | `low`, `medium`, `high` | Mesh detail level |
| `wireframe` | bool | `false` | -- | Render as wireframe |
| `inner_core` | bool | `true` | -- | Show inner core |
| `outer_shell` | bool | `true` | -- | Show outer shell |

#### \[visualizer.image\]

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `path` | string | `""` | -- | Path to visualizer image |
| `fit` | enum | `"contain"` | `contain`, `cover`, `fill` | Image fit mode |
| `opacity` | f64 | `1.0` | 0.0--1.0 | Image opacity |
| `animation` | enum | `"none"` | `none`, `pulse`, `rotate`, `bounce`, `float` | Animation style |
| `animation_speed` | f64 | `1.0` | 0.0--5.0 | Animation speed multiplier |

#### \[visualizer.video\]

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `path` | string | `""` | -- | Path to visualizer video |
| `loop` | bool | `true` | -- | Loop the video |
| `muted` | bool | `true` | -- | Mute the video |
| `fit` | enum | `"cover"` | `cover`, `contain`, `fill` | Video fit mode |
| `opacity` | f64 | `1.0` | 0.0--1.0 | Video opacity |
| `sync_to_audio` | bool | `false` | -- | Sync playback to audio input |

#### \[visualizer.particle\]

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `style` | enum | `"swirl"` | `swirl`, `fountain`, `fire`, `snow`, `stars`, `custom` | Particle effect style |
| `count` | u32 | `500` | 10--5000 | Number of particles |
| `color` | string | `"#00d4ff"` | -- | Particle color |
| `size` | f64 | `2.0` | 0.5--10.0 | Particle size |
| `speed` | f64 | `1.0` | 0.1--5.0 | Particle speed |
| `lifetime` | f64 | `3.0` | 0.5--10.0 | Particle lifetime in seconds |
| `custom_shader` | string | `""` | -- | Path to custom shader |

#### \[visualizer.waveform\]

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `style` | enum | `"bars"` | `bars`, `line`, `circular`, `mirror` | Waveform display style |
| `color` | string | `"#00d4ff"` | -- | Waveform color |
| `bar_count` | u32 | `64` | 8--256 | Number of frequency bars |
| `bar_width` | f64 | `3.0` | 1.0--10.0 | Bar width in pixels |
| `bar_gap` | f64 | `2.0` | 0.0--10.0 | Gap between bars in pixels |
| `height` | u32 | `100` | 20--500 | Waveform height in pixels |
| `smoothing` | f64 | `0.8` | 0.0--1.0 | Frequency smoothing factor |

#### Per-State Overrides

Each state section lets you override the visualizer appearance when Jarvis
enters that state. All fields are optional.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `scale` | f64 | varies | 0.1--3.0 | Scale override |
| `intensity` | f64 | varies | 0.0--3.0 | Intensity override |
| `color` | string? | varies | -- | Color override (hex) |
| `position_x` | f64? | varies | -1.0--1.0 | X position override |
| `position_y` | f64? | varies | -1.0--1.0 | Y position override |

**\[visualizer.state\_listening\]** (default state):
- `scale = 1.0`, `intensity = 1.0`

**\[visualizer.state\_speaking\]**:
- `scale = 1.1`, `intensity = 1.4`

**\[visualizer.state\_skill\]**:
- `scale = 0.9`, `intensity = 1.2`, `color = "#ffaa00"`

**\[visualizer.state\_chat\]**:
- `scale = 0.55`, `intensity = 1.3`, `position_x = 0.10`, `position_y = 0.30`

**\[visualizer.state\_idle\]**:
- `scale = 0.8`, `intensity = 0.6`, `color = "#444444"`

```toml
[visualizer]
enabled = true
type = "orb"
anchor = "center"
scale = 1.2

[visualizer.orb]
color = "#ff00ff"
rotation_speed = 1.5
mesh_detail = "high"

[visualizer.state_speaking]
scale = 1.3
intensity = 1.6
color = "#00ff88"
```

---

### \[startup\]

Startup sequence configuration.

#### \[startup.boot\_animation\]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable boot animation |
| `duration` | f64 | `4.5` | Animation duration in seconds |
| `skip_on_key` | bool | `true` | Skip animation on any keypress |
| `music_enabled` | bool | `true` | Play boot music |
| `voiceover_enabled` | bool | `true` | Play voiceover during boot |

#### \[startup.fast\_start\]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable fast-start mode (skips boot animation) |
| `delay` | f64 | `0.5` | Delay before showing UI in seconds |

#### \[startup.on\_ready\]

What to show after boot animation completes or is skipped.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `action` | enum | `"listening"` | Post-boot action: `listening`, `panels`, `chat`, `game`, `skill` |

**\[startup.on\_ready.panels\]** (when `action = "panels"`):

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `count` | u32 | `1` | 1--5 | Number of panels to open |
| `titles` | string[] | `["Bench 1"]` | -- | Panel titles |
| `auto_create` | bool | `true` | -- | Automatically create panels |

**\[startup.on\_ready.chat\]** (when `action = "chat"`):

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `room` | string | `"general"` | Chat room to join |

**\[startup.on\_ready.game\]** (when `action = "game"`):

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | `"wordle"` | Game to launch |

**\[startup.on\_ready.skill\]** (when `action = "skill"`):

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | `"code_assistant"` | Skill to activate |

```toml
[startup.boot_animation]
enabled = false

[startup.fast_start]
enabled = true
delay = 0.2

[startup.on_ready]
action = "panels"

[startup.on_ready.panels]
count = 2
titles = ["Code", "Shell"]
```

---

### \[voice\]

Voice input and audio configuration.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable voice input |
| `mode` | enum | `"ptt"` | Input mode: `ptt` (push-to-talk), `vad` (voice-activity detection) |
| `input_device` | string | `"default"` | Audio input device name |
| `sample_rate` | u32 | `24000` | Output sample rate in Hz |
| `whisper_sample_rate` | u32 | `16000` | Whisper transcription sample rate in Hz |

#### \[voice.ptt\]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `key` | string | `"Option+Period"` | Push-to-talk key combination |
| `cooldown` | f64 | `0.3` | Cooldown between activations in seconds |

#### \[voice.vad\]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `silence_threshold` | f64 | `1.0` | Silence detection threshold in seconds |
| `energy_threshold` | u32 | `300` | Audio energy threshold for voice detection |

#### \[voice.sounds\]

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `enabled` | bool | `true` | -- | Enable feedback sounds |
| `volume` | f64 | `0.5` | 0.0--1.0 | Feedback sound volume |
| `listen_start` | bool | `true` | -- | Play sound when listening starts |
| `listen_end` | bool | `true` | -- | Play sound when listening ends |

```toml
[voice]
enabled = true
mode = "vad"

[voice.vad]
silence_threshold = 0.8
energy_threshold = 250

[voice.sounds]
volume = 0.3
```

---

### \[keybinds\]

Keyboard shortcuts. Format: `"Modifier+Key"` where Modifier is `Cmd`,
`Option`, `Control`, or `Shift`. Multiple modifiers: `"Cmd+Shift+G"`.
Double press: `"Escape+Escape"`.

Keybinds are validated for uniqueness -- duplicate bindings will produce a
validation error.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `push_to_talk` | string | `"Option+Period"` | Push-to-talk activation |
| `open_assistant` | string | `"Cmd+G"` | Open assistant panel |
| `new_panel` | string | `"Cmd+T"` | Open new panel |
| `close_panel` | string | `"Escape+Escape"` | Close current panel (double press) |
| `toggle_fullscreen` | string | `"Cmd+F"` | Toggle fullscreen mode |
| `open_settings` | string | `"Cmd+,"` | Open settings |
| `open_chat` | string | `"Cmd+J"` | Open chat |
| `focus_panel_1` | string | `"Cmd+1"` | Focus panel 1 |
| `focus_panel_2` | string | `"Cmd+2"` | Focus panel 2 |
| `focus_panel_3` | string | `"Cmd+3"` | Focus panel 3 |
| `focus_panel_4` | string | `"Cmd+4"` | Focus panel 4 |
| `focus_panel_5` | string | `"Cmd+5"` | Focus panel 5 |
| `cycle_panels` | string | `"Tab"` | Cycle panels forward |
| `cycle_panels_reverse` | string | `"Shift+Tab"` | Cycle panels backward |
| `split_vertical` | string | `"Cmd+D"` | Split pane vertically |
| `split_horizontal` | string | `"Cmd+Shift+D"` | Split pane horizontally |
| `close_pane` | string | `"Cmd+W"` | Close current pane |
| `command_palette` | string | `"Cmd+Shift+P"` | Open command palette |
| `copy` | string | `"Cmd+C"` | Copy selection |
| `paste` | string | `"Cmd+V"` | Paste from clipboard |

```toml
[keybinds]
push_to_talk = "Control+Space"
new_panel = "Cmd+N"
close_panel = "Cmd+W"
toggle_fullscreen = "Cmd+Enter"
```

---

### \[panels\]

Panel behavior settings for history, input, and focus.

#### \[panels.history\]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable message history persistence |
| `max_messages` | u32 | `1000` | Maximum messages stored in history |
| `restore_on_launch` | bool | `true` | Restore history when panel reopens |

#### \[panels.input\]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `multiline` | bool | `true` | Allow multiline input |
| `auto_grow` | bool | `true` | Auto-grow input area with content |
| `max_height` | u32 | `300` | Maximum input area height in pixels |

#### \[panels.focus\]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `restore_on_activate` | bool | `true` | Restore focus on panel activation |
| `show_indicator` | bool | `true` | Show focus indicator |
| `border_glow` | bool | `true` | Glow effect on focused panel border |

```toml
[panels.history]
max_messages = 5000

[panels.input]
max_height = 200

[panels.focus]
border_glow = false
```

---

### \[games\]

Built-in games configuration.

#### \[games.enabled\]

Toggle individual games on or off. All default to `true`.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `wordle` | bool | `true` | Enable Wordle |
| `connections` | bool | `true` | Enable Connections |
| `asteroids` | bool | `true` | Enable Asteroids |
| `tetris` | bool | `true` | Enable Tetris |
| `pinball` | bool | `true` | Enable Pinball |
| `doodlejump` | bool | `true` | Enable Doodle Jump |
| `minesweeper` | bool | `true` | Enable Minesweeper |
| `draw` | bool | `true` | Enable Draw |
| `subway` | bool | `true` | Enable Subway |
| `videoplayer` | bool | `true` | Enable Video Player |

#### \[games.fullscreen\]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `keyboard_passthrough` | bool | `true` | Pass keyboard events to the game in fullscreen |
| `escape_to_exit` | bool | `true` | Press Escape to exit fullscreen |

#### \[\[games.custom\_paths\]\]

Add custom games as an array of tables. Each entry has:

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Display name of the custom game |
| `path` | string | Filesystem path to the game |

```toml
[games.enabled]
wordle = true
tetris = true
pinball = false

[[games.custom_paths]]
name = "my-game"
path = "/path/to/game"
```

---

### \[livechat\]

Livechat server and moderation settings.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `enabled` | bool | `true` | -- | Enable livechat |
| `server_port` | u32 | `19847` | 1024--65535 | Server port |
| `connection_timeout` | u32 | `10` | 5--60 | Connection timeout in seconds |

#### \[livechat.nickname\]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `default` | string | `""` | Default nickname |
| `persist` | bool | `true` | Persist nickname across sessions |
| `allow_change` | bool | `true` | Allow users to change nickname |

#### \[livechat.nickname.validation\]

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `min_length` | u32 | `1` | 1--10 | Minimum nickname length |
| `max_length` | u32 | `20` | 5--50 | Maximum nickname length |
| `pattern` | string | `"^[a-zA-Z0-9_\\- ]+$"` | -- | Regex pattern for valid nicknames |

#### \[livechat.automod\]

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `enabled` | bool | `true` | -- | Enable auto-moderation |
| `filter_profanity` | bool | `true` | -- | Filter profane messages |
| `rate_limit` | u32 | `5` | 1--20 | Messages per rate window |
| `max_message_length` | u32 | `500` | 100--2000 | Maximum message length |
| `spam_detection` | bool | `true` | -- | Enable spam detection |

```toml
[livechat]
server_port = 19847
connection_timeout = 15

[livechat.automod]
rate_limit = 3
max_message_length = 1000
```

---

### \[presence\]

Online presence / social connectivity.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `enabled` | bool | `true` | -- | Enable presence system |
| `server_url` | string | `""` | -- | Presence server URL |
| `heartbeat_interval` | u32 | `30` | 10--300 | Heartbeat interval in seconds |

```toml
[presence]
enabled = true
heartbeat_interval = 60
```

---

### \[performance\]

Performance tuning.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `preset` | enum | `"high"` | `low`, `medium`, `high`, `ultra` | Quality preset |
| `frame_rate` | u32 | `60` | 30--120 | Target frame rate |
| `orb_quality` | enum | `"high"` | `low`, `medium`, `high` | Orb rendering quality |
| `bloom_passes` | u32 | `2` | 1--4 | Number of bloom blur passes |

#### \[performance.preload\]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `themes` | bool | `true` | Preload theme assets |
| `games` | bool | `false` | Preload game assets |
| `fonts` | bool | `true` | Preload font assets |

```toml
[performance]
preset = "ultra"
frame_rate = 120
orb_quality = "high"
bloom_passes = 3

[performance.preload]
games = true
```

---

### \[updates\]

Auto-update behavior.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `check_automatically` | bool | `true` | -- | Automatically check for updates |
| `channel` | enum | `"stable"` | `stable`, `beta` | Update channel |
| `check_interval` | u32 | `86400` | 3600--604800 | Check interval in seconds (1 hour to 1 week) |
| `auto_download` | bool | `false` | -- | Automatically download updates |
| `auto_install` | bool | `false` | -- | Automatically install updates |

```toml
[updates]
channel = "beta"
check_interval = 43200
auto_download = true
```

---

### \[logging\]

Logging configuration.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `level` | enum | `"INFO"` | `DEBUG`, `INFO`, `WARNING`, `ERROR` | Log level |
| `file_logging` | bool | `true` | -- | Enable file logging |
| `max_file_size_mb` | u32 | `5` | 1--50 | Maximum log file size in MB |
| `backup_count` | u32 | `3` | 1--10 | Number of rotated log backups |
| `redact_secrets` | bool | `true` | -- | Redact secrets in log output |

```toml
[logging]
level = "DEBUG"
max_file_size_mb = 10
backup_count = 5
```

---

### \[advanced\]

Advanced settings, including experimental features and developer tools.

#### \[advanced.experimental\]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `web_rendering` | bool | `false` | Enable experimental web rendering |
| `metal_debug` | bool | `false` | Enable Metal debug layer (macOS) |

#### \[advanced.developer\]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `show_fps` | bool | `false` | Show FPS counter |
| `show_debug_hud` | bool | `false` | Show debug HUD overlay |
| `inspector_enabled` | bool | `false` | Enable web inspector |

```toml
[advanced.developer]
show_fps = true
inspector_enabled = true
```

---

### \[auto\_open\]

Panels to open automatically when Jarvis starts. Defined as an array of
`[[auto_open.panels]]` tables.

The default configuration opens two Terminal panels and one Chat panel.

Each `[[auto_open.panels]]` entry:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `kind` | enum | `"terminal"` | Panel type: `terminal`, `assistant`, `chat`, `settings`, `presence` |
| `command` | string? | `null` | Command to run (empty = `$SHELL`) |
| `args` | string[] | `[]` | Arguments for the command |
| `title` | string? | `null` | Panel title |
| `working_directory` | string? | `null` | Working directory (empty = `$HOME`) |

To disable auto-open entirely, set an empty array:

```toml
[auto_open]
panels = []
```

Example with custom layout:

```toml
[[auto_open.panels]]
kind = "terminal"
command = "claude"
title = "Claude Code"

[[auto_open.panels]]
kind = "terminal"
title = "Terminal"

[[auto_open.panels]]
kind = "chat"
title = "Chat"
```

---

### \[status\_bar\]

Status bar at the bottom of the window.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `enabled` | bool | `true` | -- | Show the status bar |
| `height` | u32 | `28` | 20--48 | Status bar height in pixels |
| `show_panel_buttons` | bool | `true` | -- | Show panel toggle buttons (left side) |
| `show_online_count` | bool | `true` | -- | Show online user count (right side) |
| `bg` | string | `"rgba(24,24,37,0.95)"` | -- | Background color (CSS color string) |

```toml
[status_bar]
enabled = true
height = 32
bg = "rgba(0,0,0,0.9)"
```

---

### \[relay\]

Mobile relay bridge configuration for connecting mobile clients.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `url` | string | `"wss://jarvis-relay-363598788638.us-central1.run.app/ws"` | WebSocket URL of the relay server |
| `auto_connect` | bool | `false` | Connect to relay automatically on startup |

```toml
[relay]
url = "wss://my-relay-server.example.com/ws"
auto_connect = true
```

---

### \[plugins\]

Plugin configuration. Jarvis supports two types of plugins:

**Bookmark plugins** -- URLs that appear in the command palette and open as
webview panes. Defined in the config file.

**Local plugins** -- HTML-based plugins discovered from the filesystem
(not configured in TOML; auto-discovered from `~/.config/jarvis/plugins/`).

#### \[\[plugins.bookmarks\]\]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | `""` | Display name in the palette |
| `url` | string | `""` | URL to open |
| `category` | string | `"Plugins"` | Palette category grouping |

```toml
[[plugins.bookmarks]]
name = "Spotify"
url = "https://open.spotify.com"
category = "Web"

[[plugins.bookmarks]]
name = "Hacker News"
url = "https://news.ycombinator.com"
category = "Web"
```

#### Local Plugins

Local plugins are discovered automatically from:

```
~/.config/jarvis/plugins/<plugin-id>/plugin.toml
```

Each plugin folder should contain a `plugin.toml` manifest and an HTML entry
point. The manifest format:

```toml
name = "My Timer"
category = "Tools"
entry = "index.html"   # default
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | folder name | Display name |
| `category` | string | `"Plugins"` | Palette category |
| `entry` | string | `"index.html"` | HTML entry point file |

---

## Theme System

Themes let you change the visual appearance of Jarvis without editing
individual color/font/effect fields. A theme is a file that specifies
overrides to the default configuration.

### Built-in Themes

Jarvis ships with 8 built-in themes:

| Name | Description |
|------|-------------|
| `jarvis-dark` | Default dark theme (Catppuccin Mocha palette) |
| `jarvis-light` | Light theme variant |
| `catppuccin-mocha` | Catppuccin Mocha community theme |
| `dracula` | Dracula color scheme |
| `gruvbox-dark` | Gruvbox dark palette |
| `nord` | Nord arctic color palette |
| `solarized-dark` | Solarized dark |
| `tokyo-night` | Tokyo Night colors |

To use a built-in theme:

```toml
[theme]
name = "dracula"
```

### Custom Themes

Custom themes can be YAML or TOML files. Set `theme.name` to either:

- A theme name (looked up in theme search paths)
- A direct file path (e.g. `/path/to/my-theme.yaml`)

### Theme File Format

Themes use the `ThemeOverrides` structure. All sections are optional -- only
the fields you specify will override the defaults.

**YAML format** (`my-theme.yaml`):

```yaml
name: my-theme
colors:
  primary: "#ff6b6b"
  secondary: "#4ecdc4"
  background: "#0d1117"
  panel_bg: "rgba(13,17,23,0.9)"
  text: "#e6edf3"
  text_muted: "#7d8590"
  border: "#21262d"
  border_focused: "rgba(255,107,107,0.15)"
  success: "#3fb950"
  warning: "#d29922"
  error: "#f85149"
font:
  family: "JetBrains Mono"
  size: 14
  ligatures: true
  nerd_font: true
  font_weight: 400
visualizer:
  orb_color: "#ff6b6b"
  orb_secondary_color: "#4ecdc4"
background:
  hex_grid_color: "#ff6b6b"
  solid_color: "#0d1117"
effects:
  scanline_intensity: 0.05
  bloom_intensity: 1.0
  glow_color: "#ff6b6b"
  glow_width: 2.0
terminal:
  cursor_style: beam
  cursor_blink: true
window:
  opacity: 0.95
  blur: true
```

**TOML format** (`my-theme.toml`):

```toml
name = "my-theme"

[colors]
primary = "#ff6b6b"
background = "#0d1117"
text = "#e6edf3"

[font]
family = "Fira Code"
size = 14

[effects]
scanline_intensity = 0.05

[window]
opacity = 0.9
```

**Overridable theme sections:**

| Section | Fields |
|---------|--------|
| `colors` | Full `ColorConfig` (all 17 color fields) |
| `font` | `family`, `size`, `title_size`, `line_height`, `nerd_font`, `ligatures`, `font_weight`, `bold_weight` |
| `visualizer` | `orb_color`, `orb_secondary_color` |
| `background` | `hex_grid_color`, `solid_color` |
| `effects` | `scanline_intensity`, `vignette_intensity`, `bloom_intensity`, `glow_color`, `glow_width` |
| `terminal` | `cursor_style`, `cursor_blink` |
| `window` | `opacity`, `blur` |

### Theme Search Paths

Themes are resolved in this priority order:

1. **User config directory**: `~/.config/jarvis/themes/`
2. **Relative to executable**: `<exe_dir>/resources/themes/`
3. **Current working directory**: `./resources/themes/`

For each directory, the following extensions are tried: `.toml`, `.yaml`, `.yml`.

If the theme `name` contains a `/` or ends in `.yaml`, `.yml`, or `.toml`,
it is treated as a direct file path.

---

## Config Validation Rules

After loading and theme application, the config is validated against these
constraints. If any fail, Jarvis returns a `ConfigError::ValidationError`
with all violations joined by `"; "`.

### Font

| Field | Min | Max |
|-------|-----|-----|
| `font.size` | 8 | 32 |
| `font.title_size` | 8 | 48 |
| `font.line_height` | 1.0 | 3.0 |

### Layout

| Field | Min | Max |
|-------|-----|-----|
| `layout.panel_gap` | 1 | 20 |
| `layout.border_radius` | 0 | 20 |
| `layout.padding` | 0 | 40 |
| `layout.max_panels` | 1 | 10 |
| `layout.default_panel_width` | 0.3 | 1.0 |
| `layout.scrollbar_width` | 1 | 10 |

### Opacity

All opacity fields are validated to the range 0.0--1.0:

- `opacity.background`
- `opacity.panel`
- `opacity.orb`
- `opacity.hex_grid`
- `opacity.hud`

### Background

| Field | Min | Max |
|-------|-----|-----|
| `background.hex_grid.opacity` | 0.0 | 1.0 |
| `background.hex_grid.animation_speed` | 0.0 | 5.0 |
| `background.hex_grid.glow_intensity` | 0.0 | 1.0 |
| `background.image.blur` | 0 | 50 |
| `background.image.opacity` | 0.0 | 1.0 |
| `background.gradient.angle` | 0 | 360 |

### Visualizer

| Field | Min | Max |
|-------|-----|-----|
| `visualizer.position_x` | -1.0 | 1.0 |
| `visualizer.position_y` | -1.0 | 1.0 |
| `visualizer.scale` | 0.1 | 3.0 |
| `visualizer.orb.intensity_base` | 0.0 | 3.0 |
| `visualizer.orb.bloom_intensity` | 0.0 | 3.0 |
| `visualizer.orb.rotation_speed` | 0.0 | 5.0 |
| `visualizer.image.opacity` | 0.0 | 1.0 |
| `visualizer.image.animation_speed` | 0.0 | 5.0 |
| `visualizer.video.opacity` | 0.0 | 1.0 |
| `visualizer.particle.count` | 10 | 5000 |
| `visualizer.particle.size` | 0.5 | 10.0 |
| `visualizer.particle.speed` | 0.1 | 5.0 |
| `visualizer.particle.lifetime` | 0.5 | 10.0 |
| `visualizer.waveform.bar_count` | 8 | 256 |
| `visualizer.waveform.bar_width` | 1.0 | 10.0 |
| `visualizer.waveform.bar_gap` | 0.0 | 10.0 |
| `visualizer.waveform.height` | 20 | 500 |
| `visualizer.waveform.smoothing` | 0.0 | 1.0 |

Per-state overrides (`state_listening`, `state_speaking`, `state_skill`, `state_chat`, `state_idle`):

| Field | Min | Max |
|-------|-----|-----|
| `scale` | 0.1 | 3.0 |
| `intensity` | 0.0 | 3.0 |
| `position_x` (if set) | -1.0 | 1.0 |
| `position_y` (if set) | -1.0 | 1.0 |

### Startup

| Field | Min | Max |
|-------|-----|-----|
| `startup.on_ready.panels.count` | 1 | 5 |

### Voice

| Field | Min | Max |
|-------|-----|-----|
| `voice.sounds.volume` | 0.0 | 1.0 |

### Performance

| Field | Min | Max |
|-------|-----|-----|
| `performance.frame_rate` | 30 | 120 |
| `performance.bloom_passes` | 1 | 4 |

### Livechat

| Field | Min | Max |
|-------|-----|-----|
| `livechat.server_port` | 1024 | 65535 |
| `livechat.connection_timeout` | 5 | 60 |
| `livechat.nickname.validation.min_length` | 1 | 10 |
| `livechat.nickname.validation.max_length` | 5 | 50 |
| `livechat.automod.rate_limit` | 1 | 20 |
| `livechat.automod.max_message_length` | 100 | 2000 |

### Presence

| Field | Min | Max |
|-------|-----|-----|
| `presence.heartbeat_interval` | 10 | 300 |

### Updates

| Field | Min | Max |
|-------|-----|-----|
| `updates.check_interval` | 3600 | 604800 |

### Logging

| Field | Min | Max |
|-------|-----|-----|
| `logging.max_file_size_mb` | 1 | 50 |
| `logging.backup_count` | 1 | 10 |

### Keybinds

All keybind values are checked for duplicates. If two different actions share
the same key combination, a validation error is raised.

---

## Hot Reload

Jarvis supports live configuration reloading -- you can edit `config.toml`
while Jarvis is running and changes will take effect automatically.

### How It Works

The reload system has three layers:

1. **ConfigWatcher** -- Uses the `notify` crate to watch the config file's
   parent directory for filesystem events (`Modify` and `Create` events).
   It filters events to only react to changes affecting the actual config
   file by matching the filename.

2. **Debouncing** -- When a change is detected, the watcher enters a 500ms
   debounce window. Any additional filesystem events during this window are
   coalesced into a single reload. This prevents rapid reloads when editors
   perform atomic saves (write to temp file + rename).

3. **ReloadManager** -- Listens for change signals from the watcher, then:
   - Re-reads and parses the TOML file
   - Applies the selected theme (if not `jarvis-dark`)
   - Validates the new configuration
   - Publishes the new config via a `tokio::sync::watch` channel

   If the reload fails (parse error, validation error), the failure is
   logged as a warning and the previous valid config remains in effect.

### What Triggers a Reload

- Saving the config file in any text editor
- Replacing the file via copy/rename
- Any `Modify` or `Create` filesystem event on the config file

### Debouncing

Changes are debounced with a 500ms window. If the watcher receives multiple
change signals within 500ms, they are collapsed into one reload event. This
ensures that common editor save patterns (write temp + rename, or multiple
write calls) only trigger a single reload.

Consumers subscribe to config updates via `tokio::sync::watch::Receiver<JarvisConfig>`,
which always holds the latest valid config.

---

## Config Saving

Jarvis can save config programmatically using `save_config()` (to the default
path) or `save_config_to_path()` (to a specific path).

Writes are atomic: the config is first written to a `.tmp` file, then renamed
to the final path. If the rename fails (which can happen on Windows), it
falls back to a direct write. Parent directories are created automatically
if they do not exist.

The saved format is pretty-printed TOML via `toml::to_string_pretty`.

---

## Example Configurations

### Minimal (Use All Defaults)

An empty file or just a theme selection:

```toml
# config.toml -- everything uses defaults
```

### Focused Developer Setup

```toml
[theme]
name = "tokyo-night"

[font]
family = "JetBrains Mono"
size = 14
ligatures = true

[terminal]
scrollback_lines = 50000
cursor_style = "beam"

[terminal.mouse]
copy_on_select = true

[shell]
program = "/bin/zsh"
login_shell = true

[shell.env]
EDITOR = "nvim"

[window]
decorations = "transparent"
opacity = 0.95
blur = true

[startup.boot_animation]
enabled = false

[startup.fast_start]
enabled = true

[[auto_open.panels]]
kind = "terminal"
command = "claude"
title = "Claude Code"

[[auto_open.panels]]
kind = "terminal"
title = "Terminal"
```

### Minimal / Performance-Conscious

```toml
[performance]
preset = "low"
frame_rate = 30
orb_quality = "low"
bloom_passes = 1

[effects]
enabled = false

[visualizer]
enabled = false

[background]
mode = "solid"
solid_color = "#000000"

[startup.boot_animation]
enabled = false

[startup.fast_start]
enabled = true
delay = 0.1
```

### Streamer Setup with Custom Colors

```toml
[theme]
name = "jarvis-dark"

[colors]
primary = "#ff6b6b"
secondary = "#4ecdc4"
background = "#0d1117"

[effects]
blur_radius = 20
saturate = 1.3

[effects.bloom]
intensity = 1.5
passes = 3

[effects.glow]
color = "#ff6b6b"
width = 3.0
intensity = 0.4

[visualizer]
type = "particle"
scale = 1.5

[visualizer.particle]
style = "fire"
count = 2000
color = "#ff6b6b"

[livechat]
enabled = true

[livechat.automod]
rate_limit = 3
max_message_length = 300

[status_bar]
enabled = true
show_online_count = true

[window]
startup_mode = "fullscreen"

[[plugins.bookmarks]]
name = "Spotify"
url = "https://open.spotify.com"
category = "Media"
```

### Waveform Visualizer with Image Background

```toml
[background]
mode = "image"

[background.image]
path = "/path/to/wallpaper.jpg"
fit = "cover"
blur = 10
opacity = 0.6

[visualizer]
type = "waveform"
anchor = "bottom-left"
scale = 1.0

[visualizer.waveform]
style = "mirror"
color = "#00d4ff"
bar_count = 128
smoothing = 0.85
height = 200
```

### Multi-Panel Workspace

```toml
[layout]
max_panels = 5
panel_gap = 4
default_panel_width = 0.5
border_width = 1.0
outer_padding = 8
inactive_opacity = 0.7

[effects]
inactive_pane_dim = true
dim_opacity = 0.5

[[auto_open.panels]]
kind = "terminal"
title = "Build"
working_directory = "/projects/my-app"

[[auto_open.panels]]
kind = "terminal"
title = "Server"
command = "npm"
args = ["run", "dev"]

[[auto_open.panels]]
kind = "assistant"
title = "AI"

[[auto_open.panels]]
kind = "chat"
title = "Chat"
```
