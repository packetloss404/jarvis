//! Session store: maps session IDs to paired desktop/mobile channels.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{mpsc, RwLock};

/// Role of a connected client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Desktop,
    Mobile,
    Host,
    Spectator,
    Member,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Bridge,
    Broadcast,
    Room,
}

impl Role {
    pub fn session_kind(self) -> SessionKind {
        match self {
            Role::Desktop | Role::Mobile => SessionKind::Bridge,
            Role::Host | Role::Spectator => SessionKind::Broadcast,
            Role::Member => SessionKind::Room,
        }
    }
}

pub struct BridgeSession {
    pub desktop_tx: Option<mpsc::Sender<String>>,
    pub mobile_tx: Option<mpsc::Sender<String>>,
    pub created_at: Instant,
}

pub struct BroadcastSession {
    pub host_tx: Option<mpsc::Sender<String>>,
    pub spectator_txs: Vec<mpsc::Sender<String>>,
    pub created_at: Instant,
}

/// One participant in an N:N room session.
pub struct RoomMember {
    pub member_id: String,
    pub tx: mpsc::Sender<String>,
}

/// Symmetric N:N session. Every member's frames fan out to all other members.
pub struct RoomSession {
    pub members: Vec<RoomMember>,
    pub created_at: Instant,
}

pub enum Session {
    Bridge(BridgeSession),
    Broadcast(BroadcastSession),
    Room(RoomSession),
}

/// Maximum number of members allowed in a single Room session. Joins beyond
/// this are rejected to bound per-room fan-out cost and memory.
pub const MAX_ROOM_MEMBERS: usize = 32;

pub struct BroadcastRegistration {
    pub host_connected: bool,
}

