# Tiling & Window Management

## Overview

Jarvis uses a **binary split tree** to manage pane layout within the application
window. Every visible pane occupies a leaf node in the tree, and interior nodes
describe how their two children are divided -- either horizontally (side by side)
or vertically (top and bottom). The system supports splitting, closing, focusing,
resizing, swapping, zooming, and tabbed stacking of panes, all driven by a
single authoritative data structure: the `SplitNode` tree.

The tiling subsystem lives in the `jarvis-tiling` crate and is intentionally
decoupled from rendering and platform windowing. The application layer
(`jarvis-app`) consumes the tiling API to dispatch user actions and synchronise
webview positions with the computed layout.

### Crate structure

```
jarvis-tiling/
  src/
    lib.rs              -- Public re-exports
    commands.rs          -- TilingCommand enum
    pane.rs              -- Pane struct (id, kind, title)
    tree/
      types.rs           -- SplitNode, Direction
      operations.rs      -- split_at, remove_pane, swap_panes, adjust_ratio
      traversal.rs       -- next_pane, prev_pane, find_neighbor
    layout/
      types.rs           -- LayoutEngine configuration
      calculation.rs     -- Recursive tree-to-rect computation
      borders.rs         -- SplitBorder computation for drag-resize
    manager/
      types.rs           -- TilingManager struct and accessors
      focus.rs           -- Focus & zoom operations
      operations.rs      -- Split, close, resize, swap
      layout_compute.rs  -- compute_layout, execute(TilingCommand)
      stacks.rs          -- Push/cycle tabs within a leaf position
    stack/
      types.rs           -- PaneStack struct
      operations.rs      -- push, remove, cycle_next, cycle_prev
    platform/
      mod.rs             -- WindowManager trait, ExternalWindow, WindowEvent
      windows.rs         -- Windows implementation
      macos/             -- macOS CoreGraphics implementation
      wayland.rs         -- Wayland stub
      x11.rs             -- X11 stub
      noop.rs            -- No-op fallback
```

---

## The Split Tree

### Data Model

The tree is represented by the `SplitNode` enum:

```rust
pub enum SplitNode {
    Leaf { pane_id: u32 },
    Split {
        direction: Direction,   // Horizontal or Vertical
        ratio: f64,             // 0.0 .. 1.0, how much space the first child gets
        first: Box<SplitNode>,
        second: Box<SplitNode>,
    },
}
```

```rust
pub enum Direction {
    Horizontal,  // Children placed side by side (left | right)
    Vertical,    // Children placed top over bottom (top / bottom)
}
```

A single pane is a `Leaf`. When the user splits a pane, that `Leaf` is
replaced by a `Split` node whose `first` child is the original pane and
`second` child is the new pane, with `ratio` initialised to `0.5` (equal
split).

### ASCII diagram: binary split tree

```
Window (800 x 600)
+-------------------------------------------+
|                                           |
|                                           |
|    Tree root: Split(H, 0.5)              |
|     /                   \                |
|  Leaf(1)         Split(V, 0.5)           |
|                   /          \           |
|               Leaf(2)     Leaf(3)        |
|                                           |
+-------------------------------------------+

Visual result:
+------------------+------------------+
|                  |                  |
|                  |    Pane 2        |
|    Pane 1        |                  |
|                  +------------------+
|                  |                  |
|                  |    Pane 3        |
|                  |                  |
+------------------+------------------+
```

### Key operations on SplitNode

