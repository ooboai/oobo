use std::collections::BTreeMap;

use crate::config::Config;
use crate::core::anchor::{
    Anchor, AuthorType, Contributor, ContributorRole, FileAttribution, FileChange, LinkType,
    SessionLink, TransparencyMode,
};
use crate::git::{commands, detect, proxy};
use crate::hooks;
use crate::remote;
use crate::remote::payload::*;
use crate::tools::cursor;

/// Called after a write operation succeeds.
/// Logs event locally, creates anchor metadata, and optionally sends to cloud.
pub fn on_write_op(cfg: &Config, args: &[&str]) -> Result<(), String> {
    let op = commands::subcommand_name(args)
        .unwrap_or("unknown")
        .to_string();

    let project_root = proxy::project_root(cfg).unwrap_or_default();

    if !project_root.is_empty() {
        super::first_use::check_first_use(cfg, &project_root);
    }

    let project_name = std::path::Path::new(&project_root)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let branch = proxy::current_branch(cfg).unwrap_or_default();
    let git_context = collect_git_context(cfg, &op);

    let tools = collect_all_tool_context(cfg, &project_root, cfg.telemetry.send_transcripts);

    let payload = EventPayload {
        event: format!("git.{op}"),
        timestamp: chrono::Utc::now(),
        project: ProjectInfo {
            root: project_root.clone(),
            name: project_name,
        },
        git: GitInfo {
            operation: op.clone(),
            branch: branch.clone(),
            commit_hash: git_context.commit_hash.clone(),
            commit_message: git_context.commit_message.clone(),
            author: git_context.author.clone(),
            files_changed: git_context.files_changed,
            insertions: git_context.insertions,
            deletions: git_context.deletions,
        },
        tools,
    };

    log_event_locally(&project_root, &op, &payload);

    if op == "commit" || op == "merge" {
        if let Err(e) = enrich_commit(cfg, &project_root, &branch, &git_context) {
            eprintln!("oobo: warning: could not enrich commit: {e}");
        }
    }

    if !cfg.server.api_key.is_empty() {
        remote::send_event(cfg, &payload);
    }
    Ok(())
}

