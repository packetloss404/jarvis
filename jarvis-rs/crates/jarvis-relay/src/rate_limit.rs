//! Simple per-IP rate limiting and global session caps.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Mutex;

/// Rate limiter configuration.
pub struct RateLimitConfig {
    /// Max concurrent WebSocket connections per IP.
    pub max_connections_per_ip: usize,
    /// Max new connections per IP within the rate window.
    pub max_connect_rate_per_ip: usize,
    /// Rate window duration in seconds.
    pub rate_window_secs: u64,
    /// Max total sessions across all IPs.
    pub max_total_sessions: usize,
    /// Max allowed session ID length (bytes).
    pub max_session_id_len: usize,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_connections_per_ip: 10,
            max_connect_rate_per_ip: 20,
            rate_window_secs: 60,
            max_total_sessions: 1000,
            max_session_id_len: 64,
        }
    }
}

struct IpState {
    /// Number of currently active connections.
    active: usize,
    /// Timestamps of recent connection attempts (within rate window).
    recent: Vec<Instant>,
}

/// Thread-safe rate limiter.
#[derive(Clone)]
pub struct RateLimiter {
    state: Arc<Mutex<HashMap<IpAddr, IpState>>>,
    config: Arc<RateLimitConfig>,
}

impl RateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(config),
        }
    }

    /// Check if a new connection from this IP is allowed. If yes, tracks it.
    /// Returns `Err(reason)` if rejected.
    pub async fn try_connect(&self, ip: IpAddr) -> Result<(), &'static str> {
        let mut map = self.state.lock().await;
        let now = Instant::now();
        let window = std::time::Duration::from_secs(self.config.rate_window_secs);

        let entry = map.entry(ip).or_insert_with(|| IpState {
            active: 0,
            recent: Vec::new(),
        });

        // Prune old timestamps outside the rate window.
        entry.recent.retain(|t| now.duration_since(*t) < window);

        // Check concurrent connection limit.
        if entry.active >= self.config.max_connections_per_ip {
            return Err("too many concurrent connections");
        }

        // Check connection rate limit.
        if entry.recent.len() >= self.config.max_connect_rate_per_ip {
            return Err("connection rate exceeded");
        }

        entry.active += 1;
        entry.recent.push(now);
        Ok(())
    }

    /// Release a connection slot for this IP.
    pub async fn disconnect(&self, ip: IpAddr) {
        let mut map = self.state.lock().await;
        if let Some(entry) = map.get_mut(&ip) {
            entry.active = entry.active.saturating_sub(1);
            // Clean up entry if no active connections and no recent history
            if entry.active == 0 && entry.recent.is_empty() {
                map.remove(&ip);
            }
        }
    }

    /// Max total sessions allowed.
    pub fn max_total_sessions(&self) -> usize {
        self.config.max_total_sessions
    }

    /// Max session ID length in bytes.
    pub fn max_session_id_len(&self) -> usize {
        self.config.max_session_id_len
    }
}
