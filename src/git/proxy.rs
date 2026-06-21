use std::process::{Command, Stdio};

use crate::config::Config;
use crate::error::{CliError, CmdResult};
use crate::git::{commands, interceptor};

/// Scrub git environment variables that would interfere with git
/// subcommands running in a different worktree/repo.
pub fn scrub_git_env(cmd: &mut Command) -> &mut Command {
    cmd.env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_QUARANTINE_PATH")
        .env_remove("GIT_INDEX_FILE")
}

/// Run the real git binary with the given arguments, inheriting stdio.
/// Returns the exit code.
pub fn run_git(cfg: &Config, args: &[&str]) -> CmdResult {
    let git_path = cfg.git_path();

    let status = Command::new(git_path)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| CliError::Git(format!("failed to run {git_path}: {e}")))?;

    Ok(status.code().unwrap_or(1))
}

/// Run a git command and capture its stdout (for internal use, not user-facing).
pub fn run_git_capture(cfg: &Config, args: &[&str]) -> Result<String, CliError> {
    run_git_capture_in(cfg, args, None)
}

pub fn run_git_capture_in(
    cfg: &Config,
    args: &[&str],
    dir: Option<&str>,
) -> Result<String, CliError> {
    let git_path = cfg.git_path();

    let mut cmd = Command::new(git_path);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    scrub_git_env(&mut cmd);
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    let output = cmd
        .output()
        .map_err(|e| CliError::Git(format!("failed to run {git_path}: {e}")))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout)
            .replace('\r', "")
            .trim()
            .to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(CliError::Git(format!(
            "git {}: {}",
            args.first().unwrap_or(&""),
            stderr.trim()
        )))
    }
}

