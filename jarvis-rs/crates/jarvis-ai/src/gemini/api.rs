//! AiClient trait implementation for GeminiClient (send_message + streaming).

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::streaming::{parse_sse_stream, SseEvent};
use crate::{AiClient, AiError, AiResponse, Message, TokenUsage, ToolCall, ToolDefinition};

use super::client::{parse_function_call, GeminiClient};

/// Accumulator for streamed Gemini response parts.
///
/// `streamGenerateContent?alt=sse` emits a sequence of `GenerateContentResponse`
/// chunks, each carrying a slice of `candidates[0].content.parts`. Text parts
/// are concatenated; `functionCall` parts arrive whole (not fragmented like
/// OpenAI), so each is turned into a `ToolCall` with a synthesized stable id
/// (`call_<n>`) using a running index across the whole stream. `usageMetadata`
/// (when present, typically on the final chunk) updates the token counts.
#[derive(Default)]
struct StreamAccumulator {
    content: String,
    tool_calls: Vec<ToolCall>,
    usage: TokenUsage,
}

impl StreamAccumulator {
    /// Ingest one parsed SSE chunk, pushing any newly emitted text via `on_chunk`.
    ///
    /// Returns an error if the chunk contains an API error payload (so the
    /// caller can propagate it instead of silently losing it).
    fn ingest(
        &mut self,
        data: &serde_json::Value,
        on_chunk: &(dyn Fn(String) + Send + Sync),
    ) -> Result<(), crate::AiError> {
        // Check if this is an error response before accessing text content.
        if let Some(error) = data.get("error") {
            return Err(crate::AiError::ApiError(error.to_string()));
        }

        if let Some(candidates) = data["candidates"].as_array() {
            for candidate in candidates {
                if let Some(parts) = candidate["content"]["parts"].as_array() {
                    for part in parts {
                        if let Some(text) = part["text"].as_str() {
                            if !text.is_empty() {
                                self.content.push_str(text);
                                on_chunk(text.to_string());
                            }
                        }
                        if let Some(fc) = part.get("functionCall") {
                            let idx = self.tool_calls.len();
                            self.tool_calls.push(parse_function_call(fc, idx));
                        }
                    }
                }
            }
        }

        if let Some(meta) = data.get("usageMetadata") {
            self.usage.input_tokens = meta["promptTokenCount"]
                .as_u64()
                .unwrap_or(self.usage.input_tokens);
            self.usage.output_tokens = meta["candidatesTokenCount"]
                .as_u64()
                .unwrap_or(self.usage.output_tokens);
        }

        Ok(())
    }
}

