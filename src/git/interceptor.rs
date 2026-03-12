use crate::config::Config;
use crate::core::anchor::{
    Anchor, AuthorType, Contributor, ContributorRole, FileAttribution, FileChange, LinkType,
    SessionLink, TransparencyMode,
};
use crate::git::{commands, detect, proxy};
use crate::hooks;
use crate::redact;
use crate::remote;
use crate::remote::payload;

/// Anchor + linked sessions + raw transcript texts (session_id, content).
type EnrichResult = (Anchor, Vec<SessionLink>, Vec<(String, String)>);

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

    log_event_locally(&project_root, &op, &git_context);

    let mut anchor_data: Option<EnrichResult> = None;

    if op == "commit" || op == "merge" {
        match enrich_commit(cfg, &project_root, &branch, &git_context) {
            Ok(data) => anchor_data = data,
            Err(e) => eprintln!("oobo: warning: could not enrich commit: {e}"),
        }
    }

    let project_sync = crate::commands::sync::resolve_project_sync(cfg);
    let effective_key = crate::commands::sync::resolve_api_key(cfg);
    let should_sync = project_sync.unwrap_or(cfg.server.sync) && !effective_key.is_empty();

    if should_sync {
        let git_remote = resolve_git_remote(cfg)
            .or_else(|| Some(String::new()));

        let (anchor_payload, transcript) = if let Some((anchor, links, transcripts)) = anchor_data {
            let transcript_messages =
                if anchor.transparency_mode == TransparencyMode::On && !transcripts.is_empty() {
                    transcripts
                        .into_iter()
                        .flat_map(|(_session_id, text)| {
                            let redacted = redact::redact(&text);
                            parse_transcript_messages(&redacted)
                        })
                        .collect()
                } else {
                    Vec::new()
                };

            (
                Some(payload::AnchorPayload {
                    anchor,
                    sessions: links,
                }),
                transcript_messages,
            )
        } else {
            (None, Vec::new())
        };

        let payload = payload::EventPayload {
            event: format!("git.{op}"),
            timestamp: chrono::Utc::now(),
            oobo_version: env!("CARGO_PKG_VERSION").to_string(),
            project: payload::ProjectInfo {
                name: project_name,
                git_remote,
            },
            anchor: anchor_payload,
            transcript,
        };

        let handle = remote::send_event(cfg, &payload, Some(&effective_key));
        let _ = handle.join();
    }

    Ok(())
}