| Method | Description |
|---|---|
| `split_at(target_id, new_id, direction)` | Replace the leaf `target_id` with a Split whose first child is `target_id` and second is `new_id`. Returns `true` on success. |
| `remove_pane(target_id)` | Remove a leaf and replace its parent Split with the surviving sibling. Cannot remove the last pane. |
| `swap_panes(a, b)` | Swap two pane IDs in-place across the tree. Both must exist. |
| `adjust_ratio(target_id, delta)` | Adjust the ratio of the parent split of `target_id`. Positive delta grows the side containing the target. Clamped to `[0.1, 0.9]`. |
| `adjust_ratio_between(first_id, second_id, delta)` | Find the unique split where `first_id` is in the first subtree and `second_id` is in the second subtree, and adjust its ratio. |
| `pane_count()` | Count all leaf nodes. |
| `contains_pane(id)` | Check if a pane ID exists anywhere in the tree. |
| `collect_pane_ids()` | Depth-first left-to-right list of all pane IDs. |
| `next_pane(current_id)` | Next pane in DFS order, wrapping around. |
| `prev_pane(current_id)` | Previous pane in DFS order, wrapping around. |
| `find_neighbor(target_id, direction)` | Find the adjacent pane in a given direction using linear ordering. |

---

## Pane Types

Each pane is identified by a numeric `PaneId(u32)` and carries a `PaneKind`
that determines what content it displays.

```rust
pub struct Pane {
    pub id: PaneId,
    pub kind: PaneKind,
    pub title: String,
}
```

### PaneKind variants

| Kind | Description |
|---|---|
| `Terminal` | An interactive terminal emulator (xterm.js in a webview). The default pane kind. |
| `Chat` | A chat interface pane, loaded from `jarvis://localhost/chat/index.html`. |
| `WebView` | A general-purpose web content pane (settings, documentation, etc.). |
| `Assistant` | An AI assistant panel. |
| `ExternalApp` | A captured external application window managed by the platform `WindowManager`. |

The `Pane` struct provides convenience constructors for each kind:

- `Pane::new_terminal(id, title)`
- `Pane::new_chat(id, title)`
- `Pane::new_webview(id, title)`
- `Pane::new_assistant(id, title)`
- `Pane::new_external(id, title)`

---

## Layout Configuration

Layout settings are defined in `LayoutConfig` (from `jarvis-config`) and
control the visual appearance of the tiling grid.

```rust
pub struct LayoutConfig {
    pub panel_gap: u32,            // Gap between panels (0-20 px), default 6
    pub border_radius: u32,        // Corner radius (0-20 px), default 8
    pub padding: u32,              // Inner pane padding (0-40 px), default 10
    pub max_panels: u32,           // Max simultaneous panels (1-10), default 5
    pub default_panel_width: f64,  // Default width fraction (0.3-1.0), default 0.72
    pub scrollbar_width: u32,      // Scrollbar width (1-10 px), default 3
    pub border_width: f64,         // Pane border width (0.0-3.0 px), default 0.0
    pub outer_padding: u32,        // Screen-edge padding (0-40 px), default 0
    pub inactive_opacity: f64,     // Opacity for unfocused panels (0.0-1.0), default 1.0
}
```

There is also an `OpacityConfig` for fine-grained transparency control of
different UI layers (background, panel, orb, hex grid, HUD).

### Mapping to the layout engine

The `LayoutEngine` struct in `jarvis-tiling` uses a subset of these settings:

```rust
pub struct LayoutEngine {
    pub gap: u32,            // from LayoutConfig.panel_gap
    pub outer_padding: u32,  // from LayoutConfig.outer_padding
    pub min_pane_size: f64,  // minimum dimension for any pane (default 50.0)
}
```

These values can be updated at runtime via `TilingManager::set_gap()` and
`TilingManager::set_outer_padding()`.

---

## The Layout Engine

The layout engine converts the split tree into concrete pixel rectangles. It is
a pure function: given a `SplitNode` tree and a viewport `Rect`, it returns
`Vec<(u32, Rect)>` -- a list of pane ID / rectangle pairs.

### Algorithm

1. **Apply outer padding.** The viewport is inset by `outer_padding` on all
   four sides.

2. **Recurse through the tree.** At each `Split` node:
   - Subtract the `gap` from the available dimension (width for Horizontal,
     height for Vertical).
   - Multiply the remaining space by `ratio` to get the first child's size.
   - The second child gets the remainder.
   - The gap is placed between the two children.

