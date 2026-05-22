//! Turn finalization — snapshot current turn state into git-backed storage.

use crate::core::turn::{TurnFileSnapshot, TurnMemoryPayload, TurnSnapshot};
use crate::error::Result;
use crate::hooks::store;

use super::ActiveSession;

pub fn finish_turn(
    project_root: &str,
    session_id: &str,
    agent: &str,
    model: Option<&str>,
    transcript_path: Option<&str>,
) -> Result<Option<String>> {
    let Some(mut state) = store::read(project_root, session_id) else {
        return Ok(None);
    };

    let has_turn_memory = state.current_turn_started_at.is_some()
        || state
            .current_turn_hook_events
            .as_ref()
            .is_some_and(|events| !events.is_empty())
        || state
            .current_turn_tool_calls
            .as_ref()
            .is_some_and(|calls| !calls.is_empty())
        || state
            .edited_files
            .as_ref()
            .is_some_and(|files| !files.is_empty());

    if !has_turn_memory {
        return Ok(None);
    }

    let source = crate::core::tool::normalize_source(agent);
    let worktree_id = crate::git::turns::worktree_id(project_root);
    let project_id = crate::project::id_for_root(project_root);
    let mut snapshot = TurnSnapshot::new(
        &project_id,
        &worktree_id,
        source,
        &state.session_id,
        state.current_turn_index,
    );
    snapshot.parent_id.clone_from(&state.last_turn_snapshot_id);
    snapshot.restored_from = take_restored_from(project_root);
    snapshot.started_at = state.current_turn_started_at;
    snapshot.ended_at = Some(chrono::Utc::now().timestamp());
    snapshot.model = model.map(str::to_string).or_else(|| state.model.clone());
    snapshot.files = turn_files(project_root, &state);

    let transcript = transcript_path
        .map(str::to_string)
        .or_else(|| state.transcript_path.clone());
    snapshot.memory = TurnMemoryPayload {
        transcript_path: transcript.clone(),
        transcript: transcript.as_deref().and_then(load_transcript_payload),
        hook_events: state.current_turn_hook_events.take().unwrap_or_default(),
        tool_calls: state.current_turn_tool_calls.take().unwrap_or_default(),
    };

    let snapshot_id = snapshot.id.clone();
    crate::git::turns::write_turn_snapshot(project_root, snapshot)?;

    state.last_turn_snapshot_id = Some(snapshot_id.clone());
    state.current_turn_started_at = None;
    state.current_turn_hook_events = None;
    state.current_turn_tool_calls = None;
    // Reset per-turn edit chain — each turn gets a fresh chain.
    state.pre_edit_pending = None;
    state.file_edit_chain = None;
    state.bump();
    store::write(project_root, session_id, &state)?;

    Ok(Some(snapshot_id))
}

pub fn mark_restored_from(project_root: &str, id: &str) -> std::io::Result<()> {
    let path = restored_from_marker_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, id)
}

fn take_restored_from(project_root: &str) -> Option<String> {
    let path = restored_from_marker_path(project_root);
    let id = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(path);
    let trimmed = id.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn restored_from_marker_path(project_root: &str) -> std::path::PathBuf {
    crate::git::detect::resolve_git_common_dir(project_root)
        .join("oobo-state")
        .join("restored-from")
}

fn turn_files(project_root: &str, state: &ActiveSession) -> Vec<TurnFileSnapshot> {
    let mut files: Vec<String> = current_turn_file_paths(project_root, state)
        .into_iter()
        .collect();
    if files.is_empty() {
        let Some(session_files) = state.edited_files.as_ref() else {
            return Vec::new();
        };
        files = session_files.iter().cloned().collect();
    }
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let (pre, post) = if let Some(chain) = state
                .file_edit_chain
                .as_ref()
                .and_then(|c| c.get(&path))
                .filter(|pairs| !pairs.is_empty())
            {
                let pre = Some(chain.first().unwrap().pre_blob.clone());
                let post = Some(chain.last().unwrap().post_blob.clone());
                (pre, post)
            } else {
                let pre = state
                    .pre_agent_snapshots
                    .as_ref()
                    .and_then(|snapshots| snapshots.get(&path))
                    .cloned();
                let post = state
                    .file_snapshots
                    .as_ref()
                    .and_then(|snapshots| snapshots.get(&path))
                    .cloned();
                (pre, post)
            };
            TurnFileSnapshot {
                pre_blob: pre,
                post_blob: post,
                path,
            }
        })
        .collect()
}

fn current_turn_file_paths(
    project_root: &str,
    state: &ActiveSession,
) -> std::collections::HashSet<String> {
    let mut files = std::collections::HashSet::new();
    if let Some(calls) = state.current_turn_tool_calls.as_ref() {
        for call in calls {
            if let Some(input) = call.input.as_ref() {
                collect_file_paths_from_value(project_root, input, &mut files);
            }
        }
    }
    if let Some(events) = state.current_turn_hook_events.as_ref() {
        for event in events {
            if let Some(payload) = event.payload.as_ref() {
                collect_file_paths_from_value(project_root, payload, &mut files);
            }
        }
    }
    files
}

fn collect_file_paths_from_value(
    project_root: &str,
    value: &serde_json::Value,
    files: &mut std::collections::HashSet<String>,
) {
    for key in ["file_path", "path"] {
        if let Some(path) = value.get(key).and_then(|v| v.as_str()) {
            push_relative_file(project_root, path, files);
        }
    }
    for key in ["modified_files", "files", "file_paths"] {
        if let Some(items) = value.get(key).and_then(|v| v.as_array()) {
            for item in items {
                if let Some(path) = item.as_str() {
                    push_relative_file(project_root, path, files);
                }
            }
        }
    }
    if let Some(input) = value.get("tool_input") {
        collect_file_paths_from_value(project_root, input, files);
    }
}

fn push_relative_file(
    project_root: &str,
    path: &str,
    files: &mut std::collections::HashSet<String>,
) {
    if path.is_empty() || path.ends_with('/') || path == "." {
        return;
    }
    let p = std::path::Path::new(path);
    let normalized = if p.is_absolute() {
        let root = std::path::Path::new(project_root);
        p.strip_prefix(root)
            .ok()
            .and_then(|rel| rel.to_str())
            .unwrap_or(path)
            .to_string()
    } else {
        path.to_string()
    };
    if !normalized.starts_with('/') && !normalized.starts_with("..") {
        files.insert(normalized);
    }
}

fn load_transcript_payload(path: &str) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text)
        .ok()
        .or(Some(serde_json::Value::String(text)))
}