/// Create an Anchor (enriched commit primitive) and write to orphan branch.
/// Returns the anchor, session links, and transcripts for the backend payload.
fn enrich_commit(
    cfg: &Config,
    project_root: &str,
    branch: &str,
    git_context: &GitContext,
) -> Result<Option<EnrichResult>, String> {
    if git_context.commit_hash.is_empty() {
        return Ok(None);
    }

    let author_info = detect::detect(project_root);
    let initial_author_type = match &author_info {
        detect::CommitAuthor::Agent { .. } => AuthorType::Agent,
        detect::CommitAuthor::Assisted { .. } => AuthorType::Assisted,
        detect::CommitAuthor::Human => AuthorType::Human,
        detect::CommitAuthor::Automated => AuthorType::Automated,
    };
    let per_file = collect_per_file_stats(cfg);
    let files_changed: Vec<String> = per_file.iter().map(|(p, _, _)| p.clone()).collect();
    let parent_commit_epoch = parent_commit_timestamp(cfg);

    let all_sessions = hooks::state::active_sessions_for_worktree(project_root);
    let active_sessions = filter_relevant_sessions(
        &all_sessions,
        project_root,
        &files_changed,
        parent_commit_epoch,
    );

    let ai_files_touched =
        collect_ai_files_touched(cfg, project_root, &active_sessions, parent_commit_epoch);

    // Recalculate author_type: if detect said "assisted" but no sessions
    // are actually relevant to this commit (no file overlap), downgrade to human.
    let author_type = if initial_author_type == AuthorType::Assisted
        && active_sessions.is_empty()
        && ai_files_touched.is_empty()
    {
        AuthorType::Human
    } else {
        initial_author_type
    };

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
                is_estimated: false,
            }
        })
        .collect();

    let session_ids: Vec<String> = session_links.iter().map(|s| s.session_id.clone()).collect();

    let transparency = resolve_transparency(cfg, project_root);

    let has_ai_sessions = !active_sessions.is_empty();
    let ai_file_set: std::collections::HashSet<&str> =
        ai_files_touched.iter().map(|(p, _)| p.as_str()).collect();

    let (snapshot_lookup, pre_agent_lookup) =
        build_snapshot_lookups(&active_sessions, &ai_files_touched);

    let mut file_changes = Vec::new();
    let mut ai_added: u32 = 0;
    let mut ai_deleted: u32 = 0;
    let mut human_added: u32 = 0;
    let mut human_deleted: u32 = 0;

    let is_agent_commit = author_type == AuthorType::Agent;

    for (path, added, deleted) in &per_file {
        let pre_blob = pre_agent_lookup.get(path.as_str()).cloned();
        let (file_ai_add, file_ai_del, file_human_add, file_human_del, attribution, agent) =
            if let Some((agent_blob, agent_name)) = snapshot_lookup.get(path.as_str()) {
                compute_precise_attribution(
                    cfg,
                    path,
                    agent_blob,
                    pre_blob.as_deref(),
                    *added,
                    *deleted,
                    agent_name.clone(),
                )
            } else if ai_file_set.contains(path.as_str()) {
                // AI edited the file (from bubble data) but no snapshot available.
                let agent_name = ai_files_touched
                    .iter()
                    .find(|(p, _)| p == path)
                    .map(|(_, a)| a.clone());
                if is_agent_commit {
                    (
                        *added,
                        *deleted,
                        0,
                        0,
                        Some(FileAttribution::Ai),
                        agent_name,
                    )
                } else {
                    // No snapshot — honest 50/50 split as best estimate.
                    let ai_a = *added / 2;
                    let ai_d = *deleted / 2;
                    (
                        ai_a,
                        ai_d,
                        added - ai_a,
                        deleted - ai_d,
                        Some(FileAttribution::Mixed),
                        agent_name,
                    )
                }
            } else if is_agent_commit && !has_ai_sessions {
                (*added, *deleted, 0, 0, Some(FileAttribution::Ai), None)
            } else {
                (0, 0, *added, *deleted, Some(FileAttribution::Human), None)
            };

        ai_added += file_ai_add;
        ai_deleted += file_ai_del;
        human_added += file_human_add;
        human_deleted += file_human_del;

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

    Ok(Some((anchor, session_links, transcripts)))
}

type PostAgentLookup<'a> = std::collections::HashMap<&'a str, (String, Option<String>)>;
type PreAgentLookup<'a> = std::collections::HashMap<&'a str, String>;

/// Build lookups from file path → blob hash for both pre-agent and post-agent snapshots.
fn build_snapshot_lookups<'a>(
    sessions: &'a [hooks::state::ActiveSession],
    ai_files_touched: &'a [(String, String)],
) -> (PostAgentLookup<'a>, PreAgentLookup<'a>) {
    let mut post_agent: std::collections::HashMap<&str, (String, Option<String>)> =
        std::collections::HashMap::new();
    let mut pre_agent: std::collections::HashMap<&str, String> = std::collections::HashMap::new();

    for session in sessions {
        if let Some(ref snapshots) = session.file_snapshots {
            for (file, blob_hash) in snapshots {
                let agent_name = ai_files_touched
                    .iter()
                    .find(|(p, _)| p == file)
                    .map(|(_, a)| a.clone());
                post_agent.insert(file.as_str(), (blob_hash.clone(), agent_name));
            }
        }
        if let Some(ref snapshots) = session.pre_agent_snapshots {
            for (file, blob_hash) in snapshots {
                pre_agent.insert(file.as_str(), blob_hash.clone());
            }
        }
    }

    (post_agent, pre_agent)
}

