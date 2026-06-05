//! AI assistant panel: key handling, runtime management, and event polling.

use jarvis_common::actions::Action;
use jarvis_common::types::PaneKind;

use super::assistant_task::assistant_task;
use super::core::JarvisApp;
use super::types::AssistantEvent;

impl JarvisApp {
    /// Handle key events for the assistant panel.
    pub(super) fn handle_assistant_key(&mut self, key_name: &str, is_press: bool) -> bool {
        if !is_press || !self.assistant_open {
            return false;
        }

        let panel = match self.assistant_panel.as_mut() {
            Some(p) => p,
            None => return false,
        };

        match key_name {
            "Escape" => {
                self.dispatch(Action::CloseOverlay);
                true
            }
            "Enter" => {
                let input = panel.take_input();
                if !input.is_empty() && !panel.is_streaming() {
                    panel.push_user_message(input.clone());
                    if let Some(ref tx) = self.assistant_tx {
                        let _ = tx.send(input);
                    }
                }
                true
            }
            "Backspace" => {
                panel.backspace();
                true
            }
            "Up" => {
                panel.scroll_up(3);
                true
            }
            "Down" => {
                panel.scroll_down(3);
                true
            }
            _ => {
                if key_name.len() == 1 {
                    let ch = key_name.chars().next().unwrap();
                    if ch.is_ascii_graphic() || ch == ' ' {
                        panel.append_char(ch);
                        return true;
                    }
                }
                false
            }
        }
    }

    /// Lazily initialize the async AI task and communication channels.
    pub(super) fn ensure_assistant_runtime(&mut self) {
        if self.assistant_tx.is_some() {
            return;
        }

        let (user_tx, user_rx) = std::sync::mpsc::channel::<String>();
        let (event_tx, event_rx) = std::sync::mpsc::channel::<AssistantEvent>();
        let (provider_tx, provider_rx) =
            std::sync::mpsc::channel::<jarvis_config::schema::AiProvider>();

        self.assistant_tx = Some(user_tx);
        self.assistant_rx = Some(event_rx);
        self.assistant_provider_tx = Some(provider_tx);

        // Snapshot the assistant config (provider selection + per-provider
        // overrides) for the async task. API keys are NOT here — they come from
        // env vars inside the factory.
        let assistant_config = self.config.assistant.clone();

        if self.tokio_runtime.is_none() {
            match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
            {
                Ok(rt) => self.tokio_runtime = Some(rt),
                Err(e) => {
                    tracing::error!("Failed to create tokio runtime: {e}");
                    return;
                }
            }
        }

        let rt = self.tokio_runtime.as_ref().unwrap();
        rt.spawn(async move {
            assistant_task(user_rx, event_tx, provider_rx, assistant_config).await;
        });
    }

    /// Switch the active AI provider at runtime (from the UI switcher).
    ///
    /// Persists the selection to config and forwards it to the async task,
    /// which rebuilds the client for subsequent turns. Starts the assistant
    /// runtime first if it isn't running yet.
    pub(super) fn set_ai_provider(&mut self, provider: jarvis_config::schema::AiProvider) {
        self.config.assistant.provider = provider;
        self.ensure_assistant_runtime();
        if let Some(ref tx) = self.assistant_provider_tx {
            if let Err(e) = tx.send(provider) {
                tracing::warn!(error = %e, "Failed to send provider switch");
            }
        }
    }

    /// Prune pending tool-approval senders whose async-side receiver has been
    /// dropped — i.e. the gate already resolved on its own (the 120s timeout
    /// fired, the Session ended, or the task died). For those entries the sender
    /// can never deliver a decision to anyone, so holding them only leaks memory.
    ///
    /// SAFETY: `oneshot::Sender::is_closed()` is true ONLY when the receiver was
    /// dropped, so this never discards a still-awaiting request — it cannot turn
    /// a would-be approve into a deny. The async side already failed closed for
    /// every pruned entry; this is pure cleanup, not a decision path.
    fn prune_stale_approvals(&mut self) {
        self.assistant_pending_approvals
            .retain(|_id, responder| !responder.is_closed());
    }

