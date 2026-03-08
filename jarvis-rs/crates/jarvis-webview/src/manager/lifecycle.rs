use std::sync::Arc;

use tracing::{debug, warn};
use wry::raw_window_handle;
use wry::WebViewBuilder;

use crate::content::ContentProvider;
use crate::ipc::IPC_INIT_SCRIPT;

use super::handle::WebViewHandle;
use super::types::WebViewConfig;
use super::WebViewManager;

impl WebViewManager {
    /// Create a new WebView as a child of the given window.
    ///
    /// The `window` must implement `raw_window_handle::HasWindowHandle`.
    /// The WebView is positioned at `bounds` within the parent window.
    pub fn create<W: raw_window_handle::HasWindowHandle>(
        &self,
        pane_id: u32,
        window: &W,
        bounds: wry::Rect,
        config: WebViewConfig,
    ) -> Result<WebViewHandle, wry::Error> {
        let events = Arc::clone(&self.events);
        let pid = pane_id;

        // Start building the WebView
        let mut builder = WebViewBuilder::new()
            .with_bounds(bounds)
            .with_transparent(config.transparent)
            .with_devtools(config.devtools)
            .with_clipboard(config.clipboard)
            .with_autoplay(config.autoplay)
            .with_focused(false);

        // Initialization script for IPC bridge
        builder = builder.with_initialization_script(IPC_INIT_SCRIPT);

        // User agent
        if let Some(ua) = &config.user_agent {
            builder = builder.with_user_agent(ua);
        }

        // IPC handler: JS -> Rust
        builder = Self::attach_ipc_handler(builder, Arc::clone(&events), pid);

        // Page load handler
        builder = Self::attach_page_load_handler(builder, Arc::clone(&events), pid);

        // Title change handler
        builder = Self::attach_title_handler(builder, Arc::clone(&events), pid);

        // Navigation handler — allowlist + any https://
        builder = Self::attach_navigation_handler(builder, Arc::clone(&events), pid);

        // Custom protocol for bundled content
        builder = self.attach_custom_protocol(builder);

        // Set initial content
        let initial_url;
        if let Some(url) = &config.url {
            builder = builder.with_url(url);
            initial_url = url.clone();
        } else if let Some(html) = &config.html {
            builder = builder.with_html(html);
            initial_url = "about:blank".to_string();
        } else {
            builder = builder.with_html("<html><body></body></html>");
            initial_url = "about:blank".to_string();
        }

        // Build as child WebView
        let webview = builder.build_as_child(window)?;

        debug!(pane_id, url = %initial_url, "WebView created");

        Ok(WebViewHandle {
            webview,
            pane_id,
            current_url: initial_url,
            current_title: String::new(),
        })
    }

    /// Set the content provider for serving bundled assets via `jarvis://`.
    pub fn set_content_provider(&mut self, provider: ContentProvider) {
        self.content_provider = Some(Arc::new(provider));
    }

    fn attach_custom_protocol<'a>(&self, mut builder: WebViewBuilder<'a>) -> WebViewBuilder<'a> {
        if let Some(provider) = &self.content_provider {
            let cp = Arc::clone(provider);
            builder = builder.with_custom_protocol("jarvis".to_string(), move |_wv_id, request| {
                let uri = request.uri().to_string();
                let path = uri
                    .strip_prefix("jarvis://localhost/")
                    .or_else(|| uri.strip_prefix("jarvis://localhost"))
                    .or_else(|| uri.strip_prefix("jarvis:///"))
                    .or_else(|| uri.strip_prefix("jarvis://"))
                    .unwrap_or("");

                // Music audio streaming with Range header support
                if let Some(encoded) = path.strip_prefix("music/stream/") {
                    return serve_music_file(encoded, &request);
                }

                // Music cover art
                if let Some(encoded) = path.strip_prefix("music/art/") {
                    return serve_music_art(encoded);
                }

                match cp.resolve(path) {
                    Some((mime, data)) => wry::http::Response::builder()
                        .status(200)
                        .header("Content-Type", mime.as_ref())
                        .header("Access-Control-Allow-Origin", "jarvis://localhost")
                        .header("Cache-Control", "no-store")
                        .body(std::borrow::Cow::from(data.into_owned()))
                        .unwrap(),
                    None => {
                        warn!(path = %path, "custom protocol: asset not found");
                        wry::http::Response::builder()
                            .status(404)
                            .body(std::borrow::Cow::from(b"Not Found".to_vec()))
                            .unwrap()
                    }
                }
            });
        }
        builder
    }
}

/// Decode a base64url-encoded file path from a music URL.
fn decode_music_path(encoded: &str) -> Option<std::path::PathBuf> {
    use base64::Engine as _;
    // Strip any query string
    let encoded = encoded.split('?').next().unwrap_or(encoded);
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()?;
    let path_str = String::from_utf8(bytes).ok()?;
    let path = std::path::PathBuf::from(&path_str);
    // Must be an existing file
    if !path.is_file() {
        return None;
    }
    Some(path)
}

/// MIME type for audio files.
fn audio_mime(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("mp3") => "audio/mpeg",
        Some("flac") => "audio/flac",
        Some("ogg" | "opus") => "audio/ogg",
        Some("m4a" | "aac") => "audio/mp4",
        Some("wav") => "audio/wav",
        Some("wma") => "audio/x-ms-wma",
        _ => "application/octet-stream",
    }
}

