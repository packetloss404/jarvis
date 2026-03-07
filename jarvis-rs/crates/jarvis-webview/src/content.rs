//! Local content serving via custom protocol.
//!
//! Registers a `jarvis://` custom protocol so that WebViews can load
//! bundled HTML/JS/CSS assets without needing a local HTTP server.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use include_dir::{include_dir, Dir};

/// Panels directory embedded at compile time so the binary is self-contained.
static EMBEDDED_PANELS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/panels");

/// Serves local files from a base directory via custom protocol.
///
/// When a WebView requests `jarvis://app/index.html`, the content
/// provider resolves it to `{base_dir}/app/index.html` and returns
/// the file contents with the appropriate MIME type.
pub struct ContentProvider {
    /// Base directory for resolving asset paths.
    base_dir: PathBuf,
    /// In-memory overrides (for dynamically generated content).
    overrides: HashMap<String, (String, Vec<u8>)>, // path -> (mime, data)
    /// Plugin directories: plugin_id -> absolute path to plugin folder.
    /// Wrapped in `Arc<RwLock>` so the custom protocol closure and the app
    /// can share the same mutable map (e.g. on config reload).
    plugin_dirs: Arc<RwLock<HashMap<String, PathBuf>>>,
}

impl ContentProvider {
    /// Create a new content provider rooted at `base_dir`.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            overrides: HashMap::new(),
            plugin_dirs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register an in-memory asset override.
    pub fn add_override(
        &mut self,
        path: impl Into<String>,
        mime: impl Into<String>,
        data: impl Into<Vec<u8>>,
    ) {
        self.overrides
            .insert(path.into(), (mime.into(), data.into()));
    }

    /// Register a local plugin directory.
    pub fn add_plugin_dir(&self, id: impl Into<String>, path: impl Into<PathBuf>) {
        if let Ok(mut dirs) = self.plugin_dirs.write() {
            dirs.insert(id.into(), path.into());
        }
    }

    /// Remove all registered plugin directories.
    pub fn clear_plugin_dirs(&self) {
        if let Ok(mut dirs) = self.plugin_dirs.write() {
            dirs.clear();
        }
    }

    /// Get a shared handle to the plugin directories map.
    ///
    /// Used to share the map between the `ContentProvider` (inside the
    /// custom protocol closure) and the app state (for reload).
    pub fn plugin_dirs_handle(&self) -> Arc<RwLock<HashMap<String, PathBuf>>> {
        Arc::clone(&self.plugin_dirs)
    }

    /// Resolve a request path to content bytes and MIME type.
    pub fn resolve(&self, path: &str) -> Option<(Cow<'_, str>, Cow<'_, [u8]>)> {
        let clean = path.trim_start_matches('/');

        // Check overrides first
        if let Some((mime, data)) = self.overrides.get(clean) {
            return Some((Cow::Borrowed(mime.as_str()), Cow::Borrowed(data.as_slice())));
        }

        // Check plugin directories: paths like "plugins/{id}/..."
        if let Some(rest) = clean.strip_prefix("plugins/") {
            if let Some(slash_pos) = rest.find('/') {
                let plugin_id = &rest[..slash_pos];
                let asset_path = &rest[slash_pos + 1..];
                return self.resolve_plugin_asset(plugin_id, asset_path);
            }
        }

        // Resolve from filesystem
        let file_path = self.base_dir.join(clean);

        // Prevent directory traversal (including symlink bypass).
        // Canonicalize both paths to resolve symlinks, `..`, etc.
        let canonical_base = std::fs::canonicalize(&self.base_dir).ok();
        let canonical_file = std::fs::canonicalize(&file_path).ok();

        if let (Some(base), Some(file)) = (&canonical_base, &canonical_file) {
            if file.starts_with(base) {
                if let Ok(data) = std::fs::read(file) {
                    let mime = mime_from_extension(&file_path);
                    return Some((Cow::Owned(mime.to_string()), Cow::Owned(data)));
                }
            }
        }

        // Fall back to embedded assets.
        // The embedded dir is rooted at assets/panels, so strip the "panels/" prefix.
        let embedded_path = clean.strip_prefix("panels/").unwrap_or(clean);
        if embedded_path.contains("..") {
            return None;
        }
        let entry = EMBEDDED_PANELS.get_file(embedded_path)?;
        let mime = mime_from_extension(Path::new(embedded_path));
        Some((Cow::Borrowed(mime), Cow::Borrowed(entry.contents())))
    }

