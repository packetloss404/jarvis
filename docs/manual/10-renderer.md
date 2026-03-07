# Renderer & Visual Effects

Jarvis uses a fully GPU-accelerated rendering pipeline built on **wgpu**, the portable Rust graphics API. Every pixel on screen -- the animated hex grid background, the glowing pane borders, the boot animation's sweeping scan line, the CRT scanlines overlaying terminal text -- is rendered through custom WGSL shaders running on the GPU. This document covers every layer of that pipeline, from device initialization through final frame presentation.

---

## Table of Contents

1. [Rendering Pipeline Overview](#rendering-pipeline-overview)
2. [GPU Context Initialization](#gpu-context-initialization)
3. [Surface Management and Resizing](#surface-management-and-resizing)
4. [Background Modes](#background-modes)
5. [The Orb Visualizer](#the-orb-visualizer)
6. [Visual Effects](#visual-effects)
7. [Glassmorphism Effects](#glassmorphism-effects)
8. [Boot Animation Sequence](#boot-animation-sequence)
9. [UI Chrome Rendering](#ui-chrome-rendering)
10. [Quad Renderer](#quad-renderer)
11. [Performance Presets and Tuning](#performance-presets-and-tuning)
12. [Frame Timing and FPS Tracking](#frame-timing-and-fps-tracking)
13. [sRGB Handling and Color Management](#srgb-handling-and-color-management)
14. [Shader Reference](#shader-reference)
15. [Glyph Atlas and Text Rendering](#glyph-atlas-and-text-rendering)
16. [Command Palette](#command-palette)
17. [Assistant Panel](#assistant-panel)
18. [Configuration Reference](#configuration-reference)

---

## Rendering Pipeline Overview

The renderer is organized as the `jarvis-renderer` crate, split into these modules:

| Module | Purpose |
|---|---|
| `gpu` | wgpu device, queue, surface, uniform buffers |
| `background` | Hex grid, gradient, and solid color backgrounds |
| `effects` | Per-pane glow, dim, and scanline post-processing |
| `boot_screen` | Full-screen boot animation with surveillance HUD |
| `quad` | Instanced rectangle drawing for UI chrome |
| `ui` | Tab bar, status bar, pane border data structures |
| `render_state` | Orchestrates all pipelines into per-frame rendering |
| `perf` | Rolling-window FPS timer |
| `assistant_panel` | Chat overlay state management |
| `command_palette` | Fuzzy-searchable action picker |
| `shaders/` | Four WGSL shader files |

### Per-Frame Render Order

Each frame follows a strict two-pass pipeline managed by `RenderState::render_background()`:

1. **Pass 1 -- Background**: Clear the surface to the configured clear color, then run the background shader (hex grid or gradient) as a full-screen triangle.
2. **Pass 2 -- UI Chrome Quads**: Overlay tab bar, status bar, and pane border rectangles via instanced quad drawing.

Text rendering (terminal content, boot screen text) is handled by `glyphon` in additional passes layered on top.

During the boot sequence, a separate three-pass pipeline runs instead:

1. **Boot shader pass**: Full-screen surveillance HUD (scan line, corner brackets, vignette).
2. **Boot quad pass**: Progress bar track and fill rectangles.
3. **Boot text pass**: Title ("J A R V I S"), cycling status messages, percentage counter.

### Key Source Files

- `jarvis-rs/crates/jarvis-renderer/src/render_state/state.rs` -- `RenderState` struct and frame rendering
- `jarvis-rs/crates/jarvis-renderer/src/lib.rs` -- public API re-exports

---

## GPU Context Initialization

The `GpuContext` struct wraps all wgpu resources needed for rendering.

### Initialization Sequence (`GpuContext::new`)

```
Window -> Instance -> Surface -> Adapter -> Device + Queue -> Surface Configuration
```

1. **Create wgpu Instance** with default backends (Vulkan on Windows/Linux, Metal on macOS, DX12 on Windows).
2. **Create Surface** from the `winit` window handle.
3. **Request Adapter** with `HighPerformance` power preference and the surface as a compatibility constraint. If no hardware adapter is found, falls back to a software adapter with `force_fallback_adapter: true`.
4. **Log adapter info** -- the GPU name, device type (discrete/integrated/software), and graphics backend are logged at `INFO` level.
5. **Request Device** with default limits and no special features. The device is labeled `"jarvis-renderer device"`.
6. **Select Surface Format** -- uses the first format reported by `surface.get_capabilities()`, falling back to `Bgra8UnormSrgb` if none is reported.
7. **Configure Surface** with:
   - `PresentMode::Fifo` (vsync, guaranteed available on all platforms)
   - `desired_maximum_frame_latency: 2` (double buffering)
   - `CompositeAlphaMode::Auto`
   - `RENDER_ATTACHMENT` usage

### Struct Layout

```rust
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
    pub surface_config: wgpu::SurfaceConfiguration,
    pub size: PhysicalSize,       // { width: u32, height: u32 }
    pub scale_factor: f64,
}
```

### Error Types

```rust
pub enum RendererError {
    SurfaceError(String),     // Surface creation/acquisition failure
    AdapterNotFound,          // No GPU adapter (hardware or software)
    DeviceError(String),      // Device request failure (OOM, etc.)
    TextError(String),        // glyphon text rendering errors
}
```

### Window Transparency (Platform-Conditional)

The application window is created with `with_transparent(true)` **only on macOS**, where it is needed for titlebar blending (content extending behind traffic light buttons). On **Windows and Linux**, the window is opaque (`with_transparent(false)`) because transparent windows cause rendering artifacts -- WebView2 on Windows composites poorly with transparent backgrounds, leading to see-through panels and ghosting when panes are moved.

Similarly, the default `WebViewConfig::transparent` is `true` on macOS and `false` on Windows/Linux. The emulator panel always uses `transparent: false` regardless of platform because WebGL canvases are invisible in transparent WebViews.

### Key Source File

- `jarvis-rs/crates/jarvis-renderer/src/gpu/context.rs`

---

## Surface Management and Resizing

When the window is resized, `GpuContext::resize(width, height)` is called:

1. Clamps both dimensions to a minimum of 1 pixel (prevents zero-size surfaces).
2. Updates the internal `PhysicalSize`.
3. Reconfigures the wgpu surface with the new dimensions.

The `RenderState` wraps this and also updates the uniform buffer's viewport dimensions and aspect ratio so shaders stay correct.

Each frame acquires the next surface texture via `GpuContext::current_texture()`, which calls `surface.get_current_texture()`. If this fails (e.g., surface lost during resize), the error is logged and propagated as `RendererError::SurfaceError`.

---

## Background Modes

The background system determines what is drawn behind all terminal content. It supports six modes defined in the config schema:

### Mode: `hex_grid` (default)

**Visual appearance**: A dark screen overlaid with a faintly glowing hexagonal grid pattern. Individual hex cells fade in and out organically using simplex noise, creating a breathing, living circuit-board aesthetic. The grid edges emit a soft colored glow (cyan by default) that pulses gently over time.

**Implementation**: Rendered entirely in `background.wgsl` via a full-screen triangle (3 vertices, no vertex buffer). The fragment shader:

1. Converts screen UV coordinates to aspect-corrected space.
2. Scales to a hex grid density of 12x.
3. Computes hex cell coordinates and distance-to-nearest-edge using `hex_coords()` and `hex_dist()`.
4. Applies `smoothstep` edge glow -- bright at hex edges (d near 0.5), dark at cell centers.
5. Modulates cell brightness with 2D simplex noise scrolling slowly over time.
6. Adds a global pulse oscillation: `0.8 + 0.2 * sin(time * 0.5)`.
7. Composites with configurable color and opacity.

**Configuration**:
- `background.hex_grid.color` -- hex color string (default: `"#00d4ff"`, a cyan)
- `background.hex_grid.opacity` -- controls overall grid brightness (default: `0.08`, very subtle)
- `background.hex_grid.animation_speed` -- speed multiplier (default: `1.0`)
- `background.hex_grid.glow_intensity` -- glow brightness (default: `0.5`)

### Mode: `solid`

**Visual appearance**: A flat, uniform color filling the entire window. No animation.

**Implementation**: Uses the wgpu clear color directly -- no shader pass is needed. The `BackgroundRenderer` returns the parsed RGB values from config as the clear color.

**Configuration**:
- `background.solid_color` -- hex color string (default: `"#000000"`)

### Mode: `gradient`

**Visual appearance**: A smooth color transition between two or more colors, either linear or radial. Creates depth without the complexity of the hex grid.

**Implementation**: Requires a shader render pass. Colors from config are parsed from hex strings to `[f64; 3]` arrays. The angle is configurable.

**Configuration**:
- `background.gradient.type` -- `"linear"` or `"radial"` (default: `"radial"`)
- `background.gradient.colors` -- array of hex color strings (default: `["#000000", "#0a1520"]`)
- `background.gradient.angle` -- rotation in degrees for linear gradients (default: `180`)

### Mode: `image`

**Visual appearance**: A static image displayed behind terminal content, with optional blur and opacity.

**Implementation**: Config schema is defined but the GPU renderer currently falls back to solid black for image and video modes. The config is ready for future implementation.

**Configuration**:
- `background.image.path` -- file path to the image
- `background.image.fit` -- `"cover"`, `"contain"`, `"fill"`, or `"tile"` (default: `"cover"`)
- `background.image.blur` -- Gaussian blur radius in pixels (default: `0`)
- `background.image.opacity` -- transparency (default: `1.0`)

### Mode: `video`

**Visual appearance**: A looping video playing behind terminal content.

**Implementation**: Config schema defined; GPU renderer falls back to solid black. Reserved for future implementation.

**Configuration**:
- `background.video.path` -- file path to the video
- `background.video.loop` -- whether to loop (default: `true`)
- `background.video.muted` -- mute audio (default: `true`)
- `background.video.fit` -- `"cover"`, `"contain"`, or `"fill"` (default: `"cover"`)

### Mode: `none`

**Visual appearance**: Pure black background with no effects.

**Implementation**: Returns black as the clear color, no shader pass needed.

### Background Pipeline Architecture

The `BackgroundPipeline` creates a wgpu render pipeline with:

- The `background.wgsl` shader compiled at build time via `include_str!`.
- A single uniform buffer holding the shared `GpuUniforms` struct (80 bytes).
- A bind group layout at group 0 binding 0 (shared with other pipelines).
- Alpha blending enabled on the color target.
- Triangle list topology with no vertex buffers (vertices generated in the vertex shader).

Each frame:
1. `BackgroundPipeline::update_uniforms()` uploads the latest `GpuUniforms` via `queue.write_buffer()`.
2. `BackgroundPipeline::render()` records a render pass that either clears to the configured color or loads existing content, then draws 3 vertices (one full-screen triangle).

### Key Source Files

- `jarvis-rs/crates/jarvis-renderer/src/background/renderer.rs` -- `BackgroundRenderer` with mode selection
- `jarvis-rs/crates/jarvis-renderer/src/background/pipeline.rs` -- `BackgroundPipeline` wgpu setup
- `jarvis-rs/crates/jarvis-renderer/src/background/types.rs` -- `BackgroundMode` enum
- `jarvis-rs/crates/jarvis-renderer/src/background/helpers.rs` -- `hex_to_rgb()` color parsing
- `jarvis-rs/crates/jarvis-renderer/src/shaders/background.wgsl` -- hex grid shader
- `jarvis-rs/crates/jarvis-config/src/schema/background.rs` -- config types

---

## The Orb Visualizer

The visualizer system renders an animated entity (typically a glowing orb) that reacts to audio input and application state changes. It is the visual centerpiece of the Jarvis UI.

### Visualizer Types

| Type | Description |
|---|---|
| `orb` (default) | A 3D sphere with inner core, outer shell, wireframe mode, bloom, and rotation |
| `image` | A static or animated image with pulse, rotate, bounce, or float effects |
| `video` | A looping video synced to audio |
| `particle` | A particle system with styles: swirl, fountain, fire, snow, stars, custom |
| `waveform` | An audio spectrum display with bars, line, circular, or mirror styles |
| `none` | Disabled |

### Orb Configuration

```toml
[visualizer]
enabled = true
type = "orb"
position_x = 0.0          # NDC center X (-1.0 to 1.0)
position_y = 0.0          # NDC center Y (-1.0 to 1.0)
scale = 1.0               # Global scale multiplier
anchor = "center"         # center, top-left, top-right, bottom-left, bottom-right
react_to_audio = true     # Pulse with microphone/audio input
react_to_state = true     # Change appearance based on app state

[visualizer.orb]
color = "#00d4ff"          # Primary orb color
secondary_color = "#0088aa"
intensity_base = 1.0
bloom_intensity = 1.0
rotation_speed = 1.0
mesh_detail = "high"       # low, medium, high
wireframe = false
inner_core = true
outer_shell = true
```

### Audio Reactivity

When `react_to_audio = true`, the orb's `audio_level` uniform (0.0 to 1.0) is updated each frame from the microphone input. This value drives:

- Scale pulsing -- the orb grows when audio is louder.
- Intensity modulation -- brighter glow at higher audio levels.
- Surface distortion (in shader) -- the orb's surface warps in response to audio peaks.

### State-Based Changes

The orb changes its visual parameters based on application state. Five states are supported, each with configurable overrides:

| State | Default Scale | Default Intensity | Color Override | Position Override |
|---|---|---|---|---|
| `listening` | 1.0 | 1.0 | None (use base) | None |
| `speaking` | 1.1 | 1.4 | None | None |
| `skill` | 0.9 | 1.2 | `#ffaa00` (amber) | None |
| `chat` | 0.55 | 1.3 | None | x=0.10, y=0.30 |
| `idle` | 0.8 | 0.6 | `#444444` (dim gray) | None |

**Visual effect of states**:
- **Listening**: Full-size orb at normal brightness, ready and alert.
- **Speaking**: Slightly enlarged orb with heightened glow, conveying active output.
- **Skill**: Slightly smaller orb with amber tint, indicating tool/skill execution.
- **Chat**: Orb shrinks and shifts to the upper-left area, making room for the chat panel.
- **Idle**: Subdued gray orb at reduced size, conveying dormancy.

### Uniform Buffer Integration

The orb's parameters flow through `GpuUniforms`:

```rust
pub orb_center_x: f32,   // NDC position
pub orb_center_y: f32,
pub orb_scale: f32,       // From state config
pub power_level: f32,     // Energy level 0.0-1.0
pub audio_level: f32,     // Microphone input level
pub intensity: f32,       // From state config
```

### Key Source Files

- `jarvis-rs/crates/jarvis-config/src/schema/visualizer.rs` -- all visualizer config types
- `jarvis-rs/crates/jarvis-renderer/src/gpu/uniforms.rs` -- orb uniforms in `GpuUniforms`

---

## Visual Effects

The effects system applies post-processing passes to enhance the sci-fi aesthetic. Effects are controlled by a master toggle and individual sub-toggles.

### Scanlines

**Visual appearance**: Faint horizontal dark lines overlaid on the screen, mimicking the visible scan lines of a CRT monitor. At the default intensity (0.08), they are barely perceptible -- a subtle texture that adds depth without obscuring content. At higher intensities, they create a pronounced retro-monitor look with alternating bright and dark horizontal bands.

**Implementation**: In `effects.wgsl`, the fragment shader computes:
```wgsl
let scan_y = in.uv.y * u.screen_height;
let scanline = sin(scan_y * 3.14159265) * 0.5 + 0.5;
color.rgb *= (1.0 - scanline_intensity * (1.0 - scanline));
```

This produces a sine-wave pattern at pixel frequency, darkening every other horizontal line.

**Configuration**:
```toml
[effects.scanlines]
enabled = true
intensity = 0.08    # 0.0 = off, 1.0 = maximum darkness
```

### Vignette

**Visual appearance**: A gradual darkening around the edges of the screen, drawing the eye toward the center. At the default intensity (1.2), the corners are noticeably darker while the center remains fully bright. This creates a spotlight-like focus effect reminiscent of a camera lens or surveillance monitor.

**Implementation**: The boot shader includes a vignette function that computes distance from the UV center (0.5, 0.5) and applies a smooth falloff:
```wgsl
fn vignette(uv: vec2<f32>) -> f32 {
    let d = distance(uv, vec2<f32>(0.5, 0.5));
    return 1.0 - smoothstep(0.4, 0.9, d) * 0.4;
}
```

The vignette intensity is passed through `GpuUniforms::vignette_intensity`.

**Configuration**:
```toml
[effects.vignette]
enabled = true
intensity = 1.2     # 0.0 = off, up to 3.0 for extreme darkening
```

### Bloom

**Visual appearance**: A soft light bleed around bright elements, giving them a halo of diffused light. Text and UI elements appear to glow softly, as if emitting light into the surrounding darkness. Multiple blur passes create a progressively smoother, more diffused glow.

**Implementation**: Configured in the schema with intensity and pass count. The bloom pipeline is defined in `BloomConfig` and the pass count is also mirrored in `PerformanceConfig::bloom_passes`.

**Configuration**:
```toml
[effects.bloom]
enabled = true
intensity = 0.9     # Brightness multiplier (0.0-3.0)
passes = 2          # Blur iterations (1-5, more = smoother but costlier)
```

### Glow (Active Pane Border)

**Visual appearance**: A colored luminous border around the currently focused terminal pane. The glow radiates outward from the pane edges, fading smoothly with distance. A subtle inner-edge highlight adds a bright accent just inside the border. The default color is a soft purple (`#cba6f7`), creating a neon-sign effect that clearly marks which pane has keyboard focus.

**Implementation**: In `effects.wgsl`, the shader computes a signed distance field (SDF) from the current fragment to the pane rectangle. For focused panes:

- **Outer glow**: Fragments outside the pane (dist > 0) within the glow width get an additive color blend that fades with `smoothstep`.
- **Inner highlight**: Fragments just inside the border (within 25% of glow width) get a subtle 15% color overlay.

```wgsl
fn sdf_rect(uv: vec2<f32>) -> f32 {
    let center = (pane_min + pane_max) * 0.5;
    let half_size = (pane_max - pane_min) * 0.5;
    let d = abs(uv - center) - half_size;
    return length(max(d, vec2(0.0))) + min(max(d.x, d.y), 0.0);
}
```

**Configuration**:
```toml
[effects.glow]
enabled = true
color = "#cba6f7"   # Glow color (hex string)
width = 2.0         # Glow spread in pixels (0.0-10.0)
intensity = 0.0     # CSS box-shadow intensity (0.0-1.0)
```

### Inactive Pane Dimming

**Visual appearance**: Unfocused panes appear visually receded, with their content rendered at reduced brightness. This creates a clear visual hierarchy -- the active pane is fully bright while background panes are muted.

**Implementation**: The `EffectsRenderer::dim_factor(is_focused)` method returns:
- `1.0` for focused panes (full brightness)
- `dim_opacity` (default 0.6) for unfocused panes when dimming is enabled
- `1.0` for unfocused panes when dimming or effects are disabled

In the shader, unfocused pane colors are multiplied by the dim factor:
```wgsl
if u.is_focused < 0.5 && u.dim_factor < 0.999 {
    color = vec4<f32>(color.rgb * u.dim_factor, color.a);
}
```

**Configuration**:
```toml
[effects]
inactive_pane_dim = true
dim_opacity = 0.6   # 0.0 = fully transparent, 1.0 = no dimming
```

### Flicker

**Visual appearance**: A very subtle, rapid brightness oscillation across the entire screen, simulating the imperceptible flicker of a CRT electron gun or a fluorescent light. At the default amplitude (0.004), it is nearly invisible -- just enough to add organic life to the display without being distracting.

**Implementation**: The flicker amplitude is passed to shaders via `GpuUniforms::flicker_amplitude`. It modulates the final pixel brightness with a time-varying offset.

**Configuration**:
```toml
[effects.flicker]
enabled = true
amplitude = 0.004   # 0.0 = off, 0.05 = very noticeable
```

### CRT Curvature

**Visual appearance**: Barrel distortion that curves the screen edges inward, simulating the convex glass of a CRT monitor. The center of the screen appears normal while edges and corners are progressively distorted.

**Implementation**: Defined in config as `crt_curvature` (boolean, default `false`). Currently marked as future/no-op in the schema -- the config field exists but the shader implementation is not yet active.

**Configuration**:
```toml
[effects]
crt_curvature = false   # Currently no-op; reserved for future use
```

### Performance-Preset Effect Mapping

| Preset | Glow | Dim | Scanlines | Bloom |
|---|---|---|---|---|
| `low` | Off | Off | Off | Off |
| `medium` | On | Off | Off | Reduced |
| `high` | On | On | On | On |
| `ultra` | On | On | On | On (max quality) |

### Key Source Files

- `jarvis-rs/crates/jarvis-renderer/src/effects/renderer.rs` -- `EffectsRenderer` with preset logic
- `jarvis-rs/crates/jarvis-renderer/src/effects/types.rs` -- `EffectsConfig` struct
- `jarvis-rs/crates/jarvis-renderer/src/shaders/effects.wgsl` -- per-pane effects shader
- `jarvis-rs/crates/jarvis-config/src/schema/effects.rs` -- all effect config types

---

## Glassmorphism Effects

Glassmorphism creates frosted-glass panel backgrounds by combining backdrop blur, color saturation boosting, and translucent panel surfaces. This gives panels a sense of depth and transparency, allowing the background (hex grid, gradient) to show through in a blurred form.

### Backdrop Blur

**Visual appearance**: Content behind panels is smoothly blurred, creating a frosted glass effect. Text and hex grid lines behind the panel become soft, indistinct shapes that provide visual context without competing with panel content.

**Configuration**:
```toml
[effects]
blur_radius = 12    # Blur radius in pixels (0-40). 0 = no blur
```

### Saturate

**Visual appearance**: Colors behind the panel are slightly boosted in saturation, making the blurred backdrop more vivid and colorful rather than washed out.

**Configuration**:
```toml
[effects]
saturate = 1.1      # Saturation multiplier (0.0-2.0). 1.0 = unchanged
```

### Panel Opacity

Panel transparency is controlled separately from effects:

```toml
[opacity]
background = 1.0    # Overall background opacity
panel = 0.85        # Terminal panel opacity
orb = 1.0           # Visualizer orb opacity
hex_grid = 0.8      # Hex grid overlay opacity
hud = 1.0           # HUD elements opacity
```

### Transition Speed

Smooth animated transitions when panels open, close, or change focus:

```toml
[effects]
transition_speed = 150   # Milliseconds (0-500). 0 = instant
```

---

## Boot Animation Sequence

The boot screen presents a full-screen surveillance-style HUD during application initialization, creating an immersive "powering up" experience.

### Visual Elements

1. **Dark background**: Solid fill using the configured background color (default: Ayu Mirage dark, `#1F2430`).
2. **Sweeping scan line**: A horizontal bright line that continuously sweeps from top to bottom every 1.2 seconds. It has a bright core and a soft trailing glow, rendered in the accent color. Looks like a radar sweep or security scanner.
3. **Corner brackets**: Four L-shaped brackets at each corner of the screen, drawn in the accent color. They scale with screen size (6% of the smaller dimension for arm length, 4% margin from edges, 2px line thickness). They evoke a heads-up display or targeting reticle.
4. **Vignette**: Edge darkening that focuses attention toward the center (40% darkening at corners).
5. **Title text**: "J A R V I S" displayed in monospace font, centered horizontally at 44% from the top. Font size scales with window height (4% of height, minimum 24px).
6. **Status messages**: Military/intelligence-themed messages that cycle every 1.5 seconds, displayed above the progress bar in the muted text color. The 14 default messages are:
   - PROVISIONING ARMAMENTS
   - CALIBRATING SENSOR ARRAY
   - ESTABLISHING SECURE CHANNELS
   - INITIALIZING NEURAL INTERFACE
   - DEPLOYING COUNTERMEASURES
   - SYNCHRONIZING THREAT MATRIX
   - LOADING TACTICAL OVERLAYS
   - VERIFYING BIOMETRIC CLEARANCE
   - ACTIVATING PERIMETER DEFENSE
   - COMPILING INTELLIGENCE BRIEFS
   - SCANNING FREQUENCY SPECTRUM
   - ENGAGING QUANTUM ENCRYPTION
   - BOOTSTRAPPING CORE SYSTEMS
   - SYSTEM ONLINE
7. **Progress bar**: Centered at 62% down the screen, 35% of window width, 4px tall. A dark track with the accent-colored fill growing left to right as progress advances from 0% to 100%.
8. **Percentage counter**: Displayed to the right of the progress bar in muted text.

### Timing

- Default total duration: **4.5 seconds**.
- Message change interval: **1.5 seconds** (cycles through 3 messages before completing).
- Can be skipped immediately with any key press (when `skip_on_key = true`).
- Progress is calculated as `elapsed / duration`, clamped to `[0.0, 1.0]`.

### Color Scheme (Defaults)

| Element | Color | Hex |
|---|---|---|
| Background | Dark blue-gray | `#1F2430` |
| Accent (brackets, scan line, progress) | Warm gold | `#FFCC66` |
| Muted text (status, percentage) | Gray-blue | `#707A8C` |
| Progress track | Very dark blue | `#171B24` |

### Render Pipeline

The `BootScreen` struct owns three sub-renderers:

1. A wgpu render pipeline using `boot.wgsl` for the full-screen HUD shader.
2. A `QuadRenderer` (separate instance from the main UI) for the progress bar.
3. A `BootTextRenderer` wrapping `glyphon` for GPU-accelerated text.

The boot shader uses a 48-byte `BootUniforms` struct:

```rust
pub struct BootUniforms {
    pub time: f32,
    pub progress: f32,
    pub screen_width: f32,
    pub screen_height: f32,
    pub accent_r: f32,
    pub accent_g: f32,
    pub accent_b: f32,
    pub bg_r: f32,
    pub bg_g: f32,
    pub bg_b: f32,
    pub opacity: f32,
    pub _pad: f32,
}
```

### Configuration

```toml
[startup.boot_animation]
enabled = true
duration = 4.5
skip_on_key = true
music_enabled = true
voiceover_enabled = true
```

### Key Source Files

- `jarvis-rs/crates/jarvis-renderer/src/boot_screen/mod.rs` -- `BootScreen` renderer
- `jarvis-rs/crates/jarvis-renderer/src/boot_screen/types.rs` -- `BootUniforms`, `BootScreenConfig`
- `jarvis-rs/crates/jarvis-renderer/src/boot_screen/text.rs` -- `BootTextRenderer` (glyphon wrapper)
- `jarvis-rs/crates/jarvis-renderer/src/shaders/boot.wgsl` -- boot HUD shader

---

## UI Chrome Rendering

UI chrome refers to the non-content visual elements surrounding the terminal grid: the tab bar, status bar, and pane borders.

### Tab Bar

**Visual appearance**: A dark horizontal bar spanning the full window width at the top. The active tab is highlighted with a slightly lighter background. Each tab shows its title text.

**Dimensions**: 32px tall (constant, `DEFAULT_TAB_BAR_HEIGHT`).

**Colors**:
- Bar background: `[0.12, 0.12, 0.14, 1.0]` (very dark gray)
- Active tab highlight: `[0.22, 0.22, 0.26, 1.0]` (slightly lighter gray)

**Tab width**: Evenly distributed across the window width (`window_width / tab_count`).

**Data structures**:
```rust
pub struct Tab {
    pub pane_id: u32,      // For click-to-focus
    pub title: String,
    pub is_active: bool,
}

pub struct TabBar {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub height: f32,       // 32.0
}
```

### Status Bar

**Visual appearance**: A dark semi-transparent bar at the bottom of the window with three text zones (left, center, right). Displays mode indicators, connection status, and other runtime information.

**Dimensions**: 24px tall (constant, `DEFAULT_STATUS_BAR_HEIGHT`).

**Colors**:
- Background: `[0.1, 0.1, 0.1, 0.9]` (near-black, 90% opaque)
- Foreground (text): `[0.9, 0.9, 0.9, 1.0]` (light gray)

**Data structure**:
```rust
pub struct StatusBar {
    pub left_text: String,
    pub center_text: String,
    pub right_text: String,
    pub height: f32,          // 24.0
    pub bg_color: [f32; 4],
    pub fg_color: [f32; 4],
}
```

### Pane Borders

**Visual appearance**: Colored rectangles drawn at the edges of each terminal pane. Focused panes get the glow effect (if enabled); unfocused panes show a simple border line.

**Data structure**:
```rust
pub struct PaneBorder {
    pub rect: Rect,            // Bounding rectangle
    pub color: [f32; 4],       // RGBA
    pub width: f32,            // Line width in pixels
    pub is_focused: bool,      // Drives glow effect
}
```

### Content Area Calculation

`UiChrome::content_rect(window_width, window_height)` computes the remaining area after subtracting chrome elements:

```
content.y      = tab_bar.height (or 0 if no tab bar)
content.height = window_height - tab_bar.height - status_bar.height
content.width  = window_width
content.x      = 0
```

This rectangle is clamped to never go negative.

### Pane Gap

Adjacent panes are separated by a configurable gap:

```toml
[layout]
panel_gap = 6    # Pixels between panes (0-20)
```

### Key Source Files

- `jarvis-rs/crates/jarvis-renderer/src/ui/chrome.rs` -- `UiChrome` struct and mutation methods
- `jarvis-rs/crates/jarvis-renderer/src/ui/layout.rs` -- content_rect, tab_bar_rect, status_bar_rect
- `jarvis-rs/crates/jarvis-renderer/src/ui/types.rs` -- Tab, TabBar, StatusBar, PaneBorder

---

## Quad Renderer

The `QuadRenderer` is a general-purpose GPU-accelerated filled rectangle renderer used for all UI chrome backgrounds (tab bar, status bar, progress bars).

### Architecture

Uses **instanced drawing** to batch up to 256 rectangles into a single draw call:

- **Unit quad**: 4 vertices forming a `[0,0]` to `[1,1]` square, indexed with 6 indices (two triangles: 0-1-2 and 0-2-3).
- **Instance buffer**: Each instance carries a `QuadInstance` (32 bytes: rect `[x, y, w, h]` + color RGBA).
- **Uniform buffer**: Viewport resolution (16 bytes: `[width, height, pad, pad]`).

### Vertex Shader

The shader scales the unit quad by the instance's rect dimensions and translates to the instance's position, then converts from pixel coordinates to NDC:

```wgsl
let pixel_x = instance.rect.x + vertex.position.x * instance.rect.z;
let pixel_y = instance.rect.y + vertex.position.y * instance.rect.w;
let ndc_x = (pixel_x / uniforms.resolution.x) * 2.0 - 1.0;
let ndc_y = 1.0 - (pixel_y / uniforms.resolution.y) * 2.0;
```

### Fragment Shader

Simply passes through the instance color:

```wgsl
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
```

### Per-Frame Usage

1. `QuadRenderer::prepare()` uploads instance data and viewport resolution to the GPU.
2. `QuadRenderer::render()` binds the pipeline, vertex/index/instance buffers, and issues an indexed instanced draw call.
3. Skips the draw call entirely if instance count is 0.

### Chrome Quad Generation

`RenderState::prepare_chrome_quads()` builds the quad list from the `UiChrome` state:

1. If a tab bar exists, push a full-width dark background quad, then a highlight quad for the active tab.
2. If a status bar exists, push its background quad.
3. Upload all quads to the GPU.

### Key Source Files

- `jarvis-rs/crates/jarvis-renderer/src/quad/renderer.rs` -- `QuadRenderer`
- `jarvis-rs/crates/jarvis-renderer/src/quad/types.rs` -- `QuadInstance`, `Vertex`, constants
- `jarvis-rs/crates/jarvis-renderer/src/quad/pipeline.rs` -- inline WGSL shader

---

## Performance Presets and Tuning

Four quality presets control the trade-off between visual fidelity and GPU/CPU load.

### Preset Definitions

| Setting | Low | Medium | High (default) | Ultra |
|---|---|---|---|---|
| Frame rate | 60 | 60 | 60 | 60 |
| Pane glow | Off | On | On | On |
| Pane dim | Off | Off | On | On |
| Scanlines | Off | Off | On | On |
| Bloom passes | 0 | 1 | 2 | 2+ |
| Orb quality | low | medium | high | high |
| Effects master | Off | On | On | On |

### Configuration

```toml
[performance]
preset = "high"        # low, medium, high, ultra
frame_rate = 60        # Target FPS
orb_quality = "high"   # low, medium, high
bloom_passes = 2       # Number of blur iterations (1-5)

[performance.preload]
themes = true
games = false
fonts = true
```

### How Presets Affect the Renderer

`EffectsRenderer::from_performance_preset()` maps the preset string to concrete `EffectsConfig` values:

- **Low**: Sets `enabled = false`, disabling all effects globally. Dim opacity is 1.0 (no dimming). This is suitable for low-end hardware or when maximum terminal responsiveness is needed.
- **Medium**: Enables the effects master switch. Turns on glow only. Dim and scanlines are disabled. A balanced choice for integrated GPUs.
- **High/Ultra**: Enables all effects: glow, dim (at 0.6 opacity), and scanlines. Full visual fidelity.

Any unrecognized preset string is treated as "high".

### Key Source Files

- `jarvis-rs/crates/jarvis-config/src/schema/performance.rs` -- `PerformanceConfig`, `PerformancePreset`
- `jarvis-rs/crates/jarvis-renderer/src/effects/renderer.rs` -- `from_performance_preset()`

---

## Frame Timing and FPS Tracking

The `FrameTimer` provides a rolling-window average of frame durations for performance monitoring.

### How It Works

1. A `VecDeque<Duration>` stores the last 120 frame durations.
2. `begin_frame()` is called at the start of each frame. It records the elapsed time since the last call and pushes it to the deque.
3. When the deque exceeds 120 samples, the oldest is dropped.
4. `fps()` computes `sample_count / total_duration` -- the harmonic mean FPS over the window.
5. `frame_time_ms()` computes `(total_duration / sample_count) * 1000` -- average frame time in milliseconds.

### Properties

- **120-sample window**: At 60 FPS, this covers 2 seconds of history. Smooths out transient spikes while remaining responsive to sustained performance changes.
- **Initial state**: Returns 0.0 for both FPS and frame time until at least one frame has been recorded.
- **Zero protection**: If total duration is zero or negative (all durations are sub-nanosecond), returns 0.0 FPS.

### First Frame Logging

The renderer logs the first successful frame presentation exactly once using an `AtomicBool` guard:

```
INFO First frame presented (1920x1080, format=Bgra8UnormSrgb)
```

### GPU-Side Time Tracking

The `GpuUniforms::update_time(dt)` method accumulates elapsed time for shader animations. It wraps at 21600 seconds (6 hours) to prevent f32 precision loss:

```rust
self.time = (self.time + dt) % 21600.0;
```

### Key Source Files

- `jarvis-rs/crates/jarvis-renderer/src/perf.rs` -- `FrameTimer`
- `jarvis-rs/crates/jarvis-renderer/src/render_state/helpers.rs` -- `log_first_frame()`
- `jarvis-rs/crates/jarvis-renderer/src/gpu/uniforms.rs` -- `update_time()`

---

## sRGB Handling and Color Management

### Surface Format Selection

The renderer uses the surface's preferred texture format, falling back to `Bgra8UnormSrgb`. On most platforms:

- **Windows (DX12)**: `Bgra8UnormSrgb`
- **macOS (Metal)**: `Bgra8UnormSrgb`
- **Linux (Vulkan)**: `Bgra8UnormSrgb` or `Bgra8Unorm`

The selected format is logged at startup along with all available formats.

### sRGB Conversion in Shaders

The boot shader (`boot.wgsl`) includes explicit sRGB-to-linear conversion functions for correct color blending:

```wgsl
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

fn srgb3(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(srgb_to_linear(c.x), srgb_to_linear(c.y), srgb_to_linear(c.z));
}
```

Config colors are specified in sRGB (hex strings like `"#FFCC66"`). When the surface format is an sRGB format (`*Srgb`), wgpu automatically applies the sRGB OETF on output. The boot shader converts input colors from sRGB to linear space before blending so that the automatic sRGB output encoding produces correct results.

### Hex Color Parsing

The `hex_to_rgb()` helper parses 6-digit hex color strings (with or without `#` prefix) into normalized `[f64; 3]` RGB values:

```rust
pub fn hex_to_rgb(hex: &str) -> Option<[f64; 3]> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 { return None; }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some([r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0])
}
```

Invalid colors fall back to sensible defaults (typically cyan `[0.0, 0.83, 1.0]` for the hex grid, or black `[0.0, 0.0, 0.0]` for solid backgrounds).

---

## Shader Reference

The renderer includes four WGSL shader files, compiled at build time via `include_str!`.

### `background.wgsl` -- Hex Grid Background

**Purpose**: Renders the animated hexagonal grid overlay.

**Vertex stage**: Generates a full-screen triangle from `vertex_index` (0, 1, 2). No vertex buffers needed.

**Fragment stage**:
1. Converts UV to aspect-corrected coordinates centered at origin.
2. Scales to hex grid density (12x).
3. Computes nearest hex cell via `hex_coords()` and edge distance via `hex_dist()`.
4. Applies edge glow with `smoothstep(0.45, 0.5, d)`.
5. Modulates with 2D simplex noise (`snoise()` based on Ashima Arts implementation).
6. Adds a slow pulse: `0.8 + 0.2 * sin(time * 0.5)`.
7. Outputs `vec4(color * alpha, alpha * bg_alpha)` with alpha blending.

**Uniform buffer**: `GpuUniforms` (80 bytes, 20 floats).

### `effects.wgsl` -- Per-Pane Effects

**Purpose**: Post-processing per pane: glow border for focused panes, dimming for unfocused panes, optional scanlines.

**Inputs**: Pane texture (sampled), `PaneUniforms` (16 floats including pane bounds, glow params, dim factor).

**Fragment stage**:
1. Samples the pane texture.
2. If focused and glow is enabled, computes SDF distance to pane rect. Applies additive outer glow and subtle inner highlight.
3. If unfocused, multiplies RGB by dim factor.
4. If scanline intensity > 0, applies sine-wave horizontal darkening.

### `boot.wgsl` -- Boot Screen HUD

**Purpose**: Renders the surveillance-style boot animation background.

**Fragment stage**:
1. Converts config colors from sRGB to linear space.
2. Draws a sweeping horizontal scan line (1.2s period) with bright core and soft trailing glow.
3. Draws corner brackets (L-shaped lines at each corner, scaling with screen size).
4. Applies vignette edge darkening.
5. Outputs with configurable opacity.

**Uniform buffer**: `BootUniforms` (48 bytes, 12 floats).

### `text.wgsl` -- Placeholder

A placeholder file noting that text rendering is handled internally by the `glyphon` library, which manages its own shaders.

### Inline Shader: Quad Pipeline

The quad renderer's shader is embedded directly in `pipeline.rs` as a Rust string constant. It transforms unit-quad vertices by instance rect dimensions, converts pixel coordinates to NDC, and passes through instance colors.

---

## Glyph Atlas and Text Rendering

### Atlas Configuration

The `AtlasConfig` controls the glyph texture atlas used by `glyphon`:

```rust
pub struct AtlasConfig {
    pub initial_size: u32,   // 512 pixels (covers most glyph sets)
    pub max_size: u32,       // 4096 pixels (GPU texture limit for atlas)
}
```

The atlas starts at 512x512 and can grow up to 4096x4096 as more unique glyphs are needed. Periodic trimming (`atlas.trim()`) frees unused glyph allocations.

### Boot Text Renderer

The `BootTextRenderer` wraps `glyphon` with a simplified API:

1. **Create once** with device, queue, and surface format.
2. **Prepare each frame** with a list of `TextEntry` items (text, position, font size, color, max width).
3. **Render** into an existing render pass.

Internally it manages:
- `FontSystem` -- system font discovery and shaping
- `SwashCache` -- rasterized glyph cache
- `TextAtlas` -- GPU texture atlas for glyph bitmaps
- `Viewport` -- resolution tracking for correct text placement
- `TextRenderer` -- the glyphon render pipeline
- A reusable pool of `Buffer` objects (grown as needed, never shrunk)

All text uses monospace fonts (`Family::Monospace`).

---

## Command Palette

The command palette is a fuzzy-searchable overlay for executing application actions.

### Modes

| Mode | Behavior |
|---|---|
| `ActionSelect` | Filter and select from the action list |
| `UrlInput` | Type a URL to navigate to |

### Features

- Case-insensitive substring filtering.
- Wrapping selection (next/prev cycle through all items).
- Keybind display strings shown alongside action labels.
- Dynamic item injection (e.g., plugin actions via `add_items()`).
- URL mode returns `Action::OpenURL(typed_url)` on confirm.

---

## Assistant Panel

The `AssistantPanel` manages the state for the AI chat overlay, including:

- Message history (`Vec<ChatMessage>` with `User` or `Assistant` roles)
- Input text buffer with character-by-character editing
- Streaming text accumulation for in-progress responses
- Scroll offset tracking
- Error state display

The panel itself does not perform rendering -- it provides the data model that the render loop draws.

---

## Configuration Reference

### Complete Effects Configuration

```toml
[effects]
enabled = true                 # Master toggle for all effects
inactive_pane_dim = true       # Dim unfocused panes
dim_opacity = 0.6              # Opacity for dimmed panes (0.0-1.0)
crt_curvature = false          # CRT barrel distortion (future)
blur_radius = 12               # Glassmorphic backdrop blur (0-40 pixels)
saturate = 1.1                 # Backdrop saturation boost (0.0-2.0)
transition_speed = 150         # Animation speed in ms (0-500)

[effects.scanlines]
enabled = true
intensity = 0.08               # Scanline darkness (0.0-1.0)

[effects.vignette]
enabled = true
intensity = 1.2                # Edge darkening strength (0.0-3.0)

[effects.bloom]
enabled = true
intensity = 0.9                # Bloom brightness (0.0-3.0)
passes = 2                     # Blur iterations (1-5)

[effects.glow]
enabled = true
color = "#cba6f7"              # Active pane glow color
width = 2.0                    # Glow spread in pixels (0.0-10.0)
intensity = 0.0                # CSS box-shadow intensity (0.0-1.0)

[effects.flicker]
enabled = true
amplitude = 0.004              # Brightness oscillation (0.0-0.05)
```

### Complete Background Configuration

```toml
[background]
mode = "hex_grid"              # hex_grid, solid, gradient, image, video, none
solid_color = "#000000"

[background.hex_grid]
color = "#00d4ff"
opacity = 0.08
animation_speed = 1.0
glow_intensity = 0.5

[background.gradient]
type = "radial"                # linear, radial
colors = ["#000000", "#0a1520"]
angle = 180

[background.image]
path = ""
fit = "cover"                  # cover, contain, fill, tile
blur = 0
opacity = 1.0

[background.video]
path = ""
loop = true
muted = true
fit = "cover"                  # cover, contain, fill
```

### Complete Visualizer Configuration

```toml
[visualizer]
enabled = true
type = "orb"                   # orb, image, video, particle, waveform, none
position_x = 0.0
position_y = 0.0
scale = 1.0
anchor = "center"              # center, top-left, top-right, bottom-left, bottom-right
react_to_audio = true
react_to_state = true

[visualizer.orb]
color = "#00d4ff"
secondary_color = "#0088aa"
intensity_base = 1.0
bloom_intensity = 1.0
rotation_speed = 1.0
mesh_detail = "high"           # low, medium, high
wireframe = false
inner_core = true
outer_shell = true

[visualizer.image]
path = ""
fit = "contain"                # contain, cover, fill
opacity = 1.0
animation = "none"             # none, pulse, rotate, bounce, float
animation_speed = 1.0

[visualizer.video]
path = ""
loop = true
muted = true
fit = "cover"
opacity = 1.0
sync_to_audio = false

[visualizer.particle]
style = "swirl"                # swirl, fountain, fire, snow, stars, custom
count = 500
color = "#00d4ff"
size = 2.0
speed = 1.0
lifetime = 3.0
custom_shader = ""

[visualizer.waveform]
style = "bars"                 # bars, line, circular, mirror
color = "#00d4ff"
bar_count = 64
bar_width = 3.0
bar_gap = 2.0
height = 100
smoothing = 0.8

[visualizer.state_listening]
scale = 1.0
intensity = 1.0

[visualizer.state_speaking]
scale = 1.1
intensity = 1.4

[visualizer.state_skill]
scale = 0.9
intensity = 1.2
color = "#ffaa00"

[visualizer.state_chat]
scale = 0.55
intensity = 1.3
position_x = 0.10
position_y = 0.30

[visualizer.state_idle]
scale = 0.8
intensity = 0.6
color = "#444444"
```

### Complete Performance Configuration

```toml
[performance]
preset = "high"                # low, medium, high, ultra
frame_rate = 60
orb_quality = "high"           # low, medium, high
bloom_passes = 2               # 1-5

[performance.preload]
themes = true
games = false
fonts = true
```

### Complete Opacity Configuration

```toml
[opacity]
background = 1.0
panel = 0.85
orb = 1.0
hex_grid = 0.8
hud = 1.0
```

### Boot Animation Configuration

```toml
[startup.boot_animation]
enabled = true
duration = 4.5
skip_on_key = true
music_enabled = true
voiceover_enabled = true
```

### Window Configuration (Rendering-Relevant)

```toml
[window]
decorations = "full"           # full, none, transparent
opacity = 1.0                  # Window-level opacity (0.0-1.0)
blur = false                   # macOS vibrancy / backdrop blur
```

### Layout Configuration (Rendering-Relevant)

```toml
[layout]
panel_gap = 6                  # Pixels between adjacent panes
border_radius = 8              # Corner rounding in pixels
border_width = 0.0             # Border line width (0.0-3.0)
outer_padding = 0              # Screen-edge padding
inactive_opacity = 1.0         # Opacity for unfocused panels
```
