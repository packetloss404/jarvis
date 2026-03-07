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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Bridge,
    Broadcast,
}

impl Role {
    pub fn session_kind(self) -> SessionKind {
        match self {
            Role::Desktop | Role::Mobile => SessionKind::Bridge,
            Role::Host | Role::Spectator => SessionKind::Broadcast,
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

pub enum Session {
    Bridge(BridgeSession),
    Broadcast(BroadcastSession),
}

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
            Some(Session::Broadcast(_)) => return Err("session kind mismatch"),
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
            Some(Session::Bridge(_)) => return Err("session kind mismatch"),
            None => return Err("session not found"),
        };

        match role {
            Role::Host => {
                if session.host_tx.is_some() {
                    return Err("host already connected");
                }
                session.host_tx = Some(tx);
            }
            Role::Spectator => {
                session.spectator_txs.push(tx);
            }
            _ => return Err("invalid role for broadcast session"),
        }

        Ok(BroadcastRegistration {
            host_connected: session.host_tx.is_some(),
        })
    }

    /// Get the peer's sender for forwarding.
    pub async fn get_peer_tx(&self, session_id: &str, role: Role) -> Option<mpsc::Sender<String>> {
        let map = self.sessions.read().await;
        let session = match map.get(session_id)? {
            Session::Bridge(session) => session,
            Session::Broadcast(_) => return None,
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
