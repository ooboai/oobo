use crate::config::Config;
use crate::core::anchor::{
    Anchor, AuthorType, Contributor, ContributorRole, FileAttribution, FileChange, LineAttribution,
    LineRange, LinkType, SessionLink, TransparencyMode,
};
use crate::git::{commands, detect, proxy};
use crate::hooks;
use crate::redact;
use crate::remote;
use crate::remote::payload;

/// A collected transcript with optional parent linkage for subagent sessions.
pub(super) struct CollectedTranscript {
    pub session_id: String,
    pub content: String,
    pub parent_session_id: Option<String>,
    pub subagent_type: Option<String>,
}

/// Anchor + linked sessions + collected transcripts.
type EnrichResult = (Anchor, Vec<SessionLink>, Vec<CollectedTranscript>);

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
            Ok(data) => {
                // Proactively compute stats for linked sessions so they're
                // available immediately without requiring `oobo scan`.
                // Spawned on a detached thread to avoid adding latency to git commit.
                if let Some((_, ref links, _)) = data {
                    let index_links: Vec<_> = links
                        .iter()
                        .map(|l| (l.session_id.clone(), l.agent.clone()))
                        .collect();
                    let pr = project_root.to_string();
                    std::thread::spawn(move || {
                        index_linked_sessions_bg(&index_links, &pr);
                    });
                }
                anchor_data = data;
            }
            Err(e) => eprintln!("oobo: warning: could not enrich commit: {e}"),
        }
    }

    let project_sync = crate::commands::sync::resolve_project_sync(cfg);
    let effective_key = crate::commands::sync::resolve_api_key(cfg);
    let should_sync = project_sync.unwrap_or(cfg.server.sync) && !effective_key.is_empty();

    if should_sync && anchor_data.is_some() {
        let git_remote = resolve_git_remote(cfg).or_else(|| Some(String::new()));

        let (anchor_payload, transcript, session_transcripts) =
            if let Some((anchor, links, transcripts)) = anchor_data {
                let is_transparent =
                    anchor.transparency_mode == TransparencyMode::On && !transcripts.is_empty();

                let (flat_messages, structured) = if is_transparent {
                    let mut structured = Vec::new();
                    for ct in &transcripts {
                        let redacted = redact::redact(&ct.content);
                        let msgs = parse_transcript_messages(&redacted);

                        structured.push(payload::SessionTranscript {
                            session_id: ct.session_id.clone(),
                            parent_session_id: ct.parent_session_id.clone(),
                            subagent_type: ct.subagent_type.clone(),
                            messages: msgs,
                        });
                    }
                    let flat: Vec<_> = structured
                        .iter()
                        .flat_map(|st| st.messages.iter().cloned())
                        .collect();
                    (flat, structured)
                } else {
                    (Vec::new(), Vec::new())
                };

                (
                    Some(payload::AnchorPayload {
                        anchor,
                        sessions: links,
                    }),
                    flat_messages,
                    structured,
                )
            } else {
                (None, Vec::new(), Vec::new())
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
            session_transcripts,
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
    let mut active_sessions = filter_relevant_sessions(
        &all_sessions,
        project_root,
        &files_changed,
        parent_commit_epoch,
    );

    // Fallback: when hooks didn't register any sessions (e.g. new project
    // where sessionStart fired before git init), discover sessions directly
    // from each tool's native storage, then apply the same file-overlap and
    // recency filters so we only link sessions that actually contributed.
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

    let now_epoch = chrono::Utc::now().timestamp();

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

    // Filter out subagent sessions before detecting file interactions —
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

    let session_ids: Vec<String> = session_links.iter().map(|s| s.session_id.clone()).collect();

    let transparency = resolve_transparency(cfg, project_root);

    let has_ai_sessions = !active_sessions.is_empty();
    let ai_file_set: std::collections::HashSet<&str> =
        ai_files_touched.iter().map(|(p, _)| p.as_str()).collect();

    let (mut snapshot_lookup, pre_agent_lookup) =
        build_snapshot_lookups(&active_sessions, &ai_files_touched);

    let is_agent_commit = author_type == AuthorType::Agent;

    // For agent-authored commits (non-interactive), the committed blob IS
    // the agent's final output. Replace any stale intermediate snapshots
    // with the committed blob so the 3-way diff doesn't misattribute the
    // agent's own revisions as human edits.
    if is_agent_commit {
        for (path, _, _) in &per_file {
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

    for (path, added, deleted) in &per_file {
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
        file_interactions: if file_interactions.is_empty() {
            None
        } else {
            Some(file_interactions)
        },
    };

    let db = crate::db::Db::open().ok();
    if let Some(ref db) = db {
        let anchor_json = serde_json::to_string(&anchor).unwrap_or_default();
        if let Err(e) = db.insert_anchor(&anchor.commit_hash, &anchor_json) {
            eprintln!("oobo: warning: could not save anchor: {e}");
        }

        // TODO: persist `peer_session_ids` to the `anchor_sessions` table
        // once a DB migration adds the column. Currently peer data is only
        // stored in the anchor JSON blob on the orphan branch.
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
                link.input_tokens,
                link.output_tokens,
                link.cache_read_tokens,
                link.cache_creation_tokens,
                link.duration_secs,
                link.tool_calls,
                link.is_subagent,
                link.parent_session_id.as_deref(),
                link.subagent_type.as_deref(),
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
                if let Some(existing) = post_agent.get(file.as_str()) {
                    if existing.0 != *blob_hash {
                        eprintln!(
                            "oobo: warning: file '{}' has snapshots from multiple sessions \
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

/// Build line attributions for pure AI files (agent_blob == committed_blob).
/// All added lines (baseline → committed) are AI-authored.
fn compute_pure_ai_line_attrs(
    cfg: &Config,
    baseline_blob: &str,
    committed_blob: &str,
    agent_name: Option<&str>,
) -> Vec<LineAttribution> {
    let ai_ranges = if baseline_blob.is_empty() {
        // New file: every line is AI
        blob_total_range(cfg, committed_blob)
            .map(|r| vec![r])
            .unwrap_or_default()
    } else {
        diff_blobs_added_ranges(cfg, baseline_blob, committed_blob).unwrap_or_default()
    };

    if ai_ranges.is_empty() {
        return Vec::new();
    }

    vec![LineAttribution {
        author: FileAttribution::Ai,
        ranges: ai_ranges,
        agent: agent_name.map(|s| s.to_string()),
    }]
}

/// Build line attributions for mixed files (both AI and human contributed).
/// Uses committed-file coordinates for all ranges:
/// - all_added = diff(baseline → committed)   → every added line in the commit
/// - human_modified = diff(agent → committed)  → lines human changed after the agent
/// - ai_ranges = all_added MINUS human_modified
fn compute_mixed_line_attrs(
    cfg: &Config,
    baseline_blob: &str,
    agent_blob: &str,
    committed_blob: &str,
    agent_name: Option<&str>,
) -> Vec<LineAttribution> {
    if committed_blob.is_empty() {
        return Vec::new();
    }

    let all_added = if baseline_blob.is_empty() {
        blob_total_range(cfg, committed_blob)
            .map(|r| vec![r])
            .unwrap_or_default()
    } else {
        diff_blobs_added_ranges(cfg, baseline_blob, committed_blob).unwrap_or_default()
    };

    if all_added.is_empty() {
        return Vec::new();
    }

    let human_ranges = diff_blobs_added_ranges(cfg, agent_blob, committed_blob).unwrap_or_default();

    if human_ranges.is_empty() {
        return vec![LineAttribution {
            author: FileAttribution::Ai,
            ranges: all_added,
            agent: agent_name.map(|s| s.to_string()),
        }];
    }

    let ai_ranges = subtract_ranges(&all_added, &human_ranges);

    let mut attrs = Vec::new();
    if !ai_ranges.is_empty() {
        attrs.push(LineAttribution {
            author: FileAttribution::Ai,
            ranges: ai_ranges,
            agent: agent_name.map(|s| s.to_string()),
        });
    }
    if !human_ranges.is_empty() {
        attrs.push(LineAttribution {
            author: FileAttribution::Human,
            ranges: human_ranges,
            agent: None,
        });
    }
    attrs
}

/// Subtract `remove` ranges from `source` ranges.
/// All ranges are sorted, non-overlapping, and 1-indexed inclusive.
/// Returns the portions of `source` that don't overlap with `remove`.
fn subtract_ranges(source: &[LineRange], remove: &[LineRange]) -> Vec<LineRange> {
    if remove.is_empty() {
        return source.to_vec();
    }

    let mut result = Vec::new();
    let mut ri = 0;

    for s in source {
        let mut cur_start = s.start;
        let cur_end = s.end;

        while ri < remove.len() && remove[ri].end < cur_start {
            ri += 1;
        }

        let mut rj = ri;
        while rj < remove.len() && remove[rj].start <= cur_end {
            let r = &remove[rj];
            if r.start > cur_start {
                result.push(LineRange::new(cur_start, r.start - 1));
            }
            cur_start = r.end + 1;
            rj += 1;
        }

        if cur_start <= cur_end {
            result.push(LineRange::new(cur_start, cur_end));
        }
    }

    result
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

/// Diff two blob objects and return the line ranges that were added in blob_b.
/// Uses `git diff -U0` to get minimal hunks with precise `@@` headers.
/// Returns Vec<LineRange> representing added lines in blob_b coordinates.
fn diff_blobs_added_ranges(cfg: &Config, blob_a: &str, blob_b: &str) -> Option<Vec<LineRange>> {
    if blob_a.is_empty() || blob_b.is_empty() {
        return None;
    }

    let output = proxy::run_git_capture(cfg, &["diff", "-U0", blob_a, blob_b]).ok()?;
    Some(parse_diff_added_ranges(&output))
}

/// Parse unified diff output (with -U0) and extract added line ranges.
/// Hunk headers look like: `@@ -old_start[,old_count] +new_start[,new_count] @@`
fn parse_diff_added_ranges(diff_output: &str) -> Vec<LineRange> {
    let mut ranges = Vec::new();

    for line in diff_output.lines() {
        if !line.starts_with("@@") {
            continue;
        }
        if let Some(plus_part) = extract_hunk_plus(line) {
            let (start, count) = parse_hunk_range(plus_part);
            if count > 0 {
                ranges.push(LineRange::new(start, start + count - 1));
            }
        }
    }

    ranges
}

/// Extract the `+start[,count]` portion from a `@@ ... @@` hunk header.
fn extract_hunk_plus(hunk_line: &str) -> Option<&str> {
    let after_at = hunk_line.strip_prefix("@@")?;
    let plus_idx = after_at.find('+')?;
    let rest = &after_at[plus_idx + 1..];
    let end = rest.find([' ', '@']).unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Parse a hunk range like "42,5" or "42" into (start, count).
/// A missing count means 1 (single-line hunk). A count of 0 means pure deletion.
fn parse_hunk_range(s: &str) -> (u32, u32) {
    if let Some((start_s, count_s)) = s.split_once(',') {
        let start = start_s.parse::<u32>().unwrap_or(1);
        let count = count_s.parse::<u32>().unwrap_or(1);
        (start, count)
    } else {
        let start = s.parse::<u32>().unwrap_or(1);
        (start, 1)
    }
}

/// Get all line numbers in a blob (1..=line_count) as a single LineRange.
fn blob_total_range(cfg: &Config, blob: &str) -> Option<LineRange> {
    let count = count_blob_lines(cfg, blob)?;
    if count == 0 {
        return None;
    }
    Some(LineRange::new(1, count))
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

/// Recency-based fallback when file overlap data is unavailable.
fn filter_by_recency(
    sessions: &[crate::hooks::state::ActiveSession],
) -> Vec<crate::hooks::state::ActiveSession> {
    let now = chrono::Utc::now().timestamp();

    if sessions.is_empty() {
        return Vec::new();
    }

    let mut sorted: Vec<&crate::hooks::state::ActiveSession> = sessions.iter().collect();
    sorted.sort_by_key(|s| std::cmp::Reverse(s.updated_at));

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

/// Fallback session discovery: scan each enabled tool's native storage for
/// recent sessions in this project. Used when `.git/oobo-sessions/` has no
/// matching state files (e.g. hooks fired before `git init`, or hooks weren't
/// installed for this project).
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
                started_at: created_secs,
                updated_at: updated_secs,
            });
        }
    }

    discovered
}

fn ms_to_secs(ms: i64) -> i64 {
    if ms > 1_000_000_000_000 {
        ms / 1000
    } else {
        ms
    }
}

/// Detect files touched by multiple sessions and return both file interactions
/// and the peer map. Accepts references to avoid cloning `ActiveSession` data.
/// Expects only top-level sessions — subagent sessions are filtered out
/// by the caller in `enrich_commit` using `subagent_runs` data.
fn detect_file_interactions_refs(
    sessions: &[&crate::hooks::state::ActiveSession],
    project_root: &str,
) -> (
    Vec<crate::core::anchor::FileInteraction>,
    std::collections::HashMap<String, Vec<String>>,
) {
    let inputs: Vec<crate::core::anchor::SessionFiles> = sessions
        .iter()
        .map(|s| {
            let (edited, read) = if s.edited_files.is_some() || s.read_files.is_some() {
                (
                    s.edited_files
                        .as_ref()
                        .map(|h| h.iter().cloned().collect())
                        .unwrap_or_default(),
                    s.read_files
                        .as_ref()
                        .map(|h| h.iter().cloned().collect())
                        .unwrap_or_default(),
                )
            } else {
                hooks::state::get_file_sets(project_root, &s.session_id)
            };
            crate::core::anchor::SessionFiles {
                session_id: s.session_id.clone(),
                edited,
                read,
            }
        })
        .collect();

    crate::core::anchor::detect_interactions(&inputs)
}

/// Proactively index linked sessions on a background thread so stats are
/// available immediately in `oobo sessions` without waiting for `oobo scan`.
fn index_linked_sessions_bg(links: &[(String, String)], project_root: &str) {
    for (session_id, agent) in links {
        let source = crate::core::tool::normalize_source(agent);
        let state = hooks::state::read_session(project_root, session_id);
        if let Err(e) = crate::commands::index::index_single_session(
            session_id,
            source,
            project_root,
            state.as_ref(),
        ) {
            eprintln!(
                "oobo: warning: could not index session {}: {e}",
                &session_id[..session_id.len().min(8)]
            );
        }
    }
}

/// Match a session's agent name to a tool's canonical name.
/// Handles Cursor's various mode names ("agent", "composer", etc.) which
/// are now normalized at hook time, plus legacy session files with old names.
fn is_agent_tool_match(session_agent: &str, tool_name: &str) -> bool {
    if session_agent == tool_name {
        return true;
    }
    crate::core::tool::is_cursor_agent(session_agent)
        && (tool_name == "composer" || tool_name == "cursor")
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
        let pid = crate::project::id_for_root(project_root);
        let _ = db.ensure_project(&pid, project_root);
        Some(pid)
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
    // Detect Claude JSONL format: entries have both "type" and "message" top-level keys.
    // Checking for both prevents false positives from non-Claude transcripts.
    let is_claude_jsonl = text
        .lines()
        .take(5)
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .any(|v| {
            let ty = v.get("type").and_then(|t| t.as_str());
            matches!(ty, Some("user" | "assistant")) && v.get("message").is_some()
        });

    if is_claude_jsonl {
        return parse_claude_jsonl_transcript(text);
    }

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

            let thinking = parsed
                .get("thinking")
                .and_then(|t| {
                    t.as_str()
                        .map(String::from)
                        .or_else(|| t.get("text").and_then(|v| v.as_str()).map(String::from))
                })
                .filter(|s| !s.is_empty());

            let timestamp_ms = parsed
                .get("timestamp")
                .and_then(|v| v.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis());

            if text.is_empty() && thinking.is_none() {
                return None;
            }
            Some(payload::TranscriptMessage {
                role,
                text: if text.is_empty() { None } else { Some(text) },
                thinking,
                tool_call: None,
                tool_result: None,
                timestamp_ms,
            })
        })
        .collect()
}

/// Delegates to the canonical Claude JSONL parser in `tools::claude::transcript`.
fn parse_claude_jsonl_transcript(text: &str) -> Vec<payload::TranscriptMessage> {
    crate::tools::claude::transcript::parse_rich_transcript_lines(text.lines())
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
///
/// Also collects subagent transcripts from the `subagents/` directory
/// alongside the parent session's transcript.
fn collect_session_transcripts(
    sessions: &[crate::hooks::state::ActiveSession],
    project_root: &str,
) -> Vec<CollectedTranscript> {
    let mut transcripts = Vec::new();

    for session in sessions {
        // 1. Collect the parent session's transcript.
        let mut found_parent = false;

        if is_cursor_session(&session.agent) {
            if let Some(rich) =
                crate::tools::cursor::composer_data::build_rich_transcript(&session.session_id)
            {
                transcripts.push(CollectedTranscript {
                    session_id: session.session_id.clone(),
                    content: rich,
                    parent_session_id: None,
                    subagent_type: None,
                });
                found_parent = true;
            }
        }

        if !found_parent {
            let raw = session
                .transcript_path
                .as_deref()
                .and_then(|tp| std::fs::read_to_string(tp).ok())
                .filter(|c| !c.is_empty());

            if let Some(content) = raw {
                transcripts.push(CollectedTranscript {
                    session_id: session.session_id.clone(),
                    content,
                    parent_session_id: None,
                    subagent_type: None,
                });
                found_parent = true;
            }
        }

        if !found_parent {
            let registry = crate::tools::registry();
            for tool in registry.all() {
                if is_agent_tool_match(&session.agent, tool.name()) {
                    if let Some(path) = tool.find_transcript(project_root, &session.session_id) {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            if !content.is_empty() {
                                transcripts.push(CollectedTranscript {
                                    session_id: session.session_id.clone(),
                                    content,
                                    parent_session_id: None,
                                    subagent_type: None,
                                });
                            }
                        }
                    }
                    break;
                }
            }
        }

        // 2. Always collect subagent transcripts for tools that support them.
        if is_cursor_session(&session.agent) {
            collect_cursor_subagent_transcripts(
                project_root,
                &session.session_id,
                &mut transcripts,
            );
        } else if is_claude_session(&session.agent) {
            collect_claude_subagent_transcripts(
                project_root,
                &session.session_id,
                &mut transcripts,
            );
        }
    }
    transcripts
}

/// Collect transcript files from the subagents/ directory for a Cursor session.
fn collect_cursor_subagent_transcripts(
    project_root: &str,
    parent_session_id: &str,
    transcripts: &mut Vec<CollectedTranscript>,
) {
    let subagents = crate::tools::cursor::transcript::find_subagent_transcripts(
        project_root,
        parent_session_id,
    );
    if subagents.is_empty() {
        return;
    }

    // Build subagent_type lookup from Cursor's session discovery.
    let type_map: std::collections::HashMap<String, String> =
        crate::tools::cursor::sessions_for_project(project_root)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| s.subagent_type.map(|t| (s.session_id, t)))
            .collect();

    for (subagent_id, path) in subagents {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if !content.is_empty() {
                let stype = type_map.get(&subagent_id).cloned();
                transcripts.push(CollectedTranscript {
                    session_id: subagent_id,
                    content,
                    parent_session_id: Some(parent_session_id.to_string()),
                    subagent_type: stype,
                });
            }
        }
    }
}

/// Collect transcript files from the subagents/ directory for a Claude Code session.
fn collect_claude_subagent_transcripts(
    project_root: &str,
    parent_session_id: &str,
    transcripts: &mut Vec<CollectedTranscript>,
) {
    let subagents = crate::tools::claude::transcript::find_subagent_transcripts(
        project_root,
        parent_session_id,
    );
    for (subagent_id, path) in subagents {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if !content.is_empty() {
                let stype = extract_claude_agent_id(&content);
                transcripts.push(CollectedTranscript {
                    session_id: subagent_id,
                    content,
                    parent_session_id: Some(parent_session_id.to_string()),
                    subagent_type: stype,
                });
            }
        }
    }
}

/// Extract `agentId` from the first entry of a Claude subagent JSONL.
fn extract_claude_agent_id(content: &str) -> Option<String> {
    for line in content.lines().take(5) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(id) = entry.get("agentId").and_then(|v| v.as_str()) {
                return Some(id.to_string());
            }
        }
    }
    None
}

