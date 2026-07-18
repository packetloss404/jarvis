# Backlog

## Portfolio audit backlog — 2026-07-17
_Findings from a 2026-07-17 code audit, preserved for later._

### Later / deferred
- **[med/M]** workspace_capture linux.rs is still a stub (both fns return 'workspace streaming on Linux is not implemented yet'); macOS (core-graphics) and Windows (xcap) are fully implemented
  - Fix: Port windows.rs's xcap Monitor capture+crop+resize+jpeg body into jarvis-app/src/app_state/workspace_capture/linux.rs (xcap is cross-platform; body copies near-verbatim), and move the xcap='0.9' dep from the [target.'cfg(windows)'] block to cfg(any(windows,linux)) in jarvis-app/Cargo.toml. Needs real Linux (X11/Wayland) testing — Wayland requires pipewire portal.
