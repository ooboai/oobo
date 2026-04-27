use crate::config::Config;
use crate::core::anchor::{
    Anchor, AnchorTurnRef, AuthorType, FileAttribution, FileChange, FileInteraction,
    LineAttribution, LinkType, SessionLink, TransparencyMode,
};
use crate::git::{commands, detect, proxy};
use crate::hooks;
use crate::redact;

use super::anchor_builder::{build_anchor, AnchorBuildInput, AttributionResult};
use super::anchor_persist::{persist_anchor_local, persist_anchor_portable};
use super::context::{collect_git_context, GitContext};
use super::line_attribution::{compute_mixed_line_attrs, compute_pure_ai_line_attrs};
use super::session_evidence::{
    detect_file_interactions_refs, filter_by_recency, is_agent_tool_match, ms_to_secs,
    normalize_path,
};
use super::transcripts::{collect_session_transcripts, CollectedTranscript};

/// Anchor finalization output: canonical anchor plus linked sessions and
/// optional transcripts for transparent persistence.
#[allow(dead_code)]
struct AnchorBundle {
    anchor: Anchor,
    session_links: Vec<SessionLink>,
    transcripts: Vec<CollectedTranscript>,
}

type FileStat = (String, u32, u32);

/// Evidence available at the git write boundary, collected from already-normalized
/// hook state first and native tool storage only as reconciliation fallback.
struct CommitEvidence {
    initial_author_type: AuthorType,
    per_file: Vec<FileStat>,
    files_changed: Vec<String>,
    active_sessions: Vec<hooks::state::ActiveSession>,
    ai_files_touched: Vec<(String, String)>,
}

struct SessionResolution {
    session_links: Vec<SessionLink>,
    file_interactions: Vec<FileInteraction>,
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
pub fn on_write_op(cfg: &Config, args: &[&str]) -> Result<(), String> {
    let op = commands::subcommand_name(args)
        .unwrap_or("unknown")
        .to_string();

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
        match enrich_commit(cfg, &project_root, &branch, &git_context) {
            Ok(_data) => {
                trace.stage("enrich_commit_done");
            }
            Err(e) => {
                trace.detail("enrich_commit_error", &e);
                eprintln!("anchor: warning: could not enrich commit: {e}");
            }
        }
    }

    Ok(())
}

