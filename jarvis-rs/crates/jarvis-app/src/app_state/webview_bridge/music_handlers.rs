//! IPC handlers for the music player plugin.
//!
//! Handles library scanning, searching, and directory configuration.
//! Audio streaming is handled by the `jarvis://` custom protocol in
//! the webview content provider.

use std::path::PathBuf;

use jarvis_webview::IpcPayload;

use crate::app_state::core::JarvisApp;
use crate::app_state::music_library;

impl JarvisApp {
    /// Handle `music_init` — load cached library or return empty state.
    pub(in crate::app_state) fn handle_music_init(
        &mut self,
        pane_id: u32,
        payload: &IpcPayload,
    ) {
        let req_id = extract_req_id(payload);

        // Try loading from cache if we don't have it in memory yet
        if self.music_library.is_none() {
            self.music_library = music_library::load_cached_library();
        }

        let (music_dir, tracks, cached) = match &self.music_library {
            Some(lib) => (
                lib.music_dir.clone(),
                serde_json::to_value(&lib.tracks).unwrap_or_default(),
                true,
            ),
            None => {
                let dir = music_library::default_music_dir();
                (dir.to_string_lossy().to_string(), serde_json::json!([]), false)
            }
        };

        let payload = serde_json::json!({
            "_reqId": req_id,
            "music_dir": music_dir,
            "tracks": tracks,
            "cached": cached,
        });

        self.music_send(pane_id, "music_init_response", &payload);
    }

    /// Handle `music_scan` — scan directory for audio files.
    pub(in crate::app_state) fn handle_music_scan(
        &mut self,
        pane_id: u32,
        payload: &IpcPayload,
    ) {
        let req_id = extract_req_id(payload);

        let dir = match payload {
            IpcPayload::Json(v) => v
                .get("path")
                .and_then(|p| p.as_str())
                .map(|s| PathBuf::from(s)),
            _ => None,
        }
        .unwrap_or_else(|| {
            self.music_library
                .as_ref()
                .map(|l| PathBuf::from(&l.music_dir))
                .unwrap_or_else(music_library::default_music_dir)
        });

        if !dir.is_dir() {
            let payload = serde_json::json!({
                "_reqId": req_id,
                "error": format!("Directory not found: {}", dir.display()),
            });
            self.music_send(pane_id, "music_scan_response", &payload);
            return;
        }

        tracing::info!(path = %dir.display(), "Scanning music directory");
        let library = music_library::scan_directory(&dir);
        let track_count = library.tracks.len();
        tracing::info!(track_count, "Music scan complete");

        music_library::save_library_cache(&library);

        let payload = serde_json::json!({
            "_reqId": req_id,
            "music_dir": library.music_dir,
            "tracks": library.tracks,
            "track_count": track_count,
        });

        self.music_library = Some(library);
        self.music_send(pane_id, "music_scan_response", &payload);
    }

    /// Handle `music_search` — filter tracks by query.
    pub(in crate::app_state) fn handle_music_search(
        &mut self,
        pane_id: u32,
        payload: &IpcPayload,
    ) {
        let req_id = extract_req_id(payload);

        let query = match payload {
            IpcPayload::Json(v) => v.get("query").and_then(|q| q.as_str()).unwrap_or(""),
            _ => "",
        }
        .to_lowercase();

        let tracks: Vec<_> = self
            .music_library
            .as_ref()
            .map(|lib| {
                lib.tracks
                    .iter()
                    .filter(|t| {
                        t.title
                            .as_deref()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&query)
                            || t.artist
                                .as_deref()
                                .unwrap_or("")
                                .to_lowercase()
                                .contains(&query)
                            || t.album
                                .as_deref()
                                .unwrap_or("")
                                .to_lowercase()
                                .contains(&query)
                    })
                    .collect()
            })
            .unwrap_or_default();

        let payload = serde_json::json!({
            "_reqId": req_id,
            "tracks": tracks,
        });

        self.music_send(pane_id, "music_search_response", &payload);
    }

    /// Handle `music_set_dir` — change the music directory.
    pub(in crate::app_state) fn handle_music_set_dir(
        &mut self,
        pane_id: u32,
        payload: &IpcPayload,
    ) {
        let req_id = extract_req_id(payload);

        let path = match payload {
            IpcPayload::Json(v) => v.get("path").and_then(|p| p.as_str()),
            _ => None,
        };

        let path = match path {
            Some(p) => p,
            None => {
                let payload = serde_json::json!({
                    "_reqId": req_id,
                    "error": "missing path",
                });
                self.music_send(pane_id, "music_set_dir_response", &payload);
                return;
            }
        };

        let dir = PathBuf::from(path);
        if !dir.is_dir() {
            let payload = serde_json::json!({
                "_reqId": req_id,
                "error": format!("Not a directory: {}", path),
            });
            self.music_send(pane_id, "music_set_dir_response", &payload);
            return;
        }

        // Update the stored directory (will be used on next scan)
        if let Some(ref mut lib) = self.music_library {
            lib.music_dir = path.to_string();
            music_library::save_library_cache(lib);
        }

        let payload = serde_json::json!({
            "_reqId": req_id,
            "music_dir": path,
        });
        self.music_send(pane_id, "music_set_dir_response", &payload);
    }

    /// Send a music IPC response to a pane.
    fn music_send(&self, pane_id: u32, kind: &str, payload: &serde_json::Value) {
        if let Some(ref registry) = self.webviews {
            if let Some(handle) = registry.get(pane_id) {
                if let Err(e) = handle.send_ipc(kind, payload) {
                    tracing::warn!(pane_id, error = %e, "Failed to send {kind}");
                }
            }
        }
    }
}

fn extract_req_id(payload: &IpcPayload) -> u64 {
    match payload {
        IpcPayload::Json(v) => v.get("_reqId").and_then(|v| v.as_u64()).unwrap_or(0),
        _ => 0,
    }
}
