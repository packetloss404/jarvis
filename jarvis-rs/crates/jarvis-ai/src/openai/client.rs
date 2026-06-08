//! OpenAI-compatible client struct, request building, and response parsing.
//!
//! Translates the provider-agnostic `Message`/`ContentBlock` interchange model
//! into the OpenAI Chat Completions `messages` array, and parses the response
//! `choices[0].message` back into a normalized `AiResponse`.

use crate::tools::to_openai_tool;
use crate::{
    AiError, AiResponse, ContentBlock, Message, Role, TokenUsage, ToolCall, ToolDefinition,
};

use super::config::OpenAiConfig;

/// OpenAI-compatible Chat Completions client (also serves MiniMax).
pub struct OpenAiClient {
    pub(crate) config: OpenAiConfig,
    pub(crate) http: reqwest::Client,
}

impl OpenAiClient {
    pub fn new(config: OpenAiConfig) -> Self {
        Self {
            config,
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    /// Full Chat Completions endpoint URL.
    pub(crate) fn api_url(&self) -> String {
        self.config.chat_completions_url()
    }

    /// Build `Authorization: Bearer <key>` headers.
    pub(crate) fn auth_headers(&self) -> Result<reqwest::header::HeaderMap, crate::AiError> {
        use reqwest::header::HeaderValue;
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {}", self.config.api_key)).map_err(|e| {
                crate::AiError::ApiError(format!("invalid Authorization header: {e}"))
            })?,
        );
        Ok(headers)
    }

    /// Build the JSON request body for the Chat Completions API.
    ///
    /// Translation rules (Message/ContentBlock -> OpenAI `messages`):
    /// - `Role::System` -> `{role:"system", content}`.
    /// - Plain user/assistant text -> `{role, content}`.
    /// - An assistant message whose blocks contain `ToolUse` ->
    ///   `{role:"assistant", content:<joined text or null>, tool_calls:[...]}`
    ///   where each tool call is
    ///   `{id, type:"function", function:{name, arguments:<JSON string>}}`.
    /// - Each `ToolResult` block -> a SEPARATE
    ///   `{role:"tool", tool_call_id, content}` message.
    pub(crate) fn build_request_body(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        stream: bool,
    ) -> serde_json::Value {
        let mut msgs: Vec<serde_json::Value> = Vec::new();

        // Inject a system message: prefer the configured system prompt, else
        // surface any Role::System message already in the list.
        if let Some(ref system) = self.config.system_prompt {
            msgs.push(serde_json::json!({ "role": "system", "content": system }));
        }

        for msg in messages {
            match msg.role {
                Role::System => {
                    // If we already injected the configured prompt, skip
                    // additional system messages to avoid duplication.
                    if self.config.system_prompt.is_none() {
                        msgs.push(serde_json::json!({
                            "role": "system",
                            "content": msg.content,
                        }));
                    }
                }
                Role::User | Role::Assistant | Role::Tool => {
                    self.push_role_message(&mut msgs, msg);
                }
            }
        }

        let mut body = serde_json::json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "messages": msgs,
        });

        if !tools.is_empty() {
            let tool_defs: Vec<_> = tools.iter().map(to_openai_tool).collect();
            body["tools"] = serde_json::json!(tool_defs);
        }

        if stream {
            body["stream"] = serde_json::json!(true);
            // Ask for usage in the terminal streaming chunk (OpenAI extension).
            body["stream_options"] = serde_json::json!({ "include_usage": true });
        }

        body
    }

    /// Translate a single user/assistant/tool `Message` into one or more
    /// OpenAI message objects (tool results expand into separate `tool` msgs).
    fn push_role_message(&self, out: &mut Vec<serde_json::Value>, msg: &Message) {
        // Plain text message (no structured blocks).
        if msg.blocks.is_empty() {
            let role = match msg.role {
                Role::Assistant => "assistant",
                // A plain Tool-role message has no tool_call_id, so treat it as
                // user text (defensive; the tool loop always uses blocks).
                _ => "user",
            };
            out.push(serde_json::json!({ "role": role, "content": msg.content }));
            return;
        }

        // Structured blocks. Partition into text, tool_use, tool_result.
        let mut text_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<serde_json::Value> = Vec::new();
        let mut tool_results: Vec<serde_json::Value> = Vec::new();

        for block in &msg.blocks {
            match block {
                ContentBlock::Text { text } => text_parts.push(text.clone()),
                ContentBlock::ToolUse { id, name, input } => {
                    // OpenAI expects arguments as a JSON STRING.
                    let arguments = serde_json::to_string(input)
                        .unwrap_or_else(|_| "{}".to_string());
                    tool_calls.push(serde_json::json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": arguments,
                        },
                    }));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    tool_results.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": tool_use_id,
                        "content": content,
                    }));
                }
            }
        }

        // Assistant turn carrying tool_use blocks -> assistant msg + tool_calls.
        if !tool_calls.is_empty() {
            let content = if text_parts.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::json!(text_parts.join(""))
            };
            out.push(serde_json::json!({
                "role": "assistant",
                "content": content,
                "tool_calls": tool_calls,
            }));
        } else if !text_parts.is_empty() {
            // Text-only block message (e.g. assistant text turn).
            let role = match msg.role {
                Role::Assistant => "assistant",
                _ => "user",
            };
            out.push(serde_json::json!({
                "role": role,
                "content": text_parts.join(""),
            }));
        }

        // Each tool_result becomes its own `tool` message.
        out.extend(tool_results);
    }

    /// Parse a non-streaming Chat Completions response into `AiResponse`.
    pub(crate) fn parse_response(&self, json: serde_json::Value) -> Result<AiResponse, AiError> {
        let message = &json["choices"][0]["message"];

        let content = message["content"].as_str().unwrap_or_default().to_string();

        let tool_calls = message["tool_calls"]
            .as_array()
            .map(|calls| {
                calls
                    .iter()
                    .map(parse_tool_call)
                    .collect::<Vec<ToolCall>>()
            })
            .unwrap_or_default();

        let usage = TokenUsage {
            input_tokens: json["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
            output_tokens: json["usage"]["completion_tokens"].as_u64().unwrap_or(0),
        };

        Ok(AiResponse {
            content,
            tool_calls,
            usage,
        })
    }
}

