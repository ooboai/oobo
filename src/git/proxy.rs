use std::process::{Command, Stdio};

use crate::config::Config;
use crate::git::{commands, interceptor};

/// Run the real git binary with the given arguments, inheriting stdio.
/// Returns the exit code.
pub fn run_git(cfg: &Config, args: &[&str]) -> Result<i32, String> {
    let git_path = cfg.git_path();

    let status = Command::new(git_path)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| format!("failed to run {git_path}: {e}"))?;

    Ok(status.code().unwrap_or(1))
}

/// Run a git command and capture its stdout (for internal use, not user-facing).
pub fn run_git_capture(cfg: &Config, args: &[&str]) -> Result<String, String> {
    let git_path = cfg.git_path();

    let output = Command::new(git_path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("failed to run {git_path}: {e}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "git {}: {}",
            args.first().unwrap_or(&""),
            stderr.trim()
        ))
    }
}

/// Get the project root via `git rev-parse --show-toplevel`.
pub fn project_root(cfg: &Config) -> Option<String> {
    run_git_capture(cfg, &["rev-parse", "--show-toplevel"]).ok()
}

/// Get current branch name.
pub fn current_branch(cfg: &Config) -> Option<String> {
    run_git_capture(cfg, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()
}

/// Run git, then intercept write ops to collect and send context.
pub fn run_and_intercept(cfg: &Config, args: &[&str]) -> Result<i32, String> {
    let exit_code = run_git(cfg, args)?;

    if exit_code == 0 && commands::is_write_op(args) && cfg.telemetry.enabled {
        // Fire-and-forget: collect context and send to server.
        // Errors are logged but never block the user.
        if let Err(e) = interceptor::on_write_op(cfg, args) {
            log_error(&format!("interceptor error: {e}"));
        }
    }

    Ok(exit_code)
}

fn log_error(msg: &str) {
    let log_dir = Config::log_dir();
    let _ = std::fs::create_dir_all(&log_dir);
    let log_file = log_dir.join("errors.log");
    let timestamp = chrono::Utc::now().to_rfc3339();
    let line = format!("[{timestamp}] {msg}\n");
    // Append to log file, ignoring errors
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
    {
        use std::io::Write;
        let _ = f.write_all(line.as_bytes());
    }
}
