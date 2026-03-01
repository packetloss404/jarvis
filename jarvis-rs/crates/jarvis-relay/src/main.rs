//! jarvis-relay: WebSocket relay server for mobile <-> desktop bridge.
//!
//! Accepts WebSocket connections, pairs them by session ID, and forwards
//! messages between desktop and mobile clients. The relay never inspects
//! message payloads -- all PTY data is E2E encrypted between endpoints.

mod connection;
mod protocol;
mod rate_limit;
mod session;

use std::time::Duration;

use clap::Parser;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;

use crate::connection::handle_connection;
use crate::rate_limit::{RateLimitConfig, RateLimiter};
use crate::session::SessionStore;

#[derive(Parser)]
#[command(
    name = "jarvis-relay",
    about = "WebSocket relay for jarvis mobile bridge"
)]
struct Args {
    /// Port to listen on.
    #[arg(short, long, default_value_t = 8080)]
    port: u16,

    /// Maximum stale session age in seconds (no mobile peer).
    #[arg(long, default_value_t = 300)]
    session_ttl: u64,

    /// Max concurrent connections per IP.
    #[arg(long, default_value_t = 10)]
    max_connections_per_ip: usize,

    /// Max total sessions.
    #[arg(long, default_value_t = 1000)]
    max_sessions: usize,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "jarvis_relay=info".into()),
        )
        .init();

    let args = Args::parse();
    let store = SessionStore::new();
    let limiter = RateLimiter::new(RateLimitConfig {
        max_connections_per_ip: args.max_connections_per_ip,
        max_total_sessions: args.max_sessions,
        ..Default::default()
    });

    let addr = format!("0.0.0.0:{}", args.port);
    let listener = TcpListener::bind(&addr)
        .await
        .expect("Failed to bind TCP listener");

    tracing::info!("jarvis-relay listening on {}", addr);

    // Spawn stale session reaper.
    let reaper_store = store.clone();
    let ttl = Duration::from_secs(args.session_ttl);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            reaper_store.reap_stale(ttl).await;
            let count = reaper_store.count().await;
            tracing::debug!(sessions = count, "Reaper tick");
        }
    });

    // Accept loop.
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                let ip = addr.ip();

                // Rate limit check before accepting WebSocket handshake.
                let limiter = limiter.clone();
                if let Err(reason) = limiter.try_connect(ip).await {
                    tracing::warn!(peer = %addr, reason = reason, "Connection rejected");
                    drop(stream);
                    continue;
                }

                let store = store.clone();
                tokio::spawn(async move {
                    match accept_async(stream).await {
                        Ok(ws) => handle_connection(ws, addr, store, &limiter).await,
                        Err(e) => {
                            tracing::warn!(peer = %addr, error = %e, "WS handshake failed");
                        }
                    }
                    limiter.disconnect(ip).await;
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "TCP accept error");
            }
        }
    }
}
