//! Async chat methods for Session (send_message + streaming) with tool loops.

use std::sync::Arc;

use crate::{AiClient, AiError, AiResponse, ContentBlock, Message, Role};

use super::manager::Session;
use super::types::{
    ApprovalDecision, ApprovalGate, ApprovalRequest, BusyGuard, ToolEvent, ToolExecutor,
    ToolOutcome, APPROVAL_TIMEOUT,
};
use super::types::tool_requires_approval;

impl Session {
    /// Add a user message and get the assistant's response.
    /// If the AI calls tools, this runs the tool-call loop automatically.
    pub async fn chat(
        &mut self,
        client: &dyn AiClient,
        user_message: impl Into<String>,
    ) -> Result<String, AiError> {
        let _guard = BusyGuard::acquire(&self.busy)?;

        self.messages
            .push(Message::text(Role::User, user_message.into()));

        let mut rounds = 0;
        loop {
            let messages = self.build_messages();
            let response = client.send_message(&messages, &self.tools).await?;
            self.tracker.record(&self.provider, &response.usage);

            if !self.should_run_tools(&response) {
                self.messages
                    .push(Message::text(Role::Assistant, response.content.clone()));
                return Ok(response.content);
            }

            rounds += 1;
            if rounds > self.max_tool_rounds {
                tracing::warn!("Max tool rounds reached, returning partial response");
                self.messages
                    .push(Message::text(Role::Assistant, response.content.clone()));
                return Ok(response.content);
            }

            self.run_tool_round(&response).await;
        }
    }

    /// Send a message with streaming, returning the full response.
    ///
    /// Control flow per round:
    /// 1. Stream the assistant response (text chunks go to `on_chunk`).
    /// 2. If the response has no tool calls, or no executor is set, finish.
    /// 3. Otherwise run each requested tool (read-only) on a blocking thread,
    ///    append a proper assistant `tool_use` turn and a user `tool_result`
    ///    turn, surface activity via the tool-event callback, and re-call.
    /// 4. `max_tool_rounds` is a hard cap to prevent runaway loops.
    pub async fn chat_streaming(
        &mut self,
        client: &dyn AiClient,
        user_message: impl Into<String>,
        on_chunk: Box<dyn Fn(String) + Send + Sync>,
    ) -> Result<String, AiError> {
        let _guard = BusyGuard::acquire(&self.busy)?;

        self.messages
            .push(Message::text(Role::User, user_message.into()));

        // Share the chunk callback across rounds (re-called each loop iteration).
        let on_chunk: Arc<dyn Fn(String) + Send + Sync> = Arc::from(on_chunk);

        let mut rounds = 0;
        loop {
            let messages = self.build_messages();
            let cb = on_chunk.clone();
            let per_round: Box<dyn Fn(String) + Send + Sync> =
                Box::new(move |c| cb(c));
            let response = client
                .send_message_streaming(&messages, &self.tools, per_round)
                .await?;
            self.tracker.record(&self.provider, &response.usage);

            if !self.should_run_tools(&response) {
                self.messages
                    .push(Message::text(Role::Assistant, response.content.clone()));
                return Ok(response.content);
            }

            rounds += 1;
            if rounds > self.max_tool_rounds {
                tracing::warn!("Max tool rounds reached, returning partial response");
                self.messages
                    .push(Message::text(Role::Assistant, response.content.clone()));
                return Ok(response.content);
            }

            self.run_tool_round(&response).await;
        }
    }

    /// Whether this response should trigger a tool round (has tool calls AND an
    /// executor is configured).
    fn should_run_tools(&self, response: &AiResponse) -> bool {
        !response.tool_calls.is_empty() && self.tool_executor.is_some()
    }

