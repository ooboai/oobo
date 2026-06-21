//! Bare `oobo` (no subcommand):
//!
//! - Inside a repo: TUI anchor feed (pretty) or list (agent/json).
//! - Outside a repo: hint to cd into a repo.

use crate::cli::OutputMode;
use crate::config::Config;
use crate::error::CmdResult;

pub fn run(cfg: &Config, mode: OutputMode) -> CmdResult {
    match crate::git::proxy::project_root(cfg) {
        Some(_) => in_repo(cfg, mode),
        None => outside_repo(mode),
    }
}

fn in_repo(cfg: &Config, mode: OutputMode) -> CmdResult {
    emit_last_drop_warning(mode);
    if mode == OutputMode::Tui {
        crate::tui::app::run(cfg)
    } else {
        crate::commands::anchors::run(cfg, 50, mode)?;
        Ok(0)
    }
}

/// If an anchor was recently dropped (e.g. due to secret detection),
/// surface a one-time warning so the user knows.
fn emit_last_drop_warning(mode: OutputMode) {
    let path = crate::paths::oobo_home()
        .join("state")
        .join("last-drop.json");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };
    let sha = parsed["commit"].as_str().unwrap_or("unknown");
    let reason = parsed["reason"].as_str().unwrap_or("unknown");
    let repo = parsed["repo"].as_str().unwrap_or("unknown");
    match mode {
        OutputMode::Tui => {
            eprintln!(
                "\x1b[33mwarning:\x1b[0m anchor for commit {} in {} was dropped ({}). \
                 Remove secrets and re-commit to capture it.",
                &sha[..sha.len().min(7)],
                repo,
                reason.replace('_', " "),
            );
        }
        OutputMode::Agent => {
            eprintln!(
                "warning: anchor for {} dropped ({})",
                &sha[..sha.len().min(7)],
                reason.replace('_', " "),
            );
        }
        OutputMode::Json => {
            // JSON consumers can read last-drop.json directly; no inline warning.
        }
    }
    // Clear after showing so the warning is one-time.
    let _ = std::fs::remove_file(&path);
}

#[allow(clippy::unnecessary_wraps)]
fn outside_repo(mode: OutputMode) -> CmdResult {
    if mode == OutputMode::Json {
        let json = serde_json::json!({ "error": "not inside a git repository" });
        crate::utils::print_json(&json);
    } else {
        eprintln!("oobo: not inside a git repository.");
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_repo_returns_exit_code_1() {
        let code = outside_repo(OutputMode::Agent).unwrap();
        assert_eq!(code, 1);
    }

    #[test]
    fn outside_repo_json_returns_exit_code_1() {
        let code = outside_repo(OutputMode::Json).unwrap();
        assert_eq!(code, 1);
    }

    #[test]
    fn outside_repo_tui_returns_exit_code_1() {
        let code = outside_repo(OutputMode::Tui).unwrap();
        assert_eq!(code, 1);
    }
}
