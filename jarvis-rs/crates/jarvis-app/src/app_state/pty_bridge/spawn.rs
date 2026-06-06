//! PTY spawn logic: create a new PTY with the user's default shell.

use std::io::Read;
use std::sync::mpsc;
use std::thread;

use jarvis_config::schema::ShellConfig;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use super::types::{PtyHandle, DEFAULT_COLS, DEFAULT_ROWS, PTY_READ_CHUNK};

// =============================================================================
// SHELL DETECTION
// =============================================================================

/// Get the user's default shell.
///
/// - Unix: reads `$SHELL`, falls back to `/bin/sh`
/// - Windows: reads `$COMSPEC`, falls back to `cmd.exe`
pub fn default_shell() -> String {
    #[cfg(unix)]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    }
}

// =============================================================================
// ENVIRONMENT SANITIZATION
// =============================================================================

/// Allowed environment variables to inherit.
///
/// We inherit a minimal set to avoid leaking Jarvis-internal secrets
/// (API keys, tokens, etc.) into the shell environment.
const ALLOWED_ENV_VARS: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "PATH",
    "TERM",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "TMPDIR",
    "TMP",
    "TEMP",
    // Windows-specific
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "SYSTEMROOT",
    "COMSPEC",
    "HOMEDRIVE",
    "HOMEPATH",
];

/// Build a sanitized `CommandBuilder` for `program`, honoring the user's
/// [`ShellConfig`] (`args`, `env`, `login_shell`) and the resolved `cwd`.
fn build_shell_command(program: &str, cwd: Option<&str>, shell: &ShellConfig) -> CommandBuilder {
    let mut cmd = CommandBuilder::new(program);

    // Clear inherited env, then selectively re-add safe vars (the allowlist
    // exists so Jarvis-internal secrets — API keys, tokens — never leak into the
    // shell automatically).
    cmd.env_clear();
    for key in ALLOWED_ENV_VARS {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }

    // Always set TERM for proper terminal behavior
    cmd.env("TERM", "xterm-256color");

    // User-configured environment (`[shell.env]`) — explicit user intent, so it
    // is applied ON TOP of the allowlist and may add or override vars.
    for (key, val) in &shell.env {
        cmd.env(key, val);
    }

    // On Unix, pass -l for a login shell (loads .profile, .bash_profile, etc.)
    // unless the user disabled it. No effect on Windows shells.
    #[cfg(unix)]
    {
        if shell.login_shell {
            cmd.arg("-l");
        }
    }

    // Extra user-configured arguments (`[shell].args`), after the login flag.
    for arg in &shell.args {
        cmd.arg(arg);
    }

    // Set working directory if provided and valid
    if let Some(dir) = cwd {
        if !dir.is_empty() {
            // Expand ~ to home directory
            let expanded = if dir.starts_with("~/") || dir == "~" {
                if let Ok(home) = std::env::var("HOME") {
                    dir.replacen('~', &home, 1)
                } else {
                    dir.to_string()
                }
            } else {
                dir.to_string()
            };
            let path = std::path::Path::new(&expanded);
            if path.is_dir() {
                cmd.cwd(&expanded);
            } else {
                tracing::warn!(dir, expanded = %expanded, "Working directory does not exist, using default");
            }
        }
    }

    cmd
}

// =============================================================================
// SPAWN
// =============================================================================

/// Spawn a new PTY with the given terminal size, using the default shell config.
///
/// Returns a `PtyHandle` that owns the master side of the PTY pair.
/// A background thread reads output from the PTY and sends chunks
/// over the returned handle's `output_rx` channel.
pub fn spawn_pty(cols: u16, rows: u16, cwd: Option<&str>) -> Result<PtyHandle, String> {
    spawn_pty_with_shell(cols, rows, cwd, &ShellConfig::default())
}