3. **Leaf nodes** emit `(pane_id, bounds)` directly.

### Horizontal split calculation

```
Available width = bounds.width - gap
First child width  = available_width * ratio
Second child width = available_width - first_width
Second child x     = bounds.x + first_width + gap
```

### Vertical split calculation

```
Available height = bounds.height - gap
First child height  = available_height * ratio
Second child height = available_height - first_height
Second child y      = bounds.y + first_height + gap
```

### ASCII diagram: gap and padding

```
Viewport: 800 x 600, outer_padding = 10, gap = 8

+---------- 800 ----------+
|  padding = 10            |
|  +--- 780 x 580 ------+ |
|  |        |  gap  |    | |
|  | Pane 1 | (8px) | P2 | |
|  |  386   |       |386 | |
|  +--------+-------+----+ |
|                          |
+--------------------------+

Available = 780 - 8 = 772
Each pane = 772 * 0.5 = 386
```

---

## Splitting

### Manual split

The user can request either a horizontal or vertical split:

- **Horizontal split** (`SplitHorizontal`): places panes side by side.
- **Vertical split** (`SplitVertical`): places panes top and bottom.

When a split is performed:

1. The currently focused leaf is replaced by a `Split` node.
2. The original pane becomes the `first` child.
3. A new pane (with auto-incremented ID) becomes the `second` child.
4. The `ratio` is set to `0.5`.
5. Focus moves to the new pane.
6. If the manager was in zoom mode, zoom is cancelled.

### Auto-split logic

The `auto_split_direction(viewport)` method on `TilingManager` chooses the
split direction based on the focused pane's current aspect ratio:

- If `width >= height` --> `Horizontal` (split side by side, making both narrower)
- If `width < height`  --> `Vertical` (split top/bottom, making both shorter)

The `NewPane` action in the application dispatch uses auto-split:

```rust
Action::NewPane => {
    let content = self.chrome.content_rect(w, h);
    let dir = self.tiling.auto_split_direction(content);
    self.tiling.split(dir);
    self.create_webview_for_pane(new_id);
    self.sync_webview_bounds();
}
```

The `max_panels` configuration value caps the total number of panes that can
exist simultaneously:

```rust
if self.tiling.pane_count() >= max {
    tracing::warn!(max, "NewPane rejected: at panel limit");
    return;
}
```

### Typed splits

`split_with(direction, kind, title)` creates a pane with a specific
`PaneKind` instead of the default `Terminal`:

```rust
// Open a Chat pane beside the current one
self.tiling.split_with(Direction::Horizontal, PaneKind::Chat, "Chat");
```

### ASCII diagram: split operation

```
Before split_at(1, 2, Horizontal):
    Leaf(1)

After:
    Split(H, 0.5)
     /         \
  Leaf(1)    Leaf(2)

Before split_at(2, 3, Vertical):
    Split(H, 0.5)
     /         \
  Leaf(1)    Leaf(2)

After:
    Split(H, 0.5)
     /              \
  Leaf(1)     Split(V, 0.5)
               /         \
            Leaf(2)    Leaf(3)
```

---

## Focus Management

### Focus tracking

The `TilingManager` stores a single `focused: u32` field that identifies the
currently focused pane. The focused pane receives keyboard input and is
highlighted with visual indicators (borders, opacity).

### Focus operations

| Method | Behaviour |
|---|---|
| `focus_next()` | Move focus to the next pane in depth-first order, wrapping around from last to first. |
| `focus_prev()` | Move focus to the previous pane, wrapping from first to last. |
| `focus_direction(dir)` | Move focus to the neighbor in a specific direction (Horizontal = right, Vertical = up). Does not wrap. |
| `focus_pane(id)` | Focus a specific pane by its numeric ID. Returns `false` if the ID does not exist. |

### Focus indicators

The `PaneBorder` struct in the renderer carries an `is_focused` flag:

