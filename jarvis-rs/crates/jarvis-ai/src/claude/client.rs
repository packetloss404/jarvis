//! Claude API client struct, request building, and response parsing.

use crate::tools::to_claude_tool;
use crate::{AiError, AiResponse, ContentBlock, Message, Role, TokenUsage, ToolCall, ToolDefinition};

use super::config::ClaudeConfig;

pub(crate) const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
pub(crate) const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Claude API client.
pub struct ClaudeClient {
    pub(crate) config: ClaudeConfig,
    pub(crate) http: reqwest::Client,
}

impl ClaudeClient {
    pub fn new(config: ClaudeConfig) -> Self {
        Self {
            config,
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    /// Return the API URL (both auth methods use api.anthropic.com).
    pub(crate) fn api_url(&self) -> &'static str {
        ANTHROPIC_API_URL
    }

    /// Build auth headers for the configured auth method.
    pub(crate) fn auth_headers(&self) -> Result<reqwest::header::HeaderMap, crate::AiError> {
        use reqwest::header::HeaderValue;
        let mut headers = reqwest::header::HeaderMap::new();
        match self.config.auth_method {
            super::config::AuthMethod::ApiKey => {
                headers.insert(
                    "x-api-key",
                    HeaderValue::from_str(&self.config.token).map_err(|e| {
                        crate::AiError::ApiError(format!("invalid API key header: {e}"))
                    })?,
                );
            }
            super::config::AuthMethod::OAuth => {
                headers.insert(
                    "Authorization",
                    HeaderValue::from_str(&format!("Bearer {}", self.config.token)).map_err(
                        |e| crate::AiError::ApiError(format!("invalid OAuth header: {e}")),
                    )?,
                );
            }
        }
        headers.insert(
            "anthropic-version",
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
        Ok(headers)
    }

    /// Build the JSON request body for the Messages API.
    pub(crate) fn build_request_body(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        stream: bool,
    ) -> serde_json::Value {
        let mut msgs = Vec::new();
        for msg in messages {
            let role = match msg.role {
                Role::User | Role::Tool => "user",
                Role::Assistant => "assistant",
                Role::System => continue, // system is separate in Claude API
            };

            // When a message carries structured blocks, emit a content-block
            // ARRAY (tool_use on assistant turns, tool_result on user turns).
            // Otherwise emit the plain-text content string (back-compat).
            let content = if msg.blocks.is_empty() {
                serde_json::json!(msg.content)
            } else {
                let blocks: Vec<serde_json::Value> = msg
                    .blocks
                    .iter()
                    .map(|block| match block {
                        ContentBlock::Text { text } => serde_json::json!({
                            "type": "text",
                            "text": text,
                        }),
                        ContentBlock::ToolUse { id, name, input } => serde_json::json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": input,
                        }),
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": content,
                            "is_error": is_error,
                        }),
                    })
                    .collect();
                serde_json::json!(blocks)
            };

            msgs.push(serde_json::json!({
                "role": role,
                "content": content,
            }));
        }

        let mut body = serde_json::json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "messages": msgs,
        });

        if let Some(ref system) = self.config.system_prompt {
            body["system"] = serde_json::json!(system);
        } else {
            // Check for system message in the messages list
            for msg in messages {
                if msg.role == Role::System {
                    body["system"] = serde_json::json!(msg.content);
                    break;
                }
            }
        }

        if !tools.is_empty() {
            let tool_defs: Vec<_> = tools.iter().map(to_claude_tool).collect();
            body["tools"] = serde_json::json!(tool_defs);
        }

        if stream {
            body["stream"] = serde_json::json!(true);
        }

        body
    }

    /// Parse a non-streaming response.
    pub(crate) fn parse_response(&self, json: serde_json::Value) -> Result<AiResponse, AiError> {
        let content = json["content"]
            .as_array()
            .and_then(|blocks| {
                blocks.iter().find_map(|b| {
                    if b["type"] == "text" {
                        b["text"].as_str().map(String::from)
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_default();

        let tool_calls = json["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b["type"] == "tool_use")
                    .map(|b| ToolCall {
                        id: b["id"].as_str().unwrap_or("").to_string(),
                        name: b["name"].as_str().unwrap_or("").to_string(),
                        arguments: b["input"].clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let usage = TokenUsage {
            input_tokens: json["usage"]["input_tokens"].as_u64().unwrap_or(0),
            output_tokens: json["usage"]["output_tokens"].as_u64().unwrap_or(0),
        };

        Ok(AiResponse {
            content,
            tool_calls,
            usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::config::AuthMethod;

    fn client() -> ClaudeClient {
        ClaudeClient::new(ClaudeConfig::new("test-token", AuthMethod::ApiKey))
    }

    #[test]
    fn plain_text_message_serializes_as_string() {
        let c = client();
        let msgs = vec![Message::text(Role::User, "hello there")];
        let body = c.build_request_body(&msgs, &[], false);
        let content = &body["messages"][0]["content"];
        assert_eq!(content, &serde_json::json!("hello there"));
        assert_eq!(body["messages"][0]["role"], "user");
    }

    #[test]
    fn tool_use_turn_serializes_as_block_array() {
        let c = client();
        let msgs = vec![Message::blocks(
            Role::Assistant,
            vec![
                ContentBlock::Text {
                    text: "let me check".into(),
                },
                ContentBlock::ToolUse {
                    id: "toolu_1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({ "path": "a.txt" }),
                },
            ],
        )];
        let body = c.build_request_body(&msgs, &[], false);
        let content = &body["messages"][0]["content"];
        assert!(content.is_array(), "tool turn must be an array");
        let arr = content.as_array().unwrap();
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[1]["type"], "tool_use");
        assert_eq!(arr[1]["id"], "toolu_1");
        assert_eq!(arr[1]["name"], "read_file");
        assert_eq!(arr[1]["input"]["path"], "a.txt");
    }

    #[test]
    fn tool_result_keyed_by_tool_use_id() {
        let c = client();
        let msgs = vec![Message::blocks(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_1".into(),
                content: "file contents".into(),
                is_error: false,
            }],
        )];
        let body = c.build_request_body(&msgs, &[], false);
        let block = &body["messages"][0]["content"][0];
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(block["type"], "tool_result");
        assert_eq!(block["tool_use_id"], "toolu_1");
        assert_eq!(block["content"], "file contents");
        assert_eq!(block["is_error"], false);
    }

    #[test]
    fn tool_result_error_flag_propagates() {
        let c = client();
        let msgs = vec![Message::blocks(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_x".into(),
                content: "Access denied".into(),
                is_error: true,
            }],
        )];
        let body = c.build_request_body(&msgs, &[], false);
        assert_eq!(body["messages"][0]["content"][0]["is_error"], true);
    }

    #[test]
    fn full_tool_roundtrip_serialization() {
        let c = client();
        let msgs = vec![
            Message::text(Role::User, "read a.txt"),
            Message::blocks(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({ "path": "a.txt" }),
                }],
            ),
            Message::blocks(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    content: "hello".into(),
                    is_error: false,
                }],
            ),
        ];
        let body = c.build_request_body(&msgs, &[], false);
        let m = body["messages"].as_array().unwrap();
        assert_eq!(m.len(), 3);
        assert_eq!(m[0]["content"], serde_json::json!("read a.txt"));
        assert_eq!(m[1]["content"][0]["type"], "tool_use");
        assert_eq!(m[2]["content"][0]["type"], "tool_result");
        // tool_use_id on the result must match the tool_use id.
        assert_eq!(m[1]["content"][0]["id"], m[2]["content"][0]["tool_use_id"]);
    }
}