/// Serve an audio file with Range header support for seeking.
fn serve_music_file(
    encoded: &str,
    request: &wry::http::Request<Vec<u8>>,
) -> wry::http::Response<std::borrow::Cow<'static, [u8]>> {
    let path = match decode_music_path(encoded) {
        Some(p) => p,
        None => {
            return wry::http::Response::builder()
                .status(404)
                .body(std::borrow::Cow::from(b"File not found".to_vec()))
                .unwrap();
        }
    };

    let file_size = match std::fs::metadata(&path) {
        Ok(m) => m.len(),
        Err(_) => {
            return wry::http::Response::builder()
                .status(404)
                .body(std::borrow::Cow::from(b"File not found".to_vec()))
                .unwrap();
        }
    };

    let mime = audio_mime(&path);

    // Parse Range header
    if let Some(range_val) = request.headers().get("Range").and_then(|v| v.to_str().ok()) {
        if let Some(range) = parse_range(range_val, file_size) {
            let (start, end) = range;
            let length = end - start + 1;

            use std::io::{Read, Seek, SeekFrom};
            let mut file = match std::fs::File::open(&path) {
                Ok(f) => f,
                Err(_) => {
                    return wry::http::Response::builder()
                        .status(500)
                        .body(std::borrow::Cow::from(b"Read error".to_vec()))
                        .unwrap();
                }
            };

            let _ = file.seek(SeekFrom::Start(start));
            let mut buf = vec![0u8; length as usize];
            let _ = file.read_exact(&mut buf);

            return wry::http::Response::builder()
                .status(206)
                .header("Content-Type", mime)
                .header("Content-Length", length.to_string())
                .header("Content-Range", format!("bytes {start}-{end}/{file_size}"))
                .header("Accept-Ranges", "bytes")
                .header("Access-Control-Allow-Origin", "jarvis://localhost")
                .body(std::borrow::Cow::from(buf))
                .unwrap();
        }
    }

    // No Range header — serve entire file
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(_) => {
            return wry::http::Response::builder()
                .status(500)
                .body(std::borrow::Cow::from(b"Read error".to_vec()))
                .unwrap();
        }
    };

    wry::http::Response::builder()
        .status(200)
        .header("Content-Type", mime)
        .header("Content-Length", data.len().to_string())
        .header("Accept-Ranges", "bytes")
        .header("Access-Control-Allow-Origin", "jarvis://localhost")
        .body(std::borrow::Cow::from(data))
        .unwrap()
}

/// Serve embedded cover art from an audio file.
fn serve_music_art(encoded: &str) -> wry::http::Response<std::borrow::Cow<'static, [u8]>> {
    let path = match decode_music_path(encoded) {
        Some(p) => p,
        None => {
            return wry::http::Response::builder()
                .status(404)
                .body(std::borrow::Cow::from(b"File not found".to_vec()))
                .unwrap();
        }
    };

    // Read cover art using lofty
    use lofty::file::TaggedFileExt;
    use lofty::picture::PictureType;

    let tagged = match lofty::read_from_path(&path) {
        Ok(t) => t,
        Err(_) => {
            return wry::http::Response::builder()
                .status(404)
                .body(std::borrow::Cow::from(b"No art found".to_vec()))
                .unwrap();
        }
    };

    let tag = match tagged.primary_tag().or_else(|| tagged.first_tag()) {
        Some(t) => t,
        None => {
            return wry::http::Response::builder()
                .status(404)
                .body(std::borrow::Cow::from(b"No tags".to_vec()))
                .unwrap();
        }
    };

    let pictures = tag.pictures();
    let pic = match pictures
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first())
    {
        Some(p) => p,
        None => {
            return wry::http::Response::builder()
                .status(404)
                .body(std::borrow::Cow::from(b"No cover art".to_vec()))
                .unwrap();
        }
    };

    let mime = pic
        .mime_type()
        .map(|m| m.to_string())
        .unwrap_or_else(|| "image/jpeg".to_string());

    wry::http::Response::builder()
        .status(200)
        .header("Content-Type", mime)
        .header("Access-Control-Allow-Origin", "jarvis://localhost")
        .header("Cache-Control", "max-age=3600")
        .body(std::borrow::Cow::from(pic.data().to_vec()))
        .unwrap()
}

/// Parse an HTTP Range header value like "bytes=0-1023".
fn parse_range(range_str: &str, file_size: u64) -> Option<(u64, u64)> {
    let range_str = range_str.strip_prefix("bytes=")?;
    let mut parts = range_str.splitn(2, '-');
    let start_str = parts.next()?.trim();
    let end_str = parts.next()?.trim();

    if start_str.is_empty() {
        // Suffix range: "bytes=-500" means last 500 bytes
        let suffix: u64 = end_str.parse().ok()?;
        let start = file_size.saturating_sub(suffix);
        Some((start, file_size - 1))
    } else {
        let start: u64 = start_str.parse().ok()?;
        let end = if end_str.is_empty() {
            file_size - 1
        } else {
            end_str.parse().ok()?
        };
        if start > end || start >= file_size {
            return None;
        }
        Some((start, end.min(file_size - 1)))
    }
}
