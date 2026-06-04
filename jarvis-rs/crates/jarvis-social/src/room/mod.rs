//! Relay Room transport for presence.
//!
//! A thin WebSocket client that speaks the relay's symmetric **Room** protocol
//! (`room_hello` → `room_ready` + `member_joined`* + `member_count`, then opaque
//! text fan-out). This replaces the old Supabase/Phoenix `realtime` transport.
//!
//! The client is intentionally generic: it knows nothing about presence
//! semantics. It surfaces relay control frames (`member_joined`/`member_left`/
//! `member_count`) and every opaque text frame as [`RoomEvent`]s, and accepts
//! opaque text frames to broadcast via [`RoomClient::send`]. Presence-specific
//! framing lives in [`crate::presence`].

mod client;
mod protocol;

pub use client::RoomClient;
pub use protocol::{RoomConfig, RoomControl, RoomEvent};
