//! Background task that translates relay [`RoomEvent`]s into [`PresenceEvent`]s
//! and maintains the online-user roster.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, RwLock};
use tracing::debug;

use crate::identity::Identity;
use crate::protocol::{OnlineUser, PresenceFrame, UserStatus};
use crate::room::RoomEvent;

use super::types::PresenceEvent;

type Roster = Arc<RwLock<HashMap<String, OnlineUser>>>;

// ---------------------------------------------------------------------------
// Event Translator
// ---------------------------------------------------------------------------

/// Translate relay Room events into presence events.
///
/// Roster maintenance:
/// - On `Ready` we announce our own presence frame and seed the roster with
///   ourselves.
/// - On `MemberJoined` we re-announce so the newcomer learns our entry. (We do
///   not synthesize a roster entry for the joiner here — we wait for their
///   `presence` frame, which carries their display name / status.)
/// - On `MemberLeft` we drop the member (keyed by user id == member id) and
///   emit `UserOffline`.
/// - Incoming `presence` / `activity_update` frames insert/update roster
///   entries and emit `UserOnline` / `ActivityChanged`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn event_translator(
    mut room_rx: mpsc::Receiver<RoomEvent>,
    event_tx: mpsc::Sender<PresenceEvent>,
    online_users: Roster,
    connected: Arc<RwLock<bool>>,
    self_activity: Arc<RwLock<(UserStatus, Option<String>)>>,
    identity: Identity,
    announce_tx: mpsc::Sender<String>,
) {
    let our_user_id = identity.user_id.clone();

    while let Some(event) = room_rx.recv().await {
        match event {
            RoomEvent::Ready { .. } => {
                *connected.write().await = true;

                // Seed the roster with ourselves and announce.
                let frame = announce_self(&identity, &self_activity, &online_users).await;
                let _ = announce_tx.send(frame).await;

                let count = online_users.read().await.len() as u32;
                let _ = event_tx
                    .send(PresenceEvent::Connected {
                        online_count: count,
                    })
                    .await;
            }
            RoomEvent::MemberJoined { member_id } => {
                // A peer joined (or the relay replayed an existing member).
                // Re-announce so they learn our roster entry.
                debug!(member = %member_id, "presence member joined");
                let frame = announce_self(&identity, &self_activity, &online_users).await;
                let _ = announce_tx.send(frame).await;
            }
            RoomEvent::MemberLeft { member_id } => {
                let display_name = {
                    let mut users = online_users.write().await;
                    let removed = users.remove(&member_id);
                    removed
                        .map(|u| u.display_name)
                        .unwrap_or_else(|| "Unknown".to_string())
                };
                let _ = event_tx
                    .send(PresenceEvent::UserOffline {
                        user_id: member_id,
                        display_name,
                    })
                    .await;
            }
            RoomEvent::MemberCount { count } => {
                debug!(count, "presence member_count");
            }
            RoomEvent::Frame(text) => {
                handle_frame(&text, &online_users, &event_tx, &our_user_id).await;
            }
            RoomEvent::Disconnected => {
                *connected.write().await = false;
                online_users.write().await.clear();
                let _ = event_tx.send(PresenceEvent::Disconnected).await;
            }
            RoomEvent::RelayError(msg) => {
                let _ = event_tx.send(PresenceEvent::Error(msg)).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build our own presence frame, seed the roster with ourselves, and return
/// the serialized frame ready to broadcast.
async fn announce_self(
    identity: &Identity,
    self_activity: &Arc<RwLock<(UserStatus, Option<String>)>>,
    online_users: &Roster,
) -> String {
    let (status, activity) = self_activity.read().await.clone();
    let me = OnlineUser {
        user_id: identity.user_id.clone(),
        display_name: identity.display_name.clone(),
        status,
        activity,
    };
    online_users
        .write()
        .await
        .insert(me.user_id.clone(), me.clone());
    serde_json::to_string(&PresenceFrame::Presence { user: me })
        .unwrap_or_else(|_| String::from("{}"))
}

/// Parse and dispatch one opaque presence frame.
async fn handle_frame(
    text: &str,
    online_users: &Roster,
    event_tx: &mpsc::Sender<PresenceEvent>,
    our_user_id: &str,
) {
    let frame = match serde_json::from_str::<PresenceFrame>(text) {
        Ok(f) => f,
        Err(e) => {
            debug!(error = %e, "unparseable presence frame");
            return;
        }
    };

    match frame {
        PresenceFrame::Presence { user } => {
            if user.user_id == our_user_id {
                return; // never echo ourselves into UI events
            }
            let is_new = {
                let mut users = online_users.write().await;
                let is_new = !users.contains_key(&user.user_id);
                users.insert(user.user_id.clone(), user.clone());
                is_new
            };
            if is_new {
                let _ = event_tx.send(PresenceEvent::UserOnline(user)).await;
            } else {
                let _ = event_tx.send(PresenceEvent::ActivityChanged(user)).await;
            }
        }
        PresenceFrame::ActivityUpdate(p) => {
            if p.user_id == our_user_id {
                return;
            }
            let user = OnlineUser {
                user_id: p.user_id.clone(),
                display_name: p.display_name,
                status: p.status,
                activity: p.activity,
            };
            let is_new = {
                let mut users = online_users.write().await;
                let is_new = !users.contains_key(&user.user_id);
                users.insert(user.user_id.clone(), user.clone());
                is_new
            };
            let event = if is_new {
                PresenceEvent::UserOnline(user)
            } else {
                PresenceEvent::ActivityChanged(user)
            };
            let _ = event_tx.send(event).await;
        }
        PresenceFrame::GameInvite(p) => {
            if p.user_id == our_user_id {
                return;
            }
            let _ = event_tx
                .send(PresenceEvent::GameInvite {
                    user_id: p.user_id,
                    display_name: p.display_name,
                    game: p.game,
                    code: p.code,
                })
                .await;
        }
        PresenceFrame::Poke(p) => {
            // Directed: only surface pokes aimed at us.
            if p.target_user_id == our_user_id {
                let _ = event_tx
                    .send(PresenceEvent::Poked {
                        user_id: p.user_id,
                        display_name: p.display_name,
                    })
                    .await;
            }
        }
        PresenceFrame::ChatMessage(p) => {
            if p.user_id == our_user_id {
                return;
            }
            let _ = event_tx
                .send(PresenceEvent::ChatMessage {
                    user_id: p.user_id,
                    display_name: p.display_name,
                    channel: p.channel,
                    content: p.content,
                })
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{GameInvitePayload, PokePayload};

    fn roster() -> Roster {
        Arc::new(RwLock::new(HashMap::new()))
    }

    fn chan() -> (mpsc::Sender<PresenceEvent>, mpsc::Receiver<PresenceEvent>) {
        mpsc::channel(16)
    }

    fn presence_frame(user_id: &str, name: &str) -> String {
        serde_json::to_string(&PresenceFrame::Presence {
            user: OnlineUser {
                user_id: user_id.into(),
                display_name: name.into(),
                status: UserStatus::Online,
                activity: None,
            },
        })
        .unwrap()
    }

    #[tokio::test]
    async fn presence_frame_adds_user_and_emits_online() {
        let users = roster();
        let (tx, mut rx) = chan();
        handle_frame(&presence_frame("u2", "Bob"), &users, &tx, "u1").await;

        assert!(users.read().await.contains_key("u2"));
        match rx.recv().await.unwrap() {
            PresenceEvent::UserOnline(u) => {
                assert_eq!(u.user_id, "u2");
                assert_eq!(u.display_name, "Bob");
            }
            other => panic!("expected UserOnline, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn second_presence_frame_emits_activity_changed() {
        let users = roster();
        let (tx, mut rx) = chan();
        handle_frame(&presence_frame("u2", "Bob"), &users, &tx, "u1").await;
        let _ = rx.recv().await; // UserOnline

        // Re-announce with a different activity → ActivityChanged, not UserOnline.
        let frame = serde_json::to_string(&PresenceFrame::Presence {
            user: OnlineUser {
                user_id: "u2".into(),
                display_name: "Bob".into(),
                status: UserStatus::InGame,
                activity: Some("chess".into()),
            },
        })
        .unwrap();
        handle_frame(&frame, &users, &tx, "u1").await;
        match rx.recv().await.unwrap() {
            PresenceEvent::ActivityChanged(u) => {
                assert_eq!(u.status, UserStatus::InGame);
                assert_eq!(u.activity.as_deref(), Some("chess"));
            }
            other => panic!("expected ActivityChanged, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn own_presence_frame_is_ignored() {
        let users = roster();
        let (tx, mut rx) = chan();
        // A frame whose user_id == ours must not produce a UI event.
        handle_frame(&presence_frame("u1", "Me"), &users, &tx, "u1").await;
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn poke_only_surfaces_when_targeted_at_us() {
        let users = roster();
        let (tx, mut rx) = chan();

        let to_other = serde_json::to_string(&PresenceFrame::Poke(PokePayload {
            user_id: "u2".into(),
            display_name: "Bob".into(),
            target_user_id: "someone_else".into(),
        }))
        .unwrap();
        handle_frame(&to_other, &users, &tx, "u1").await;
        assert!(rx.try_recv().is_err(), "poke for others must be filtered");

        let to_us = serde_json::to_string(&PresenceFrame::Poke(PokePayload {
            user_id: "u2".into(),
            display_name: "Bob".into(),
            target_user_id: "u1".into(),
        }))
        .unwrap();
        handle_frame(&to_us, &users, &tx, "u1").await;
        match rx.recv().await.unwrap() {
            PresenceEvent::Poked { display_name, .. } => assert_eq!(display_name, "Bob"),
            other => panic!("expected Poked, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn game_invite_is_surfaced() {
        let users = roster();
        let (tx, mut rx) = chan();
        let frame = serde_json::to_string(&PresenceFrame::GameInvite(GameInvitePayload {
            user_id: "u2".into(),
            display_name: "Bob".into(),
            game: "tetris".into(),
            code: Some("XYZ".into()),
        }))
        .unwrap();
        handle_frame(&frame, &users, &tx, "u1").await;
        match rx.recv().await.unwrap() {
            PresenceEvent::GameInvite { game, code, .. } => {
                assert_eq!(game, "tetris");
                assert_eq!(code.as_deref(), Some("XYZ"));
            }
            other => panic!("expected GameInvite, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn member_left_removes_from_roster() {
        // Drive the full translator with a MemberLeft after seeding a user.
        let users = roster();
        users.write().await.insert(
            "u2".into(),
            OnlineUser {
                user_id: "u2".into(),
                display_name: "Bob".into(),
                status: UserStatus::Online,
                activity: None,
            },
        );

        let (room_tx, room_rx) = mpsc::channel(8);
        let (ev_tx, mut ev_rx) = chan();
        let (announce_tx, _announce_rx) = mpsc::channel(8);
        let connected = Arc::new(RwLock::new(true));
        let self_activity = Arc::new(RwLock::new((UserStatus::Online, None)));
        let identity = Identity::generate("me");
        let users_clone = Arc::clone(&users);

        let handle = tokio::spawn(async move {
            event_translator(
                room_rx,
                ev_tx,
                users_clone,
                connected,
                self_activity,
                identity,
                announce_tx,
            )
            .await;
        });

        room_tx
            .send(RoomEvent::MemberLeft {
                member_id: "u2".into(),
            })
            .await
            .unwrap();

        match ev_rx.recv().await.unwrap() {
            PresenceEvent::UserOffline { user_id, .. } => assert_eq!(user_id, "u2"),
            other => panic!("expected UserOffline, got {other:?}"),
        }
        assert!(!users.read().await.contains_key("u2"));

        drop(room_tx);
        let _ = handle.await;
    }
}
