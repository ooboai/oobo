use std::collections::BTreeMap;

use crate::config::Config;
use crate::cursor;
use crate::git::{commands, proxy};
use crate::server;
use crate::server::payload::*;

/// Called after a write operation succeeds.
/// Collects git + AI tool context and sends to the dashboard.
pub fn on_write_op(cfg: &Config, args: &[&str]) -> Result<(), String> {
    if cfg.server.api_key.is_empty() {
        return Ok(());
    }

    let op = commands::subcommand_name(args)
        .unwrap_or("unknown")
        .to_string();

    let project_root = proxy::project_root(cfg).unwrap_or_default();
    let project_name = std::path::Path::new(&project_root)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let branch = proxy::current_branch(cfg).unwrap_or_default();
    let git_context = collect_git_context(cfg, &op);

    let tools = collect_all_tool_context(cfg, &project_root);

    let payload = EventPayload {
        event: format!("git.{op}"),
        timestamp: chrono::Utc::now(),
        project: ProjectInfo {
            root: project_root,
            name: project_name,
        },
        git: GitInfo {
            operation: op,
            branch,
            commit_hash: git_context.commit_hash,
            commit_message: git_context.commit_message,
            author: git_context.author,
            files_changed: git_context.files_changed,
            insertions: git_context.insertions,
            deletions: git_context.deletions,
        },
        tools,
    };

    server::send_event(cfg, &payload);
    Ok(())
}

struct GitContext {
    commit_hash: String,
    commit_message: String,
    author: String,
    files_changed: u32,
    insertions: u32,
    deletions: u32,
}

fn collect_git_context(cfg: &Config, op: &str) -> GitContext {
    let mut ctx = GitContext {
        commit_hash: String::new(),
        commit_message: String::new(),
        author: String::new(),
        files_changed: 0,
        insertions: 0,
        deletions: 0,
    };

    if op == "commit" || op == "merge" || op == "cherry-pick" || op == "revert" {
        ctx.commit_hash = proxy::run_git_capture(cfg, &["rev-parse", "HEAD"]).unwrap_or_default();
        ctx.commit_message =
            proxy::run_git_capture(cfg, &["log", "-1", "--format=%s"]).unwrap_or_default();
        ctx.author =
            proxy::run_git_capture(cfg, &["log", "-1", "--format=%an <%ae>"]).unwrap_or_default();

        if let Ok(stat) = proxy::run_git_capture(cfg, &["diff", "--shortstat", "HEAD~1", "HEAD"]) {
            parse_shortstat(&stat, &mut ctx);
        }
    }

    ctx
}

fn parse_shortstat(stat: &str, ctx: &mut GitContext) {
    for part in stat.split(',') {
        let part = part.trim();
        if part.contains("file") {
            if let Some(n) = part.split_whitespace().next() {
                ctx.files_changed = n.parse().unwrap_or(0);
            }
        } else if part.contains("insertion") {
            if let Some(n) = part.split_whitespace().next() {
                ctx.insertions = n.parse().unwrap_or(0);
            }
        } else if part.contains("deletion") {
            if let Some(n) = part.split_whitespace().next() {
                ctx.deletions = n.parse().unwrap_or(0);
            }
        }
    }
}

pub fn collect_all_tool_context(cfg: &Config, project_root: &str) -> BTreeMap<String, ToolContext> {
    let mut tools = BTreeMap::new();

    macro_rules! collect_tool {
        ($enabled:expr, $name:expr, $sessions:expr, $count_fn:expr) => {
            if $enabled {
                let sessions = $sessions;
                if let Some(ctx) = tool_context_from_sessions(&sessions, $count_fn) {
                    tools.insert($name.into(), ctx);
                }
            }
        };
    }

    macro_rules! collect_tool_with_stats {
        ($enabled:expr, $name:expr, $sessions:expr, $count_fn:expr, $stats_fn:expr) => {
            if $enabled {
                let sessions = $sessions;
                if let Some(ctx) =
                    tool_context_from_sessions_with_stats(&sessions, $count_fn, $stats_fn)
                {
                    tools.insert($name.into(), ctx);
                }
            }
        };
    }

    collect_tool!(
        cfg.cursor.enabled,
        "cursor",
        cursor::sessions_for_project(project_root).unwrap_or_default(),
        cursor::transcript::count_messages
    );
    collect_tool_with_stats!(
        cfg.claude.enabled,
        "claude",
        crate::claude::sessions_for_project(project_root).unwrap_or_default(),
        crate::claude::transcript::count_messages,
        crate::claude::transcript::stats_for_session
    );
    collect_tool!(
        cfg.windsurf.enabled,
        "windsurf",
        crate::windsurf::sessions_for_project(project_root).unwrap_or_default(),
        crate::windsurf::transcript::count_messages
    );
    collect_tool!(
        cfg.trae.enabled,
        "trae",
        crate::trae::sessions_for_project(project_root).unwrap_or_default(),
        crate::trae::transcript::count_messages
    );
    collect_tool!(
        cfg.aider.enabled,
        "aider",
        crate::aider::sessions_for_project(project_root).unwrap_or_default(),
        crate::aider::transcript::count_messages
    );
    collect_tool!(
        cfg.continue_dev.enabled,
        "continue",
        crate::continue_dev::sessions_for_project(project_root).unwrap_or_default(),
        crate::continue_dev::transcript::count_messages
    );
    collect_tool_with_stats!(
        cfg.copilot.enabled,
        "copilot",
        crate::copilot::sessions_for_project(project_root).unwrap_or_default(),
        crate::copilot::transcript::count_messages,
        crate::copilot::transcript::stats_for_session
    );
    collect_tool!(
        cfg.zed.enabled,
        "zed",
        crate::zed::sessions_for_project(project_root).unwrap_or_default(),
        crate::zed::transcript::count_messages
    );
    collect_tool_with_stats!(
        cfg.codex.enabled,
        "codex",
        crate::codex::sessions_for_project(project_root).unwrap_or_default(),
        crate::codex::transcript::count_messages,
        crate::codex::transcript::stats_for_session
    );
    collect_tool_with_stats!(
        cfg.opencode.enabled,
        "opencode",
        crate::opencode::sessions_for_project(project_root).unwrap_or_default(),
        crate::opencode::transcript::count_messages,
        crate::opencode::transcript::stats_for_session
    );

    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_shortstat() {
        let mut ctx = GitContext {
            commit_hash: String::new(),
            commit_message: String::new(),
            author: String::new(),
            files_changed: 0,
            insertions: 0,
            deletions: 0,
        };

        parse_shortstat(
            " 3 files changed, 42 insertions(+), 10 deletions(-)",
            &mut ctx,
        );
        assert_eq!(ctx.files_changed, 3);
        assert_eq!(ctx.insertions, 42);
        assert_eq!(ctx.deletions, 10);
    }

    #[test]
    fn test_parse_shortstat_insert_only() {
        let mut ctx = GitContext {
            commit_hash: String::new(),
            commit_message: String::new(),
            author: String::new(),
            files_changed: 0,
            insertions: 0,
            deletions: 0,
        };

        parse_shortstat(" 1 file changed, 5 insertions(+)", &mut ctx);
        assert_eq!(ctx.files_changed, 1);
        assert_eq!(ctx.insertions, 5);
        assert_eq!(ctx.deletions, 0);
    }
}
