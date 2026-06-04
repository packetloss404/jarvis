//! Read-only tool executor.
//!
//! Maps a tool name + JSON arguments to a result string for the READ-ONLY
//! filesystem tools only: `read_file`, `search_files`, `search_content`, and
//! `list_directory`. There is intentionally NO support for `run_command` or
//! `write_file` — those tools are out of scope for this milestone and cannot be
//! executed even if the model somehow requests them.
//!
//! Every filesystem access is jailed through [`ToolSandbox::validate_path`]
//! before any disk I/O happens, and every tool's output is capped to avoid
//! flooding the model context (or a DoS via huge files / search results).

use std::path::{Path, PathBuf};

use regex::Regex;
use walkdir::WalkDir;

use super::sandbox::ToolSandbox;

/// Maximum characters returned by any single tool call. Output longer than this
/// is truncated and a notice is appended.
pub const MAX_TOOL_OUTPUT: usize = 12_000;

/// The read-only tool names this executor handles. Anything else is rejected.
pub const READ_ONLY_TOOLS: &[&str] =
    &["read_file", "search_files", "search_content", "list_directory"];

/// Executes read-only filesystem tools inside a [`ToolSandbox`].
pub struct ReadOnlyToolExecutor {
    sandbox: ToolSandbox,
    /// Sandbox root, used to resolve relative paths supplied by the model.
    root: PathBuf,
}

impl ReadOnlyToolExecutor {
    /// Create an executor rooted at `root`. The sandbox restricts all access to
    /// this directory subtree; `root` should be a canonicalized absolute path.
    pub fn new(root: PathBuf) -> Self {
        Self {
            sandbox: ToolSandbox::new(root.clone()),
            root,
        }
    }

