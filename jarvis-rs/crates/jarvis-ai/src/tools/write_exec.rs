//! Write + exec tool executor (A2) — SCAFFOLD.
//!
//! Owns the two mutating/exec tools that A1 deliberately excluded:
//! `write_file` and `run_command`. Unlike [`ReadOnlyToolExecutor`], every tool
//! here can change the system, so it is gated by THREE non-negotiable layers:
//!
//! 1. **Approval** — the Session tool loop blocks each of these calls on an
//!    explicit human decision (see `Session::with_approval_gate`) BEFORE this
//!    executor is ever invoked. This type runs only AFTER approval; it does not
//!    itself perform the prompt, but it is the thing that approval guards.
//! 2. **Path jail** — `write_file` resolves its target through
//!    [`ToolSandbox::validate_path`] (sandbox root, never home; blocked
//!    segments rejected) before any write.
//! 3. **Argv exec + allowlist + limits** — `run_command` NEVER touches a shell.
//!    Downstream it MUST parse the command into argv, validate `argv[0]` against
//!    [`ToolSandbox::validate_command`], and exec via
//!    `std::process::Command::new(argv0).args(&argv[1..])` with the sandbox root
//!    as cwd, a wall-clock timeout, and an output cap. NO `sh -c` / `cmd /c` /
//!    `shell:true` — literal args can never inject.
//!
//! This is the FOUNDATION scaffold: the public API is fixed so downstream
//! agents fill in the bodies. Both tools currently return a "not yet
//! implemented" error so the executor is wired but inert and the workspace
//! builds green. The read-only tools are delegated to [`ReadOnlyToolExecutor`]
//! so a single executor can serve the whole tool set when write/exec is enabled.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::executor::{ReadOnlyToolExecutor, MAX_TOOL_OUTPUT, READ_ONLY_TOOLS};
use super::sandbox::ToolSandbox;

/// Default wall-clock timeout for a single `run_command` invocation. The child
/// is killed and an error is returned on expiry.
pub const RUN_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum number of bytes `write_file` will accept in `content`. Larger writes
/// are rejected outright (the assistant should not be dumping megabytes to disk).
pub const MAX_WRITE_BYTES: usize = 1_000_000;

/// How often the timeout loop polls the child process for exit while waiting on
/// the wall-clock deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The mutating/exec tool names this executor owns (in addition to the
/// read-only set it delegates).
pub const WRITE_EXEC_TOOLS: &[&str] = &["write_file", "run_command"];

/// Executes the full tool set (read-only + write/exec) inside a [`ToolSandbox`].
///
/// Construct this ONLY when the config opts into write/exec
/// (`AssistantConfig::tools_mode == ReadWrite`). The default assistant keeps
/// using [`ReadOnlyToolExecutor`], so this type is never even constructed in the
/// default (read-only) configuration.
///
/// NOTE: this executor performs NO approval itself — approval is enforced one
/// layer up by the Session tool loop's approval gate, which blocks before this
/// `execute` is called for any approval-required tool.
pub struct WriteExecToolExecutor {
    sandbox: ToolSandbox,
    /// Sandbox root; used to resolve relative paths and as the exec cwd.
    root: PathBuf,
    /// Delegate for the read-only tools, so the combined executor can serve the
    /// entire tool set from one object.
    read_only: ReadOnlyToolExecutor,
    /// Per-command wall-clock timeout for `run_command`.
    command_timeout: Duration,
}

impl WriteExecToolExecutor {
    /// Create an executor rooted at `root`. `root` should be a canonicalized
    /// absolute path (the workspace dir, never the user's home).
    pub fn new(root: PathBuf) -> Self {
        Self {
            sandbox: ToolSandbox::new(root.clone()),
            read_only: ReadOnlyToolExecutor::new(root.clone()),
            root,
            command_timeout: RUN_COMMAND_TIMEOUT,
        }
    }

    /// Override the per-command timeout (builder style).
    pub fn with_command_timeout(mut self, timeout: Duration) -> Self {
        self.command_timeout = timeout;
        self
    }

