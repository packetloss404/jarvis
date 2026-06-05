//! AI engine for Jarvis.
//!
//! Provides Claude and Whisper API clients with:
//! - Streaming (SSE) support
//! - Tool calling (function use)
//! - Session management with automatic tool-call loops
//! - Token usage tracking

pub mod claude;
pub mod gemini;
pub mod openai;
pub mod session;
pub mod streaming;
pub mod token_tracker;
pub mod tools;
pub mod whisper;

use async_trait::async_trait;

pub use claude::{ClaudeClient, ClaudeConfig};
pub use gemini::{GeminiClient, GeminiConfig};
pub use openai::{OpenAiClient, OpenAiConfig};
pub use session::{
    ApprovalDecision, ApprovalGate, ApprovalReceiver, ApprovalRequest, Session, ToolEvent,
    ToolEventCallback, ToolExecutor, ToolOutcome, APPROVAL_REQUIRED_TOOLS, APPROVAL_TIMEOUT,
};
pub use token_tracker::TokenTracker;
pub use tools::{
    builtin_tools, read_only_tools, ReadOnlyToolExecutor, WriteExecToolExecutor, WRITE_EXEC_TOOLS,
};
pub use whisper::{WhisperClient, WhisperConfig};

#[async_trait]
pub trait AiClient: Send + Sync {
    async fn send_message(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<AiResponse, AiError>;

    async fn send_message_streaming(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        on_chunk: Box<dyn Fn(String) + Send + Sync>,
    ) -> Result<AiResponse, AiError>;
}

/// A single conversation message.
///
/// Most messages are plain text (`content`), but a message may also carry
/// structured content blocks (`blocks`) to represent Claude `tool_use` and
/// `tool_result` turns. When `blocks` is non-empty, request serialization
/// emits a content-block array; otherwise it emits the plain `content` string
/// (back-compat with the original text-only model).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// Optional structured content blocks (tool_use / tool_result). When empty,
    /// the message is treated as a plain-text message carrying `content`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<ContentBlock>,
}

impl Message {
    /// Construct a plain-text message (back-compat constructor).
    pub fn text(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            blocks: Vec::new(),
        }
    }

    /// Construct a message carrying structured content blocks.
    pub fn blocks(role: Role, blocks: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content: String::new(),
            blocks,
        }
    }
}

/// A structured content block within a message.
///
/// Mirrors the subset of Claude's content-block model that Jarvis uses:
/// assistant `tool_use` blocks and user `tool_result` blocks (plus plain text).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text block.
    Text { text: String },
    /// A tool invocation requested by the assistant.
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// The result of executing a tool, sent back as a user turn.
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct AiResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: TokenUsage,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl TokenUsage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("API error: {0}")]
    ApiError(String),
    #[error("Rate limited")]
    RateLimited,
    #[error("Network error: {0}")]
    NetworkError(String),
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Timeout")]
    Timeout,
}