    /// Returns the read-only subset of tool names this executor supports.
    pub fn tool_names() -> &'static [&'static str] {
        READ_ONLY_TOOLS
    }

    /// Execute a tool call by name. Returns the tool output (or an error string,
    /// which is surfaced to the model as a `tool_result` with `is_error`).
    ///
    /// `run_command` and `write_file` are explicitly rejected.
    pub fn execute(&self, name: &str, args: &serde_json::Value) -> Result<String, String> {
        match name {
            "read_file" => self.read_file(args),
            "search_files" => self.search_files(args),
            "search_content" => self.search_content(args),
            "list_directory" => self.list_directory(args),
            // Explicitly out of scope for this milestone — never executable.
            "run_command" | "write_file" => Err(format!(
                "Tool '{name}' is disabled: this assistant has read-only access only."
            )),
            other => Err(format!("Unknown tool: {other}")),
        }
    }

    /// Resolve a (possibly relative) path argument against the sandbox root,
    /// then jail it through the sandbox. Returns the canonical, in-jail path.
    fn resolve(&self, raw: &str) -> Result<PathBuf, String> {
        let p = Path::new(raw);
        let joined = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.root.join(p)
        };
        self.sandbox.validate_path(&joined)
    }

    fn read_file(&self, args: &serde_json::Value) -> Result<String, String> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| "read_file: missing 'path' argument".to_string())?;
        let canonical = self.resolve(path)?;

        let meta = std::fs::metadata(&canonical)
            .map_err(|e| format!("read_file: cannot stat '{path}': {e}"))?;
        if meta.is_dir() {
            return Err(format!("read_file: '{path}' is a directory"));
        }

        let content = std::fs::read_to_string(&canonical)
            .map_err(|e| format!("read_file: cannot read '{path}': {e}"))?;
        Ok(cap_output(&content))
    }

    fn search_files(&self, args: &serde_json::Value) -> Result<String, String> {
        let pattern = args["pattern"]
            .as_str()
            .ok_or_else(|| "search_files: missing 'pattern' argument".to_string())?;
        let base = match args["directory"].as_str() {
            Some(dir) => self.resolve(dir)?,
            None => self.root.clone(),
        };
        // Validate the base directory is inside the jail (resolve already did for
        // the explicit-directory case; for the default root it is trivially in).
        let _ = self.sandbox.validate_path(&base)?;

        // glob against base/pattern.
        let glob_pat = base.join(pattern);
        let glob_str = glob_pat.to_string_lossy().to_string();
        let entries = glob::glob(&glob_str)
            .map_err(|e| format!("search_files: invalid glob pattern: {e}"))?;

        let mut out = String::new();
        let mut count = 0usize;
        for entry in entries {
            let path = match entry {
                Ok(p) => p,
                Err(_) => continue,
            };
            // Re-jail every matched path before reporting it.
            if self.sandbox.validate_path(&path).is_err() {
                continue;
            }
            let display = self.relativize(&path);
            out.push_str(&display);
            out.push('\n');
            count += 1;
            if out.len() > MAX_TOOL_OUTPUT {
                break;
            }
        }

        if count == 0 {
            return Ok(format!("No files matched pattern '{pattern}'."));
        }
        Ok(cap_output(&out))
    }

    fn search_content(&self, args: &serde_json::Value) -> Result<String, String> {
        let pattern = args["pattern"]
            .as_str()
            .ok_or_else(|| "search_content: missing 'pattern' argument".to_string())?;
        let re = Regex::new(pattern)
            .map_err(|e| format!("search_content: invalid regex: {e}"))?;

        let base = match args["directory"].as_str() {
            Some(dir) => self.resolve(dir)?,
            None => self.root.clone(),
        };
        let _ = self.sandbox.validate_path(&base)?;

        let file_glob = args["file_pattern"].as_str();
        let file_matcher = match file_glob {
            Some(g) => Some(
                glob::Pattern::new(g)
                    .map_err(|e| format!("search_content: invalid file_pattern: {e}"))?,
            ),
            None => None,
        };

        let mut out = String::new();
        'walk: for entry in WalkDir::new(&base).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            // Jail every file before opening it.
            if self.sandbox.validate_path(path).is_err() {
                continue;
            }
            if let Some(ref m) = file_matcher {
                let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
                if !m.matches(&name) {
                    continue;
                }
            }
            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue, // skip binary / unreadable files
            };
            let rel = self.relativize(path);
            for (lineno, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    let trimmed: String = line.chars().take(300).collect();
                    out.push_str(&format!("{rel}:{}: {trimmed}\n", lineno + 1));
                    if out.len() > MAX_TOOL_OUTPUT {
                        break 'walk;
                    }
                }
            }
        }

        if out.is_empty() {
            return Ok(format!("No matches for pattern '{pattern}'."));
        }
        Ok(cap_output(&out))
    }

    fn list_directory(&self, args: &serde_json::Value) -> Result<String, String> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| "list_directory: missing 'path' argument".to_string())?;
        let canonical = self.resolve(path)?;

        let meta = std::fs::metadata(&canonical)
            .map_err(|e| format!("list_directory: cannot stat '{path}': {e}"))?;
        if !meta.is_dir() {
            return Err(format!("list_directory: '{path}' is not a directory"));
        }

        let read = std::fs::read_dir(&canonical)
            .map_err(|e| format!("list_directory: cannot read '{path}': {e}"))?;
        let mut entries: Vec<String> = Vec::new();
        for ent in read.flatten() {
            let p = ent.path();
            // Skip entries the sandbox would block (e.g. .ssh, .env).
            if self.sandbox.validate_path(&p).is_err() {
                continue;
            }
            let name = ent.file_name().to_string_lossy().to_string();
            let suffix = if p.is_dir() { "/" } else { "" };
            entries.push(format!("{name}{suffix}"));
        }
        entries.sort();

        if entries.is_empty() {
            return Ok("(empty directory)".to_string());
        }
        Ok(cap_output(&entries.join("\n")))
    }

    /// Render `path` relative to the sandbox root when possible, for compact,
    /// non-leaky output. Falls back to the full path.
    fn relativize(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string())
    }
}

