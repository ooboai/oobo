//! Shared feed model for the anchor memory view.
//!
//! Both the TUI and CLI agent/json renderers load from this model,
//! eliminating duplicate logic for merging git log + orphan data + turns.

use std::collections::{HashMap, HashSet};

use crate::config::Config;
use crate::core::anchor::Anchor;
use crate::core::turn::TurnSnapshot;

/// Kind of memory row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Anchor,
    Shadow,
}

impl RowKind {
    pub fn agent_label(self) -> &'static str {
        match self {
            RowKind::Anchor => "anchor",
            RowKind::Shadow => "shadow",
        }
    }

    pub fn json_label(self) -> &'static str {
        match self {
            RowKind::Anchor => "anchor",
            RowKind::Shadow => "shadow_anchor",
        }
    }
}

/// A unified row in the memory feed.
#[derive(Debug, Clone)]
pub struct FeedRow {
    pub kind: RowKind,
    pub id: String,
    pub subject: String,
    pub intent: Option<String>,
    pub timestamp: i64,
    pub tool: Option<String>,
    pub tokens: i64,
    pub session_count: usize,
    pub ai_pct: Option<i64>,
    pub files: usize,
    pub tool_calls: usize,
    pub session_id: Option<String>,
    pub turn_index: Option<i64>,
    pub parent_anchor: Option<String>,
    pub restored_from: Option<String>,
    /// Set when this shadow row comes from a different worktree.
    pub worktree_hint: Option<String>,
}

/// Options for loading the feed.
pub struct LoadOptions {
    pub limit: usize,
    pub since: Option<i64>,
    pub tool: Option<String>,
}

/// Load the memory feed for a project.
pub fn load(cfg: &Config, project_root: &str, opts: &LoadOptions) -> Result<Vec<FeedRow>, String> {
    let n = opts.limit.max(1);
    let git_count = n * 4;
    let log = crate::git::proxy::run_git_capture_in(
        cfg,
        &["log", &format!("-{git_count}"), "--format=%H|||%s|||%ct"],
        Some(project_root),
    )
    .unwrap_or_default();

    let (all_anchors, all_links) = crate::git::anchor_cache::load_anchors_cached(project_root);
    let anchor_map: HashMap<String, &Anchor> = all_anchors
        .iter()
        .map(|a| (a.commit_hash.clone(), a))
        .collect();

    let mut out: Vec<FeedRow> = Vec::new();
    for line in log.lines() {
        let parts: Vec<&str> = line.splitn(3, "|||").collect();
        if parts.len() < 3 {
            continue;
        }
        let sha = parts[0].to_string();
        let subject = parts[1].to_string();
        let ts: i64 = parts[2].parse().unwrap_or(0);

        if let Some(s) = opts.since {
            if ts < s {
                continue;
            }
        }

        let (tool, tokens, count) = if anchor_map.contains_key(&sha) {
            let links = all_links.get(&sha).cloned().unwrap_or_default();
            summarize_links(&links)
        } else {
            (None, 0, 0)
        };

        if let Some(t) = opts.tool.as_deref() {
            let want = t.to_lowercase();
            let has = tool.as_deref().map(str::to_lowercase).unwrap_or_default();
            if has != want {
                continue;
            }
        }

        let ai_pct = anchor_map
            .get(&sha)
            .and_then(|a| a.ai_percentage)
            .map(|p| p.round() as i64);

        let intent = anchor_map.get(&sha).and_then(|a| a.intent.clone());

        out.push(FeedRow {
            kind: RowKind::Anchor,
            id: sha,
            subject,
            intent,
            timestamp: ts,
            tool,
            tokens,
            session_count: count,
            ai_pct,
            files: 0,
            tool_calls: 0,
            session_id: None,
            turn_index: None,
            parent_anchor: None,
            restored_from: None,
            worktree_hint: None,
        });
    }

    let parents = build_shadow_parents(&all_anchors);
    let current_wt = crate::git::turns::worktree_id(project_root);
    out.extend(load_shadow_rows(project_root, opts, &parents, &current_wt));
    sort_rows(&mut out);
    out.truncate(opts.limit);
    Ok(out)
}

fn summarize_links(links: &[crate::core::anchor::SessionLink]) -> (Option<String>, i64, usize) {
    if links.is_empty() {
        return (None, 0, 0);
    }
    let tool = Some(links[0].agent.clone());
    let total: i64 = links
        .iter()
        .map(|l| {
            l.input_tokens.unwrap_or(0) as i64
                + l.output_tokens.unwrap_or(0) as i64
                + l.cache_read_tokens.unwrap_or(0) as i64
                + l.cache_creation_tokens.unwrap_or(0) as i64
        })
        .sum();
    (tool, total, links.len())
}