```rust
pub struct PaneBorder {
    pub rect: Rect,
    pub color: [f32; 4],
    pub width: f32,
    pub is_focused: bool,
}
```

When `LayoutConfig.inactive_opacity < 1.0`, unfocused panes are rendered with
reduced opacity. The `LayoutConfig.border_width` setting controls whether
pane borders are visible at all (default `0.0` means no border).

### Focus flow in the application

When the user triggers a focus action (via keybind), the dispatch handler:

1. Calls the appropriate `TilingManager` focus method.
2. Calls `notify_focus_changed()` to inform webviews of the new focus state.
3. Sets `needs_redraw = true` to refresh the UI.

---

## Zoom Mode

Zoom mode makes a single pane fill the entire content area, hiding all other
panes.

### Behaviour

- **Toggle on:** `zoom_toggle()` sets `zoomed = Some(focused_id)`.
- **Toggle off:** calling `zoom_toggle()` again sets `zoomed = None`.
- Zooming requires at least 2 panes; a single-pane window cannot zoom.
- Any split operation automatically cancels zoom.
- Closing a pane cancels zoom.

### Layout impact

When zoomed, `compute_layout()` short-circuits:

```rust
if let Some(zoomed_id) = self.zoomed {
    return vec![(zoomed_id, viewport)];
}
```

This means only the zoomed pane's webview is positioned; all other webviews
remain at their previous positions (off-screen or behind the zoomed pane,
depending on the platform).

---

## Resizing

Pane sizes are controlled by the `ratio` field on `Split` nodes. Two resize
mechanisms are available: keyboard and mouse drag.

### Keyboard resize

The `ResizePane { direction, delta }` action maps to `TilingCommand::Resize`:

```rust
Action::ResizePane { direction, delta } => {
    let tiling_dir = match direction {
        Left | Right => Direction::Horizontal,
        Up | Down    => Direction::Vertical,
    };
    let signed_delta = match direction {
        Right | Down =>  delta,
        Left  | Up   => -delta,
    };
    self.tiling.execute(TilingCommand::Resize(tiling_dir, signed_delta));
}
```

Inside `TilingManager::resize()`, each step adjusts the ratio by 5%:

```rust
let delta_f = delta as f64 * 0.05; // 5% per step
self.tree.adjust_ratio(self.focused, delta_f)
```

The ratio is clamped to `[0.1, 0.9]`, preventing any pane from being resized
to invisibility.

### Mouse drag resize

Mouse drag resize is a three-phase interaction implemented across `resize_drag.rs`,
`borders.rs`, and `event_handler.rs`.

#### Phase 1: Hit testing (cursor move)

On every cursor movement, the system:

1. Computes all `SplitBorder` entries from the current tree and viewport using
   `compute_borders()`.
2. Hit-tests the cursor position against each border using a 6-pixel hit zone
   on each side.
3. Changes the cursor icon to `ColResize` (vertical divider) or `RowResize`
   (horizontal divider) when hovering a border.

#### Phase 2: Drag start (mouse button press)

When the user presses the mouse button over a border:

```rust
self.drag_state = Some(DragState {
    border: border.clone(),
    start_pos,  // x for horizontal splits, y for vertical
});
```

#### Phase 3: Drag update (cursor move while dragging)

While `drag_state` is `Some`, each cursor movement:

1. Computes the pixel delta from the drag start position.
2. Converts pixels to a ratio delta using `pixel_to_ratio()`:
   `ratio_delta = pixel_delta / span_of_split_bounds`
3. Calls `adjust_ratio_between(first_pane, second_pane, ratio_delta)` to update
   the correct split node.
4. Updates `start_pos` to the current position for incremental dragging.
5. Calls `sync_webview_bounds()` to reposition webviews.

#### Phase 4: Drag end (mouse button release)

```rust
self.drag_state = None;
// Reset cursor to default
```

### SplitBorder structure

