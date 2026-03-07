//! IPC handlers for chat relay streaming.

use std::time::{Duration, Instant};

use jarvis_webview::IpcPayload;

use crate::app_state::core::{ChatStreamHostState, JarvisApp};

const CHAT_STREAM_FRAME_INTERVAL: Duration = Duration::from_millis(350);

impl JarvisApp {
    /// Handle a `chat_stream_control` IPC request from the chat panel.
    pub(in crate::app_state) fn handle_chat_stream_control(
        &mut self,
        pane_id: u32,
        payload: &IpcPayload,
    ) {
        let obj = match payload {
            IpcPayload::Json(v) => v,
            _ => return,
        };

        let req_id = obj.get("_reqId").and_then(|v| v.as_u64()).unwrap_or(0);
        let action = obj
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("status");

        match action {
            "status" => self.chat_stream_respond_status(pane_id, req_id, None),
            "start" => self.chat_stream_start(pane_id, req_id),
            "stop" => {
                self.stop_chat_stream_for_controller(pane_id, "stream stopped");
                self.chat_stream_respond_status(pane_id, req_id, None);
            }
            _ => self.chat_stream_respond_status(pane_id, req_id, Some("unknown action")),
        }
    }

    pub(in crate::app_state) fn poll_chat_stream(&mut self) {
        let Some(state) = self.chat_stream_host.clone() else {
            return;
        };

        if Instant::now().duration_since(self.last_chat_stream_frame_at)
            < CHAT_STREAM_FRAME_INTERVAL
        {
            return;
        }

        self.last_chat_stream_frame_at = Instant::now();

        let payload = match capture_workspace_frame(self) {
            Ok(frame) => serde_json::json!({
                "mime": "image/jpeg",
                "dataUrl": frame,
                "title": "Workspace",
            }),
            Err(error) => serde_json::json!({
                "error": error,
            }),
        };

        if let Some(ref registry) = self.webviews {
            if let Some(handle) = registry.get(state.controller_pane_id) {
                if let Err(e) = handle.send_ipc("chat_stream_host_frame", &payload) {
                    tracing::warn!(
                        pane_id = state.controller_pane_id,
                        error = %e,
                        "Failed to send chat stream frame"
                    );
                }
            }
        }
    }

    pub(in crate::app_state) fn stop_chat_stream_for_pane(&mut self, pane_id: u32, reason: &str) {
        let should_stop = self
            .chat_stream_host
            .as_ref()
            .map(|state| state.controller_pane_id == pane_id)
            .unwrap_or(false);

        if should_stop {
            self.stop_chat_stream(reason);
        }
    }

    pub(in crate::app_state) fn stop_chat_stream_for_controller(
        &mut self,
        pane_id: u32,
        reason: &str,
    ) {
        let should_stop = self
            .chat_stream_host
            .as_ref()
            .map(|state| state.controller_pane_id == pane_id)
            .unwrap_or(false);

        if should_stop {
            self.stop_chat_stream(reason);
        }
    }

    fn chat_stream_start(&mut self, pane_id: u32, req_id: u64) {
        if let Err(error) = ensure_workspace_capture_available() {
            self.chat_stream_respond_status(pane_id, req_id, Some(error));
            return;
        }

        self.chat_stream_host = Some(ChatStreamHostState {
            controller_pane_id: pane_id,
        });
        self.last_chat_stream_frame_at = Instant::now() - CHAT_STREAM_FRAME_INTERVAL;

        self.chat_stream_respond_status(pane_id, req_id, None);
    }

    fn stop_chat_stream(&mut self, reason: &str) {
        let Some(state) = self.chat_stream_host.take() else {
            return;
        };

        let payload = serde_json::json!({ "reason": reason });

        if let Some(ref registry) = self.webviews {
            if let Some(handle) = registry.get(state.controller_pane_id) {
                if let Err(e) = handle.send_ipc("chat_stream_host_stopped", &payload) {
                    tracing::warn!(
                        pane_id = state.controller_pane_id,
                        error = %e,
                        "Failed to notify chat stream stop"
                    );
                }
            }
        }
    }