/// Create an Anchor (enriched commit primitive) and write to orphan branch.
/// Returns the anchor, session links, and transcripts for local persistence.
fn enrich_commit(
    cfg: &Config,
    project_root: &str,
    branch: &str,
    git_context: &GitContext,
) -> Result<Option<AnchorBundle>, String> {
    if git_context.commit_hash.is_empty() {
        return Ok(None);
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

    Ok(Some(AnchorBundle {
        anchor,
        session_links,
        transcripts,
    }))
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
    // fallback behavior, not the preferred path.
    if active_sessions.is_empty() && !project_root.is_empty() {
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

fn resolve_sessions(
    project_root: &str,
    active_sessions: &[hooks::state::ActiveSession],
    ai_files_touched: &[(String, String)],
    now_epoch: i64,
) -> SessionResolution {
    let cursor_ids: Vec<String> = active_sessions
        .iter()
        .filter(|s| is_cursor_session(&s.agent))
        .map(|s| s.session_id.clone())
        .collect();
    let bubble_data = crate::tools::cursor::composer_data::preload_bubble_data_for(&cursor_ids);
    let composer_data = crate::tools::cursor::composer_data::preload_composer_data_for(&cursor_ids);

    let mut session_links: Vec<SessionLink> = active_sessions
        .iter()
        .map(|s| {
            let touched: Vec<String> = ai_files_touched
                .iter()
                .filter(|(_, agent)| agent == &s.agent)
                .map(|(path, _)| path.clone())
                .collect();

            let native = extract_live_stats(
                &s.session_id,
                &s.agent,
                project_root,
                &bubble_data,
                &composer_data,
            );

            let native_inp = native.as_ref().and_then(|n| n.input_tokens);
            let native_out = native.as_ref().and_then(|n| n.output_tokens);
            let has_native_tokens =
                native_inp.is_some_and(|v| v > 0) || native_out.is_some_and(|v| v > 0);

            let (final_input, final_output, is_estimated) = if has_native_tokens {
                (native_inp, native_out, false)
            } else {
                let msgs = load_session_messages(
                    &s.session_id,
                    &s.agent,
                    project_root,
                    &bubble_data,
                    &composer_data,
                );
                if let Some((inp, out)) = count_tokens_from_messages(&msgs, s.model.as_deref()) {
                    (Some(inp), Some(out), true)
                } else {
                    (None, None, false)
                }
            };

            let duration_fallback = {
                let elapsed = now_epoch - s.started_at;
                if elapsed > 0 {
                    Some(elapsed as u64)
                } else {
                    None
                }
            };

            SessionLink {
                session_id: s.session_id.clone(),
                agent: s.agent.clone(),
                model: s.model.clone(),
                link_type: LinkType::Explicit,
                input_tokens: final_input,
                output_tokens: final_output,
                cache_read_tokens: native.as_ref().and_then(|n| n.cache_read_tokens),
                cache_creation_tokens: native.as_ref().and_then(|n| n.cache_creation_tokens),
                duration_secs: native
                    .as_ref()
                    .and_then(|n| n.duration_secs)
                    .or(duration_fallback),
                tool_calls: native
                    .as_ref()
                    .map(|n| n.tool_call_count)
                    .filter(|&c| c > 0),
                files_touched: if touched.is_empty() {
                    None
                } else {
                    Some(
                        touched
                            .into_iter()
                            .map(|p| redact::sanitize_path(&p, project_root))
                            .collect(),
                    )
                },
                tool_usage: s.tool_usage.clone(),
                tool_failures: s.tool_failures,
                subagent_count: s
                    .subagent_runs
                    .as_ref()
                    .map(|r| r.len() as u32)
                    .filter(|&c| c > 0),
                bash_commands: s.bash_commands.as_ref().map(|cmds| {
                    cmds.iter()
                        .map(|c| redact::sanitize_for_public(c, project_root))
                        .collect()
                }),
                thinking_duration_ms: s.thinking_duration_ms,
                compact_count: s.compact_count,
                is_subagent: false,
                parent_session_id: None,
                subagent_type: None,
                is_estimated,
                peer_session_ids: Vec::new(),
            }
        })
        .collect();

    // Filter out subagent sessions before detecting file interactions:
    // a parent and its subagent touching the same files is expected, not novel.
    let subagent_ids: std::collections::HashSet<String> = active_sessions
        .iter()
        .filter_map(|s| s.subagent_runs.as_ref())
        .flat_map(|runs| runs.iter().map(|r| r.agent_id.clone()))
        .collect();
    let top_level_sessions: Vec<&_> = active_sessions
        .iter()
        .filter(|s| !subagent_ids.contains(&s.session_id))
        .collect();
    let (raw_file_interactions, peer_map) =
        detect_file_interactions_refs(&top_level_sessions, project_root);
    let file_interactions: Vec<_> = raw_file_interactions
        .into_iter()
        .map(|mut fi| {
            fi.path = redact::sanitize_path(&fi.path, project_root);
            fi
        })
        .collect();
    for link in session_links.iter_mut() {
        if let Some(peers) = peer_map.get(&link.session_id) {
            link.peer_session_ids = peers.clone();
        }
    }

    SessionResolution {
        session_links,
        file_interactions,
    }
}

fn resolve_file_attribution(
    cfg: &Config,
    git_context: &GitContext,
    author_type: &AuthorType,
    active_sessions: &[hooks::state::ActiveSession],
    ai_files_touched: &[(String, String)],
    per_file: &[FileStat],
) -> AttributionResult {
    let has_ai_sessions = !active_sessions.is_empty();
    let ai_file_set: std::collections::HashSet<&str> =
        ai_files_touched.iter().map(|(p, _)| p.as_str()).collect();

    let (mut snapshot_lookup, pre_agent_lookup) =
        build_snapshot_lookups(active_sessions, ai_files_touched);

    let is_agent_commit = author_type == &AuthorType::Agent;

    // For agent-authored commits (non-interactive), the committed blob IS
    // the agent's final output. Replace any stale intermediate snapshots
    // with the committed blob so the 3-way diff doesn't misattribute the
    // agent's own revisions as human edits.
    if is_agent_commit {
        for (path, _, _) in per_file {
            if let Some(entry) = snapshot_lookup.get_mut(path.as_str()) {
                if let Ok(committed) =
                    proxy::run_git_capture(cfg, &["rev-parse", &format!("HEAD:{path}")])
                {
                    let committed = committed.trim().to_string();
                    if !committed.is_empty() && committed != entry.0 {
                        entry.0 = committed;
                    }
                }
            }
        }
    }

    let mut file_changes = Vec::new();
    let mut ai_added: u32 = 0;
    let mut ai_deleted: u32 = 0;
    let mut human_added: u32 = 0;
    let mut human_deleted: u32 = 0;

    for (path, added, deleted) in per_file {
        let pre_blob = pre_agent_lookup.get(path.as_str()).cloned();
        let (
            file_ai_add,
            file_ai_del,
            file_human_add,
            file_human_del,
            attribution,
            agent,
            line_attrs,
        ) = if let Some((agent_blob, agent_name)) = snapshot_lookup.get(path.as_str()) {
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
                    Vec::new(),
                )
            } else {
                let ai_a = *added / 2;
                let ai_d = *deleted / 2;
                (
                    ai_a,
                    ai_d,
                    added - ai_a,
                    deleted - ai_d,
                    Some(FileAttribution::Mixed),
                    agent_name,
                    Vec::new(),
                )
            }
        } else if is_agent_commit && !has_ai_sessions {
            (
                *added,
                *deleted,
                0,
                0,
                Some(FileAttribution::Ai),
                None,
                Vec::new(),
            )
        } else {
            (
                0,
                0,
                *added,
                *deleted,
                Some(FileAttribution::Human),
                None,
                Vec::new(),
            )
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
            line_attributions: line_attrs,
        });
    }

    let total_lines = git_context.insertions + git_context.deletions;
    let ai_percentage = if total_lines > 0 {
        let pct = ((ai_added + ai_deleted) as f64 / total_lines as f64) * 100.0;
        Some(pct.min(100.0))
    } else {
        None
    };

    AttributionResult {
        file_changes,
        ai_added,
        ai_deleted,
        human_added,
        human_deleted,
        ai_percentage,
    }
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
                if let Some(existing) = post_agent.get(file.as_str()) {
                    if existing.0 != *blob_hash {
                        eprintln!(
                            "anchor: warning: file '{}' has snapshots from multiple sessions \
                             (using session {})",
                            file, session.session_id
                        );
                    }
                }
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
) -> (
    u32,
    u32,
    u32,
    u32,
    Option<FileAttribution>,
    Option<String>,
    Vec<LineAttribution>,
) {
    let committed_blob = proxy::run_git_capture(cfg, &["rev-parse", &format!("HEAD:{file_path}")])
        .unwrap_or_default()
        .trim()
        .to_string();

    // Use pre-agent snapshot if available, otherwise fall back to parent commit blob
    let baseline_blob = if let Some(pre) = pre_agent_blob {
        pre.to_string()
    } else {
        proxy::run_git_capture(cfg, &["rev-parse", &format!("HEAD~1:{file_path}")])
            .unwrap_or_default()
            .trim()
            .to_string()
    };

    // If agent blob matches committed blob → pure AI (human didn't change it after the agent)
    if !committed_blob.is_empty() && agent_blob == committed_blob {
        let line_attrs =
            compute_pure_ai_line_attrs(cfg, &baseline_blob, &committed_blob, agent_name.as_deref());
        return (
            total_added,
            total_deleted,
            0,
            0,
            Some(FileAttribution::Ai),
            agent_name,
            line_attrs,
        );
    }

    // If agent blob matches baseline → AI changed nothing → pure human
    if !baseline_blob.is_empty() && agent_blob == baseline_blob {
        return (
            0,
            0,
            total_added,
            total_deleted,
            Some(FileAttribution::Human),
            None,
            Vec::new(),
        );
    }

    // AI contribution: baseline → agent_blob
    let ai_stats = if baseline_blob.is_empty() {
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

    let line_attrs = compute_mixed_line_attrs(
        cfg,
        &baseline_blob,
        agent_blob,
        &committed_blob,
        agent_name.as_deref(),
    );

    (
        ai_add,
        ai_del,
        human_add,
        human_del,
        attribution,
        agent_name,
        line_attrs,
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
                let normalized = normalize_path(file, project_root);
                if seen.insert(normalized.clone()) {
                    result.push((normalized, session.agent.clone()));
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

/// Fallback session discovery: scan each enabled tool's native storage for
/// recent sessions in this project. Used when the DB has no matching active
/// sessions (e.g. hooks fired before `git init`, or hooks weren't installed
/// for this project).
///
/// Only returns sessions updated after `since_epoch` (the parent commit time)
/// and within the last 2 hours — we don't want to link stale sessions.
fn discover_sessions_from_tools(
    cfg: &crate::config::Config,
    project_root: &str,
    since_epoch: i64,
) -> Vec<crate::hooks::state::ActiveSession> {
    let now_secs = chrono::Utc::now().timestamp();
    let max_age_secs: i64 = 7200; // 2 hours
    let cutoff_secs = std::cmp::max(since_epoch, now_secs - max_age_secs);

    let registry = crate::tools::registry();
    let mut discovered: Vec<crate::hooks::state::ActiveSession> = Vec::new();
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    for tool in registry.enabled(cfg) {
        let sessions = match tool.sessions_for_project(project_root) {
            Ok(s) => s,
            Err(_) => continue,
        };

        for session in sessions {
            if session.is_subagent() {
                continue;
            }

            let updated_ms = session.updated_at.or(session.created_at).unwrap_or(0);
            // Tool sessions store timestamps in milliseconds; normalize to
            // seconds to match hook-based ActiveSession conventions.
            let updated_secs = ms_to_secs(updated_ms);
            if updated_secs <= cutoff_secs {
                continue;
            }
            if seen_ids.contains(&session.session_id) {
                continue;
            }
            seen_ids.insert(session.session_id.clone());

            let created_secs = session.created_at.map(ms_to_secs).unwrap_or(updated_secs);

            discovered.push(crate::hooks::state::ActiveSession {
                session_id: session.session_id,
                agent: session.source.clone(),
                model: None,
                worktree: Some(project_root.to_string()),
                transcript_path: None,
                pre_agent_snapshots: None,
                file_snapshots: None,
                edited_files: None,
                read_files: None,
                tool_usage: None,
                tool_failures: None,
                bash_commands: None,
                subagent_runs: None,
                thinking_duration_ms: None,
                compact_count: None,
                current_turn_index: 0,
                current_turn_started_at: None,
                current_turn_hook_events: None,
                current_turn_tool_calls: None,
                last_turn_snapshot_id: None,
                started_at: created_secs,
                updated_at: updated_secs,
            });
        }
    }

    discovered
}

fn is_cursor_session(agent: &str) -> bool {
    crate::core::tool::is_cursor_agent(agent)
}

/// Extract live session stats directly from the tool's data files at commit
/// time. No prior manual reindex needed — reads Cursor's state.vscdb, Claude's
/// JSONL transcripts, etc.
fn extract_live_stats(
    session_id: &str,
    agent: &str,
    project_root: &str,
    bubble_data: &std::collections::HashMap<
        String,
        crate::tools::cursor::composer_data::BubbleSession,
    >,
    preloaded_composer: &std::collections::HashMap<
        String,
        crate::tools::cursor::composer_data::ComposerSession,
    >,
) -> Option<crate::analytics::NativeStats> {
    use crate::tools::cursor::composer_data;

    if is_cursor_session(agent) {
        if let Some(bubble) = bubble_data.get(session_id) {
            let mut stats = composer_data::native_stats_from_bubble(bubble);
            if stats.duration_secs.is_none() {
                if let Some(cs) = preloaded_composer.get(session_id) {
                    if let (Some(c), Some(u)) = (cs.created_at, cs.last_updated_at) {
                        if u > c {
                            stats.duration_secs = Some(((u - c) / 1000) as u64);
                        }
                    }
                }
            }
            return Some(stats);
        }
        if let Some(cs) = preloaded_composer.get(session_id) {
            return Some(composer_data::native_stats_from_session(cs));
        }
        return None;
    }

    if agent == "claude" {
        return crate::tools::claude::transcript::extract_native_stats(project_root, session_id);
    }

    if agent == "gemini" {
        if let Some(stats) =
            crate::tools::gemini::transcript::stats_for_session(project_root, session_id)
        {
            return Some(crate::analytics::NativeStats {
                input_tokens: stats.input_tokens,
                output_tokens: stats.output_tokens,
                cache_read_tokens: stats.cache_read_tokens,
                cache_creation_tokens: stats.cache_creation_tokens,
                duration_secs: stats.duration_secs,
                files_touched: stats.files_touched,
                tool_call_count: stats.tool_call_count,
            });
        }
        return None;
    }

    // Fall through to the tool registry for all other tools (codex, opencode,
    // aider, copilot, windsurf, trae, zed) so commit-time stats aren't lost.
    let registry = crate::tools::registry();
    for tool in registry.all() {
        if !is_agent_tool_match(agent, tool.name()) {
            continue;
        }
        if let Ok(sessions) = tool.sessions_for_project(project_root) {
            if let Some(tool_session) = sessions.iter().find(|s| s.session_id == session_id) {
                return tool.extract_native_stats(tool_session);
            }
        }
        // One agent name maps to exactly one tool — don't try others.
        break;
    }

    None
}

/// Count tokens from a message slice using tiktoken. Iterates directly over
/// the borrowed slice to avoid cloning message strings into intermediate pairs.
fn count_tokens_from_messages(
    messages: &[crate::core::message::Message],
    model: Option<&str>,
) -> Option<(u64, u64)> {
    use crate::analytics::tokenizer;

    if messages.is_empty() {
        return None;
    }

    let family = model
        .map(tokenizer::detect_family)
        .unwrap_or(tokenizer::ModelFamily::Cl100k);
    let inp: u64 = messages
        .iter()
        .filter(|m| tokenizer::is_input_role(&m.role))
        .map(|m| tokenizer::count_tokens(&m.text, family))
        .sum();
    let out: u64 = messages
        .iter()
        .filter(|m| tokenizer::is_output_role(&m.role))
        .map(|m| tokenizer::count_tokens(&m.text, family))
        .sum();
    if inp > 0 || out > 0 {
        Some((inp, out))
    } else {
        None
    }
}

/// Load conversation messages for a session, preferring already-loaded data.
/// For Cursor: uses preloaded bubble data, falls back to preloaded composerData.
/// For other agents: delegates to the tool registry's transcript parser.
fn load_session_messages(
    session_id: &str,
    agent: &str,
    project_root: &str,
    bubble_data: &std::collections::HashMap<
        String,
        crate::tools::cursor::composer_data::BubbleSession,
    >,
    preloaded_composer: &std::collections::HashMap<
        String,
        crate::tools::cursor::composer_data::ComposerSession,
    >,
) -> Vec<crate::core::message::Message> {
    if is_cursor_session(agent) {
        if let Some(bs) = bubble_data.get(session_id) {
            if !bs.messages.is_empty() {
                return bs.messages.clone();
            }
        }
        if let Some(cs) = preloaded_composer.get(session_id) {
            if !cs.messages.is_empty() {
                return cs.messages.clone();
            }
        }
        Vec::new()
    } else {
        let source = crate::core::tool::normalize_source(agent);
        crate::session::parse_messages_for_session(project_root, session_id, source)
    }
}

/// Resolve the effective transparency mode for a project. `.oobo/config` is the
/// only per-project source of truth; global config supplies the default.
fn resolve_transparency(cfg: &Config, project_root: &str) -> crate::core::anchor::TransparencyMode {
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
    session_links: &[SessionLink],
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_tokens_from_messages_with_bubble_data() {
        use crate::core::message::Message;
        use crate::tools::cursor::composer_data::BubbleSession;

        let mut bubble_data = std::collections::HashMap::new();
        bubble_data.insert(
            "test-session".to_string(),
            BubbleSession {
                messages: vec![
                    Message {
                        role: "user".to_string(),
                        text: "How do I implement a binary search in Rust?".to_string(),
                        timestamp_ms: Some(1000),
                    },
                    Message {
                        role: "assistant".to_string(),
                        text: "Here is a binary search implementation in Rust using iterators and pattern matching for clean, idiomatic code.".to_string(),
                        timestamp_ms: Some(2000),
                    },
                ],
                total_input_tokens: 0,
                total_output_tokens: 0,
                files_touched: Vec::new(),
                code_block_count: 0,
                tool_call_count: 0,
                is_agentic: true,
            },
        );

        let composer_data = std::collections::HashMap::new();
        let msgs = load_session_messages(
            "test-session",
            "cursor",
            "/tmp/project",
            &bubble_data,
            &composer_data,
        );
        let result = count_tokens_from_messages(&msgs, None);
        assert!(result.is_some(), "should produce tiktoken estimates");
        let (inp, out) = result.unwrap();
        assert!(inp > 0, "input tokens should be > 0");
        assert!(out > 0, "output tokens should be > 0");
    }

    #[test]
    fn test_count_tokens_from_messages_with_model() {
        use crate::core::message::Message;

        let messages = vec![
            Message {
                role: "user".to_string(),
                text: "What is Rust programming language?".to_string(),
                timestamp_ms: None,
            },
            Message {
                role: "assistant".to_string(),
                text: "Rust is a systems programming language focused on safety and performance."
                    .to_string(),
                timestamp_ms: None,
            },
        ];

        let result_cl100k = count_tokens_from_messages(&messages, Some("claude-sonnet-4"));
        let result_o200k = count_tokens_from_messages(&messages, Some("gpt-4o"));
        assert!(result_cl100k.is_some());
        assert!(result_o200k.is_some());
        let (inp1, out1) = result_cl100k.unwrap();
        let (inp2, out2) = result_o200k.unwrap();
        assert!(inp1 > 0 && out1 > 0);
        assert!(inp2 > 0 && out2 > 0);
    }

    #[test]
    fn test_load_session_messages_empty_bubble() {
        let mut bubble_data = std::collections::HashMap::new();
        bubble_data.insert(
            "empty-session".to_string(),
            crate::tools::cursor::composer_data::BubbleSession::default(),
        );
        let composer_data = std::collections::HashMap::new();

        let msgs = load_session_messages(
            "empty-session",
            "cursor",
            "/tmp/project",
            &bubble_data,
            &composer_data,
        );
        let result = count_tokens_from_messages(&msgs, None);
        assert!(result.is_none(), "empty messages should return None");
    }

    #[test]
    fn test_load_session_messages_unknown_session() {
        let bubble_data = std::collections::HashMap::new();
        let composer_data = std::collections::HashMap::new();
        let msgs = load_session_messages(
            "nonexistent-session",
            "cursor",
            "/tmp/project",
            &bubble_data,
            &composer_data,
        );
        let result = count_tokens_from_messages(&msgs, None);
        assert!(result.is_none(), "unknown session should return None");
    }

    #[test]
    fn test_load_session_messages_falls_back_to_composer_data() {
        use crate::core::message::Message;
        use crate::tools::cursor::composer_data::ComposerSession;

        let bubble_data = std::collections::HashMap::new();
        let mut composer_data = std::collections::HashMap::new();
        composer_data.insert(
            "composer-session".to_string(),
            ComposerSession {
                composer_id: "composer-session".to_string(),
                name: "test".to_string(),
                mode: "agent".to_string(),
                status: "done".to_string(),
                created_at: Some(1000),
                last_updated_at: Some(2000),
                messages: vec![
                    Message {
                        role: "user".to_string(),
                        text: "Help me write a function".to_string(),
                        timestamp_ms: Some(1000),
                    },
                    Message {
                        role: "assistant".to_string(),
                        text: "Here is a function implementation.".to_string(),
                        timestamp_ms: Some(1500),
                    },
                ],
                files_touched: Vec::new(),
                code_block_count: 0,
                is_agentic: true,
            },
        );

        let msgs = load_session_messages(
            "composer-session",
            "cursor",
            "/tmp/project",
            &bubble_data,
            &composer_data,
        );
        assert!(!msgs.is_empty(), "should load messages from composer data");
        let result = count_tokens_from_messages(&msgs, None);
        assert!(result.is_some());
    }

    #[test]
    fn test_count_tokens_includes_system_and_tool_roles() {
        use crate::core::message::Message;

        let messages = vec![
            Message {
                role: "system".to_string(),
                text: "You are a helpful coding assistant with access to tools.".to_string(),
                timestamp_ms: None,
            },
            Message {
                role: "user".to_string(),
                text: "Read the file".to_string(),
                timestamp_ms: None,
            },
            Message {
                role: "assistant".to_string(),
                text: "Reading the file now.".to_string(),
                timestamp_ms: None,
            },
            Message {
                role: "tool".to_string(),
                text: "fn main() { println!(\"hello world\"); }".to_string(),
                timestamp_ms: None,
            },
        ];

        let result = count_tokens_from_messages(&messages, None);
        assert!(result.is_some());
        let (inp, _out) = result.unwrap();
        let user_only_msgs = vec![Message {
            role: "user".to_string(),
            text: "Read the file".to_string(),
            timestamp_ms: None,
        }];
        let user_only = count_tokens_from_messages(&user_only_msgs, None);
        let (user_inp, _) = user_only.unwrap();
        assert!(
            inp > user_inp,
            "input tokens should include system + tool, not just user"
        );
    }
}