```rust
pub struct SplitBorder {
    pub direction: Direction,  // Which axis the split divides
    pub position: f64,         // Pixel position of the divider line
    pub start: f64,            // Start of the divider span
    pub end: f64,              // End of the divider span
    pub first_pane: u32,       // A pane ID from the first subtree
    pub second_pane: u32,      // A pane ID from the second subtree
    pub bounds: Rect,          // Bounding rect of the entire split region
}
```

The `compute_borders()` function walks the tree recursively, producing one
`SplitBorder` per interior `Split` node. Each border records the pane IDs on
either side so the correct split can be identified for ratio adjustment.

---

## Swap Panes

The `SwapPane(direction)` action exchanges the focused pane's position with
its neighbor in the given direction:

```rust
pub fn swap(&mut self, direction: Direction) -> bool {
    if let Some(neighbor) = self.tree.find_neighbor(self.focused, direction) {
        self.tree.swap_panes(self.focused, neighbor)
    } else {
        false
    }
}
```

`swap_panes()` traverses all leaf nodes and swaps the two IDs in place. The
tree structure (split directions and ratios) is preserved; only the pane ID
assignments change.

```
Before swap_panes(1, 2):
    Split(H, 0.5)
     /         \
  Leaf(1)    Leaf(2)

After:
    Split(H, 0.5)
     /         \
  Leaf(2)    Leaf(1)
```

---

## Pane Stacks (Tabs)

A single leaf position in the tree can host multiple panes as a **stack**
(tabbed interface). Only the active pane in the stack is visible; the others
are preserved in memory.

### PaneStack

```rust
pub struct PaneStack {
    panes: Vec<u32>,       // Ordered list of pane IDs
    active_index: usize,   // Index of the visible pane
}
```

### Stack operations

| Method | Description |
|---|---|
| `push(pane_id)` | Add a pane to the stack and make it active. |
| `remove(pane_id)` | Remove a pane. If it was active, the previous pane becomes active. Cannot remove the last pane. |
| `cycle_next()` | Advance to the next tab, wrapping around. |
| `cycle_prev()` | Go to the previous tab, wrapping around. |
| `set_active(pane_id)` | Activate a specific tab by ID. |
| `active()` | Get the currently visible pane ID. |
| `contains(pane_id)` | Check if a pane is in this stack. |
| `pane_ids()` | Get all pane IDs in insertion order. |

### TilingManager integration

- `push_to_stack(kind, title)` -- Adds a new pane of the given kind to the
  stack at the focused leaf position.
- `cycle_stack_next()` / `cycle_stack_prev()` -- Cycle tabs on the focused leaf.

---

## UI Chrome: Tab Bar and Status Bar

UI chrome elements -- the tab bar and status bar -- are managed by the
`UiChrome` struct in the `jarvis-renderer` crate. These elements surround the
content area and affect the viewport available for tiling.

### UiChrome structure

```rust
pub struct UiChrome {
    pub tab_bar: Option<TabBar>,
    pub status_bar: Option<StatusBar>,
    pub borders: Vec<PaneBorder>,
    pub pane_gap: f32,
}
```

### Tab bar

The tab bar sits at the top of the window, with a default height of **32
pixels**.

```rust
pub struct TabBar {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub height: f32,        // Default: 32.0
}

pub struct Tab {
    pub pane_id: u32,
    pub title: String,
    pub is_active: bool,
}
```

Each tab maps to a pane ID. The active tab index is clamped to the valid range
when set.

### Status bar

The status bar sits at the bottom of the window, with a default height of
**24 pixels**.

```rust
pub struct StatusBar {
    pub left_text: String,
    pub center_text: String,
    pub right_text: String,
    pub height: f32,        // Default: 24.0
    pub bg_color: [f32; 4], // Default: dark gray (0.1, 0.1, 0.1, 0.9)
    pub fg_color: [f32; 4], // Default: light gray (0.9, 0.9, 0.9, 1.0)
}
```

The status bar provides three text regions (left, center, right) for displaying
mode information, file paths, cursor position, or other status data.

### Content rect computation

