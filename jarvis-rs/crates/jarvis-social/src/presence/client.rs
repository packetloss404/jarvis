//! Presence client backed by the project's relay Room transport.
//!
//! Joins one global presence Room (`member_id` = the desktop's stable user id)
//! and maps presence semantics onto opaque Room frames ([`PresenceFrame`]).
//! The public surface (`start` / `update_activity` / `send_invite` /
//! `send_poke` / `send_chat` / `online_users` / `disconnect`) is stable, so the
//! app layer needs no changes.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, RwLock};

use crate::identity::Identity;
use crate::protocol::{
    ActivityUpdatePayload, ChatMessagePayload, GameInvitePayload, OnlineUser, PokePayload,
    PresenceFrame, UserStatus,
};
use crate::room::{RoomClient, RoomConfig};

use super::event_translator::event_translator;
use super::types::{PresenceConfig, PresenceEvent};

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Presence client that maintains a connection to the relay presence Room.
pub struct PresenceClient {
    config: PresenceConfig,
    identity: Identity,
    /// Current list of online users (keyed by user id).
    online_users: Arc<RwLock<HashMap<String, OnlineUser>>>,
    /// Our last-announced activity, re-broadcast when a new member joins.
    self_activity: Arc<RwLock<(UserStatus, Option<String>)>>,
    /// Handle to the relay Room transport.
    room: Option<RoomClient>,
    /// Whether we're currently connected (registered in the room).
    connected: Arc<RwLock<bool>>,
}

impl PresenceClient {
    pub fn new(identity: Identity, config: PresenceConfig) -> Self {
        Self {
            config,
            identity,
            online_users: Arc::new(RwLock::new(HashMap::new())),
            self_activity: Arc::new(RwLock::new((UserStatus::Online, None))),
            room: None,
            connected: Arc::new(RwLock::new(false)),
        }
    }

    /// Our own roster entry as a presence frame, reflecting current activity.
    async fn self_presence_frame(&self) -> PresenceFrame {
        let (status, activity) = self.self_activity.read().await.clone();
        PresenceFrame::Presence {
            user: OnlineUser {
                user_id: self.identity.user_id.clone(),
                display_name: self.identity.display_name.clone(),
                status,
                activity,
            },
        }
    }

    /// Start the presence connection. Returns a receiver for presence events.
    /// The connection runs in a background task with auto-reconnect.
    pub fn start(&mut self) -> mpsc::Receiver<PresenceEvent> {
        let (event_tx, event_rx) = mpsc::channel(256);

        let room_config = RoomConfig {
            relay_url: self.config.relay_url.clone(),
            session_id: self.config.room_id.clone(),
            member_id: self.identity.user_id.clone(),
            reconnect_delay_secs: self.config.reconnect_delay,
            max_reconnect_delay_secs: self.config.max_reconnect_delay,
        };

        let (room, room_event_rx) = RoomClient::connect(room_config);

        let online_users = Arc::clone(&self.online_users);
        let connected = Arc::clone(&self.connected);
        let self_activity = Arc::clone(&self.self_activity);
        let identity = self.identity.clone();

        // The translator needs to send our own presence frame on join /
        // member_joined; give it a sender into the room.
        let announce_tx = room.frame_sender();

        tokio::spawn(async move {
            event_translator(
                room_event_rx,
                event_tx,
                online_users,
                connected,
                self_activity,
                identity,
                announce_tx,
            )
            .await;
        });

        self.room = Some(room);
        event_rx
    }

    /// Update activity status.
    pub async fn update_activity(&self, status: UserStatus, activity: Option<String>) {
        *self.self_activity.write().await = (status, activity.clone());

        // Reflect our own status locally so the UI updates immediately.
        if let Some(u) = self
            .online_users
            .write()
            .await
            .get_mut(&self.identity.user_id)
        {
            u.status = status;
            u.activity = activity.clone();
        }

        if let Some(room) = &self.room {
            let frame = PresenceFrame::ActivityUpdate(ActivityUpdatePayload {
                user_id: self.identity.user_id.clone(),
                display_name: self.identity.display_name.clone(),
                status,
                activity,
            });
            send_frame(room, &frame).await;
            // Also send a `presence` frame so the canonical roster entry stays
            // in sync for members that only track presence frames.
            send_frame(room, &self.self_presence_frame().await).await;
        }
    }

    /// Send a game invite.
    pub async fn send_invite(&self, game: &str, code: Option<String>) {
        if let Some(room) = &self.room {
            let frame = PresenceFrame::GameInvite(GameInvitePayload {
                user_id: self.identity.user_id.clone(),
                display_name: self.identity.display_name.clone(),
                game: game.to_string(),
                code,
            });
            send_frame(room, &frame).await;
        }
    }

    /// Poke a user.
    pub async fn send_poke(&self, target_user_id: &str) {
        if let Some(room) = &self.room {
            let frame = PresenceFrame::Poke(PokePayload {
                user_id: self.identity.user_id.clone(),
                display_name: self.identity.display_name.clone(),
                target_user_id: target_user_id.to_string(),
            });
            send_frame(room, &frame).await;
        }
    }

    /// Send a chat message to a channel.
    pub async fn send_chat(&self, channel: &str, content: &str, reply_to: Option<String>) {
        if let Some(room) = &self.room {
            let frame = PresenceFrame::ChatMessage(ChatMessagePayload {
                user_id: self.identity.user_id.clone(),
                display_name: self.identity.display_name.clone(),
                channel: channel.to_string(),
                content: content.to_string(),
                timestamp: super::helpers::chrono_now(),
                reply_to,
            });
            send_frame(room, &frame).await;
        }
    }

    /// Disconnect from the presence server.
    pub async fn disconnect(&self) {
        if let Some(room) = &self.room {
            room.disconnect().await;
        }
    }

    /// Get the current list of online users.
    pub async fn online_users(&self) -> Vec<OnlineUser> {
        self.online_users.read().await.values().cloned().collect()
    }

    /// Check if connected.
    pub async fn is_connected(&self) -> bool {
        *self.connected.read().await
    }

    /// Get our identity.
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
}

/// Serialize and queue a presence frame for broadcast to the room.
async fn send_frame(room: &RoomClient, frame: &PresenceFrame) {
    if let Ok(json) = serde_json::to_string(frame) {
        room.send(json).await;
    }
}