/// Create an Anchor (enriched commit primitive) and write to orphan branch.
fn enrich_commit(
    cfg: &Config,
    project_root: &str,
    branch: &str,
    git_context: &GitContext,
) -> Result<(), String> {
    if git_context.commit_hash.is_empty() {
        return Ok(());
    }

    let author_info = detect::detect(project_root);
    let author_type = match &author_info {
        detect::CommitAuthor::Agent { .. } => AuthorType::Agent,
        detect::CommitAuthor::Assisted { .. } => AuthorType::Assisted,
        detect::CommitAuthor::Human => AuthorType::Human,
        detect::CommitAuthor::Automated => AuthorType::Automated,
    };
    let active_sessions = hooks::state::active_sessions_for_worktree(project_root);

    let ai_files_touched = collect_ai_files_touched(cfg, project_root, &active_sessions);

    let session_links: Vec<SessionLink> = active_sessions
        .iter()
        .map(|s| {
            let touched: Vec<String> = ai_files_touched
                .iter()
                .filter(|(_, agent)| agent == &s.agent)
                .map(|(path, _)| path.clone())
                .collect();
            SessionLink {
                session_id: s.session_id.clone(),
                agent: s.agent.clone(),
                model: s.model.clone(),
                link_type: LinkType::Explicit,
                input_tokens: None,
                output_tokens: None,
                cache_read_tokens: None,
                cache_creation_tokens: None,
                duration_secs: None,
                tool_calls: None,
                files_touched: if touched.is_empty() {
                    None
                } else {
                    Some(touched)
                },
                is_subagent: false,
            }
        })
        .collect();

    let session_ids: Vec<String> = session_links.iter().map(|s| s.session_id.clone()).collect();

    let transparency = resolve_transparency(cfg, project_root);

    let per_file = collect_per_file_stats(cfg);
    let files_changed: Vec<String> = per_file.iter().map(|(p, _, _)| p.clone()).collect();

    let has_ai_sessions = !active_sessions.is_empty();
    let ai_file_set: std::collections::HashSet<&str> =
        ai_files_touched.iter().map(|(p, _)| p.as_str()).collect();

    let mut file_changes = Vec::new();
    let mut ai_added: u32 = 0;
    let mut ai_deleted: u32 = 0;
    let mut human_added: u32 = 0;
    let mut human_deleted: u32 = 0;

    let is_agent_commit = author_type == AuthorType::Agent;

    for (path, added, deleted) in &per_file {
        let (attribution, agent) = if ai_file_set.contains(path.as_str()) {
            let agent_name = ai_files_touched
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, a)| a.clone());
            (Some(FileAttribution::Ai), agent_name)
        } else if is_agent_commit && has_ai_sessions {
            let agent_name = active_sessions.first().map(|s| s.agent.clone());
            (Some(FileAttribution::Ai), agent_name)
        } else if has_ai_sessions {
            (Some(FileAttribution::Mixed), None)
        } else if is_agent_commit {
            (Some(FileAttribution::Ai), None)
        } else {
            (Some(FileAttribution::Human), None)
        };

        match attribution {
            Some(FileAttribution::Ai) => {
                ai_added += added;
                ai_deleted += deleted;
            }
            Some(FileAttribution::Human) => {
                human_added += added;
                human_deleted += deleted;
            }
            Some(FileAttribution::Mixed) => {
                let ai_add = added / 2;
                let ai_del = deleted / 2;
                ai_added += ai_add;
                ai_deleted += ai_del;
                human_added += added - ai_add;
                human_deleted += deleted - ai_del;
            }
            None => {
                human_added += added;
                human_deleted += deleted;
            }
        }

        file_changes.push(FileChange {
            path: path.clone(),
            added: *added,
            deleted: *deleted,
            attribution,
            agent,
        });
    }

    let total_lines = git_context.insertions + git_context.deletions;
    let ai_percentage = if total_lines > 0 {
        Some(((ai_added + ai_deleted) as f64 / total_lines as f64) * 100.0)
    } else {
        None
    };

    let mut contributors = vec![Contributor {
        name: git_context.author.clone(),
        role: ContributorRole::Human,
        model: None,
    }];
    for link in &session_links {
        if !contributors.iter().any(|c| c.name == link.agent) {
            contributors.push(Contributor {
                name: link.agent.clone(),
                role: ContributorRole::Agent,
                model: link.model.clone(),
            });
        }
    }

    let anchor = Anchor {
        oobo_version: Anchor::oobo_version().to_string(),
        commit_hash: git_context.commit_hash.clone(),
        branch: branch.to_string(),
        author: git_context.author.clone(),
        author_type,
        contributors,
        committed_at: chrono::Utc::now().timestamp(),
        message: git_context.commit_message.clone(),
        files_changed,
        added: git_context.insertions,
        deleted: git_context.deletions,
        file_changes,
        ai_added,
        ai_deleted,
        human_added,
        human_deleted,
        ai_percentage,
        session_ids,
        summary: None,
        intent: None,
        reasoning: None,
        transparency_mode: transparency,
    };

    let db = crate::db::Db::open().ok();
    if let Some(ref db) = db {
        let anchor_json = serde_json::to_string(&anchor).unwrap_or_default();
        if let Err(e) = db.insert_anchor(&anchor.commit_hash, &anchor_json) {
            eprintln!("oobo: warning: could not save anchor: {e}");
        }

        for link in &session_links {
            let lt = match link.link_type {
                LinkType::Explicit => "explicit",
                LinkType::Inferred => "inferred",
            };
            if let Err(e) = db.insert_anchor_session(
                &anchor.commit_hash,
                &link.session_id,
                &link.agent,
                link.model.as_deref(),
                lt,
                link.files_touched.as_deref(),
            ) {
                eprintln!("oobo: warning: could not save session link: {e}");
            }
        }
    }

    let transcripts = if transparency == TransparencyMode::On {
        collect_session_transcripts(&active_sessions, project_root)
    } else {
        Vec::new()
    };
    if let Err(e) = super::orphan::write_anchor(project_root, &anchor, &session_links, &transcripts)
    {
        eprintln!("oobo: warning: could not write anchor to orphan branch: {e}");
    }

    Ok(())
}