The `content_rect()` method computes the rectangle available for tiling content
after subtracting chrome elements:

```rust
pub fn content_rect(&self, window_width: f32, window_height: f32) -> Rect {
    let top = tab_bar.height or 0.0;
    let bottom = status_bar.height or 0.0;
    Rect {
        x: 0.0,
        y: top,
        width: window_width,
        height: (window_height - top - bottom).max(0.0),
    }
}
```

### ASCII diagram: chrome layout

```
+========================================+
|  Tab Bar (32px)   [Tab1] [Tab2] [Tab3] |
+========================================+
|                                        |
|        Content Area (tiling)           |
|                                        |
|    +------------------+-----------+    |
|    |                  |           |    |
|    |    Pane 1        |  Pane 2   |    |
|    |                  |           |    |
|    +------------------+-----------+    |
|                                        |
+========================================+
|  Status Bar (24px)  [left] [ctr] [rgt] |
+========================================+
```

---

## Webview Bounds Synchronisation

Each pane in Jarvis corresponds to a `wry` webview that needs its position and
size updated whenever the layout changes. This is handled by
`sync_webview_bounds()` in the application state.

### The sync pipeline

```
User action (split, resize, zoom, etc.)
     |
     v
TilingManager updates the SplitNode tree
     |
     v
sync_webview_bounds() is called
     |
     v
1. Get logical window size (physical / scale_factor)
     |
     v
2. Compute tiling viewport via tiling_viewport()
   - Uses content_rect() if chrome (tab/status bar) exists
   - Falls back to config-based offsets (titlebar, status bar height)
     |
     v
3. compute_layout(viewport) -> Vec<(pane_id, Rect)>
   - If zoomed: returns only the zoomed pane filling the viewport
   - Otherwise: recursively computes all pane rects from the split tree
     |
     v
4. For each (pane_id, rect):
   - Convert Rect to wry::Rect via tiling_rect_to_wry()
   - Call handle.set_bounds(wry_rect)
```

### Coordinate conversion

The tiling engine uses `f64` logical coordinates. These are converted to wry's
`LogicalPosition` and `LogicalSize`:

```rust
pub fn tiling_rect_to_wry(rect: &Rect) -> wry::Rect {
    wry::Rect {
        position: Position::Logical(LogicalPosition::new(rect.x, rect.y)),
        size: Size::Logical(LogicalSize::new(rect.width, rect.height)),
    }
}
```

### When sync is triggered

`sync_webview_bounds()` is called after:

- `NewPane` (split + create webview)
- `ClosePane` (remove pane + destroy webview)
- `SplitHorizontal` / `SplitVertical`
- `ZoomPane` (toggle zoom)
- `ResizePane` (keyboard resize)
- `SwapPane`
- `OpenSettings` / `OpenChat` (typed splits)
- Mouse drag resize (on every cursor move during drag)
- Window resize events

---

## TilingManager: Full API Reference

The `TilingManager` is the primary interface for the tiling subsystem. It
coordinates the split tree, pane registry, focus state, zoom, and stacks.

### State

```rust
pub struct TilingManager {
    tree: SplitNode,                    // Split tree root
    panes: HashMap<u32, Pane>,          // Pane registry
    stacks: HashMap<u32, PaneStack>,    // Tab stacks at leaf positions
    focused: u32,                       // Currently focused pane ID
    zoomed: Option<u32>,                // Zoomed pane (if any)
    layout_engine: LayoutEngine,        // Gap, padding, min size
    next_id: u32,                       // Auto-incrementing ID counter
}
```

### Initialisation

- `TilingManager::new()` -- Creates a manager with one terminal pane (ID = 1).
- `TilingManager::with_layout(engine)` -- Same, with a custom layout engine.
- `TilingManager::default()` -- Alias for `new()`.

### Accessors

