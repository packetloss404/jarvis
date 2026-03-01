//! WebView lifecycle management.
//!
//! `WebViewManager` creates, tracks, and destroys `wry::WebView` instances,
//! one per pane that needs embedded web content (games, chat, docs, etc.).

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::content::ContentProvider;
use crate::events::WebViewEvent;

mod handle;
pub mod handlers;
mod lifecycle;
mod registry;
mod types;

pub use handle::WebViewHandle;
pub use registry::WebViewRegistry;
pub use types::WebViewConfig;

/// Manages all WebView instances across tiling panes.
pub struct WebViewManager {
    /// Event sink — events are pushed here for the main event loop to consume.
    pub(crate) events: Arc<Mutex<Vec<WebViewEvent>>>,
    /// Optional content provider for the `jarvis://` custom protocol.
    content_provider: Option<Arc<ContentProvider>>,
    /// Runtime-toggleable flag: when true, allow navigation to any `https://` URL.
    pub allow_open_url: Arc<AtomicBool>,
}

impl WebViewManager {
    /// Create a new WebView manager.
    pub fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
            content_provider: None,
            allow_open_url: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Drain all pending events.
    pub fn drain_events(&self) -> Vec<WebViewEvent> {
        let mut events = self.events.lock().unwrap();
        std::mem::take(&mut *events)
    }
}

impl Default for WebViewManager {
    fn default() -> Self {
        Self::new()
    }
}
