//! Presence client backed by the relay Room transport.
//!
//! Joins one global presence Room, tracks the online-user roster, broadcasts
//! activity updates, and receives pokes / game invites / activity from other
//! users. The transport layer is handled by [`crate::room::RoomClient`].

mod client;
mod event_translator;
mod helpers;
mod types;

pub use client::PresenceClient;
pub use types::{PresenceConfig, PresenceEvent, DEFAULT_PRESENCE_ROOM_ID};
