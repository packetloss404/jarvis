//! Sandbox that restricts tool operations to a specific directory.

use std::path::{Component, Path, PathBuf};

/// Sensitive path segments that must never be accessed, regardless of sandbox
/// location. These are matched for EXACT EQUALITY against the individual
/// components of a canonical path (see [`path_has_blocked_segment`]) — NOT as a
/// substring — so `release.env.example` does not trip the `.env` rule while a
/// real `.env` file or `.ssh` directory still does.
/// `/etc/passwd` and `/etc/shadow` are deliberately NOT here: they are absolute
/// paths outside any workspace sandbox and are already rejected by the
/// in-sandbox jail check. Listing a bare `passwd`/`shadow` component here would
/// over-block legitimate in-workspace files of that name.
const BLOCKED_PATH_SEGMENTS: &[&str] = &[".ssh", ".aws", ".gnupg", ".env", ".git"];

/// Commands allowed for execution inside the sandbox.
///
/// IMPORTANT (Windows / BatBadBut, CVE-2024-24576): tools that on Windows exist
/// ONLY as a `.cmd`/`.bat` batch shim (notably `npm`, `npx`, `yarn`) are
/// deliberately EXCLUDED. `std::process::Command::new` re-invokes `cmd.exe` for
/// a batch target, which re-introduces shell metacharacter interpretation and
/// defeats the no-shell guarantee. Only `.exe`-backed programs are kept here.
/// The executor additionally rejects any resolved argv0 that is a `.cmd`/`.bat`/
/// `.com` file (see `run_command`), so even a future allowlist mistake fails
/// closed.
const ALLOWED_COMMANDS: &[&str] = &[
    "ls", "cat", "head", "tail", "wc", "find", "grep", "rg", "git", "cargo", "rustc", "node",
    "python3", "echo", "mkdir", "cp", "mv", "rm", "touch",
];

/// Return true if any component of `path` exactly equals a blocked segment.
///
/// Matching on whole [`Path::components`] (rather than a substring of the path
/// string) means we block `.../.ssh/...` or a leaf named `.env` but NOT
/// `release.env.example` or `my.git.notes`.
pub fn path_has_blocked_segment(path: &Path) -> Option<&'static str> {
    for comp in path.components() {
        if let Component::Normal(os) = comp {
            // Compare case-insensitively on Windows where the filesystem is.
            let name = os.to_string_lossy();
            for seg in BLOCKED_PATH_SEGMENTS {
                let matches = if cfg!(windows) {
                    name.eq_ignore_ascii_case(seg)
                } else {
                    name == **seg
                };
                if matches {
                    return Some(seg);
                }
            }
        }
    }
    None
}

/// Sandbox that restricts tool operations to a specific directory
/// and validates commands against an allowlist.
pub struct ToolSandbox {
    sandbox_dir: PathBuf,
}

impl ToolSandbox {
    /// Create a new sandbox rooted at the given directory.
    ///
    /// The `sandbox_dir` should be a canonicalized absolute path.
    pub fn new(sandbox_dir: PathBuf) -> Self {
        Self { sandbox_dir }
    }

    /// The canonical sandbox root.
    pub fn root(&self) -> &Path {
        &self.sandbox_dir
    }

    /// Validate that `path` resolves to a location inside the sandbox
    /// and does not reference any sensitive path segments.
    ///
    /// If the path does not exist yet, the parent directory is canonicalized
    /// instead (to support file-creation use cases).
    ///
    /// Returns the canonicalized path on success.
    pub fn validate_path(&self, path: &Path) -> Result<PathBuf, String> {
        // Canonicalize the path — fall back to the parent when the leaf doesn't exist yet.
        let canonical: PathBuf = std::fs::canonicalize(path).or_else(|_| {
            let parent = path
                .parent()
                .ok_or_else(|| "Access denied: cannot resolve parent directory".to_string())?;
            let canon_parent = std::fs::canonicalize(parent).map_err(|e| {
                format!(
                    "Access denied: cannot resolve path '{}': {e}",
                    parent.display()
                )
            })?;
            let file_name = path
                .file_name()
                .ok_or_else(|| "Access denied: path has no file name".to_string())?;
            // Re-attach the final component so the caller gets the full intended path.
            Ok::<PathBuf, String>(canon_parent.join(file_name))
        })?;

        // The resolved path must live inside the sandbox.
        if !canonical.starts_with(&self.sandbox_dir) {
            return Err(format!(
                "Access denied: path '{}' is outside sandbox '{}'",
                canonical.display(),
                self.sandbox_dir.display(),
            ));
        }

        // Reject any path whose canonical components hit a blocked segment.
        // EXACT component match (not substring) — see `path_has_blocked_segment`.
        if let Some(seg) = path_has_blocked_segment(&canonical) {
            return Err(format!(
                "Access denied: path '{}' contains blocked segment '{seg}'",
                canonical.display(),
            ));
        }

        Ok(canonical)
    }

