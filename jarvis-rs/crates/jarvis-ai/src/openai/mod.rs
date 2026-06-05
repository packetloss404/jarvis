//! OpenAI-compatible Chat Completions client.
//!
//! Implements the `AiClient` trait for the OpenAI Chat Completions API shape
//! (`POST <base_url>/chat/completions`, `Authorization: Bearer`). A single
//! parameterized client serves BOTH OpenAI's own API and any OpenAI-compatible
//! endpoint — currently MiniMax — differing only in `base_url` and default
//! `model`. (Gemini uses a different wire format and is handled separately.)
//!
//! API keys are read from environment variables only (`OPENAI_API_KEY`,
//! `MINIMAX_API_KEY`) and are never written to config or logs.

mod api;
mod client;
mod config;

pub use client::OpenAiClient;
pub use config::{
    OpenAiConfig, DEFAULT_MINIMAX_BASE_URL, DEFAULT_MINIMAX_MODEL, DEFAULT_OPENAI_BASE_URL,
    DEFAULT_OPENAI_MODEL,
};
