use crate::config::Config;
use crate::core::anchor::{AnchorTurnRef, AuthorType, TransparencyMode};
use crate::git::{commands, detect, proxy};
use crate::hooks;

use super::anchor_builder::{build_anchor, AnchorBuildInput};
use super::anchor_persist::persist_anchor_local;
use super::context::{collect_git_context_at, GitContext};
use super::file_attribution::{resolve_file_attribution, FileStat};
use super::session_evidence::is_agent_tool_match;
use super::session_resolver::{
    collect_ai_files_touched, discover_sessions_from_tools, filter_relevant_sessions,
    resolve_sessions, SessionResolution,
};

struct CommitEvidence {
    initial_author_type: AuthorType,
    per_file: Vec<FileStat>,
    files_changed: Vec<String>,
    active_sessions: Vec<hooks::state::ActiveSession>,
    ai_files_touched: Vec<(String, String)>,
}

impl CommitEvidence {
    fn resolved_author_type(&self) -> AuthorType {
        let has_ai_evidence = !self.active_sessions.is_empty() || !self.ai_files_touched.is_empty();
        match &self.initial_author_type {
            // Downgrade: detect said assisted but no sessions overlap this commit.
            AuthorType::Assisted if !has_ai_evidence => AuthorType::Human,
            // Upgrade: detect said human (e.g. ended session was skipped, no env
            // vars) but file-overlap matching found a relevant session.
            AuthorType::Human if has_ai_evidence => AuthorType::Assisted,
            // Upgrade: detect said automated (non-interactive, no active session at
            // check time) but session evidence was found via recency/file overlap.
            AuthorType::Automated if has_ai_evidence => AuthorType::Agent,
            other => other.clone(),
        }
    }
}

/// Everything the enrichment pipeline produced for one commit, returned
/// so the async worker can layer v2 store writes and content claims on
/// top without re-deriving evidence.
pub struct EnrichOutcome {
    pub anchor: crate::core::anchor::Anchor,
    pub session_links: Vec<crate::core::anchor::SessionLink>,
    pub active_sessions: Vec<hooks::state::ActiveSession>,
}

/// Called after a write operation succeeds. The git write path must cost
/// nothing: commits are appended to the spool and a detached worker is
/// spawned to do the heavy enrichment asynchronously.
#[tracing::instrument(skip_all, fields(op))]
pub fn on_write_op(cfg: &Config, args: &[&str]) -> Result<(), String> {
    let op = commands::subcommand_name(args)
        .unwrap_or("unknown")
        .to_string();
    tracing::Span::current().record("op", op.as_str());

    let project_root = proxy::project_root(cfg).unwrap_or_default();
    if project_root.is_empty()
        || cfg.is_ignored(&project_root)
        || !crate::project_config::is_enabled(&project_root)
    {
        return Ok(());
    }

    super::first_use::check_first_use(cfg, &project_root);

    if op == "commit" || op == "merge" {
        match crate::git::spool::append_commit(&project_root) {
            Ok(_) => crate::worker::kick(cfg, &project_root),
            Err(e) => tracing::warn!(%e, "could not spool commit"),
        }
    }

    Ok(())
}

/// Create an Anchor (enriched commit primitive) for a specific sha and
/// write it to the orphan branch. Runs inside the detached worker —
/// never on the git write path.
pub fn enrich_commit_for(
    cfg: &Config,
    project_root: &str,
    branch: &str,
    sha: &str,
) -> Option<EnrichOutcome> {
    let git_context = collect_git_context_at(cfg, project_root, sha);
    enrich_commit(cfg, project_root, branch, &git_context)
}

