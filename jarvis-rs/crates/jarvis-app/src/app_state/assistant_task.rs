//! Background async task that manages the AI assistant session.
//!
//! Supports multiple providers (Claude, OpenAI, MiniMax, Gemini) selected from
//! config and switchable at runtime via the provider channel. The Session tool
//! loop and the read-only tool set are provider-agnostic and stay identical
//! across providers.

use jarvis_ai::{
    AiClient, ApprovalDecision, ApprovalReceiver, ApprovalRequest, ReadOnlyToolExecutor, ToolEvent,
    ToolOutcome, WriteExecToolExecutor,
};
use jarvis_config::schema::{AiProvider, AssistantConfig};

use super::types::AssistantEvent;

/// System prompt for the read-only agentic assistant (A1 default posture).
const SYSTEM_PROMPT: &str = "You are Jarvis, an AI assistant embedded in a terminal emulator. \
     You have READ-ONLY access to the project workspace via tools: read_file, \
     search_files, search_content, and list_directory. Use them to ground your \
     answers in the actual files. You CANNOT run commands or modify files. \
     Be concise and helpful. Use plain text, not markdown.";

/// System prompt when write/exec tools are enabled (read_write mode).
///
/// Tells the model the mutating tools exist AND that each one requires explicit
/// human approval before it runs, so it does not assume a write/command silently
/// succeeded.
const SYSTEM_PROMPT_READ_WRITE: &str =
    "You are Jarvis, an AI assistant embedded in a terminal emulator. \
     You have read access to the project workspace via read_file, search_files, \
     search_content, and list_directory. You ALSO have write_file and run_command, \
     which modify files / run programs in the workspace sandbox. Every write_file \
     and run_command call requires EXPLICIT human approval before it executes; if \
     the user denies it, the tool does not run and you must continue without it. \
     Prefer read-only tools to understand the project before proposing changes. \
     Be concise and helpful. Use plain text, not markdown.";

/// Resolve the workspace directory the tool sandbox is rooted at.
///
/// Uses the current working directory (the project dir) and NEVER the user's
/// home directory. Returns a canonicalized absolute path.
fn workspace_root() -> std::path::PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    std::fs::canonicalize(&cwd).unwrap_or(cwd)
}

/// A built AI client plus its human-readable model name (for the UI header).
struct BuiltClient {
    client: Box<dyn AiClient>,
    model_name: String,
}

/// Build a `Box<dyn AiClient>` for the selected provider.
///
/// API keys are read from environment variables ONLY (never from config).
/// Returns a clear error string if the provider's key env var is missing so
/// the caller can surface it as an assistant error instead of panicking.
///
/// Pick the system prompt for the session's tool posture. Read-write mode gets
/// a prompt that tells the model the mutating tools exist and are approval-gated;
/// otherwise the A1 read-only prompt is used.
fn system_prompt_for(config: &AssistantConfig) -> &'static str {
    if config.allow_write_exec() {
        SYSTEM_PROMPT_READ_WRITE
    } else {
        SYSTEM_PROMPT
    }
}

/// All providers receive the SAME tool set via the Session; this factory only
/// constructs the transport client. The system prompt reflects the configured
/// tool posture (read-only vs read-write).
fn build_client(
    provider: AiProvider,
    config: &AssistantConfig,
) -> Result<BuiltClient, String> {
    let system_prompt = system_prompt_for(config);
    match provider {
        AiProvider::Claude => {
            let mut cfg = jarvis_ai::ClaudeConfig::from_env()
                .map_err(|e| format!("Claude API not configured: {e}"))?
                .with_system_prompt(system_prompt);
            if !config.claude.model.is_empty() {
                cfg = cfg.with_model(config.claude.model.clone());
            }
            let model_name = cfg.model.clone();
            Ok(BuiltClient {
                client: Box::new(jarvis_ai::ClaudeClient::new(cfg)),
                model_name,
            })
        }
        AiProvider::OpenAi => {
            let mut cfg = jarvis_ai::OpenAiConfig::from_openai_env()
                .map_err(|e| format!("OpenAI API not configured: {e}"))?
                .with_system_prompt(system_prompt);
            if !config.openai.model.is_empty() {
                cfg = cfg.with_model(config.openai.model.clone());
            }
            if !config.openai.base_url.is_empty() {
                cfg = cfg.with_base_url(config.openai.base_url.clone());
            }
            let model_name = cfg.model.clone();
            Ok(BuiltClient {
                client: Box::new(jarvis_ai::OpenAiClient::new(cfg)),
                model_name,
            })
        }
        AiProvider::MiniMax => {
            let mut cfg = jarvis_ai::OpenAiConfig::from_minimax_env()
                .map_err(|e| format!("MiniMax API not configured: {e}"))?
                .with_system_prompt(system_prompt);
            if !config.minimax.model.is_empty() {
                cfg = cfg.with_model(config.minimax.model.clone());
            }
            if !config.minimax.base_url.is_empty() {
                cfg = cfg.with_base_url(config.minimax.base_url.clone());
            }
            let model_name = cfg.model.clone();
            Ok(BuiltClient {
                client: Box::new(jarvis_ai::OpenAiClient::new(cfg)),
                model_name,
            })
        }
        AiProvider::Gemini => {
            let mut cfg = jarvis_ai::GeminiConfig::from_env()
                .map_err(|e| format!("Gemini API not configured: {e}"))?
                .with_system_prompt(system_prompt);
            if !config.gemini.model.is_empty() {
                cfg = cfg.with_model(config.gemini.model.clone());
            }
            if !config.gemini.base_url.is_empty() {
                cfg = cfg.with_base_url(config.gemini.base_url.clone());
            }
            let model_name = cfg.model.clone();
            Ok(BuiltClient {
                client: Box::new(jarvis_ai::GeminiClient::new(cfg)),
                model_name,
            })
        }
    }
}

