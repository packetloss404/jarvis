//! Conversation session management.
//!
//! A `Session` holds the conversation history (messages), manages
//! context windows, and orchestrates the tool-call loop.

mod chat;
mod manager;
mod types;

pub use manager::Session;
pub use types::{
    tool_requires_approval, ApprovalDecision, ApprovalGate, ApprovalReceiver, ApprovalRequest,
    ToolEvent, ToolEventCallback, ToolExecutor, ToolOutcome, APPROVAL_REQUIRED_TOOLS,
    APPROVAL_TIMEOUT,
};
