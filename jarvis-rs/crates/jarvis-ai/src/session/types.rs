//! Session types and concurrency guards.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::AiError;

/// The outcome of executing a single tool call.
#[derive(Debug, Clone)]
pub struct ToolOutcome {
    /// The tool's output (or error text when `is_error` is set).
    pub content: String,
    /// Whether the tool failed (surfaced to the model as `is_error`).
    pub is_error: bool,
}

impl ToolOutcome {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

/// Callback for executing tool calls. Takes a tool name + arguments,
/// returns the tool's [`ToolOutcome`]. This runs synchronously; the session
/// drives it via `spawn_blocking` so the async loop is never blocked.
pub type ToolExecutor =
    Box<dyn Fn(&str, &serde_json::Value) -> ToolOutcome + Send + Sync>;

/// The human's decision on a mutating/exec tool-call approval request.
///
/// A `Deny` is the FAIL-CLOSED default: it is what the gate resolves to on an
/// explicit deny, on a timeout, AND if the decision channel is dropped without
/// a response. The model receives an `is_error` tool_result and nothing runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// The human explicitly approved the tool call. Execution may proceed.
    Approve,
    /// The human denied the call, or the request timed out / was dropped.
    Deny,
}

impl ApprovalDecision {
    /// Whether this decision permits execution.
    pub fn is_approved(self) -> bool {
        matches!(self, ApprovalDecision::Approve)
    }
}

/// A request for human approval of a single mutating/exec tool call.
///
/// Created by the Session tool loop the moment it encounters a tool that
/// requires approval (e.g. `write_file`, `run_command`). It is handed to the
/// [`ApprovalGate`] callback, which forwards it to the UI (across threads) and
/// returns a channel that eventually resolves to an [`ApprovalDecision`].
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// Unique id for this request, used by the app layer to key the pending
    /// decision channel and to match the UI's approve/deny response back.
    pub id: String,
    /// The tool the model wants to run (e.g. "write_file", "run_command").
    pub tool: String,
    /// A human-readable, single-screen summary of EXACTLY what will happen
    /// (the command argv, the target path, a content preview, etc.). This is
    /// what the human sees and approves.
    pub summary: String,
}

/// The decision channel returned by an [`ApprovalGate`].
///
/// The gate hands one end (the [`tokio::sync::oneshot::Sender`]) to the app
/// layer's pending-approvals map and returns the receiver here; the Session
/// awaits it (under a timeout) to learn the human's decision.
pub type ApprovalReceiver = tokio::sync::oneshot::Receiver<ApprovalDecision>;

/// The approval seam installed on a [`Session`](super::manager::Session).
///
/// Given an [`ApprovalRequest`], it must:
/// 1. register a pending decision channel keyed by `request.id`,
/// 2. forward the request to the human (UI), and
/// 3. return the receiving end of that channel.
///
/// The Session then awaits the returned [`ApprovalReceiver`] under a wall-clock
/// timeout; a timeout, an explicit deny, or a dropped sender all FAIL CLOSED to
/// [`ApprovalDecision::Deny`]. The closure runs on the async task thread, so it
/// must be cheap and non-blocking (just send a message + stash a sender).
pub type ApprovalGate =
    Box<dyn Fn(ApprovalRequest) -> ApprovalReceiver + Send + Sync>;

/// Default wall-clock timeout for a pending approval. On expiry the gate
/// FAILS CLOSED (treats the request as denied) so a tool can never run because
/// the human merely never answered.
pub const APPROVAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Tool names that MUST pass the approval gate before executing. Read-only
/// tools are intentionally absent — they never prompt.
pub const APPROVAL_REQUIRED_TOOLS: &[&str] = &["write_file", "run_command"];

/// Whether a tool of this name requires human approval before it may run.
pub fn tool_requires_approval(name: &str) -> bool {
    APPROVAL_REQUIRED_TOOLS.contains(&name)
}

/// Callback invoked as tool activity happens, so the app layer can surface it
/// (e.g. render an inline "read_file(path)" line). Receives a [`ToolEvent`].
pub type ToolEventCallback = Box<dyn Fn(ToolEvent) + Send + Sync>;

/// A tool-activity event surfaced to the app layer during the tool loop.
#[derive(Debug, Clone)]
pub enum ToolEvent {
    /// The model requested a tool call.
    Call {
        name: String,
        input: serde_json::Value,
    },
    /// A tool call finished with the given (possibly truncated) summary.
    ///
    /// `id` is the originating tool-call id, so the UI can correlate this result
    /// to the exact approval card / tool bubble it belongs to (read-only and
    /// denied results also flow through here, so positional matching is unsafe).
    Result {
        id: String,
        name: String,
        summary: String,
        is_error: bool,
    },
}

/// Guard that clears the `busy` flag on drop, ensuring it is always released
/// even if the future is cancelled or an early return occurs.
///
/// Holds an owned `Arc` clone of the flag (rather than borrowing the session)
/// so the guard can live across `&mut self` calls in the tool loop.
pub(crate) struct BusyGuard {
    flag: Arc<AtomicBool>,
}

impl BusyGuard {
    /// Attempt to acquire the busy lock. Returns `Err` if already busy.
    pub(crate) fn acquire(flag: &Arc<AtomicBool>) -> Result<Self, AiError> {
        if flag
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Err(AiError::ApiError(
                "Session is busy with another request".into(),
            ));
        }
        Ok(Self { flag: flag.clone() })
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}
