//! Gemini client struct, request building, and response parsing.
//!
//! Translates the provider-agnostic `Message`/`ContentBlock` interchange model
//! into the Gemini `contents` array, and parses `candidates[0].content.parts`
//! back into a normalized `AiResponse`.
//!
//! Gemini-specific quirks handled here:
//! - Roles: assistant -> `"model"`; user/tool -> `"user"`. There is NO system
//!   role; the system prompt is passed via the top-level `systemInstruction`.
//! - An assistant `ToolUse` block -> a `model` turn with a `functionCall` part.
//! - A `ToolResult` block -> a `user` turn with a `functionResponse` part. The
//!   wire format keys the response by function NAME (not id), so we resolve each
//!   `tool_use_id` back to the name of the matching earlier `functionCall`.
//! - Responses omit tool-call ids, so we synthesize stable ids (`call_<n>`).

use std::collections::HashMap;

use crate::tools::to_gemini_tool;
use crate::{
    AiError, AiResponse, ContentBlock, Message, Role, TokenUsage, ToolCall, ToolDefinition,
};

use super::config::GeminiConfig;

/// Gemini (Generative Language API) client.
pub struct GeminiClient {
    pub(crate) config: GeminiConfig,
    pub(crate) http: reqwest::Client,
}

impl GeminiClient {
    pub fn new(config: GeminiConfig) -> Self {
        Self {
            config,
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    /// Full endpoint URL for `generateContent` or `streamGenerateContent`.
    pub(crate) fn api_url(&self, stream: bool) -> String {
        let method = if stream {
            "streamGenerateContent"
        } else {
            "generateContent"
        };
        self.config.method_url(method)
    }

    /// `x-goog-api-key` auth header (Gemini does not use Bearer auth).
    pub(crate) fn auth_headers(&self) -> Result<reqwest::header::HeaderMap, crate::AiError> {
        use reqwest::header::HeaderValue;
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "x-goog-api-key",
            HeaderValue::from_str(&self.config.api_key).map_err(|e| {
                crate::AiError::ApiError(format!("invalid x-goog-api-key header: {e}"))
            })?,
        );
        Ok(headers)
    }

    /// Build the JSON request body for the Gemini API.
    ///
    /// Translation rules (Message/ContentBlock -> Gemini `contents`):
    /// - `Role::System` (or the configured system prompt) -> top-level
    ///   `systemInstruction:{parts:[{text}]}` (Gemini has no system role).
    /// - Plain user/assistant text -> `{role:"user"|"model", parts:[{text}]}`.
    /// - An assistant `ToolUse` block -> a `model` turn with
    ///   `parts:[{functionCall:{name, args}}]`.
    /// - A `ToolResult` block -> a `user` turn with
    ///   `parts:[{functionResponse:{name, response:{...}}}]`, where `name` is
    ///   resolved from the `tool_use_id` of the matching earlier `functionCall`.
    /// - Tools -> `tools:[{functionDeclarations:[...]}]`.
    pub(crate) fn build_request_body(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> serde_json::Value {
        let mut contents: Vec<serde_json::Value> = Vec::new();
        // Map tool_use_id -> function name, so a later ToolResult can name the
        // function it responds to (Gemini keys responses by name, not id).
        let mut id_to_name: HashMap<String, String> = HashMap::new();

        for msg in messages {
            if msg.role == Role::System {
                // Folded into systemInstruction below; skip here.
                continue;
            }
            self.push_content(&mut contents, msg, &mut id_to_name);
        }

        let mut body = serde_json::json!({
            "contents": contents,
            "generationConfig": {
                "maxOutputTokens": self.config.max_tokens,
                "temperature": self.config.temperature,
            }
        });

        // System instruction: prefer the configured prompt, else the first
        // Role::System message in the conversation.
        let system_text = self.config.system_prompt.clone().or_else(|| {
            messages
                .iter()
                .find(|m| m.role == Role::System)
                .map(|m| m.content.clone())
        });
        if let Some(text) = system_text {
            body["systemInstruction"] = serde_json::json!({
                "parts": [{ "text": text }]
            });
        }

        if !tools.is_empty() {
            let tool_defs: Vec<_> = tools.iter().map(to_gemini_tool).collect();
            body["tools"] = serde_json::json!([{ "functionDeclarations": tool_defs }]);
        }

        body
    }

    /// Translate a single user/assistant/tool `Message` into one Gemini
    /// `contents[]` entry (a role + parts array).
    fn push_content(
        &self,
        out: &mut Vec<serde_json::Value>,
        msg: &Message,
        id_to_name: &mut HashMap<String, String>,
    ) {
        let role = match msg.role {
            Role::Assistant => "model",
            // user/tool turns are "user"; system is handled separately.
            _ => "user",
        };

        // Plain text message (no structured blocks).
        if msg.blocks.is_empty() {
            out.push(serde_json::json!({
                "role": role,
                "parts": [{ "text": msg.content }],
            }));
            return;
        }

        // Structured blocks -> ordered parts.
        let mut parts: Vec<serde_json::Value> = Vec::new();
        for block in &msg.blocks {
            match block {
                ContentBlock::Text { text } => {
                    parts.push(serde_json::json!({ "text": text }));
                }
                ContentBlock::ToolUse { id, name, input } => {
                    // Remember which function this id names, for the response.
                    id_to_name.insert(id.clone(), name.clone());
                    parts.push(serde_json::json!({
                        "functionCall": { "name": name, "args": input },
                    }));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    // Resolve the function name from the originating tool_use id.
                    let name = id_to_name
                        .get(tool_use_id)
                        .cloned()
                        .unwrap_or_else(|| tool_use_id.clone());
                    parts.push(serde_json::json!({
                        "functionResponse": {
                            "name": name,
                            // Gemini requires response to be a JSON object.
                            "response": { "content": content },
                        },
                    }));
                }
            }
        }

        if !parts.is_empty() {
            out.push(serde_json::json!({ "role": role, "parts": parts }));
        }
    }

    /// Parse a non-streaming `generateContent` response into `AiResponse`.
    pub(crate) fn parse_response(&self, json: serde_json::Value) -> Result<AiResponse, AiError> {
        let candidates = json["candidates"]
            .as_array()
            .ok_or_else(|| AiError::ParseError("no candidates in response".into()))?;
        let first = candidates
            .first()
            .ok_or_else(|| AiError::ParseError("empty candidates".into()))?;

        let parts = first["content"]["parts"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        for part in &parts {
            if let Some(text) = part["text"].as_str() {
                content.push_str(text);
            }
            if let Some(fc) = part.get("functionCall") {
                tool_calls.push(parse_function_call(fc, tool_calls.len()));
            }
        }

        let usage = TokenUsage {
            input_tokens: json["usageMetadata"]["promptTokenCount"]
                .as_u64()
                .unwrap_or(0),
            output_tokens: json["usageMetadata"]["candidatesTokenCount"]
                .as_u64()
                .unwrap_or(0),
        };

        Ok(AiResponse {
            content,
            tool_calls,
            usage,
        })
    }
}

/// Build a normalized `ToolCall` from a Gemini `functionCall` part.
///
/// Gemini omits tool-call ids, so we synthesize a stable one from the call's
/// position in the response (`call_<index>`). The same id is later echoed back
/// in the `functionCall`/`functionResponse` turns, so it round-trips cleanly.
pub(crate) fn parse_function_call(fc: &serde_json::Value, index: usize) -> ToolCall {
    let name = fc["name"].as_str().unwrap_or("").to_string();
    let arguments = fc
        .get("args")
        .cloned()
        .unwrap_or(serde_json::Value::Object(Default::default()));
    ToolCall {
        id: format!("call_{index}"),
        name,
        arguments,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gemini::config::DEFAULT_GEMINI_BASE_URL;

    fn client() -> GeminiClient {
        GeminiClient::new(GeminiConfig::new(
            "test-key",
            "gemini-2.0-flash",
            DEFAULT_GEMINI_BASE_URL,
        ))
    }

    #[test]
    fn plain_text_user_message_maps_to_user_role() {
        let c = client();
        let body = c.build_request_body(&[Message::text(Role::User, "hello")], &[]);
        let m = &body["contents"][0];
        assert_eq!(m["role"], "user");
        assert_eq!(m["parts"][0]["text"], "hello");
    }

    #[test]
    fn assistant_text_maps_to_model_role() {
        let c = client();
        let body = c.build_request_body(&[Message::text(Role::Assistant, "hi back")], &[]);
        assert_eq!(body["contents"][0]["role"], "model");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "hi back");
    }

    #[test]
    fn system_message_folds_into_system_instruction() {
        let c = client();
        let msgs = vec![
            Message::text(Role::System, "be terse"),
            Message::text(Role::User, "hi"),
        ];
        let body = c.build_request_body(&msgs, &[]);
        // No system turn in contents; it goes to systemInstruction.
        assert_eq!(body["contents"].as_array().unwrap().len(), 1);
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "be terse");
    }

    #[test]
    fn configured_system_prompt_becomes_system_instruction() {
        let c = GeminiClient::new(
            GeminiConfig::new("k", "gemini-2.0-flash", DEFAULT_GEMINI_BASE_URL)
                .with_system_prompt("you are jarvis"),
        );
        let body = c.build_request_body(&[Message::text(Role::User, "hi")], &[]);
        assert_eq!(
            body["systemInstruction"]["parts"][0]["text"],
            "you are jarvis"
        );
    }

    #[test]
    fn tool_use_assistant_turn_serializes_to_function_call() {
        let c = client();
        let msgs = vec![Message::blocks(
            Role::Assistant,
            vec![
                ContentBlock::Text {
                    text: "let me check".into(),
                },
                ContentBlock::ToolUse {
                    id: "call_0".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({ "path": "a.txt" }),
                },
            ],
        )];
        let body = c.build_request_body(&msgs, &[]);
        let parts = body["contents"][0]["parts"].as_array().unwrap();
        assert_eq!(body["contents"][0]["role"], "model");
        assert_eq!(parts[0]["text"], "let me check");
        let fc = &parts[1]["functionCall"];
        assert_eq!(fc["name"], "read_file");
        // args is a JSON OBJECT (not a string, unlike OpenAI).
        assert_eq!(fc["args"]["path"], "a.txt");
    }

    #[test]
    fn tool_result_serializes_to_function_response_with_name() {
        let c = client();
        // The assistant turn names call_0 -> read_file; the result must resolve
        // that name into the functionResponse.
        let msgs = vec![
            Message::blocks(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_0".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({ "path": "a.txt" }),
                }],
            ),
            Message::blocks(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_0".into(),
                    content: "file contents".into(),
                    is_error: false,
                }],
            ),
        ];
        let body = c.build_request_body(&msgs, &[]);
        let result_turn = &body["contents"][1];
        assert_eq!(result_turn["role"], "user");
        let fr = &result_turn["parts"][0]["functionResponse"];
        // Name resolved from the originating tool_use id.
        assert_eq!(fr["name"], "read_file");
        // response must be a JSON object.
        assert!(fr["response"].is_object());
        assert_eq!(fr["response"]["content"], "file contents");
    }

    #[test]
    fn tools_serialize_to_function_declarations() {
        let c = client();
        let tools = crate::tools::read_only_tools();
        let body = c.build_request_body(&[Message::text(Role::User, "hi")], &tools);
        let decls = body["tools"][0]["functionDeclarations"].as_array().unwrap();
        assert!(!decls.is_empty());
        assert!(decls[0]["name"].is_string());
        assert!(decls[0]["parameters"].is_object());
    }

    #[test]
    fn parse_response_extracts_text_and_function_calls() {
        let c = client();
        let json = serde_json::json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [
                        { "text": "checking now" },
                        { "functionCall": {
                            "name": "read_file",
                            "args": { "path": "main.rs" }
                        }}
                    ]
                }
            }],
            "usageMetadata": {
                "promptTokenCount": 12,
                "candidatesTokenCount": 7
            }
        });
        let resp = c.parse_response(json).unwrap();
        assert_eq!(resp.content, "checking now");
        assert_eq!(resp.tool_calls.len(), 1);
        // Synthesized id is stable from index.
        assert_eq!(resp.tool_calls[0].id, "call_0");
        assert_eq!(resp.tool_calls[0].name, "read_file");
        assert_eq!(resp.tool_calls[0].arguments["path"], "main.rs");
        assert_eq!(resp.usage.input_tokens, 12);
        assert_eq!(resp.usage.output_tokens, 7);
    }

    #[test]
    fn parse_response_synthesizes_distinct_ids_for_parallel_calls() {
        let c = client();
        let json = serde_json::json!({
            "candidates": [{
                "content": { "parts": [
                    { "functionCall": { "name": "read_file", "args": {} } },
                    { "functionCall": { "name": "list_directory", "args": {} } }
                ]}
            }]
        });
        let resp = c.parse_response(json).unwrap();
        assert_eq!(resp.tool_calls.len(), 2);
        assert_eq!(resp.tool_calls[0].id, "call_0");
        assert_eq!(resp.tool_calls[1].id, "call_1");
        assert_ne!(resp.tool_calls[0].id, resp.tool_calls[1].id);
    }

    #[test]
    fn parse_response_errors_on_missing_candidates() {
        let c = client();
        let json = serde_json::json!({ "usageMetadata": {} });
        assert!(c.parse_response(json).is_err());
    }

    #[test]
    fn full_tool_roundtrip_serialization() {
        let c = client();
        let msgs = vec![
            Message::text(Role::User, "read a.txt"),
            Message::blocks(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_0".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({ "path": "a.txt" }),
                }],
            ),
            Message::blocks(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_0".into(),
                    content: "hello".into(),
                    is_error: false,
                }],
            ),
        ];
        let body = c.build_request_body(&msgs, &[]);
        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "read a.txt");
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][0]["functionCall"]["name"], "read_file");
        assert_eq!(contents[2]["role"], "user");
        // functionResponse name resolved back to the function called.
        assert_eq!(
            contents[2]["parts"][0]["functionResponse"]["name"],
            "read_file"
        );
    }
}
