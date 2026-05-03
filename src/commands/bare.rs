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
    if mode == OutputMode::Tui { crate::tui::app::run(cfg) } else {
        crate::commands::anchors::run(cfg, 50, mode)?;
        Ok(0)
    }
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