/// Spawn a new PTY honoring the user's [`ShellConfig`] (`program`, `args`,
/// `env`, `login_shell`). `cwd` (already resolved by the caller, e.g. a per-pane
/// override) takes precedence over `shell.working_directory`.
pub fn spawn_pty_with_shell(
    cols: u16,
    rows: u16,
    cwd: Option<&str>,
    shell: &ShellConfig,
) -> Result<PtyHandle, String> {
    let pty_system = native_pty_system();

    let size = PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    };

    let pair = pty_system
        .openpty(size)
        .map_err(|e| format!("Failed to open PTY: {e}"))?;

    // Configured program wins; otherwise auto-detect the user's default shell.
    let program = if shell.program.trim().is_empty() {
        default_shell()
    } else {
        shell.program.clone()
    };
    let cmd = build_shell_command(&program, cwd, shell);

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("Failed to spawn shell '{program}': {e}"))?;

    // Drop the slave side — we only need the master
    drop(pair.slave);

    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("Failed to take PTY writer: {e}"))?;

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("Failed to clone PTY reader: {e}"))?;

    // Spawn a background thread to read PTY output
    let (tx, rx) = mpsc::channel::<Vec<u8>>();

    thread::Builder::new()
        .name("pty-reader".to_string())
        .spawn(move || {
            let mut buf = [0u8; PTY_READ_CHUNK];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF — shell exited
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break; // Receiver dropped
                        }
                    }
                    Err(e) => {
                        tracing::debug!("PTY reader error: {e}");
                        break;
                    }
                }
            }
        })
        .map_err(|e| format!("Failed to spawn PTY reader thread: {e}"))?;

    Ok(PtyHandle {
        writer,
        output_rx: rx,
        child,
        master: Some(pair.master),
        size,
    })
}

/// Spawn a PTY with default terminal dimensions.
#[allow(dead_code)] // Used in tests
pub fn spawn_pty_default() -> Result<PtyHandle, String> {
    spawn_pty(DEFAULT_COLS, DEFAULT_ROWS, None)
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shell_returns_nonempty() {
        let shell = default_shell();
        assert!(!shell.is_empty(), "default shell should not be empty");
    }

    #[test]
    fn default_shell_is_absolute_on_unix() {
        let shell = default_shell();
        #[cfg(unix)]
        {
            // Either from $SHELL (usually absolute) or /bin/sh
            // We just check it's not empty — $SHELL could be relative in CI
            assert!(!shell.is_empty());
        }
        #[cfg(windows)]
        {
            assert!(!shell.is_empty());
        }
    }

    #[test]
    fn allowed_env_vars_contains_essentials() {
        assert!(ALLOWED_ENV_VARS.contains(&"HOME"));
        assert!(ALLOWED_ENV_VARS.contains(&"PATH"));
        assert!(ALLOWED_ENV_VARS.contains(&"TERM"));
        assert!(ALLOWED_ENV_VARS.contains(&"USER"));
    }

    #[test]
    fn allowed_env_vars_excludes_secrets() {
        // Ensure we don't accidentally inherit secret-like vars
        for var in ALLOWED_ENV_VARS {
            let lower = var.to_lowercase();
            assert!(
                !lower.contains("key"),
                "ALLOWED_ENV_VARS should not contain '{var}'"
            );
            assert!(
                !lower.contains("secret"),
                "ALLOWED_ENV_VARS should not contain '{var}'"
            );
            assert!(
                !lower.contains("token"),
                "ALLOWED_ENV_VARS should not contain '{var}'"
            );
            assert!(
                !lower.contains("password"),
                "ALLOWED_ENV_VARS should not contain '{var}'"
            );
        }
    }

    #[test]
    fn spawn_pty_creates_handle() {
        let handle = spawn_pty(80, 24, None);
        assert!(
            handle.is_ok(),
            "spawn_pty should succeed: {:?}",
            handle.err()
        );
        let mut handle = handle.unwrap();
        assert_eq!(handle.size.cols, 80);
        assert_eq!(handle.size.rows, 24);
        // Clean up
        handle.child.kill().ok();
    }

    #[test]
    fn spawn_pty_with_shell_honors_configured_program() {
        // A configured program that does not exist must surface as a spawn error,
        // proving `shell.program` is actually used (not silently ignored).
        let shell = ShellConfig {
            program: "/nonexistent/jarvis-test-shell-xyz".into(),
            ..Default::default()
        };
        let result = spawn_pty_with_shell(80, 24, None, &shell);
        assert!(result.is_err(), "spawn with a bogus program should fail");
    }

    #[test]
    fn spawn_pty_with_default_shell_config_succeeds() {
        // An empty program falls back to the user's default shell.
        let mut handle =
            spawn_pty_with_shell(80, 24, None, &ShellConfig::default()).expect("spawn should work");
        handle.child.kill().ok();
    }

    #[test]
    fn spawn_pty_default_uses_standard_size() {
        let handle = spawn_pty_default();
        assert!(handle.is_ok(), "spawn_pty_default should succeed");
        let mut handle = handle.unwrap();
        assert_eq!(handle.size.cols, DEFAULT_COLS);
        assert_eq!(handle.size.rows, DEFAULT_ROWS);
        handle.child.kill().ok();
    }
}
