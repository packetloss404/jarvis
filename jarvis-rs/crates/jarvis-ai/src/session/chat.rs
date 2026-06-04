//! Async chat methods for Session (send_message + streaming) with tool loops.

use std::sync::Arc;

use crate::{AiClient, AiError, AiResponse, ContentBlock, Message, Role};

use super::manager::Session;
use super::types::{BusyGuard, ToolEvent, ToolExecutor, ToolOutcome};

impl Session {
    /// Add a user message and get the assistant's response.
    /// If the AI calls tools, this runs the tool-call loop automatically.
    pub async fn chat(
        &mut self,
        client: &dyn AiClient,
        user_message: impl Into<String>,
    ) -> Result<String, AiError> {
        let _guard = BusyGuard::acquire(&self.busy)?;

        self.messages
            .push(Message::text(Role::User, user_message.into()));

        let mut rounds = 0;
        loop {
            let messages = self.build_messages();
            let response = client.send_message(&messages, &self.tools).await?;
            self.tracker.record(&self.provider, &response.usage);

            if !self.should_run_tools(&response) {
                self.messages
                    .push(Message::text(Role::Assistant, response.content.clone()));
                return Ok(response.content);
            }

            rounds += 1;
            if rounds > self.max_tool_rounds {
                tracing::warn!("Max tool rounds reached, returning partial response");
                self.messages
                    .push(Message::text(Role::Assistant, response.content.clone()));
                return Ok(response.content);
            }

            self.run_tool_round(&response).await;
        }
    }

    /// Send a message with streaming, returning the full response.
    ///
    /// Control flow per round:
    /// 1. Stream the assistant response (text chunks go to `on_chunk`).
    /// 2. If the response has no tool calls, or no executor is set, finish.
    /// 3. Otherwise run each requested tool (read-only) on a blocking thread,
    ///    append a proper assistant `tool_use` turn and a user `tool_result`
    ///    turn, surface activity via the tool-event callback, and re-call.
    /// 4. `max_tool_rounds` is a hard cap to prevent runaway loops.
    pub async fn chat_streaming(
        &mut self,
        client: &dyn AiClient,
        user_message: impl Into<String>,
        on_chunk: Box<dyn Fn(String) + Send + Sync>,
    ) -> Result<String, AiError> {
        let _guard = BusyGuard::acquire(&self.busy)?;

        self.messages
            .push(Message::text(Role::User, user_message.into()));

        // Share the chunk callback across rounds (re-called each loop iteration).
        let on_chunk: Arc<dyn Fn(String) + Send + Sync> = Arc::from(on_chunk);

        let mut rounds = 0;
        loop {
            let messages = self.build_messages();
            let cb = on_chunk.clone();
            let per_round: Box<dyn Fn(String) + Send + Sync> =
                Box::new(move |c| cb(c));
            let response = client
                .send_message_streaming(&messages, &self.tools, per_round)
                .await?;
            self.tracker.record(&self.provider, &response.usage);

            if !self.should_run_tools(&response) {
                self.messages
                    .push(Message::text(Role::Assistant, response.content.clone()));
                return Ok(response.content);
            }

            rounds += 1;
            if rounds > self.max_tool_rounds {
                tracing::warn!("Max tool rounds reached, returning partial response");
                self.messages
                    .push(Message::text(Role::Assistant, response.content.clone()));
                return Ok(response.content);
            }

            self.run_tool_round(&response).await;
        }
    }

    /// Whether this response should trigger a tool round (has tool calls AND an
    /// executor is configured).
    fn should_run_tools(&self, response: &AiResponse) -> bool {
        !response.tool_calls.is_empty() && self.tool_executor.is_some()
    }

    /// Execute all tool calls in `response`, appending the assistant `tool_use`
    /// turn and the following user `tool_result` turn to the conversation.
    ///
    /// The sync executor runs on a blocking thread so the async loop is never
    /// blocked. Each call/result is surfaced through the tool-event callback.
    async fn run_tool_round(&mut self, response: &AiResponse) {
        // 1. Assistant turn: text (if any) + tool_use blocks.
        let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
        if !response.content.is_empty() {
            assistant_blocks.push(ContentBlock::Text {
                text: response.content.clone(),
            });
        }
        for call in &response.tool_calls {
            assistant_blocks.push(ContentBlock::ToolUse {
                id: call.id.clone(),
                name: call.name.clone(),
                input: call.arguments.clone(),
            });
        }
        self.messages
            .push(Message::blocks(Role::Assistant, assistant_blocks));

        // 2. Run each tool and collect matching tool_result blocks.
        let executor = self
            .tool_executor
            .as_ref()
            .expect("run_tool_round called without executor");
        let mut result_blocks: Vec<ContentBlock> = Vec::new();
        for call in &response.tool_calls {
            // Surface the call.
            self.emit_tool_event(ToolEvent::Call {
                name: call.name.clone(),
                input: call.arguments.clone(),
            });

            let outcome =
                run_tool_blocking(executor.clone(), &call.name, &call.arguments).await;

            // Surface the result (summarized for display).
            self.emit_tool_event(ToolEvent::Result {
                name: call.name.clone(),
                summary: summarize(&outcome.content),
                is_error: outcome.is_error,
            });

            result_blocks.push(ContentBlock::ToolResult {
                tool_use_id: call.id.clone(),
                content: outcome.content,
                is_error: outcome.is_error,
            });
        }

        // 3. User turn carrying the tool_result blocks.
        self.messages
            .push(Message::blocks(Role::User, result_blocks));
    }

    fn emit_tool_event(&self, event: ToolEvent) {
        if let Some(ref cb) = self.tool_event_callback {
            cb(event);
        }
    }
}

/// Run a sync tool executor on a blocking thread so it doesn't stall the async
/// runtime. Falls back to inline execution if not on a Tokio runtime.
async fn run_tool_blocking(
    executor: Arc<ToolExecutor>,
    name: &str,
    args: &serde_json::Value,
) -> ToolOutcome {
    let name = name.to_string();
    let args = args.clone();
    match tokio::runtime::Handle::try_current() {
        Ok(_) => {
            tokio::task::spawn_blocking(move || executor(&name, &args))
                .await
                .unwrap_or_else(|e| ToolOutcome::error(format!("tool task failed: {e}")))
        }
        Err(_) => executor(&name, &args),
    }
}

/// Produce a short single-line summary of a tool result for inline display.
fn summarize(content: &str) -> String {
    let first_line = content.lines().next().unwrap_or("").trim();
    let line_count = content.lines().count();
    let mut s: String = first_line.chars().take(120).collect();
    if line_count > 1 {
        s.push_str(&format!(" (+{} more lines)", line_count - 1));
    }
    if s.is_empty() {
        s.push_str("(no output)");
    }
    s
}
