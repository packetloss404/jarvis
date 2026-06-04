//! Google Gemini (Generative Language API) client.
//!
//! Implements the `AiClient` trait for Gemini models via the
//! `generateContent` / `streamGenerateContent` endpoints. Unlike the OpenAI
//! provider, Gemini uses a distinct wire format: a `contents` array of
//! role-tagged `parts`, `functionCall` / `functionResponse` parts for tool use,
//! a top-level `systemInstruction` (no system role), and `?alt=sse` streaming.
//! Requests/responses are translated to and from the provider-agnostic
//! `Message`/`ContentBlock` / `AiResponse` model.
//!
//! API keys are read from environment variables only (`GEMINI_API_KEY` or
//! `GOOGLE_API_KEY`) and are never written to config or logs.

mod api;
mod client;
mod config;

pub use client::GeminiClient;
pub use config::{GeminiConfig, DEFAULT_GEMINI_BASE_URL, DEFAULT_GEMINI_MODEL};
