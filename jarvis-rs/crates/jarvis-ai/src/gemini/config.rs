//! Google Gemini API client configuration.
//!
//! Targets the Generative Language API (`generateContent` /
//! `streamGenerateContent`). The API key always comes from an environment
//! variable (`GEMINI_API_KEY` or `GOOGLE_API_KEY`) — it is NEVER read from or
//! written to the config file, and is redacted in `Debug`.

use std::fmt;

use crate::AiError;

/// Default Gemini chat model.
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-2.0-flash";
/// Default Gemini API base URL. The client appends
/// `/models/{model}:{method}` (e.g. `:generateContent`).
pub const DEFAULT_GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

/// Configuration for a Gemini (Generative Language API) client.
///
/// The `base_url` is the API root (e.g.
/// `https://generativelanguage.googleapis.com/v1beta`); the client appends
/// `/models/{model}:generateContent` (or `:streamGenerateContent?alt=sse`).
#[derive(Clone)]
pub struct GeminiConfig {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    pub max_tokens: u32,
    pub temperature: f64,
    pub system_prompt: Option<String>,
}

impl fmt::Debug for GeminiConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeminiConfig")
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("max_tokens", &self.max_tokens)
            .field("temperature", &self.temperature)
            .field("system_prompt", &self.system_prompt)
            .finish()
    }
}

impl GeminiConfig {
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

    /// Build a Gemini config from the environment.
    ///
    /// Reads the API key from `GEMINI_API_KEY`, falling back to `GOOGLE_API_KEY`,
    /// and uses the default Gemini base URL and model.
    pub fn from_env() -> Result<Self, AiError> {
        let key = std::env::var("GEMINI_API_KEY")
            .or_else(|_| std::env::var("GOOGLE_API_KEY"))
            .map_err(|_| {
                AiError::ApiError(
                    "Gemini API not configured. Set GEMINI_API_KEY (or GOOGLE_API_KEY)."
                        .into(),
                )
            })?;
        Ok(Self::new(
            key,
            DEFAULT_GEMINI_MODEL,
            DEFAULT_GEMINI_BASE_URL,
        ))
    }

    /// Build the full endpoint URL for a given method.
    ///
    /// Produces `<base_url>/models/<model>:<method>`. The base URL's trailing
    /// slash (if any) is trimmed so the path is well-formed.
    pub(crate) fn method_url(&self, method: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        format!("{base}/models/{}:{method}", self.model)
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
    fn method_url_builds_models_path() {
        let c = GeminiConfig::new("k", "gemini-2.0-flash", DEFAULT_GEMINI_BASE_URL);
        assert_eq!(
            c.method_url("generateContent"),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent"
        );
    }

    #[test]
    fn method_url_trims_trailing_slash() {
        let c = GeminiConfig::new("k", "m", "https://generativelanguage.googleapis.com/v1beta/");
        assert_eq!(
            c.method_url("streamGenerateContent"),
            "https://generativelanguage.googleapis.com/v1beta/models/m:streamGenerateContent"
        );
    }

    #[test]
    fn debug_redacts_api_key() {
        let c = GeminiConfig::new("super-secret", "gemini-2.0-flash", DEFAULT_GEMINI_BASE_URL);
        let dbg = format!("{c:?}");
        assert!(dbg.contains("[REDACTED]"));
        assert!(!dbg.contains("super-secret"));
    }
}
