use crate::config::Config;
use crate::core::anchor::{AnchorTurnRef, AuthorType, TransparencyMode};
use crate::git::{commands, detect, proxy};
use crate::hooks;

use super::anchor_builder::{build_anchor, AnchorBuildInput};
use super::anchor_persist::{persist_anchor_local, persist_anchor_portable};
use super::context::{collect_git_context, GitContext};
use super::file_attribution::{resolve_file_attribution, FileStat};
use super::session_evidence::is_agent_tool_match;
use super::session_resolver::{
    collect_ai_files_touched, discover_sessions_from_tools, filter_relevant_sessions,
    resolve_sessions, SessionResolution,
};
use super::transcripts::collect_session_transcripts;

struct CommitEvidence {
    initial_author_type: AuthorType,
    per_file: Vec<FileStat>,
    files_changed: Vec<String>,
    active_sessions: Vec<hooks::state::ActiveSession>,
    ai_files_touched: Vec<(String, String)>,
}

impl CommitEvidence {
    fn resolved_author_type(&self) -> AuthorType {
        // If detect said "assisted" but no sessions are actually relevant to
        // this commit, downgrade to human.
        if self.initial_author_type == AuthorType::Assisted
            && self.active_sessions.is_empty()
            && self.ai_files_touched.is_empty()
        {
            AuthorType::Human
        } else {
            self.initial_author_type.clone()
        }
    }
}

/// Called after a write operation succeeds.
/// Logs event locally and creates anchor metadata on the orphan branch.
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

    let branch = proxy::current_branch(cfg).unwrap_or_default();
    let git_context = collect_git_context(cfg, &op);
    let trace = crate::trace::Trace::new(&format!("git.{op}"));
    trace.stage("collect_git_context");

    if op == "commit" || op == "merge" {
        trace.stage("enrich_commit_start");
        enrich_commit(cfg, &project_root, &branch, &git_context);
        trace.stage("enrich_commit_done");
    }

    Ok(())
}

/// Create an Anchor (enriched commit primitive) and write to orphan branch.
fn enrich_commit(
    cfg: &Config,
    project_root: &str,
    branch: &str,
    git_context: &GitContext,
) -> Option<()> {
    if git_context.commit_hash.is_empty() {
        return None;
    }

    let evidence = load_commit_evidence(cfg, project_root);
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

    let SessionResolution {
        mut session_links,
        file_interactions,
    } = resolve_sessions(
        project_root,
        active_sessions,
        ai_files_touched,
        chrono::Utc::now().timestamp(),
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
        files_changed: evidence.files_changed,
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
        chrono::Utc::now().timestamp(),
        &mut session_links,
    );
    trace.stage("enrich_session_links");

    persist_anchor_local(project_root, &anchor, &session_links);
    trace.stage("persist_anchor_local");

    let transcripts = if transparency == TransparencyMode::On {
        collect_session_transcripts(active_sessions, project_root)
    } else {
        Vec::new()
    };
    persist_anchor_portable(project_root, &anchor, &session_links, &transcripts);
    trace.stage("persist_anchor_portable");

    Some(())
}

fn load_commit_evidence(cfg: &Config, project_root: &str) -> CommitEvidence {
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
    let mut active_sessions = filter_relevant_sessions(
        &all_sessions,
        project_root,
        &files_changed,
        parent_commit_epoch,
    );

    // Reconcile missing hook state by scanning native tool storage. This is
    // fallback behavior, not the preferred path. Only fire when the hook layer
    // had *no* sessions at all — if it found sessions but none overlapped the
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
fn parent_commit_timestamp(cfg: &Config) -> i64 {
    let output = proxy::run_git_capture(cfg, &["log", "-1", "--format=%at", "HEAD~1"]);
    output
        .unwrap_or_default()
        .trim()
        .parse::<i64>()
        .unwrap_or(0)
}

/// Collect per-file line stats from the most recent commit via `git diff-tree --numstat`.
fn collect_per_file_stats(cfg: &Config) -> Vec<FileStat> {
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
                turn_index: session.current_turn_index,
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
    use crate::attribution::turn_store::{write_turns, CollectingSink};
    use crate::taps::TapArtifact;

    let taps = collect_enabled_taps(cfg);

    for link in session_links {
        for tap in &taps {
            if !is_agent_tool_match(&link.agent, tap.source()) {
                continue;
            }
            let mut sink = CollectingSink::new();
            let _ = tap.ingest_session(&link.session_id, TapArtifact::SelfLookup, &mut sink);
            if !sink.turns.is_empty() {
                let _ = write_turns(project_root, tap.source(), &link.session_id, &sink.turns);
            }
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