/// Truncate `s` to [`MAX_TOOL_OUTPUT`] characters, appending a notice when cut.
fn cap_output(s: &str) -> String {
    if s.len() <= MAX_TOOL_OUTPUT {
        return s.to_string();
    }
    // Truncate on a char boundary at or before the cap.
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

    fn setup() -> (ReadOnlyToolExecutor, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "jarvis_exec_test_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        let canonical = fs::canonicalize(&dir).unwrap();
        (ReadOnlyToolExecutor::new(canonical.clone()), canonical)
    }

    #[test]
    fn read_file_happy_path() {
        let (exec, dir) = setup();
        fs::write(dir.join("hello.txt"), "hello world").unwrap();
        let out = exec
            .execute("read_file", &serde_json::json!({ "path": "hello.txt" }))
            .unwrap();
        assert_eq!(out, "hello world");
    }

    #[test]
    fn read_file_path_jail_enforced() {
        let (exec, _dir) = setup();
        // Attempt to escape the sandbox.
        let res = exec.execute(
            "read_file",
            &serde_json::json!({ "path": "../../../etc/passwd" }),
        );
        assert!(res.is_err(), "path traversal must be rejected");
    }

    #[test]
    fn read_file_absolute_outside_jail_rejected() {
        let (exec, _dir) = setup();
        let res = exec.execute("read_file", &serde_json::json!({ "path": "/etc/hosts" }));
        assert!(res.is_err(), "absolute path outside jail must be rejected");
    }

    #[test]
    fn list_directory_happy_path() {
        let (exec, dir) = setup();
        fs::write(dir.join("a.txt"), "a").unwrap();
        fs::create_dir_all(dir.join("sub")).unwrap();
        let out = exec
            .execute("list_directory", &serde_json::json!({ "path": "." }))
            .unwrap();
        assert!(out.contains("a.txt"), "got: {out}");
        assert!(out.contains("sub/"), "got: {out}");
    }

    #[test]
    fn search_files_glob() {
        let (exec, dir) = setup();
        fs::write(dir.join("one.rs"), "fn main() {}").unwrap();
        fs::write(dir.join("two.rs"), "fn other() {}").unwrap();
        fs::write(dir.join("note.txt"), "text").unwrap();
        let out = exec
            .execute("search_files", &serde_json::json!({ "pattern": "*.rs" }))
            .unwrap();
        assert!(out.contains("one.rs"), "got: {out}");
        assert!(out.contains("two.rs"), "got: {out}");
        assert!(!out.contains("note.txt"), "got: {out}");
    }

    #[test]
    fn search_content_regex() {
        let (exec, dir) = setup();
        fs::write(dir.join("code.rs"), "let x = 1;\nfn target() {}\nlet y = 2;").unwrap();
        let out = exec
            .execute(
                "search_content",
                &serde_json::json!({ "pattern": "fn \\w+", "file_pattern": "*.rs" }),
            )
            .unwrap();
        assert!(out.contains("target"), "got: {out}");
        assert!(out.contains("code.rs:2"), "should report line number, got: {out}");
    }

    #[test]
    fn output_truncation() {
        let (exec, dir) = setup();
        let big = "x".repeat(MAX_TOOL_OUTPUT + 5000);
        fs::write(dir.join("big.txt"), &big).unwrap();
        let out = exec
            .execute("read_file", &serde_json::json!({ "path": "big.txt" }))
            .unwrap();
        assert!(out.len() <= MAX_TOOL_OUTPUT + 100, "output should be capped");
        assert!(out.contains("truncated"), "should note truncation");
    }

    #[test]
    fn run_command_and_write_file_disabled() {
        let (exec, _dir) = setup();
        let res = exec.execute("run_command", &serde_json::json!({ "command": "ls" }));
        assert!(res.is_err(), "run_command must be disabled");
        assert!(res.unwrap_err().contains("read-only"));

        let res = exec.execute(
            "write_file",
            &serde_json::json!({ "path": "x", "content": "y" }),
        );
        assert!(res.is_err(), "write_file must be disabled");
    }

    #[test]
    fn blocked_segment_in_listing_skipped() {
        let (exec, dir) = setup();
        fs::write(dir.join("normal.txt"), "ok").unwrap();
        fs::write(dir.join(".env"), "SECRET=1").unwrap();
        let out = exec
            .execute("list_directory", &serde_json::json!({ "path": "." }))
            .unwrap();
        assert!(out.contains("normal.txt"));
        assert!(!out.contains(".env"), "blocked segment must be hidden, got: {out}");
    }
}