/// Thread-safe session store.
#[derive(Clone)]
pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Ensure a session exists with the requested kind.
    pub async fn ensure_session(
        &self,
        session_id: &str,
        kind: SessionKind,
    ) -> Result<bool, &'static str> {
        let mut map = self.sessions.write().await;
        if let Some(existing) = map.get(session_id) {
            let existing_kind = match existing {
                Session::Bridge(_) => SessionKind::Bridge,
                Session::Broadcast(_) => SessionKind::Broadcast,
                Session::Room(_) => SessionKind::Room,
            };
            if existing_kind == kind {
                return Ok(false);
            }
            return Err("session kind mismatch");
        }

        let session = match kind {
            SessionKind::Bridge => Session::Bridge(BridgeSession {
                desktop_tx: None,
                mobile_tx: None,
                created_at: Instant::now(),
            }),
            SessionKind::Broadcast => Session::Broadcast(BroadcastSession {
                host_tx: None,
                spectator_txs: Vec::new(),
                created_at: Instant::now(),
            }),
            SessionKind::Room => Session::Room(RoomSession {
                members: Vec::new(),
                created_at: Instant::now(),
            }),
        };

        map.insert(session_id.to_string(), session);
        Ok(true)
    }

    /// Register a bridge client's sender. Returns the peer's sender if already connected.
    pub async fn register_bridge(
        &self,
        session_id: &str,
        role: Role,
        tx: mpsc::Sender<String>,
    ) -> Result<Option<mpsc::Sender<String>>, &'static str> {
        let mut map = self.sessions.write().await;
        let session = match map.get_mut(session_id) {
            Some(Session::Bridge(session)) => session,
            Some(_) => return Err("session kind mismatch"),
            None => return Err("session not found"),
        };

        let peer_tx = match role {
            Role::Desktop => {
                if session.desktop_tx.is_some() {
                    return Err("desktop already connected");
                }
                session.desktop_tx = Some(tx);
                session.mobile_tx.clone()
            }
            Role::Mobile => {
                if session.mobile_tx.is_some() {
                    return Err("mobile already connected");
                }
                session.mobile_tx = Some(tx);
                session.desktop_tx.clone()
            }
            _ => return Err("invalid role for bridge session"),
        };

        Ok(peer_tx)
    }

    pub async fn register_broadcast(
        &self,
        session_id: &str,
        role: Role,
        tx: mpsc::Sender<String>,
    ) -> Result<BroadcastRegistration, &'static str> {
        let mut map = self.sessions.write().await;
        let session = match map.get_mut(session_id) {
            Some(Session::Broadcast(session)) => session,
            Some(_) => return Err("session kind mismatch"),
            None => return Err("session not found"),
        };

        const MAX_SPECTATORS: usize = 100;

        match role {
            Role::Host => {
                if session.host_tx.is_some() {
                    return Err("host already connected");
                }
                session.host_tx = Some(tx);
            }
            Role::Spectator => {
                if session.spectator_txs.len() >= MAX_SPECTATORS {
                    tracing::warn!("Spectator cap reached");
                    return Err("spectator cap reached");
                }
                session.spectator_txs.push(tx);
            }
            _ => return Err("invalid role for broadcast session"),
        }

        Ok(BroadcastRegistration {
            host_connected: session.host_tx.is_some(),
        })
    }

    /// Register a room member's sender. If a member with the same `member_id`
    /// already exists, its `tx` is replaced (reconnect); otherwise a new member
    /// is appended (rejected once the room is at [`MAX_ROOM_MEMBERS`]). The
    /// session must already exist (see `ensure_session`).
    pub async fn register_room(
        &self,
        session_id: &str,
        member_id: String,
        tx: mpsc::Sender<String>,
    ) -> Result<(), &'static str> {
        let mut map = self.sessions.write().await;
        let session = match map.get_mut(session_id) {
            Some(Session::Room(session)) => session,
            Some(_) => return Err("session kind mismatch"),
            None => return Err("session not found"),
        };

        if let Some(existing) = session.members.iter_mut().find(|m| m.member_id == member_id) {
            // Same member_id reconnecting -> replace the channel (does not count
            // against the cap since the member already occupies a slot).
            existing.tx = tx;
        } else {
            if session.members.len() >= MAX_ROOM_MEMBERS {
                return Err("room at capacity");
            }
            session.members.push(RoomMember { member_id, tx });
        }

        Ok(())
    }

    /// Every member's sender EXCEPT the one matching `member_id`, so a member
    /// never receives its own frames.
    pub async fn room_targets_excluding(
        &self,
        session_id: &str,
        member_id: &str,
    ) -> Vec<mpsc::Sender<String>> {
        let map = self.sessions.read().await;
        match map.get(session_id) {
            Some(Session::Room(session)) => session
                .members
                .iter()
                .filter(|m| m.member_id != member_id)
                .map(|m| m.tx.clone())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Current roster of member IDs for a room.
    pub async fn room_member_ids(&self, session_id: &str) -> Vec<String> {
        let map = self.sessions.read().await;
        match map.get(session_id) {
            Some(Session::Room(session)) => {
                session.members.iter().map(|m| m.member_id.clone()).collect()
            }
            _ => Vec::new(),
        }
    }

    /// Number of members currently in a room.
    pub async fn room_member_count(&self, session_id: &str) -> usize {
        let map = self.sessions.read().await;
        match map.get(session_id) {
            Some(Session::Room(session)) => session.members.len(),
            _ => 0,
        }
    }

    /// Remove a member from a room, but ONLY if the stored channel still belongs
    /// to `tx` (the caller's own connection). This guards against a stale
    /// cleanup from a dropped connection evicting a freshly-reconnected member
    /// that reused the same `member_id`: on reconnect `register_room` replaces
    /// the stored `tx`, so the old connection's `tx` no longer matches and its
    /// late cleanup becomes a no-op.
    ///
    /// If the room is now empty, the session is removed and `true` is returned.
    pub async fn unregister_member(
        &self,
        session_id: &str,
        member_id: &str,
        tx: &mpsc::Sender<String>,
    ) -> bool {
        let mut map = self.sessions.write().await;
        if let Some(Session::Room(session)) = map.get_mut(session_id) {
            // Only evict the member if the stored channel is the SAME channel we
            // registered (i.e. it was not replaced by a reconnect).
            let owns_slot = session
                .members
                .iter()
                .any(|m| m.member_id == member_id && m.tx.same_channel(tx));
            if !owns_slot {
                return false;
            }
            session.members.retain(|m| m.member_id != member_id);
            if session.members.is_empty() {
                map.remove(session_id);
                return true;
            }
        }
        false
    }

    /// Get the peer's sender for forwarding.
    pub async fn get_peer_tx(&self, session_id: &str, role: Role) -> Option<mpsc::Sender<String>> {
        let map = self.sessions.read().await;
        let session = match map.get(session_id)? {
            Session::Bridge(session) => session,
            _ => return None,
        };
        match role {
            Role::Desktop => session.mobile_tx.clone(),
            Role::Mobile => session.desktop_tx.clone(),
            _ => None,
        }
    }

    pub async fn broadcast_targets(&self, session_id: &str) -> Vec<mpsc::Sender<String>> {
        let map = self.sessions.read().await;
        match map.get(session_id) {
            Some(Session::Broadcast(session)) => {
                let mut targets = Vec::with_capacity(session.spectator_txs.len() + 1);
                if let Some(host_tx) = &session.host_tx {
                    targets.push(host_tx.clone());
                }
                targets.extend(session.spectator_txs.iter().cloned());
                targets
            }
            _ => Vec::new(),
        }
    }

    pub async fn spectator_targets(&self, session_id: &str) -> Vec<mpsc::Sender<String>> {
        let map = self.sessions.read().await;
        match map.get(session_id) {
            Some(Session::Broadcast(session)) => session.spectator_txs.clone(),
            _ => Vec::new(),
        }
    }

    pub async fn viewer_count(&self, session_id: &str) -> usize {
        let map = self.sessions.read().await;
        match map.get(session_id) {
            Some(Session::Broadcast(session)) => session.spectator_txs.len(),
            _ => 0,
        }
    }

    /// Unregister a client. Returns true if session was removed (both gone).
    pub async fn unregister(&self, session_id: &str, role: Role) -> bool {
        let mut map = self.sessions.write().await;
        if let Some(session) = map.get_mut(session_id) {
            match session {
                Session::Bridge(session) => {
                    match role {
                        Role::Desktop => session.desktop_tx = None,
                        Role::Mobile => session.mobile_tx = None,
                        _ => return false,
                    }
                    if session.desktop_tx.is_none() && session.mobile_tx.is_none() {
                        map.remove(session_id);
                        return true;
                    }
                }
                Session::Broadcast(session) => {
                    match role {
                        Role::Host => session.host_tx = None,
                        Role::Spectator => session.spectator_txs.retain(|tx| !tx.is_closed()),
                        _ => return false,
                    }
                    if session.host_tx.is_none() && session.spectator_txs.is_empty() {
                        map.remove(session_id);
                        return true;
                    }
                }
                // Room sessions use `unregister_member` (keyed by member_id), not
                // the role-based path. Reaching here would be a caller bug.
                Session::Room(_) => return false,
            }
        }
        false
    }

    /// Reap sessions older than `max_age` with no mobile peer.
    pub async fn reap_stale(&self, max_age: std::time::Duration) {
        let mut map = self.sessions.write().await;
        let now = Instant::now();
        map.retain(|id, session| {
            let stale = match session {
                Session::Bridge(session) => {
                    session.mobile_tx.is_none() && now.duration_since(session.created_at) > max_age
                }
                Session::Broadcast(session) => {
                    session.host_tx.is_none()
                        && session.spectator_txs.is_empty()
                        && now.duration_since(session.created_at) > max_age
                }
                Session::Room(session) => {
                    session.members.is_empty()
                        && now.duration_since(session.created_at) > max_age
                }
            };
            if stale {
                tracing::info!(session_id = %id, "Reaping stale session");
            }
            !stale
        });
    }

    /// Check if a session exists.
    pub async fn exists(&self, session_id: &str) -> bool {
        self.sessions.read().await.contains_key(session_id)
    }

    /// Number of active sessions.
    pub async fn count(&self) -> usize {
        self.sessions.read().await.len()
    }
}

#[cfg(test)]
mod room_tests {
    use super::*;

    fn chan() -> (mpsc::Sender<String>, mpsc::Receiver<String>) {
        mpsc::channel::<String>(8)
    }

    #[tokio::test]
    async fn register_room_adds_members() {
        let store = SessionStore::new();
        store.ensure_session("s", SessionKind::Room).await.unwrap();

        let (a_tx, _a_rx) = chan();
        let (b_tx, _b_rx) = chan();
        store.register_room("s", "a".into(), a_tx).await.unwrap();
        store.register_room("s", "b".into(), b_tx).await.unwrap();

        assert_eq!(store.room_member_count("s").await, 2);
        let mut ids = store.room_member_ids("s").await;
        ids.sort();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
    }

    #[tokio::test]
    async fn room_targets_exclude_sender_include_others() {
        let store = SessionStore::new();
        store.ensure_session("s", SessionKind::Room).await.unwrap();

        let (a_tx, _a_rx) = chan();
        let (b_tx, mut b_rx) = chan();
        let (c_tx, mut c_rx) = chan();
        store.register_room("s", "a".into(), a_tx).await.unwrap();
        store.register_room("s", "b".into(), b_tx).await.unwrap();
        store.register_room("s", "c".into(), c_tx).await.unwrap();

        // Sender "a" fans out to b and c, but not a.
        let targets = store.room_targets_excluding("s", "a").await;
        assert_eq!(targets.len(), 2);
        for t in targets {
            t.send("hello".into()).await.unwrap();
        }
        assert_eq!(b_rx.recv().await.unwrap(), "hello");
        assert_eq!(c_rx.recv().await.unwrap(), "hello");
    }

    #[tokio::test]
    async fn member_reconnect_replaces_tx() {
        let store = SessionStore::new();
        store.ensure_session("s", SessionKind::Room).await.unwrap();

        let (old_tx, mut old_rx) = chan();
        store.register_room("s", "a".into(), old_tx).await.unwrap();

        // Reconnect with the same member_id -> tx is replaced, not duplicated.
        let (new_tx, mut new_rx) = chan();
        store.register_room("s", "a".into(), new_tx).await.unwrap();
        assert_eq!(store.room_member_count("s").await, 1);

        // A frame to the (other-excluding) roster from a different member reaches
        // only the new tx. Add a second member to be the sender.
        let (b_tx, _b_rx) = chan();
        store.register_room("s", "b".into(), b_tx).await.unwrap();
        let targets = store.room_targets_excluding("s", "b").await;
        assert_eq!(targets.len(), 1);
        targets[0].send("ping".into()).await.unwrap();
        assert_eq!(new_rx.recv().await.unwrap(), "ping");
        // The old channel never received it.
        assert!(old_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn unregister_member_removes_session_when_empty() {
        let store = SessionStore::new();
        store.ensure_session("s", SessionKind::Room).await.unwrap();

        let (a_tx, _a_rx) = chan();
        let (b_tx, _b_rx) = chan();
        store
            .register_room("s", "a".into(), a_tx.clone())
            .await
            .unwrap();
        store
            .register_room("s", "b".into(), b_tx.clone())
            .await
            .unwrap();

        // Removing one member keeps the session alive.
        assert!(!store.unregister_member("s", "a", &a_tx).await);
        assert!(store.exists("s").await);
        assert_eq!(store.room_member_count("s").await, 1);

        // Removing the last member drops the session and returns true.
        assert!(store.unregister_member("s", "b", &b_tx).await);
        assert!(!store.exists("s").await);
    }

    #[tokio::test]
    async fn unregister_member_after_reconnect_is_noop() {
        // A stale cleanup from the OLD connection must not evict a member that
        // reconnected with the same member_id (its slot now holds a new tx).
        let store = SessionStore::new();
        store.ensure_session("s", SessionKind::Room).await.unwrap();

        let (old_tx, _old_rx) = chan();
        store
            .register_room("s", "a".into(), old_tx.clone())
            .await
            .unwrap();

        // Member "a" reconnects with a fresh channel (replaces the stored tx).
        let (new_tx, _new_rx) = chan();
        store
            .register_room("s", "a".into(), new_tx.clone())
            .await
            .unwrap();
        assert_eq!(store.room_member_count("s").await, 1);

        // The OLD connection's late cleanup must NOT evict the reconnected member.
        assert!(!store.unregister_member("s", "a", &old_tx).await);
        assert_eq!(store.room_member_count("s").await, 1);
        assert!(store.exists("s").await);

        // The current (new) connection's cleanup DOES evict and drops the room.
        assert!(store.unregister_member("s", "a", &new_tx).await);
        assert!(!store.exists("s").await);
    }

    #[tokio::test]
    async fn register_room_rejects_beyond_member_cap() {
        let store = SessionStore::new();
        store.ensure_session("s", SessionKind::Room).await.unwrap();

        // Fill the room to the cap.
        for i in 0..MAX_ROOM_MEMBERS {
            let (tx, _rx) = chan();
            store
                .register_room("s", format!("m{i}"), tx)
                .await
                .unwrap();
        }
        assert_eq!(store.room_member_count("s").await, MAX_ROOM_MEMBERS);

        // One more NEW member is rejected.
        let (tx, _rx) = chan();
        assert_eq!(
            store.register_room("s", "overflow".into(), tx).await,
            Err("room at capacity")
        );

        // But an existing member reconnecting is still allowed (replaces tx).
        let (tx, _rx) = chan();
        assert!(store.register_room("s", "m0".into(), tx).await.is_ok());
        assert_eq!(store.room_member_count("s").await, MAX_ROOM_MEMBERS);
    }

    #[tokio::test]
    async fn reap_stale_removes_empty_old_room() {
        let store = SessionStore::new();
        store.ensure_session("s", SessionKind::Room).await.unwrap();
        // Empty room reaped once it exceeds max_age (0 here).
        store.reap_stale(std::time::Duration::from_secs(0)).await;
        assert!(!store.exists("s").await);
    }
}