/// Get the project root via `git rev-parse --show-toplevel`.
pub fn project_root(cfg: &Config) -> Option<String> {
    run_git_capture(cfg, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(|s| crate::utils::normalize_win_path(&s).to_string())
}

/// Get the project root for a given working directory (no Config needed).
pub fn project_root_from(cwd: &str) -> String {
    let root = crate::utils::normalize_win_path(cwd);
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let mut cmd = Command::new(git);
    cmd.args(["rev-parse", "--show-toplevel"])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    scrub_git_env(&mut cmd);
    cmd.output()
        .ok()
        .filter(|o| o.status.success())
        .map_or_else(
            || root.to_string(),
            |o| {
                let raw = String::from_utf8_lossy(&o.stdout)
                    .replace('\r', "")
                    .trim()
                    .to_string();
                crate::utils::normalize_win_path(&raw).to_string()
            },
        )
}

/// Get current branch name.
pub fn current_branch(cfg: &Config) -> Option<String> {
    run_git_capture(cfg, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()
}

/// Run git, then intercept write ops to collect and send context.
pub fn run_and_intercept(cfg: &Config, args: &[&str]) -> CmdResult {
    if commands::is_write_op(args) && cfg.telemetry.enabled {
        std::env::set_var("OOBO_INTERCEPTED", "1");
    }

    let pre_rewrite_map = if is_rewrite_op(args) && cfg.telemetry.enabled {
        capture_pre_rewrite_commits(cfg)
    } else {
        Vec::new()
    };

    let exit_code = run_git(cfg, args)?;

    if exit_code != 0 {
        return Ok(exit_code);
    }

    if !pre_rewrite_map.is_empty() {
        if let Some(root) = project_root(cfg) {
            if let Err(e) = super::orphan::rekey_anchors(&root, &pre_rewrite_map) {
                log_error(&format!("oobo rekey error: {e}"));
            }
        }
    }

    // For clone, resolve the new directory; for everything else, use cwd.
    let root_for_ignore = if is_clone(args) {
        resolve_clone_dir(args).map(|d| {
            let abs = if std::path::Path::new(&d).is_absolute() {
                d
            } else {
                std::env::current_dir()
                    .map(|cwd| cwd.join(&d).to_string_lossy().to_string())
                    .unwrap_or(d)
            };
            abs
        })
    } else {
        project_root(cfg)
    };

    if let Some(ref root) = root_for_ignore {
        if cfg.is_ignored(root) || !crate::project_config::is_enabled(root) {
            return Ok(exit_code);
        }
    }

    if commands::is_write_op(args) && cfg.telemetry.enabled {
        if let Err(e) = interceptor::on_write_op(cfg, args) {
            log_error(&format!("interceptor error: {e}"));
        }
    }

    if is_clone(args) {
        post_clone(cfg, args);
    }

    if is_fetch(args) {
        if let Some(root) = project_root(cfg) {
            crate::commands::sync::auto_hydrate(&root);
        }
    }

    Ok(exit_code)
}

fn is_clone(args: &[&str]) -> bool {
    commands::subcommand_name(args) == Some("clone")
}

fn is_fetch(args: &[&str]) -> bool {
    commands::subcommand_name(args) == Some("fetch")
}

/// After a successful `git clone`, resolve the cloned directory and fetch
/// the orphan branch if it exists on the remote.
fn post_clone(_cfg: &Config, args: &[&str]) {
    let clone_dir = match resolve_clone_dir(args) {
        Some(d) => d,
        None => return,
    };

    let root = project_root_from(&clone_dir);
    if root.is_empty() {
        return;
    }

    // v2 store: fetch independently of v1 — a v2-only repo must still
    // hydrate (and vice versa during the transition window).
    if crate::git::orphan::v2::remote_branch_exists(&root) {
        if let Err(e) = crate::git::orphan::v2::fetch_and_reconcile(&root) {
            eprintln!("oobo: could not fetch v2 anchor metadata: {e}");
        }
    }

    if !crate::git::orphan::remote_branch_exists(&root) {
        return;
    }

    if let Err(e) = crate::git::orphan::fetch_and_reconcile(&root) {
        eprintln!("oobo: could not fetch anchor metadata: {e}");
        return;
    }

    crate::commands::sync::auto_hydrate(&root);
}

/// Clone flags that consume the next argument as a value.
const CLONE_FLAGS_WITH_VALUE: &[&str] = &[
    "--depth",
    "--branch",
    "-b",
    "--reference",
    "--origin",
    "-o",
    "--template",
    "--separate-git-dir",
    "--jobs",
    "-j",
    "--filter",
    "--recurse-submodules",
    "-c",
];

/// Determine the directory a `git clone` created.
/// `git clone <url> [dir]`  --  if dir is given, use it; otherwise derive from the URL.
fn resolve_clone_dir(args: &[&str]) -> Option<String> {
    let after_clone: Vec<&str> = args
        .iter()
        .copied()
        .skip_while(|a| a != &"clone")
        .skip(1)
        .collect();

    let mut positional = Vec::new();
    let mut skip_next = false;
    for arg in &after_clone {
        if skip_next {
            skip_next = false;
            continue;
        }
        if CLONE_FLAGS_WITH_VALUE.contains(arg) {
            skip_next = true;
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        positional.push(*arg);
    }

    match positional.len() {
        0 => None,
        1 => {
            let url = positional[0];
            let name = url.rsplit('/').next().unwrap_or(url);
            let name = name.strip_suffix(".git").unwrap_or(name);
            if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            }
        }
        _ => Some(positional.last().unwrap().to_string()),
    }
}

fn log_error(msg: &str) {
    let log_dir = Config::log_dir();
    let _ = std::fs::create_dir_all(&log_dir);
    let log_file = log_dir.join("errors.log");
    let timestamp = chrono::Utc::now().to_rfc3339();
    let line = format!("[{timestamp}] {msg}\n");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
    {
        use std::io::Write;
        let _ = f.write_all(line.as_bytes());
    }
}

fn is_rewrite_op(args: &[&str]) -> bool {
    matches!(
        commands::subcommand_name(args),
        Some("rebase" | "cherry-pick")
    )
}

/// Capture current branch's commit hash → tree hash mapping before a rewrite.
/// After the rewrite, we match by tree hash to find old→new SHA pairs.
fn capture_pre_rewrite_commits(cfg: &Config) -> Vec<(String, String)> {
    let output = run_git_capture(cfg, &["log", "--format=%H %T", "HEAD"]);
    match output {
        Ok(text) => text
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(2, ' ');
                let hash = parts.next()?.to_string();
                let tree = parts.next()?.to_string();
                Some((hash, tree))
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_clone_dir_from_url() {
        let args = &["clone", "https://github.com/foo/bar.git"];
        assert_eq!(resolve_clone_dir(args), Some("bar".to_string()));
    }

    #[test]
    fn test_resolve_clone_dir_from_url_no_git_suffix() {
        let args = &["clone", "https://github.com/foo/bar"];
        assert_eq!(resolve_clone_dir(args), Some("bar".to_string()));
    }

    #[test]
    fn test_resolve_clone_dir_explicit_target() {
        let args = &["clone", "https://github.com/foo/bar.git", "my-dir"];
        assert_eq!(resolve_clone_dir(args), Some("my-dir".to_string()));
    }

    #[test]
    fn test_resolve_clone_dir_with_flags() {
        let args = &["clone", "--depth", "1", "https://github.com/foo/bar.git"];
        assert_eq!(resolve_clone_dir(args), Some("bar".to_string()));
    }

    #[test]
    fn test_resolve_clone_dir_with_branch_flag() {
        let args = &[
            "clone",
            "-b",
            "main",
            "https://github.com/foo/bar.git",
            "target",
        ];
        assert_eq!(resolve_clone_dir(args), Some("target".to_string()));
    }

    #[test]
    fn test_resolve_clone_dir_ssh() {
        let args = &["clone", "git@github.com:foo/bar.git"];
        assert_eq!(resolve_clone_dir(args), Some("bar".to_string()));
    }

    #[test]
    fn test_resolve_clone_dir_empty() {
        let args: &[&str] = &["clone"];
        assert_eq!(resolve_clone_dir(args), None);
    }

    #[test]
    fn test_is_clone() {
        assert!(is_clone(&["clone", "url"]));
        assert!(!is_clone(&["commit", "-m", "x"]));
        assert!(is_clone(&["-C", "/tmp", "clone", "url"]));
    }

    #[test]
    fn test_is_fetch() {
        assert!(is_fetch(&["fetch"]));
        assert!(is_fetch(&["fetch", "origin"]));
        assert!(!is_fetch(&["pull"]));
    }

    #[test]
    fn test_resolve_clone_dir_absolute_path() {
        let args = &["clone", "https://github.com/foo/bar.git", "/tmp/my-clone"];
        assert_eq!(resolve_clone_dir(args), Some("/tmp/my-clone".to_string()));
    }

    #[test]
    fn test_resolve_clone_dir_with_config_override() {
        let args = &[
            "clone",
            "-c",
            "core.autocrlf=true",
            "https://github.com/foo/bar.git",
        ];
        assert_eq!(resolve_clone_dir(args), Some("bar".to_string()));
    }

    #[test]
    fn test_resolve_clone_dir_depth_branch_target() {
        let args = &[
            "clone",
            "--depth",
            "1",
            "-b",
            "main",
            "https://github.com/foo/bar.git",
            "out",
        ];
        assert_eq!(resolve_clone_dir(args), Some("out".to_string()));
    }

    #[test]
    fn test_resolve_clone_dir_bare_flag() {
        let args = &["clone", "--bare", "https://github.com/foo/bar.git"];
        assert_eq!(resolve_clone_dir(args), Some("bar".to_string()));
    }
}