| Method | Returns |
|---|---|
| `focused_id()` | The focused pane's numeric ID. |
| `is_zoomed()` | Whether zoom mode is active. |
| `zoomed_id()` | `Some(id)` of the zoomed pane, or `None`. |
| `pane_count()` | Total number of panes across all stacks. |
| `pane(id)` | `Option<&Pane>` for a given ID. |
| `pane_mut(id)` | `Option<&mut Pane>` for a given ID. |
| `tree()` | `&SplitNode` reference to the tree root. |
| `tree_mut()` | `&mut SplitNode` mutable reference. |
| `gap()` | Current inter-pane gap in pixels. |
| `outer_padding()` | Current outer padding in pixels. |
| `stack(leaf_id)` | `Option<&PaneStack>` at a leaf. |
| `panes_by_kind(kind)` | All pane IDs matching a specific `PaneKind`. |
| `ordered_pane_ids()` | Pane IDs in depth-first order (visual order). |

### Commands

The `execute(cmd: TilingCommand) -> bool` method dispatches any tiling command:

```rust
pub enum TilingCommand {
    SplitHorizontal,
    SplitVertical,
    Close,
    Resize(Direction, i32),
    Swap(Direction),
    FocusNext,
    FocusPrev,
    FocusDirection(Direction),
    Zoom,
}
```

---

## Platform Window Management

The `platform` module provides a `WindowManager` trait for controlling external
application windows. This is used by the `ExternalApp` pane kind to capture and
tile third-party windows within the Jarvis workspace.

```rust
pub trait WindowManager: Send + Sync {
    fn list_windows(&self) -> Result<Vec<ExternalWindow>>;
    fn set_window_frame(&self, window_id: WindowId, frame: Rect) -> Result<()>;
    fn focus_window(&self, window_id: WindowId) -> Result<()>;
    fn set_minimized(&self, window_id: WindowId, minimized: bool) -> Result<()>;
    fn watch_windows(&self, callback: Box<dyn Fn(WindowEvent) + Send>) -> Result<WatchHandle>;
}
```

Platform implementations:

- **macOS**: CoreGraphics-based window management (`MacOsWindowManager`).
- **Windows / Linux**: Stub implementations; `create_window_manager()` returns
  the `NoopWindowManager` on non-macOS platforms.

---

## Layout Engine Internals (Developer Reference)

This section is for developers working on the tiling subsystem.

### Adding a new split direction or layout mode

The layout engine is designed around the binary split tree. To add a new layout
mode (e.g., spiral, grid):

1. Either extend `SplitNode` with a new variant, or add a separate layout
   strategy enum at the `TilingManager` level.
2. Implement the `compute` method for your new layout in `layout/calculation.rs`.
3. Update `compute_borders()` in `layout/borders.rs` if the new layout has
   resizable dividers.

### Ratio constraints

All ratios are clamped to `[0.1, 0.9]` in `adjust_ratio()` and
`adjust_ratio_between()`. This ensures no pane can be collapsed below 10% of
the parent split's extent. The `min_pane_size` field (default 50px) on
`LayoutEngine` is available for future enforcement but is not currently
applied during computation.

### ID allocation

Pane IDs are allocated by a simple auto-incrementing counter (`next_id`) in
`TilingManager`. IDs are never reused within a session. This simplifies
bookkeeping because webview handles, PTY processes, and other resources can
use the pane ID as a stable key.

### Serialisation

Both `SplitNode` and `PaneStack` derive `Serialize` and `Deserialize` (via
serde), enabling session state to be saved and restored. `Direction` is also
serialisable.

### Testing

The tiling crate has comprehensive unit tests covering:

- Tree operations: split, remove, swap, ratio adjustment, traversal.
- Layout computation: single pane, splits with gap, outer padding, nested
  splits.
- Border computation and hit testing: border positions, hit zones, ratio
  conversion.
- Stack operations: push, remove, cycle, serialisation round-trip.
- Manager integration: split/close lifecycle, focus cycling, zoom toggle,
  command dispatch, typed splits, multi-split scenarios.

Run the tiling tests with:

```
cargo test -p jarvis-tiling
```