fn load_shadow_rows(
    project_root: &str,
    opts: &LoadOptions,
    parents: &HashMap<String, String>,
    current_wt: &str,
) -> Vec<FeedRow> {
    crate::git::turns::list_turn_snapshots(project_root)
        .into_iter()
        .filter_map(|turn| shadow_to_row(&turn, opts, parents, current_wt))
        .collect()
}

fn shadow_to_row(
    turn: &TurnSnapshot,
    opts: &LoadOptions,
    parents: &HashMap<String, String>,
    current_wt: &str,
) -> Option<FeedRow> {
    // Hide turns that have already been consumed by an anchor.
    if parents.contains_key(&turn.id) {
        return None;
    }

    let ts = turn.ended_at.or(turn.started_at).unwrap_or(turn.created_at);
    if let Some(s) = opts.since {
        if ts < s {
            return None;
        }
    }
    if let Some(t) = opts.tool.as_deref() {
        if !turn.source.eq_ignore_ascii_case(t) {
            return None;
        }
    }
    let worktree_hint = if turn.worktree_id == current_wt {
        None
    } else {
        turn.branch
            .clone()
            .or_else(|| Some(turn.worktree_id[..8.min(turn.worktree_id.len())].to_string()))
    };

    Some(FeedRow {
        kind: RowKind::Shadow,
        id: turn.id.clone(),
        subject: turn_subject(turn),
        intent: None,
        timestamp: ts,
        tool: Some(turn.source.clone()),
        tokens: 0,
        session_count: 1,
        ai_pct: None,
        files: turn_file_count(turn),
        tool_calls: turn.memory.tool_calls.len(),
        session_id: Some(turn.session_id.clone()),
        turn_index: Some(turn.turn_index),
        parent_anchor: parents.get(&turn.id).cloned(),
        restored_from: turn.restored_from.clone(),
        worktree_hint,
    })
}

fn turn_subject(turn: &TurnSnapshot) -> String {
    for event in &turn.memory.hook_events {
        let Some(payload) = event.payload.as_ref() else {
            continue;
        };
        for key in ["prompt", "message", "text", "input"] {
            if let Some(value) = payload.get(key).and_then(|v| v.as_str()) {
                let value = value.lines().next().unwrap_or(value).trim();
                if !value.is_empty() {
                    return value.to_string();
                }
            }
        }
    }
    let sid: String = turn.session_id.chars().take(8).collect();
    format!("session {sid}")
}

fn turn_file_count(turn: &TurnSnapshot) -> usize {
    let mut files = HashSet::new();
    for call in &turn.memory.tool_calls {
        if let Some(input) = call.input.as_ref() {
            collect_file_paths_from_value(input, &mut files);
        }
    }
    for event in &turn.memory.hook_events {
        if let Some(payload) = event.payload.as_ref() {
            collect_file_paths_from_value(payload, &mut files);
        }
    }
    if files.is_empty() {
        turn.files.len()
    } else {
        files.len()
    }
}

fn collect_file_paths_from_value(value: &serde_json::Value, files: &mut HashSet<String>) {
    for key in ["file_path", "path"] {
        if let Some(path) = value.get(key).and_then(|v| v.as_str()) {
            push_counted_file(path, files);
        }
    }
    for key in ["modified_files", "files", "file_paths"] {
        if let Some(items) = value.get(key).and_then(|v| v.as_array()) {
            for item in items {
                if let Some(path) = item.as_str() {
                    push_counted_file(path, files);
                }
            }
        }
    }
    if let Some(input) = value.get("tool_input") {
        collect_file_paths_from_value(input, files);
    }
}

fn push_counted_file(path: &str, files: &mut HashSet<String>) {
    if path.is_empty() || path == "." || path.ends_with('/') {
        return;
    }
    files.insert(path.to_string());
}

fn build_shadow_parents(all_anchors: &[Anchor]) -> HashMap<String, String> {
    let mut parents = HashMap::new();
    for anchor in all_anchors {
        for turn in &anchor.turns {
            parents
                .entry(turn.id.clone())
                .or_insert_with(|| anchor.commit_hash.clone());
        }
    }
    parents
}

fn sort_rows(rows: &mut [FeedRow]) {
    rows.sort_by(|a, b| {
        if a.parent_anchor.as_deref() == Some(b.id.as_str()) {
            return std::cmp::Ordering::Greater;
        }
        if b.parent_anchor.as_deref() == Some(a.id.as_str()) {
            return std::cmp::Ordering::Less;
        }
        if a.parent_anchor.is_some() && a.parent_anchor == b.parent_anchor {
            return a
                .turn_index
                .cmp(&b.turn_index)
                .then_with(|| a.id.cmp(&b.id));
        }
        b.timestamp.cmp(&a.timestamp).then_with(|| b.id.cmp(&a.id))
    });
}
