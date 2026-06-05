//! OpenAI-compatible API client configuration.
//!
//! A single `OpenAiConfig` parameterizes the client for BOTH OpenAI's own
//! Chat Completions API and any OpenAI-compatible endpoint (currently MiniMax).
//! Only the `base_url` and default `model` differ between providers; the wire
//! format (`/chat/completions`, `Authorization: Bearer`) is identical.

use std::fmt;

use crate::AiError;

/// Default OpenAI chat model.
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-4o";
/// Default OpenAI API base URL (the `/chat/completions` path is appended).
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

/// Default MiniMax chat model.
pub const DEFAULT_MINIMAX_MODEL: &str = "MiniMax-M2";
/// Default MiniMax OpenAI-compatible API base URL.
pub const DEFAULT_MINIMAX_BASE_URL: &str = "https://api.minimax.io/v1";

/// Configuration for an OpenAI-compatible Chat Completions client.
///
/// The `base_url` is the API root (e.g. `https://api.openai.com/v1`); the
/// client appends `/chat/completions`. The API key always comes from an
/// environment variable — it is NEVER read from or written to the config file.
#[derive(Clone)]
pub struct OpenAiConfig {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    pub max_tokens: u32,
    pub temperature: f64,
    pub system_prompt: Option<String>,
}

impl fmt::Debug for OpenAiConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiConfig")
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("max_tokens", &self.max_tokens)
            .field("temperature", &self.temperature)
            .field("system_prompt", &self.system_prompt)
            .finish()
    }
}

impl OpenAiConfig {
    /// Construct a config with an explicit api key, model, and base URL.
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            base_url: base_url.into(),
            max_tokens: 4096,
            temperature: 0.7,
            system_prompt: None,
        }
    }

    /// Build an OpenAI config from the environment.
    ///
    /// Reads the API key from `OPENAI_API_KEY` and uses the default OpenAI base
    /// URL and model.
    pub fn from_openai_env() -> Result<Self, AiError> {
        let key = std::env::var("OPENAI_API_KEY").map_err(|_| {
            AiError::ApiError(
                "OpenAI API not configured. Set OPENAI_API_KEY.".into(),
            )
        })?;
        Ok(Self::new(
            key,
            DEFAULT_OPENAI_MODEL,
            DEFAULT_OPENAI_BASE_URL,
        ))
    }

    /// Build a MiniMax config from the environment.
    ///
    /// Reads the API key from `MINIMAX_API_KEY` and targets MiniMax's
    /// OpenAI-compatible endpoint. The base URL is a normal field, so callers
    /// can override it (e.g. for a regional endpoint).
    pub fn from_minimax_env() -> Result<Self, AiError> {
        let key = std::env::var("MINIMAX_API_KEY").map_err(|_| {
            AiError::ApiError(
                "MiniMax API not configured. Set MINIMAX_API_KEY.".into(),
            )
        })?;
        Ok(Self::new(
            key,
            DEFAULT_MINIMAX_MODEL,
            DEFAULT_MINIMAX_BASE_URL,
        ))
    }

    /// Full Chat Completions endpoint URL (`<base_url>/chat/completions`).
    pub(crate) fn chat_completions_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        format!("{base}/chat/completions")
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    pub fn with_temperature(mut self, temperature: f64) -> Self {
        self.temperature = temperature;
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_completions_url_appends_path() {
        let c = OpenAiConfig::new("k", "gpt-4o", "https://api.openai.com/v1");
        assert_eq!(
            c.chat_completions_url(),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn chat_completions_url_trims_trailing_slash() {
        let c = OpenAiConfig::new("k", "m", "https://api.minimax.io/v1/");
        assert_eq!(
            c.chat_completions_url(),
            "https://api.minimax.io/v1/chat/completions"
        );
    }

    #[test]
    fn debug_redacts_api_key() {
        let c = OpenAiConfig::new("super-secret", "gpt-4o", DEFAULT_OPENAI_BASE_URL);
        let dbg = format!("{c:?}");
        assert!(dbg.contains("[REDACTED]"));
        assert!(!dbg.contains("super-secret"));
    }
}