fn enrich_commit(
    cfg: &Config,
    project_root: &str,
    branch: &str,
    git_context: &GitContext,
) -> Option<EnrichOutcome> {
    if git_context.commit_hash.is_empty() {
        return None;
    }

    let evidence = load_commit_evidence(cfg, project_root, &git_context.commit_hash);
    let trace = crate::trace::Trace::new("anchor.finalize");
    trace.detail(
        "load_commit_evidence",
        format!(
            "files={} sessions={} ai_files={}",
            evidence.files_changed.len(),
            evidence.active_sessions.len(),
            evidence.ai_files_touched.len()
        ),
    );
    let author_type = evidence.resolved_author_type();
    let active_sessions = &evidence.active_sessions;
    let ai_files_touched = &evidence.ai_files_touched;
    let per_file = &evidence.per_file;
    let files_changed = evidence.files_changed.clone();

    let SessionResolution {
        mut session_links,
        file_interactions,
    } = resolve_sessions(
        project_root,
        active_sessions,
        ai_files_touched,
        git_context.committed_at,
    );
    trace.detail(
        "resolve_sessions",
        format!(
            "links={} interactions={}",
            session_links.len(),
            file_interactions.len()
        ),
    );

    let transparency = resolve_transparency(cfg, project_root);

    let attribution = resolve_file_attribution(
        cfg,
        git_context,
        &author_type,
        active_sessions,
        ai_files_touched,
        per_file,
    );
    trace.detail(
        "resolve_file_attribution",
        format!(
            "ai_added={} human_added={}",
            attribution.ai_added, attribution.human_added
        ),
    );

    let anchor = build_anchor(AnchorBuildInput {
        branch,
        git_context,
        author_type,
        session_links: &session_links,
        files_changed,
        attribution,
        transparency,
        file_interactions,
        turns: collect_anchor_turn_refs(project_root, active_sessions),
    });
    trace.stage("build_anchor");

    capture_turns_for_sessions(cfg, project_root, &session_links);
    trace.stage("capture_turns");

    crate::attribution::runner::enrich_session_links(
        project_root,
        &git_context.commit_hash,
        git_context.committed_at,
        &mut session_links,
    );
    trace.stage("enrich_session_links");

    persist_anchor_local(project_root, &anchor, &session_links);
    trace.stage("persist_anchor_local");

    Some(EnrichOutcome {
        anchor,
        session_links,
        active_sessions: evidence.active_sessions,
    })
}

fn load_commit_evidence(cfg: &Config, project_root: &str, sha: &str) -> CommitEvidence {
    let author_info = detect::detect(project_root);
    let initial_author_type = match &author_info {
        detect::CommitAuthor::Agent { .. } => AuthorType::Agent,
        detect::CommitAuthor::Assisted { .. } => AuthorType::Assisted,
        detect::CommitAuthor::Human => AuthorType::Human,
        detect::CommitAuthor::Automated => AuthorType::Automated,
    };
    let per_file = collect_per_file_stats(cfg, project_root, sha);
    let files_changed: Vec<String> = per_file.iter().map(|(p, _, _)| p.clone()).collect();
    let parent_commit_epoch = parent_commit_timestamp(cfg, project_root, sha);

    let all_sessions = hooks::state::active_sessions_for_worktree(project_root);
    let mut active_sessions = filter_relevant_sessions(
        &all_sessions,
        project_root,
        &files_changed,
        parent_commit_epoch,
    );

    // Reconcile missing hook state by scanning native tool storage. This is
    // fallback behavior, not the preferred path. Only fire when the hook layer
    // had *no* sessions at all  --  if it found sessions but none overlapped the
    // committed files, we should respect that verdict instead of re-discovering
    // those same sessions through tool storage and matching by recency alone.
    if active_sessions.is_empty() && all_sessions.is_empty() && !project_root.is_empty() {
        let fallback = discover_sessions_from_tools(cfg, project_root, parent_commit_epoch);
        if !fallback.is_empty() {
            active_sessions = filter_relevant_sessions(
                &fallback,
                project_root,
                &files_changed,
                parent_commit_epoch,
            );
        }
    }

    let ai_files_touched =
        collect_ai_files_touched(cfg, project_root, &active_sessions, parent_commit_epoch);

    CommitEvidence {
        initial_author_type,
        per_file,
        files_changed,
        active_sessions,
        ai_files_touched,
    }
}

