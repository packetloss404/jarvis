# AI Assistant

This document is the definitive reference for Jarvis's built-in AI assistant: the
multi-provider client layer, the agentic tool-call loop, and the tool sandbox /
human-approval safety model.

The assistant lives in the `jarvis-ai` crate (the provider clients, the session
tool loop, and the tools), is configured by the `[assistant]` section of
`config.toml` (`jarvis-config`), and is driven by a background async task in
`jarvis-app` that bridges the panel webview to the AI runtime.

---

## Table of Contents

1. [System Overview](#system-overview)
2. [The Interchange Model](#the-interchange-model)
3. [The `AiClient` Trait](#the-aiclient-trait)
4. [Providers](#providers)
   - [Claude](#claude)
   - [OpenAI / MiniMax (shared client)](#openai--minimax-shared-client)
   - [Google Gemini](#google-gemini)
5. [Configuration](#configuration)
6. [Switching Providers In-Panel](#switching-providers-in-panel)
7. [API Keys & Environment Variables](#api-keys--environment-variables)
8. [The Agentic Tool Loop](#the-agentic-tool-loop)
9. [The Tool Set](#the-tool-set)
10. [Tools & Safety](#tools--safety)
    - [Read-Only by Default](#read-only-by-default)
    - [The Fail-Closed Approval Gate](#the-fail-closed-approval-gate)
    - [The Sandbox Jail](#the-sandbox-jail)
    - [No-Shell Argv Execution](#no-shell-argv-execution)
    - [Output Caps & Timeouts](#output-caps--timeouts)
11. [Panel Integration](#panel-integration)
12. [Appendix: End-to-End Flow](#appendix-end-to-end-flow)

---

## System Overview

The assistant is a streaming, tool-using agent that can talk to one of four AI
providers and ground its answers in the actual project workspace through a small
set of sandboxed filesystem tools.

**Key architectural decisions:**

- **One trait, four providers.** Every provider implements the same `AiClient`
  trait. The rest of the system (the session, the tool loop, the panel) is
  provider-agnostic.
- **Normalized interchange model.** Conversation state is held in a
  provider-neutral `Message` / `ContentBlock` model. Each client translates that
  model into (and parses responses back from) its provider's wire format at the
  edge.
- **Agentic tool loop.** A `Session` runs the model in a loop: stream a response,
  execute any requested tools, feed the results back, repeat -- bounded by a hard
  round cap.
- **Read-only and approval-required by default.** The default posture exposes
  only read-only filesystem tools. Mutating tools (`write_file`, `run_command`)
  are opt-in *and* every mutating call blocks on explicit human approval that
  fails closed.
- **API keys never touch config.** Provider keys come from environment variables
  only; they are never written to `config.toml` and are redacted in `Debug`.

### Crate Layout

| Crate / module | Path | Responsibility |
|----------------|------|----------------|
| `jarvis-ai` | `jarvis-rs/crates/jarvis-ai/src/lib.rs` | `AiClient` trait + `Message`/`ContentBlock`/`AiResponse` model |
| `jarvis-ai::claude` | `.../jarvis-ai/src/claude/` | Anthropic Messages API client |
| `jarvis-ai::openai` | `.../jarvis-ai/src/openai/` | OpenAI Chat Completions client (also serves MiniMax) |
| `jarvis-ai::gemini` | `.../jarvis-ai/src/gemini/` | Google Generative Language API client |
| `jarvis-ai::router` | `.../jarvis-ai/src/router.rs` | `Provider` enum + skill-based `SkillRouter` |
| `jarvis-ai::session` | `.../jarvis-ai/src/session/` | `Session`, the tool loop (`chat.rs`), approval types |
| `jarvis-ai::tools` | `.../jarvis-ai/src/tools/` | Tool definitions, sandbox, read-only + write/exec executors |
| `jarvis-config` | `.../jarvis-config/src/schema/assistant.rs` | `[assistant]` config schema |
| `jarvis-app` | `.../jarvis-app/src/app_state/assistant*.rs` | Background task, panel IPC, approval bridge |
| Panel | `jarvis-rs/assets/panels/assistant/index.html` | The assistant webview UI |

---

## The Interchange Model

**Source:** `jarvis-rs/crates/jarvis-ai/src/lib.rs`

All conversation state is stored in a single provider-neutral model. Each client
translates this model into its provider's request format and parses the response
back into a normalized `AiResponse`.

### `Message`

```rust
pub struct Message {
    pub role: Role,                  // User | Assistant | System | Tool
    pub content: String,             // plain-text body
    pub blocks: Vec<ContentBlock>,   // structured blocks (tool turns)
}
```

A message is either a **plain-text message** (`Message::text(role, content)`,
empty `blocks`) or a **structured message** (`Message::blocks(role, blocks)`).
When `blocks` is non-empty the client serializes a content-block array; otherwise
it serializes the plain `content` string. This dual representation keeps simple
turns simple while still expressing tool turns.

### `ContentBlock`

```rust
pub enum ContentBlock {
    Text       { text: String },
    ToolUse    { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
}
```

- `ToolUse` -- the assistant's request to run a tool (carried on an `Assistant`
  turn).
- `ToolResult` -- the result of running that tool, keyed by `tool_use_id` and
  carrying an `is_error` flag (carried on a following `User` turn).

This mirrors the subset of Claude's content-block model that Jarvis uses; the
OpenAI and Gemini clients map it onto their own formats.

### `AiResponse`

What every client returns after a turn:

```rust
pub struct AiResponse {
    pub content: String,          // assistant text
    pub tool_calls: Vec<ToolCall>, // requested tool invocations
    pub usage: TokenUsage,        // input/output token counts
}

pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}
```

A `ToolDefinition` (`name`, `description`, `parameters` JSON Schema) describes a
tool to the model; converters turn it into each provider's tool format.

### `AiError`

A single error enum spans all providers: `ApiError`, `RateLimited`,
`NetworkError`, `ParseError`, `Timeout`.

---

## The `AiClient` Trait

**Source:** `jarvis-rs/crates/jarvis-ai/src/lib.rs`

```rust
#[async_trait]
pub trait AiClient: Send + Sync {
    async fn send_message(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<AiResponse, AiError>;

    async fn send_message_streaming(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        on_chunk: Box<dyn Fn(String) + Send + Sync>,
    ) -> Result<AiResponse, AiError>;
}
```

Both methods take the normalized message history and tool definitions. The
streaming variant additionally takes an `on_chunk` callback invoked for each text
delta as it arrives. Streaming is parsed from Server-Sent Events via a shared SSE
parser (`streaming.rs`); the streaming method still returns the full assembled
`AiResponse` (including any tool calls) when the stream ends.

Each provider's HTTP client is built with a 10-second connect timeout and a
120-second overall request timeout.

---

## Providers

There are **four** providers, exposed by the `Provider` enum
(`router.rs`) and selected in config by the `AiProvider` enum
(`jarvis-config`):

```rust
pub enum Provider { Claude, OpenAi, MiniMax, Gemini }
```

Crucially, **OpenAI and MiniMax share a single client.** Both speak the OpenAI
Chat Completions wire format (`/chat/completions`, `Authorization: Bearer`), so a
single parameterized `OpenAiClient` serves both -- only the default model and
base URL differ. Gemini uses Google's distinct Generative Language API wire
format and has its own client.

### Claude

**Source:** `jarvis-rs/crates/jarvis-ai/src/claude/`

- **Endpoint:** `https://api.anthropic.com/v1/messages` (`anthropic-version: 2023-06-01`).
- **Auth:** two methods. `ApiKey` sends `x-api-key`; `OAuth` sends
  `Authorization: Bearer`.
- **Default model:** `claude-sonnet-4-20250514`, `max_tokens = 4096`.
- **Tool format:** tools become `{ name, description, input_schema }`.
- **Content blocks:** serialized directly as Claude `text` / `tool_use` /
  `tool_result` blocks (a near-identity mapping). The system prompt is a separate
  top-level `system` field, not a message.
- **Streaming:** parses Anthropic SSE events (`content_block_delta` text/JSON
  deltas, `content_block_start`/`stop` for tool_use blocks, `message_start`/
  `message_delta` for token usage).

### OpenAI / MiniMax (shared client)

**Source:** `jarvis-rs/crates/jarvis-ai/src/openai/`

A single `OpenAiClient` / `OpenAiConfig` serves both providers; only `base_url`
and the default `model` differ.

| | OpenAI | MiniMax |
|---|--------|---------|
| Default base URL | `https://api.openai.com/v1` | `https://api.minimax.io/v1` |
| Default model | `gpt-4o` | `MiniMax-M2` |

- **Endpoint:** `<base_url>/chat/completions`.
- **Auth:** `Authorization: Bearer <key>`.
- **Tool format:** `{ type: "function", function: { name, description, parameters } }`.
- **Content blocks -> messages:** the system prompt becomes a leading
  `{role:"system"}` message; an assistant `ToolUse` block becomes
  `{role:"assistant", content, tool_calls:[{id, type:"function", function:{name,
  arguments}}]}` where **`arguments` is a JSON *string*** (not an object); each
  `ToolResult` block expands into a separate `{role:"tool", tool_call_id,
  content}` message.
- **Streaming:** requests `stream_options: { include_usage: true }` so the
  terminal chunk carries token usage.

### Google Gemini

**Source:** `jarvis-rs/crates/jarvis-ai/src/gemini/`

- **Endpoint:** `<base_url>/models/<model>:generateContent` (or
  `:streamGenerateContent`); default base URL
  `https://generativelanguage.googleapis.com/v1beta`, default model
  `gemini-2.0-flash`.
- **Auth:** `x-goog-api-key` header (no Bearer).
- **Tool format:** `tools: [{ functionDeclarations: [{ name, description,
  parameters }] }]`.
- **Roles:** assistant -> `"model"`, user/tool -> `"user"`. There is **no system
  role** -- the system prompt is passed as a top-level `systemInstruction`.
- **Content blocks:** an assistant `ToolUse` block becomes a `model` turn with a
  `functionCall: { name, args }` part (args is a JSON **object**, unlike OpenAI's
  string). A `ToolResult` becomes a `user` turn with a `functionResponse` part.
  Because Gemini keys responses by function **name** (not id), the client tracks a
  `tool_use_id -> name` map so each result can name the call it answers.
- **Synthesized ids:** Gemini responses omit tool-call ids, so the client
  synthesizes stable ids (`call_0`, `call_1`, ...) by position.

---

## Configuration

**Source:** `jarvis-rs/crates/jarvis-config/src/schema/assistant.rs`

The assistant is configured by the `[assistant]` section. Every field has a safe
default, so an empty section (or no section) yields the default posture: Claude,
read-only tools, approval required.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `provider` | enum | `"claude"` | Active provider: `claude`, `openai`, `minimax`, `gemini` |
| `tools_mode` | enum | `"read_only"` | `read_only` or `read_write` (opt-in to write/exec tools) |
| `require_approval` | bool | `true` | Whether mutating tool calls require explicit human approval |

Per-provider sub-tables hold model / base-URL overrides. An empty string means
"use the client default."

#### `[assistant.claude]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `model` | string | `""` | Model override (empty = client default) |

#### `[assistant.openai]` / `[assistant.minimax]` / `[assistant.gemini]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `model` | string | `""` | Model override (empty = client default) |
| `base_url` | string | `""` | API base-URL override (empty = client default) |

Note: `[assistant.claude]` has no `base_url` (the endpoint is fixed).

#### Example

```toml
[assistant]
provider = "minimax"

[assistant.openai]
model = "gpt-4o-mini"

[assistant.minimax]
model = "MiniMax-Text-01"
base_url = "https://api.minimax.io/v1"

[assistant.gemini]
model = "gemini-2.5-flash"
```

Enabling write/exec tools (opt-in; approval still required by default):

```toml
[assistant]
provider = "claude"
tools_mode = "read_write"
require_approval = true   # leave this true unless you really mean it
```

> **Safety note.** `require_approval` exists as an escape hatch, not a
> convenience. When `tools_mode = "read_only"` it is moot (no mutating tools are
> exposed at all). Even with `tools_mode = "read_write"`, the default
> `require_approval = true` means every `write_file` / `run_command` still blocks
> on a human Approve.

---

## Switching Providers In-Panel

The assistant panel header has a provider dropdown (`#provider-select`) with
options for Claude, OpenAI, MiniMax, and Gemini. Changing it sends a
`set_ai_provider` IPC message:

```javascript
sendIpc('set_ai_provider', { provider: 'gemini' });
```

In Rust (`assistant_handlers.rs`), `handle_set_ai_provider` validates the name
against the allowlist (`claude` / `openai` / `minimax` / `gemini`; case-sensitive,
unknown names rejected), then calls `set_ai_provider`
(`app_state/assistant.rs`), which:

1. Persists the selection into the in-memory config (`config.assistant.provider`).
2. Ensures the async assistant runtime is started.
3. Forwards the new provider over a channel to the background task.

The background task (`assistant_task.rs`) applies the switch **before the next
message**: it rebuilds the transport client for the new provider while
**preserving the existing conversation history** in the `Session`. The new model
name and provider label are sent back to the panel (`assistant_config` /
`assistant_provider` IPC), which updates the header and the dropdown. If the new
provider's API key is missing, the switch reports an error and the previous client
is kept.

The tool set, system prompt posture, and tool loop are identical across
providers -- only the transport client changes.

---

## API Keys & Environment Variables

**API keys are never stored in `config.toml` and never logged.** Each provider's
config redacts the key in its `Debug` impl, and keys are read from environment
variables inside the client factory only.

| Provider | Env var(s) | Notes |
|----------|-----------|-------|
| Claude | `ANTHROPIC_API_KEY` | API-key auth (`x-api-key`) |
| Claude (OAuth) | `CLAUDE_CODE_OAUTH_TOKEN` | OAuth Bearer; falls back to `~/.claude/.credentials.json` (`claudeAiOauth.accessToken`, written by `claude auth login`) |
| OpenAI | `OPENAI_API_KEY` | |
| MiniMax | `MINIMAX_API_KEY` | |
| Gemini | `GEMINI_API_KEY`, else `GOOGLE_API_KEY` | first match wins |

Claude resolution order is: `ANTHROPIC_API_KEY` -> `CLAUDE_CODE_OAUTH_TOKEN` ->
`~/.claude/.credentials.json`. If a selected provider's key is missing, the client
build fails with a clear error (e.g. *"OpenAI API not configured. Set
OPENAI_API_KEY."*) which surfaces as an assistant error in the panel rather than a
crash; the task stays alive so switching to a configured provider still works.

---

## The Agentic Tool Loop

**Source:** `jarvis-rs/crates/jarvis-ai/src/session/chat.rs`, `session/manager.rs`

A `Session` owns the conversation history, the tool definitions, the tool
executor, an optional approval gate, a token tracker, and a hard round cap
(`max_tool_rounds`, default **10**). It is built fluently:

```rust
let session = Session::new("claude")
    .with_system_prompt(prompt)
    .with_tools(tools)
    .with_tool_executor(executor)
    .with_tool_event_callback(on_event)
    .with_approval_gate(gate);   // only installed in read_write mode
```

A single `Session` processes one request at a time; a `BusyGuard` (an atomic
flag released on drop) rejects re-entrant calls with *"Session is busy"*.

### Loop Control Flow

`chat_streaming` (and the non-streaming `chat`) drives the loop:

1. Push the user message; build the message list (system prompt + history).
2. Call the client (streaming text deltas to `on_chunk`); record token usage.
3. **If the response has no tool calls** (or no executor is configured), append
   the assistant text and return -- the loop ends.
4. Otherwise increment the round counter. If it exceeds `max_tool_rounds`, log a
   warning, append the partial response, and return (runaway guard).
5. Run a **tool round** (below), then loop back to step 1 so the model can react
   to the tool results.

### A Tool Round

`run_tool_round` translates one model response into proper conversation turns:

1. **Assistant turn.** Build an `Assistant` message containing the response text
   (if any) as a `Text` block plus one `ToolUse` block per requested call, and
   push it to history.
2. **Execute each call.** For each `ToolCall`:
   - Emit a `ToolEvent::Call` (so the panel can render the activity).
   - **If the tool requires approval** (`write_file` / `run_command`), route it
     through the approval gate and *block on the human's decision*. A non-approve
     decision produces an `is_error` `ToolResult` telling the model the tool was
     not executed and must not be retried -- and the executor is never touched.
   - Otherwise run the executor on a blocking thread (`spawn_blocking`, so the
     async runtime is never stalled).
   - Emit a `ToolEvent::Result` (with a short summary and `is_error` flag) keyed
     by the originating call id.
3. **User turn.** Collect the `ToolResult` blocks into a single `User` message and
   push it. The next loop iteration re-sends everything to the model.

`tool_use` and `tool_result` blocks always carry matching ids, so the
conversation round-trips cleanly through every provider's wire format.

### Tool Events

`ToolEvent::Call { name, input }` and `ToolEvent::Result { id, name, summary,
is_error }` are surfaced through the session's `tool_event_callback`. The app
layer forwards them to the panel as `tool_call` / `tool_result` IPC messages.
Results are correlated to UI elements by **call id** (not FIFO position), because
read-only, denied, and approved results can interleave.

---

## The Tool Set

**Source:** `jarvis-rs/crates/jarvis-ai/src/tools/definitions.rs`

There are six built-in tools. Four are read-only; two are mutating/exec.

| Tool | Mode | Parameters | Description |
|------|------|------------|-------------|
| `read_file` | read-only | `path` | Read a file's contents |
| `list_directory` | read-only | `path` | List files/dirs at a path |
| `search_files` | read-only | `pattern`, `directory?` | Glob for files (e.g. `**/*.rs`) |
| `search_content` | read-only | `pattern`, `directory?`, `file_pattern?` | Regex-search file contents |
| `write_file` | **write** | `path`, `content` | Write a file (create if missing) |
| `run_command` | **exec** | `command` | Run one allowlisted program, no shell |

`read_only_tools()` returns exactly the first four; `builtin_tools()` returns all
six. The set offered to the model is chosen in lock-step with the executor (see
below): in read-only mode, `write_file` / `run_command` are not even advertised,
so the model cannot request them.

Each definition is converted to the active provider's format by `to_claude_tool`,
`to_openai_tool`, or `to_gemini_tool`.

---

## Tools & Safety

The assistant's safety model has several independent layers. **The default
posture is read-only and approval-required**, and that posture is enforced both by
what is advertised to the model and by what the executor will run.

### Read-Only by Default

**Source:** `jarvis-rs/crates/jarvis-ai/src/tools/executor.rs`,
`jarvis-rs/crates/jarvis-app/src/app_state/assistant_task.rs`

The background task picks the executor + tool definitions together based on
`tools_mode`:

- **`read_only` (default):** a `ReadOnlyToolExecutor` plus `read_only_tools()`.
  Only `read_file`, `search_files`, `search_content`, `list_directory` exist. The
  executor *explicitly rejects* `run_command` and `write_file` even if the model
  somehow names them (*"this assistant has read-only access only"*). No approval
  gate is installed because there is nothing to gate.
- **`read_write` (opt-in):** a `WriteExecToolExecutor` plus `builtin_tools()`.
  The full set is exposed and a real approval gate is installed.

The two system prompts match the posture. The read-only prompt tells the model it
has read-only access and cannot run commands or modify files; the read-write
prompt tells the model the mutating tools exist *and that every one requires
explicit human approval before it runs*.

### The Fail-Closed Approval Gate

**Source:** `jarvis-rs/crates/jarvis-ai/src/session/types.rs`,
`session/chat.rs`, `jarvis-app/src/app_state/assistant_task.rs`,
`webview_bridge/assistant_handlers.rs`

`APPROVAL_REQUIRED_TOOLS = ["write_file", "run_command"]`. Every call to a tool in
this list **must** clear an explicit human decision before the executor is
touched. Read-only tools never consult the gate.

The gate is a closure: given an `ApprovalRequest { id, tool, summary }` it returns
a `oneshot::Receiver<ApprovalDecision>`. The Session awaits that receiver under a
wall-clock timeout (`APPROVAL_TIMEOUT = 120s`). The decision is binary --
`Approve` or `Deny` -- and **`Deny` is the fail-closed default**. The call is
rejected (and nothing runs) when:

- **no gate is installed** (a missing gate can never auto-approve a dangerous
  tool),
- the human explicitly **denies**,
- the **120s timeout** elapses with no answer, or
- the **decision channel is dropped** without a response (e.g. the panel went
  away).

In every rejected case the model receives an `is_error` `ToolResult` saying the
tool was denied and must not be retried.

#### What the Human Approves

The `ApprovalRequest.summary` carries the **full, untruncated** command or file
content the human is asked to OK -- "what you approve is what runs." A model
cannot hide a payload past a display cutoff: for `run_command` the summary is the
complete command (run in the workspace root -- no model-supplied cwd is honored or
shown); for `write_file` it is the target path, byte count, and the **entire**
content. The panel's summary box is scrollable.

#### Bridge & Default Posture

When `require_approval = true` (the default), the background task installs a gate
that ships each request (plus the oneshot sender) to the main thread, which
stashes the sender keyed by `id` and shows the panel's approval card. The panel
answers with `assistant_tool_approve` / `assistant_tool_deny` (carrying the `id`),
which resolves the sender. The panel makes **Deny the focused default** and
de-emphasizes Approve; an accidental Enter or Escape denies. A client-side 115s
timeout (just under the Rust 120s gate) mirrors the fail-closed deny.

`require_approval = false` is a deliberate opt-out that installs an
*auto-approve* gate -- the only path that runs a mutating tool without a prompt,
and it requires **both** `read_write` mode **and** `require_approval = false` set
explicitly. There is no silent default that skips approval: even "no approval"
requires an explicit gate, because the absence of a gate fails closed.

### The Sandbox Jail

**Source:** `jarvis-rs/crates/jarvis-ai/src/tools/sandbox.rs`

Every filesystem access is jailed by a `ToolSandbox` rooted at the **workspace
directory** (the current working dir, canonicalized) -- never the user's home.

`validate_path` canonicalizes the target (falling back to the parent for
not-yet-existing files), then enforces two rules:

1. **Containment:** the resolved path must `starts_with` the sandbox root.
   Traversal (`../../etc/passwd`), absolute paths outside the root, and
   symlink escapes are rejected.
2. **Blocked segments:** the canonical path must not contain a sensitive segment
   -- `.ssh`, `.aws`, `.gnupg`, `.env`, `.git`. Matching is **exact, per path
   component** (not substring), so `release.env.example` is allowed but a real
   `.env` file or `.ssh` directory is blocked. Read-only listings silently skip
   blocked entries.

`validate_arg_path` applies the *same* jail to path-like **arguments** passed to
`run_command`: a leading `~` is redirected to a marker outside the sandbox (so
`~/...` is always rejected), `.`/`..` are normalized lexically (so a missing
intermediate component can't slip an escape past canonicalization), and the
deepest existing ancestor is canonicalized for symlink safety.

### No-Shell Argv Execution

**Source:** `jarvis-rs/crates/jarvis-ai/src/tools/write_exec.rs`

`run_command` **never invokes a shell.** The command string is tokenized with
`shell_words::split` (POSIX quoting only). Shell metacharacters -- `;`, `&&`,
`|`, `$(...)`, backticks, `>`, `<`, `*` -- are **never interpreted**; they survive
as inert literal characters inside argv tokens. The raw string is never handed to
`sh -c` / `cmd /c`, so injection is impossible by construction. (For example,
`git status; rm -rf .` runs `git` with literal args `status;`, `rm`, `-rf`, `.`;
no `rm` ever executes.)

Beyond no-shell, `run_command` enforces:

- **Bare-name argv0.** `argv[0]` may not contain a separator, be absolute, or be
  `..`. Path-qualified program names are refused.
- **Command allowlist.** `argv[0]` must be on the sandbox allowlist:
  `ls`, `cat`, `head`, `tail`, `wc`, `find`, `grep`, `rg`, `git`, `cargo`,
  `rustc`, `node`, `python3`, `echo`, `mkdir`, `cp`, `mv`, `rm`, `touch`.
- **BatBadBut refusal (Windows / CVE-2024-24576).** `npm`, `npx`, `yarn` are
  deliberately **not** allowlisted because on Windows they exist only as
  `.cmd`/`.bat` shims, and `Command::new` re-invokes `cmd.exe` for a batch target
  -- which would re-introduce shell metacharacter interpretation. As a backstop,
  the executor resolves argv0 against PATH (trusted dirs only, never cwd) and
  **refuses any `.cmd` / `.bat` / `.com` target outright**, so even a future
  allowlist slip fails closed.
- **Per-argument path jail.** Every path-like token in `argv[1..]` is run through
  `validate_arg_path`, so an allowlisted `cat` / `grep` / `rm` / `mv` can't read
  or destroy files outside the workspace via an argument. (This is a best-effort
  heuristic over syntactically path-like tokens; the human approval gate is the
  real backstop and sees the full literal command.)

The child is spawned via `Command::new(argv0).args(&argv[1..])` with the sandbox
root as cwd, `stdin` nulled, and stdout/stderr piped.

`write_file` similarly jails its target: it walks up to the deepest existing
ancestor, validates *that* through the sandbox, re-attaches the missing tail
lexically (rejecting any `..`), creates the jailed parent dirs, then
re-validates the full target before writing. It refuses to clobber a directory.

### Output Caps & Timeouts

To protect the model context (and prevent DoS via huge files or runaway
processes):

| Limit | Value | Applies to |
|-------|-------|-----------|
| `MAX_TOOL_OUTPUT` | 12,000 chars | Every tool's output (truncated with a notice) |
| `MAX_WRITE_BYTES` | 1,000,000 bytes | `write_file` content (oversize rejected) |
| `RUN_COMMAND_TIMEOUT` | 30 s | `run_command` wall-clock (child killed on expiry) |

`run_command` drains stdout/stderr on dedicated threads (so a child filling a pipe
buffer can't deadlock the timeout poll), polls for exit until the deadline, kills
the child if it overruns, and returns the combined output prefixed with the exit
code. Search tools cap per-line length and stop appending once the output cap is
reached.

---

## Panel Integration

**Source:** `jarvis-rs/assets/panels/assistant/index.html`,
`jarvis-rs/crates/jarvis-app/src/app_state/assistant.rs`,
`assistant_task.rs`

The assistant panel is a webview (`jarvis://localhost/assistant/index.html`)
wired to the background task through IPC.

### Lifecycle

1. The panel loads, registers IPC handlers, and sends `assistant_ready`.
2. Rust lazily starts the async runtime (`ensure_assistant_runtime`): it spawns a
   single-worker Tokio runtime running `assistant_task`, and creates the
   user-input, event, and provider-switch channels.
3. The task builds the client for the configured provider and replies with
   `assistant_config` (model name) and `assistant_provider` (active provider).

### Messages

| Direction | IPC kind | Payload | Purpose |
|-----------|----------|---------|---------|
| JS -> Rust | `assistant_input` | `{ text }` | User message (capped at 4096 chars) |
| JS -> Rust | `assistant_ready` | -- | Panel loaded; start the runtime |
| JS -> Rust | `set_ai_provider` | `{ provider }` | Switch provider |
| JS -> Rust | `assistant_tool_approve` | `{ id }` | Approve a pending mutating tool |
| JS -> Rust | `assistant_tool_deny` | `{ id }` | Deny a pending mutating tool |
| Rust -> JS | `assistant_chunk` | `{ text }` | Streaming text delta |
| Rust -> JS | `assistant_output` | `{ text }` | Final full response |
| Rust -> JS | `assistant_error` | `{ message }` | Error |
| Rust -> JS | `assistant_config` | `{ model_name }` | Active model name (header) |
| Rust -> JS | `assistant_provider` | `{ provider }` | Active provider (dropdown) |
| Rust -> JS | `tool_call` | `{ name, input }` | A tool call started |
| Rust -> JS | `tool_result` | `{ id, name, summary, is_error }` | A tool call finished |
| Rust -> JS | `tool_approval_request` | `{ id, tool, summary }` | Approval needed |

All of these `kind`s are on the IPC allowlist (see the *WebView System & IPC
Bridge* chapter).

### Rendering

- **Streaming text** accumulates into a single assistant bubble as `assistant_chunk`
  events arrive; `assistant_output` finalizes it.
- **Read-only tool activity** renders a collapsible `🔧 tool(arg)` line that fills
  in with the result summary; an error result is styled distinctly.
- **Approval cards** render a prominent warning card with the tool name and the
  full summary. The summary and tool name are model-controlled text and are always
  rendered via `textContent` (never `innerHTML`), so embedded markup is inert.
  Deny is focused by default; nothing is ever auto-approved client-side.

### Pending-Approval Bookkeeping

The main thread keeps a map of pending approval senders keyed by request id. On
each poll it prunes entries whose receiver was dropped (the gate already resolved
on its own -- timeout, session end, or task death), which is pure cleanup and can
never turn a would-be approve into a deny. When the human answers,
`resolve_tool_approval` pops the sender and delivers the decision; an unknown or
already-resolved id is harmless (the gate already failed closed).

---

## Appendix: End-to-End Flow

### A read-only turn

```
User types in panel
    |
    v  assistant_input { text }
handle_assistant_input -> assistant_tx channel
    |
    v
assistant_task: session.chat_streaming(client, text, on_chunk)
    |
    +-- client.send_message_streaming(...)  -> SSE text deltas
    |        |  on_chunk -> StreamChunk events -> assistant_chunk IPC
    |        v
    |   AiResponse { tool_calls: [read_file ...] }
    |
    +-- run_tool_round:
    |     emit ToolEvent::Call  -> tool_call IPC (panel renders 🔧)
    |     ReadOnlyToolExecutor (sandbox-jailed, output-capped)
    |     emit ToolEvent::Result -> tool_result IPC
    |     append tool_result block; loop again
    |
    v  (no more tool calls)
AiResponse text -> Done -> assistant_output IPC
```

### A mutating turn (read_write mode)

```
AiResponse { tool_calls: [run_command ...] }
    |
    v  tool requires approval
request_approval(gate, id, "run_command", args)
    |
    v  ApprovalRequest { id, tool, summary=<full command> }
ToolApprovalRequest event -> main thread stashes sender by id
    |                          -> tool_approval_request IPC (panel shows card)
    |
    +-- human clicks Approve  -> assistant_tool_approve { id }
    |        -> responder.send(Approve)
    |        -> WriteExecToolExecutor.run_command (no shell, allowlist,
    |           argv jail, 30s timeout, output cap)
    |        -> tool_result IPC (card: "approved · done")
    |
    +-- human clicks Deny / 120s timeout / channel dropped
             -> ApprovalDecision::Deny (fail closed)
             -> is_error tool_result: "User denied... NOT executed"
             -> executor never touched
```
