//! Internal types and constants for the app state module.

use std::time::Duration;

use jarvis_ai::{ApprovalDecision, ApprovalRequest};
use jarvis_social::UserStatus;

/// Events received from the async AI task.
pub(super) enum AssistantEvent {
    /// The assistant runtime has initialized with the given model.
    Initialized { model_name: String },
    /// The active AI provider changed (e.g. via the UI switcher). Carries the
    /// lowercase provider label ("claude" | "openai" | "minimax").
    ProviderChanged { provider: String },
    /// A streaming text chunk arrived.
    StreamChunk(String),
    /// The assistant requested a (read-only) tool call.
    ToolCall {
        name: String,
        input: serde_json::Value,
    },
    /// A tool call finished with the given short summary. `id` is the
    /// originating tool-call id so the panel can correlate the result to the
    /// exact approval card / tool bubble (FIFO matching is unsafe — read-only
    /// and denied results interleave).
    ToolResult {
        id: String,
        name: String,
        summary: String,
        is_error: bool,
    },
    /// The async tool loop is requesting human approval for a mutating/exec
    /// tool call (write_file / run_command). Carries the request (id + tool +
    /// human-readable summary) and the oneshot SENDER the main thread must
    /// resolve once the human answers (via the panel's approve/deny IPC).
    ///
    /// The main thread stashes the sender in its pending-approvals map keyed by
    /// `request.id`, forwards the request to the panel, and resolves the sender
    /// on approve/deny. If the sender is dropped (e.g. panel gone), the async
    /// side's awaiting gate fails closed (deny). A 120s timeout on the async
    /// side also fails closed independently.
    ToolApprovalRequest {
        request: ApprovalRequest,
        responder: tokio::sync::oneshot::Sender<ApprovalDecision>,
    },
    /// The full response is complete.
    Done,
    /// An error occurred.
    Error(String),
}

/// Commands sent from the sync main thread to the async presence task.
pub(super) enum PresenceCommand {
    /// Send a poke to a specific user.
    Poke { target_user_id: String },
    /// Update our activity status.
    UpdateActivity {
        status: UserStatus,
        activity: Option<String>,
    },
}

/// How often to poll for events (approx 60 Hz).
pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(16);
