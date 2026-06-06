//! Voice input configuration types.
//!
//! Voice input is push-to-talk speech-to-text: hold the `[keybinds].push_to_talk`
//! key (default `F4`) to record from the microphone; on release the audio is
//! transcribed by OpenAI Whisper and the text is placed in the assistant input
//! box for review. Every field here is actually consumed at runtime — see
//! `jarvis-ai/src/voice` (capture) and `jarvis-app/src/app_state/voice.rs`
//! (gating + transcription).

use serde::{Deserialize, Serialize};

/// Voice input configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    /// Master toggle. OFF by default — voice input captures the microphone, so
    /// it is explicit opt-in. Enabling it ALSO requires `OPENAI_API_KEY` (the
    /// Whisper key); with both set, hold the push-to-talk key to record.
    pub enabled: bool,
    /// Input device name to capture from, or `"default"` for the system default
    /// input device. A name that doesn't match any available device falls back
    /// to the default (with a warning).
    pub input_device: String,
    /// Spoken language hint passed to Whisper (ISO-639-1, e.g. `"en"`). `None`
    /// lets Whisper auto-detect.
    pub language: Option<String>,
    /// Whisper transcription model.
    pub model: String,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            input_device: "default".into(),
            language: None,
            model: "whisper-1".into(),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_config_defaults() {
        let c = VoiceConfig::default();
        assert!(!c.enabled); // opt-in (mic capture)
        assert_eq!(c.input_device, "default");
        assert_eq!(c.language, None);
        assert_eq!(c.model, "whisper-1");
    }

    #[test]
    fn voice_config_partial_toml() {
        let c: VoiceConfig = toml::from_str(
            r#"
enabled = true
language = "en"
"#,
        )
        .unwrap();
        assert!(c.enabled);
        assert_eq!(c.language.as_deref(), Some("en"));
        assert_eq!(c.input_device, "default"); // default preserved
        assert_eq!(c.model, "whisper-1");
    }
}
