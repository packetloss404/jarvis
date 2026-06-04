//! Session struct and conversation management.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::token_tracker::TokenTracker;
use crate::{Message, Role, ToolDefinition};

use super::types::{ToolEventCallback, ToolExecutor};

/// A conversation session with message history and tool execution.
pub struct Session {
    /// Conversation message history.
    pub(super) messages: Vec<Message>,
    /// System prompt (prepended to every API call).
    pub(super) system_prompt: Option<String>,
    /// Available tool definitions.
    pub(super) tools: Vec<ToolDefinition>,
    /// Tool executor callback (shared so it can run on a blocking thread).
    pub(super) tool_executor: Option<Arc<ToolExecutor>>,
    /// Optional callback surfacing tool activity to the app layer.
    pub(super) tool_event_callback: Option<Arc<ToolEventCallback>>,
    /// Token usage tracker.
    pub(super) tracker: TokenTracker,
    /// Maximum tool-call loop iterations to prevent infinite loops.
    pub(super) max_tool_rounds: u32,
    /// Provider name for token tracking.
    pub(super) provider: String,
    /// Whether the session is currently processing a request.
    pub(super) busy: Arc<AtomicBool>,
}

impl Session {
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            messages: Vec::new(),
            system_prompt: None,
            tools: Vec::new(),
            tool_executor: None,
            tool_event_callback: None,
            tracker: TokenTracker::new(),
            max_tool_rounds: 10,
            provider: provider.into(),
            busy: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_tool_executor(mut self, executor: ToolExecutor) -> Self {
        self.tool_executor = Some(Arc::new(executor));
        self
    }

    /// Register a callback that is invoked as tool calls/results occur, so the
    /// app layer can render tool activity inline.
    pub fn with_tool_event_callback(mut self, callback: ToolEventCallback) -> Self {
        self.tool_event_callback = Some(Arc::new(callback));
        self
    }

    pub fn with_max_tool_rounds(mut self, max: u32) -> Self {
        self.max_tool_rounds = max;
        self
    }

    pub(crate) fn build_messages(&self) -> Vec<Message> {
        let mut msgs = Vec::new();
        if let Some(ref system) = self.system_prompt {
            msgs.push(Message::text(Role::System, system.clone()));
        }
        msgs.extend(self.messages.clone());
        msgs
    }

    /// Get the full conversation history.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Get the token tracker.
    pub fn tracker(&self) -> &TokenTracker {
        &self.tracker
    }

    /// Clear conversation history.
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Number of messages in history.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new("default")
    }
}