    /// Resolve an asset from a plugin directory with containment check.
    fn resolve_plugin_asset(
        &self,
        plugin_id: &str,
        asset_path: &str,
    ) -> Option<(Cow<'_, str>, Cow<'_, [u8]>)> {
        let plugin_base = {
            let dirs = self.plugin_dirs.read().ok()?;
            dirs.get(plugin_id)?.clone()
        };

        let file_path = plugin_base.join(asset_path);

        // Containment check: canonicalize both to prevent traversal
        let canonical_base = std::fs::canonicalize(&plugin_base).ok()?;
        let canonical_file = std::fs::canonicalize(&file_path).ok()?;
        if !canonical_file.starts_with(&canonical_base) {
            return None;
        }

        let data = std::fs::read(&canonical_file).ok()?;
        let mime = mime_from_extension(&file_path);
        Some((Cow::Owned(mime.to_string()), Cow::Owned(data)))
    }

    /// The base directory for assets.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }
}

/// Guess MIME type from file extension.
fn mime_from_extension(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") | Some("htm") => "text/html",
        Some("css") => "text/css",
        Some("js") | Some("mjs") => "application/javascript",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("wasm") => "application/wasm",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("mp3") => "audio/mpeg",
        Some("ogg") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("webp") => "image/webp",
        Some("txt") => "text/plain",
        Some("xml") => "application/xml",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Path to the assets directory at the workspace root.
    fn assets_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent() // crates/
            .unwrap()
            .parent() // workspace root
            .unwrap()
            .join("assets")
    }

    // -----------------------------------------------------------------
    // Panel file resolution
    // -----------------------------------------------------------------

    #[test]
    fn resolve_chat_panel() {
        let cp = ContentProvider::new(assets_dir());
        let result = cp.resolve("panels/chat/index.html");
        assert!(result.is_some(), "chat panel should resolve");
        let (mime, data) = result.unwrap();
        assert_eq!(mime.as_ref(), "text/html");
        assert!(data.len() > 1000, "chat.html should be >1KB");
        let html = String::from_utf8_lossy(&data);
        assert!(
            html.contains("JARVIS Livechat"),
            "should contain chat title"
        );
    }

    #[test]
    fn resolve_terminal_panel() {
        let cp = ContentProvider::new(assets_dir());
        let result = cp.resolve("panels/terminal/index.html");
        assert!(result.is_some(), "terminal panel should resolve");
        let (mime, data) = result.unwrap();
        assert_eq!(mime.as_ref(), "text/html");
        let html = String::from_utf8_lossy(&data);
        assert!(html.contains("xterm"), "should contain xterm.js reference");
        assert!(html.contains("integrity="), "CDN scripts must have SRI");
    }

    #[test]
    fn resolve_assistant_panel() {
        let cp = ContentProvider::new(assets_dir());
        let result = cp.resolve("panels/assistant/index.html");
        assert!(result.is_some(), "assistant panel should resolve");
        let (mime, data) = result.unwrap();
        assert_eq!(mime.as_ref(), "text/html");
        let html = String::from_utf8_lossy(&data);
        assert!(html.contains("Assistant"), "should contain Assistant title");
        assert!(
            html.contains("sendIpc"),
            "assistant panel must use IPC bridge"
        );
    }

    #[test]
    fn resolve_presence_panel() {
        let cp = ContentProvider::new(assets_dir());
        let result = cp.resolve("panels/presence/index.html");
        assert!(result.is_some(), "presence panel should resolve");
        let (mime, _) = result.unwrap();
        assert_eq!(mime.as_ref(), "text/html");
    }

    #[test]
    fn resolve_settings_panel() {
        let cp = ContentProvider::new(assets_dir());
        let result = cp.resolve("panels/settings/index.html");
        assert!(result.is_some(), "settings panel should resolve");
        let (mime, _) = result.unwrap();
        assert_eq!(mime.as_ref(), "text/html");
    }

    // -----------------------------------------------------------------
    // Game files
    // -----------------------------------------------------------------

    #[test]
    fn resolve_all_game_panels() {
        let cp = ContentProvider::new(assets_dir());
        let games = [
            "panels/games/asteroids.html",
            "panels/games/tetris.html",
            "panels/games/minesweeper.html",
            "panels/games/pinball.html",
            "panels/games/doodlejump.html",
            "panels/games/subway.html",
            "panels/games/draw.html",
            "panels/games/videoplayer.html",
            "panels/games/emulator.html",
        ];
        for game in &games {
            let result = cp.resolve(game);
            assert!(result.is_some(), "{game} should resolve");
            let (mime, _) = result.unwrap();
            assert_eq!(mime.as_ref(), "text/html", "{game} should be text/html");
        }
    }

    #[test]
    fn resolve_pinball_assets() {
        let cp = ContentProvider::new(assets_dir());
        let css = cp.resolve("panels/games/pinball.css");
        assert!(css.is_some(), "pinball.css should resolve");
        assert_eq!(css.unwrap().0.as_ref(), "text/css");

        let js = cp.resolve("panels/games/pinball.js");
        assert!(js.is_some(), "pinball.js should resolve");
        assert_eq!(js.unwrap().0.as_ref(), "application/javascript");
    }

    // -----------------------------------------------------------------
    // Security: directory traversal
    // -----------------------------------------------------------------

    #[test]
    fn traversal_with_dotdot_is_blocked() {
        let cp = ContentProvider::new(assets_dir());
        assert!(
            cp.resolve("../../etc/passwd").is_none(),
            "directory traversal with ../../ must be blocked"
        );
    }

    #[test]
    fn traversal_with_absolute_path_is_blocked() {
        let cp = ContentProvider::new(assets_dir());
        assert!(
            cp.resolve("/etc/passwd").is_none(),
            "absolute path traversal must be blocked"
        );
    }

    #[test]
    fn traversal_with_encoded_dotdot_is_blocked() {
        let cp = ContentProvider::new(assets_dir());
        // Even if someone tries to sneak ../ as part of a path
        assert!(
            cp.resolve("panels/../../../etc/passwd").is_none(),
            "nested traversal must be blocked"
        );
    }

    #[test]
    fn nonexistent_file_returns_none() {
        let cp = ContentProvider::new(assets_dir());
        assert!(cp.resolve("panels/does_not_exist.html").is_none());
    }

    // -----------------------------------------------------------------
    // MIME types
    // -----------------------------------------------------------------

    #[test]
    fn mime_type_html() {
        assert_eq!(mime_from_extension(Path::new("test.html")), "text/html");
        assert_eq!(mime_from_extension(Path::new("test.htm")), "text/html");
    }

    #[test]
    fn mime_type_css() {
        assert_eq!(mime_from_extension(Path::new("style.css")), "text/css");
    }

    #[test]
    fn mime_type_javascript() {
        assert_eq!(
            mime_from_extension(Path::new("app.js")),
            "application/javascript"
        );
        assert_eq!(
            mime_from_extension(Path::new("module.mjs")),
            "application/javascript"
        );
    }

    #[test]
    fn mime_type_unknown_is_octet_stream() {
        assert_eq!(
            mime_from_extension(Path::new("data.xyz")),
            "application/octet-stream"
        );
    }

    // -----------------------------------------------------------------
    // In-memory overrides
    // -----------------------------------------------------------------

    #[test]
    fn override_takes_precedence() {
        let mut cp = ContentProvider::new(assets_dir());
        cp.add_override(
            "panels/chat/index.html",
            "text/html",
            b"<html>override</html>".to_vec(),
        );
        let result = cp.resolve("panels/chat/index.html");
        assert!(result.is_some());
        let (mime, data) = result.unwrap();
        assert_eq!(mime.as_ref(), "text/html");
        assert_eq!(data.as_ref(), b"<html>override</html>");
    }

    #[test]
    fn override_for_nonexistent_path() {
        let mut cp = ContentProvider::new(assets_dir());
        cp.add_override(
            "virtual/page.html",
            "text/html",
            b"<html>virtual</html>".to_vec(),
        );
        let result = cp.resolve("virtual/page.html");
        assert!(result.is_some());
        let (_, data) = result.unwrap();
        assert_eq!(data.as_ref(), b"<html>virtual</html>");
    }

    // -----------------------------------------------------------------
    // Leading slash handling
    // -----------------------------------------------------------------

    #[test]
    fn resolve_with_leading_slash() {
        let cp = ContentProvider::new(assets_dir());
        let result = cp.resolve("/panels/chat/index.html");
        assert!(result.is_some(), "leading slash should be stripped");
    }

    // -----------------------------------------------------------------
    // Security invariants for HTML content
    // -----------------------------------------------------------------

    #[test]
    fn chat_html_has_e2e_encryption() {
        let cp = ContentProvider::new(assets_dir());
        let (_, data) = cp.resolve("panels/chat/index.html").unwrap();
        let html = String::from_utf8_lossy(&data);
        assert!(
            html.contains("AES-GCM") || html.contains("encrypt"),
            "chat.html must have E2E encryption"
        );
    }

    #[test]
    fn chat_html_has_automod() {
        let cp = ContentProvider::new(assets_dir());
        let (_, data) = cp.resolve("panels/chat/index.html").unwrap();
        let html = String::from_utf8_lossy(&data);
        assert!(
            html.contains("AutoMod") || html.contains("automod"),
            "chat.html must have automod"
        );
    }

    #[test]
    fn chat_html_has_supabase_sri() {
        let cp = ContentProvider::new(assets_dir());
        let (_, data) = cp.resolve("panels/chat/index.html").unwrap();
        let html = String::from_utf8_lossy(&data);
        assert!(
            html.contains("integrity="),
            "Supabase CDN script must have SRI integrity hash"
        );
    }

    #[test]
    fn terminal_html_has_xterm_sri() {
        let cp = ContentProvider::new(assets_dir());
        let (_, data) = cp.resolve("panels/terminal/index.html").unwrap();
        let html = String::from_utf8_lossy(&data);
        // Count integrity attributes — should have at least 3 (xterm CSS, xterm JS, fit addon)
        let integrity_count = html.matches("integrity=").count();
        assert!(
            integrity_count >= 3,
            "terminal HTML must have SRI on all CDN resources, found {integrity_count}"
        );
    }

    #[test]
    fn presence_html_uses_jarvis_ipc() {
        let cp = ContentProvider::new(assets_dir());
        let (_, data) = cp.resolve("panels/presence/index.html").unwrap();
        let html = String::from_utf8_lossy(&data);
        assert!(
            html.contains("window.jarvis.ipc"),
            "presence panel must use jarvis IPC bridge, not webkit messageHandlers"
        );
        assert!(
            !html.contains("webkit.messageHandlers"),
            "presence panel must NOT use webkit messageHandlers (ported to jarvis IPC)"
        );
    }

    #[test]
    fn no_innerhtml_with_user_data_in_new_panels() {
        let cp = ContentProvider::new(assets_dir());
        for panel in &[
            "panels/terminal/index.html",
            "panels/presence/index.html",
            "panels/settings/index.html",
        ] {
            let (_, data) = cp.resolve(panel).unwrap();
            let html = String::from_utf8_lossy(&data);
            assert!(
                !html.contains(".innerHTML"),
                "{panel} must not use innerHTML (XSS risk)"
            );
        }
    }

    // -----------------------------------------------------------------
    // Plugin directory resolution
    // -----------------------------------------------------------------

    #[test]
    fn resolve_plugin_asset() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join("my-plugin");
        std::fs::create_dir(&plugin_dir).unwrap();
        std::fs::write(plugin_dir.join("index.html"), "<html>plugin</html>").unwrap();

        let cp = ContentProvider::new(assets_dir());
        cp.add_plugin_dir("my-plugin", &plugin_dir);

        let result = cp.resolve("plugins/my-plugin/index.html");
        assert!(result.is_some(), "plugin asset should resolve");
        let (mime, data) = result.unwrap();
        assert_eq!(mime.as_ref(), "text/html");
        assert_eq!(data.as_ref(), b"<html>plugin</html>");
    }

    #[test]
    fn plugin_traversal_is_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join("evil-plugin");
        std::fs::create_dir(&plugin_dir).unwrap();
        std::fs::write(plugin_dir.join("index.html"), "ok").unwrap();

        let cp = ContentProvider::new(assets_dir());
        cp.add_plugin_dir("evil-plugin", &plugin_dir);

        assert!(
            cp.resolve("plugins/evil-plugin/../../etc/passwd").is_none(),
            "directory traversal from plugin dir must be blocked"
        );
    }

    #[test]
    fn unknown_plugin_returns_none() {
        let cp = ContentProvider::new(assets_dir());
        assert!(cp.resolve("plugins/nonexistent/index.html").is_none());
    }

    #[test]
    fn clear_plugin_dirs_works() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join("test-plugin");
        std::fs::create_dir(&plugin_dir).unwrap();
        std::fs::write(plugin_dir.join("index.html"), "ok").unwrap();

        let cp = ContentProvider::new(assets_dir());
        cp.add_plugin_dir("test-plugin", &plugin_dir);
        assert!(cp.resolve("plugins/test-plugin/index.html").is_some());

        cp.clear_plugin_dirs();
        assert!(cp.resolve("plugins/test-plugin/index.html").is_none());
    }

    // -----------------------------------------------------------------
    // Total assets size budget
    // -----------------------------------------------------------------

    #[test]
    fn total_assets_under_2mb() {
        let cp = ContentProvider::new(assets_dir());
        let all_files = [
            "panels/assistant/index.html",
            "panels/chat/index.html",
            "panels/games/asteroids.html",
            "panels/games/tetris.html",
            "panels/games/minesweeper.html",
            "panels/games/pinball.html",
            "panels/games/pinball.css",
            "panels/games/pinball.js",
            "panels/games/doodlejump.html",
            "panels/games/subway.html",
            "panels/games/draw.html",
            "panels/games/videoplayer.html",
            "panels/games/emulator.html",
            "panels/terminal/index.html",
            "panels/presence/index.html",
            "panels/settings/index.html",
        ];
        let total: usize = all_files
            .iter()
            .map(|f| {
                cp.resolve(f)
                    .unwrap_or_else(|| panic!("{f} should resolve"))
                    .1
                    .len()
            })
            .sum();
        assert!(
            total < 2_000_000,
            "total assets size {total} bytes exceeds 2MB budget"
        );
    }
}
