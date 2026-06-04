//! Background async task that manages the Claude AI session.

use jarvis_ai::{ReadOnlyToolExecutor, ToolEvent, ToolOutcome};

use super::types::AssistantEvent;

/// System prompt for the read-only agentic assistant.
const SYSTEM_PROMPT: &str = "You are Jarvis, an AI assistant embedded in a terminal emulator. \
     You have READ-ONLY access to the project workspace via tools: read_file, \
     search_files, search_content, and list_directory. Use them to ground your \
     answers in the actual files. You CANNOT run commands or modify files. \
     Be concise and helpful. Use plain text, not markdown.";

/// Resolve the workspace directory the tool sandbox is rooted at.
///
/// Uses the current working directory (the project dir) and NEVER the user's
/// home directory. Returns a canonicalized absolute path.
fn workspace_root() -> std::path::PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    std::fs::canonicalize(&cwd).unwrap_or(cwd)
}

/// Background task that manages the Claude AI session.
pub(super) async fn assistant_task(
    user_rx: std::sync::mpsc::Receiver<String>,
    event_tx: std::sync::mpsc::Sender<AssistantEvent>,
) {
    let config = match jarvis_ai::ClaudeConfig::from_env() {
        Ok(c) => c.with_system_prompt(SYSTEM_PROMPT),
        Err(e) => {
            let _ = event_tx.send(AssistantEvent::Error(format!(
                "Claude API not configured: {e}"
            )));
            return;
        }
    };

    let _ = event_tx.send(AssistantEvent::Initialized {
        model_name: config.model.clone(),
    });

    let client = jarvis_ai::ClaudeClient::new(config);

    // Root the sandbox at the explicit workspace directory (never home).
    let root = workspace_root();
    tracing::info!(root = %root.display(), "Assistant tool sandbox rooted");
    let executor = ReadOnlyToolExecutor::new(root);

    // Read-only tool executor: maps tool name -> outcome, fully jailed.
    let tool_exec = Box::new(move |name: &str, args: &serde_json::Value| {
        match executor.execute(name, args) {
            Ok(out) => ToolOutcome::ok(out),
            Err(err) => ToolOutcome::error(err),
        }
    });

    // Surface tool activity to the app layer via the event channel.
    let evt_tx = event_tx.clone();
    let tool_events = Box::new(move |event: ToolEvent| match event {
        ToolEvent::Call { name, input } => {
            let _ = evt_tx.send(AssistantEvent::ToolCall { name, input });
        }
        ToolEvent::Result {
            name,
            summary,
            is_error,
        } => {
            let _ = evt_tx.send(AssistantEvent::ToolResult {
                name,
                summary,
                is_error,
            });
        }
    });

    let mut session = jarvis_ai::Session::new("claude")
        .with_system_prompt(SYSTEM_PROMPT)
        // Expose ONLY the read-only subset — run_command/write_file are excluded
        // entirely, so the model cannot even request them.
        .with_tools(jarvis_ai::read_only_tools())
        .with_tool_executor(tool_exec)
        .with_tool_event_callback(tool_events);

    while let Ok(msg) = tokio::task::block_in_place(|| user_rx.recv()) {
        let tx = event_tx.clone();
        let on_chunk = Box::new(move |chunk: String| {
            let _ = tx.send(AssistantEvent::StreamChunk(chunk));
        });

        match session.chat_streaming(&client, &msg, on_chunk).await {
            Ok(_) => {
                let _ = event_tx.send(AssistantEvent::Done);
            }
            Err(e) => {
                let _ = event_tx.send(AssistantEvent::Error(e.to_string()));
            }
        }
    }
}