    fn chat_stream_respond_status(&self, pane_id: u32, req_id: u64, error: Option<&str>) {
        let registry = match &self.webviews {
            Some(r) => r,
            None => return,
        };
        let handle = match registry.get(pane_id) {
            Some(h) => h,
            None => return,
        };

        let payload = match (&self.chat_stream_host, error) {
            (_, Some(error)) => serde_json::json!({
                "_reqId": req_id,
                "error": error,
                "relayUrl": self.config.relay.url.clone(),
            }),
            (Some(state), None) => serde_json::json!({
                "_reqId": req_id,
                "relayUrl": self.config.relay.url.clone(),
                "active": true,
                "sourceTitle": "Workspace",
                "isController": state.controller_pane_id == pane_id,
            }),
            (None, None) => serde_json::json!({
                "_reqId": req_id,
                "relayUrl": self.config.relay.url.clone(),
                "active": false,
            }),
        };

        if let Err(e) = handle.send_ipc("chat_stream_control_response", &payload) {
            tracing::warn!(pane_id, error = %e, "Failed to send chat_stream_control_response");
        }
    }
}

#[cfg(target_os = "macos")]
fn ensure_workspace_capture_available() -> Result<(), &'static str> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn ensure_workspace_capture_available() -> Result<(), &'static str> {
    Err("workspace streaming currently supports macOS only")
}

#[cfg(target_os = "macos")]
fn capture_workspace_frame(app: &JarvisApp) -> Result<String, String> {
    use core_graphics::display::{
        kCGNullWindowID, kCGWindowImageDefault, kCGWindowListOptionOnScreenOnly, CGDisplay,
    };
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use image::codecs::jpeg::JpegEncoder;
    use image::imageops::FilterType;
    use image::{DynamicImage, ImageBuffer, Rgba};

    let window = app
        .window
        .as_ref()
        .ok_or_else(|| "window unavailable".to_string())?;
    let pos = window
        .outer_position()
        .map_err(|e| format!("window position unavailable: {e}"))?;
    let size = window.outer_size();

    if size.width == 0 || size.height == 0 {
        return Err("window is not visible yet".into());
    }

    let bounds = CGRect::new(
        &CGPoint::new(pos.x as f64, pos.y as f64),
        &CGSize::new(size.width as f64, size.height as f64),
    );
    let image = CGDisplay::screenshot(
        bounds,
        kCGWindowListOptionOnScreenOnly,
        kCGNullWindowID,
        kCGWindowImageDefault,
    )
    .ok_or_else(|| "screen capture failed".to_string())?;

    let width = image.width() as u32;
    let height = image.height() as u32;
    let bytes_per_row = image.bytes_per_row();
    let raw = image.data().bytes().to_vec();

    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height as usize {
        let row = &raw[y * bytes_per_row..y * bytes_per_row + (width as usize * 4)];
        for px in row.chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }

    let image = ImageBuffer::<Rgba<u8>, _>::from_raw(width, height, rgba)
        .ok_or_else(|| "failed to decode screenshot".to_string())?;

    let dynamic = DynamicImage::ImageRgba8(image);
    let resized = dynamic.resize(640, 360, FilterType::Triangle).to_rgb8();

    let mut jpg = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut jpg, 35);
    encoder
        .encode_image(&DynamicImage::ImageRgb8(resized))
        .map_err(|e| format!("jpeg encode failed: {e}"))?;

    use base64::Engine as _;
    Ok(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(jpg)
    ))
}

#[cfg(not(target_os = "macos"))]
fn capture_workspace_frame(_app: &JarvisApp) -> Result<String, String> {
    Err("workspace streaming currently supports macOS only".into())
}