#[async_trait]
impl AiClient for GeminiClient {
    async fn send_message(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<AiResponse, AiError> {
        let body = self.build_request_body(messages, tools);
        let url = self.api_url(false);

        debug!(model = %self.config.model, "Gemini API request");

        let response = self
            .http
            .post(&url)
            .headers(self.auth_headers()?)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AiError::NetworkError(e.to_string()))?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AiError::RateLimited);
        }
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            let text = text.chars().take(200).collect::<String>();
            return Err(AiError::ApiError(format!("HTTP {status}: {text}")));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AiError::ParseError(e.to_string()))?;

        self.parse_response(json)
    }

    async fn send_message_streaming(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        on_chunk: Box<dyn Fn(String) + Send + Sync>,
    ) -> Result<AiResponse, AiError> {
        let body = self.build_request_body(messages, tools);
        // SSE framing: one JSON object per `data:` event.
        let url = format!("{}?alt=sse", self.api_url(true));

        debug!(model = %self.config.model, "Gemini API streaming request");

        let response = self
            .http
            .post(&url)
            .headers(self.auth_headers()?)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AiError::NetworkError(e.to_string()))?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AiError::RateLimited);
        }
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            let text = text.chars().take(200).collect::<String>();
            return Err(AiError::ApiError(format!("HTTP {status}: {text}")));
        }

        let mut acc = StreamAccumulator::default();
        let mut stream_error: Option<AiError> = None;

        parse_sse_stream(response, |event: SseEvent| {
            // Stop processing further chunks if a previous chunk returned an error.
            if stream_error.is_some() {
                return;
            }
            let data = event.data.trim();
            if data.is_empty() {
                return;
            }
            let json: serde_json::Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => return,
            };
            if let Err(e) = acc.ingest(&json, on_chunk.as_ref()) {
                stream_error = Some(e);
            }
        })
        .await?;

        if let Some(e) = stream_error {
            return Err(e);
        }

        if acc.usage.input_tokens == 0 && acc.usage.output_tokens == 0 {
            warn!("No usage data received in Gemini streaming response");
        }

        Ok(AiResponse {
            content: acc.content,
            tool_calls: acc.tool_calls,
            usage: acc.usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A no-op `on_chunk` for tests that only check final accumulation.
    fn noop() -> Box<dyn Fn(String) + Send + Sync> {
        Box::new(|_| {})
    }

    #[test]
    fn accumulates_streamed_text_chunks() {
        let mut acc = StreamAccumulator::default();
        let cb = noop();
        acc.ingest(
            &serde_json::json!({
                "candidates": [{ "content": { "parts": [{ "text": "Hel" }] } }]
            }),
            cb.as_ref(),
        ).unwrap();
        acc.ingest(
            &serde_json::json!({
                "candidates": [{ "content": { "parts": [{ "text": "lo" }] } }]
            }),
            cb.as_ref(),
        ).unwrap();
        assert_eq!(acc.content, "Hello");
        assert!(acc.tool_calls.is_empty());
    }

    #[test]
    fn on_chunk_receives_each_text_part() {
        let collected = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let c2 = collected.clone();
        let cb: Box<dyn Fn(String) + Send + Sync> =
            Box::new(move |s| c2.lock().unwrap().push(s));
        let mut acc = StreamAccumulator::default();
        acc.ingest(
            &serde_json::json!({
                "candidates": [{ "content": { "parts": [{ "text": "a" }, { "text": "b" }] } }]
            }),
            cb.as_ref(),
        ).unwrap();
        assert_eq!(*collected.lock().unwrap(), vec!["a", "b"]);
    }

    #[test]
    fn accumulates_streamed_function_calls_with_synthesized_ids() {
        let mut acc = StreamAccumulator::default();
        let cb = noop();
        acc.ingest(
            &serde_json::json!({
                "candidates": [{ "content": { "parts": [
                    { "functionCall": { "name": "read_file", "args": { "path": "a.txt" } } }
                ] } }]
            }),
            cb.as_ref(),
        ).unwrap();
        acc.ingest(
            &serde_json::json!({
                "candidates": [{ "content": { "parts": [
                    { "functionCall": { "name": "list_directory", "args": { "path": "." } } }
                ] } }]
            }),
            cb.as_ref(),
        ).unwrap();
        assert_eq!(acc.tool_calls.len(), 2);
        assert_eq!(acc.tool_calls[0].id, "call_0");
        assert_eq!(acc.tool_calls[0].name, "read_file");
        assert_eq!(acc.tool_calls[0].arguments["path"], "a.txt");
        assert_eq!(acc.tool_calls[1].id, "call_1");
        assert_eq!(acc.tool_calls[1].name, "list_directory");
    }

    #[test]
    fn accumulates_usage_metadata() {
        let mut acc = StreamAccumulator::default();
        let cb = noop();
        acc.ingest(
            &serde_json::json!({
                "candidates": [{ "content": { "parts": [{ "text": "hi" }] } }],
                "usageMetadata": { "promptTokenCount": 9, "candidatesTokenCount": 3 }
            }),
            cb.as_ref(),
        ).unwrap();
        assert_eq!(acc.usage.input_tokens, 9);
        assert_eq!(acc.usage.output_tokens, 3);
    }

    #[test]
    fn mixed_text_and_function_call_in_one_chunk() {
        let mut acc = StreamAccumulator::default();
        let cb = noop();
        acc.ingest(
            &serde_json::json!({
                "candidates": [{ "content": { "parts": [
                    { "text": "let me look" },
                    { "functionCall": { "name": "read_file", "args": {} } }
                ] } }]
            }),
            cb.as_ref(),
        ).unwrap();
        assert_eq!(acc.content, "let me look");
        assert_eq!(acc.tool_calls.len(), 1);
        assert_eq!(acc.tool_calls[0].name, "read_file");
    }

    #[test]
    fn ingest_returns_error_on_api_error_chunk() {
        let mut acc = StreamAccumulator::default();
        let cb = noop();
        let result = acc.ingest(
            &serde_json::json!({ "error": { "code": 400, "message": "bad request" } }),
            cb.as_ref(),
        );
        assert!(result.is_err());
    }
}