    /// Poll for assistant events from the async task (non-blocking).
    pub(super) fn poll_assistant(&mut self) {
        // Opportunistically drop senders whose receivers timed out / went away so
        // the pending-approvals map can't accumulate orphaned entries over a long
        // session. Cheap: one pass over a normally-tiny map per poll tick.
        if !self.assistant_pending_approvals.is_empty() {
            self.prune_stale_approvals();
        }

        if let Some(ref rx) = self.assistant_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    AssistantEvent::Initialized { model_name } => {
                        self.send_assistant_ipc(
                            "assistant_config",
                            &serde_json::json!({ "model_name": model_name }),
                        );
                    }
                    AssistantEvent::ProviderChanged { ref provider } => {
                        self.send_assistant_ipc(
                            "assistant_provider",
                            &serde_json::json!({ "provider": provider }),
                        );
                    }
                    AssistantEvent::StreamChunk(ref chunk) => {
                        if let Some(ref mut panel) = self.assistant_panel {
                            panel.append_streaming_chunk(chunk);
                        }
                        self.send_assistant_ipc(
                            "assistant_chunk",
                            &serde_json::json!({ "text": chunk }),
                        );
                    }
                    AssistantEvent::ToolCall { ref name, ref input } => {
                        self.send_assistant_ipc(
                            "tool_call",
                            &serde_json::json!({ "name": name, "input": input }),
                        );
                    }
                    AssistantEvent::ToolResult {
                        ref id,
                        ref name,
                        ref summary,
                        is_error,
                    } => {
                        self.send_assistant_ipc(
                            "tool_result",
                            &serde_json::json!({
                                "id": id,
                                "name": name,
                                "summary": summary,
                                "is_error": is_error,
                            }),
                        );
                    }
                    AssistantEvent::ToolApprovalRequest { request, responder } => {
                        // Stash the responder so the panel's approve/deny IPC can
                        // resolve it; forward the request to the panel for display.
                        // If a stale entry exists for this id, dropping it fails
                        // that prior gate closed (deny) — safe by construction.
                        self.assistant_pending_approvals
                            .insert(request.id.clone(), responder);
                        self.send_assistant_ipc(
                            "tool_approval_request",
                            &serde_json::json!({
                                "id": request.id,
                                "tool": request.tool,
                                "summary": request.summary,
                            }),
                        );
                    }
                    AssistantEvent::Done => {
                        // Capture the accumulated text before finishing
                        let full_text = self
                            .assistant_panel
                            .as_ref()
                            .map(|p| p.streaming_text().to_string())
                            .unwrap_or_default();
                        if let Some(ref mut panel) = self.assistant_panel {
                            panel.finish_streaming();
                        }
                        self.send_assistant_ipc(
                            "assistant_output",
                            &serde_json::json!({ "text": full_text }),
                        );
                    }
                    AssistantEvent::Error(ref msg) => {
                        tracing::warn!("Assistant error: {msg}");
                        if let Some(ref mut panel) = self.assistant_panel {
                            panel.set_error(msg.clone());
                            panel.finish_streaming();
                        }
                        self.send_assistant_ipc(
                            "assistant_error",
                            &serde_json::json!({ "message": msg }),
                        );
                    }
                }
                self.needs_redraw = true;
            }
        }
    }

    /// Send an IPC message to all assistant webview panes.
    fn send_assistant_ipc(&self, kind: &str, payload: &serde_json::Value) {
        let pane_ids = self.tiling.panes_by_kind(PaneKind::Assistant);
        if let Some(ref registry) = self.webviews {
            for pane_id in pane_ids {
                if let Some(handle) = registry.get(pane_id) {
                    if let Err(e) = handle.send_ipc(kind, payload) {
                        tracing::warn!(pane_id, kind, error = %e, "Failed to send assistant IPC");
                    }
                }
            }
        }
    }
}
