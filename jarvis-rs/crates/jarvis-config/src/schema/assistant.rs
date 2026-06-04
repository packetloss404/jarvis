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
}

impl Default for AssistantConfig {
    fn default() -> Self {
        Self {
            provider: AiProvider::default(),
            claude: ClaudeProviderConfig::default(),
            openai: OpenAiProviderConfig::default(),
            minimax: MiniMaxProviderConfig::default(),
            gemini: GeminiProviderConfig::default(),
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
