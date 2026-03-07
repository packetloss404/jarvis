//! IPC handlers for chat relay streaming.

use jarvis_common::types::PaneKind;
use jarvis_webview::IpcPayload;

use crate::app_state::core::{ChatStreamHostState, JarvisApp};

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

    pub(in crate::app_state) fn stop_chat_stream_for_pane(&mut self, pane_id: u32, reason: &str) {
        let should_stop = self
            .chat_stream_host
            .as_ref()
            .map(|state| state.controller_pane_id == pane_id || state.source_pane_id == pane_id)
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
        let source_pane_id = match self.resolve_chat_stream_source(pane_id) {
            Some(id) => id,
            None => {
                self.chat_stream_respond_status(
                    pane_id,
                    req_id,
                    Some("focus a terminal pane before starting a live stream"),
                );
                return;
            }
        };

        let source = match self.tiling.pane(source_pane_id) {
            Some(pane) if pane.kind == PaneKind::Terminal => pane,
            _ => {
                self.chat_stream_respond_status(
                    pane_id,
                    req_id,
                    Some("selected pane is not a terminal"),
                );
                return;
            }
        };

        if !self.ptys.contains(source_pane_id) {
            self.chat_stream_respond_status(
                pane_id,
                req_id,
                Some("selected terminal is not ready yet"),
            );
            return;
        }

        self.chat_stream_host = Some(ChatStreamHostState {
            controller_pane_id: pane_id,
            source_pane_id,
            source_title: source.title.clone(),
        });

        self.chat_stream_respond_status(pane_id, req_id, None);
    }

    fn resolve_chat_stream_source(&self, controller_pane_id: u32) -> Option<u32> {
        let focused = self.tiling.focused_id();
        if focused != controller_pane_id
            && self
                .tiling
                .pane(focused)
                .map(|pane| pane.kind == PaneKind::Terminal)
                .unwrap_or(false)
        {
            return Some(focused);
        }

        if let Some(last) = self.last_terminal_focus {
            if last != controller_pane_id
                && self
                    .tiling
                    .pane(last)
                    .map(|pane| pane.kind == PaneKind::Terminal)
                    .unwrap_or(false)
            {
                return Some(last);
            }
        }

        self.tiling
            .panes_by_kind(PaneKind::Terminal)
            .into_iter()
            .find(|id| *id != controller_pane_id)
    }

    fn stop_chat_stream(&mut self, reason: &str) {
        let Some(state) = self.chat_stream_host.take() else {
            return;
        };

        let payload = serde_json::json!({
            "reason": reason,
        });

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
                "sourcePaneId": state.source_pane_id,
                "sourceTitle": state.source_title,
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