    /// Execute all tool calls in `response`, appending the assistant `tool_use`
    /// turn and the following user `tool_result` turn to the conversation.
    ///
    /// The sync executor runs on a blocking thread so the async loop is never
    /// blocked. Each call/result is surfaced through the tool-event callback.
    async fn run_tool_round(&mut self, response: &AiResponse) {
        // 1. Assistant turn: text (if any) + tool_use blocks.
        let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
        if !response.content.is_empty() {
            assistant_blocks.push(ContentBlock::Text {
                text: response.content.clone(),
            });
        }
        for call in &response.tool_calls {
            assistant_blocks.push(ContentBlock::ToolUse {
                id: call.id.clone(),
                name: call.name.clone(),
                input: call.arguments.clone(),
            });
        }
        self.messages
            .push(Message::blocks(Role::Assistant, assistant_blocks));

        // 2. Run each tool and collect matching tool_result blocks.
        let executor = self
            .tool_executor
            .as_ref()
            .expect("run_tool_round called without executor");
        let mut result_blocks: Vec<ContentBlock> = Vec::new();
        for call in &response.tool_calls {
            // Surface the call.
            self.emit_tool_event(ToolEvent::Call {
                name: call.name.clone(),
                input: call.arguments.clone(),
            });

            // APPROVAL GATE: mutating/exec tools must clear an explicit human
            // decision before they touch the executor. Read-only tools skip
            // this entirely. A missing gate, a deny, a timeout, or a dropped
            // channel all FAIL CLOSED — the call is rejected and nothing runs.
            if tool_requires_approval(&call.name) {
                let decision = request_approval(
                    self.approval_gate.as_deref(),
                    &call.id,
                    &call.name,
                    &call.arguments,
                )
                .await;
                if !decision.is_approved() {
                    let outcome = ToolOutcome::error(format!(
                        "User denied this tool call. The '{}' tool was NOT executed. \
                         Do not retry it; continue without it or ask the user how to proceed.",
                        call.name
                    ));
                    self.emit_tool_event(ToolEvent::Result {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        summary: "denied by user".to_string(),
                        is_error: true,
                    });
                    result_blocks.push(ContentBlock::ToolResult {
                        tool_use_id: call.id.clone(),
                        content: outcome.content,
                        is_error: outcome.is_error,
                    });
                    continue;
                }
            }

            let outcome =
                run_tool_blocking(executor.clone(), &call.name, &call.arguments).await;

            // Surface the result (summarized for display).
            self.emit_tool_event(ToolEvent::Result {
                id: call.id.clone(),
                name: call.name.clone(),
                summary: summarize(&outcome.content),
                is_error: outcome.is_error,
            });

            result_blocks.push(ContentBlock::ToolResult {
                tool_use_id: call.id.clone(),
                content: outcome.content,
                is_error: outcome.is_error,
            });
        }

        // 3. User turn carrying the tool_result blocks.
        self.messages
            .push(Message::blocks(Role::User, result_blocks));
    }

    fn emit_tool_event(&self, event: ToolEvent) {
        if let Some(ref cb) = self.tool_event_callback {
            cb(event);
        }
    }
}

/// Run a sync tool executor on a blocking thread so it doesn't stall the async
/// runtime. Falls back to inline execution if not on a Tokio runtime.
async fn run_tool_blocking(
    executor: Arc<ToolExecutor>,
    name: &str,
    args: &serde_json::Value,
) -> ToolOutcome {
    let name = name.to_string();
    let args = args.clone();
    match tokio::runtime::Handle::try_current() {
        Ok(_) => {
            tokio::task::spawn_blocking(move || executor(&name, &args))
                .await
                .unwrap_or_else(|e| ToolOutcome::error(format!("tool task failed: {e}")))
        }
        Err(_) => executor(&name, &args),
    }
}

/// Route a mutating/exec tool call through the human-approval gate and await
/// the decision under [`APPROVAL_TIMEOUT`]. FAILS CLOSED to
/// [`ApprovalDecision::Deny`] when:
/// * no gate is installed,
/// * the timeout elapses before the human answers, or
/// * the decision channel is dropped without a response.
async fn request_approval(
    gate: Option<&ApprovalGate>,
    call_id: &str,
    tool: &str,
    args: &serde_json::Value,
) -> ApprovalDecision {
    request_approval_with_timeout(gate, call_id, tool, args, APPROVAL_TIMEOUT).await
}