    /// All tool names this executor can handle (read-only + write/exec).
    pub fn tool_names() -> Vec<&'static str> {
        READ_ONLY_TOOLS
            .iter()
            .copied()
            .chain(WRITE_EXEC_TOOLS.iter().copied())
            .collect()
    }

    /// Execute a tool call by name.
    ///
    /// Read-only tools are delegated to the inner [`ReadOnlyToolExecutor`].
    /// `write_file` and `run_command` are owned here.
    ///
    /// SAFETY CONTRACT (must hold once implemented): the caller has ALREADY
    /// obtained human approval for any tool in [`WRITE_EXEC_TOOLS`] before
    /// calling this. This method does not re-check approval.
    pub fn execute(&self, name: &str, args: &serde_json::Value) -> Result<String, String> {
        match name {
            "write_file" => self.write_file(args),
            "run_command" => self.run_command(args),
            // Everything else is read-only; delegate.
            other if READ_ONLY_TOOLS.contains(&other) => self.read_only.execute(other, args),
            other => Err(format!("Unknown tool: {other}")),
        }
    }

    /// Write `content` to `path`, jailed through [`ToolSandbox::validate_path`].
    ///
    /// Safety: the target is resolved against the sandbox root and validated by
    /// the sandbox (which rejects escapes, home, and blocked segments such as
    /// `.env` / `.ssh`) BEFORE any disk I/O. Content size is capped at
    /// [`MAX_WRITE_BYTES`]. This runs only AFTER the Session approval gate has
    /// returned `Approve` for the call.
    fn write_file(&self, args: &serde_json::Value) -> Result<String, String> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| "write_file: missing 'path' argument".to_string())?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| "write_file: missing 'content' argument".to_string())?;

        if content.len() > MAX_WRITE_BYTES {
            return Err(format!(
                "write_file: content is {} bytes, exceeds the {MAX_WRITE_BYTES}-byte limit",
                content.len()
            ));
        }

        // Resolve to an absolute target lexically rooted in the sandbox.
        let raw = Path::new(path);
        let abs = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.root.join(raw)
        };

        // STEP 1: jail the *parent* before creating anything. We walk up to the
        // deepest ancestor that already exists, validate IT through the sandbox
        // (so symlink/escape/blocked-segment checks run against a real, resolvable
        // path), then re-attach the missing tail lexically. This lets us create
        // nested intermediate dirs that don't exist yet while never escaping.
        let parent = abs
            .parent()
            .ok_or_else(|| "write_file: target has no parent directory".to_string())?;
        let mut existing = parent;
        let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
        loop {
            if existing.exists() {
                break;
            }
            match (existing.file_name(), existing.parent()) {
                (Some(name), Some(up)) => {
                    tail.push(name);
                    existing = up;
                }
                // Ran out of ancestors without finding an existing one.
                _ => return Err(format!("write_file: cannot resolve any ancestor of '{path}'")),
            }
        }
        // Validate the deepest existing ancestor lives in the jail.
        let canon_existing = self.sandbox.validate_path(existing)?;
        // Re-attach the missing components (in original order) to the validated,
        // canonical ancestor. The result is guaranteed under the sandbox root.
        let mut target_parent = canon_existing;
        for comp in tail.iter().rev() {
            // Reject any traversal/separator smuggled into a single component.
            if *comp == std::ffi::OsStr::new("..") {
                return Err(format!("write_file: '{path}' contains a parent traversal"));
            }
            target_parent.push(comp);
        }
        // The reconstructed parent must still be inside the jail.
        if !target_parent.starts_with(&self.root) {
            return Err(format!(
                "write_file: path '{path}' resolves outside the sandbox"
            ));
        }

        let file_name = abs
            .file_name()
            .ok_or_else(|| format!("write_file: '{path}' has no file name"))?;
        let final_target = target_parent.join(file_name);

        // Refuse to clobber a directory with a file write.
        if let Ok(meta) = std::fs::metadata(&final_target) {
            if meta.is_dir() {
                return Err(format!("write_file: '{path}' is a directory"));
            }
        }

        // STEP 2: create the (now jailed) parent dirs.
        std::fs::create_dir_all(&target_parent)
            .map_err(|e| format!("write_file: cannot create parent dirs for '{path}': {e}"))?;

        // STEP 3: re-validate the full target now that its parent exists — this
        // is the authoritative jail check (catches blocked segments in the leaf
        // and any symlink that may have appeared) — then write.
        let canonical = self.sandbox.validate_path(&final_target)?;

        std::fs::write(&canonical, content.as_bytes())
            .map_err(|e| format!("write_file: cannot write '{path}': {e}"))?;

        let rel = canonical
            .strip_prefix(&self.root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| canonical.to_string_lossy().to_string());
        Ok(format!("Wrote {} bytes to {rel}", content.len()))
    }

    /// Execute a command WITHOUT a shell.
    ///
    /// Safety contract (all enforced here):
    /// 1. The command string is tokenized into argv with [`shell_words::split`],
    ///    which applies POSIX quoting rules but NEVER interprets shell
    ///    metacharacters (`;`, `&&`, `|`, `$(...)`, backticks, redirects) as
    ///    operators — they become inert literal characters inside tokens. The
    ///    raw string is NEVER handed to `sh -c` / `cmd /c`, so injection is
    ///    impossible by construction.
    /// 2. `argv[0]` must be a bare program name (no `/` or `\\`, not absolute,
    ///    not `..`) and must be on the sandbox allowlist
    ///    ([`ToolSandbox::validate_command`]). On Windows it is additionally
    ///    refused if it resolves to a `.cmd`/`.bat`/`.com` batch shim, which
    ///    would re-enter cmd.exe (BatBadBut / CVE-2024-24576).
    /// 2b. Every path-like token in `argv[1..]` is jailed through
    ///    [`ToolSandbox::validate_arg_path`] (same containment + blocked-segment
    ///    rules as `write_file`), so an allowlisted `cat`/`grep`/`rm`/`mv`
    ///    cannot read or destroy files outside the workspace via an argument.
    /// 3. The child is spawned via
    ///    `Command::new(argv0).args(&argv[1..]).current_dir(self.root)` with no
    ///    inherited stdin and piped stdout/stderr.
    /// 4. A wall-clock timeout (`self.command_timeout`) kills the child on expiry.
    /// 5. Combined stdout+stderr is capped at [`MAX_TOOL_OUTPUT`].
    ///
    /// Runs only AFTER the Session approval gate returns `Approve`.
    fn run_command(&self, args: &serde_json::Value) -> Result<String, String> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| "run_command: missing 'command' argument".to_string())?;

        // Tokenize WITHOUT a shell. Metacharacters survive as literals.
        let argv = shell_words::split(command)
            .map_err(|e| format!("run_command: cannot parse command: {e}"))?;

        let argv0 = match argv.first() {
            Some(a) if !a.is_empty() => a.as_str(),
            _ => return Err("run_command: empty command".to_string()),
        };

        // Reject anything that is not a bare program name. A path-qualified
        // argv0 (absolute, or containing a separator, or `..`) could reach a
        // binary outside the allowlist's intent, so it is refused outright. The
        // allowlist itself is matched on the bare name.
        if argv0.contains('/') || argv0.contains('\\') || argv0 == ".." || Path::new(argv0).is_absolute() {
            return Err(format!(
                "run_command: '{argv0}' must be a bare program name (no path components)"
            ));
        }

        // Allowlist check on the bare program name.
        self.sandbox.validate_command(argv0)?;

        // BatBadBut / CVE-2024-24576 (Windows): `Command::new` on a `.cmd`/`.bat`
        // target re-invokes cmd.exe, which re-interprets shell metacharacters in
        // the (otherwise literal) args and defeats the no-shell guarantee. Resolve
        // argv0 to a concrete executable and REFUSE any batch/com shim. The
        // allowlist already excludes the usual `.cmd`-only tools (npm/npx/yarn);
        // this is the executor-level backstop so even a future allowlist slip
        // fails closed.
        if let Some(resolved) = resolve_executable(argv0) {
            if is_batch_shim(&resolved) {
                return Err(format!(
                    "run_command: '{argv0}' resolves to a batch shim ({}) which would re-enter \
                     a shell; refused.",
                    resolved.display()
                ));
            }
        }

        // PATH-JAIL FOR EXEC ARGUMENTS. argv0 is allowlisted, but its arguments
        // are otherwise unrestricted — so an allowlisted `cat`/`grep`/`rm`/`mv`
        // could read or destroy files OUTSIDE the workspace via a path arg. For
        // every token in argv[1..] that LOOKS path-like, require it to stay
        // inside the sandbox jail (same logic as write_file's validate_path),
        // rejecting the whole command on any escape or blocked segment.
        //
        // HEURISTIC LIMITS (the approval gate is the real backstop): this only
        // inspects tokens that syntactically look like paths. It cannot catch a
        // path supplied via stdin, an `@responsefile`, an obscure flag that
        // glues a value (`--file=/etc/passwd` IS caught since the token contains
        // `/`, but `-f/etc/passwd` style single-token flags are best-effort), or
        // a path assembled by the program itself. Also note `shell_words::split`
        // applies POSIX escaping, so a Windows backslash path (`C:\dir`) is
        // mangled at tokenization (backslashes are consumed) — such a token then
        // also reaches the program mangled and cannot reliably name the intended
        // location anyway. It is defense-in-depth, not a complete capability
        // boundary; the human approval gate sees the full literal command.
        for token in &argv[1..] {
            if looks_path_like(token) {
                self.sandbox.validate_arg_path(token)?;
            }
        }

        // Spawn WITHOUT any shell. Literal args cannot inject.
        let mut child = Command::new(argv0)
            .args(&argv[1..])
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("run_command: failed to spawn '{argv0}': {e}"))?;

        // Drain stdout+stderr on threads so a child that fills a pipe buffer
        // cannot deadlock us while we poll for the timeout.
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>();
        let err_tx = out_tx.clone();
        let out_handle = stdout.map(|mut s| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = s.read_to_end(&mut buf);
                let _ = out_tx.send(buf);
            })
        });
        let err_handle = stderr.map(|mut s| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = s.read_to_end(&mut buf);
                let _ = err_tx.send(buf);
            })
        });

        // Poll for exit until the wall-clock deadline; kill on expiry.
        let deadline = Instant::now() + self.command_timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(format!(
                            "run_command: '{argv0}' timed out after {}s and was killed",
                            self.command_timeout.as_secs()
                        ));
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(e) => {
                    let _ = child.kill();
                    return Err(format!("run_command: error waiting on '{argv0}': {e}"));
                }
            }
        };

        // Join the drain threads to collect captured output.
        let mut combined = Vec::new();
        if let Some(h) = out_handle {
            let _ = h.join();
        }
        if let Some(h) = err_handle {
            let _ = h.join();
        }
        while let Ok(chunk) = out_rx.try_recv() {
            combined.extend_from_slice(&chunk);
        }

        let output_text = String::from_utf8_lossy(&combined);
        let capped = cap_output(&output_text);

        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".to_string());
        Ok(format!("exit code: {code}\n{capped}"))
    }
}