    /// Validate a path-like ARGUMENT passed to `run_command`, applying the SAME
    /// jail logic as [`validate_path`]: it must resolve within the sandbox root
    /// and must not name a blocked segment.
    ///
    /// `raw` is the token exactly as it appeared in argv. A leading `~` is
    /// expanded to a marker OUTSIDE the sandbox so any `~/...` reference is
    /// rejected (we never resolve to the real home). A relative token is
    /// resolved against the sandbox root; an absolute token is taken as-is.
    ///
    /// Returns `Ok(())` if the argument stays inside the jail, `Err` otherwise.
    pub fn validate_arg_path(&self, raw: &str) -> Result<(), String> {
        let expanded: PathBuf = if let Some(rest) = strip_tilde(raw) {
            // `~` / `~/...` / `~user` — never resolve to the real home. Point at
            // a clearly-outside-sandbox location so the jail check below rejects
            // it deterministically.
            let mut p = PathBuf::from(if cfg!(windows) { "C:\\__home__" } else { "/__home__" });
            if !rest.is_empty() {
                p.push(rest);
            }
            p
        } else {
            let p = Path::new(raw);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                self.sandbox_dir.join(p)
            }
        };

        // Lexically resolve `.`/`..` WITHOUT touching the filesystem, so a
        // missing intermediate component can't make canonicalize() bail before
        // we've caught the escape. (We still run the canonical jail check on the
        // deepest existing ancestor for symlink safety, below.)
        let lexical = lexically_normalize(&expanded);

        if !lexical.starts_with(&self.sandbox_dir) {
            return Err(format!(
                "Access denied: command path argument '{raw}' resolves outside the sandbox"
            ));
        }
        if let Some(seg) = path_has_blocked_segment(&lexical) {
            return Err(format!(
                "Access denied: command path argument '{raw}' contains blocked segment '{seg}'"
            ));
        }

        // Belt-and-suspenders: if the deepest existing ancestor canonicalizes
        // (resolving symlinks), it too must remain in the jail. A non-existent
        // path simply skips this — the lexical check already passed.
        let mut probe = lexical.as_path();
        loop {
            if let Ok(canon) = std::fs::canonicalize(probe) {
                if !canon.starts_with(&self.sandbox_dir) {
                    return Err(format!(
                        "Access denied: command path argument '{raw}' escapes the sandbox via a symlink"
                    ));
                }
                if let Some(seg) = path_has_blocked_segment(&canon) {
                    return Err(format!(
                        "Access denied: command path argument '{raw}' contains blocked segment '{seg}'"
                    ));
                }
                break;
            }
            match probe.parent() {
                Some(p) if p != probe => probe = p,
                _ => break,
            }
        }

        Ok(())
    }

    /// Validate that the first word of `cmd` is in the command allowlist.
    pub fn validate_command(&self, cmd: &str) -> Result<(), String> {
        let first_word = cmd.split_whitespace().next().unwrap_or("");

        // Reject path-qualified names outright (covers `/usr/bin/ls`, `..\\ls`,
        // `sub/ls`). A bare program name has no separators.
        if first_word.contains('/') || first_word.contains('\\') {
            return Err(format!(
                "Command not allowed: '{first_word}' must be a bare program name"
            ));
        }

        if ALLOWED_COMMANDS.contains(&first_word) {
            Ok(())
        } else {
            Err(format!("Command not allowed: {first_word}"))
        }
    }
}