/// Inner approval routine with an explicit timeout so the deny-on-timeout path
/// is testable without waiting the production [`APPROVAL_TIMEOUT`].
async fn request_approval_with_timeout(
    gate: Option<&ApprovalGate>,
    call_id: &str,
    tool: &str,
    args: &serde_json::Value,
    timeout: std::time::Duration,
) -> ApprovalDecision {
    let gate = match gate {
        Some(g) => g,
        // No gate installed → cannot approve → fail closed.
        None => return ApprovalDecision::Deny,
    };

    let request = ApprovalRequest {
        id: call_id.to_string(),
        tool: tool.to_string(),
        summary: approval_summary(tool, args),
    };

    let rx = gate(request);
    match tokio::time::timeout(timeout, rx).await {
        // Human answered in time.
        Ok(Ok(decision)) => decision,
        // Sender dropped without a decision → fail closed.
        Ok(Err(_)) => ApprovalDecision::Deny,
        // Timed out → fail closed.
        Err(_) => ApprovalDecision::Deny,
    }
}

/// Build the human-readable approval summary for a mutating/exec tool call:
/// the EXACT command / path / content the human is being asked to OK.
///
/// INTEGRITY: the summary carries the FULL command / content / args — never a
/// silent truncation. "What you approve is what runs": a model must not be able
/// to hide payload past a display cutoff. The panel's summary box is scrollable.
///
/// `working_directory` is intentionally NOT surfaced for `run_command`: the
/// executor ALWAYS runs in the sandbox root and ignores any model-supplied cwd,
/// so showing one would be a bait-and-switch. The schema no longer accepts it.
fn approval_summary(tool: &str, args: &serde_json::Value) -> String {
    match tool {
        "run_command" => {
            let cmd = args["command"].as_str().unwrap_or("<missing command>");
            // Full command, no truncation. Runs in the sandbox root (no cwd shown).
            format!("run_command (in workspace root): `{cmd}`")
        }
        "write_file" => {
            let path = args["path"].as_str().unwrap_or("<missing path>");
            let content = args["content"].as_str().unwrap_or("");
            let bytes = content.len();
            // FULL content — what the human approves is exactly what is written.
            format!("write_file: {path} ({bytes} bytes)\n--- content ---\n{content}")
        }
        other => {
            // Generic fallback so a future approval-required tool still gets a
            // meaningful prompt — with the FULL args, not a truncated preview.
            format!("{other}: {}", full_json(args))
        }
    }
}

/// Render JSON args in full (no truncation) for a generic approval summary.
fn full_json(args: &serde_json::Value) -> String {
    serde_json::to_string_pretty(args).unwrap_or_else(|_| "<unserializable args>".to_string())
}

