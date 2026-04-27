//! Bare `oobo` (no subcommand) — four-quadrant behavior:
//!
//! - Inside a repo: delegates to `anchors` (with limit=50) for agent/json,
//!   or an anchor-feed TUI for pretty (Phase 3.8 will introduce a proper
//!   unified TUI; for v1 we fall back to the anchors list).
//! - Outside a repo: shows a cross-project listing.

use crate::cli::OutputMode;
use crate::config::Config;

const BARE_LIMIT: usize = 50;

pub fn run(cfg: &Config, mode: OutputMode) -> Result<i32, String> {
    match crate::git::proxy::project_root(cfg) {
        Some(_) => in_repo(cfg, mode),
        None => cross_project(cfg, mode),
    }
}

fn in_repo(cfg: &Config, mode: OutputMode) -> Result<i32, String> {
    match mode {
        OutputMode::Tui => crate::tui::app::run(cfg),
        _ => {
            crate::commands::anchors::run(cfg, BARE_LIMIT, mode)?;
            Ok(0)
        }
    }
}

fn cross_project(_cfg: &Config, mode: OutputMode) -> Result<i32, String> {
    match mode {
        OutputMode::Json => {
            let json = serde_json::json!({
                "projects": [],
                "stats": { "projects": 0, "anchors": 0, "tokens": 0, "ai_pct": 0 },
            });
            crate::utils::print_json(&json);
        }
        OutputMode::Agent => {
            println!("anchor: not a git repository. cd into a project and run `anchor` to see your anchor feed.");
        }
        OutputMode::Tui => {
            println!("anchor: not a git repository. cd into a project and run `anchor` to see your anchor feed.");
        }
    }
    Ok(0)
}