/// If `raw` begins with a `~` (home reference), return the remainder after the
/// leading `~[user]` and the following separator. Returns `None` when `raw`
/// does not start with `~`.
fn strip_tilde(raw: &str) -> Option<&str> {
    let rest = raw.strip_prefix('~')?;
    // `~`, `~/foo`, `~\foo`, or `~user/foo` — drop everything up to and
    // including the first separator (or all of it if there is none).
    let after = rest
        .find(['/', '\\'])
        .map(|i| &rest[i + 1..])
        .unwrap_or("");
    Some(after)
}

/// Lexically normalize a path, collapsing `.` and `..` WITHOUT touching the
/// filesystem. A `..` that would pop above the root is dropped (it cannot
/// escape lexically). This is intentionally conservative: it never resolves
/// symlinks (that is the canonical pass's job).
fn lexically_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                if !out.pop() {
                    // Above the anchor; keep a literal `..` so a clearly-relative
                    // escape still fails the `starts_with(root)` check.
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_segment_exact_match_not_substring() {
        // Real blocked segments hit.
        assert!(path_has_blocked_segment(Path::new("/home/u/.ssh/id_rsa")).is_some());
        assert!(path_has_blocked_segment(Path::new("/proj/.env")).is_some());
        assert!(path_has_blocked_segment(Path::new("/proj/.git/config")).is_some());
        // Look-alikes that merely CONTAIN the substring must NOT match.
        assert!(path_has_blocked_segment(Path::new("/proj/release.env.example")).is_none());
        assert!(path_has_blocked_segment(Path::new("/proj/my.git.notes")).is_none());
        assert!(path_has_blocked_segment(Path::new("/proj/passwdmgr.txt")).is_none());
    }

    #[test]
    fn validate_command_rejects_separators() {
        let sb = ToolSandbox::new(PathBuf::from("/tmp/x"));
        assert!(sb.validate_command("/bin/ls").is_err());
        assert!(sb.validate_command("..\\ls").is_err());
        assert!(sb.validate_command("sub/ls").is_err());
        assert!(sb.validate_command("ls").is_ok());
        assert!(sb.validate_command("curl").is_err());
    }

    #[test]
    fn npm_npx_yarn_not_allowlisted() {
        let sb = ToolSandbox::new(PathBuf::from("/tmp/x"));
        for c in ["npm", "npx", "yarn"] {
            assert!(
                sb.validate_command(c).is_err(),
                "{c} must not be allowlisted (Windows .cmd shim risk)"
            );
        }
        // node/cargo (real .exe-backed) stay allowed.
        assert!(sb.validate_command("node").is_ok());
        assert!(sb.validate_command("cargo").is_ok());
    }

    #[test]
    fn validate_arg_path_rejects_escapes() {
        let dir = std::env::temp_dir().join(format!("jarvis_sb_arg_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let root = std::fs::canonicalize(&dir).unwrap();
        let sb = ToolSandbox::new(root.clone());

        // Traversal escape.
        assert!(sb.validate_arg_path("../../etc/passwd").is_err());
        // Tilde / home.
        assert!(sb.validate_arg_path("~/.ssh/id_rsa").is_err());
        assert!(sb.validate_arg_path("~").is_err());
        // Absolute outside.
        let outside = if cfg!(windows) { "C:\\Windows\\system32\\drivers\\etc\\hosts" } else { "/etc/passwd" };
        assert!(sb.validate_arg_path(outside).is_err());
        // Blocked segment even when nominally in-sandbox.
        assert!(sb.validate_arg_path("./.env").is_err());
        assert!(sb.validate_arg_path("sub/.ssh/key").is_err());

        // In-sandbox relative paths are allowed.
        assert!(sb.validate_arg_path("./file.txt").is_ok());
        assert!(sb.validate_arg_path("a/b/c.txt").is_ok());
        // Look-alike is allowed (exact-match, not substring).
        assert!(sb.validate_arg_path("release.env.example").is_ok());
    }
}
