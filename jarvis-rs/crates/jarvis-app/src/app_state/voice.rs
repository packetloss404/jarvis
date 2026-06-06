//! Push-to-talk voice input wiring.
//!
//! Hold the configured push-to-talk key (default `F4`, see
//! `[keybinds].push_to_talk`) to record from the default microphone; on release
//! the captured WAV is transcribed by OpenAI Whisper on the async runtime and
//! the resulting text is delivered to the assistant panel's input textarea via
//! the `voice_transcript` IPC message for the user to review and send manually.
//! It is NEVER auto-sent to the assistant.
//!
//! Gating: recording only starts when `[voice].enabled` is true AND
//! `OPENAI_API_KEY` is set in the environment (the Whisper key convention,
//! matching `jarvis-ai/src/openai/config.rs`). Otherwise the keybind is a
//! logged no-op.

use jarvis_ai::{VoiceRecorder, WhisperClient, WhisperConfig};

use super::core::JarvisApp;

impl JarvisApp {
    /// Whether voice input is usable right now: enabled in config AND an
    /// `OPENAI_API_KEY` is present for Whisper.
    fn voice_available(&self) -> Option<String> {
        if !self.config.voice.enabled {
            return None;
        }
        match std::env::var("OPENAI_API_KEY") {
            Ok(key) if !key.is_empty() => Some(key),
            _ => None,
        }
    }

    /// Handle the push-to-talk key DOWN: start capturing microphone audio.
    ///
    /// No-op (logged) when voice is disabled, no API key is set, or a recording
    /// is already in progress (key auto-repeat).
    pub(super) fn handle_push_to_talk(&mut self) {
        if self.voice_recorder.is_some() {
            // Key auto-repeat while already held — ignore.
            return;
        }
        if self.voice_available().is_none() {
            tracing::info!(
                enabled = self.config.voice.enabled,
                "push-to-talk ignored: voice disabled or OPENAI_API_KEY unset"
            );
            return;
        }

        match VoiceRecorder::start(&self.config.voice.input_device) {
            Ok(recorder) => {
                tracing::info!("push-to-talk: recording started");
                self.voice_recorder = Some(recorder);
            }
            Err(e) => {
                tracing::warn!(error = %e, "push-to-talk: failed to start recorder");
            }
        }
    }

    /// Handle the push-to-talk key UP: stop capturing, then transcribe the WAV
    /// on the async runtime and route the transcript to the assistant input.
    pub(super) fn handle_release_push_to_talk(&mut self) {
        let recorder = match self.voice_recorder.take() {
            Some(r) => r,
            None => return, // not recording (e.g. start was gated out)
        };

        // Re-check the key at release time (it may have been unset mid-hold).
        let api_key = match self.voice_available() {
            Some(k) => k,
            None => {
                tracing::info!("push-to-talk released but voice no longer available; discarding");
                return;
            }
        };

        let wav = recorder.stop();
        tracing::info!(bytes = wav.len(), "push-to-talk: recording stopped, transcribing");

        // Ensure the shared tokio runtime + transcript channel exist.
        self.ensure_voice_channel();
        if self.tokio_runtime.is_none() {
            self.ensure_assistant_runtime(); // builds self.tokio_runtime as a side effect
        }
        let rt = match self.tokio_runtime.as_ref() {
            Some(rt) => rt,
            None => {
                tracing::warn!("push-to-talk: no tokio runtime; cannot transcribe");
                return;
            }
        };
        let tx = match self.voice_transcript_tx.clone() {
            Some(tx) => tx,
            None => return,
        };

        let mut whisper_cfg = WhisperConfig::new(api_key);
        whisper_cfg.model = self.config.voice.model.clone();
        whisper_cfg.language = self.config.voice.language.clone();

        rt.spawn(async move {
            let client = WhisperClient::new(whisper_cfg);
            match client.transcribe(wav, "audio.wav").await {
                Ok(text) => {
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        tracing::info!("voice transcript empty; nothing to insert");
                    } else if let Err(e) = tx.send(trimmed.to_string()) {
                        tracing::warn!(error = %e, "failed to deliver voice transcript");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "voice transcription failed");
                }
            }
        });
    }

    /// Lazily create the voice-transcript channel (sender stored, receiver
    /// polled in `poll_voice_transcripts`).
    fn ensure_voice_channel(&mut self) {
        if self.voice_transcript_tx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        self.voice_transcript_tx = Some(tx);
        self.voice_transcript_rx = Some(rx);
    }

    /// Drain any ready voice transcripts and push them into the assistant
    /// panel's input textarea (review-before-send; never auto-submitted).
    pub(super) fn poll_voice_transcripts(&mut self) {
        // Collect first to avoid borrowing self across the send loop.
        let texts: Vec<String> = match self.voice_transcript_rx {
            Some(ref rx) => rx.try_iter().collect(),
            None => return,
        };
        for text in texts {
            tracing::info!(len = text.len(), "voice transcript -> assistant input");
            self.send_assistant_ipc("voice_transcript", &serde_json::json!({ "text": text }));
        }
    }
}