/// Compute exact AI vs human line counts for a file using blob diffs.
///
/// Uses three reference points:
/// - `pre_agent_blob`: file state before the agent started (from `before-submit-prompt`)
/// - `agent_blob`: file state after the agent finished (from `stop`)
/// - committed blob: what was actually committed (`HEAD:{file}`)
///
/// AI contribution = diff(pre_agent → agent_blob)
/// Human contribution = diff(agent_blob → committed)
///
/// Falls back to parent commit blob if pre_agent_blob isn't available.
fn compute_precise_attribution(
    cfg: &Config,
    file_path: &str,
    agent_blob: &str,
    pre_agent_blob: Option<&str>,
    total_added: u32,
    total_deleted: u32,
    agent_name: Option<String>,
) -> (u32, u32, u32, u32, Option<FileAttribution>, Option<String>) {
    let committed_blob = proxy::run_git_capture(cfg, &["rev-parse", &format!("HEAD:{file_path}")])
        .unwrap_or_default()
        .trim()
        .to_string();

    // If agent blob matches committed blob → pure AI (human didn't change it after the agent)
    if !committed_blob.is_empty() && agent_blob == committed_blob {
        return (
            total_added,
            total_deleted,
            0,
            0,
            Some(FileAttribution::Ai),
            agent_name,
        );
    }

    // Use pre-agent snapshot if available, otherwise fall back to parent commit blob
    let baseline_blob = if let Some(pre) = pre_agent_blob {
        pre.to_string()
    } else {
        proxy::run_git_capture(cfg, &["rev-parse", &format!("HEAD~1:{file_path}")])
            .unwrap_or_default()
            .trim()
            .to_string()
    };

    // If agent blob matches baseline → AI changed nothing → pure human
    if !baseline_blob.is_empty() && agent_blob == baseline_blob {
        return (
            0,
            0,
            total_added,
            total_deleted,
            Some(FileAttribution::Human),
            None,
        );
    }

    // AI contribution: baseline → agent_blob
    let ai_stats = if baseline_blob.is_empty() {
        // New file: baseline doesn't exist. Count lines in the agent's blob.
        count_blob_lines(cfg, agent_blob).map(|n| (n, 0))
    } else {
        diff_blobs_numstat(cfg, &baseline_blob, agent_blob)
    };
    // Human contribution: agent_blob → committed
    let human_stats = diff_blobs_numstat(cfg, agent_blob, &committed_blob);

    let (ai_add, ai_del) = ai_stats.unwrap_or((total_added, total_deleted));
    let (human_add, human_del) = human_stats.unwrap_or((0, 0));

    // Clamp to total
    let ai_add = ai_add.min(total_added);
    let ai_del = ai_del.min(total_deleted);
    let human_add = human_add.min(total_added.saturating_sub(ai_add));
    let human_del = human_del.min(total_deleted.saturating_sub(ai_del));

    let attribution = if human_add == 0 && human_del == 0 {
        Some(FileAttribution::Ai)
    } else if ai_add == 0 && ai_del == 0 {
        Some(FileAttribution::Human)
    } else {
        Some(FileAttribution::Mixed)
    };

    (
        ai_add,
        ai_del,
        human_add,
        human_del,
        attribution,
        agent_name,
    )
}

/// Count lines in a git blob object.
fn count_blob_lines(cfg: &Config, blob: &str) -> Option<u32> {
    if blob.is_empty() {
        return None;
    }
    let content = proxy::run_git_capture(cfg, &["cat-file", "-p", blob]).ok()?;
    Some(content.lines().count() as u32)
}