/// Heuristic: does this argv token look like a filesystem path we must jail?
///
/// We jail a token when it is absolute, starts with `~` (home), contains a
/// path separator (`/` or `\`), or contains a `..` traversal. A token that is
/// just a bare word (e.g. a subcommand, a flag like `-rf`, a pattern) is left
/// alone — it cannot, by itself, name a location outside the workspace. The
/// path-jail in `validate_arg_path` then enforces containment. This is
/// best-effort: see the limits documented at the call site.
fn looks_path_like(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    token.starts_with('~')
        || token.contains('/')
        || token.contains('\\')
        || token.contains("..")
        || Path::new(token).is_absolute()
}

/// Resolve a bare argv0 to a concrete executable path so we can inspect its
/// extension. On Windows this consults `PATHEXT`-style suffixes against PATH;
/// on unix it returns the first executable match on PATH. Returns `None` if no
/// match is found (the spawn itself will then surface a "failed to spawn").
///
/// NOTE: we intentionally search trusted PATH dirs only — never the current
/// working directory — to avoid cwd-in-PATH argv0 shadowing (a workspace file
/// named like an allowlisted tool must not be picked up). On Windows, the cwd
/// is normally searched first by the loader; resolving here from PATH only
/// documents and mitigates that risk for the shim check.
fn resolve_executable(argv0: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(|e| e.to_string())
            .collect()
    } else {
        Vec::new()
    };

    for dir in std::env::split_paths(&path_var) {
        // Direct hit (argv0 already carries an extension, or unix).
        let direct = dir.join(argv0);
        if direct.is_file() {
            return Some(direct);
        }
        // Windows: try each PATHEXT suffix in priority order.
        for ext in &exts {
            let candidate = dir.join(format!("{argv0}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// True if `path` ends in a Windows batch/com shim extension (`.cmd`, `.bat`,
/// `.com`) — case-insensitive. Such targets re-enter cmd.exe under
/// `Command::new` (BatBadBut / CVE-2024-24576) and must be refused.
fn is_batch_shim(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => {
            let e = ext.to_ascii_lowercase();
            e == "cmd" || e == "bat" || e == "com"
        }
        None => false,
    }
}

/// Truncate `s` to [`MAX_TOOL_OUTPUT`] characters, appending a notice when cut.
fn cap_output(s: &str) -> String {
    if s.len() <= MAX_TOOL_OUTPUT {
        return s.to_string();
    }
    let mut end = MAX_TOOL_OUTPUT;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = s[..end].to_string();
    out.push_str("\n\n[output truncated: exceeded 12000 characters]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup() -> (WriteExecToolExecutor, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "jarvis_wexec_test_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        let canonical = fs::canonicalize(&dir).unwrap();
        (WriteExecToolExecutor::new(canonical.clone()), canonical)
    }

    // The command used for cross-platform exec tests. `echo` is on the allowlist
    // and exists on both Windows (as a cmd builtin shim is NOT used — but the
    // standalone is absent), so we standardize on a binary present everywhere.
    // `cargo` is guaranteed in this build environment and is on the allowlist.
    fn exec_probe() -> &'static str {
        if cfg!(windows) {
            // `where` is not on the allowlist; use `cargo --version` which is
            // allowlisted and present in the toolchain.
            "cargo"
        } else {
            "echo"
        }
    }

    #[test]
    fn write_exec_tool_names_includes_both_sets() {
        let names = WriteExecToolExecutor::tool_names();
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"run_command"));
    }

    // ---- write_file --------------------------------------------------------

    #[test]
    fn write_file_happy_path() {
        let (exec, dir) = setup();
        let out = exec
            .execute(
                "write_file",
                &serde_json::json!({ "path": "out.txt", "content": "hello" }),
            )
            .unwrap();
        assert!(out.contains("5 bytes"), "got: {out}");
        let written = fs::read_to_string(dir.join("out.txt")).unwrap();
        assert_eq!(written, "hello");
    }

    #[test]
    fn write_file_creates_parent_dirs() {
        let (exec, dir) = setup();
        exec.execute(
            "write_file",
            &serde_json::json!({ "path": "a/b/c.txt", "content": "x" }),
        )
        .unwrap();
        assert!(dir.join("a/b/c.txt").exists());
    }

    #[test]
    fn write_file_path_jail_traversal_rejected() {
        let (exec, _dir) = setup();
        let res = exec.execute(
            "write_file",
            &serde_json::json!({ "path": "../escape.txt", "content": "pwn" }),
        );
        assert!(res.is_err(), "path traversal escape must be rejected");
    }

    #[test]
    fn write_file_absolute_outside_jail_rejected() {
        let (exec, _dir) = setup();
        let target = if cfg!(windows) {
            "C:/Windows/Temp/jarvis_escape.txt"
        } else {
            "/tmp/jarvis_escape.txt"
        };
        let res = exec.execute(
            "write_file",
            &serde_json::json!({ "path": target, "content": "pwn" }),
        );
        assert!(res.is_err(), "absolute path outside jail must be rejected");
    }

    #[test]
    fn write_file_blocked_segment_rejected() {
        let (exec, _dir) = setup();
        let res = exec.execute(
            "write_file",
            &serde_json::json!({ "path": ".env", "content": "SECRET=1" }),
        );
        assert!(res.is_err(), "blocked segment (.env) must be rejected");
    }

    #[test]
    fn write_file_oversize_rejected() {
        let (exec, _dir) = setup();
        let big = "x".repeat(MAX_WRITE_BYTES + 1);
        let res = exec.execute(
            "write_file",
            &serde_json::json!({ "path": "big.txt", "content": big }),
        );
        assert!(res.is_err(), "oversize content must be rejected");
        assert!(res.unwrap_err().contains("limit"));
    }

    // ---- run_command: injection is inert -----------------------------------

    #[test]
    fn run_command_semicolon_does_not_chain() {
        // `git status; rm -rf ~` must NOT run a shell. It tokenizes to
        // ["git", "status;", "rm", "-rf", "~"] — argv0 is `git` (allowlisted),
        // and `git` receives the literal inert args (git will just error on the
        // bogus subcommand). Crucially, no `rm` ever runs.
        let (exec, dir) = setup();
        // create a sentinel file that a real `rm -rf` would be tempted to remove
        fs::write(dir.join("sentinel.txt"), "keep").unwrap();
        let res = exec.execute(
            "run_command",
            &serde_json::json!({ "command": "git status; rm -rf ." }),
        );
        // git runs (allowlisted) but the chain never executes as a shell.
        assert!(res.is_ok(), "git argv0 is allowlisted; got: {res:?}");
        // The sentinel must still exist — no shell, no rm.
        assert!(dir.join("sentinel.txt").exists(), "no shell chaining occurred");
    }

    #[test]
    fn run_command_and_operator_is_literal() {
        // `x && y` tokenizes to ["x", "&&", "y"]; argv0 `x` is not allowlisted.
        let (exec, _dir) = setup();
        let res = exec.execute(
            "run_command",
            &serde_json::json!({ "command": "x && y" }),
        );
        assert!(res.is_err(), "non-allowlisted argv0 must be rejected");
        assert!(res.unwrap_err().contains("not allowed"));
    }

    #[test]
    fn run_command_subshell_is_literal_argv() {
        // `echo $(whoami)` / cargo equivalent: the `$(...)` is a literal token,
        // never command-substituted. We assert no substitution by checking the
        // literal survives. Use git so argv0 is allowlisted on all platforms.
        let (exec, _dir) = setup();
        let res = exec
            .execute(
                "run_command",
                &serde_json::json!({ "command": "git $(whoami)" }),
            )
            .unwrap();
        // git received the literal "$(whoami)" as a bad subcommand; output names
        // it back verbatim, proving no substitution happened.
        assert!(
            res.contains("$(whoami)") || res.contains("whoami"),
            "subshell token must be passed literally; got: {res}"
        );
    }

    #[test]
    fn run_command_pipe_is_literal() {
        // A pipe (`|`) must NEVER be interpreted as a shell operator: it
        // tokenizes to an inert literal arg handed to argv0, and `sh` is never
        // spawned. We assert this with an allowlisted argv0 that is GUARANTEED
        // present in this toolchain (`cargo` on every platform, `grep` on unix)
        // so the test is portable: the binary's absence is not what we test.
        let (exec, _dir) = setup();
        // argv0 = cargo (allowlisted + always present); the `| sh` tokens become
        // literal inert args to cargo. No shell, no pipe, `sh` never runs.
        let res = exec
            .execute(
                "run_command",
                &serde_json::json!({ "command": "cargo | sh" }),
            )
            .expect("cargo argv0 is allowlisted and present; pipe must be a literal arg");
        // cargo received the literal "|" / "sh" tokens as bad args and reports
        // them back; crucially execution completed WITHOUT a shell chaining to sh.
        assert!(res.contains("exit code:"), "must run argv0 directly; got: {res}");

        // And confirm a non-allowlisted argv0 in a "piped" command is still
        // rejected on the allowlist (the `|` never rescues it via a shell).
        let res = exec.execute(
            "run_command",
            &serde_json::json!({ "command": "grep foo | sh" }),
        );
        match res {
            // unix: grep present + allowlisted -> runs, no shell, no pipe.
            Ok(out) => assert!(out.contains("exit code:"), "got: {out}"),
            // windows: grep absent -> spawn error is acceptable; the ONLY thing
            // that must never happen is a shell interpreting the pipe.
            Err(e) => assert!(
                e.contains("failed to spawn"),
                "the pipe must never be shell-interpreted; got: {e}"
            ),
        }
    }

    // ---- run_command: argv path-jail (#2) ----------------------------------

    #[test]
    fn run_command_arg_path_traversal_rejected() {
        // `cat ../../etc/passwd` must be refused BEFORE any spawn: the path arg
        // escapes the sandbox jail. The safety property is rejection, not a
        // particular error string.
        let (exec, _dir) = setup();
        let res = exec.execute(
            "run_command",
            &serde_json::json!({ "command": "cat ../../etc/passwd" }),
        );
        assert!(res.is_err(), "out-of-jail path arg must be rejected");
        assert!(
            res.unwrap_err().contains("outside the sandbox"),
            "must be a jail rejection, never a shell interpretation"
        );
    }

    #[test]
    fn run_command_arg_tilde_home_rejected() {
        let (exec, _dir) = setup();
        let res = exec.execute(
            "run_command",
            &serde_json::json!({ "command": "cat ~/.ssh/id_rsa" }),
        );
        assert!(res.is_err(), "~/.ssh path arg must be rejected");
    }

    #[test]
    fn run_command_arg_absolute_outside_rejected() {
        let (exec, _dir) = setup();
        // NOTE: `shell_words::split` applies POSIX escaping, so a backslash path
        // (`C:\Windows`) would be mangled at tokenization. We use a forward-slash
        // absolute path, which survives tokenization on both platforms and is the
        // form the path-jail must reject.
        let outside = if cfg!(windows) {
            "rm C:/Windows/system32/drivers/etc/hosts"
        } else {
            "rm /outside"
        };
        let res = exec.execute("run_command", &serde_json::json!({ "command": outside }));
        assert!(res.is_err(), "absolute out-of-jail path arg must be rejected");
    }

    #[test]
    fn run_command_arg_blocked_segment_rejected() {
        let (exec, _dir) = setup();
        // In-sandbox-looking but hits a blocked segment (.env).
        let res = exec.execute(
            "run_command",
            &serde_json::json!({ "command": "cat ./.env" }),
        );
        assert!(res.is_err(), "blocked .env segment in arg must be rejected");
    }

    #[test]
    fn run_command_in_sandbox_path_allowed() {
        // `cat ./file.txt` against an in-sandbox file must pass the jail and run.
        let (exec, dir) = setup();
        fs::write(dir.join("file.txt"), "hi").unwrap();
        let cmd = if cfg!(windows) {
            // `cat` may be absent on Windows; use an allowlisted+present probe
            // that still carries an in-jail path arg so the jail logic runs.
            "cargo locate-project --manifest-path ./file.txt"
        } else {
            "cat ./file.txt"
        };
        let res = exec.execute("run_command", &serde_json::json!({ "command": cmd }));
        match res {
            // Ran (jail passed). For unix `cat` the body echoes the content.
            Ok(out) => assert!(out.contains("exit code:"), "got: {out}"),
            // A missing binary is a spawn error — acceptable. What must NOT
            // happen is a jail rejection of an in-sandbox path.
            Err(e) => assert!(
                e.contains("failed to spawn"),
                "in-sandbox path must not be jail-rejected; got: {e}"
            ),
        }
    }

    // ---- run_command: Windows batch-shim rejection (#1) --------------------

    #[cfg(windows)]
    #[test]
    fn run_command_rejects_cmd_shim_with_metachar_args() {
        use std::path::PathBuf;
        // Create a fake allowlisted tool that exists ONLY as a `.cmd` shim on a
        // PATH dir we control, then confirm run_command refuses it rather than
        // letting cmd.exe shell-interpret the metacharacter args.
        let dir = std::env::temp_dir().join(format!(
            "jarvis_batshim_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        // `echo` is allowlisted. Provide it only as echo.cmd.
        let shim = dir.join("echo.cmd");
        fs::write(&shim, "@echo off\r\necho pwned\r\n").unwrap();

        // Prepend our dir to PATH for this process.
        let old_path = std::env::var_os("PATH").unwrap_or_default();
        let mut parts: Vec<PathBuf> = vec![dir.clone()];
        parts.extend(std::env::split_paths(&old_path));
        let new_path = std::env::join_paths(parts).unwrap();
        std::env::set_var("PATH", &new_path);

        let root = fs::canonicalize(&dir).unwrap();
        let exec = WriteExecToolExecutor::new(root);
        // Metacharacter args that a shell WOULD interpret. They must stay inert,
        // and crucially the .cmd shim must be refused outright.
        let res = exec.execute(
            "run_command",
            &serde_json::json!({ "command": "echo a & calc.exe" }),
        );

        // Restore PATH before asserting.
        std::env::set_var("PATH", &old_path);

        assert!(res.is_err(), "a .cmd shim argv0 must be refused");
        let e = res.unwrap_err();
        assert!(
            e.contains("batch shim"),
            "must be a batch-shim refusal (no shell re-entry); got: {e}"
        );
    }

    // ---- run_command: validation -------------------------------------------

    #[test]
    fn run_command_empty_rejected() {
        let (exec, _dir) = setup();
        let res = exec.execute("run_command", &serde_json::json!({ "command": "   " }));
        assert!(res.is_err(), "empty command must be rejected");
    }

    #[test]
    fn run_command_path_qualified_argv0_rejected() {
        let (exec, _dir) = setup();
        for cmd in ["/bin/ls", "../ls", "./ls", "sub/ls"] {
            let res = exec.execute("run_command", &serde_json::json!({ "command": cmd }));
            assert!(res.is_err(), "path-qualified argv0 '{cmd}' must be rejected");
        }
    }

    #[test]
    fn run_command_non_allowlisted_rejected() {
        let (exec, _dir) = setup();
        let res = exec.execute(
            "run_command",
            &serde_json::json!({ "command": "curl http://evil" }),
        );
        assert!(res.is_err(), "curl is not allowlisted");
        assert!(res.unwrap_err().contains("not allowed"));
    }

    // ---- run_command: behavior ---------------------------------------------

    #[test]
    fn run_command_happy_path_reports_exit_code() {
        let (exec, _dir) = setup();
        let res = exec
            .execute(
                "run_command",
                &serde_json::json!({ "command": format!("{} --version", exec_probe()) }),
            )
            .unwrap();
        assert!(res.contains("exit code:"), "must report exit code; got: {res}");
    }

    #[test]
    fn run_command_runs_in_sandbox_cwd() {
        // `git rev-parse --show-toplevel`-free check: write a file then list via
        // an allowlisted command's view of cwd. Simplest: ensure cwd is the root
        // by having the command observe a file we placed there. We use `ls`/`dir`
        // surrogate via git status output not being reliable; instead verify the
        // process cwd by reading it back through a written marker.
        let (exec, dir) = setup();
        fs::write(dir.join("marker_in_root.txt"), "1").unwrap();
        // `find` is allowlisted on unix; skip the assertion on windows where the
        // allowlist binary may not be present, but still confirm cwd indirectly
        // by a successful run.
        if cfg!(windows) {
            let res = exec.execute(
                "run_command",
                &serde_json::json!({ "command": "cargo --version" }),
            );
            assert!(res.is_ok());
        } else {
            let res = exec
                .execute(
                    "run_command",
                    &serde_json::json!({ "command": "ls" }),
                )
                .unwrap();
            assert!(
                res.contains("marker_in_root.txt"),
                "command cwd must be the sandbox root; got: {res}"
            );
        }
    }

    #[test]
    fn run_command_timeout_kills_long_command() {
        // A short timeout against a deliberately long-running allowlisted command.
        let (exec, _dir) = setup();
        // Use a long-sleeping command. `python3 -c "import time; time.sleep(30)"`
        // on unix; on windows use a long cargo build-free sleeper. python3 is
        // allowlisted; skip if python3 is unavailable by checking the error kind.
        let long_cmd = if cfg!(windows) {
            // node is allowlisted; setTimeout keeps it alive.
            "node -e setTimeout(function(){},30000)"
        } else {
            "python3 -c import time;time.sleep(30)"
        };
        let exec = exec.with_command_timeout(Duration::from_millis(300));
        let start = Instant::now();
        let res = exec.execute("run_command", &serde_json::json!({ "command": long_cmd }));
        let elapsed = start.elapsed();
        // Either it timed out (interpreter present) or failed to spawn (absent).
        if let Err(e) = &res {
            if e.contains("timed out") {
                assert!(
                    elapsed < Duration::from_secs(5),
                    "timeout should fire promptly; took {elapsed:?}"
                );
            }
            // If the interpreter isn't installed it'll be a spawn error — still a
            // valid (non-hanging) outcome for this environment.
        } else {
            panic!("a 30s sleeper under a 300ms timeout must not succeed: {res:?}");
        }
    }

    #[test]
    fn run_command_output_is_capped() {
        // Generate output larger than MAX_TOOL_OUTPUT and confirm truncation.
        let (exec, _dir) = setup();
        let big = "A".repeat(MAX_TOOL_OUTPUT + 5000);
        let cmd = if cfg!(windows) {
            format!("node -e process.stdout.write('{big}')")
        } else {
            format!("python3 -c print('{}')", "A".repeat(MAX_TOOL_OUTPUT + 5000))
        };
        let _ = big;
        let res = exec.execute("run_command", &serde_json::json!({ "command": cmd }));
        if let Ok(out) = res {
            // exit-code prefix + capped body; allow some slack for the prefix.
            assert!(
                out.len() <= MAX_TOOL_OUTPUT + 200,
                "output must be capped; len={}",
                out.len()
            );
            assert!(out.contains("truncated"), "must note truncation; got tail");
        }
        // If the interpreter is absent (spawn error), the cap simply isn't
        // exercised in this environment — acceptable.
    }

    // ---- delegation --------------------------------------------------------

    #[test]
    fn read_only_tools_still_delegated() {
        let (exec, dir) = setup();
        fs::write(dir.join("d.txt"), "data").unwrap();
        let out = exec
            .execute("read_file", &serde_json::json!({ "path": "d.txt" }))
            .unwrap();
        assert_eq!(out, "data");
    }

    #[test]
    fn unknown_tool_rejected() {
        let (exec, _dir) = setup();
        let res = exec.execute("nope", &serde_json::json!({}));
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("Unknown tool"));
    }
}
