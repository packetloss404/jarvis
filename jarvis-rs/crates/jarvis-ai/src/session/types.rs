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
    Result {
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
