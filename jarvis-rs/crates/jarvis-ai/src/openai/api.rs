//! AiClient trait implementation for OpenAiClient (send_message + streaming).

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::streaming::{parse_sse_stream, SseEvent};
use crate::{AiClient, AiError, AiResponse, Message, TokenUsage, ToolCall, ToolDefinition};

use super::client::OpenAiClient;

/// Accumulator for streamed `tool_calls` fragments.
///
/// In an OpenAI streaming response, each `choices[0].delta.tool_calls[]` entry
/// carries an `index`. The `id` and `function.name` arrive once (usually on the
/// first fragment for an index); `function.arguments` streams as a sequence of
/// string fragments that must be CONCATENATED. This accumulates by index and
/// assembles final `ToolCall`s in index order.
#[derive(Default)]
pub(crate) struct ToolCallAccumulator {
    /// (index, id, name, arguments-string-so-far), kept ordered by first-seen.
    entries: Vec<ToolCallFragment>,
}

struct ToolCallFragment {
    index: u64,
    id: String,
    name: String,
    arguments: String,
}

impl ToolCallAccumulator {
    /// Ingest one `delta.tool_calls[]` fragment.
    pub(crate) fn ingest(&mut self, fragment: &serde_json::Value) {
        let index = fragment["index"].as_u64().unwrap_or(0);
        let slot = match self.entries.iter_mut().find(|e| e.index == index) {
            Some(e) => e,
            None => {
                self.entries.push(ToolCallFragment {
                    index,
                    id: String::new(),
                    name: String::new(),
                    arguments: String::new(),
                });
                self.entries.last_mut().unwrap()
            }
        };

        if let Some(id) = fragment["id"].as_str() {
            if !id.is_empty() {
                slot.id = id.to_string();
            }
        }
        if let Some(name) = fragment["function"]["name"].as_str() {
            if !name.is_empty() {
                slot.name = name.to_string();
            }
        }
        if let Some(args) = fragment["function"]["arguments"].as_str() {
            slot.arguments.push_str(args);
        }
    }

    /// Whether any tool-call fragments were accumulated.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Assemble the final `ToolCall`s, parsing each `arguments` JSON string.
    pub(crate) fn finish(self) -> Vec<ToolCall> {
        let mut entries = self.entries;
        entries.sort_by_key(|e| e.index);
        entries
            .into_iter()
            .filter(|e| !e.name.is_empty())
            .map(|e| {
                let arguments = serde_json::from_str(&e.arguments)
                    .unwrap_or(serde_json::Value::Null);
                ToolCall {
                    id: e.id,
                    name: e.name,
                    arguments,
                }
            })
            .collect()
    }
}

#[async_trait]
impl AiClient for OpenAiClient {
    async fn send_message(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<AiResponse, AiError> {
        let body = self.build_request_body(messages, tools, false);

        debug!(model = %self.config.model, "OpenAI-compatible API request");

        let response = self
            .http
            .post(self.api_url())
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
        let body = self.build_request_body(messages, tools, true);

        debug!(model = %self.config.model, "OpenAI-compatible API streaming request");

        let response = self
            .http
            .post(self.api_url())
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

        let mut full_content = String::new();
        let mut tool_acc = ToolCallAccumulator::default();
        let mut usage = TokenUsage::default();

        parse_sse_stream(response, |event: SseEvent| {
            let data = event.data.trim();
            // OpenAI signals end-of-stream with a literal `[DONE]` data line.
            if data == "[DONE]" {
                return;
            }

            let json: serde_json::Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => return,
            };

            // Usage (final chunk when stream_options.include_usage is set).
            if let Some(u) = json.get("usage").filter(|u| !u.is_null()) {
                usage.input_tokens = u["prompt_tokens"].as_u64().unwrap_or(usage.input_tokens);
                usage.output_tokens =
                    u["completion_tokens"].as_u64().unwrap_or(usage.output_tokens);
            }

            let delta = &json["choices"][0]["delta"];

            // Streamed text content.
            if let Some(text) = delta["content"].as_str() {
                if !text.is_empty() {
                    full_content.push_str(text);
                    on_chunk(text.to_string());
                }
            }

            // Streamed tool-call fragments (accumulate by index).
            if let Some(fragments) = delta["tool_calls"].as_array() {
                for fragment in fragments {
                    tool_acc.ingest(fragment);
                }
            }
        })
        .await?;

        if usage.input_tokens == 0 && usage.output_tokens == 0 {
            warn!("No usage data received in streaming response");
        }

        let tool_calls = if tool_acc.is_empty() {
            Vec::new()
        } else {
            tool_acc.finish()
        };

        Ok(AiResponse {
            content: full_content,
            tool_calls,
            usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_streamed_tool_call_fragments() {
        // Simulate the fragment sequence OpenAI sends: id+name once, then
        // arguments streamed as string pieces.
        let mut acc = ToolCallAccumulator::default();
        acc.ingest(&serde_json::json!({
            "index": 0,
            "id": "call_1",
            "type": "function",
            "function": { "name": "read_file", "arguments": "" }
        }));
        acc.ingest(&serde_json::json!({
            "index": 0,
            "function": { "arguments": "{\"pa" }
        }));
        acc.ingest(&serde_json::json!({
            "index": 0,
            "function": { "arguments": "th\":\"a.txt\"}" }
        }));

        let calls = acc.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments["path"], "a.txt");
    }

    #[test]
    fn accumulates_multiple_parallel_tool_calls_by_index() {
        let mut acc = ToolCallAccumulator::default();
        // Two tool calls interleaved by index.
        acc.ingest(&serde_json::json!({
            "index": 0, "id": "c0",
            "function": { "name": "read_file", "arguments": "{\"path\":" }
        }));
        acc.ingest(&serde_json::json!({
            "index": 1, "id": "c1",
            "function": { "name": "list_directory", "arguments": "{\"path\":" }
        }));
        acc.ingest(&serde_json::json!({
            "index": 0, "function": { "arguments": "\"a.txt\"}" }
        }));
        acc.ingest(&serde_json::json!({
            "index": 1, "function": { "arguments": "\".\"}" }
        }));

        let calls = acc.finish();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "c0");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments["path"], "a.txt");
        assert_eq!(calls[1].id, "c1");
        assert_eq!(calls[1].name, "list_directory");
        assert_eq!(calls[1].arguments["path"], ".");
    }

    #[test]
    fn empty_accumulator_yields_no_calls() {
        let acc = ToolCallAccumulator::default();
        assert!(acc.is_empty());
        assert!(acc.finish().is_empty());
    }
}
