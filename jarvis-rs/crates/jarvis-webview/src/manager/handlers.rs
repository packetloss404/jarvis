use std::sync::{Arc, Mutex};

use tracing::{debug, warn};
use wry::WebViewBuilder;

use crate::events::{PageLoadState, WebViewEvent};

use super::WebViewManager;

// =============================================================================
// NAVIGATION ALLOWLIST
// =============================================================================

/// Allowed URL prefixes for webview navigation.
///
/// Any `https://` URL is always permitted. For non-https schemes, only these
/// origins are allowed:
/// - `jarvis://` — custom protocol for bundled panel assets
/// - `about:blank` — default empty page
/// - `http://jarvis.localhost` — WebView2 Windows rewrite of jarvis://
pub const ALLOWED_NAV_PREFIXES: &[&str] = &[
    "jarvis://",
    // On Windows, WebView2 rewrites custom protocols: jarvis://localhost/… → http://jarvis.localhost/…
    "http://jarvis.localhost",
    "about:blank",
];

/// Check whether a URL is allowed by the navigation allowlist.
pub fn is_navigation_allowed(url: &str) -> bool {
    ALLOWED_NAV_PREFIXES
        .iter()
        .any(|prefix| url.starts_with(prefix))
}

// =============================================================================
// HANDLER ATTACHMENTS
// =============================================================================

impl WebViewManager {
    pub(super) fn attach_ipc_handler<'a>(
        builder: WebViewBuilder<'a>,
        events: Arc<Mutex<Vec<WebViewEvent>>>,
        pid: u32,
    ) -> WebViewBuilder<'a> {
        builder.with_ipc_handler(move |request| {
            let body = request.body().to_string();

            // Validate that the IPC body is valid JSON before forwarding
            if serde_json::from_str::<serde_json::Value>(&body).is_err() {
                warn!(
                    pane_id = pid,
                    body_len = body.len(),
                    "IPC message rejected: invalid JSON"
                );
                return;
            }

            debug!(pane_id = pid, body_len = body.len(), "IPC message from JS");
            if let Ok(mut evts) = events.lock() {
                evts.push(WebViewEvent::IpcMessage { pane_id: pid, body });
            }
        })
    }

    pub(super) fn attach_page_load_handler<'a>(
        builder: WebViewBuilder<'a>,
        events: Arc<Mutex<Vec<WebViewEvent>>>,
        pid: u32,
    ) -> WebViewBuilder<'a> {
        builder.with_on_page_load_handler(move |event, url| {
            let state = PageLoadState::from(event);
            debug!(pane_id = pid, ?state, url = %url, "page load");
            if let Ok(mut evts) = events.lock() {
                evts.push(WebViewEvent::PageLoad {
                    pane_id: pid,
                    state,
                    url,
                });
            }
        })
    }

    pub(super) fn attach_title_handler<'a>(
        builder: WebViewBuilder<'a>,
        events: Arc<Mutex<Vec<WebViewEvent>>>,
        pid: u32,
    ) -> WebViewBuilder<'a> {
        builder.with_document_title_changed_handler(move |title| {
            debug!(pane_id = pid, title = %title, "title changed");
            if let Ok(mut evts) = events.lock() {
                evts.push(WebViewEvent::TitleChanged {
                    pane_id: pid,
                    title,
                });
            }
        })
    }

    pub(super) fn attach_navigation_handler<'a>(
        builder: WebViewBuilder<'a>,
        events: Arc<Mutex<Vec<WebViewEvent>>>,
        pid: u32,
    ) -> WebViewBuilder<'a> {
        builder.with_navigation_handler(move |url| {
            if !url.starts_with("https://")
                && !url.starts_with("http://")
                && !is_navigation_allowed(&url)
            {
                warn!(
                    pane_id = pid,
                    url = %url,
                    "navigation blocked: URL not in allowlist"
                );
                return false;
            }

            debug!(pane_id = pid, url = %url, "navigation allowed");
            if let Ok(mut evts) = events.lock() {
                evts.push(WebViewEvent::NavigationRequested { pane_id: pid, url });
            }
            true
        })
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- Allowed URLs --

    #[test]
    fn allows_jarvis_protocol() {
        assert!(is_navigation_allowed(
            "jarvis://localhost/terminal/index.html"
        ));
        assert!(is_navigation_allowed("jarvis://localhost/chat/index.html"));
        assert!(is_navigation_allowed(
            "jarvis://localhost/games/tetris.html"
        ));
    }

    #[test]
    fn allows_about_blank() {
        assert!(is_navigation_allowed("about:blank"));
    }

    // -- https:// URLs are allowed by the navigation handler, not by is_navigation_allowed --

    #[test]
    fn https_not_in_allowlist_but_allowed_by_handler() {
        // is_navigation_allowed only covers non-https schemes.
        // https:// is handled directly in attach_navigation_handler.
        assert!(!is_navigation_allowed("https://evil.com"));
        assert!(!is_navigation_allowed("https://google.com"));
        assert!(!is_navigation_allowed(
            "https://ojmqzagktzkualzgpcbq.supabase.co/rest/v1/channels"
        ));
    }

    // -- Blocked URLs --

    #[test]
    fn blocks_file_protocol() {
        assert!(!is_navigation_allowed("file:///etc/passwd"));
        assert!(!is_navigation_allowed("file:///Users/cw/.ssh/id_rsa"));
        assert!(!is_navigation_allowed("file://localhost/etc/hosts"));
    }

    #[test]
    fn allows_webview2_rewritten_custom_protocol() {
        // WebView2 on Windows rewrites jarvis://localhost/… → http://jarvis.localhost/…
        assert!(is_navigation_allowed(
            "http://jarvis.localhost/boot/index.html"
        ));
        assert!(is_navigation_allowed(
            "http://jarvis.localhost/terminal/index.html"
        ));
    }

    #[test]
    fn http_not_in_allowlist_but_allowed_by_handler() {
        // is_navigation_allowed only covers non-http/https schemes.
        // http:// is handled directly in attach_navigation_handler.
        assert!(!is_navigation_allowed("http://evil.com"));
        assert!(!is_navigation_allowed("http://localhost:8080"));
    }

    #[test]
    fn blocks_javascript_protocol() {
        assert!(!is_navigation_allowed("javascript:alert(1)"));
        assert!(!is_navigation_allowed("javascript:void(0)"));
    }

    #[test]
    fn blocks_data_protocol() {
        assert!(!is_navigation_allowed("data:text/html,<h1>XSS</h1>"));
        assert!(!is_navigation_allowed(
            "data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg=="
        ));
    }

    #[test]
    fn blocks_empty_and_garbage() {
        assert!(!is_navigation_allowed(""));
        assert!(!is_navigation_allowed("   "));
        assert!(!is_navigation_allowed("not-a-url"));
        assert!(!is_navigation_allowed("ftp://files.example.com"));
    }

    // -- Allowlist structure --

    #[test]
    fn allowlist_has_expected_entries() {
        assert_eq!(ALLOWED_NAV_PREFIXES.len(), 3);
        assert!(ALLOWED_NAV_PREFIXES.contains(&"jarvis://"));
        assert!(ALLOWED_NAV_PREFIXES.contains(&"about:blank"));
        assert!(ALLOWED_NAV_PREFIXES.contains(&"http://jarvis.localhost"));
    }
}