/// Collect per-file line stats from the most recent commit via `git diff-tree --numstat`.
fn collect_per_file_stats(cfg: &Config) -> Vec<(String, u32, u32)> {
    let output = proxy::run_git_capture(
        cfg,
        &["diff-tree", "--no-commit-id", "--numstat", "-r", "HEAD"],
    )
    .unwrap_or_default();

    output
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 3 {
                let added: u32 = parts[0].parse().unwrap_or(0);
                let deleted: u32 = parts[1].parse().unwrap_or(0);
                let path = parts[2].to_string();
                Some((path, added, deleted))
            } else {
                None
            }
        })
        .collect()
}

/// Collect files touched by AI tools during active sessions.
/// Returns (file_path, agent_name) pairs.
fn collect_ai_files_touched(
    cfg: &Config,
    project_root: &str,
    active_sessions: &[hooks::state::ActiveSession],
) -> Vec<(String, String)> {
    if active_sessions.is_empty() {
        return Vec::new();
    }

    let registry = crate::tools::registry();
    let mut result: Vec<(String, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for session in active_sessions {
        for tool in registry.all() {
            let is_match = tool.name() == session.agent
                || (session.agent == "cursor" && tool.name() == "cursor");

            if !is_match {
                continue;
            }

            if !tool.enabled(cfg) {
                break;
            }

            // Try to get files_touched from the tool's native stats
            if let Ok(sessions) = tool.sessions_for_project(project_root) {
                if let Some(tool_session) =
                    sessions.iter().find(|s| s.session_id == session.session_id)
                {
                    if let Some(stats) = tool.extract_native_stats(tool_session) {
                        for file in &stats.files_touched {
                            let normalized = normalize_path(file, project_root);
                            if seen.insert(normalized.clone()) {
                                result.push((normalized, session.agent.clone()));
                            }
                        }
                    }
                }
            }

            break;
        }
    }

    result
}

/// Normalize a file path to be relative to the project root.
fn normalize_path(file_path: &str, project_root: &str) -> String {
    let path = std::path::Path::new(file_path);
    if path.is_absolute() {
        path.strip_prefix(project_root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| file_path.to_string())
    } else {
        file_path.to_string()
    }
}

fn log_event_locally(project_root: &str, op: &str, payload: &EventPayload) {
    let db = match crate::db::Db::open() {
        Ok(db) => db,
        Err(_) => return,
    };

    let project_id = if project_root.is_empty() {
        None
    } else {
        let slug = crate::paths::slug_from_path(project_root);
        let _ = db.ensure_project(&slug, project_root);
        Some(slug)
    };

    let data = serde_json::to_string(payload).ok();

    if let Err(e) = db.insert_event(&crate::db::events::EventRow {
        id: None,
        event: format!("git.{op}"),
        project_id,
        timestamp: chrono::Utc::now().timestamp(),
        data,
        synced: false,
    }) {
        eprintln!("oobo: warning: could not save event: {e}");
    }
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

pub fn collect_all_tool_context(
    cfg: &Config,
    project_root: &str,
    include_transcripts: bool,
) -> BTreeMap<String, ToolContext> {
    let mut tools = BTreeMap::new();

    macro_rules! collect_tool {
        ($enabled:expr, $name:expr, $sessions:expr, $count_fn:expr, $source:expr) => {
            if $enabled {
                let sessions = $sessions;
                let transcript = if include_transcripts {
                    load_transcript(&sessions, $source)
                } else {
                    None
                };
                if let Some(ctx) = tool_context_from_sessions(&sessions, $count_fn, transcript) {
                    tools.insert($name.into(), ctx);
                }
            }
        };
    }

    macro_rules! collect_tool_with_stats {
        ($enabled:expr, $name:expr, $sessions:expr, $count_fn:expr, $stats_fn:expr, $source:expr) => {
            if $enabled {
                let sessions = $sessions;
                let transcript = if include_transcripts {
                    load_transcript(&sessions, $source)
                } else {
                    None
                };
                if let Some(ctx) = tool_context_from_sessions_with_stats(
                    &sessions, $count_fn, $stats_fn, transcript,
                ) {
                    tools.insert($name.into(), ctx);
                }
            }
        };
    }

    collect_tool!(
        cfg.cursor.enabled,
        "cursor",
        cursor::sessions_for_project(project_root).unwrap_or_default(),
        cursor::transcript::count_messages,
        "cursor"
    );
    collect_tool_with_stats!(
        cfg.claude.enabled,
        "claude",
        crate::tools::claude::sessions_for_project(project_root).unwrap_or_default(),
        crate::tools::claude::transcript::count_messages,
        crate::tools::claude::transcript::stats_for_session,
        "claude"
    );
    collect_tool!(
        cfg.windsurf.enabled,
        "windsurf",
        crate::tools::windsurf::sessions_for_project(project_root).unwrap_or_default(),
        crate::tools::windsurf::transcript::count_messages,
        "windsurf"
    );
    collect_tool!(
        cfg.trae.enabled,
        "trae",
        crate::tools::trae::sessions_for_project(project_root).unwrap_or_default(),
        crate::tools::trae::transcript::count_messages,
        "trae"
    );
    collect_tool!(
        cfg.aider.enabled,
        "aider",
        crate::tools::aider::sessions_for_project(project_root).unwrap_or_default(),
        crate::tools::aider::transcript::count_messages,
        "aider"
    );
    collect_tool_with_stats!(
        cfg.copilot.enabled,
        "copilot",
        crate::tools::copilot::sessions_for_project(project_root).unwrap_or_default(),
        crate::tools::copilot::transcript::count_messages,
        crate::tools::copilot::transcript::stats_for_session,
        "copilot"
    );
    collect_tool!(
        cfg.zed.enabled,
        "zed",
        crate::tools::zed::sessions_for_project(project_root).unwrap_or_default(),
        crate::tools::zed::transcript::count_messages,
        "zed"
    );
    collect_tool_with_stats!(
        cfg.codex.enabled,
        "codex",
        crate::tools::codex::sessions_for_project(project_root).unwrap_or_default(),
        crate::tools::codex::transcript::count_messages,
        crate::tools::codex::transcript::stats_for_session,
        "codex"
    );
    collect_tool_with_stats!(
        cfg.opencode.enabled,
        "opencode",
        crate::tools::opencode::sessions_for_project(project_root).unwrap_or_default(),
        crate::tools::opencode::transcript::count_messages,
        crate::tools::opencode::transcript::stats_for_session,
        "opencode"
    );

    tools
}

/// Collect raw transcript content for active sessions (for full-transparency mode).
/// Reads the original JSONL/text file so tool calls and structure are preserved.
/// Prefers the `transcript_path` stored in the session state file (set by the
/// `stop` hook), falling back to the tool registry's `find_transcript`.
fn collect_session_transcripts(
    sessions: &[crate::hooks::state::ActiveSession],
    project_root: &str,
) -> Vec<(String, String)> {
    let mut transcripts = Vec::new();

    for session in sessions {
        let raw = session
            .transcript_path
            .as_deref()
            .and_then(|tp| std::fs::read_to_string(tp).ok())
            .filter(|c| !c.is_empty());

        if let Some(content) = raw {
            transcripts.push((session.session_id.clone(), content));
            continue;
        }

        let registry = crate::tools::registry();
        for tool in registry.all() {
            if tool.name() == session.agent
                || (session.agent == "cursor" && tool.name() == "composer")
            {
                if let Some(path) = tool.find_transcript(project_root, &session.session_id) {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if !content.is_empty() {
                            transcripts.push((session.session_id.clone(), content));
                        }
                    }
                }
                break;
            }
        }
    }
    transcripts
}

/// Resolve the effective transparency mode for a project. Per-project settings
/// in the DB override the global config default.
fn resolve_transparency(cfg: &Config, project_root: &str) -> crate::core::anchor::TransparencyMode {
    if let Ok(db) = crate::db::Db::open() {
        if let Ok(settings) = db.get_project_settings_by_path(project_root) {
            if let Some(ref mode) = settings.transparency {
                return match mode.as_str() {
                    "on" | "full" | "full_transparency" => {
                        crate::core::anchor::TransparencyMode::On
                    }
                    _ => crate::core::anchor::TransparencyMode::Off,
                };
            }
        }
    }
    cfg.transparency_mode()
}

fn load_transcript(
    sessions: &[crate::tools::cursor::Session],
    source: &str,
) -> Option<Vec<TranscriptMessage>> {
    if sessions.is_empty() {
        return None;
    }
    let recent = &sessions[0];
    let path = crate::session::find_transcript_path(recent)?;
    let messages = crate::session::parse_messages(&path, source);
    if messages.is_empty() {
        return None;
    }
    Some(
        messages
            .into_iter()
            .map(|m| TranscriptMessage {
                role: m.role,
                text: m.text,
            })
            .collect(),
    )
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
