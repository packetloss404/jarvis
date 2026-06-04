//! Background async task that manages the AI assistant session.
//!
//! Supports multiple providers (Claude, OpenAI, MiniMax, Gemini) selected from
//! config and switchable at runtime via the provider channel. The Session tool
//! loop and the read-only tool set are provider-agnostic and stay identical
//! across providers.

use jarvis_ai::{AiClient, ReadOnlyToolExecutor, ToolEvent, ToolOutcome};
use jarvis_config::schema::{AiProvider, AssistantConfig};

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
/// All providers receive the SAME read-only tool set via the Session; this
/// factory only constructs the transport client.
fn build_client(
    provider: AiProvider,
    config: &AssistantConfig,
) -> Result<BuiltClient, String> {
    match provider {
        AiProvider::Claude => {
            let mut cfg = jarvis_ai::ClaudeConfig::from_env()
                .map_err(|e| format!("Claude API not configured: {e}"))?
                .with_system_prompt(SYSTEM_PROMPT);
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
                .with_system_prompt(SYSTEM_PROMPT);
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
                .with_system_prompt(SYSTEM_PROMPT);
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
                .with_system_prompt(SYSTEM_PROMPT);
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

    let mut session = jarvis_ai::Session::new(provider_label(current_provider))
        .with_system_prompt(SYSTEM_PROMPT)
        // Expose ONLY the read-only subset — run_command/write_file are excluded
        // entirely, so the model cannot even request them. SAME set for every
        // provider.
        .with_tools(jarvis_ai::read_only_tools())
        .with_tool_executor(tool_exec)
        .with_tool_event_callback(tool_events);

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