fn is_cursor_session(agent: &str) -> bool {
    crate::core::tool::is_cursor_agent(agent)
}

fn is_claude_session(agent: &str) -> bool {
    agent == "claude"
}

/// Extract live session stats directly from the tool's data files at commit
/// time. No prior `oobo index` needed — reads Cursor's state.vscdb, Claude's
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
                model: stats.model,
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

    #[test]
    fn test_filter_by_recency_empty() {
        let result = filter_by_recency(&[]);
        assert!(result.is_empty());
    }

    fn make_session(
        id: &str,
        edited: Vec<&str>,
        read: Vec<&str>,
    ) -> crate::hooks::state::ActiveSession {
        let now = chrono::Utc::now().timestamp();
        crate::hooks::state::ActiveSession {
            session_id: id.into(),
            agent: "claude".into(),
            model: None,
            worktree: None,
            transcript_path: None,
            pre_agent_snapshots: None,
            file_snapshots: None,
            edited_files: if edited.is_empty() {
                None
            } else {
                Some(edited.into_iter().map(|s| s.to_string()).collect())
            },
            read_files: if read.is_empty() {
                None
            } else {
                Some(read.into_iter().map(|s| s.to_string()).collect())
            },
            tool_usage: None,
            tool_failures: None,
            bash_commands: None,
            subagent_runs: None,
            thinking_duration_ms: None,
            compact_count: None,
            started_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn test_detect_file_interactions_refs_uses_active_session_data() {
        let sessions = [
            make_session("s1", vec!["a.rs"], vec![]),
            make_session("s2", vec!["a.rs"], vec![]),
        ];
        let refs: Vec<&_> = sessions.iter().collect();
        let (interactions, peers) = detect_file_interactions_refs(&refs, "/tmp");
        assert_eq!(interactions.len(), 1);
        assert_eq!(interactions[0].path, "a.rs");
        assert_eq!(peers.get("s1").unwrap(), &vec!["s2".to_string()]);
        assert_eq!(peers.get("s2").unwrap(), &vec!["s1".to_string()]);
    }

    #[test]
    fn test_detect_file_interactions_refs_no_overlap() {
        let sessions = [
            make_session("s1", vec!["a.rs"], vec![]),
            make_session("s2", vec!["b.rs"], vec![]),
        ];
        let refs: Vec<&_> = sessions.iter().collect();
        let (interactions, peers) = detect_file_interactions_refs(&refs, "/tmp");
        assert!(interactions.is_empty());
        assert!(peers.is_empty());
    }

    #[test]
    fn test_detect_file_interactions_refs_single_session() {
        let sessions = [make_session("s1", vec!["a.rs"], vec!["b.rs"])];
        let refs: Vec<&_> = sessions.iter().collect();
        let (interactions, _) = detect_file_interactions_refs(&refs, "/tmp");
        assert!(interactions.is_empty());
    }

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

    #[test]
    fn test_parse_transcript_messages_extracts_thinking() {
        let lines = [
            r#"{"role":"assistant","text":"I see the issue.","thinking":{"text":"Let me analyze this carefully...","duration_ms":1500},"timestamp":"2026-01-15T10:00:01Z"}"#,
            r#"{"role":"assistant","text":"Here is my fix.","timestamp":"2026-01-15T10:00:05Z"}"#,
        ];
        let input = lines.join("\n");
        let msgs = parse_transcript_messages(&input);

        assert_eq!(msgs.len(), 2);
        assert_eq!(
            msgs[0].thinking.as_deref(),
            Some("Let me analyze this carefully...")
        );
        assert_eq!(msgs[0].text.as_deref(), Some("I see the issue."));
        assert!(msgs[0].timestamp_ms.is_some());

        assert!(msgs[1].thinking.is_none());
        assert_eq!(msgs[1].text.as_deref(), Some("Here is my fix."));
    }

    #[test]
    fn test_parse_transcript_messages_thinking_only_entry() {
        let input = r#"{"role":"assistant","thinking":{"text":"Internal reasoning step here."}}"#;
        let msgs = parse_transcript_messages(input);

        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].text.is_none());
        assert_eq!(
            msgs[0].thinking.as_deref(),
            Some("Internal reasoning step here.")
        );
    }

    #[test]
    fn test_parse_transcript_messages_thinking_as_plain_string() {
        let input = r#"{"role":"assistant","text":"result","thinking":"plain string thinking"}"#;
        let msgs = parse_transcript_messages(input);

        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].thinking.as_deref(), Some("plain string thinking"));
    }

    // ── Line attribution tests ──────────────────────────────────────────

    #[test]
    fn test_parse_diff_added_ranges_single_hunk() {
        let diff = "\
diff --git a/blob1 b/blob2
--- a/blob1
+++ b/blob2
@@ -1,3 +1,5 @@
 existing line
+new line 1
+new line 2
 another existing
";
        let ranges = parse_diff_added_ranges(diff);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 1);
        assert_eq!(ranges[0].end, 5);
    }

    #[test]
    fn test_parse_diff_added_ranges_multiple_hunks() {
        let diff = "\
@@ -0,0 +1,3 @@
+line1
+line2
+line3
@@ -10,0 +14,2 @@
+inserted1
+inserted2
";
        let ranges = parse_diff_added_ranges(diff);
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0].start, 1);
        assert_eq!(ranges[0].end, 3);
        assert_eq!(ranges[1].start, 14);
        assert_eq!(ranges[1].end, 15);
    }

    #[test]
    fn test_parse_diff_added_ranges_single_line() {
        let diff = "@@ -5,0 +6 @@\n+one new line\n";
        let ranges = parse_diff_added_ranges(diff);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 6);
        assert_eq!(ranges[0].end, 6);
    }

    #[test]
    fn test_parse_diff_added_ranges_deletion_only() {
        let diff = "@@ -1,3 +1,0 @@\n-deleted1\n-deleted2\n-deleted3\n";
        let ranges = parse_diff_added_ranges(diff);
        assert!(ranges.is_empty());
    }

    #[test]
    fn test_parse_diff_added_ranges_empty() {
        assert!(parse_diff_added_ranges("").is_empty());
    }

    #[test]
    fn test_extract_hunk_plus_standard() {
        assert_eq!(extract_hunk_plus("@@ -1,3 +4,5 @@"), Some("4,5"));
    }

    #[test]
    fn test_extract_hunk_plus_no_count() {
        assert_eq!(extract_hunk_plus("@@ -1 +4 @@"), Some("4"));
    }

    #[test]
    fn test_extract_hunk_plus_with_context() {
        assert_eq!(extract_hunk_plus("@@ -1,3 +4,5 @@ fn main()"), Some("4,5"));
    }

    #[test]
    fn test_parse_hunk_range_with_count() {
        assert_eq!(parse_hunk_range("42,5"), (42, 5));
    }

    #[test]
    fn test_parse_hunk_range_no_count() {
        assert_eq!(parse_hunk_range("42"), (42, 1));
    }

    #[test]
    fn test_parse_hunk_range_zero_count() {
        assert_eq!(parse_hunk_range("10,0"), (10, 0));
    }

    #[test]
    fn test_subtract_ranges_no_overlap() {
        let source = vec![
            LineRange { start: 1, end: 5 },
            LineRange { start: 10, end: 15 },
        ];
        let remove = vec![LineRange { start: 6, end: 9 }];
        let result = subtract_ranges(&source, &remove);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], LineRange { start: 1, end: 5 });
        assert_eq!(result[1], LineRange { start: 10, end: 15 });
    }

    #[test]
    fn test_subtract_ranges_full_overlap() {
        let source = vec![LineRange { start: 5, end: 10 }];
        let remove = vec![LineRange { start: 3, end: 12 }];
        let result = subtract_ranges(&source, &remove);
        assert!(result.is_empty());
    }

    #[test]
    fn test_subtract_ranges_partial_overlap() {
        let source = vec![LineRange { start: 1, end: 10 }];
        let remove = vec![LineRange { start: 4, end: 6 }];
        let result = subtract_ranges(&source, &remove);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], LineRange { start: 1, end: 3 });
        assert_eq!(result[1], LineRange { start: 7, end: 10 });
    }

    #[test]
    fn test_subtract_ranges_empty_remove() {
        let source = vec![LineRange { start: 1, end: 5 }];
        let result = subtract_ranges(&source, &[]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], LineRange { start: 1, end: 5 });
    }

    #[test]
    fn test_subtract_ranges_multiple_removals() {
        let source = vec![LineRange { start: 1, end: 20 }];
        let remove = vec![
            LineRange { start: 3, end: 5 },
            LineRange { start: 10, end: 12 },
        ];
        let result = subtract_ranges(&source, &remove);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], LineRange { start: 1, end: 2 });
        assert_eq!(result[1], LineRange { start: 6, end: 9 });
        assert_eq!(result[2], LineRange { start: 13, end: 20 });
    }

    #[test]
    fn test_subtract_ranges_adjacent_no_overlap() {
        let source = vec![LineRange { start: 6, end: 10 }];
        let remove = vec![LineRange { start: 1, end: 5 }];
        let result = subtract_ranges(&source, &remove);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], LineRange { start: 6, end: 10 });
    }

    #[test]
    fn test_subtract_ranges_shared_boundary() {
        let source = vec![LineRange { start: 5, end: 15 }];
        let remove = vec![LineRange { start: 5, end: 10 }];
        let result = subtract_ranges(&source, &remove);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], LineRange { start: 11, end: 15 });
    }

    #[test]
    fn test_subtract_ranges_single_point_exact() {
        let source = vec![LineRange { start: 5, end: 5 }];
        let remove = vec![LineRange { start: 5, end: 5 }];
        let result = subtract_ranges(&source, &remove);
        assert!(result.is_empty());
    }

    #[test]
    fn test_subtract_ranges_remove_tail() {
        let source = vec![LineRange { start: 1, end: 10 }];
        let remove = vec![LineRange { start: 8, end: 10 }];
        let result = subtract_ranges(&source, &remove);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], LineRange { start: 1, end: 7 });
    }
}