/// Diff two blob objects and return (added, deleted) line counts.
fn diff_blobs_numstat(cfg: &Config, blob_a: &str, blob_b: &str) -> Option<(u32, u32)> {
    if blob_a.is_empty() || blob_b.is_empty() {
        return None;
    }

    let output = proxy::run_git_capture(cfg, &["diff", "--numstat", blob_a, blob_b]).ok()?;

    // Output format: "added\tdeleted\t" (no filename for blob diffs)
    let line = output.lines().next()?;
    let parts: Vec<&str> = line.split('\t').collect();
    if parts.len() >= 2 {
        let added: u32 = parts[0].parse().unwrap_or(0);
        let deleted: u32 = parts[1].parse().unwrap_or(0);
        Some((added, deleted))
    } else {
        None
    }
}

/// Get the parent commit's author timestamp (Unix epoch seconds).
/// Returns 0 if there's no parent (initial commit).
fn parent_commit_timestamp(cfg: &Config) -> i64 {
    let output = proxy::run_git_capture(cfg, &["log", "-1", "--format=%at", "HEAD~1"]);
    output
        .unwrap_or_default()
        .trim()
        .parse::<i64>()
        .unwrap_or(0)
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
///
/// For Cursor sessions, reads `edit_file_v2` tool calls from the bubble DB
/// which gives us exact file paths. Falls back to the tool registry for
/// non-Cursor tools.
fn collect_ai_files_touched(
    cfg: &Config,
    project_root: &str,
    active_sessions: &[hooks::state::ActiveSession],
    since_epoch: i64,
) -> Vec<(String, String)> {
    if active_sessions.is_empty() {
        return Vec::new();
    }

    let mut result: Vec<(String, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for session in active_sessions {
        // Session file_snapshots are the strongest signal: the hooks captured
        // exactly which files the agent edited. Check these first.
        if let Some(ref snapshots) = session.file_snapshots {
            for file in snapshots.keys() {
                if seen.insert(file.clone()) {
                    result.push((file.clone(), session.agent.clone()));
                }
            }
        }

        if is_cursor_session(&session.agent) {
            let files = crate::tools::cursor::composer_data::files_edited_in_session(
                &session.session_id,
                project_root,
                since_epoch,
            );
            for file in files {
                if seen.insert(file.clone()) {
                    result.push((file, session.agent.clone()));
                }
            }
            continue;
        }

        let registry = crate::tools::registry();
        for tool in registry.all() {
            if !is_agent_tool_match(&session.agent, tool.name()) {
                continue;
            }

            if !tool.enabled(cfg) {
                break;
            }

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

/// Filter active sessions to only those relevant to the current commit.
///
/// Primary signal: **file overlap** — if a session edited files that appear in
/// the commit, it's linked. This is much more reliable than timestamps because
/// we know exactly which files each Cursor session touched via `edit_file_v2`
/// bubbles.
///
/// Fallback (non-Cursor tools or when DB is unavailable): recency-based
/// heuristic — most recently updated session within a reasonable window.
///
/// When multiple sessions touch the same file, all are included (they both
/// contributed).
/// `since_epoch`: only count edits after this timestamp (parent commit time).
fn filter_relevant_sessions(
    sessions: &[crate::hooks::state::ActiveSession],
    project_root: &str,
    committed_files: &[String],
    since_epoch: i64,
) -> Vec<crate::hooks::state::ActiveSession> {
    if sessions.is_empty() {
        return Vec::new();
    }

    let committed_set: std::collections::HashSet<&str> =
        committed_files.iter().map(|s| s.as_str()).collect();

    let mut matched_by_files: Vec<crate::hooks::state::ActiveSession> = Vec::new();
    let mut unresolved: Vec<&crate::hooks::state::ActiveSession> = Vec::new();

    for session in sessions {
        if is_cursor_session(&session.agent) {
            let edited = crate::tools::cursor::composer_data::files_edited_in_session(
                &session.session_id,
                project_root,
                since_epoch,
            );
            let has_overlap = edited.iter().any(|f| committed_set.contains(f.as_str()));
            if has_overlap {
                matched_by_files.push(session.clone());
            }
            // If a Cursor session edited files but none overlap, skip it —
            // it was working on something else.
            // If it edited zero files (e.g. DB unavailable), fall through
            // to recency.
            if edited.is_empty() && !has_overlap {
                unresolved.push(session);
            }
        } else {
            unresolved.push(session);
        }
    }

    if !matched_by_files.is_empty() {
        let now = chrono::Utc::now().timestamp();
        for s in &unresolved {
            if now - s.updated_at < 120 {
                matched_by_files.push((*s).clone());
            }
        }
        return matched_by_files;
    }

    // No file-based matches — fall back to recency heuristic.
    // This handles: single session, non-Cursor tools, DB unavailable.
    filter_by_recency(sessions)
}

/// Recency-based fallback when file overlap data is unavailable.
fn filter_by_recency(
    sessions: &[crate::hooks::state::ActiveSession],
) -> Vec<crate::hooks::state::ActiveSession> {
    let now = chrono::Utc::now().timestamp();

    let mut sorted: Vec<&crate::hooks::state::ActiveSession> = sessions.iter().collect();
    sorted.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    let most_recent = sorted[0].updated_at;

    if now - most_recent < 120 {
        return sorted
            .into_iter()
            .filter(|s| most_recent - s.updated_at < 10)
            .cloned()
            .collect();
    }

    sorted
        .into_iter()
        .filter(|s| now - s.updated_at < 1800)
        .cloned()
        .collect()
}

/// Match a session's agent name to a tool's canonical name.
/// Handles Cursor's various mode names ("agent", "composer", etc.) which
/// are now normalized at hook time, plus legacy session files with old names.
fn is_agent_tool_match(session_agent: &str, tool_name: &str) -> bool {
    if session_agent == tool_name {
        return true;
    }
    let cursor_aliases = [
        "cursor", "agent", "composer", "ask", "edit", "normal", "chat",
    ];
    let cursor_tools = ["composer", "cursor"];
    cursor_aliases.contains(&session_agent) && cursor_tools.contains(&tool_name)
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

fn log_event_locally(project_root: &str, op: &str, git_context: &GitContext) {
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

    let data = serde_json::to_string(&serde_json::json!({
        "commit_hash": git_context.commit_hash,
        "commit_message": git_context.commit_message,
        "author": git_context.author,
        "files_changed": git_context.files_changed,
        "insertions": git_context.insertions,
        "deletions": git_context.deletions,
    }))
    .ok();

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

fn resolve_git_remote(cfg: &Config) -> Option<String> {
    proxy::run_git_capture(cfg, &["remote", "get-url", "origin"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn parse_transcript_messages(text: &str) -> Vec<payload::TranscriptMessage> {
    text.lines()
        .filter_map(|line| {
            let parsed: serde_json::Value = serde_json::from_str(line).ok()?;
            let role = parsed
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let text = parsed
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
                .or_else(|| parsed.get("text").and_then(|t| t.as_str()))
                .unwrap_or("")
                .to_string();
            if text.is_empty() {
                return None;
            }
            Some(payload::TranscriptMessage { role, text })
        })
        .collect()
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

/// Collect rich transcript content for active sessions (for full-transparency mode).
///
/// Priority order per session:
/// 1. Cursor's bubbleId: DB — includes thinking, tool calls, timestamps, tokens
/// 2. transcript_path from the stop hook payload
/// 3. Tool registry's find_transcript (JSONL/text file)
fn collect_session_transcripts(
    sessions: &[crate::hooks::state::ActiveSession],
    project_root: &str,
) -> Vec<(String, String)> {
    let mut transcripts = Vec::new();

    for session in sessions {
        if is_cursor_session(&session.agent) {
            if let Some(rich) =
                crate::tools::cursor::composer_data::build_rich_transcript(&session.session_id)
            {
                transcripts.push((session.session_id.clone(), rich));
                continue;
            }
        }

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
            if is_agent_tool_match(&session.agent, tool.name()) {
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

fn is_cursor_session(agent: &str) -> bool {
    matches!(
        agent,
        "cursor" | "agent" | "composer" | "ask" | "edit" | "normal" | "chat"
    )
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
