use crate::config::Config;
use crate::core::anchor::{AuthorType, FileAttribution, FileChange, LineAttribution};
use crate::hooks;

use super::anchor_builder::AttributionResult;
use super::context::GitContext;
use super::line_attribution::{compute_mixed_line_attrs, compute_pure_ai_line_attrs};
use super::proxy;

pub(super) type FileStat = (String, u32, u32);

type PostAgentLookup<'a> = std::collections::HashMap<&'a str, (String, Option<String>)>;
type PreAgentLookup<'a> = std::collections::HashMap<&'a str, String>;
type EditChainLookup<'a> = std::collections::HashMap<&'a str, Vec<hooks::state::FileEditPair>>;

pub(super) fn resolve_file_attribution(
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
    let edit_chain_lookup = build_edit_chain_lookup(active_sessions);

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
            compute_precise_attribution(cfg, &FileAttributionInput {
                file_path: path,
                agent_blob,
                pre_agent_blob: pre_blob.as_deref(),
                total_added: *added,
                total_deleted: *deleted,
                agent_name: agent_name.clone(),
                edit_chain: edit_chain_lookup.get(path.as_str()),
            })
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
        let pct = (f64::from(ai_added + ai_deleted) / f64::from(total_lines)) * 100.0;
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

/// Build a lookup of per-file edit chains from all active sessions.
fn build_edit_chain_lookup<'a>(
    sessions: &'a [hooks::state::ActiveSession],
) -> EditChainLookup<'a> {
    let mut lookup: EditChainLookup<'a> = std::collections::HashMap::new();
    for session in sessions {
        if let Some(ref chain) = session.file_edit_chain {
            for (file, pairs) in chain {
                lookup
                    .entry(file.as_str())
                    .or_default()
                    .extend(pairs.iter().cloned());
            }
        }
    }
    for pairs in lookup.values_mut() {
        pairs.sort_by_key(|p| p.timestamp);
    }
    lookup
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
/// When `edit_chain` is available (from preToolUse/postToolUse pairs), uses
/// per-edit diffs for more precise line attribution instead of a single
/// pre_agent -> agent_blob diff.
struct FileAttributionInput<'a> {
    file_path: &'a str,
    agent_blob: &'a str,
    pre_agent_blob: Option<&'a str>,
    total_added: u32,
    total_deleted: u32,
    agent_name: Option<String>,
    edit_chain: Option<&'a Vec<hooks::state::FileEditPair>>,
}

fn compute_precise_attribution(
    cfg: &Config,
    input: &FileAttributionInput<'_>,
) -> (
    u32,
    u32,
    u32,
    u32,
    Option<FileAttribution>,
    Option<String>,
    Vec<LineAttribution>,
) {
    let file_path = input.file_path;
    let agent_blob = input.agent_blob;
    let pre_agent_blob = input.pre_agent_blob;
    let total_added = input.total_added;
    let total_deleted = input.total_deleted;
    let agent_name = &input.agent_name;
    let edit_chain = input.edit_chain;

    let committed_blob = proxy::run_git_capture(cfg, &["rev-parse", &format!("HEAD:{file_path}")])
        .unwrap_or_default()
        .trim()
        .to_string();

    let baseline_blob = if let Some(pre) = pre_agent_blob {
        pre.to_string()
    } else {
        proxy::run_git_capture(cfg, &["rev-parse", &format!("HEAD~1:{file_path}")])
            .unwrap_or_default()
            .trim()
            .to_string()
    };

    if !committed_blob.is_empty() && agent_blob == committed_blob {
        let line_attrs =
            compute_pure_ai_line_attrs(cfg, &baseline_blob, &committed_blob, agent_name.as_deref());
        return (
            total_added,
            total_deleted,
            0,
            0,
            Some(FileAttribution::Ai),
            agent_name.clone(),
            line_attrs,
        );
    }

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

    let ai_stats = if baseline_blob.is_empty() {
        count_blob_lines(cfg, agent_blob).map(|n| (n, 0))
    } else {
        diff_blobs_numstat(cfg, &baseline_blob, agent_blob)
    };
    let human_stats = diff_blobs_numstat(cfg, agent_blob, &committed_blob);

    let (ai_add, ai_del) = ai_stats.unwrap_or((total_added, total_deleted));
    let (human_add, human_del) = human_stats.unwrap_or((0, 0));

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

    let line_attrs = if let Some(chain) = edit_chain.filter(|c| c.len() >= 2) {
        super::line_attribution::compute_chain_line_attrs(
            cfg,
            chain,
            &baseline_blob,
            &committed_blob,
            agent_name.as_deref(),
        )
    } else {
        compute_mixed_line_attrs(
            cfg,
            &baseline_blob,
            agent_blob,
            &committed_blob,
            agent_name.as_deref(),
        )
    };

    (
        ai_add,
        ai_del,
        human_add,
        human_del,
        attribution,
        agent_name.clone(),
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