/// Get the parent commit's author timestamp (Unix epoch seconds).
/// Returns 0 if there's no parent (initial commit).
fn parent_commit_timestamp(cfg: &Config, project_root: &str, sha: &str) -> i64 {
    let parent = format!("{sha}~1");
    let output = proxy::run_git_capture_in(
        cfg,
        &["log", "-1", "--format=%at", &parent],
        Some(project_root),
    );
    output
        .unwrap_or_default()
        .trim()
        .parse::<i64>()
        .unwrap_or(0)
}

/// Collect per-file line stats for a commit via `git diff-tree --numstat`.
fn collect_per_file_stats(cfg: &Config, project_root: &str, sha: &str) -> Vec<FileStat> {
    let output = proxy::run_git_capture_in(
        cfg,
        &[
            "diff-tree",
            "--no-commit-id",
            "--numstat",
            "-r",
            "--root",
            sha,
        ],
        Some(project_root),
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

/// Resolve the effective transparency mode for a project.
fn resolve_transparency(cfg: &Config, project_root: &str) -> TransparencyMode {
    if let Some(mode) = crate::project_config::transparency_mode(project_root) {
        return mode;
    }

    cfg.transparency_mode()
}

fn collect_anchor_turn_refs(
    project_root: &str,
    active_sessions: &[hooks::state::ActiveSession],
) -> Vec<AnchorTurnRef> {
    let mut refs = Vec::new();
    for session in active_sessions {
        let Some(turn_id) = session.last_turn_snapshot_id.as_deref() else {
            continue;
        };
        if refs.iter().any(|turn: &AnchorTurnRef| turn.id == turn_id) {
            continue;
        }
        if let Some(snapshot) = crate::git::turns::read_turn_snapshot(project_root, turn_id) {
            refs.push(AnchorTurnRef {
                id: snapshot.id,
                session_id: snapshot.session_id,
                source: snapshot.source,
                turn_index: snapshot.turn_index,
                tree_hash: snapshot.tree_hash,
            });
        } else {
            refs.push(AnchorTurnRef {
                id: turn_id.to_string(),
                session_id: session.session_id.clone(),
                source: crate::core::tool::normalize_source(&session.agent).to_string(),
                // The index points at the in-flight turn; the last
                // FINISHED snapshot (which `turn_id` names) is one back.
                turn_index: (session.current_turn_index - 1).max(0),
                tree_hash: None,
            });
        }
    }
    refs
}

fn capture_turns_for_sessions(
    cfg: &Config,
    project_root: &str,
    session_links: &[crate::core::anchor::SessionLink],
) {
    for link in session_links {
        let transcript_path = crate::hooks::store::read(project_root, &link.session_id)
            .and_then(|s| s.transcript_path);
        ingest_turns_for_session(
            cfg,
            project_root,
            &link.agent,
            &link.session_id,
            transcript_path.as_deref(),
        );
    }
}

/// Ingest one session's native tap turns into the per-repo turn cache.
///
/// Prefers the hook-recorded `transcript_path` (the exact artifact the
/// tool reported — works for every tool), falling back to the tap's own
/// artifact lookup for taps that support `SelfLookup`. Without this,
/// taps like Claude (no `SelfLookup`) never populate the cache during
/// the live flow and v2 turn records lose tokens/model/prompt.
pub(crate) fn ingest_turns_for_session(
    cfg: &Config,
    project_root: &str,
    agent: &str,
    session_id: &str,
    transcript_path: Option<&str>,
) {
    use crate::attribution::turn_store::{write_turns, CollectingSink};
    use crate::taps::TapArtifact;

    for tap in collect_enabled_taps(cfg) {
        if !is_agent_tool_match(agent, tap.source()) {
            continue;
        }
        let mut sink = CollectingSink::new();
        if let Some(p) = transcript_path
            .map(std::path::Path::new)
            .filter(|p| p.exists())
        {
            let _ = tap.ingest_session(session_id, TapArtifact::File(p), &mut sink);
        }
        if sink.turns.is_empty() {
            let _ = tap.ingest_session(session_id, TapArtifact::SelfLookup, &mut sink);
        }
        if !sink.turns.is_empty() {
            let _ = write_turns(project_root, tap.source(), session_id, &sink.turns);
        }
    }
}

fn collect_enabled_taps(cfg: &Config) -> Vec<Box<dyn crate::taps::TurnTap>> {
    use crate::taps::TurnTap;
    let mut taps: Vec<Box<dyn TurnTap>> = Vec::new();

    let claude_tap = crate::taps::claude::ClaudeTurnTap;
    if claude_tap.enabled(cfg) {
        taps.push(Box::new(claude_tap));
    }

    let codex_tap = crate::taps::codex::CodexTurnTap;
    if codex_tap.enabled(cfg) {
        taps.push(Box::new(codex_tap));
    }

    let cursor_tap = crate::taps::cursor::CursorTurnTap;
    if cursor_tap.enabled(cfg) {
        taps.push(Box::new(cursor_tap));
    }

    let opencode_tap = crate::taps::opencode::OpenCodeTurnTap;
    if opencode_tap.enabled(cfg) {
        taps.push(Box::new(opencode_tap));
    }

    taps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::anchor::AuthorType;

    fn empty_evidence(author_type: AuthorType) -> CommitEvidence {
        CommitEvidence {
            initial_author_type: author_type,
            per_file: Vec::new(),
            files_changed: Vec::new(),
            active_sessions: Vec::new(),
            ai_files_touched: Vec::new(),
        }
    }

    #[test]
    fn test_resolved_downgrades_assisted_without_evidence() {
        let evidence = empty_evidence(AuthorType::Assisted);
        assert_eq!(evidence.resolved_author_type(), AuthorType::Human);
    }

    #[test]
    fn test_resolved_keeps_assisted_with_sessions() {
        let mut evidence = empty_evidence(AuthorType::Assisted);
        let now = chrono::Utc::now().timestamp();
        evidence
            .active_sessions
            .push(crate::hooks::state::ActiveSession {
                session_id: "s1".into(),
                agent: "claude".into(),
                model: None,
                worktree: None,
                transcript_path: None,
                pre_agent_snapshots: None,
                file_snapshots: None,
                edited_files: None,
                read_files: None,
                file_events: None,
                tool_usage: None,
                tool_failures: None,
                bash_commands: None,
                subagent_runs: None,
                resumed_from: None,
                thinking_duration_ms: None,
                compact_count: None,
                turn_count: None,
                context_tokens: None,
                context_window_size: None,
                current_turn_index: 0,
                current_turn_started_at: None,
                current_turn_hook_events: None,
                current_turn_tool_calls: None,
                last_turn_snapshot_id: None,
                pre_edit_pending: None,
                file_edit_chain: None,
                foreign_repos: None,
                event_seq: 0,
                started_at: now,
                updated_at: now,
                ended_at: None,
            });
        assert_eq!(evidence.resolved_author_type(), AuthorType::Assisted);
    }

    #[test]
    fn test_resolved_upgrades_human_with_ai_evidence() {
        let mut evidence = empty_evidence(AuthorType::Human);
        evidence
            .ai_files_touched
            .push(("src/main.rs".into(), "claude".into()));
        assert_eq!(
            evidence.resolved_author_type(),
            AuthorType::Assisted,
            "human should upgrade to assisted when AI file evidence exists"
        );
    }

    #[test]
    fn test_resolved_keeps_human_without_evidence() {
        let evidence = empty_evidence(AuthorType::Human);
        assert_eq!(evidence.resolved_author_type(), AuthorType::Human);
    }

    #[test]
    fn test_resolved_keeps_agent_regardless() {
        let evidence = empty_evidence(AuthorType::Agent);
        assert_eq!(evidence.resolved_author_type(), AuthorType::Agent);
    }
}
