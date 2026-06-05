//! AI assistant / provider configuration types.
//!
//! Selects which AI provider the assistant uses and holds per-provider model
//! and base-URL overrides. API KEYS ARE NEVER STORED HERE — they come from
//! environment variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`,
//! `MINIMAX_API_KEY`, `GEMINI_API_KEY`/`GOOGLE_API_KEY`, etc.) and are never
//! written to config or logs.

use serde::{Deserialize, Serialize};

/// Which AI provider the assistant should use.
///
/// `openai` and `minimax` share the OpenAI Chat Completions wire format;
/// `gemini` uses Google's Generative Language API.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum AiProvider {
    #[default]
    Claude,
    OpenAi,
    MiniMax,
    Gemini,
}

/// Per-provider model / base-URL overrides for the OpenAI provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenAiProviderConfig {
    /// Override the model (empty = client default, e.g. "gpt-4o").
    pub model: String,
    /// Override the API base URL (empty = client default).
    pub base_url: String,
}

impl Default for OpenAiProviderConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            base_url: String::new(),
        }
    }
}

/// Per-provider model / base-URL overrides for the MiniMax provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MiniMaxProviderConfig {
    /// Override the model (empty = client default, e.g. "MiniMax-M2").
    pub model: String,
    /// Override the OpenAI-compatible base URL (empty = client default).
    pub base_url: String,
}

impl Default for MiniMaxProviderConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            base_url: String::new(),
        }
    }
}

/// Per-provider model / base-URL overrides for the Gemini provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeminiProviderConfig {
    /// Override the model (empty = client default, e.g. "gemini-2.0-flash").
    pub model: String,
    /// Override the API base URL (empty = client default).
    pub base_url: String,
}

impl Default for GeminiProviderConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            base_url: String::new(),
        }
    }
}

/// Per-provider model override for the Claude provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeProviderConfig {
    /// Override the model (empty = client default).
    pub model: String,
}

impl Default for ClaudeProviderConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
        }
    }
}

/// Tool-permission mode for the assistant.
///
/// `ReadOnly` (the DEFAULT) exposes only the read-only filesystem tools
/// (`read_file`, `search_files`, `search_content`, `list_directory`) — exactly
/// the A1 behavior; the model cannot even request `write_file` / `run_command`.
///
/// `ReadWrite` additionally exposes the mutating/exec tools (`write_file`,
/// `run_command`). This is OPT-IN, and even when enabled every mutating/exec
/// call still blocks on explicit human approval (see
/// [`AssistantConfig::require_approval`]) — there is no silent auto-approve in
/// the default posture.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum AssistantToolsMode {
    /// Read-only filesystem tools only (default; A1 behavior).
    #[default]
    ReadOnly,
    /// Read-only tools plus write_file + run_command (approval-gated).
    ReadWrite,
}

impl AssistantToolsMode {
    /// Whether write/exec tools are enabled in this mode.
    pub fn allows_write_exec(self) -> bool {
        matches!(self, AssistantToolsMode::ReadWrite)
    }
}

/// Default for [`AssistantConfig::require_approval`]: approval is ALWAYS
/// required unless the user explicitly turns it off in config. This keeps the
/// safe posture (no silent auto-approve) the default.
fn default_require_approval() -> bool {
    true
}

/// AI assistant configuration: provider selector plus per-provider overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AssistantConfig {
    /// Which provider the assistant uses by default.
    pub provider: AiProvider,
    pub claude: ClaudeProviderConfig,
    pub openai: OpenAiProviderConfig,
    pub minimax: MiniMaxProviderConfig,
    pub gemini: GeminiProviderConfig,
    /// Tool-permission mode. DEFAULT is [`AssistantToolsMode::ReadOnly`] — the
    /// A1 read-only behavior. Set to `read_write` to opt into approval-gated
    /// write_file + run_command.
    pub tools_mode: AssistantToolsMode,
    /// Whether mutating/exec tool calls require explicit human approval.
    /// DEFAULT is `true`. This should stay `true`; it exists as an escape hatch,
    /// not a convenience. When `tools_mode` is `read_only` it is moot (no
    /// mutating tools are exposed at all).
    #[serde(default = "default_require_approval")]
    pub require_approval: bool,
}

impl AssistantConfig {
    /// Whether write/exec tools should be exposed to the model. True only when
    /// the mode opts in. Approval is enforced separately.
    pub fn allow_write_exec(&self) -> bool {
        self.tools_mode.allows_write_exec()
    }
}