/// Parse a single OpenAI `tool_calls[]` entry into a normalized `ToolCall`.
///
/// `function.arguments` arrives as a JSON STRING; we parse it into a
/// `serde_json::Value` so the rest of Jarvis sees structured input.
pub(crate) fn parse_tool_call(call: &serde_json::Value) -> ToolCall {
    let id = call["id"].as_str().unwrap_or("").to_string();
    let name = call["function"]["name"].as_str().unwrap_or("").to_string();
    let arguments = call["function"]["arguments"]
        .as_str()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(serde_json::Value::Null);
    ToolCall {
        id,
        name,
        arguments,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openai::config::DEFAULT_OPENAI_BASE_URL;

    fn client() -> OpenAiClient {
        OpenAiClient::new(OpenAiConfig::new(
            "test-key",
            "gpt-4o",
            DEFAULT_OPENAI_BASE_URL,
        ))
    }

    #[test]
    fn plain_text_message_serializes_as_string() {
        let c = client();
        let msgs = vec![Message::text(Role::User, "hello there")];
        let body = c.build_request_body(&msgs, &[], false);
        let m = &body["messages"][0];
        assert_eq!(m["role"], "user");
        assert_eq!(m["content"], "hello there");
    }

    #[test]
    fn system_role_message_maps_to_system() {
        let c = client();
        let msgs = vec![
            Message::text(Role::System, "be terse"),
            Message::text(Role::User, "hi"),
        ];
        let body = c.build_request_body(&msgs, &[], false);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "be terse");
        assert_eq!(body["messages"][1]["role"], "user");
    }

    #[test]
    fn configured_system_prompt_is_first_message() {
        let c = OpenAiClient::new(
            OpenAiConfig::new("k", "gpt-4o", DEFAULT_OPENAI_BASE_URL)
                .with_system_prompt("you are jarvis"),
        );
        let msgs = vec![Message::text(Role::User, "hi")];
        let body = c.build_request_body(&msgs, &[], false);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "you are jarvis");
        assert_eq!(body["messages"][1]["role"], "user");
    }

    #[test]
    fn tool_use_assistant_turn_serializes_to_openai_tool_calls() {
        let c = client();
        let msgs = vec![Message::blocks(
            Role::Assistant,
            vec![
                ContentBlock::Text {
                    text: "let me check".into(),
                },
                ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({ "path": "a.txt" }),
                },
            ],
        )];
        let body = c.build_request_body(&msgs, &[], false);
        let m = &body["messages"][0];
        assert_eq!(m["role"], "assistant");
        assert_eq!(m["content"], "let me check");
        let tc = &m["tool_calls"][0];
        assert_eq!(tc["id"], "call_1");
        assert_eq!(tc["type"], "function");
        assert_eq!(tc["function"]["name"], "read_file");
        // arguments must be a JSON STRING, not an object.
        let args = tc["function"]["arguments"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(args).unwrap();
        assert_eq!(parsed["path"], "a.txt");
    }

    #[test]
    fn tool_use_without_text_uses_null_content() {
        let c = client();
        let msgs = vec![Message::blocks(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_x".into(),
                name: "list_directory".into(),
                input: serde_json::json!({ "path": "." }),
            }],
        )];
        let body = c.build_request_body(&msgs, &[], false);
        assert!(body["messages"][0]["content"].is_null());
        assert_eq!(body["messages"][0]["tool_calls"][0]["id"], "call_x");
    }

    #[test]
    fn tool_result_becomes_separate_tool_message() {
        let c = client();
        let msgs = vec![Message::blocks(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: "file contents".into(),
                is_error: false,
            }],
        )];
        let body = c.build_request_body(&msgs, &[], false);
        let m = &body["messages"][0];
        assert_eq!(m["role"], "tool");
        assert_eq!(m["tool_call_id"], "call_1");
        assert_eq!(m["content"], "file contents");
    }

    #[test]
    fn full_tool_roundtrip_serialization() {
        let c = client();
        let msgs = vec![
            Message::text(Role::User, "read a.txt"),
            Message::blocks(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({ "path": "a.txt" }),
                }],
            ),
            Message::blocks(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".into(),
                    content: "hello".into(),
                    is_error: false,
                }],
            ),
        ];
        let body = c.build_request_body(&msgs, &[], false);
        let m = body["messages"].as_array().unwrap();
        assert_eq!(m.len(), 3);
        assert_eq!(m[0]["role"], "user");
        assert_eq!(m[0]["content"], "read a.txt");
        assert_eq!(m[1]["role"], "assistant");
        assert_eq!(m[1]["tool_calls"][0]["id"], "call_1");
        assert_eq!(m[2]["role"], "tool");
        // tool_call_id on the result must match the tool_call id.
        assert_eq!(
            m[1]["tool_calls"][0]["id"],
            m[2]["tool_call_id"]
        );
    }

    #[test]
    fn tools_serialize_to_openai_function_format() {
        let c = client();
        let tools = crate::tools::read_only_tools();
        let body = c.build_request_body(&[Message::text(Role::User, "hi")], &tools, false);
        let arr = body["tools"].as_array().unwrap();
        assert!(!arr.is_empty());
        assert_eq!(arr[0]["type"], "function");
        assert!(arr[0]["function"]["name"].is_string());
        assert!(arr[0]["function"]["parameters"].is_object());
    }

    #[test]
    fn parse_response_extracts_content_and_tool_calls() {
        let c = client();
        let json = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "checking now",
                    "tool_calls": [{
                        "id": "call_42",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"main.rs\"}"
                        }
                    }]
                }
            }],
            "usage": { "prompt_tokens": 12, "completion_tokens": 7 }
        });
        let resp = c.parse_response(json).unwrap();
        assert_eq!(resp.content, "checking now");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "call_42");
        assert_eq!(resp.tool_calls[0].name, "read_file");
        // arguments JSON string must be parsed into a Value.
        assert_eq!(resp.tool_calls[0].arguments["path"], "main.rs");
        assert_eq!(resp.usage.input_tokens, 12);
        assert_eq!(resp.usage.output_tokens, 7);
    }

    #[test]
    fn parse_response_handles_null_content() {
        let c = client();
        let json = serde_json::json!({
            "choices": [{
                "message": {
                    "content": serde_json::Value::Null,
                    "tool_calls": [{
                        "id": "c1",
                        "type": "function",
                        "function": { "name": "list_directory", "arguments": "{}" }
                    }]
                }
            }]
        });
        let resp = c.parse_response(json).unwrap();
        assert_eq!(resp.content, "");
        assert_eq!(resp.tool_calls.len(), 1);
    }
}
