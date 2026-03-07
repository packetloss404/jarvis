//! IPC handlers for the emulator panel.
//!
//! Handles `emulator_list_roms` (scan ~/ROMs directory) and
//! `emulator_load_rom` (read a ROM file and return as base64).

use jarvis_webview::IpcPayload;

use crate::app_state::core::JarvisApp;

/// Maximum ROM file size: 64 MB.
const MAX_ROM_SIZE: u64 = 64 * 1024 * 1024;

/// Allowed ROM file extensions.
const ROM_EXTENSIONS: &[&str] = &[
    "nes", "smc", "sfc", "gb", "gbc", "gba", "nds", "n64", "z64", "v64", "bin", "cue", "iso", "md",
    "smd", "sms", "gg", "a26", "zip", "7z",
];

/// Return the ROMs directory path (~/ ROMs).
fn roms_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join("ROMs"))
}

impl JarvisApp {
    /// Handle `emulator_list_roms` — scan ~/ROMs and return filenames.
    pub(in crate::app_state) fn handle_emulator_list_roms(
        &mut self,
        pane_id: u32,
        payload: &IpcPayload,
    ) {
        let req_id = match payload {
            IpcPayload::Json(v) => v.get("_reqId").and_then(|v| v.as_u64()).unwrap_or(0),
            _ => 0,
        };

        let dir = match roms_dir() {
            Some(d) => d,
            None => {
                self.emulator_respond(
                    pane_id,
                    req_id,
                    "emulator_list_roms_response",
                    None,
                    Some("cannot determine home directory"),
                );
                return;
            }
        };

        if !dir.is_dir() {
            self.emulator_respond(
                pane_id,
                req_id,
                "emulator_list_roms_response",
                None,
                Some("~/ROMs directory not found"),
            );
            return;
        }

        let mut roms = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if ROM_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            roms.push(name.to_string());
                        }
                    }
                }
            }
        }

        roms.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));

        let registry = match &self.webviews {
            Some(r) => r,
            None => return,
        };
        let handle = match registry.get(pane_id) {
            Some(h) => h,
            None => return,
        };

        let payload = serde_json::json!({
            "_reqId": req_id,
            "roms": roms,
        });

        if let Err(e) = handle.send_ipc("emulator_list_roms_response", &payload) {
            tracing::warn!(pane_id, error = %e, "Failed to send emulator_list_roms_response");
        }
    }

    /// Handle `emulator_load_rom` — read a ROM from ~/ROMs and return as base64.
    pub(in crate::app_state) fn handle_emulator_load_rom(
        &mut self,
        pane_id: u32,
        payload: &IpcPayload,
    ) {
        let obj = match payload {
            IpcPayload::Json(v) => v,
            _ => {
                tracing::warn!(pane_id, "emulator_load_rom: expected JSON payload");
                return;
            }
        };

        let req_id = obj.get("_reqId").and_then(|v| v.as_u64()).unwrap_or(0);

        let filename = match obj.get("filename").and_then(|v| v.as_str()) {
            Some(f) => f,
            None => {
                self.emulator_respond(
                    pane_id,
                    req_id,
                    "emulator_load_rom_response",
                    None,
                    Some("missing filename"),
                );
                return;
            }
        };

        // Prevent directory traversal
        if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
            self.emulator_respond(
                pane_id,
                req_id,
                "emulator_load_rom_response",
                None,
                Some("invalid filename"),
            );
            return;
        }

        let dir = match roms_dir() {
            Some(d) => d,
            None => {
                self.emulator_respond(
                    pane_id,
                    req_id,
                    "emulator_load_rom_response",
                    None,
                    Some("cannot determine home directory"),
                );
                return;
            }
        };

        let path = dir.join(filename);

        // Extra traversal check via canonicalize
        let canonical_dir = match std::fs::canonicalize(&dir) {
            Ok(c) => c,
            Err(_) => {
                self.emulator_respond(
                    pane_id,
                    req_id,
                    "emulator_load_rom_response",
                    None,
                    Some("ROMs directory not found"),
                );
                return;
            }
        };
        let canonical_path = match std::fs::canonicalize(&path) {
            Ok(c) => c,
            Err(_) => {
                self.emulator_respond(
                    pane_id,
                    req_id,
                    "emulator_load_rom_response",
                    None,
                    Some("ROM file not found"),
                );
                return;
            }
        };
        if !canonical_path.starts_with(&canonical_dir) {
            self.emulator_respond(
                pane_id,
                req_id,
                "emulator_load_rom_response",
                None,
                Some("invalid path"),
            );
            return;
        }

        let metadata = match std::fs::metadata(&canonical_path) {
            Ok(m) => m,
            Err(e) => {
                self.emulator_respond(
                    pane_id,
                    req_id,
                    "emulator_load_rom_response",
                    None,
                    Some(&format!("file error: {e}")),
                );
                return;
            }
        };

        if metadata.len() > MAX_ROM_SIZE {
            self.emulator_respond(
                pane_id,
                req_id,
                "emulator_load_rom_response",
                None,
                Some("ROM too large (max 64MB)"),
            );
            return;
        }

        let bytes = match std::fs::read(&canonical_path) {
            Ok(b) => b,
            Err(e) => {
                self.emulator_respond(
                    pane_id,
                    req_id,
                    "emulator_load_rom_response",
                    None,
                    Some(&format!("read error: {e}")),
                );
                return;
            }
        };

        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

        let registry = match &self.webviews {
            Some(r) => r,
            None => return,
        };
        let handle = match registry.get(pane_id) {
            Some(h) => h,
            None => return,
        };

        let payload = serde_json::json!({
            "_reqId": req_id,
            "data": b64,
            "filename": filename,
        });

        if let Err(e) = handle.send_ipc("emulator_load_rom_response", &payload) {
            tracing::warn!(pane_id, error = %e, "Failed to send emulator_load_rom_response");
        }
    }

    /// Generic response helper for emulator IPC messages.
    fn emulator_respond(
        &self,
        pane_id: u32,
        req_id: u64,
        kind: &str,
        data: Option<&serde_json::Value>,
        error: Option<&str>,
    ) {
        let registry = match &self.webviews {
            Some(r) => r,
            None => return,
        };
        let handle = match registry.get(pane_id) {
            Some(h) => h,
            None => return,
        };

        let payload = if let Some(d) = data {
            let mut v = d.clone();
            if let Some(obj) = v.as_object_mut() {
                obj.insert("_reqId".into(), serde_json::json!(req_id));
            }
            v
        } else {
            serde_json::json!({
                "_reqId": req_id,
                "error": error.unwrap_or("unknown error"),
            })
        };

        if let Err(e) = handle.send_ipc(kind, &payload) {
            tracing::warn!(pane_id, error = %e, "Failed to send {}", kind);
        }
    }
}