impl Default for AssistantConfig {
    fn default() -> Self {
        Self {
            provider: AiProvider::default(),
            claude: ClaudeProviderConfig::default(),
            openai: OpenAiProviderConfig::default(),
            minimax: MiniMaxProviderConfig::default(),
            gemini: GeminiProviderConfig::default(),
            tools_mode: AssistantToolsMode::default(),
            require_approval: default_require_approval(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_provider_is_claude() {
        let c = AssistantConfig::default();
        assert_eq!(c.provider, AiProvider::Claude);
        assert!(c.openai.model.is_empty());
        assert!(c.openai.base_url.is_empty());
        assert!(c.minimax.model.is_empty());
    }

    #[test]
    fn default_tools_mode_is_read_only() {
        let c = AssistantConfig::default();
        // Safety: the default posture must stay read-only with approval required.
        assert_eq!(c.tools_mode, AssistantToolsMode::ReadOnly);
        assert!(!c.allow_write_exec(), "default must NOT allow write/exec");
        assert!(c.require_approval, "approval must default to required");
    }

    #[test]
    fn tools_mode_serializes_snake_case() {
        let c = AssistantConfig {
            tools_mode: AssistantToolsMode::ReadWrite,
            ..Default::default()
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"read_write\""), "got: {json}");
        assert!(c.allow_write_exec());
    }

    #[test]
    fn tools_mode_and_approval_from_toml() {
        let toml_str = r#"
[assistant]
provider = "claude"
tools_mode = "read_write"
require_approval = true
"#;
        #[derive(Deserialize)]
        struct Wrapper {
            assistant: AssistantConfig,
        }
        let w: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(w.assistant.tools_mode, AssistantToolsMode::ReadWrite);
        assert!(w.assistant.allow_write_exec());
        assert!(w.assistant.require_approval);
    }

    #[test]
    fn omitting_tools_fields_keeps_safe_defaults() {
        // A config that sets only the provider must still be read-only +
        // approval-required (fields default safely when absent).
        let toml_str = r#"
[assistant]
provider = "openai"
"#;
        #[derive(Deserialize)]
        struct Wrapper {
            assistant: AssistantConfig,
        }
        let w: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(w.assistant.tools_mode, AssistantToolsMode::ReadOnly);
        assert!(!w.assistant.allow_write_exec());
        assert!(w.assistant.require_approval);
    }

    #[test]
    fn provider_serializes_lowercase() {
        let c = AssistantConfig {
            provider: AiProvider::OpenAi,
            ..Default::default()
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"openai\""));

        let c = AssistantConfig {
            provider: AiProvider::MiniMax,
            ..Default::default()
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"minimax\""));

        let c = AssistantConfig {
            provider: AiProvider::Gemini,
            ..Default::default()
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"gemini\""));
    }

    #[test]
    fn assistant_config_from_toml() {
        let toml_str = r#"
[assistant]
provider = "minimax"

[assistant.openai]
model = "gpt-4o-mini"

[assistant.minimax]
model = "MiniMax-Text-01"
base_url = "https://api.minimax.io/v1"

[assistant.gemini]
model = "gemini-2.0-flash"
"#;
        #[derive(Deserialize)]
        struct Wrapper {
            assistant: AssistantConfig,
        }
        let w: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(w.assistant.provider, AiProvider::MiniMax);
        assert_eq!(w.assistant.openai.model, "gpt-4o-mini");
        assert_eq!(w.assistant.minimax.model, "MiniMax-Text-01");
        assert_eq!(w.assistant.minimax.base_url, "https://api.minimax.io/v1");
        assert_eq!(w.assistant.gemini.model, "gemini-2.0-flash");
        // Unset overrides stay empty (client defaults applied at build time).
        assert!(w.assistant.openai.base_url.is_empty());
        assert!(w.assistant.claude.model.is_empty());
        assert!(w.assistant.gemini.base_url.is_empty());
    }

    #[test]
    fn gemini_provider_from_toml() {
        let toml_str = r#"
[assistant]
provider = "gemini"

[assistant.gemini]
model = "gemini-2.5-flash"
base_url = "https://generativelanguage.googleapis.com/v1beta"
"#;
        #[derive(Deserialize)]
        struct Wrapper {
            assistant: AssistantConfig,
        }
        let w: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(w.assistant.provider, AiProvider::Gemini);
        assert_eq!(w.assistant.gemini.model, "gemini-2.5-flash");
        assert_eq!(
            w.assistant.gemini.base_url,
            "https://generativelanguage.googleapis.com/v1beta"
        );
    }
}