/// Provider label string sent to the UI (matches the serde lowercase form).
fn provider_label(provider: AiProvider) -> &'static str {
    match provider {
        AiProvider::Claude => "claude",
        AiProvider::OpenAi => "openai",
        AiProvider::MiniMax => "minimax",
        AiProvider::Gemini => "gemini",
    }
}

/// Background task that manages the AI assistant session.
///
/// `config` provides the initial provider selection and per-provider overrides.
/// `provider_rx` delivers runtime provider switches from the UI switcher; on
/// each switch the client is rebuilt for subsequent turns (the conversation
/// history in the Session is preserved).
pub(super) async fn assistant_task(
    user_rx: std::sync::mpsc::Receiver<String>,
    event_tx: std::sync::mpsc::Sender<AssistantEvent>,
    provider_rx: std::sync::mpsc::Receiver<AiProvider>,
    config: AssistantConfig,
) {
    let mut current_provider = config.provider;

    // Build the initial client. On failure, surface a clear error and keep the
    // task alive so a later provider switch (to a configured provider) works.
    let mut client: Option<Box<dyn AiClient>> = match build_client(current_provider, &config) {
        Ok(built) => {
            let _ = event_tx.send(AssistantEvent::Initialized {
                model_name: built.model_name,
            });
            let _ = event_tx.send(AssistantEvent::ProviderChanged {
                provider: provider_label(current_provider).to_string(),
            });
            Some(built.client)
        }
        Err(e) => {
            let _ = event_tx.send(AssistantEvent::Error(e));
            None
        }
    };

    // Root the sandbox at the explicit workspace directory (never home).
    let root = workspace_root();
    let write_exec = config.allow_write_exec();
    tracing::info!(
        root = %root.display(),
        write_exec,
        "Assistant tool sandbox rooted"
    );

    // Build the tool executor + the tool DEFINITIONS offered to the model in
    // lock-step. The two must agree: we never advertise a tool the executor
    // cannot run, and (critically) we never advertise write_file/run_command
    // unless write/exec is explicitly enabled in config. In the default
    // read-only posture this is byte-for-byte the A1 behavior: only the
    // read-only set is exposed, so the model cannot even REQUEST a mutating tool,
    // and the approval gate (installed below) is never invoked.
    let (tool_exec, tools): (
        Box<dyn Fn(&str, &serde_json::Value) -> ToolOutcome + Send + Sync>,
        Vec<jarvis_ai::ToolDefinition>,
    ) = if write_exec {
        // Read-write posture: full tool set + the jailed write/exec executor.
        // Every write_file/run_command still blocks on the approval gate in the
        // Session loop before this executor is ever reached.
        let executor = WriteExecToolExecutor::new(root);
        let exec = Box::new(move |name: &str, args: &serde_json::Value| {
            match executor.execute(name, args) {
                Ok(out) => ToolOutcome::ok(out),
                Err(err) => ToolOutcome::error(err),
            }
        }) as Box<dyn Fn(&str, &serde_json::Value) -> ToolOutcome + Send + Sync>;
        (exec, jarvis_ai::builtin_tools())
    } else {
        // Read-only posture (A1): only read-only tools, jailed, no mutation.
        let executor = ReadOnlyToolExecutor::new(root);
        let exec = Box::new(move |name: &str, args: &serde_json::Value| {
            match executor.execute(name, args) {
                Ok(out) => ToolOutcome::ok(out),
                Err(err) => ToolOutcome::error(err),
            }
        }) as Box<dyn Fn(&str, &serde_json::Value) -> ToolOutcome + Send + Sync>;
        (exec, jarvis_ai::read_only_tools())
    };

    // Surface tool activity to the app layer via the event channel.
    let evt_tx = event_tx.clone();
    let tool_events = Box::new(move |event: ToolEvent| match event {
        ToolEvent::Call { name, input } => {
            let _ = evt_tx.send(AssistantEvent::ToolCall { name, input });
        }
        ToolEvent::Result {
            id,
            name,
            summary,
            is_error,
        } => {
            let _ = evt_tx.send(AssistantEvent::ToolResult {
                id,
                name,
                summary,
                is_error,
            });
        }
    });

    let mut session = jarvis_ai::Session::new(provider_label(current_provider))
        .with_system_prompt(system_prompt_for(&config))
        // Tool set chosen above in lock-step with the executor: read-only by
        // default (A1), full set only when write/exec is enabled in config.
        .with_tools(tools)
        .with_tool_executor(tool_exec)
        .with_tool_event_callback(tool_events);

    // Install the human-approval gate ONLY when write/exec is enabled. In the
    // read-only default no approval-required tool is exposed, so there is nothing
    // to gate (matches A1: no gate, no prompt path).
    //
    // When write/exec IS enabled:
    //   * require_approval = true (the default, recommended): install the real
    //     gate. For each request it creates a oneshot, ships the request + sender
    //     to the MAIN thread (which stashes the sender keyed by id and shows the
    //     panel prompt), and returns the receiver for the Session to await under
    //     its 120s timeout. A dropped sender or a timeout FAILS CLOSED (deny) and
    //     nothing executes.
    //   * require_approval = false (explicit opt-out): install an auto-approve
    //     gate so the user's choice is honored. This is the ONLY path that runs a
    //     mutating tool without a prompt, and it requires BOTH read_write mode AND
    //     require_approval = false set deliberately in config. Without a gate the
    //     Session would fail closed (a missing gate denies), so an explicit
    //     auto-approve gate is required to realize "no approval" — there is no
    //     silent default that skips it.
    if write_exec {
        if config.require_approval {
            let gate_tx = event_tx.clone();
            let approval_gate = Box::new(move |request: ApprovalRequest| -> ApprovalReceiver {
                let (responder, receiver) = tokio::sync::oneshot::channel::<ApprovalDecision>();
                if gate_tx
                    .send(AssistantEvent::ToolApprovalRequest { request, responder })
                    .is_err()
                {
                    // Main thread is gone: the returned receiver resolves to a
                    // dropped-sender error, which the Session treats as deny
                    // (fail closed).
                }
                receiver
            });
            session = session.with_approval_gate(approval_gate);
        } else {
            tracing::warn!(
                "Assistant write/exec enabled with require_approval = false: \
                 mutating tools will run WITHOUT a human prompt"
            );
            let auto_approve = Box::new(move |_request: ApprovalRequest| -> ApprovalReceiver {
                let (responder, receiver) = tokio::sync::oneshot::channel::<ApprovalDecision>();
                // Resolve immediately to Approve; on send failure the dropped
                // sender fails closed (deny), which is the safe direction.
                let _ = responder.send(ApprovalDecision::Approve);
                receiver
            });
            session = session.with_approval_gate(auto_approve);
        }
    }

    while let Ok(msg) = tokio::task::block_in_place(|| user_rx.recv()) {
        // Apply any pending provider switch(es) before handling this message.
        while let Ok(new_provider) = provider_rx.try_recv() {
            if new_provider == current_provider {
                continue;
            }
            match build_client(new_provider, &config) {
                Ok(built) => {
                    current_provider = new_provider;
                    client = Some(built.client);
                    let _ = event_tx.send(AssistantEvent::Initialized {
                        model_name: built.model_name,
                    });
                    let _ = event_tx.send(AssistantEvent::ProviderChanged {
                        provider: provider_label(current_provider).to_string(),
                    });
                }
                Err(e) => {
                    // Keep the previous client; just report the failure.
                    let _ = event_tx.send(AssistantEvent::Error(e));
                }
            }
        }

        let active = match client.as_deref() {
            Some(c) => c,
            None => {
                let _ = event_tx.send(AssistantEvent::Error(
                    "No AI provider configured. Set the provider's API key environment \
                     variable and try again."
                        .to_string(),
                ));
                continue;
            }
        };

        let tx = event_tx.clone();
        let on_chunk = Box::new(move |chunk: String| {
            let _ = tx.send(AssistantEvent::StreamChunk(chunk));
        });

        match session.chat_streaming(active, &msg, on_chunk).await {
            Ok(_) => {
                let _ = event_tx.send(AssistantEvent::Done);
            }
            Err(e) => {
                let _ = event_tx.send(AssistantEvent::Error(e.to_string()));
            }
        }
    }
}