/// Produce a short single-line summary of a tool result for inline display.
fn summarize(content: &str) -> String {
    let first_line = content.lines().next().unwrap_or("").trim();
    let line_count = content.lines().count();
    let mut s: String = first_line.chars().take(120).collect();
    if line_count > 1 {
        s.push_str(&format!(" (+{} more lines)", line_count - 1));
    }
    if s.is_empty() {
        s.push_str("(no output)");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AiClient, AiError, AiResponse, ContentBlock, ToolCall, ToolDefinition, ToolOutcome,
        TokenUsage,
    };
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// A scripted AI client. Returns `responses[i]` on the i-th call, clamping to
    /// the last one. This lets a test drive the tool loop: a first response that
    /// requests a tool, then a final text response that ends the loop.
    struct ScriptedClient {
        responses: Vec<AiResponse>,
        calls: AtomicUsize,
    }

    impl ScriptedClient {
        fn new(responses: Vec<AiResponse>) -> Self {
            Self {
                responses,
                calls: AtomicUsize::new(0),
            }
        }

        fn next(&self) -> AiResponse {
            let i = self.calls.fetch_add(1, Ordering::SeqCst);
            let idx = i.min(self.responses.len() - 1);
            self.responses[idx].clone()
        }
    }

    #[async_trait]
    impl AiClient for ScriptedClient {
        async fn send_message(
            &self,
            _messages: &[crate::Message],
            _tools: &[ToolDefinition],
        ) -> Result<AiResponse, AiError> {
            Ok(self.next())
        }

        async fn send_message_streaming(
            &self,
            _messages: &[crate::Message],
            _tools: &[ToolDefinition],
            _on_chunk: Box<dyn Fn(String) + Send + Sync>,
        ) -> Result<AiResponse, AiError> {
            Ok(self.next())
        }
    }

    fn tool_call(id: &str, name: &str) -> AiResponse {
        AiResponse {
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments: serde_json::json!({ "path": "x.txt", "content": "hi" }),
            }],
            usage: TokenUsage::default(),
        }
    }

    fn final_text(text: &str) -> AiResponse {
        AiResponse {
            content: text.to_string(),
            tool_calls: Vec::new(),
            usage: TokenUsage::default(),
        }
    }

    /// Records which tool names the executor was actually asked to run.
    #[derive(Clone, Default)]
    struct ExecLog(Arc<Mutex<Vec<String>>>);

    impl ExecLog {
        fn names(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    /// Build an executor that logs every invocation and returns a benign result.
    fn logging_executor(log: ExecLog) -> ToolExecutor {
        Box::new(move |name: &str, _args: &serde_json::Value| {
            log.0.lock().unwrap().push(name.to_string());
            ToolOutcome::ok(format!("ran {name}"))
        })
    }

    /// Find the single `tool_result` block in the session history for `id`.
    fn tool_result_for<'a>(
        session: &'a Session,
        id: &str,
    ) -> Option<(&'a str, bool)> {
        for msg in session.messages() {
            for block in &msg.blocks {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } = block
                {
                    if tool_use_id == id {
                        return Some((content.as_str(), *is_error));
                    }
                }
            }
        }
        None
    }

    /// A mutating tool whose approval is DENIED executes nothing and feeds the
    /// model an is_error tool_result.
    #[tokio::test]
    async fn denied_mutating_tool_executes_nothing_and_errors() {
        let log = ExecLog::default();
        // Gate that always denies (resolves the oneshot to Deny immediately).
        let gate: ApprovalGate = Box::new(|_req: ApprovalRequest| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(ApprovalDecision::Deny);
            rx
        });

        let mut session = Session::new("test")
            .with_tools(vec![]) // defs irrelevant; executor presence drives the loop
            .with_tool_executor(logging_executor(log.clone()))
            .with_approval_gate(gate);

        let client = ScriptedClient::new(vec![
            tool_call("call-1", "write_file"),
            final_text("done"),
        ]);

        let out = session
            .chat_streaming(&client, "please write", Box::new(|_| {}))
            .await
            .unwrap();

        assert_eq!(out, "done");
        // Executor was NEVER invoked for the denied tool.
        assert!(
            log.names().is_empty(),
            "denied tool must not execute; ran: {:?}",
            log.names()
        );
        // The model received an is_error tool_result for that call.
        let (content, is_error) =
            tool_result_for(&session, "call-1").expect("missing tool_result");
        assert!(is_error, "denied tool_result must be is_error");
        assert!(
            content.contains("denied"),
            "deny message should say it was denied; got: {content}"
        );
    }

    /// An APPROVED mutating tool runs the executor exactly once.
    #[tokio::test]
    async fn approved_mutating_tool_executes() {
        let log = ExecLog::default();
        let gate: ApprovalGate = Box::new(|_req: ApprovalRequest| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(ApprovalDecision::Approve);
            rx
        });

        let mut session = Session::new("test")
            .with_tool_executor(logging_executor(log.clone()))
            .with_approval_gate(gate);

        let client = ScriptedClient::new(vec![
            tool_call("call-1", "write_file"),
            final_text("done"),
        ]);

        session
            .chat_streaming(&client, "please write", Box::new(|_| {}))
            .await
            .unwrap();

        assert_eq!(log.names(), vec!["write_file".to_string()]);
        let (_c, is_error) = tool_result_for(&session, "call-1").unwrap();
        assert!(!is_error, "approved tool_result must not be an error");
    }

    /// A read-only tool NEVER triggers the approval gate and runs directly, even
    /// when a gate is installed that would deny everything.
    #[tokio::test]
    async fn read_only_tool_bypasses_approval() {
        let log = ExecLog::default();
        let approval_calls = Arc::new(AtomicUsize::new(0));
        let ac = approval_calls.clone();
        // A gate that would DENY — if a read-only tool consulted it, the tool
        // would not run. We assert it is never even invoked.
        let gate: ApprovalGate = Box::new(move |_req: ApprovalRequest| {
            ac.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(ApprovalDecision::Deny);
            rx
        });

        let mut session = Session::new("test")
            .with_tool_executor(logging_executor(log.clone()))
            .with_approval_gate(gate);

        let mut read_call = tool_call("call-1", "read_file");
        read_call.tool_calls[0].arguments = serde_json::json!({ "path": "x.txt" });
        let client = ScriptedClient::new(vec![read_call, final_text("done")]);

        session
            .chat_streaming(&client, "read it", Box::new(|_| {}))
            .await
            .unwrap();

        // Gate untouched; read-only tool executed; result is not an error.
        assert_eq!(
            approval_calls.load(Ordering::SeqCst),
            0,
            "read-only tool must not consult the approval gate"
        );
        assert_eq!(log.names(), vec!["read_file".to_string()]);
        let (_c, is_error) = tool_result_for(&session, "call-1").unwrap();
        assert!(!is_error);
    }

    /// A pending approval that never resolves before the timeout FAILS CLOSED to
    /// Deny. Uses the injectable-timeout helper so the test is fast.
    #[tokio::test]
    async fn approval_timeout_fails_closed_to_deny() {
        // Gate returns a receiver whose sender is held forever (never resolves).
        let held: Arc<Mutex<Option<tokio::sync::oneshot::Sender<ApprovalDecision>>>> =
            Arc::new(Mutex::new(None));
        let held2 = held.clone();
        let gate: ApprovalGate = Box::new(move |_req: ApprovalRequest| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            // Stash the sender so it is NOT dropped — the only way the receiver
            // resolves is via the timeout path.
            *held2.lock().unwrap() = Some(tx);
            rx
        });

        let decision = request_approval_with_timeout(
            Some(&gate),
            "call-1",
            "run_command",
            &serde_json::json!({ "command": "ls" }),
            Duration::from_millis(50),
        )
        .await;

        assert_eq!(
            decision,
            ApprovalDecision::Deny,
            "an unanswered approval must time out to Deny (fail closed)"
        );
        // Sanity: the sender really was held (so this was a timeout, not a drop).
        assert!(held.lock().unwrap().is_some());
    }

    /// No gate installed → a mutating tool fails closed (deny) and never runs.
    #[tokio::test]
    async fn missing_gate_fails_closed() {
        let log = ExecLog::default();
        let mut session = Session::new("test")
            .with_tool_executor(logging_executor(log.clone()));
        // NOTE: no .with_approval_gate(...)

        let client = ScriptedClient::new(vec![
            tool_call("call-1", "run_command"),
            final_text("done"),
        ]);

        session
            .chat_streaming(&client, "run it", Box::new(|_| {}))
            .await
            .unwrap();

        assert!(
            log.names().is_empty(),
            "with no gate, a mutating tool must not run"
        );
        let (_c, is_error) = tool_result_for(&session, "call-1").unwrap();
        assert!(is_error, "missing gate must fail closed with is_error");
    }
}
